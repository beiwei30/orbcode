use std::{collections::HashSet, path::PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{SessionStoreError, transcript::decode_session_transcript_with_outcome};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChildSessionStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

impl ChildSessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildSessionMetadata {
    #[serde(rename = "childSessionId")]
    pub child_session_id: String,
    #[serde(rename = "parentSessionId")]
    pub parent_session_id: String,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    #[serde(rename = "agentType")]
    pub agent_type: String,
    #[serde(rename = "sourceToolUseId")]
    pub source_tool_use_id: String,
    pub cwd: String,
    pub model: Option<String>,
    #[serde(rename = "permissionMode")]
    pub permission_mode: Option<String>,
    #[serde(rename = "promptPreview")]
    pub prompt_preview: String,
    pub status: ChildSessionStatus,
    #[serde(rename = "startedAt")]
    pub started_at: i64,
    #[serde(rename = "endedAt", skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(rename = "lastActivityAt")]
    pub last_activity_at: i64,
    #[serde(rename = "errorMessage", skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StartChildSessionInput {
    pub child_session_id: String,
    pub parent_session_id: String,
    pub agent_id: String,
    pub agent_type: String,
    pub source_tool_use_id: String,
    pub cwd: String,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub prompt: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChildSessionCleanupResult {
    pub metadata_removed: usize,
    pub transcripts_removed: usize,
}

impl ChildSessionCleanupResult {
    fn merge(&mut self, other: ChildSessionCleanupResult) {
        self.metadata_removed += other.metadata_removed;
        self.transcripts_removed += other.transcripts_removed;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChildSessionStorageHealth {
    pub metadata_records: usize,
    pub transcript_records: usize,
    pub corrupt_metadata_records: usize,
    pub corrupt_transcripts: usize,
    pub orphan_metadata_records: usize,
    pub orphan_transcripts: usize,
    pub workflow_children_without_transcripts: usize,
}

const PROMPT_PREVIEW_LIMIT: usize = 240;

fn truncate_for_preview(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= PROMPT_PREVIEW_LIMIT {
        return trimmed.to_string();
    }
    let mut preview: String = trimmed.chars().take(PROMPT_PREVIEW_LIMIT).collect();
    preview.push('…');
    preview
}

#[derive(Clone)]
pub struct ChildSessionStore {
    root: PathBuf,
}

impl ChildSessionStore {
    /// `sessions_dir` is the directory containing live session registry files;
    /// child metadata lives under `<sessions_dir>/agents/`.
    #[allow(clippy::needless_pass_by_value)] // Constructor API takes ownership of the configured root path.
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            root: sessions_dir.join("agents"),
        }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    fn path_for(&self, child_session_id: &str) -> PathBuf {
        let sanitized = sanitize_session_id(child_session_id);
        self.root.join(format!("{sanitized}.json"))
    }

    pub fn transcript_path_for(&self, child_session_id: &str) -> PathBuf {
        let sanitized = sanitize_session_id(child_session_id);
        self.root
            .join("transcripts")
            .join(format!("{sanitized}.jsonl"))
    }

    pub async fn start(
        &self,
        input: StartChildSessionInput,
    ) -> Result<ChildSessionMetadata, SessionStoreError> {
        tokio::fs::create_dir_all(&self.root).await?;
        let now = Utc::now().timestamp_millis();
        let metadata = ChildSessionMetadata {
            child_session_id: input.child_session_id,
            parent_session_id: input.parent_session_id,
            agent_id: input.agent_id,
            agent_type: input.agent_type,
            source_tool_use_id: input.source_tool_use_id,
            cwd: input.cwd,
            model: input.model,
            permission_mode: input.permission_mode,
            prompt_preview: truncate_for_preview(&input.prompt),
            status: ChildSessionStatus::Running,
            started_at: now,
            ended_at: None,
            last_activity_at: now,
            error_message: None,
        };
        self.write(&metadata).await?;
        Ok(metadata)
    }

    pub async fn complete(&self, child_session_id: &str) -> Result<(), SessionStoreError> {
        self.finalize(child_session_id, ChildSessionStatus::Completed, None)
            .await
    }

    pub async fn cancel(&self, child_session_id: &str) -> Result<(), SessionStoreError> {
        self.finalize(child_session_id, ChildSessionStatus::Cancelled, None)
            .await
    }

    pub async fn fail(&self, child_session_id: &str, error: &str) -> Result<(), SessionStoreError> {
        self.finalize(
            child_session_id,
            ChildSessionStatus::Failed,
            Some(error.to_string()),
        )
        .await
    }

    async fn finalize(
        &self,
        child_session_id: &str,
        status: ChildSessionStatus,
        error_message: Option<String>,
    ) -> Result<(), SessionStoreError> {
        let Some(mut metadata) = self.load(child_session_id).await? else {
            return Ok(());
        };
        let now = Utc::now().timestamp_millis();
        metadata.status = status;
        metadata.ended_at = Some(now);
        metadata.last_activity_at = now;
        metadata.error_message = error_message;
        self.write(&metadata).await
    }

    pub async fn load(
        &self,
        child_session_id: &str,
    ) -> Result<Option<ChildSessionMetadata>, SessionStoreError> {
        let path = self.path_for(child_session_id);
        if !tokio::fs::try_exists(&path).await? {
            return Ok(None);
        }
        let contents = tokio::fs::read_to_string(path).await?;
        Ok(Some(serde_json::from_str(&contents)?))
    }

    pub async fn list_for_parent(
        &self,
        parent_session_id: &str,
    ) -> Result<Vec<ChildSessionMetadata>, SessionStoreError> {
        let mut results = self.list_all().await?;
        results.retain(|metadata| metadata.parent_session_id == parent_session_id);
        Ok(results)
    }

    pub async fn list_all(&self) -> Result<Vec<ChildSessionMetadata>, SessionStoreError> {
        if !tokio::fs::try_exists(&self.root).await? {
            return Ok(Vec::new());
        }
        let mut entries = tokio::fs::read_dir(&self.root).await?;
        let mut results = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(contents) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            let Ok(metadata) = serde_json::from_str::<ChildSessionMetadata>(&contents) else {
                continue;
            };
            results.push(metadata);
        }
        results.sort_by_key(|metadata| metadata.started_at);
        Ok(results)
    }

    pub async fn remove(
        &self,
        child_session_id: &str,
    ) -> Result<ChildSessionCleanupResult, SessionStoreError> {
        let mut result = ChildSessionCleanupResult::default();
        let metadata_path = self.path_for(child_session_id);
        match tokio::fs::remove_file(&metadata_path).await {
            Ok(()) => result.metadata_removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let transcript_path = self.transcript_path_for(child_session_id);
        match tokio::fs::remove_file(&transcript_path).await {
            Ok(()) => result.transcripts_removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        Ok(result)
    }

    pub async fn remove_for_parent(
        &self,
        parent_session_id: &str,
    ) -> Result<ChildSessionCleanupResult, SessionStoreError> {
        let children = self.list_for_parent(parent_session_id).await?;
        let mut result = ChildSessionCleanupResult::default();
        for child in children {
            result.merge(self.remove(&child.child_session_id).await?);
        }
        Ok(result)
    }

    pub async fn storage_health(
        &self,
        known_parent_session_ids: &HashSet<String>,
        scoped_cwds: Option<&HashSet<String>>,
    ) -> Result<ChildSessionStorageHealth, SessionStoreError> {
        let mut health = ChildSessionStorageHealth::default();
        let mut metadata_transcript_names = HashSet::new();
        let mut transcript_session_ids = std::collections::HashMap::new();
        let mut ignored_transcript_names = HashSet::new();

        if tokio::fs::try_exists(&self.root).await? {
            let mut entries = tokio::fs::read_dir(&self.root).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let Ok(contents) = tokio::fs::read_to_string(&path).await else {
                    health.corrupt_metadata_records += 1;
                    continue;
                };
                let Ok(metadata) = serde_json::from_str::<ChildSessionMetadata>(&contents) else {
                    health.corrupt_metadata_records += 1;
                    continue;
                };
                let transcript_path = self.transcript_path_for(&metadata.child_session_id);
                if scoped_cwds.is_some_and(|cwds| !cwds.contains(&metadata.cwd)) {
                    if let Some(file_name) =
                        transcript_path.file_name().and_then(|name| name.to_str())
                    {
                        ignored_transcript_names.insert(file_name.to_string());
                    }
                    continue;
                }

                health.metadata_records += 1;
                if !known_parent_session_ids.contains(&metadata.parent_session_id) {
                    health.orphan_metadata_records += 1;
                }

                if let Some(file_name) = transcript_path.file_name().and_then(|name| name.to_str())
                {
                    metadata_transcript_names.insert(file_name.to_string());
                    transcript_session_ids
                        .insert(file_name.to_string(), metadata.child_session_id.clone());
                }
                if !tokio::fs::try_exists(&transcript_path).await?
                    && metadata.source_tool_use_id.starts_with("workflow:")
                {
                    health.workflow_children_without_transcripts += 1;
                }
            }
        }

        let transcript_dir = self.root.join("transcripts");
        if tokio::fs::try_exists(&transcript_dir).await? {
            let mut entries = tokio::fs::read_dir(&transcript_dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                    continue;
                }
                let file_name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string();
                if ignored_transcript_names.contains(&file_name) {
                    continue;
                }
                health.transcript_records += 1;
                if !metadata_transcript_names.contains(&file_name) {
                    health.orphan_transcripts += 1;
                }

                let session_id = transcript_session_ids
                    .get(&file_name)
                    .cloned()
                    .or_else(|| {
                        path.file_stem()
                            .and_then(|name| name.to_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                let Ok(contents) = tokio::fs::read_to_string(&path).await else {
                    health.corrupt_transcripts += 1;
                    continue;
                };
                let outcome = decode_session_transcript_with_outcome(session_id, &contents);
                if outcome.session.is_none() {
                    health.corrupt_transcripts += 1;
                }
            }
        }

        Ok(health)
    }

    async fn write(&self, metadata: &ChildSessionMetadata) -> Result<(), SessionStoreError> {
        let path = self.path_for(&metadata.child_session_id);
        let payload = serde_json::to_string_pretty(metadata)?;
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, payload).await?;
        tokio::fs::rename(tmp, path).await?;
        Ok(())
    }
}

fn sanitize_session_id(child_session_id: &str) -> String {
    child_session_id
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '_',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn sample_input(child_id: &str, parent_id: &str) -> StartChildSessionInput {
        StartChildSessionInput {
            child_session_id: child_id.to_string(),
            parent_session_id: parent_id.to_string(),
            agent_id: "agent-1".to_string(),
            agent_type: "rust-reviewer".to_string(),
            source_tool_use_id: "tool-use-1".to_string(),
            cwd: "/tmp/project".to_string(),
            model: Some("claude-haiku-4-5".to_string()),
            permission_mode: Some("plan".to_string()),
            prompt: "  please review main.rs for ownership issues\n".to_string(),
        }
    }

    async fn write_child_transcript(store: &ChildSessionStore, child_id: &str) {
        let path = store.transcript_path_for(child_id);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        let line = serde_json::to_string(&json!({
            "type": "user",
            "uuid": format!("{child_id}-user"),
            "timestamp": "2026-04-10T00:00:00.000Z",
            "message": { "role": "user", "content": "child prompt" },
            "cwd": "/tmp/project",
            "sessionId": child_id,
        }))
        .unwrap();
        tokio::fs::write(path, format!("{line}\n")).await.unwrap();
    }

    #[tokio::test]
    async fn start_persists_running_metadata_with_preview() {
        let temp = tempdir().unwrap();
        let store = ChildSessionStore::new(temp.path().to_path_buf());
        let metadata = store
            .start(sample_input("parent-1:agent-1", "parent-1"))
            .await
            .unwrap();

        assert_eq!(metadata.status, ChildSessionStatus::Running);
        assert_eq!(metadata.agent_type, "rust-reviewer");
        assert_eq!(metadata.parent_session_id, "parent-1");
        assert_eq!(
            metadata.prompt_preview,
            "please review main.rs for ownership issues"
        );
        assert!(metadata.ended_at.is_none());

        let loaded = store.load("parent-1:agent-1").await.unwrap().unwrap();
        assert_eq!(loaded, metadata);
    }

    #[tokio::test]
    async fn complete_marks_completed_and_records_ended_at() {
        let temp = tempdir().unwrap();
        let store = ChildSessionStore::new(temp.path().to_path_buf());
        store
            .start(sample_input("parent-1:agent-1", "parent-1"))
            .await
            .unwrap();
        store.complete("parent-1:agent-1").await.unwrap();

        let loaded = store.load("parent-1:agent-1").await.unwrap().unwrap();
        assert_eq!(loaded.status, ChildSessionStatus::Completed);
        assert!(loaded.ended_at.is_some());
        assert!(loaded.error_message.is_none());
    }

    #[tokio::test]
    async fn fail_records_error_message() {
        let temp = tempdir().unwrap();
        let store = ChildSessionStore::new(temp.path().to_path_buf());
        store
            .start(sample_input("parent-1:agent-1", "parent-1"))
            .await
            .unwrap();
        store
            .fail("parent-1:agent-1", "stream timeout")
            .await
            .unwrap();

        let loaded = store.load("parent-1:agent-1").await.unwrap().unwrap();
        assert_eq!(loaded.status, ChildSessionStatus::Failed);
        assert_eq!(loaded.error_message.as_deref(), Some("stream timeout"));
    }

    #[tokio::test]
    async fn cancel_marks_cancelled() {
        let temp = tempdir().unwrap();
        let store = ChildSessionStore::new(temp.path().to_path_buf());
        store
            .start(sample_input("parent-1:agent-1", "parent-1"))
            .await
            .unwrap();
        store.cancel("parent-1:agent-1").await.unwrap();

        let loaded = store.load("parent-1:agent-1").await.unwrap().unwrap();
        assert_eq!(loaded.status, ChildSessionStatus::Cancelled);
    }

    #[tokio::test]
    async fn list_for_parent_returns_only_children_of_requested_parent() {
        let temp = tempdir().unwrap();
        let store = ChildSessionStore::new(temp.path().to_path_buf());
        store
            .start(sample_input("parent-1:agent-1", "parent-1"))
            .await
            .unwrap();
        store
            .start(sample_input("parent-1:agent-2", "parent-1"))
            .await
            .unwrap();
        store
            .start(sample_input("parent-2:agent-3", "parent-2"))
            .await
            .unwrap();

        let mut children = store.list_for_parent("parent-1").await.unwrap();
        children.sort_by(|left, right| left.child_session_id.cmp(&right.child_session_id));
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].child_session_id, "parent-1:agent-1");
        assert_eq!(children[1].child_session_id, "parent-1:agent-2");
    }

    #[tokio::test]
    async fn list_all_returns_all_child_metadata() {
        let temp = tempdir().unwrap();
        let store = ChildSessionStore::new(temp.path().to_path_buf());
        store
            .start(sample_input("parent-1:agent-1", "parent-1"))
            .await
            .unwrap();
        store
            .start(sample_input("parent-2:agent-2", "parent-2"))
            .await
            .unwrap();

        let mut children = store.list_all().await.unwrap();
        children.sort_by(|left, right| left.child_session_id.cmp(&right.child_session_id));
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].child_session_id, "parent-1:agent-1");
        assert_eq!(children[1].child_session_id, "parent-2:agent-2");
    }

    #[tokio::test]
    async fn remove_for_parent_removes_metadata_and_transcripts_for_matching_children() {
        let temp = tempdir().unwrap();
        let store = ChildSessionStore::new(temp.path().to_path_buf());
        store
            .start(sample_input("parent-1:agent-1", "parent-1"))
            .await
            .unwrap();
        store
            .start(sample_input("parent-1:agent-2", "parent-1"))
            .await
            .unwrap();
        store
            .start(sample_input("parent-2:agent-3", "parent-2"))
            .await
            .unwrap();
        write_child_transcript(&store, "parent-1:agent-1").await;
        write_child_transcript(&store, "parent-1:agent-2").await;
        write_child_transcript(&store, "parent-2:agent-3").await;

        let result = store.remove_for_parent("parent-1").await.unwrap();

        assert_eq!(result.metadata_removed, 2);
        assert_eq!(result.transcripts_removed, 2);
        assert!(store.load("parent-1:agent-1").await.unwrap().is_none());
        assert!(store.load("parent-1:agent-2").await.unwrap().is_none());
        assert!(store.load("parent-2:agent-3").await.unwrap().is_some());
        assert!(
            !tokio::fs::try_exists(store.transcript_path_for("parent-1:agent-1"))
                .await
                .unwrap()
        );
        assert!(
            tokio::fs::try_exists(store.transcript_path_for("parent-2:agent-3"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn storage_health_reports_child_transcript_mismatches() {
        let temp = tempdir().unwrap();
        let store = ChildSessionStore::new(temp.path().to_path_buf());
        store
            .start(sample_input("parent-1:agent-good", "parent-1"))
            .await
            .unwrap();
        write_child_transcript(&store, "parent-1:agent-good").await;

        let mut journal_only = sample_input("parent-1:agent-journal", "parent-1");
        journal_only.source_tool_use_id = "workflow:run-1:step.0".to_string();
        store.start(journal_only).await.unwrap();

        store
            .start(sample_input(
                "missing-parent:agent-orphan",
                "missing-parent",
            ))
            .await
            .unwrap();

        tokio::fs::write(store.root().join("bad.json"), "not-json")
            .await
            .unwrap();
        let transcript_dir = store.root().join("transcripts");
        tokio::fs::create_dir_all(&transcript_dir).await.unwrap();
        tokio::fs::write(transcript_dir.join("orphan.jsonl"), "not-json\n")
            .await
            .unwrap();

        let known_parents = HashSet::from(["parent-1".to_string()]);
        let health = store.storage_health(&known_parents, None).await.unwrap();

        assert_eq!(health.metadata_records, 3);
        assert_eq!(health.transcript_records, 2);
        assert_eq!(health.corrupt_metadata_records, 1);
        assert_eq!(health.corrupt_transcripts, 1);
        assert_eq!(health.orphan_metadata_records, 1);
        assert_eq!(health.orphan_transcripts, 1);
        assert_eq!(health.workflow_children_without_transcripts, 1);
    }

    #[tokio::test]
    async fn storage_health_can_scope_child_metadata_by_cwd() {
        let temp = tempdir().unwrap();
        let store = ChildSessionStore::new(temp.path().to_path_buf());
        let mut in_scope = sample_input("parent-1:agent-in", "parent-1");
        in_scope.cwd = "/tmp/in-scope".to_string();
        store.start(in_scope).await.unwrap();
        write_child_transcript(&store, "parent-1:agent-in").await;

        let mut out_of_scope = sample_input("missing-parent:agent-out", "missing-parent");
        out_of_scope.cwd = "/tmp/out-of-scope".to_string();
        store.start(out_of_scope).await.unwrap();
        write_child_transcript(&store, "missing-parent:agent-out").await;

        let known_parents = HashSet::from(["parent-1".to_string()]);
        let scoped_cwds = HashSet::from(["/tmp/in-scope".to_string()]);
        let health = store
            .storage_health(&known_parents, Some(&scoped_cwds))
            .await
            .unwrap();

        assert_eq!(health.metadata_records, 1);
        assert_eq!(health.transcript_records, 1);
        assert_eq!(health.orphan_metadata_records, 0);
        assert_eq!(health.orphan_transcripts, 0);
    }

    #[tokio::test]
    async fn finalize_is_noop_when_metadata_absent() {
        let temp = tempdir().unwrap();
        let store = ChildSessionStore::new(temp.path().to_path_buf());
        // Should not error even if the child never started.
        store.complete("missing-id").await.unwrap();
        store.fail("missing-id", "oops").await.unwrap();
        store.cancel("missing-id").await.unwrap();
        assert!(store.load("missing-id").await.unwrap().is_none());
    }

    #[test]
    fn sanitize_replaces_path_separators() {
        assert_eq!(sanitize_session_id("a/b:c"), "a_b_c");
    }

    #[test]
    fn transcript_path_uses_sanitized_child_id() {
        let temp = tempdir().unwrap();
        let store = ChildSessionStore::new(temp.path().to_path_buf());
        assert_eq!(
            store
                .transcript_path_for("parent:workflow/agent")
                .file_name(),
            Some(std::ffi::OsStr::new("parent_workflow_agent.jsonl"))
        );
    }
}
