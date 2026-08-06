use std::path::PathBuf;

use orbcode_protocol::{
    EffortLevel, SessionGoal, SessionGoalTurnTerminalKind, SessionRecord, TokenUsage,
    TranscriptMessage,
};
use serde_json::Value;

use crate::{
    SessionStoreError,
    files::{SessionStorageHealth, SessionWriteHints, TranscriptFileStore},
    tool_results::ToolResultStore,
};

#[derive(Clone)]
pub struct SessionStore {
    transcript_files: TranscriptFileStore,
    tool_results: ToolResultStore,
}

impl SessionStore {
    pub async fn append_goal_snapshot(&self, goal: &SessionGoal) -> Result<(), SessionStoreError> {
        self.transcript_files.append_goal_snapshot(goal).await
    }

    pub async fn append_goal_cleared(
        &self,
        session_id: &str,
        goal_id: &str,
        revision: u64,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .append_goal_cleared(session_id, goal_id, revision)
            .await
    }

    pub async fn append_goal_turn_start(
        &self,
        session_id: &str,
        goal_id: &str,
        goal_revision: u64,
        turn_id: &str,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .append_goal_turn_start(session_id, goal_id, goal_revision, turn_id)
            .await
    }

    pub async fn append_goal_snapshot_and_turn_start(
        &self,
        goal: &SessionGoal,
        turn_id: &str,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .append_goal_snapshot_and_turn_start(goal, turn_id)
            .await
    }

    pub async fn append_goal_turn_terminal(
        &self,
        session_id: &str,
        goal_id: &str,
        goal_revision: u64,
        turn_id: &str,
        terminal_kind: SessionGoalTurnTerminalKind,
        usage: &TokenUsage,
        elapsed_seconds: u64,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .append_goal_turn_terminal(
                session_id,
                goal_id,
                goal_revision,
                turn_id,
                terminal_kind,
                usage,
                elapsed_seconds,
            )
            .await
    }

    pub async fn append_goal_turn_terminal_and_snapshot(
        &self,
        goal: &SessionGoal,
        started_revision: u64,
        turn_id: &str,
        terminal_kind: SessionGoalTurnTerminalKind,
        usage: &TokenUsage,
        elapsed_seconds: u64,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .append_goal_turn_terminal_and_snapshot(
                goal,
                started_revision,
                turn_id,
                terminal_kind,
                usage,
                elapsed_seconds,
            )
            .await
    }

    pub fn new(current_project_dir: PathBuf, cwd: PathBuf, anthropic_model: String) -> Self {
        Self {
            transcript_files: TranscriptFileStore::new(
                current_project_dir.clone(),
                cwd,
                anthropic_model,
            ),
            tool_results: ToolResultStore::new(current_project_dir),
        }
    }

    pub fn path(&self, session_id: &str) -> PathBuf {
        self.transcript_files.path(session_id)
    }

    pub fn record_session_location(
        &self,
        session_id: &str,
        path: &std::path::Path,
        cwd: &std::path::Path,
    ) {
        self.transcript_files
            .record_session_location(session_id, path, cwd);
    }

    pub fn record_session_cwd(&self, session_id: &str, cwd: &std::path::Path) {
        let cwd = cwd.display().to_string();
        self.transcript_files
            .record_session_cwd(session_id, Some(&cwd));
    }

    pub fn recorded_session_cwd(&self, session_id: &str) -> Option<std::path::PathBuf> {
        self.transcript_files.recorded_cwd_for(session_id)
    }

    pub async fn persist_tool_result(
        &self,
        session_id: &str,
        tool_use_id: &str,
        content: &str,
    ) -> Result<String, SessionStoreError> {
        self.tool_results
            .persist(session_id, tool_use_id, content)
            .await
    }

    pub async fn apply_tool_result_budget(
        &self,
        session_id: &str,
        messages: &mut [TranscriptMessage],
    ) -> Result<(), SessionStoreError> {
        self.tool_results
            .apply_budget_replacements(session_id, messages)
            .await
    }

    pub async fn load_session(&self, session_id: &str) -> Result<SessionRecord, SessionStoreError> {
        self.transcript_files.load_session(session_id).await
    }

    pub async fn load_session_any_project(
        &self,
        session_id: &str,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.transcript_files
            .load_session_any_project(session_id)
            .await
    }

    pub async fn load_session_from_path_as(
        &self,
        session_id: &str,
        path: &std::path::Path,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.transcript_files
            .load_session_from_path_as(session_id, path)
            .await
    }

    pub async fn load_latest_project_session(&self) -> Result<SessionRecord, SessionStoreError> {
        self.transcript_files.load_latest_project_session().await
    }

    pub async fn load_latest_project_session_for_prefixes(
        &self,
        prefixes: &[String],
    ) -> Result<SessionRecord, SessionStoreError> {
        self.transcript_files
            .load_latest_project_session_for_prefixes(prefixes)
            .await
    }

    pub async fn load_session_if_present(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionRecord>, SessionStoreError> {
        self.transcript_files
            .load_session_if_present(session_id)
            .await
    }

    pub async fn load_project_sessions(&self) -> Result<Vec<SessionRecord>, SessionStoreError> {
        self.transcript_files.load_project_sessions().await
    }

    pub async fn load_project_sessions_for_prefixes(
        &self,
        prefixes: &[String],
    ) -> Result<Vec<SessionRecord>, SessionStoreError> {
        self.transcript_files
            .load_project_sessions_for_prefixes(prefixes)
            .await
    }

    pub async fn remove_session_file_if_exists(
        &self,
        session_id: &str,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .remove_session_file_if_exists(session_id)
            .await
    }

    pub async fn remove_last_user_prompt_if_matches(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .remove_last_user_prompt_if_matches(session_id, prompt)
            .await
    }

    pub async fn append_progress_for_latest_parent(
        &self,
        session_id: &str,
        progress: Value,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .append_progress_for_latest_parent(session_id, progress)
            .await
    }

    pub async fn append_progress_for_latest_parent_if_present(
        &self,
        session_id: &str,
        progress: Value,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .append_progress_for_latest_parent_if_present(session_id, progress)
            .await
    }

    pub async fn append_progress_for_tool_use_parent(
        &self,
        session_id: &str,
        tool_use_id: &str,
        progress: Value,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .append_progress_for_tool_use_parent(session_id, tool_use_id, progress)
            .await
    }

    pub async fn append_message_line(
        &self,
        session_id: &str,
        message: &TranscriptMessage,
        parent_uuid: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .append_message_line(session_id, message, parent_uuid)
            .await
    }

    pub async fn persist_full_session(
        &self,
        session: &SessionRecord,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files.persist_full_session(session).await
    }

    pub async fn append_session_context_line(
        &self,
        session_id: &str,
        additional_directories: &[PathBuf],
        session_allowed_tools: &[String],
        session_disallowed_tools: &[String],
        session_effort: Option<EffortLevel>,
    ) -> Result<(), SessionStoreError> {
        self.transcript_files
            .append_session_context_line(
                session_id,
                additional_directories,
                session_allowed_tools,
                session_disallowed_tools,
                session_effort,
            )
            .await
    }

    pub async fn record_session_hints(&self, session_id: &str, hints: SessionWriteHints) {
        self.transcript_files
            .record_session_hints(session_id, hints)
            .await
    }

    /// Append a `custom-title` entry to the transcript, overriding the
    /// auto-generated title for future loads. The append uses
    /// `OpenOptions::create(true)`, so renaming a session before any
    /// model-visible message has been recorded materializes the transcript
    /// with the title row, making the session visible in `/sessions` and
    /// resumable instead of dropped on the floor.
    pub async fn rename_session(
        &self,
        session_id: &str,
        new_title: &str,
    ) -> Result<(), SessionStoreError> {
        let trimmed = new_title.trim();
        if trimmed.is_empty() {
            return Err(SessionStoreError::Config(
                "session title must not be empty".into(),
            ));
        }
        self.transcript_files
            .append_custom_title_line(session_id, trimmed)
            .await
    }

    pub async fn load_project_session_summaries(
        &self,
    ) -> Result<Vec<orbcode_protocol::SessionSummary>, SessionStoreError> {
        self.transcript_files.load_project_session_summaries().await
    }

    pub async fn load_project_session_summaries_for_prefixes(
        &self,
        prefixes: &[String],
    ) -> Result<Vec<orbcode_protocol::SessionSummary>, SessionStoreError> {
        self.transcript_files
            .load_project_session_summaries_for_prefixes(prefixes)
            .await
    }

    /// See [`TranscriptFileStore::storage_health`]. Returned snapshot is
    /// the data the doctor uses to decide whether to surface a recovery
    /// hint for the current project's transcript directory.
    pub async fn storage_health(&self) -> SessionStorageHealth {
        self.transcript_files.storage_health().await
    }

    /// See [`TranscriptFileStore::gc_stale_sessions`].
    pub async fn gc_stale_sessions(
        &self,
        threshold_days: u64,
    ) -> Result<crate::files::GcResult, SessionStoreError> {
        self.transcript_files
            .gc_stale_sessions(threshold_days)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_protocol::MessageRole;

    #[tokio::test]
    async fn session_store_persists_transcripts_and_tool_results() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = SessionStore::new(
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
            .expect("append transcript message");
        let tool_result_path = store
            .persist_tool_result("session-1", "tool-1", "full output")
            .await
            .expect("persist tool result");

        let session = store.load_session("session-1").await.expect("load session");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(
            tokio::fs::read_to_string(tool_result_path)
                .await
                .expect("read tool result"),
            "full output"
        );
    }
}
