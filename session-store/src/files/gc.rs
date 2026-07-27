use std::path::PathBuf;

use chrono::Utc;
use uuid::Uuid;

use super::TranscriptFileStore;
use crate::{
    ChildSessionStorageHealth, SessionStoreError,
    transcript::decode_session_transcript_with_outcome,
};

/// Lightweight summary of transcript storage health, scoped to one or
/// more project directories. Surfaced by the `doctor` command so users
/// can spot disk-full, corrupt-transcript, and stray-tmp problems
/// before they start losing data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionStorageHealth {
    pub project_dir: PathBuf,
    pub total_transcripts: usize,
    pub corrupt_transcripts: usize,
    pub recoverable_transcripts: usize,
    pub trailing_partial_lines: usize,
    pub stray_tmp_files: usize,
    pub writable: bool,
    pub write_probe_error: Option<String>,
    pub child_sessions: ChildSessionStorageHealth,
    pub child_session_scan_error: Option<String>,
}

/// Result returned by [`TranscriptFileStore::gc_stale_sessions`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcResult {
    pub inspected: usize,
    pub removed: usize,
    pub removed_ids: Vec<String>,
    pub removed_child_metadata: usize,
    pub removed_child_transcripts: usize,
}

impl TranscriptFileStore {
    /// Scans `current_project_dir` and returns a [`SessionStorageHealth`]
    /// snapshot. The scan never panics on a corrupt or unreadable
    /// transcript — those are tallied so the caller can surface a recovery
    /// hint instead of aborting startup.
    pub async fn storage_health(&self) -> SessionStorageHealth {
        let project_dir = self.current_project_dir.clone();
        let mut health = SessionStorageHealth {
            project_dir: project_dir.clone(),
            ..SessionStorageHealth::default()
        };

        if let Err(error) = tokio::fs::create_dir_all(&project_dir).await {
            health.write_probe_error = Some(format!("create project dir: {error}"));
            return health;
        }

        let mut dir = match tokio::fs::read_dir(&project_dir).await {
            Ok(dir) => dir,
            Err(error) => {
                health.write_probe_error = Some(format!("read project dir: {error}"));
                return health;
            }
        };

        while let Ok(Some(entry)) = dir.next_entry().await {
            let path = entry.path();
            let extension = path.extension().and_then(|ext| ext.to_str());
            match extension {
                Some("tmp") => {
                    if path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.contains(".jsonl."))
                    {
                        health.stray_tmp_files += 1;
                    }
                }
                Some("jsonl") => {
                    health.total_transcripts += 1;
                    let contents = match tokio::fs::read_to_string(&path).await {
                        Ok(value) => value,
                        Err(_) => {
                            health.corrupt_transcripts += 1;
                            continue;
                        }
                    };
                    let session_id = path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("")
                        .to_string();
                    let outcome = decode_session_transcript_with_outcome(session_id, &contents);
                    if outcome.session.is_none() {
                        health.corrupt_transcripts += 1;
                    } else if outcome.skipped_line_count > 0 || outcome.trailing_partial_line {
                        health.recoverable_transcripts += 1;
                    }
                    if outcome.trailing_partial_line {
                        health.trailing_partial_lines += 1;
                    }
                }
                _ => {}
            }
        }

        let probe = project_dir.join(format!(".orbcode-doctor-{}.probe", Uuid::new_v4()));
        match tokio::fs::write(&probe, b"ok").await {
            Ok(()) => {
                let _ = tokio::fs::remove_file(&probe).await;
                health.writable = true;
            }
            Err(error) => {
                health.write_probe_error = Some(format!("{error}"));
            }
        }

        health
    }

    /// Deletes stale session transcripts from `current_project_dir`.
    ///
    /// A session is eligible for GC when its `updated_at` is older than
    /// `threshold_days` **and** it is either empty (`message_count == 0`)
    /// or corrupt. Sessions with content are always preserved.
    pub async fn gc_stale_sessions(
        &self,
        threshold_days: u64,
    ) -> Result<GcResult, SessionStoreError> {
        use orbcode_protocol::SessionStatus;

        let cutoff = Utc::now() - chrono::Duration::days(threshold_days as i64);
        let summaries = self.load_project_session_summaries().await?;

        let mut result = GcResult::default();
        for summary in &summaries {
            result.inspected += 1;
            if summary.updated_at >= cutoff {
                continue;
            }
            let eligible = summary.message_count == 0
                || matches!(summary.status, SessionStatus::Corrupt { .. });
            if !eligible {
                continue;
            }
            if let Some(path) = &summary.transcript_path {
                // A `Corrupt` verdict can originate from a transient read
                // error that was cached in the index. Re-decode before
                // deleting so a one-time blip never permanently loses a
                // healthy transcript. A session that still fails to read
                // transiently is preserved; genuine corruption (or a
                // now-empty transcript) falls through to removal.
                if matches!(summary.status, SessionStatus::Corrupt { .. }) {
                    match self
                        .load_session_from_path(std::path::Path::new(path))
                        .await
                    {
                        Ok(session) if !session.messages.is_empty() => continue,
                        Ok(_) => {}
                        Err(error) if error.is_transient() => continue,
                        Err(_) => {}
                    }
                }
                match tokio::fs::remove_file(path).await {
                    Ok(()) => {
                        result.removed += 1;
                        result.removed_ids.push(summary.session_id.clone());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_protocol::{MessageRole, TranscriptMessage};
    use serde_json::json;

    #[tokio::test]
    async fn storage_health_counts_corrupt_partial_and_tmp_artifacts() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        // good transcript
        store
            .append_message_line(
                "good",
                &TranscriptMessage::new(MessageRole::User, "hello"),
                None,
            )
            .await
            .expect("append good");
        // corrupt transcript — entirely unparseable
        tokio::fs::write(temp.path().join("corrupt.jsonl"), "not-json\n")
            .await
            .expect("write corrupt");
        // recoverable transcript: one valid line, one truncated trailing line
        let recoverable = serde_json::to_string(&json!({
            "type": "user",
            "uuid": "user-1",
            "timestamp": "2026-05-23T01:00:00.000Z",
            "message": { "role": "user", "content": "first" },
            "sessionId": "recoverable",
            "cwd": "/tmp",
        }))
        .expect("serialize line");
        tokio::fs::write(
            temp.path().join("recoverable.jsonl"),
            format!("{recoverable}\n{{\"type\":\"user\""),
        )
        .await
        .expect("write recoverable");
        // stray tmp file from a crashed atomic write
        tokio::fs::write(temp.path().join("session-x.jsonl.deadbeef.tmp"), "leftover")
            .await
            .expect("write stray tmp");

        let health = store.storage_health().await;
        assert_eq!(health.total_transcripts, 3);
        assert_eq!(health.corrupt_transcripts, 1);
        assert_eq!(health.recoverable_transcripts, 1);
        assert_eq!(health.trailing_partial_lines, 1);
        assert_eq!(health.stray_tmp_files, 1);
        assert!(health.writable);
        assert!(health.write_probe_error.is_none());
    }

    fn set_file_mtime_old(path: &std::path::Path) {
        use std::fs;
        use std::time::{Duration, SystemTime};
        let old_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_577_836_800); // 2020-01-01
        let times = fs::FileTimes::new().set_modified(old_time);
        let file = fs::File::options()
            .write(true)
            .open(path)
            .expect("open for mtime");
        file.set_times(times).expect("set mtime");
    }

    #[tokio::test]
    async fn gc_stale_sessions_removes_empty_old_sessions() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        // Empty session (old): write a transcript with only whitespace / no messages
        let empty_path = temp.path().join("empty-old.jsonl");
        tokio::fs::write(&empty_path, "\n")
            .await
            .expect("write empty session");
        set_file_mtime_old(&empty_path);

        // Session with content (old): has a real message with old timestamp
        let old_ts = "2020-01-01T00:00:00.000Z";
        let payload = serde_json::to_string(&json!({
            "type": "user",
            "uuid": "msg-1",
            "timestamp": old_ts,
            "message": { "role": "user", "content": "hello world" },
            "sessionId": "has-content",
            "cwd": "/tmp",
        }))
        .expect("serialize");
        let content_path = temp.path().join("has-content.jsonl");
        tokio::fs::write(&content_path, format!("{payload}\n"))
            .await
            .expect("write session with content");
        set_file_mtime_old(&content_path);

        // Corrupt session (old)
        let corrupt_path = temp.path().join("corrupt-old.jsonl");
        tokio::fs::write(&corrupt_path, "not-json\n")
            .await
            .expect("write corrupt session");
        set_file_mtime_old(&corrupt_path);

        // Recent empty session (should NOT be removed)
        store
            .append_message_line(
                "recent-session",
                &TranscriptMessage::new(MessageRole::User, "fresh"),
                None,
            )
            .await
            .expect("append recent");

        let result = store.gc_stale_sessions(1).await.expect("gc");

        assert_eq!(result.removed, 2);
        assert!(result.removed_ids.contains(&"empty-old".to_string()));
        assert!(result.removed_ids.contains(&"corrupt-old".to_string()));
        assert!(!result.removed_ids.contains(&"has-content".to_string()));
        assert!(!result.removed_ids.contains(&"recent-session".to_string()));

        assert!(!temp.path().join("empty-old.jsonl").exists());
        assert!(!temp.path().join("corrupt-old.jsonl").exists());
        assert!(temp.path().join("has-content.jsonl").exists());
        assert!(temp.path().join("recent-session.jsonl").exists());
    }

    #[tokio::test]
    async fn gc_redecodes_before_deleting_transiently_corrupt_session() {
        use crate::files::session_index::{self, SessionIndex};
        use orbcode_protocol::{SessionStatus, SessionSummary};

        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        // A healthy transcript with real content, but with an old mtime so it
        // is a GC candidate by age.
        let old_ts = "2020-01-01T00:00:00.000Z";
        let payload = serde_json::to_string(&json!({
            "type": "user",
            "uuid": "msg-1",
            "timestamp": old_ts,
            "message": { "role": "user", "content": "important work" },
            "sessionId": "transient",
            "cwd": "/tmp",
        }))
        .expect("serialize");
        let path = temp.path().join("transient.jsonl");
        let bytes = format!("{payload}\n");
        tokio::fs::write(&path, &bytes)
            .await
            .expect("write healthy session");
        set_file_mtime_old(&path);

        // Simulate a previously-cached `Corrupt` verdict from a transient read
        // error: poison the on-disk index so the warm-cache path serves the
        // Corrupt summary without re-decoding.
        let mtime = std::fs::metadata(&path)
            .expect("metadata")
            .modified()
            .expect("mtime");
        let mut index = SessionIndex::new_for_test();
        index.upsert(
            "transient".to_string(),
            SessionSummary {
                session_id: "transient".to_string(),
                title: None,
                custom_title: None,
                message_count: 0,
                created_at: Utc::now(),
                updated_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1_577_836_800, 0)
                    .unwrap(),
                cwd: None,
                git_branch: None,
                model: None,
                provider: None,
                transcript_path: Some(path.display().to_string()),
                status: SessionStatus::Corrupt {
                    reason: "io: simulated transient blip".to_string(),
                },
                total_input_tokens: 0,
                total_output_tokens: 0,
                duration_secs: None,
            },
            mtime,
            bytes.len() as u64,
        );
        session_index::save_index(temp.path(), &index).await;

        let result = store.gc_stale_sessions(1).await.expect("gc");

        assert_eq!(
            result.removed, 0,
            "healthy transcript must survive re-decode"
        );
        assert!(!result.removed_ids.contains(&"transient".to_string()));
        assert!(
            temp.path().join("transient.jsonl").exists(),
            "transcript with content was deleted despite re-decode guard"
        );
    }

    #[tokio::test]
    async fn gc_stale_sessions_preserves_recent_empty_sessions() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = TranscriptFileStore::new(
            temp.path().to_path_buf(),
            PathBuf::from("/tmp/project"),
            "claude-sonnet-4".to_string(),
        );

        // Write a corrupt transcript that will get a recent mtime
        tokio::fs::write(temp.path().join("recent-corrupt.jsonl"), "bad\n")
            .await
            .expect("write corrupt");

        let result = store.gc_stale_sessions(30).await.expect("gc");
        assert_eq!(result.removed, 0);
        assert!(temp.path().join("recent-corrupt.jsonl").exists());
    }
}
