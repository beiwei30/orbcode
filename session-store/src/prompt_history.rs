use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use chrono::Utc;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use crate::SessionStoreError;

/// Cap on how many history entries the picker can scan in one pass.
/// Matches `MAX_HISTORY_ITEMS` in `claude-code/src/history.ts`.
pub const MAX_HISTORY_ITEMS: usize = 100;

/// Environment variable that turns prompt-history writes into no-ops.
/// Matches `claude-code/src/history.ts:addToHistory`.
const SKIP_ENV_VAR: &str = "CLAUDE_CODE_SKIP_PROMPT_HISTORY";

#[derive(Clone)]
pub struct PromptHistoryStore {
    home_dir: PathBuf,
    history_path: PathBuf,
    cwd: PathBuf,
    /// Tracks the last `(session_id, timestamp_ms, display)` we appended in this
    /// process. Used by `remove_last` to undo the most recent append: fast-path
    /// in-memory removal is impossible because we already flushed to disk, so
    /// we record a skip-tuple and filter it out of `load_recent_for_session`.
    skip_state: Arc<Mutex<SkipState>>,
}

#[derive(Default)]
struct SkipState {
    last_appended: Option<AppendedEntry>,
    skipped: HashSet<SkippedKey>,
}

#[derive(Clone)]
struct AppendedEntry {
    session_id: String,
    timestamp_ms: i64,
    display: String,
}

#[derive(Clone, Hash, Eq, PartialEq)]
struct SkippedKey {
    session_id: String,
    timestamp_ms: i64,
    display: String,
}

impl PromptHistoryStore {
    pub fn new(home_dir: PathBuf, history_path: PathBuf, cwd: PathBuf) -> Self {
        Self {
            home_dir,
            history_path,
            cwd,
            skip_state: Arc::new(Mutex::new(SkipState::default())),
        }
    }

    pub async fn load_recent(&self, limit: usize) -> Result<Vec<String>, SessionStoreError> {
        self.load_recent_for_session(None, limit).await
    }

    /// Newest-first project-scoped history with the current session's entries
    /// emitted ahead of other sessions, mirroring `getHistory` in
    /// `claude-code/src/history.ts`. The combined output is bounded by
    /// `min(limit, MAX_HISTORY_ITEMS)`.
    pub async fn load_recent_for_session(
        &self,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, SessionStoreError> {
        let limit = limit.min(MAX_HISTORY_ITEMS);
        if limit == 0 {
            return Ok(Vec::new());
        }
        if !tokio::fs::try_exists(&self.history_path).await? {
            return Ok(Vec::new());
        }

        let project = self.cwd.display().to_string();
        let contents = tokio::fs::read_to_string(&self.history_path).await?;
        let skipped = self
            .skip_state
            .lock()
            .expect("skip state lock")
            .skipped
            .clone();
        let mut seen: HashSet<String> = HashSet::new();
        let mut current = Vec::new();
        let mut other = Vec::new();

        for line in contents.lines().rev() {
            let Ok(parsed) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(display) = parsed.get("display").and_then(Value::as_str) else {
                continue;
            };
            let entry_project = parsed
                .get("project")
                .and_then(Value::as_str)
                .or_else(|| parsed.get("cwd").and_then(Value::as_str));
            if entry_project != Some(project.as_str()) {
                continue;
            }
            let entry_session = parsed.get("sessionId").and_then(Value::as_str);
            let timestamp_ms = parsed
                .get("timestamp")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            if let Some(sid) = entry_session
                && skipped.contains(&SkippedKey {
                    session_id: sid.to_string(),
                    timestamp_ms,
                    display: display.to_string(),
                })
            {
                continue;
            }
            if !seen.insert(display.to_string()) {
                continue;
            }
            let belongs_to_current = session_id.is_some() && session_id == entry_session;
            if belongs_to_current {
                current.push(display.to_string());
            } else {
                other.push(display.to_string());
            }
            if current.len() + other.len() >= limit {
                break;
            }
        }

        let mut entries = current;
        entries.extend(other);
        entries.truncate(limit);
        Ok(entries)
    }

    pub async fn append(&self, session_id: &str, prompt: &str) -> Result<(), SessionStoreError> {
        if env_truthy(SKIP_ENV_VAR) {
            return Ok(());
        }
        tokio::fs::create_dir_all(&self.home_dir).await?;
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&self.history_path).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            tokio::fs::set_permissions(&self.history_path, perms).await?;
        }
        let project = self.cwd.display().to_string();
        let timestamp_ms = Utc::now().timestamp_millis();
        let entry = json!({
            "display": prompt,
            "timestamp": timestamp_ms,
            "cwd": project,
            "project": project,
            "sessionId": session_id,
        });
        file.write_all(serde_json::to_string(&entry)?.as_bytes())
            .await?;
        file.write_all(b"\n").await?;
        // Flush so the entry survives a crash/quit — the tokio file buffers
        // writes, and the transcript path flushes for exactly this reason.
        file.flush().await?;

        let mut state = self.skip_state.lock().expect("skip state lock");
        state.last_appended = Some(AppendedEntry {
            session_id: session_id.to_string(),
            timestamp_ms,
            display: prompt.to_string(),
        });
        Ok(())
    }

    /// Mark the most recent append (in this process) as removed without
    /// rewriting the file. The entry stays on disk but is filtered out of
    /// subsequent `load_recent_for_session` calls — used when a turn is
    /// auto-restored on interrupt before any response arrived. One-shot:
    /// repeated calls without an intervening `append` are no-ops.
    pub fn remove_last(&self) {
        let mut state = self.skip_state.lock().expect("skip state lock");
        if let Some(entry) = state.last_appended.take() {
            state.skipped.insert(SkippedKey {
                session_id: entry.session_id,
                timestamp_ms: entry.timestamp_ms,
                display: entry.display,
            });
        }
    }

    /// Reset both the last-appended pointer and the skip-set. Mirrors
    /// `clearPendingHistoryEntries` from history.ts — used by tests and any
    /// future session-clear path that wants to forget process-local state.
    pub fn clear_pending(&self) {
        let mut state = self.skip_state.lock().expect("skip state lock");
        state.last_appended = None;
        state.skipped.clear();
    }
}

fn env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

#[cfg(test)]
// The guard below is a std Mutex held across .await purely to serialize tests
// that mutate process-wide env vars; nothing inside the guarded section is
// async-safe relative to the data the lock protects.
#[allow(clippy::await_holding_lock)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    /// Cargo runs tests in parallel inside one process. Any test that mutates
    /// the SKIP_ENV_VAR must hold this lock so other tests don't observe the
    /// truthy value mid-run and short-circuit their own appends.
    static ENV_GUARD: StdMutex<()> = StdMutex::new(());

    async fn make_store() -> (tempfile::TempDir, PromptHistoryStore, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().expect("create temp dir");
        let home_dir = temp.path().join("home");
        let cwd = temp.path().join("project");
        let history_path = home_dir.join("history.jsonl");
        tokio::fs::create_dir_all(&home_dir)
            .await
            .expect("create home dir");
        tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
        let store = PromptHistoryStore::new(home_dir.clone(), history_path.clone(), cwd.clone());
        (temp, store, history_path, cwd)
    }

    #[tokio::test]
    async fn load_recent_filters_project_and_deduplicates_newest_first() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let home_dir = temp.path().join("home");
        let cwd = temp.path().join("project");
        let other_cwd = temp.path().join("other");
        let history_path = home_dir.join("history.jsonl");
        tokio::fs::create_dir_all(&home_dir)
            .await
            .expect("create home dir");
        tokio::fs::write(
            &history_path,
            format!(
                "{}\n{}\n{}\n{}\nnot-json\n",
                json!({ "display": "older", "project": cwd.display().to_string() }),
                json!({ "display": "skip-other", "project": other_cwd.display().to_string() }),
                json!({ "display": "older", "project": cwd.display().to_string() }),
                json!({ "display": "newer", "cwd": cwd.display().to_string() })
            ),
        )
        .await
        .expect("write history");

        let store = PromptHistoryStore::new(home_dir, history_path, cwd);

        assert_eq!(
            store.load_recent(5).await.expect("load history"),
            vec!["newer".to_string(), "older".to_string()]
        );
    }

    #[tokio::test]
    async fn load_recent_caps_at_max_history_items() {
        let (_temp, store, history_path, cwd) = make_store().await;
        let project = cwd.display().to_string();
        let mut lines = String::new();
        for index in 0..(MAX_HISTORY_ITEMS + 25) {
            let entry = json!({
                "display": format!("entry-{index}"),
                "project": project,
                "sessionId": "session-a",
                "timestamp": index as i64,
            });
            lines.push_str(&serde_json::to_string(&entry).expect("serialize"));
            lines.push('\n');
        }
        tokio::fs::write(&history_path, lines)
            .await
            .expect("write history");

        let history = store.load_recent(usize::MAX).await.expect("load history");
        assert_eq!(history.len(), MAX_HISTORY_ITEMS);
        assert_eq!(
            history.first().expect("first entry"),
            &format!("entry-{}", MAX_HISTORY_ITEMS + 24)
        );
    }

    #[tokio::test]
    async fn load_recent_orders_current_session_first() {
        let (_temp, store, history_path, cwd) = make_store().await;
        let project = cwd.display().to_string();
        let lines = [
            json!({ "display": "session-a-old", "project": project, "sessionId": "session-a", "timestamp": 1 }),
            json!({ "display": "session-b-mid", "project": project, "sessionId": "session-b", "timestamp": 2 }),
            json!({ "display": "session-a-new", "project": project, "sessionId": "session-a", "timestamp": 3 }),
            json!({ "display": "session-b-new", "project": project, "sessionId": "session-b", "timestamp": 4 }),
        ]
        .map(|v| serde_json::to_string(&v).expect("serialize"))
        .join("\n");
        tokio::fs::write(&history_path, format!("{lines}\n"))
            .await
            .expect("write history");

        let ordered = store
            .load_recent_for_session(Some("session-a"), 10)
            .await
            .expect("load history");
        assert_eq!(
            ordered,
            vec![
                "session-a-new".to_string(),
                "session-a-old".to_string(),
                "session-b-new".to_string(),
                "session-b-mid".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn append_persists_entry_and_skip_env_short_circuits() {
        let _guard = ENV_GUARD.lock().expect("env guard");
        let (_temp, store, _history_path, _cwd) = make_store().await;
        struct Restore(&'static str, Option<String>);
        impl Drop for Restore {
            fn drop(&mut self) {
                if let Some(value) = self.1.take() {
                    unsafe { std::env::set_var(self.0, value) };
                } else {
                    unsafe { std::env::remove_var(self.0) };
                }
            }
        }
        let _restore = Restore(SKIP_ENV_VAR, std::env::var(SKIP_ENV_VAR).ok());
        unsafe { std::env::remove_var(SKIP_ENV_VAR) };

        store
            .append("session-x", "hello")
            .await
            .expect("append entry");
        let history = store.load_recent(5).await.expect("load history");
        assert_eq!(history, vec!["hello".to_string()]);

        unsafe { std::env::set_var(SKIP_ENV_VAR, "1") };
        store
            .append("session-x", "skipped")
            .await
            .expect("append while skipped");
        let history = store.load_recent(5).await.expect("load history");
        assert_eq!(history, vec!["hello".to_string()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn append_sets_owner_only_permissions() {
        let _guard = ENV_GUARD.lock().expect("env guard");
        use std::os::unix::fs::PermissionsExt;
        let (_temp, store, history_path, _cwd) = make_store().await;
        store
            .append("session-y", "perm-check")
            .await
            .expect("append entry");
        let metadata = tokio::fs::metadata(&history_path)
            .await
            .expect("history metadata");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }

    #[tokio::test]
    async fn remove_last_filters_most_recent_entry_for_session() {
        let _guard = ENV_GUARD.lock().expect("env guard");
        let (_temp, store, _history_path, _cwd) = make_store().await;
        store
            .append("session-z", "first")
            .await
            .expect("append first");
        store
            .append("session-z", "second")
            .await
            .expect("append second");
        store.remove_last();

        let history = store
            .load_recent_for_session(Some("session-z"), 5)
            .await
            .expect("load history");
        assert_eq!(history, vec!["first".to_string()]);

        // remove_last is one-shot: a second call with no append is a no-op
        store.remove_last();
        let history = store
            .load_recent_for_session(Some("session-z"), 5)
            .await
            .expect("load history");
        assert_eq!(history, vec!["first".to_string()]);
    }
}
