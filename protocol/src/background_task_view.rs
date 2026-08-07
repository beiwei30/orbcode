use std::fmt;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;

use crate::provider::ProviderId;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, JsonSchema,
)]
#[non_exhaustive]
pub enum BackgroundTaskViewKind {
    BackgroundJob,
    LocalAgent,
    LocalShell,
    Workflow,
}

impl fmt::Display for BackgroundTaskViewKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackgroundJob => f.write_str("background_job"),
            Self::LocalAgent => f.write_str("local_agent"),
            Self::LocalShell => f.write_str("local_shell"),
            Self::Workflow => f.write_str("workflow"),
        }
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, JsonSchema,
)]
#[non_exhaustive]
pub enum BackgroundTaskViewStatus {
    Queued,
    PermissionPending,
    Running,
    Interrupting,
    Completed,
    Failed,
    Cancelled,
    Orphaned,
    Unknown,
}

impl BackgroundTaskViewStatus {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::PermissionPending | Self::Running | Self::Interrupting
        )
    }

    pub fn is_terminal(self) -> bool {
        !self.is_active()
    }
}

impl fmt::Display for BackgroundTaskViewStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Queued => "queued",
            Self::PermissionPending => "permission_pending",
            Self::Running => "running",
            Self::Interrupting => "interrupting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Orphaned => "orphaned",
            Self::Unknown => "unknown",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct BackgroundTaskProgressEvent {
    #[schemars(with = "String")]
    pub timestamp: DateTime<Utc>,
    pub event: String,
    pub step_key: Option<String>,
    pub kind: Option<String>,
    pub message: Option<String>,
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, JsonSchema,
)]
#[non_exhaustive]
pub enum WorkflowStepViewStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl fmt::Display for WorkflowStepViewStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct WorkflowStepView {
    pub step_key: String,
    pub parent_key: Option<String>,
    pub depth: u32,
    pub kind: String,
    pub label: String,
    pub status: WorkflowStepViewStatus,
    #[schemars(with = "Option<String>")]
    pub started_at: Option<DateTime<Utc>>,
    #[schemars(with = "Option<String>")]
    pub finished_at: Option<DateTime<Utc>>,
    pub output: Option<String>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct BackgroundTaskView {
    pub task_id: String,
    pub session_id: String,
    pub kind: BackgroundTaskViewKind,
    pub status: BackgroundTaskViewStatus,

    pub description: String,
    pub cwd: String,

    #[schemars(with = "String")]
    pub created_at: DateTime<Utc>,
    #[schemars(with = "String")]
    pub updated_at: DateTime<Utc>,
    #[schemars(with = "Option<String>")]
    pub started_at: Option<DateTime<Utc>>,
    #[schemars(with = "Option<String>")]
    pub finished_at: Option<DateTime<Utc>>,

    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub error: Option<String>,

    pub model: Option<String>,
    pub provider: Option<ProviderId>,
    pub permission_mode: Option<String>,

    pub agent_type: Option<String>,
    pub child_session_id: Option<String>,

    pub cancellation_reason: Option<String>,

    pub label: Option<String>,

    pub log_tail: Option<Vec<String>>,
    pub progress_events: Option<Vec<BackgroundTaskProgressEvent>>,
    pub workflow_steps: Option<Vec<WorkflowStepView>>,
}

impl BackgroundTaskView {
    pub fn elapsed_ms(&self) -> i64 {
        let start = self.started_at.unwrap_or(self.created_at);
        let end = self.finished_at.unwrap_or_else(Utc::now);
        (end - start).num_milliseconds()
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

impl fmt::Display for BackgroundTaskView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.status)?;
        match (&self.provider, &self.model) {
            (Some(provider), Some(model)) => write!(f, " [{provider}/{model}]")?,
            (None, Some(model)) => write!(f, " [{model}]")?,
            _ => {}
        }
        if let Some(detail) = self.exit_detail() {
            write!(f, " ({detail})")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(status: BackgroundTaskViewStatus) -> BackgroundTaskView {
        BackgroundTaskView {
            task_id: "test-id".to_string(),
            session_id: "sess-1".to_string(),
            kind: BackgroundTaskViewKind::BackgroundJob,
            status,
            description: "test prompt".to_string(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            finished_at: None,
            pid: None,
            exit_code: None,
            signal: None,
            error: None,
            model: Some("claude-sonnet-4-6".to_string()),
            provider: Some(ProviderId::Anthropic),
            permission_mode: None,
            agent_type: None,
            child_session_id: None,
            cancellation_reason: None,
            label: None,
            log_tail: None,
            progress_events: None,
            workflow_steps: None,
        }
    }

    #[test]
    fn active_status_variants() {
        assert!(BackgroundTaskViewStatus::Queued.is_active());
        assert!(BackgroundTaskViewStatus::PermissionPending.is_active());
        assert!(BackgroundTaskViewStatus::Running.is_active());
        assert!(BackgroundTaskViewStatus::Interrupting.is_active());
        assert!(!BackgroundTaskViewStatus::Completed.is_active());
        assert!(!BackgroundTaskViewStatus::Failed.is_active());
        assert!(!BackgroundTaskViewStatus::Cancelled.is_active());
        assert!(!BackgroundTaskViewStatus::Orphaned.is_active());
        assert!(!BackgroundTaskViewStatus::Unknown.is_active());
    }

    #[test]
    fn display_with_provider_and_model() {
        let v = view(BackgroundTaskViewStatus::Running);
        assert_eq!(v.to_string(), "running [anthropic/claude-sonnet-4-6]");
    }

    #[test]
    fn display_with_exit_detail() {
        let mut v = view(BackgroundTaskViewStatus::Failed);
        v.exit_code = Some(1);
        v.signal = Some(9);
        assert_eq!(
            v.to_string(),
            "failed [anthropic/claude-sonnet-4-6] (exit=1 signal=9)"
        );
    }

    #[test]
    fn display_without_provider() {
        let mut v = view(BackgroundTaskViewStatus::Completed);
        v.provider = None;
        assert_eq!(v.to_string(), "completed [claude-sonnet-4-6]");
    }

    #[test]
    fn display_without_model_or_provider() {
        let mut v = view(BackgroundTaskViewStatus::Completed);
        v.provider = None;
        v.model = None;
        assert_eq!(v.to_string(), "completed");
    }

    #[test]
    fn elapsed_ms_uses_started_and_finished() {
        let now = Utc::now();
        let mut v = view(BackgroundTaskViewStatus::Completed);
        v.created_at = now - chrono::Duration::seconds(100);
        v.started_at = Some(now - chrono::Duration::seconds(50));
        v.finished_at = Some(now - chrono::Duration::seconds(10));
        assert_eq!(v.elapsed_ms(), 40_000);
    }

    #[test]
    fn elapsed_ms_falls_back_to_created_at() {
        let now = Utc::now();
        let mut v = view(BackgroundTaskViewStatus::Completed);
        v.created_at = now - chrono::Duration::seconds(30);
        v.started_at = None;
        v.finished_at = Some(now - chrono::Duration::seconds(10));
        assert_eq!(v.elapsed_ms(), 20_000);
    }

    #[test]
    fn exit_detail_none_when_active() {
        let v = view(BackgroundTaskViewStatus::Running);
        assert_eq!(v.exit_detail(), None);
    }

    #[test]
    fn exit_detail_formats_code_and_signal() {
        let mut v = view(BackgroundTaskViewStatus::Failed);
        v.exit_code = Some(137);
        assert_eq!(v.exit_detail(), Some("exit=137".to_string()));

        v.signal = Some(9);
        assert_eq!(v.exit_detail(), Some("exit=137 signal=9".to_string()));
    }
}
