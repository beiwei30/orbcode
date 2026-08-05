use std::path::{Path, PathBuf};

use chrono::Utc;
use orbcode_protocol::{EffortLevel, SessionRecord, TranscriptMessage};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::TranscriptFileStore;
use crate::{
    SessionStoreError,
    entries::{progress_transcript_entry, transcript_entries},
    transcript::{
        CUSTOM_TITLE_ENTRY_TYPE, SESSION_CONTEXT_ENTRY_TYPE, TRANSCRIPT_ENTRYPOINT,
        TRANSCRIPT_VERSION,
    },
};

impl TranscriptFileStore {
    pub async fn append_custom_title_line(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<(), SessionStoreError> {
        let cwd = self.cwd_for(session_id);
        let entry = json!({
            "type": CUSTOM_TITLE_ENTRY_TYPE,
            "customTitle": title,
            "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "sessionId": session_id,
            "cwd": cwd.display().to_string(),
            "entrypoint": TRANSCRIPT_ENTRYPOINT,
            "version": TRANSCRIPT_VERSION,
        });
        self.append_entries(session_id, vec![entry]).await
    }

    pub async fn append_session_context_line(
        &self,
        session_id: &str,
        additional_directories: &[PathBuf],
        session_allowed_tools: &[String],
        session_disallowed_tools: &[String],
        session_effort: Option<EffortLevel>,
    ) -> Result<(), SessionStoreError> {
        let cwd = self.cwd_for(session_id);
        let entry = json!({
            "type": SESSION_CONTEXT_ENTRY_TYPE,
            "additionalDirectories": additional_directories
                .iter()
                .map(|directory| directory.display().to_string())
                .collect::<Vec<_>>(),
            "sessionPermissions": {
                "allow": session_allowed_tools,
                "deny": session_disallowed_tools,
            },
            "sessionEffort": session_effort.map(EffortLevel::as_str),
            "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "sessionId": session_id,
            "cwd": cwd.display().to_string(),
            "entrypoint": TRANSCRIPT_ENTRYPOINT,
            "version": TRANSCRIPT_VERSION,
        });
        self.append_entries(session_id, vec![entry]).await
    }

    pub async fn remove_session_file_if_exists(
        &self,
        session_id: &str,
    ) -> Result<(), SessionStoreError> {
        match tokio::fs::remove_file(self.path(session_id)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn remove_last_user_prompt_if_matches(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<(), SessionStoreError> {
        let Some(mut session) = self.load_session_if_present(session_id).await? else {
            return Ok(());
        };
        let should_remove = session.messages.last().is_some_and(|message| {
            matches!(message.role, orbcode_protocol::MessageRole::User) && message.content == prompt
        });
        if !should_remove {
            return Ok(());
        }
        session.messages.pop();
        if session.messages.is_empty() {
            self.remove_session_file_if_exists(session_id).await?;
            return Ok(());
        }
        self.persist_full_session(&session).await
    }

    pub async fn append_progress_line(
        &self,
        session_id: &str,
        parent_uuid: Option<&str>,
        progress: Value,
    ) -> Result<(), SessionStoreError> {
        let timestamp = progress
            .get("timestamp")
            .and_then(Value::as_str)
            .map_or_else(
                || Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                str::to_string,
            );
        let cwd = self.cwd_for(session_id);
        let entry = progress_transcript_entry(&cwd, session_id, parent_uuid, &timestamp, progress);
        self.append_entries(session_id, vec![entry]).await
    }

    pub async fn append_progress_for_latest_parent(
        &self,
        session_id: &str,
        progress: Value,
    ) -> Result<(), SessionStoreError> {
        let parent_uuid = self
            .load_session_if_present(session_id)
            .await?
            .and_then(|session| session.messages.last().map(|message| message.id.clone()));
        self.append_progress_line(session_id, parent_uuid.as_deref(), progress)
            .await
    }

    /// Append a progress line anchored to the message that emitted the tool
    /// with `tool_use_id`, rather than to whatever message is currently newest.
    ///
    /// A detached background agent runs concurrently with its parent session:
    /// by the time it emits progress, the parent's *latest* message can be an
    /// unrelated later entry, so anchoring on "latest" misattributes the
    /// progress. Falls back to the latest message when the tool_use cannot be
    /// located (e.g. not yet persisted).
    pub async fn append_progress_for_tool_use_parent(
        &self,
        session_id: &str,
        tool_use_id: &str,
        progress: Value,
    ) -> Result<(), SessionStoreError> {
        let session = self.load_session_if_present(session_id).await?;
        let parent_uuid = session.and_then(|session| {
            session
                .messages
                .iter()
                .rev()
                .find(|message| {
                    message.blocks.iter().any(|block| {
                        matches!(
                            block,
                            orbcode_protocol::TranscriptBlock::ToolUse { id, .. } if id == tool_use_id
                        )
                    })
                })
                .or_else(|| session.messages.last())
                .map(|message| message.id.clone())
        });
        self.append_progress_line(session_id, parent_uuid.as_deref(), progress)
            .await
    }

    pub async fn append_progress_for_latest_parent_if_present(
        &self,
        session_id: &str,
        progress: Value,
    ) -> Result<(), SessionStoreError> {
        if !self.session_file_exists(session_id).await.unwrap_or(false) {
            return Ok(());
        }
        let parent_uuid = self
            .load_session_if_present(session_id)
            .await
            .ok()
            .flatten()
            .and_then(|session| session.messages.last().map(|message| message.id.clone()));
        self.append_progress_line(session_id, parent_uuid.as_deref(), progress)
            .await
    }

    pub async fn append_message_line(
        &self,
        session_id: &str,
        message: &TranscriptMessage,
        parent_uuid: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        let decorate_with_hints = message.transcript_provenance.is_none();
        self.append_entries_with_hint_policy(
            session_id,
            transcript_entries(
                &self.cwd_for(session_id),
                &self.anthropic_model,
                session_id,
                message,
                parent_uuid,
            ),
            decorate_with_hints,
        )
        .await
    }

    pub async fn persist_full_session(
        &self,
        session: &SessionRecord,
    ) -> Result<(), SessionStoreError> {
        let hints = self.hints_for(&session.session_id).await;
        let cwd = session
            .cwd
            .as_deref()
            .filter(|cwd| !cwd.is_empty())
            .map_or_else(|| self.cwd_for(&session.session_id), PathBuf::from);
        let mut lines = Vec::new();
        let mut parent_uuid: Option<String> = None;
        if let Some(custom_title) = session.custom_title.as_deref() {
            let trimmed = custom_title.trim();
            if !trimmed.is_empty() {
                let entry = json!({
                    "type": CUSTOM_TITLE_ENTRY_TYPE,
                    "customTitle": trimmed,
                    "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    "sessionId": session.session_id,
                    "cwd": cwd.display().to_string(),
                    "entrypoint": TRANSCRIPT_ENTRYPOINT,
                    "version": TRANSCRIPT_VERSION,
                });
                lines.push(serde_json::to_string(&entry)?);
            }
        }
        if !session.additional_directories.is_empty()
            || !session.session_allowed_tools.is_empty()
            || !session.session_disallowed_tools.is_empty()
            || session.session_effort.is_some()
        {
            let entry = json!({
                "type": SESSION_CONTEXT_ENTRY_TYPE,
                "additionalDirectories": session.additional_directories.clone(),
                "sessionPermissions": {
                    "allow": session.session_allowed_tools.clone(),
                    "deny": session.session_disallowed_tools.clone(),
                },
                "sessionEffort": session.session_effort.map(EffortLevel::as_str),
                "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "sessionId": session.session_id,
                "cwd": cwd.display().to_string(),
                "entrypoint": TRANSCRIPT_ENTRYPOINT,
                "version": TRANSCRIPT_VERSION,
            });
            lines.push(serde_json::to_string(&entry)?);
        }
        for message in &session.messages {
            for mut entry in transcript_entries(
                &cwd,
                &self.anthropic_model,
                &session.session_id,
                message,
                parent_uuid.as_deref(),
            ) {
                if message.transcript_provenance.is_none() {
                    Self::decorate_entry_with_hints(&mut entry, &hints);
                }
                lines.push(serde_json::to_string(&entry)?);
            }
            parent_uuid = Some(message.id.clone());
        }
        let mut payload = lines.join("\n");
        if !payload.is_empty() {
            payload.push('\n');
        }

        let lock = self.session_write_lock(&session.session_id).await;
        let _write_guard = lock.lock().await;
        let path = self.path(&session.session_id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                SessionStoreError::transcript_io("create project dir", parent, error)
            })?;
        }
        let tmp_path = path.with_extension(format!("jsonl.{}.tmp", Uuid::new_v4()));
        // If the tmp write or atomic rename fails partway, leaving the
        // sentinel file behind would slowly fill the project dir and
        // confuse later session-summary scans. Best-effort remove on any
        // failure path before propagating the original error.
        if let Err(error) = tokio::fs::write(&tmp_path, payload).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(SessionStoreError::transcript_io(
                "write transcript",
                &tmp_path,
                error,
            ));
        }
        if let Err(error) = tokio::fs::rename(&tmp_path, &path).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(SessionStoreError::transcript_io(
                "rename transcript",
                &path,
                error,
            ));
        }
        Ok(())
    }

    pub(crate) async fn append_entries(
        &self,
        session_id: &str,
        entries: Vec<Value>,
    ) -> Result<(), SessionStoreError> {
        self.append_entries_with_hint_policy(session_id, entries, true)
            .await
    }

    async fn append_entries_with_hint_policy(
        &self,
        session_id: &str,
        mut entries: Vec<Value>,
        decorate_with_hints: bool,
    ) -> Result<(), SessionStoreError> {
        let hints = self.hints_for(session_id).await;
        let mut payload = String::new();
        for entry in entries.iter_mut() {
            if decorate_with_hints {
                Self::decorate_entry_with_hints(entry, &hints);
            }
            payload.push_str(&serde_json::to_string(&entry)?);
            payload.push('\n');
        }

        let lock = self.session_write_lock(session_id).await;
        let _write_guard = lock.lock().await;
        let path = self.path(session_id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                SessionStoreError::transcript_io("create project dir", parent, error)
            })?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|error| SessionStoreError::transcript_io("open transcript", &path, error))?;
        // Ensure a stray prior partial line cannot fuse with this append.
        // If the file ends without a newline (e.g. truncated by a prior
        // crash), prepend one so the next reader sees a clean boundary
        // instead of two records glued together.
        match file.metadata().await {
            Ok(metadata) if metadata.len() > 0 => {
                if let Err(error) = ensure_trailing_newline(&path).await {
                    return Err(SessionStoreError::transcript_io(
                        "repair transcript boundary",
                        &path,
                        error,
                    ));
                }
            }
            Ok(_) => {}
            Err(error) => {
                return Err(SessionStoreError::transcript_io(
                    "stat transcript",
                    &path,
                    error,
                ));
            }
        }
        file.write_all(payload.as_bytes())
            .await
            .map_err(|error| SessionStoreError::transcript_io("append transcript", &path, error))?;
        file.flush()
            .await
            .map_err(|error| SessionStoreError::transcript_io("flush transcript", &path, error))?;
        Ok(())
    }
}

/// If the file does not currently end in a newline, append one so that a
/// previously truncated final record cannot fuse with the next appended
/// line. Returns the underlying io error verbatim — callers wrap it.
async fn ensure_trailing_newline(path: &Path) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)
        .await?;
    let len = file.metadata().await?.len();
    if len == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::End(-1)).await?;
    let mut buf = [0u8; 1];
    file.read_exact(&mut buf).await?;
    if buf[0] != b'\n' {
        file.write_all(b"\n").await?;
        // tokio's File buffers writes and does not flush on drop, so the
        // separating newline must be flushed here. Otherwise the caller's
        // append (a second O_APPEND handle) can reach the OS first and the
        // late newline lands after the payload — fusing the prior partial
        // record and leaving a stray trailing blank line.
        file.flush().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};

    #[tokio::test]
    async fn append_message_line_writes_transcript_entries() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );
        let message = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "Read".to_string(),
                input: r#"{"file_path":"README.md"}"#.to_string(),
            }],
        );

        store
            .append_message_line("session-1", &message, Some("user-1"))
            .await
            .expect("append message");

        let contents = tokio::fs::read_to_string(temp.path().join("session-1.jsonl"))
            .await
            .expect("read transcript");
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 1);
        let entry: Value = serde_json::from_str(lines[0]).expect("parse transcript entry");
        assert_eq!(entry.get("type").and_then(Value::as_str), Some("assistant"));
        assert_eq!(
            entry
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|block| block.get("input"))
                .and_then(|input| input.get("file_path"))
                .and_then(Value::as_str),
            Some("README.md")
        );
    }

    #[tokio::test]
    async fn remove_session_file_if_exists_ignores_missing_file() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );
        store
            .append_message_line(
                "session-1",
                &TranscriptMessage::new(MessageRole::User, "hello"),
                None,
            )
            .await
            .expect("append message");
        assert!(
            store
                .session_file_exists("session-1")
                .await
                .expect("check existing file")
        );

        store
            .remove_session_file_if_exists("session-1")
            .await
            .expect("remove transcript");
        store
            .remove_session_file_if_exists("session-1")
            .await
            .expect("missing transcript is ok");

        assert!(
            !store
                .session_file_exists("session-1")
                .await
                .expect("check removed file")
        );
    }

    #[tokio::test]
    async fn remove_last_user_prompt_if_matches_rewrites_remaining_session() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );
        store
            .append_message_line(
                "session-1",
                &TranscriptMessage::new(MessageRole::System, "context"),
                None,
            )
            .await
            .expect("append system");
        let parent_uuid = store
            .load_session("session-1")
            .await
            .expect("load parent")
            .messages[0]
            .id
            .clone();
        store
            .append_message_line(
                "session-1",
                &TranscriptMessage::new(MessageRole::User, "blocked prompt"),
                Some(&parent_uuid),
            )
            .await
            .expect("append prompt");

        store
            .remove_last_user_prompt_if_matches("session-1", "blocked prompt")
            .await
            .expect("remove prompt");

        let session = store.load_session("session-1").await.expect("load session");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].role, MessageRole::System);
        assert_eq!(session.messages[0].content, "context");
    }

    #[tokio::test]
    async fn remove_last_user_prompt_if_matches_deletes_prompt_only_session() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );
        store
            .append_message_line(
                "session-1",
                &TranscriptMessage::new(MessageRole::User, "blocked prompt"),
                None,
            )
            .await
            .expect("append prompt");

        store
            .remove_last_user_prompt_if_matches("session-1", "blocked prompt")
            .await
            .expect("remove prompt");

        assert!(
            !store
                .session_file_exists("session-1")
                .await
                .expect("check removed file")
        );
    }

    #[tokio::test]
    async fn append_progress_for_latest_parent_uses_last_message_id() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );
        let message = TranscriptMessage::new(MessageRole::Assistant, "working");
        let message_id = message.id.clone();
        store
            .append_message_line("session-1", &message, None)
            .await
            .expect("append message");

        store
            .append_progress_for_latest_parent(
                "session-1",
                serde_json::json!({
                    "uuid": "progress-1",
                    "timestamp": "2026-01-01T00:00:00.000Z",
                    "data": { "message": "still working" }
                }),
            )
            .await
            .expect("append progress");

        let contents = tokio::fs::read_to_string(temp.path().join("session-1.jsonl"))
            .await
            .expect("read transcript");
        let entries = contents
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("parse entry"))
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[1].get("parentUuid").and_then(Value::as_str),
            Some(message_id.as_str())
        );
    }

    #[tokio::test]
    async fn append_progress_for_tool_use_parent_anchors_to_spawning_message() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        // The message that emitted the Agent tool_use.
        let spawning = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "agent-tool-1".to_string(),
                name: "Agent".to_string(),
                input: "{}".to_string(),
            }],
        );
        let spawning_id = spawning.id.clone();
        store
            .append_message_line("session-1", &spawning, None)
            .await
            .expect("append spawning message");

        // The parent session moves on to an unrelated later message while the
        // detached background agent is still running.
        let later = TranscriptMessage::new(MessageRole::User, "a later, unrelated prompt");
        let later_id = later.id.clone();
        assert_ne!(spawning_id, later_id);
        store
            .append_message_line("session-1", &later, Some(&spawning_id))
            .await
            .expect("append later message");

        store
            .append_progress_for_tool_use_parent(
                "session-1",
                "agent-tool-1",
                serde_json::json!({
                    "uuid": "progress-1",
                    "timestamp": "2026-01-01T00:00:00.000Z",
                    "data": { "message": "agent still working" }
                }),
            )
            .await
            .expect("append progress");

        let contents = tokio::fs::read_to_string(temp.path().join("session-1.jsonl"))
            .await
            .expect("read transcript");
        let entries = contents
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("parse entry"))
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 3);
        // The progress line must anchor to the spawning tool_use message, not
        // the unrelated later prompt.
        assert_eq!(
            entries[2].get("parentUuid").and_then(Value::as_str),
            Some(spawning_id.as_str()),
            "background progress must attach to the spawning tool_use, not the latest message"
        );
    }

    #[tokio::test]
    async fn append_progress_for_latest_parent_if_present_skips_missing_session() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        store
            .append_progress_for_latest_parent_if_present(
                "missing-session",
                serde_json::json!({ "data": { "message": "ignored" } }),
            )
            .await
            .expect("missing session is ignored");

        assert!(
            !store
                .session_file_exists("missing-session")
                .await
                .expect("check missing session")
        );
    }

    #[tokio::test]
    async fn record_session_hints_decorates_appended_entries_with_branch_and_provider() {
        use super::super::SessionWriteHints;
        use orbcode_protocol::ProviderId;

        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        store
            .record_session_hints(
                "session-1",
                SessionWriteHints {
                    git_branch: Some("feature/x".to_string()),
                    provider: Some(ProviderId::Anthropic),
                },
            )
            .await;
        store
            .append_message_line(
                "session-1",
                &TranscriptMessage::new(MessageRole::User, "hello"),
                None,
            )
            .await
            .expect("append message");

        let contents = tokio::fs::read_to_string(temp.path().join("session-1.jsonl"))
            .await
            .expect("read transcript");
        let entry: Value =
            serde_json::from_str(contents.lines().next().expect("entry")).expect("parse entry");
        assert_eq!(
            entry.get("gitBranch").and_then(Value::as_str),
            Some("feature/x")
        );
        assert_eq!(
            entry.get("provider").and_then(Value::as_str),
            Some("anthropic")
        );
    }

    #[tokio::test]
    async fn append_custom_title_line_overrides_decoded_title() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );
        store
            .append_message_line(
                "session-1",
                &TranscriptMessage::new(MessageRole::User, "first user message"),
                None,
            )
            .await
            .expect("append user");

        store
            .append_custom_title_line("session-1", "  Refactor: shipping the picker  ")
            .await
            .expect("append custom title");

        let session = store.load_session("session-1").await.expect("load session");
        assert_eq!(
            session.custom_title.as_deref(),
            Some("Refactor: shipping the picker")
        );
        assert_eq!(
            session.display_title(),
            Some("Refactor: shipping the picker")
        );
        // Original auto title is preserved alongside.
        assert!(session.title.is_some());
    }

    #[tokio::test]
    async fn persist_full_session_writes_custom_title_when_present() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );
        let mut session = SessionRecord {
            session_id: "session-1".to_string(),
            title: None,
            custom_title: Some("Custom Topic".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            cwd: None,
            git_branch: None,
            model: None,
            provider: None,
            additional_directories: Vec::new(),
            session_allowed_tools: Vec::new(),
            session_disallowed_tools: Vec::new(),
            session_effort: None,
            messages: Vec::new(),
        };
        session.push_message(TranscriptMessage::new(MessageRole::User, "hi"));
        store.persist_full_session(&session).await.expect("persist");
        let reloaded = store.load_session("session-1").await.expect("load");
        assert_eq!(reloaded.custom_title.as_deref(), Some("Custom Topic"));
        assert_eq!(reloaded.display_title(), Some("Custom Topic"));
    }

    #[tokio::test]
    async fn append_entries_heals_trailing_partial_line_before_new_append() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        // Simulate a transcript whose final line never got its newline —
        // e.g. the process crashed mid-append. A naive concatenation
        // would fuse the two records and break the next decode.
        let path = temp.path().join("session-1.jsonl");
        tokio::fs::write(&path, "{\"type\":\"user\",\"truncated\":true")
            .await
            .expect("write truncated transcript");

        store
            .append_message_line(
                "session-1",
                &TranscriptMessage::new(MessageRole::User, "second"),
                None,
            )
            .await
            .expect("append after truncation");

        let contents = tokio::fs::read_to_string(&path).await.expect("read healed");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        // The first line stays truncated (we cannot reconstruct it), but
        // is now bounded by a newline so the second append parses cleanly.
        assert!(lines[0].starts_with("{\"type\":\"user\""));
        let second: Value =
            serde_json::from_str(lines[1]).expect("second appended line parses standalone");
        assert_eq!(second.get("type").and_then(Value::as_str), Some("user"));
    }

    #[tokio::test]
    async fn persist_full_session_reports_disk_full_with_hint() {
        use std::io::{Error, ErrorKind};

        let error = Error::new(ErrorKind::StorageFull, "no space left on device");
        let wrapped = SessionStoreError::transcript_io(
            "write transcript",
            std::path::Path::new("/tmp/session.jsonl"),
            error,
        );
        let rendered = format!("{wrapped}");
        assert!(rendered.contains("write transcript"));
        assert!(rendered.contains("/tmp/session.jsonl"));
        assert!(
            rendered.contains("disk full"),
            "expected disk-full recovery hint in {rendered}"
        );
    }

    #[tokio::test]
    async fn persist_full_session_rewrites_transcript() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );
        tokio::fs::write(temp.path().join("session-1.jsonl"), "stale\n")
            .await
            .expect("write stale transcript");
        let mut session = SessionRecord {
            session_id: "session-1".to_string(),
            title: None,
            custom_title: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            cwd: None,
            git_branch: None,
            model: None,
            provider: None,
            additional_directories: Vec::new(),
            session_allowed_tools: Vec::new(),
            session_disallowed_tools: Vec::new(),
            session_effort: None,
            messages: Vec::new(),
        };
        session.push_message(TranscriptMessage::new(MessageRole::User, "hello"));

        store
            .persist_full_session(&session)
            .await
            .expect("rewrite transcript");

        let contents = tokio::fs::read_to_string(temp.path().join("session-1.jsonl"))
            .await
            .expect("read transcript");
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 1);
        assert!(!contents.contains("stale"));
        let entry: Value = serde_json::from_str(lines[0]).expect("parse transcript entry");
        assert_eq!(entry.get("type").and_then(Value::as_str), Some("user"));
    }
}
