mod events;
mod lifecycle;
mod output;

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use orbcode_config::PermissionMode;
use orbcode_protocol::ProviderId;
use serde::{Deserialize, Serialize};

use orbcode_core::CoreError;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Orphaned,
    /// Catch-all for status strings this build does not recognize (forward
    /// compatibility) and for jobs whose real state can no longer be determined.
    #[serde(other)]
    Unknown,
}

impl fmt::Display for BackgroundJobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Orphaned => "orphaned",
            Self::Unknown => "unknown",
        };
        f.write_str(value)
    }
}

impl BackgroundJobStatus {
    /// A job that may still be making progress and is worth polling.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    /// A job that has reached a final, persisted resting state.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Orphaned
        )
    }

    /// Whether moving from `self` to `next` is a legal lifecycle transition.
    /// Staying in the same state is always permitted so that idempotent
    /// re-marks (e.g. a worker re-asserting `Running`) are not rejected.
    pub fn can_transition_to(self, next: Self) -> bool {
        use BackgroundJobStatus::*;
        if self == next {
            return true;
        }
        match self {
            Queued => matches!(next, Running | Failed | Cancelled | Unknown),
            Running => matches!(next, Completed | Failed | Cancelled | Orphaned | Unknown),
            Completed | Failed | Cancelled | Orphaned => false,
            Unknown => matches!(
                next,
                Queued | Running | Completed | Failed | Cancelled | Orphaned
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskRetrievalStatus {
    Success,
    Timeout,
    NotReady,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackgroundTaskProgress {
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub pid: Option<u32>,
    pub log_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProgressSummary {
    pub last_lines: Vec<String>,
    pub elapsed_ms: i64,
    pub output_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackgroundTaskOutput {
    pub task_id: String,
    pub task_type: String,
    pub status: BackgroundJobStatus,
    pub description: String,
    pub output: String,
    pub output_path: String,
    pub error: Option<String>,
    pub result: Option<String>,
    pub progress: BackgroundTaskProgress,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_summary: Option<ProgressSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackgroundTaskOutputResponse {
    pub retrieval_status: BackgroundTaskRetrievalStatus,
    pub task: Option<BackgroundTaskOutput>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackgroundJobRecord {
    pub job_id: String,
    pub session_id: String,
    pub prompt: String,
    pub cwd: String,
    pub provider: ProviderId,
    pub fallback_provider: Option<ProviderId>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    pub status: BackgroundJobStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub pid: Option<u32>,
    pub log_path: String,
    pub error: Option<String>,
    /// Process exit code captured for the job, when known.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Terminating signal number captured for the job, when known.
    #[serde(default)]
    pub signal: Option<i32>,
    /// Byte length of the log at the last persisted lifecycle update; a
    /// follower can resume reading the log file from here.
    #[serde(default)]
    pub last_log_offset: u64,
    /// Human-readable reason a job was cancelled, distinct from `error`.
    #[serde(default)]
    pub cancellation_reason: Option<String>,
}

impl BackgroundJobRecord {
    pub fn summary(&self) -> BackgroundJobSummary {
        BackgroundJobSummary {
            job_id: self.job_id.clone(),
            session_id: self.session_id.clone(),
            prompt: truncate_prompt(&self.prompt),
            provider: self.provider,
            model: self.model.clone(),
            status: self.status,
            created_at: self.created_at,
            updated_at: self.updated_at,
            pid: self.pid,
            exit_code: self.exit_code,
            signal: self.signal,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundJobSummary {
    pub job_id: String,
    pub session_id: String,
    pub prompt: String,
    pub provider: ProviderId,
    pub model: String,
    pub status: BackgroundJobStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

impl BackgroundJobSummary {
    pub fn elapsed_ms(&self) -> i64 {
        let start = self.created_at;
        (Utc::now() - start).num_milliseconds()
    }

    pub fn exit_detail(&self) -> Option<String> {
        if self.status.is_active() {
            return None;
        }
        match (self.exit_code, self.signal) {
            (Some(code), Some(sig)) => Some(format!("exit={code} signal={sig}")),
            (Some(code), None) => Some(format!("exit={code}")),
            (None, Some(sig)) => Some(format!("signal={sig}")),
            (None, None) => None,
        }
    }
}

impl fmt::Display for BackgroundJobSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = self.status;
        let provider = self.provider;
        let model = &self.model;
        let exit = self.exit_detail().unwrap_or_default();
        if model.is_empty() {
            if exit.is_empty() {
                write!(f, "{status} [{provider}]")
            } else {
                write!(f, "{status} [{provider}] ({exit})")
            }
        } else if exit.is_empty() {
            write!(f, "{status} [{provider}/{model}]")
        } else {
            write!(f, "{status} [{provider}/{model}] ({exit})")
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundJobDetail {
    pub job_id: String,
    pub session_id: String,
    pub prompt: String,
    pub cwd: String,
    pub provider: ProviderId,
    pub fallback_provider: Option<ProviderId>,
    pub model: String,
    pub permission_mode: Option<PermissionMode>,
    pub status: BackgroundJobStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub error: Option<String>,
    pub cancellation_reason: Option<String>,
    pub log_tail: Vec<String>,
    pub progress_summary: Option<ProgressSummary>,
    pub elapsed_ms: i64,
}

#[derive(Clone)]
pub struct BackgroundManager {
    pub(super) jobs_dir: PathBuf,
    pub(super) logs_dir: PathBuf,
    pub(super) cancel_tokens: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

#[derive(Clone, Debug)]
pub struct CreateBackgroundJob {
    pub session_id: String,
    pub prompt: String,
    pub cwd: PathBuf,
    pub provider: ProviderId,
    pub fallback_provider: Option<ProviderId>,
    pub model: String,
    pub permission_mode: Option<PermissionMode>,
}

impl BackgroundManager {
    pub fn new(home_dir: PathBuf) -> Self {
        let root_dir = home_dir.join("background");
        let jobs_dir = root_dir.join("jobs");
        let logs_dir = root_dir.join("logs");
        Self {
            jobs_dir,
            logs_dir,
            cancel_tokens: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Read a record straight from disk without running orphan reconciliation.
    /// Mutation helpers and the `task_output` poll loop use this so that
    /// asserting a state (e.g. `Running` with a freshly assigned pid) is never
    /// second-guessed by a concurrent liveness probe.
    pub(super) async fn read_record(&self, job_id: &str) -> Result<BackgroundJobRecord, CoreError> {
        self.ensure_dirs().await?;
        let path = self.job_path(job_id);
        if !tokio::fs::try_exists(&path).await? {
            return Err(CoreError::Config(format!(
                "background job not found: {job_id}"
            )));
        }
        let contents = tokio::fs::read_to_string(path).await?;
        Ok(serde_json::from_str(&contents)?)
    }

    pub(super) async fn save_job(&self, record: &BackgroundJobRecord) -> Result<(), CoreError> {
        self.ensure_dirs().await?;
        let path = self.job_path(&record.job_id);
        let tmp_path = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        tokio::fs::write(&tmp_path, serde_json::to_string_pretty(record)?).await?;
        tokio::fs::rename(tmp_path, path).await?;
        Ok(())
    }

    pub(super) async fn ensure_dirs(&self) -> Result<(), CoreError> {
        tokio::fs::create_dir_all(&self.jobs_dir).await?;
        tokio::fs::create_dir_all(&self.logs_dir).await?;
        Ok(())
    }

    pub(super) fn job_path(&self, job_id: &str) -> PathBuf {
        self.jobs_dir.join(format!("{job_id}.json"))
    }
}

/// Best-effort process liveness probe used by orphan detection. On Unix this
/// shells out to `kill -0 <pid>` (matching the existing `kill -TERM` cancel
/// path); any non-success exit — including `ESRCH`/`EPERM`/out-of-range pid — is
/// treated as "not alive". On non-Unix targets we cannot reliably probe, so we
/// assume the process is alive to avoid falsely orphaning live jobs.
pub(super) async fn process_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use std::process::Stdio;
        tokio::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

pub(super) fn truncate_prompt(prompt: &str) -> String {
    let trimmed = prompt.trim();
    let mut chars = trimmed.chars();
    let short = chars.by_ref().take(48).collect::<String>();
    if chars.next().is_some() {
        format!("{short}...")
    } else if short.is_empty() {
        "empty".to_string()
    } else {
        short
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_and_terminal_classification() {
        use BackgroundJobStatus::*;
        assert!(Queued.is_active());
        assert!(Running.is_active());
        assert!(!Completed.is_active());
        assert!(!Orphaned.is_active());
        assert!(!Unknown.is_active());

        assert!(Completed.is_terminal());
        assert!(Failed.is_terminal());
        assert!(Cancelled.is_terminal());
        assert!(Orphaned.is_terminal());
        assert!(!Queued.is_terminal());
        assert!(!Running.is_terminal());
        assert!(!Unknown.is_terminal());
    }

    #[test]
    fn transition_validation_accepts_legal_and_rejects_illegal() {
        use BackgroundJobStatus::*;
        assert!(Queued.can_transition_to(Running));
        assert!(Queued.can_transition_to(Cancelled));
        assert!(Queued.can_transition_to(Failed));
        assert!(Running.can_transition_to(Completed));
        assert!(Running.can_transition_to(Failed));
        assert!(Running.can_transition_to(Cancelled));
        assert!(Running.can_transition_to(Orphaned));
        assert!(Unknown.can_transition_to(Running));
        assert!(Unknown.can_transition_to(Completed));
        assert!(Running.can_transition_to(Running));
        assert!(Completed.can_transition_to(Completed));

        assert!(!Completed.can_transition_to(Running));
        assert!(!Cancelled.can_transition_to(Running));
        assert!(!Failed.can_transition_to(Completed));
        assert!(!Orphaned.can_transition_to(Running));
        assert!(!Queued.can_transition_to(Completed));
        assert!(!Queued.can_transition_to(Orphaned));
    }

    #[test]
    fn status_serde_round_trip_for_new_variants() {
        for status in [BackgroundJobStatus::Orphaned, BackgroundJobStatus::Unknown] {
            let json = serde_json::to_string(&status).expect("serialize");
            let back: BackgroundJobStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(status, back);
        }
        assert_eq!(
            serde_json::to_string(&BackgroundJobStatus::Orphaned).unwrap(),
            "\"orphaned\""
        );
        assert_eq!(
            serde_json::to_string(&BackgroundJobStatus::Unknown).unwrap(),
            "\"unknown\""
        );
        let parsed: BackgroundJobStatus =
            serde_json::from_str("\"some_future_state\"").expect("deserialize unknown tag");
        assert_eq!(parsed, BackgroundJobStatus::Unknown);
    }

    #[test]
    fn record_deserializes_legacy_json_without_new_fields() {
        let legacy = r#"{
            "job_id": "job-1",
            "session_id": "session-1",
            "prompt": "legacy prompt",
            "cwd": "/tmp",
            "provider": "anthropic",
            "fallback_provider": null,
            "status": "running",
            "created_at": "2026-05-28T00:00:00Z",
            "updated_at": "2026-05-28T00:00:00Z",
            "started_at": null,
            "finished_at": null,
            "pid": null,
            "log_path": "/tmp/job-1.log",
            "error": null
        }"#;
        let record: BackgroundJobRecord =
            serde_json::from_str(legacy).expect("parse legacy record");
        assert_eq!(record.status, BackgroundJobStatus::Running);
        assert_eq!(record.exit_code, None);
        assert_eq!(record.signal, None);
        assert_eq!(record.last_log_offset, 0);
        assert_eq!(record.cancellation_reason, None);
        assert_eq!(record.model, "");
        assert_eq!(record.permission_mode, None);
    }

    fn test_summary(status: BackgroundJobStatus) -> BackgroundJobSummary {
        BackgroundJobSummary {
            job_id: "j1".to_string(),
            session_id: "s1".to_string(),
            prompt: "test".to_string(),
            provider: ProviderId::Anthropic,
            model: "claude-sonnet-4-20250514".to_string(),
            status,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            pid: None,
            exit_code: None,
            signal: None,
        }
    }

    #[test]
    fn summary_display_completed_with_exit_code() {
        let summary = BackgroundJobSummary {
            exit_code: Some(0),
            ..test_summary(BackgroundJobStatus::Completed)
        };
        assert_eq!(
            summary.to_string(),
            "completed [anthropic/claude-sonnet-4-20250514] (exit=0)"
        );
    }

    #[test]
    fn summary_display_failed_with_exit_and_signal() {
        let summary = BackgroundJobSummary {
            exit_code: Some(137),
            signal: Some(9),
            ..test_summary(BackgroundJobStatus::Failed)
        };
        assert_eq!(
            summary.to_string(),
            "failed [anthropic/claude-sonnet-4-20250514] (exit=137 signal=9)"
        );
    }

    #[test]
    fn summary_display_orphaned_with_signal_only() {
        let summary = BackgroundJobSummary {
            signal: Some(9),
            ..test_summary(BackgroundJobStatus::Orphaned)
        };
        assert_eq!(
            summary.to_string(),
            "orphaned [anthropic/claude-sonnet-4-20250514] (signal=9)"
        );
    }

    #[test]
    fn summary_display_active_status_no_exit_detail() {
        let summary = BackgroundJobSummary {
            pid: Some(42),
            ..test_summary(BackgroundJobStatus::Running)
        };
        assert_eq!(
            summary.to_string(),
            "running [anthropic/claude-sonnet-4-20250514]"
        );
    }

    #[test]
    fn summary_display_cancelled_no_exit_info() {
        let summary = test_summary(BackgroundJobStatus::Cancelled);
        assert_eq!(
            summary.to_string(),
            "cancelled [anthropic/claude-sonnet-4-20250514]"
        );
    }

    #[test]
    fn summary_display_empty_model_shows_provider_only() {
        let summary = BackgroundJobSummary {
            model: String::new(),
            ..test_summary(BackgroundJobStatus::Running)
        };
        assert_eq!(summary.to_string(), "running [anthropic]");
    }

    #[test]
    fn record_serde_round_trip_includes_model_and_permission_mode() {
        let now = Utc::now();
        let record = BackgroundJobRecord {
            job_id: "j1".to_string(),
            session_id: "s1".to_string(),
            prompt: "serde round trip".to_string(),
            cwd: "/tmp".to_string(),
            provider: ProviderId::Anthropic,
            fallback_provider: None,
            model: "claude-sonnet-4-20250514".to_string(),
            permission_mode: Some(PermissionMode::Default),
            status: BackgroundJobStatus::Queued,
            created_at: now,
            updated_at: now,
            started_at: None,
            finished_at: None,
            pid: None,
            log_path: "/tmp/j1.log".to_string(),
            error: None,
            exit_code: None,
            signal: None,
            last_log_offset: 0,
            cancellation_reason: None,
        };
        let json = serde_json::to_string(&record).expect("serialize");
        let back: BackgroundJobRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.model, "claude-sonnet-4-20250514");
        assert_eq!(back.permission_mode, Some(PermissionMode::Default));
        assert!(json.contains("\"model\""));
        assert!(json.contains("\"permission_mode\""));
    }

    #[test]
    fn summary_elapsed_ms_is_non_negative() {
        let summary = test_summary(BackgroundJobStatus::Running);
        assert!(summary.elapsed_ms() >= 0);
    }
}
