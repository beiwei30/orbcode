mod gc;
mod listing;
mod persistence;
pub(crate) mod session_index;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use orbcode_protocol::ProviderId;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::SessionStoreError;

pub use gc::{GcResult, SessionStorageHealth};

#[derive(Clone, Debug, Default)]
pub struct SessionWriteHints {
    pub git_branch: Option<String>,
    pub provider: Option<ProviderId>,
}

#[derive(Clone)]
pub struct TranscriptFileStore {
    pub(crate) projects_dir: PathBuf,
    pub(crate) current_project_dir: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) anthropic_model: String,
    /// Per-session append serialization. The map is guarded so concurrent
    /// sessions can write in parallel while a single session's appends stay
    /// strictly ordered — the prior global lock made every transcript fight
    /// the same mutex even when they touched different files.
    pub(crate) write_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    pub(crate) hints: Arc<Mutex<HashMap<String, SessionWriteHints>>>,
    pub(crate) session_paths: Arc<RwLock<HashMap<String, PathBuf>>>,
    pub(crate) session_cwds: Arc<RwLock<HashMap<String, PathBuf>>>,
}

impl TranscriptFileStore {
    pub fn new(current_project_dir: PathBuf, cwd: PathBuf, anthropic_model: String) -> Self {
        let projects_dir = current_project_dir
            .parent()
            .map_or_else(|| current_project_dir.clone(), PathBuf::from);
        Self {
            projects_dir,
            current_project_dir,
            cwd,
            anthropic_model,
            write_locks: Arc::new(Mutex::new(HashMap::new())),
            hints: Arc::new(Mutex::new(HashMap::new())),
            session_paths: Arc::new(RwLock::new(HashMap::new())),
            session_cwds: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) async fn session_write_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut guard = self.write_locks.lock().await;
        guard
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn record_session_hints(&self, session_id: &str, hints: SessionWriteHints) {
        let mut guard = self.hints.lock().await;
        guard.insert(session_id.to_string(), hints);
    }

    pub(crate) async fn hints_for(&self, session_id: &str) -> SessionWriteHints {
        self.hints
            .lock()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn decorate_entry_with_hints(entry: &mut Value, hints: &SessionWriteHints) {
        let Some(object) = entry.as_object_mut() else {
            return;
        };
        if let Some(branch) = hints.git_branch.as_deref()
            && !branch.is_empty()
            && !object.contains_key("gitBranch")
        {
            object.insert("gitBranch".to_string(), Value::String(branch.to_string()));
        }
        if let Some(provider) = hints.provider
            && !object.contains_key("provider")
        {
            object.insert(
                "provider".to_string(),
                Value::String(provider.as_str().to_string()),
            );
        }
    }

    pub fn path(&self, session_id: &str) -> PathBuf {
        if let Some(path) = self.session_path_hint(session_id) {
            return path;
        }
        self.current_project_dir.join(format!("{session_id}.jsonl"))
    }

    pub(crate) fn session_path_hint(&self, session_id: &str) -> Option<PathBuf> {
        match self.session_paths.read() {
            Ok(guard) => guard.get(session_id).cloned(),
            Err(poisoned) => poisoned.into_inner().get(session_id).cloned(),
        }
    }

    pub(crate) fn record_session_path(&self, session_id: &str, path: &Path) {
        let mut guard = match self.session_paths.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.insert(session_id.to_string(), path.to_path_buf());
    }

    pub(crate) fn cwd_for(&self, session_id: &str) -> PathBuf {
        match self.session_cwds.read() {
            Ok(guard) => guard
                .get(session_id)
                .cloned()
                .unwrap_or_else(|| self.cwd.clone()),
            Err(poisoned) => poisoned
                .into_inner()
                .get(session_id)
                .cloned()
                .unwrap_or_else(|| self.cwd.clone()),
        }
    }

    pub(crate) fn recorded_cwd_for(&self, session_id: &str) -> Option<PathBuf> {
        match self.session_cwds.read() {
            Ok(guard) => guard.get(session_id).cloned(),
            Err(poisoned) => poisoned.into_inner().get(session_id).cloned(),
        }
    }

    pub(crate) fn record_session_cwd(&self, session_id: &str, cwd: Option<&str>) {
        let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) else {
            return;
        };
        let mut guard = match self.session_cwds.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.insert(session_id.to_string(), PathBuf::from(cwd));
    }

    pub fn record_session_location(&self, session_id: &str, path: &Path, cwd: &Path) {
        self.record_session_path(session_id, path);
        let mut guard = match self.session_cwds.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.insert(session_id.to_string(), cwd.to_path_buf());
    }

    pub async fn session_file_exists(&self, session_id: &str) -> Result<bool, SessionStoreError> {
        Ok(tokio::fs::try_exists(self.path(session_id)).await?)
    }
}

pub(crate) fn sort_sessions_newest_first(sessions: &mut [orbcode_protocol::SessionRecord]) {
    sessions.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
}

pub(crate) fn deduplicate_sessions_by_id(
    mut sessions: Vec<orbcode_protocol::SessionRecord>,
) -> Vec<orbcode_protocol::SessionRecord> {
    sort_sessions_newest_first(&mut sessions);
    let mut deduped = Vec::new();
    for session in sessions {
        if !deduped
            .iter()
            .any(|existing: &orbcode_protocol::SessionRecord| {
                existing.session_id == session.session_id
            })
        {
            deduped.push(session);
        }
    }
    deduped
}

pub(crate) fn deduplicate_summaries_by_id(
    mut summaries: Vec<orbcode_protocol::SessionSummary>,
) -> Vec<orbcode_protocol::SessionSummary> {
    summaries.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    let mut deduped = Vec::new();
    for summary in summaries {
        if !deduped
            .iter()
            .any(|existing: &orbcode_protocol::SessionSummary| {
                existing.session_id == summary.session_id
            })
        {
            deduped.push(summary);
        }
    }
    deduped
}

pub(crate) fn project_prefix_index(prefixes: &[String]) -> Vec<String> {
    let mut indexed = prefixes
        .iter()
        .filter(|prefix| !prefix.is_empty())
        .map(|prefix| {
            if cfg!(windows) {
                prefix.to_lowercase()
            } else {
                prefix.clone()
            }
        })
        .collect::<Vec<_>>();
    indexed.sort_by_key(|prefix| std::cmp::Reverse(prefix.len()));
    indexed.dedup();
    indexed
}

pub(crate) fn project_dir_matches_any_prefix(dir_name: &str, prefixes: &[String]) -> bool {
    let dir_name = if cfg!(windows) {
        dir_name.to_lowercase()
    } else {
        dir_name.to_string()
    };
    prefixes.iter().any(|prefix| {
        dir_name == *prefix
            || dir_name
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('-'))
    })
}

pub(crate) fn short_corrupt_reason(error: &SessionStoreError) -> String {
    match error {
        SessionStoreError::SessionNotFound(_) => "transcript empty or unreadable".to_string(),
        SessionStoreError::Io(io_error) => format!("io: {io_error}"),
        SessionStoreError::TranscriptIo {
            operation, source, ..
        } => format!("io {operation}: {source}"),
        SessionStoreError::Json(json_error) => format!("json: {json_error}"),
        SessionStoreError::Config(message) => message.clone(),
    }
}
