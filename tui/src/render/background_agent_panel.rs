#[cfg(test)]
use std::time::Instant;

use orbcode_protocol::{BackgroundTaskView, BackgroundTaskViewKind, BackgroundTaskViewStatus};
use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};

use crate::render::text_utils::StyledLine;
use crate::tui_theme::{active_palette, subtle_style};

const HEADER: &str = "Background tasks (TaskStop to cancel):";
const ROW_PREFIX: &str = "  ↗ ";

pub(crate) fn render_background_task_summary_lines(rows: &[BackgroundTaskView]) -> Vec<StyledLine> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(Line::from(vec![Span::styled(
        HEADER.to_string(),
        subtle_style(),
    )]));
    for row in rows {
        lines.push(render_row(row));
        if let Some(progress) = progress_text(row) {
            lines.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(progress, subtle_style()),
            ]));
        }
    }
    lines
}

fn render_row(row: &BackgroundTaskView) -> StyledLine {
    let (status_label, status_style) = match row.status {
        BackgroundTaskViewStatus::Queued | BackgroundTaskViewStatus::Running => (
            "running",
            Style::default()
                .fg(active_palette().claude)
                .add_modifier(Modifier::BOLD),
        ),
        BackgroundTaskViewStatus::PermissionPending => (
            "pending",
            Style::default()
                .fg(active_palette().claude)
                .add_modifier(Modifier::BOLD),
        ),
        BackgroundTaskViewStatus::Interrupting => (
            "stopping",
            Style::default()
                .fg(active_palette().claude)
                .add_modifier(Modifier::BOLD),
        ),
        BackgroundTaskViewStatus::Completed => ("done", subtle_style()),
        BackgroundTaskViewStatus::Failed => ("failed", Style::default().fg(active_palette().error)),
        BackgroundTaskViewStatus::Cancelled => ("cancelled", subtle_style()),
        BackgroundTaskViewStatus::Orphaned => {
            ("orphaned", Style::default().fg(active_palette().error))
        }
        _ => ("unknown", subtle_style()),
    };
    let task_type = task_type_label(row);
    let description = truncate_description(&row.description, 48);
    Line::from(vec![
        Span::styled(ROW_PREFIX, subtle_style()),
        Span::styled(
            row.task_id.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(format!("[{status_label}]"), status_style),
        Span::raw("  "),
        Span::styled(task_type, subtle_style()),
        Span::raw("  "),
        Span::styled(description, subtle_style()),
    ])
}

fn task_type_label(row: &BackgroundTaskView) -> String {
    match row.kind {
        BackgroundTaskViewKind::LocalAgent => {
            row.agent_type.as_deref().unwrap_or("agent").to_string()
        }
        BackgroundTaskViewKind::Workflow => "workflow".to_string(),
        BackgroundTaskViewKind::BackgroundJob => "job".to_string(),
        BackgroundTaskViewKind::LocalShell => "shell".to_string(),
        _ => "task".to_string(),
    }
}

fn progress_text(row: &BackgroundTaskView) -> Option<String> {
    let events = row.progress_events.as_ref()?;
    let event = events
        .iter()
        .rev()
        .find(|event| event.step_key.is_some() || event.output.is_some())?;
    let step = event.step_key.as_deref().unwrap_or("-");
    let event_label = match event.event.as_str() {
        "step_started" => "started",
        "step_completed" => "completed",
        "step_failed" => "failed",
        "step_cancelled" => "cancelled",
        "agent_started" => "agent",
        "phase_started" => "phase",
        "parallel_started" => "parallel",
        value => value,
    };
    let detail = event
        .message
        .as_deref()
        .or(event.output.as_deref())
        .map(|value| truncate_description(value, 56));
    match detail {
        Some(detail) if !detail.is_empty() => Some(format!("{step} {event_label}: {detail}")),
        _ => Some(format!("{step} {event_label}")),
    }
}

fn truncate_description(description: &str, max_chars: usize) -> String {
    let trimmed = description.trim();
    let mut chars = trimmed.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background_agent_panel::BackgroundAgentPanelState;
    use chrono::Utc;
    use orbcode_protocol::BackgroundTaskProgressEvent;
    use orbcode_tools::{BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus};

    fn rec(
        id: &str,
        kind: BackgroundTaskKind,
        status: BackgroundTaskStatus,
        prompt: &str,
    ) -> BackgroundTaskRecord {
        BackgroundTaskRecord {
            job_id: id.to_string(),
            session_id: "session-1".to_string(),
            prompt: prompt.to_string(),
            cwd: "/tmp".to_string(),
            status,
            created_at: "2026-05-27T00:00:00Z".to_string(),
            updated_at: "2026-05-27T00:00:00Z".to_string(),
            started_at: None,
            finished_at: None,
            pid: None,
            log_path: "/tmp/log".to_string(),
            error: None,
            task_kind: kind,
            tool_use_id: None,
            child_session_id: None,
            agent_type: Some("Explore".to_string()),
            model: None,
            permission_mode: None,
            result: None,
            exit_code: None,
            signal: None,
            extra: serde_json::Map::new(),
        }
    }

    fn line_text(line: &StyledLine) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn renders_full_task_id_for_running_agents() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        let now = Instant::now();
        panel.apply_records(
            vec![rec(
                "agent-3f2b490694454a859e64d23d4e39c7cb",
                BackgroundTaskKind::LocalAgent,
                BackgroundTaskStatus::Running,
                "summarize the workspace",
            )],
            now,
        );
        let lines = render_background_task_summary_lines(panel.rows());
        assert!(!lines.is_empty(), "panel should render when active");
        let header = line_text(&lines[0]);
        assert!(header.starts_with("Background tasks"));
        let row = line_text(&lines[1]);
        assert!(
            row.contains("agent-3f2b490694454a859e64d23d4e39c7cb"),
            "row must include the full literal task_id: {row}"
        );
        assert!(row.contains("[running]"), "running marker missing: {row}");
        assert!(row.contains("Explore"), "agent_type missing: {row}");
    }

    #[test]
    fn renders_workflow_rows_with_workflow_label() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        let now = Instant::now();
        panel.apply_records(
            vec![rec(
                "workflow-1234",
                BackgroundTaskKind::Workflow,
                BackgroundTaskStatus::Running,
                "Run workflow check",
            )],
            now,
        );
        let lines = render_background_task_summary_lines(panel.rows());
        let row = line_text(&lines[1]);
        assert!(row.contains("workflow-1234"), "{row}");
        assert!(row.contains("workflow"), "{row}");
        assert!(row.contains("Run workflow check"), "{row}");
    }

    #[test]
    fn renders_workflow_progress_under_row() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        let now = Instant::now();
        let mut view = orbcode_tools::task_record_to_view(&rec(
            "workflow-1234",
            BackgroundTaskKind::Workflow,
            BackgroundTaskStatus::Running,
            "Run workflow check",
        ));
        view.progress_events = Some(vec![BackgroundTaskProgressEvent {
            timestamp: Utc::now(),
            event: "step_completed".to_string(),
            step_key: Some("step.0".to_string()),
            kind: Some("log".to_string()),
            message: None,
            output: Some("checkpoint complete".to_string()),
            child_session_id: None,
        }]);
        panel.apply_views_for_test(vec![view], now);

        let lines = render_background_task_summary_lines(panel.rows());
        let progress = line_text(&lines[2]);
        assert!(progress.contains("step.0 completed"), "{progress}");
        assert!(progress.contains("checkpoint complete"), "{progress}");
    }

    #[test]
    fn renders_task_summary_lines_from_views_without_panel_state() {
        let mut view = orbcode_tools::task_record_to_view(&rec(
            "workflow-1234",
            BackgroundTaskKind::Workflow,
            BackgroundTaskStatus::Running,
            "Run workflow check",
        ));
        view.progress_events = Some(vec![BackgroundTaskProgressEvent {
            timestamp: Utc::now(),
            event: "phase_started".to_string(),
            step_key: Some("step.0".to_string()),
            kind: Some("phase".to_string()),
            message: Some("Review phase".to_string()),
            output: None,
            child_session_id: None,
        }]);

        let lines = render_background_task_summary_lines(&[view]);

        let header = line_text(&lines[0]);
        assert!(header.starts_with("Background tasks"), "{header}");
        let row = line_text(&lines[1]);
        assert!(row.contains("workflow-1234"), "{row}");
        assert!(row.contains("workflow"), "{row}");
        let progress = line_text(&lines[2]);
        assert!(progress.contains("step.0 phase"), "{progress}");
        assert!(progress.contains("Review phase"), "{progress}");
    }

    #[test]
    fn empty_panel_renders_nothing() {
        let panel = BackgroundAgentPanelState::new();
        let lines = render_background_task_summary_lines(panel.rows());
        assert!(lines.is_empty());
    }

    #[test]
    fn long_prompt_is_truncated_with_ellipsis() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        let long = "x".repeat(120);
        panel.apply_records(
            vec![rec(
                "agent-zz",
                BackgroundTaskKind::LocalAgent,
                BackgroundTaskStatus::Running,
                &long,
            )],
            Instant::now(),
        );
        let lines = render_background_task_summary_lines(panel.rows());
        let row = line_text(&lines[1]);
        assert!(
            row.ends_with('…'),
            "long description should be truncated: {row}"
        );
    }
}
