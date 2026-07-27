#[cfg(test)]
use chrono::Utc;
#[cfg(test)]
use orbcode_config::PermissionMode;
use orbcode_protocol::{BackgroundTaskView, BackgroundTaskViewKind, BackgroundTaskViewStatus};
#[cfg(test)]
use orbcode_tools::{LocalShellTaskRecord, LocalShellTaskStatus};

use crate::background::{BackgroundJobDetail, BackgroundJobStatus, BackgroundJobSummary};

fn map_job_status(status: BackgroundJobStatus) -> BackgroundTaskViewStatus {
    match status {
        BackgroundJobStatus::Queued => BackgroundTaskViewStatus::Queued,
        BackgroundJobStatus::Running => BackgroundTaskViewStatus::Running,
        BackgroundJobStatus::Completed => BackgroundTaskViewStatus::Completed,
        BackgroundJobStatus::Failed => BackgroundTaskViewStatus::Failed,
        BackgroundJobStatus::Cancelled => BackgroundTaskViewStatus::Cancelled,
        BackgroundJobStatus::Orphaned => BackgroundTaskViewStatus::Orphaned,
        BackgroundJobStatus::Unknown => BackgroundTaskViewStatus::Unknown,
    }
}

#[cfg(test)]
fn map_shell_status(status: LocalShellTaskStatus) -> BackgroundTaskViewStatus {
    match status {
        LocalShellTaskStatus::Queued => BackgroundTaskViewStatus::Queued,
        LocalShellTaskStatus::PermissionPending => BackgroundTaskViewStatus::PermissionPending,
        LocalShellTaskStatus::Running => BackgroundTaskViewStatus::Running,
        LocalShellTaskStatus::Interrupting => BackgroundTaskViewStatus::Interrupting,
        LocalShellTaskStatus::Succeeded => BackgroundTaskViewStatus::Completed,
        LocalShellTaskStatus::Failed => BackgroundTaskViewStatus::Failed,
        LocalShellTaskStatus::Interrupted => BackgroundTaskViewStatus::Cancelled,
    }
}

pub fn job_summary_to_view(summary: &BackgroundJobSummary) -> BackgroundTaskView {
    BackgroundTaskView {
        task_id: summary.job_id.clone(),
        session_id: summary.session_id.clone(),
        kind: BackgroundTaskViewKind::BackgroundJob,
        status: map_job_status(summary.status),
        description: summary.prompt.clone(),
        cwd: String::new(),
        created_at: summary.created_at,
        updated_at: summary.updated_at,
        started_at: None,
        finished_at: None,
        pid: summary.pid,
        exit_code: summary.exit_code,
        signal: summary.signal,
        error: None,
        model: Some(summary.model.clone()),
        provider: Some(summary.provider),
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

pub fn job_detail_to_view(detail: &BackgroundJobDetail) -> BackgroundTaskView {
    BackgroundTaskView {
        task_id: detail.job_id.clone(),
        session_id: detail.session_id.clone(),
        kind: BackgroundTaskViewKind::BackgroundJob,
        status: map_job_status(detail.status),
        description: detail.prompt.clone(),
        cwd: detail.cwd.clone(),
        created_at: detail.created_at,
        updated_at: detail.updated_at,
        started_at: detail.started_at,
        finished_at: detail.finished_at,
        pid: detail.pid,
        exit_code: detail.exit_code,
        signal: detail.signal,
        error: detail.error.clone(),
        model: Some(detail.model.clone()),
        provider: Some(detail.provider),
        permission_mode: detail.permission_mode.map(|mode| mode.as_str().to_string()),
        agent_type: None,
        child_session_id: None,
        cancellation_reason: detail.cancellation_reason.clone(),
        label: None,
        log_tail: Some(detail.log_tail.clone()),
        progress_events: None,
        workflow_steps: None,
    }
}

#[cfg(test)]
pub fn shell_task_to_view(record: &LocalShellTaskRecord) -> BackgroundTaskView {
    BackgroundTaskView {
        task_id: record.task_id.clone(),
        session_id: record.session_id.clone(),
        kind: BackgroundTaskViewKind::LocalShell,
        status: map_shell_status(record.status),
        description: record.command.clone(),
        cwd: record.cwd.clone(),
        created_at: record.created_at,
        updated_at: record.updated_at,
        started_at: record.current_attempt.as_ref().and_then(|a| a.started_at),
        finished_at: record.current_attempt.as_ref().and_then(|a| a.finished_at),
        pid: record.current_attempt.as_ref().and_then(|a| a.pid),
        exit_code: record.current_attempt.as_ref().and_then(|a| a.exit_code),
        signal: record.current_attempt.as_ref().and_then(|a| a.signal),
        error: record.last_error.clone(),
        model: None,
        provider: None,
        permission_mode: None,
        agent_type: None,
        child_session_id: None,
        cancellation_reason: None,
        label: record.label.clone(),
        log_tail: None,
        progress_events: None,
        workflow_steps: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_protocol::ProviderId;
    use orbcode_tools::{
        BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus, LocalShellAttempt,
        task_record_to_view,
    };

    #[test]
    fn job_summary_converts_to_view() {
        let summary = BackgroundJobSummary {
            job_id: "job-1".to_string(),
            session_id: "sess-1".to_string(),
            prompt: "Run tests".to_string(),
            provider: ProviderId::Anthropic,
            model: "claude-sonnet-4-6".to_string(),
            status: BackgroundJobStatus::Running,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            pid: Some(123),
            exit_code: None,
            signal: None,
        };
        let view = job_summary_to_view(&summary);
        assert_eq!(view.task_id, "job-1");
        assert_eq!(view.kind, BackgroundTaskViewKind::BackgroundJob);
        assert_eq!(view.status, BackgroundTaskViewStatus::Running);
        assert_eq!(view.provider, Some(ProviderId::Anthropic));
        assert_eq!(view.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(view.pid, Some(123));
    }

    #[test]
    fn job_detail_converts_to_view_with_log_tail() {
        let detail = BackgroundJobDetail {
            job_id: "job-2".to_string(),
            session_id: "sess-1".to_string(),
            prompt: "Fix the bug".to_string(),
            cwd: "/home/user".to_string(),
            provider: ProviderId::Anthropic,
            fallback_provider: None,
            model: "opus".to_string(),
            permission_mode: Some(PermissionMode::Default),
            status: BackgroundJobStatus::Completed,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            finished_at: None,
            pid: None,
            exit_code: Some(0),
            signal: None,
            error: None,
            cancellation_reason: None,
            log_tail: vec!["line1".to_string(), "line2".to_string()],
            progress_summary: None,
            elapsed_ms: 5000,
        };
        let view = job_detail_to_view(&detail);
        assert_eq!(view.status, BackgroundTaskViewStatus::Completed);
        assert_eq!(view.cwd, "/home/user");
        assert_eq!(view.permission_mode.as_deref(), Some("default"));
        assert_eq!(
            view.log_tail,
            Some(vec!["line1".to_string(), "line2".to_string()])
        );
    }

    #[test]
    fn task_record_local_agent_converts_to_view() {
        let record = BackgroundTaskRecord {
            job_id: "agent-1".to_string(),
            session_id: "sess-1".to_string(),
            prompt: "explore codebase".to_string(),
            cwd: "/tmp".to_string(),
            status: BackgroundTaskStatus::Running,
            created_at: "2026-06-01T00:00:00Z".to_string(),
            updated_at: "2026-06-01T00:01:00Z".to_string(),
            started_at: Some("2026-06-01T00:00:05Z".to_string()),
            finished_at: None,
            pid: Some(456),
            log_path: "/tmp/log".to_string(),
            error: None,
            task_kind: BackgroundTaskKind::LocalAgent,
            tool_use_id: None,
            child_session_id: Some("child-sess".to_string()),
            agent_type: Some("Explore".to_string()),
            model: Some("sonnet".to_string()),
            permission_mode: None,
            result: None,
            exit_code: None,
            signal: None,
            extra: serde_json::Map::new(),
        };
        let view = task_record_to_view(&record);
        assert_eq!(view.kind, BackgroundTaskViewKind::LocalAgent);
        assert_eq!(view.agent_type.as_deref(), Some("Explore"));
        assert_eq!(view.child_session_id.as_deref(), Some("child-sess"));
        assert_eq!(view.provider, None);
        assert!(view.started_at.is_some());
    }

    #[test]
    fn task_record_background_job_gets_provider() {
        let record = BackgroundTaskRecord {
            job_id: "job-3".to_string(),
            session_id: "sess-1".to_string(),
            prompt: "do work".to_string(),
            cwd: "/tmp".to_string(),
            status: BackgroundTaskStatus::Queued,
            created_at: "2026-06-01T00:00:00Z".to_string(),
            updated_at: "2026-06-01T00:00:00Z".to_string(),
            started_at: None,
            finished_at: None,
            pid: None,
            log_path: "/tmp/log".to_string(),
            error: None,
            task_kind: BackgroundTaskKind::BackgroundJob,
            tool_use_id: None,
            child_session_id: None,
            agent_type: None,
            model: None,
            permission_mode: None,
            result: None,
            exit_code: None,
            signal: None,
            extra: serde_json::Map::new(),
        };
        let view = task_record_to_view(&record);
        assert_eq!(view.kind, BackgroundTaskViewKind::BackgroundJob);
        assert_eq!(view.provider, Some(ProviderId::Anthropic));
    }

    #[test]
    fn shell_task_converts_to_view() {
        let record = LocalShellTaskRecord {
            task_id: "shell-1".to_string(),
            session_id: "sess-1".to_string(),
            command: "ls -la".to_string(),
            cwd: "/home".to_string(),
            label: Some("list files".to_string()),
            status: LocalShellTaskStatus::Running,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            current_attempt: Some(LocalShellAttempt {
                attempt: 1,
                pid: Some(789),
                started_at: Some(Utc::now()),
                finished_at: None,
                exit_code: None,
                signal: None,
                log_byte_start: 0,
                log_byte_end: 0,
                kill_reason: None,
                terminal_status: None,
            }),
            history: Vec::new(),
            restart_count: 0,
            output_bytes: 0,
            log_path: "/tmp/log".to_string(),
            last_error: None,
            kill_signal_sent: None,
        };
        let view = shell_task_to_view(&record);
        assert_eq!(view.kind, BackgroundTaskViewKind::LocalShell);
        assert_eq!(view.status, BackgroundTaskViewStatus::Running);
        assert_eq!(view.description, "ls -la");
        assert_eq!(view.label.as_deref(), Some("list files"));
        assert_eq!(view.pid, Some(789));
        assert!(view.provider.is_none());
        assert!(view.model.is_none());
    }

    #[test]
    fn shell_status_mapping() {
        assert_eq!(
            map_shell_status(LocalShellTaskStatus::Succeeded),
            BackgroundTaskViewStatus::Completed
        );
        assert_eq!(
            map_shell_status(LocalShellTaskStatus::Interrupted),
            BackgroundTaskViewStatus::Cancelled
        );
        assert_eq!(
            map_shell_status(LocalShellTaskStatus::PermissionPending),
            BackgroundTaskViewStatus::PermissionPending
        );
        assert_eq!(
            map_shell_status(LocalShellTaskStatus::Interrupting),
            BackgroundTaskViewStatus::Interrupting
        );
    }
}
