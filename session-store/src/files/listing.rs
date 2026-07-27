use std::path::{Path, PathBuf};

use chrono::Utc;
use orbcode_protocol::SessionRecord;

use super::{
    TranscriptFileStore, deduplicate_sessions_by_id, deduplicate_summaries_by_id,
    project_dir_matches_any_prefix, project_prefix_index, short_corrupt_reason,
    sort_sessions_newest_first,
};
use crate::{SessionStoreError, transcript::decode_session_transcript_with_outcome};

impl TranscriptFileStore {
    pub async fn load_session(&self, session_id: &str) -> Result<SessionRecord, SessionStoreError> {
        let path = self.path(session_id);
        if !tokio::fs::try_exists(&path).await? {
            return Err(SessionStoreError::SessionNotFound(session_id.to_string()));
        }
        self.load_session_from_path_as(session_id, &path).await
    }

    pub async fn load_session_if_present(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionRecord>, SessionStoreError> {
        let path = self.path(session_id);
        if tokio::fs::try_exists(path).await? {
            self.load_session(session_id).await.map(Some)
        } else {
            Ok(None)
        }
    }

    pub async fn load_project_sessions(&self) -> Result<Vec<SessionRecord>, SessionStoreError> {
        self.load_sessions_from_project_dirs(vec![self.current_project_dir.clone()])
            .await
    }

    pub async fn load_project_sessions_for_prefixes(
        &self,
        prefixes: &[String],
    ) -> Result<Vec<SessionRecord>, SessionStoreError> {
        let dirs = self.project_dirs_for_prefixes(prefixes).await?;
        self.load_sessions_from_project_dirs(dirs).await
    }

    async fn load_sessions_from_project_dirs(
        &self,
        project_dirs: Vec<PathBuf>,
    ) -> Result<Vec<SessionRecord>, SessionStoreError> {
        let mut sessions = Vec::new();

        for project_dir in project_dirs {
            tokio::fs::create_dir_all(&project_dir).await?;
            let mut dir = tokio::fs::read_dir(&project_dir).await?;
            while let Some(entry) = dir.next_entry().await? {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Ok(session) = self.load_session_from_path(&path).await {
                    sessions.push(session);
                }
            }
        }

        Ok(deduplicate_sessions_by_id(sessions))
    }

    pub async fn load_session_any_project(
        &self,
        session_id: &str,
    ) -> Result<SessionRecord, SessionStoreError> {
        match self.load_session(session_id).await {
            Ok(session) => return Ok(session),
            Err(SessionStoreError::SessionNotFound(_)) => {}
            Err(error) => return Err(error),
        }

        let mut dir = match tokio::fs::read_dir(&self.projects_dir).await {
            Ok(dir) => dir,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SessionStoreError::SessionNotFound(session_id.to_string()));
            }
            Err(error) => return Err(error.into()),
        };
        let mut candidates = Vec::new();
        while let Some(entry) = dir.next_entry().await? {
            let file_type = entry.file_type().await?;
            if !file_type.is_dir() {
                continue;
            }
            let path = entry.path().join(format!("{session_id}.jsonl"));
            let Ok(metadata) = tokio::fs::metadata(&path).await else {
                continue;
            };
            let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
            candidates.push((modified, path));
        }
        candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));

        let Some((_, path)) = candidates.into_iter().next() else {
            return Err(SessionStoreError::SessionNotFound(session_id.to_string()));
        };
        self.load_session_from_path(&path).await
    }

    pub async fn load_latest_project_session(&self) -> Result<SessionRecord, SessionStoreError> {
        let mut sessions = self.load_project_sessions().await?;
        sort_sessions_newest_first(&mut sessions);
        sessions
            .into_iter()
            .next()
            .ok_or_else(|| SessionStoreError::SessionNotFound("latest".to_string()))
    }

    pub async fn load_latest_project_session_for_prefixes(
        &self,
        prefixes: &[String],
    ) -> Result<SessionRecord, SessionStoreError> {
        let mut sessions = self.load_project_sessions_for_prefixes(prefixes).await?;
        sort_sessions_newest_first(&mut sessions);
        sessions
            .into_iter()
            .next()
            .ok_or_else(|| SessionStoreError::SessionNotFound("latest".to_string()))
    }

    /// Like [`Self::load_project_sessions`] but includes a [`SessionSummary`]
    /// for every `.jsonl` file in the project directory — including
    /// transcripts that fail to decode. Failed entries are surfaced with
    /// [`SessionStatus::Corrupt`] so the picker can show them and let the
    /// user delete or skip them instead of silently dropping them.
    pub async fn load_project_session_summaries(
        &self,
    ) -> Result<Vec<orbcode_protocol::SessionSummary>, SessionStoreError> {
        self.load_session_summaries_from_project_dirs(vec![self.current_project_dir.clone()])
            .await
    }

    pub async fn load_project_session_summaries_for_prefixes(
        &self,
        prefixes: &[String],
    ) -> Result<Vec<orbcode_protocol::SessionSummary>, SessionStoreError> {
        let dirs = self.project_dirs_for_prefixes(prefixes).await?;
        self.load_session_summaries_from_project_dirs(dirs).await
    }

    pub(crate) async fn project_dirs_for_prefixes(
        &self,
        prefixes: &[String],
    ) -> Result<Vec<PathBuf>, SessionStoreError> {
        if prefixes.is_empty() {
            return Ok(vec![self.current_project_dir.clone()]);
        }

        let mut dir = match tokio::fs::read_dir(&self.projects_dir).await {
            Ok(dir) => dir,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(vec![self.current_project_dir.clone()]);
            }
            Err(error) => return Err(error.into()),
        };
        let indexed = project_prefix_index(prefixes);
        let mut dirs = Vec::new();
        while let Some(entry) = dir.next_entry().await? {
            let file_type = entry.file_type().await?;
            if !file_type.is_dir() {
                continue;
            }
            let Some(dir_name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if project_dir_matches_any_prefix(&dir_name, &indexed) {
                let path = entry.path();
                if !dirs.iter().any(|existing| existing == &path) {
                    dirs.push(path);
                }
            }
        }
        if dirs.is_empty() {
            dirs.push(self.current_project_dir.clone());
        }
        Ok(dirs)
    }

    async fn load_session_summaries_from_project_dirs(
        &self,
        project_dirs: Vec<PathBuf>,
    ) -> Result<Vec<orbcode_protocol::SessionSummary>, SessionStoreError> {
        use super::session_index;
        use orbcode_protocol::{SessionStatus, SessionSummary};

        let mut summaries = Vec::new();
        for project_dir in project_dirs {
            tokio::fs::create_dir_all(&project_dir).await?;
            let mut index = session_index::load_index(&project_dir).await;
            let mut index_dirty = false;
            let mut seen_ids = Vec::new();
            let mut dir = tokio::fs::read_dir(&project_dir).await?;

            while let Some(entry) = dir.next_entry().await? {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }
                let Some(session_id) = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                seen_ids.push(session_id.clone());
                let transcript_path = path.display().to_string();

                let metadata = tokio::fs::metadata(&path).await.ok();
                let file_mtime = metadata.as_ref().and_then(|m| m.modified().ok());
                let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

                if let Some(mtime) = file_mtime
                    && index.is_fresh(&session_id, mtime, file_size)
                    && let Some(cached) = index.get(&session_id)
                {
                    let mut summary = cached.clone();
                    summary.transcript_path = Some(transcript_path);
                    summaries.push(summary);
                    continue;
                }

                match self.load_session_from_path(&path).await {
                    Ok(session) => {
                        let mut summary = session.summary();
                        summary.transcript_path = Some(transcript_path);
                        if let Some(mtime) = file_mtime {
                            index.upsert(session_id, summary.clone(), mtime, file_size);
                            index_dirty = true;
                        }
                        summaries.push(summary);
                    }
                    Err(error) => {
                        let mtime_dt = file_mtime
                            .and_then(|modified| {
                                chrono::DateTime::<chrono::Utc>::from_timestamp(
                                    modified
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .ok()?
                                        .as_secs() as i64,
                                    0,
                                )
                            })
                            .unwrap_or_else(Utc::now);
                        let summary = SessionSummary {
                            session_id: session_id.clone(),
                            title: None,
                            custom_title: None,
                            message_count: 0,
                            created_at: mtime_dt,
                            updated_at: mtime_dt,
                            cwd: None,
                            git_branch: None,
                            model: None,
                            provider: None,
                            transcript_path: Some(transcript_path),
                            status: SessionStatus::Corrupt {
                                reason: short_corrupt_reason(&error),
                            },
                            total_input_tokens: 0,
                            total_output_tokens: 0,
                            duration_secs: None,
                        };
                        // Never persist a `Corrupt` verdict for a transient
                        // read error (EINTR/EMFILE/disk hiccup). Caching it
                        // would make `is_fresh` short-circuit every later
                        // listing to the same stale `Corrupt` result, and
                        // `gc_stale_sessions` would then delete the intact
                        // transcript. Genuine parse/corruption errors are
                        // still cached.
                        if let Some(mtime) = file_mtime
                            && !error.is_transient()
                        {
                            index.upsert(session_id, summary.clone(), mtime, file_size);
                            index_dirty = true;
                        }
                        summaries.push(summary);
                    }
                }
            }

            let seen_refs: Vec<&str> = seen_ids.iter().map(String::as_str).collect();
            let before = index.entries_len();
            index.retain_session_ids(&seen_refs);
            if index.entries_len() != before {
                index_dirty = true;
            }
            if index_dirty {
                session_index::save_index(&project_dir, &index).await;
            }
        }
        Ok(deduplicate_summaries_by_id(summaries))
    }

    pub async fn load_session_from_path(
        &self,
        path: &Path,
    ) -> Result<SessionRecord, SessionStoreError> {
        let session_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| SessionStoreError::Config("invalid transcript filename".into()))?
            .to_string();
        self.load_session_from_path_as(&session_id, path).await
    }

    pub async fn load_session_from_path_as(
        &self,
        session_id: &str,
        path: &Path,
    ) -> Result<SessionRecord, SessionStoreError> {
        let contents = tokio::fs::read_to_string(path)
            .await
            .map_err(|error| SessionStoreError::transcript_io("read transcript", path, error))?;
        let outcome = decode_session_transcript_with_outcome(session_id.to_string(), &contents);
        let session = outcome
            .session
            .ok_or_else(|| SessionStoreError::SessionNotFound(session_id.to_string()))?;
        self.record_session_path(session_id, path);
        self.record_session_cwd(session_id, session.cwd.as_deref());
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_protocol::{MessageRole, TranscriptMessage};

    #[tokio::test]
    async fn load_session_reads_existing_transcript() {
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

        let session = store.load_session("session-1").await.expect("load session");

        assert_eq!(session.session_id, "session-1");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "hello");
    }

    #[tokio::test]
    async fn load_session_if_present_distinguishes_missing_from_invalid() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        assert!(
            store
                .load_session_if_present("missing")
                .await
                .expect("missing transcript is ok")
                .is_none()
        );

        tokio::fs::write(temp.path().join("invalid.jsonl"), "\n")
            .await
            .expect("write invalid transcript");
        assert!(matches!(
            store.load_session_if_present("invalid").await,
            Err(SessionStoreError::SessionNotFound(session_id)) if session_id == "invalid"
        ));
    }

    #[tokio::test]
    async fn load_project_sessions_skips_invalid_transcripts() {
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
        tokio::fs::write(temp.path().join("invalid.jsonl"), "\n")
            .await
            .expect("write invalid transcript");
        tokio::fs::write(temp.path().join("notes.txt"), "ignored")
            .await
            .expect("write non-transcript");

        let sessions = store
            .load_project_sessions()
            .await
            .expect("load project sessions");

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "session-1");
    }

    #[tokio::test]
    async fn load_project_sessions_for_prefixes_includes_same_repo_worktree_dirs() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let projects = temp.path().join("projects");
        let main = projects.join("repo");
        let linked = projects.join("repo-linked");
        let other = projects.join("other");
        tokio::fs::create_dir_all(&main).await.expect("main dir");
        tokio::fs::create_dir_all(&linked)
            .await
            .expect("linked dir");
        tokio::fs::create_dir_all(&other).await.expect("other dir");
        let store = TranscriptFileStore::new(
            main.clone(),
            PathBuf::from("/tmp/repo"),
            "claude-sonnet-4".to_string(),
        );

        let write_user = |dir: PathBuf,
                          session_id: &'static str,
                          timestamp: &'static str,
                          content: &'static str| async move {
            let payload = serde_json::to_string(&serde_json::json!({
                "type": "user",
                "uuid": format!("{session_id}-user"),
                "timestamp": timestamp,
                "message": { "role": "user", "content": content },
                "cwd": format!("/tmp/{session_id}"),
                "sessionId": session_id,
            }))
            .expect("serialize transcript");
            tokio::fs::write(
                dir.join(format!("{session_id}.jsonl")),
                format!("{payload}\n"),
            )
            .await
            .expect("write transcript");
        };
        write_user(
            main.clone(),
            "shared-session",
            "2026-04-10T00:00:00.000Z",
            "main copy",
        )
        .await;
        write_user(
            linked.clone(),
            "linked-session",
            "2026-04-10T00:00:03.000Z",
            "linked copy",
        )
        .await;
        write_user(
            linked.clone(),
            "shared-session",
            "2026-04-10T00:00:05.000Z",
            "newer linked copy",
        )
        .await;
        write_user(
            other,
            "other-session",
            "2026-04-10T00:00:10.000Z",
            "other copy",
        )
        .await;
        tokio::fs::write(linked.join("broken-session.jsonl"), "not-json\n")
            .await
            .expect("write corrupt transcript");

        let sessions = store
            .load_project_sessions_for_prefixes(&["repo".to_string()])
            .await
            .expect("load same repo sessions");
        let session_ids = sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(session_ids, vec!["shared-session", "linked-session"]);
        assert_eq!(sessions[0].messages[0].content, "newer linked copy");

        let latest = store
            .load_latest_project_session_for_prefixes(&["repo".to_string()])
            .await
            .expect("load latest same repo session");
        assert_eq!(latest.session_id, "shared-session");

        let summaries = store
            .load_project_session_summaries_for_prefixes(&["repo".to_string()])
            .await
            .expect("load same repo summaries");
        assert!(summaries.iter().any(|summary| {
            summary.session_id == "broken-session"
                && matches!(
                    summary.status,
                    orbcode_protocol::SessionStatus::Corrupt { .. }
                )
        }));
        assert!(
            !summaries
                .iter()
                .any(|summary| summary.session_id == "other-session")
        );
    }

    #[tokio::test]
    async fn load_project_session_summaries_includes_corrupt_entries() {
        use orbcode_protocol::SessionStatus;

        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );
        store
            .append_message_line(
                "good-session",
                &TranscriptMessage::new(MessageRole::User, "hello"),
                None,
            )
            .await
            .expect("append good session");
        tokio::fs::write(temp.path().join("bad-session.jsonl"), "not-json\n")
            .await
            .expect("write corrupt transcript");

        let summaries = store
            .load_project_session_summaries()
            .await
            .expect("list summaries");

        assert_eq!(summaries.len(), 2);
        let bad = summaries
            .iter()
            .find(|summary| summary.session_id == "bad-session")
            .expect("corrupt summary present");
        assert!(matches!(bad.status, SessionStatus::Corrupt { .. }));
        assert!(bad.transcript_path.is_some());
        let good = summaries
            .iter()
            .find(|summary| summary.session_id == "good-session")
            .expect("good summary present");
        assert!(matches!(good.status, SessionStatus::Available));
        assert!(good.transcript_path.as_deref().is_some());
    }

    #[tokio::test]
    async fn enriched_summary_has_token_counts_and_duration() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        let user_ts = "2026-05-01T10:00:00.000Z";
        let asst_ts = "2026-05-01T10:05:00.000Z";
        let user_entry = serde_json::to_string(&serde_json::json!({
            "type": "user",
            "uuid": "u1",
            "timestamp": user_ts,
            "message": { "role": "user", "content": "hello" },
            "sessionId": "enriched",
            "cwd": "/tmp",
        }))
        .expect("serialize user");
        let asst_entry = serde_json::to_string(&serde_json::json!({
            "type": "assistant",
            "uuid": "a1",
            "parentUuid": "u1",
            "timestamp": asst_ts,
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "hi"}],
                "model": "claude-sonnet-4",
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 50,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                    "total_tokens": 150
                }
            },
            "sessionId": "enriched",
            "cwd": "/tmp",
        }))
        .expect("serialize assistant");
        tokio::fs::write(
            temp.path().join("enriched.jsonl"),
            format!("{user_entry}\n{asst_entry}\n"),
        )
        .await
        .expect("write transcript");

        let summaries = store
            .load_project_session_summaries()
            .await
            .expect("load summaries");
        let summary = summaries
            .iter()
            .find(|s| s.session_id == "enriched")
            .expect("find enriched summary");

        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.total_input_tokens, 100);
        assert_eq!(summary.total_output_tokens, 50);
        assert!(summary.duration_secs.is_some());
        assert_eq!(summary.duration_secs.unwrap(), 300);
    }

    #[tokio::test]
    async fn summaries_sorted_by_updated_at_desc_with_id_tiebreaker() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        let write_session = |id: &'static str, ts: &'static str| {
            let dir = temp.path().to_path_buf();
            async move {
                let payload = serde_json::to_string(&serde_json::json!({
                    "type": "user",
                    "uuid": format!("{id}-u"),
                    "timestamp": ts,
                    "message": { "role": "user", "content": format!("msg {id}") },
                    "sessionId": id,
                    "cwd": "/tmp",
                }))
                .expect("serialize");
                tokio::fs::write(dir.join(format!("{id}.jsonl")), format!("{payload}\n"))
                    .await
                    .expect("write");
            }
        };

        // Same timestamp for aaa and bbb to test tiebreaker
        write_session("bbb-session", "2026-01-15T10:00:00.000Z").await;
        write_session("aaa-session", "2026-01-15T10:00:00.000Z").await;
        write_session("ccc-session", "2026-01-20T10:00:00.000Z").await;

        let summaries = store.load_project_session_summaries().await.expect("load");

        let ids: Vec<&str> = summaries.iter().map(|s| s.session_id.as_str()).collect();
        // Newest first, then alphabetical for ties
        assert_eq!(ids, vec!["ccc-session", "aaa-session", "bbb-session"]);
    }

    #[tokio::test]
    async fn warm_index_serves_summaries_without_full_decode() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        store
            .append_message_line(
                "indexed-session",
                &TranscriptMessage::new(MessageRole::User, "first message"),
                None,
            )
            .await
            .expect("append");

        let first_load = store
            .load_project_session_summaries()
            .await
            .expect("first load");
        assert_eq!(first_load.len(), 1);
        assert_eq!(first_load[0].session_id, "indexed-session");
        assert_eq!(first_load[0].message_count, 1);

        let index_path = super::super::session_index::index_path(temp.path());
        assert!(index_path.exists(), "index file should be written");

        let second_load = store
            .load_project_session_summaries()
            .await
            .expect("second load (warm)");
        assert_eq!(second_load.len(), 1);
        assert_eq!(second_load[0].session_id, "indexed-session");
        assert_eq!(second_load[0].message_count, 1);
    }

    #[tokio::test]
    async fn index_detects_stale_entry_after_transcript_append() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        store
            .append_message_line(
                "grow",
                &TranscriptMessage::new(MessageRole::User, "msg 1"),
                None,
            )
            .await
            .expect("append 1");

        let first = store.load_project_session_summaries().await.expect("first");
        assert_eq!(first[0].message_count, 1);

        store
            .append_message_line(
                "grow",
                &TranscriptMessage::new(MessageRole::Assistant, "reply"),
                None,
            )
            .await
            .expect("append 2");

        let second = store
            .load_project_session_summaries()
            .await
            .expect("second");
        assert_eq!(second[0].message_count, 2);
    }

    #[tokio::test]
    async fn index_handles_deleted_transcript() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        store
            .append_message_line(
                "ephemeral",
                &TranscriptMessage::new(MessageRole::User, "temp"),
                None,
            )
            .await
            .expect("append");

        let _ = store
            .load_project_session_summaries()
            .await
            .expect("populate index");

        store
            .remove_session_file_if_exists("ephemeral")
            .await
            .expect("delete");

        let after = store
            .load_project_session_summaries()
            .await
            .expect("after delete");
        assert!(after.is_empty());
    }

    #[tokio::test]
    async fn corrupt_index_falls_back_to_full_scan() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        store
            .append_message_line(
                "survives",
                &TranscriptMessage::new(MessageRole::User, "ok"),
                None,
            )
            .await
            .expect("append");

        let index_path = super::super::session_index::index_path(temp.path());
        tokio::fs::write(&index_path, "NOT JSON")
            .await
            .expect("corrupt index");

        let summaries = store
            .load_project_session_summaries()
            .await
            .expect("fallback load");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].session_id, "survives");
    }
}
