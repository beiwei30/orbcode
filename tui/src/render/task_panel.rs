use std::time::Instant;

use ratatui::{
    prelude::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use orbcode_tools::{TaskListSnapshot, TaskStatusKind, TaskView};

use crate::render::text_utils::StyledLine;
use crate::task_panel::{RECENT_COMPLETED_TTL, TaskPanelState};
use crate::tui_theme::{active_palette, subtle_style};

const NESTED_PREFIX: &str = "  └ ";
const NESTED_CONTINUATION_PREFIX: &str = "    ";
const STANDALONE_HEADER_PREFIX: &str = "Tasks ";
const COMPACT_HEADER_PREFIX: &str = "Tasks: ";

#[derive(Clone, Copy)]
pub(crate) enum TaskPanelLayout {
    Standalone,
    Nested,
    #[allow(dead_code)]
    Compact,
}

pub(crate) fn render_task_panel_lines(
    panel: &TaskPanelState,
    layout: TaskPanelLayout,
    width: usize,
    now: Instant,
) -> Vec<StyledLine> {
    if !panel.is_visible() {
        return Vec::new();
    }
    let Some(snapshot) = panel.snapshot() else {
        return Vec::new();
    };

    match layout {
        TaskPanelLayout::Compact => {
            if !panel.is_expanded() {
                return compact_header_only(snapshot);
            }
            render_panel(snapshot, panel, layout, width, now)
        }
        TaskPanelLayout::Standalone | TaskPanelLayout::Nested => {
            if !panel.is_expanded() {
                return Vec::new();
            }
            render_panel(snapshot, panel, layout, width, now)
        }
    }
}

fn compact_header_only(snapshot: &TaskListSnapshot) -> Vec<StyledLine> {
    vec![Line::from(vec![Span::styled(
        format!(
            "{}{} ({} done, {} active, {} pending)",
            COMPACT_HEADER_PREFIX,
            snapshot.summary.total,
            snapshot.summary.completed,
            snapshot.summary.in_progress,
            snapshot.summary.pending,
        ),
        subtle_style(),
    )])]
}

fn render_panel(
    snapshot: &TaskListSnapshot,
    panel: &TaskPanelState,
    layout: TaskPanelLayout,
    width: usize,
    now: Instant,
) -> Vec<StyledLine> {
    let header_prefix = match layout {
        TaskPanelLayout::Standalone => STANDALONE_HEADER_PREFIX,
        TaskPanelLayout::Nested => NESTED_PREFIX,
        TaskPanelLayout::Compact => COMPACT_HEADER_PREFIX,
    };

    let max_display = pick_visible_capacity(snapshot.tasks.len(), width);
    let (visible, hidden) = prioritize(panel, &snapshot.tasks, max_display, now);

    let mut lines = Vec::with_capacity(visible.len() + 2);
    if !matches!(layout, TaskPanelLayout::Nested) {
        lines.push(Line::from(vec![Span::styled(
            format!(
                "{}{} task{} ({} done, {} active, {} pending)",
                header_prefix,
                snapshot.summary.total,
                if snapshot.summary.total == 1 { "" } else { "s" },
                snapshot.summary.completed,
                snapshot.summary.in_progress,
                snapshot.summary.pending,
            ),
            subtle_style(),
        )]));
    }

    for (index, task) in visible.iter().enumerate() {
        let prefix = match layout {
            TaskPanelLayout::Nested if index == 0 => NESTED_PREFIX,
            TaskPanelLayout::Nested => NESTED_CONTINUATION_PREFIX,
            _ => "",
        };
        lines.push(render_task_line(task, prefix, width));
    }

    if !hidden.is_empty() {
        let summary = match layout {
            TaskPanelLayout::Nested => {
                format!("{}{}", NESTED_CONTINUATION_PREFIX, hidden_summary(&hidden))
            }
            _ => hidden_summary(&hidden),
        };
        lines.push(Line::from(vec![Span::styled(summary, subtle_style())]));
    }

    lines
}

fn render_task_line(task: &TaskView, prefix: &str, width: usize) -> StyledLine {
    let (icon, base_style) = task_glyph(task.status);
    let mut spans = Vec::with_capacity(6);
    spans.push(Span::styled(format!("{prefix}{icon} "), base_style));

    let subject_style = match task.status {
        TaskStatusKind::Completed => subtle_style().add_modifier(Modifier::CROSSED_OUT),
        TaskStatusKind::InProgress => Style::default()
            .fg(active_palette().claude)
            .add_modifier(Modifier::BOLD),
        TaskStatusKind::Pending => {
            if task.open_blockers.is_empty() {
                Style::default()
            } else {
                subtle_style()
            }
        }
    };

    let display_subject = truncate_to_width(&task.subject, max_subject_width(width, task, prefix));
    spans.push(Span::styled(display_subject, subject_style));

    if let Some(owner) = task.owner.as_ref() {
        spans.push(Span::styled(format!(" (@{owner})"), subtle_style()));
    }
    if !task.open_blockers.is_empty() {
        let blocker_text = task
            .open_blockers
            .iter()
            .map(|id| format!("#{id}"))
            .collect::<Vec<_>>()
            .join(", ");
        spans.push(Span::styled(
            format!(" ⟂ blocked by {blocker_text}"),
            subtle_style(),
        ));
    }

    Line::from(spans)
}

fn task_glyph(status: TaskStatusKind) -> (&'static str, Style) {
    let palette = active_palette();
    match status {
        TaskStatusKind::Completed => (
            "✔",
            Style::default()
                .fg(palette.success)
                .add_modifier(Modifier::BOLD),
        ),
        TaskStatusKind::InProgress => (
            "◼",
            Style::default()
                .fg(palette.claude)
                .add_modifier(Modifier::BOLD),
        ),
        TaskStatusKind::Pending => ("☐", Style::default().add_modifier(Modifier::DIM)),
    }
}

fn max_subject_width(width: usize, task: &TaskView, prefix: &str) -> usize {
    let prefix_width = prefix.width();
    let icon_overhead = 2;
    let owner_overhead = task
        .owner
        .as_ref()
        .map_or(0, |o| format!(" (@{o})").width());
    let blockers_overhead = if task.open_blockers.is_empty() {
        0
    } else {
        let inner = task
            .open_blockers
            .iter()
            .map(|id| format!("#{id}").width())
            .sum::<usize>()
            + (task.open_blockers.len().saturating_sub(1)) * 2;
        " ⟂ blocked by ".width() + inner
    };
    width
        .saturating_sub(prefix_width)
        .saturating_sub(icon_overhead)
        .saturating_sub(owner_overhead)
        .saturating_sub(blockers_overhead)
        .max(8)
}

pub(crate) fn truncate_to_width(value: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if value.width() <= max_width {
        return value.to_string();
    }
    let mut budget = max_width.saturating_sub(1);
    let mut result = String::new();
    for ch in value.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if ch_width > budget {
            break;
        }
        budget -= ch_width;
        result.push(ch);
    }
    result.push('…');
    result
}

fn pick_visible_capacity(total_tasks: usize, width: usize) -> usize {
    let by_width = if width >= 60 { 10 } else { 5 };
    total_tasks.min(by_width).max(3)
}

fn prioritize<'a>(
    panel: &TaskPanelState,
    tasks: &'a [TaskView],
    max_display: usize,
    now: Instant,
) -> (Vec<&'a TaskView>, Vec<&'a TaskView>) {
    let mut sorted: Vec<&TaskView> = tasks.iter().collect();
    sorted.sort_by_key(|t| task_sort_key(t));
    if sorted.len() <= max_display {
        return (sorted, Vec::new());
    }
    let mut recent_completed: Vec<&TaskView> = Vec::new();
    let mut older_completed: Vec<&TaskView> = Vec::new();
    for task in sorted.iter().copied() {
        if task.status != TaskStatusKind::Completed {
            continue;
        }
        if let Some(ts) = panel.completion_timestamp(&task.id)
            && now.duration_since(ts) < RECENT_COMPLETED_TTL
        {
            recent_completed.push(task);
            continue;
        }
        older_completed.push(task);
    }
    let in_progress: Vec<&TaskView> = sorted
        .iter()
        .copied()
        .filter(|t| t.status == TaskStatusKind::InProgress)
        .collect();
    let mut pending: Vec<&TaskView> = sorted
        .iter()
        .copied()
        .filter(|t| t.status == TaskStatusKind::Pending)
        .collect();
    pending.sort_by(|a, b| {
        let a_blocked = !a.open_blockers.is_empty();
        let b_blocked = !b.open_blockers.is_empty();
        a_blocked
            .cmp(&b_blocked)
            .then_with(|| task_sort_key(a).cmp(&task_sort_key(b)))
    });
    let prioritized: Vec<&TaskView> = recent_completed
        .into_iter()
        .chain(in_progress)
        .chain(pending)
        .chain(older_completed)
        .collect();
    let visible: Vec<&TaskView> = prioritized.iter().take(max_display).copied().collect();
    let hidden: Vec<&TaskView> = prioritized.into_iter().skip(max_display).collect();
    (visible, hidden)
}

fn task_sort_key(task: &TaskView) -> (u64, String) {
    (task.id.parse::<u64>().unwrap_or(u64::MAX), task.id.clone())
}

fn hidden_summary(hidden: &[&TaskView]) -> String {
    let mut completed = 0usize;
    let mut in_progress = 0usize;
    let mut pending = 0usize;
    for task in hidden {
        match task.status {
            TaskStatusKind::Completed => completed += 1,
            TaskStatusKind::InProgress => in_progress += 1,
            TaskStatusKind::Pending => pending += 1,
        }
    }
    let mut parts = Vec::new();
    if in_progress > 0 {
        parts.push(format!("{in_progress} in progress"));
    }
    if pending > 0 {
        parts.push(format!("{pending} pending"));
    }
    if completed > 0 {
        parts.push(format!("{completed} completed"));
    }
    format!(" … +{}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_tools::{TaskListSnapshot, TaskListSummary, TaskStatusKind, TaskView};
    use std::path::PathBuf;

    fn snapshot_with(tasks: Vec<TaskView>) -> TaskListSnapshot {
        let summary = TaskListSummary {
            total: tasks.len(),
            completed: tasks
                .iter()
                .filter(|t| t.status == TaskStatusKind::Completed)
                .count(),
            in_progress: tasks
                .iter()
                .filter(|t| t.status == TaskStatusKind::InProgress)
                .count(),
            pending: tasks
                .iter()
                .filter(|t| t.status == TaskStatusKind::Pending)
                .count(),
        };
        TaskListSnapshot {
            task_list_id: "test".to_string(),
            directory: PathBuf::from("/tmp/orbcode-task-render"),
            tasks,
            summary,
            fingerprint: 0,
        }
    }

    fn task(id: &str, subject: &str, status: TaskStatusKind) -> TaskView {
        TaskView {
            id: id.to_string(),
            subject: subject.to_string(),
            description: String::new(),
            active_form: None,
            owner: None,
            status,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            open_blockers: Vec::new(),
        }
    }

    fn render_text(lines: &[StyledLine]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    fn panel_with_snapshot(snapshot: TaskListSnapshot) -> TaskPanelState {
        let mut panel = TaskPanelState::new(Some("test-session"), true);
        panel.clear_awaiting_session_activity();
        panel.apply_snapshot(snapshot_with(vec![]), Instant::now());
        panel.apply_snapshot(snapshot, Instant::now());
        panel
    }

    #[test]
    fn task_list_renders_empty_when_no_tasks() {
        let panel = panel_with_snapshot(snapshot_with(vec![]));
        let lines =
            render_task_panel_lines(&panel, TaskPanelLayout::Standalone, 80, Instant::now());
        assert!(lines.is_empty());
    }

    #[test]
    fn task_list_renders_pending_in_progress_completed_glyphs() {
        let panel = panel_with_snapshot(snapshot_with(vec![
            task("1", "Design", TaskStatusKind::Completed),
            task("2", "Build", TaskStatusKind::InProgress),
            task("3", "Test", TaskStatusKind::Pending),
        ]));
        let lines =
            render_task_panel_lines(&panel, TaskPanelLayout::Standalone, 80, Instant::now());
        let rendered = render_text(&lines);
        assert!(rendered[0].starts_with("Tasks 3 tasks (1 done, 1 active, 1 pending)"));
        assert!(rendered[1].contains("✔ Design"));
        assert!(rendered[2].contains("◼ Build"));
        assert!(rendered[3].contains("☐ Test"));
    }

    #[test]
    fn task_list_renders_owner_and_blockers() {
        let mut blocked = task("2", "Implement", TaskStatusKind::Pending);
        blocked.owner = Some("alice".to_string());
        blocked.open_blockers = vec!["1".to_string()];
        let panel = panel_with_snapshot(snapshot_with(vec![
            task("1", "Design", TaskStatusKind::Pending),
            blocked,
        ]));
        let lines =
            render_task_panel_lines(&panel, TaskPanelLayout::Standalone, 80, Instant::now());
        let rendered = render_text(&lines);
        assert!(rendered[2].contains("Implement (@alice)"));
        assert!(rendered[2].contains("blocked by #1"));
    }

    #[test]
    fn task_list_truncates_long_subjects_for_narrow_widths() {
        let panel = panel_with_snapshot(snapshot_with(vec![task(
            "1",
            "A really really really long subject that should be truncated",
            TaskStatusKind::Pending,
        )]));
        let lines =
            render_task_panel_lines(&panel, TaskPanelLayout::Standalone, 30, Instant::now());
        let rendered = render_text(&lines);
        let task_line = rendered[1].clone();
        assert!(
            task_line.contains("…"),
            "expected truncation, got `{task_line}`"
        );
        assert!(
            task_line.width() <= 30,
            "rendered width {} > budget",
            task_line.width()
        );
    }

    #[test]
    fn task_list_overflow_lists_hidden_summary() {
        let tasks: Vec<TaskView> = (1..=12)
            .map(|i| {
                task(
                    &i.to_string(),
                    &format!("Task {i}"),
                    TaskStatusKind::Pending,
                )
            })
            .collect();
        let panel = panel_with_snapshot(snapshot_with(tasks));
        let lines =
            render_task_panel_lines(&panel, TaskPanelLayout::Standalone, 80, Instant::now());
        let rendered = render_text(&lines);
        let last = rendered.last().expect("trailing summary");
        assert!(
            last.contains("+2 pending"),
            "expected hidden summary, got `{last}`"
        );
    }

    #[test]
    fn compact_layout_shows_only_header_when_collapsed() {
        let mut panel = panel_with_snapshot(snapshot_with(vec![task(
            "1",
            "Run",
            TaskStatusKind::InProgress,
        )]));
        panel.toggle();
        let lines = render_task_panel_lines(&panel, TaskPanelLayout::Compact, 80, Instant::now());
        let rendered = render_text(&lines);
        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].starts_with("Tasks: 1"));
    }

    #[test]
    fn nested_layout_matches_spinner_task_list_shape() {
        let panel = panel_with_snapshot(snapshot_with(vec![
            task("1", "Design", TaskStatusKind::Completed),
            task("2", "Run", TaskStatusKind::InProgress),
        ]));
        let lines = render_task_panel_lines(&panel, TaskPanelLayout::Nested, 80, Instant::now());
        let rendered = render_text(&lines);
        assert_eq!(rendered.len(), 2);
        assert_eq!(rendered[0], "  └ ✔ Design");
        assert_eq!(rendered[1], "    ◼ Run");
    }
}
