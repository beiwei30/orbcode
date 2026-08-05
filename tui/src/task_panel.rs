use std::collections::HashMap;
use std::time::{Duration, Instant};

use orbcode_app_server_client::AppClient;
use orbcode_tools::{
    TaskListSnapshot, TaskListSummary, TaskStatusKind, TaskView, session_task_list_id,
};

pub(crate) const RECENT_COMPLETED_TTL: Duration = Duration::from_secs(30);
pub(crate) const HIDE_DELAY: Duration = Duration::from_secs(5);
pub(crate) const POLL_FALLBACK: Duration = Duration::from_secs(5);

pub(crate) struct TaskPanelState {
    task_list_id: String,
    snapshot: Option<TaskListSnapshot>,
    expanded: bool,
    user_collapsed: bool,
    hidden: bool,
    /// Suppresses auto-show on startup so that stale tasks from a previous
    /// session don't appear. Cleared when genuinely new tasks arrive (i.e.
    /// created during this session) or the user explicitly toggles the panel.
    awaiting_session_activity: bool,
    completion_at: Option<Instant>,
    completion_timestamps: HashMap<String, Instant>,
    last_refreshed_at: Option<Instant>,
    dirty: bool,
    refresh_in_flight: bool,
}

impl TaskPanelState {
    pub(crate) fn new(session_id: Option<&str>, is_new_session: bool) -> Self {
        let task_list_id = session_task_list_id(session_id);
        Self {
            task_list_id,
            snapshot: None,
            expanded: false,
            user_collapsed: false,
            hidden: true,
            awaiting_session_activity: is_new_session,
            completion_at: None,
            completion_timestamps: HashMap::new(),
            last_refreshed_at: None,
            dirty: true,
            refresh_in_flight: false,
        }
    }

    pub(crate) fn snapshot(&self) -> Option<&TaskListSnapshot> {
        self.snapshot.as_ref()
    }

    pub(crate) fn is_visible(&self) -> bool {
        if self.hidden {
            return false;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        !snapshot.tasks.is_empty()
    }

    pub(crate) fn is_expanded(&self) -> bool {
        self.expanded && self.is_visible()
    }

    pub(crate) fn toggle(&mut self) -> bool {
        if self.awaiting_session_activity
            && self.snapshot.as_ref().is_some_and(|s| !s.tasks.is_empty())
        {
            self.awaiting_session_activity = false;
            self.hidden = false;
            self.expanded = true;
            self.user_collapsed = false;
        } else if self.expanded {
            self.expanded = false;
            self.user_collapsed = true;
        } else {
            self.expanded = true;
            self.user_collapsed = false;
        }
        self.is_visible()
    }

    #[cfg(test)]
    pub(crate) fn clear_awaiting_session_activity(&mut self) {
        self.awaiting_session_activity = false;
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub(crate) fn needs_refresh(&self, now: Instant) -> bool {
        if self.refresh_in_flight {
            return false;
        }
        if self.dirty {
            return true;
        }
        match self.last_refreshed_at {
            Some(at) => now.duration_since(at) >= POLL_FALLBACK,
            None => true,
        }
    }

    pub(crate) async fn refresh(&mut self, app_server: &AppClient) -> bool {
        if self.refresh_in_flight {
            return false;
        }
        self.refresh_in_flight = true;
        let result = app_server.load_task_list_snapshot(&self.task_list_id).await;
        self.refresh_in_flight = false;
        let result = match result {
            Ok(value) => value,
            Err(_) => {
                self.last_refreshed_at = Some(Instant::now());
                self.dirty = false;
                return false;
            }
        };
        let snapshot = match parse_task_list_snapshot(result) {
            Some(snapshot) => snapshot,
            None => {
                self.last_refreshed_at = Some(Instant::now());
                self.dirty = false;
                return false;
            }
        };
        self.apply_snapshot(snapshot, Instant::now())
    }

    pub(crate) fn apply_snapshot(&mut self, snapshot: TaskListSnapshot, now: Instant) -> bool {
        let previous = self.snapshot.take();
        let prev_ids: Vec<String> = previous
            .as_ref()
            .map(|s| s.tasks.iter().map(|t| t.id.clone()).collect())
            .unwrap_or_default();
        let current_ids: Vec<String> = snapshot.tasks.iter().map(|t| t.id.clone()).collect();
        let new_task_ids: Vec<&String> = current_ids
            .iter()
            .filter(|id| !prev_ids.contains(id))
            .collect();

        for task in &snapshot.tasks {
            if task.status == TaskStatusKind::Completed {
                self.completion_timestamps
                    .entry(task.id.clone())
                    .or_insert(now);
            } else {
                self.completion_timestamps.remove(&task.id);
            }
        }
        let live_ids: std::collections::HashSet<&str> =
            snapshot.tasks.iter().map(|t| t.id.as_str()).collect();
        self.completion_timestamps
            .retain(|id, _| live_ids.contains(id.as_str()));

        let has_open = snapshot
            .tasks
            .iter()
            .any(|t| t.status != TaskStatusKind::Completed);

        if !new_task_ids.is_empty() && previous.is_some() {
            self.awaiting_session_activity = false;
            self.expanded = true;
            self.user_collapsed = false;
        }

        if self.awaiting_session_activity {
            // Keep hidden until genuine task activity in this session.
        } else if snapshot.tasks.is_empty() {
            self.hidden = true;
            self.completion_at = None;
        } else if has_open {
            self.hidden = false;
            self.completion_at = None;
        } else {
            self.hidden = false;
            self.completion_at = self.completion_at.or(Some(now));
            if let Some(at) = self.completion_at
                && now.duration_since(at) >= HIDE_DELAY
            {
                self.hidden = true;
            }
        }
        if self.hidden {
            self.expanded = false;
        }

        let changed = previous.as_ref() != Some(&snapshot);
        self.snapshot = Some(snapshot);
        self.last_refreshed_at = Some(now);
        self.dirty = false;
        changed
    }

    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        let Some(at) = self.completion_at else {
            return false;
        };
        if snapshot.tasks.is_empty() {
            return false;
        }
        if now.duration_since(at) >= HIDE_DELAY && !self.hidden {
            self.hidden = true;
            self.expanded = false;
            true
        } else {
            false
        }
    }

    pub(crate) fn completion_timestamp(&self, id: &str) -> Option<Instant> {
        self.completion_timestamps.get(id).copied()
    }

    pub(crate) fn active_task_status_label(&self) -> Option<String> {
        if !self.is_visible() {
            return None;
        }
        let task = self
            .snapshot
            .as_ref()?
            .tasks
            .iter()
            .find(|task| task.status == TaskStatusKind::InProgress)?;
        let label = task
            .active_form
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&task.subject)
            .trim();
        if label.is_empty() {
            return None;
        }
        if label.ends_with('…') || label.ends_with("...") {
            Some(label.to_string())
        } else {
            Some(format!("{label}…"))
        }
    }
}

fn parse_task_list_snapshot(
    value: orbcode_app_server_client::TaskListResult,
) -> Option<TaskListSnapshot> {
    let mut tasks = Vec::with_capacity(value.tasks.len());
    let mut summary = TaskListSummary::default();
    for entry in value.tasks {
        let status = match entry.status.as_str() {
            "InProgress" => TaskStatusKind::InProgress,
            "Completed" => TaskStatusKind::Completed,
            _ => TaskStatusKind::Pending,
        };
        match status {
            TaskStatusKind::InProgress => summary.in_progress += 1,
            TaskStatusKind::Completed => summary.completed += 1,
            TaskStatusKind::Pending => summary.pending += 1,
        }
        summary.total += 1;
        tasks.push(TaskView {
            id: entry.id,
            subject: entry.subject,
            description: entry.description,
            active_form: None,
            owner: None,
            status,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            open_blockers: Vec::new(),
        });
    }
    Some(TaskListSnapshot {
        task_list_id: value.task_list_id,
        directory: value.directory,
        tasks,
        summary,
        fingerprint: 0,
    })
}

pub(crate) fn task_tool_changes_panel(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "TaskCreate"
            | "task-create"
            | "TaskUpdate"
            | "task-update"
            | "TaskList"
            | "task-list"
            | "TaskGet"
            | "task-get"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use orbcode_tools::{TaskListSnapshot, TaskListSummary, TaskStatusKind, TaskView};

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
            directory: PathBuf::from("/tmp/orbcode-task-panel-tests"),
            tasks,
            summary,
            fingerprint: 0,
        }
    }

    fn task(id: &str, status: TaskStatusKind) -> TaskView {
        TaskView {
            id: id.to_string(),
            subject: format!("Task {id}"),
            description: String::new(),
            active_form: None,
            owner: None,
            status,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            open_blockers: Vec::new(),
        }
    }

    fn active_panel() -> TaskPanelState {
        let mut panel = TaskPanelState::new(Some("test-session"), true);
        panel.awaiting_session_activity = false;
        panel
    }

    #[test]
    fn empty_snapshot_hides_panel() {
        let mut panel = active_panel();
        panel.apply_snapshot(snapshot_with(vec![]), Instant::now());
        assert!(!panel.is_visible());
    }

    #[test]
    fn stale_tasks_stay_hidden_on_startup() {
        let mut panel = TaskPanelState::new(Some("test-session"), true);
        panel.apply_snapshot(
            snapshot_with(vec![task("1", TaskStatusKind::Pending)]),
            Instant::now(),
        );
        assert!(
            !panel.is_visible(),
            "stale tasks from a previous session should not auto-show"
        );
    }

    #[test]
    fn new_task_auto_expands_panel() {
        let mut panel = active_panel();
        panel.apply_snapshot(snapshot_with(vec![]), Instant::now());
        panel.apply_snapshot(
            snapshot_with(vec![task("1", TaskStatusKind::Pending)]),
            Instant::now(),
        );
        assert!(panel.is_visible());
        assert!(
            panel.is_expanded(),
            "panel should auto-expand on first task"
        );
    }

    #[test]
    fn toggle_reveals_stale_tasks() {
        let mut panel = TaskPanelState::new(Some("test-session"), true);
        panel.apply_snapshot(
            snapshot_with(vec![task("1", TaskStatusKind::Pending)]),
            Instant::now(),
        );
        assert!(!panel.is_visible());
        panel.toggle();
        assert!(panel.is_visible(), "toggle should reveal stale tasks");
        assert!(panel.is_expanded());
    }

    #[test]
    fn user_collapse_persists_through_refresh_without_new_tasks() {
        let mut panel = active_panel();
        panel.apply_snapshot(snapshot_with(vec![]), Instant::now());
        panel.apply_snapshot(
            snapshot_with(vec![task("1", TaskStatusKind::Pending)]),
            Instant::now(),
        );
        panel.toggle();
        assert!(!panel.is_expanded());
        panel.apply_snapshot(
            snapshot_with(vec![task("1", TaskStatusKind::InProgress)]),
            Instant::now(),
        );
        assert!(
            !panel.is_expanded(),
            "user collapse should survive non-creation refresh"
        );
    }

    #[test]
    fn new_task_reopens_user_collapsed_panel() {
        let mut panel = active_panel();
        panel.apply_snapshot(snapshot_with(vec![]), Instant::now());
        panel.apply_snapshot(
            snapshot_with(vec![task("1", TaskStatusKind::Pending)]),
            Instant::now(),
        );
        panel.toggle();
        assert!(!panel.is_expanded());
        panel.apply_snapshot(
            snapshot_with(vec![
                task("1", TaskStatusKind::Pending),
                task("2", TaskStatusKind::Pending),
            ]),
            Instant::now(),
        );
        assert!(
            panel.is_expanded(),
            "creating a new task should reopen the panel"
        );
    }

    #[test]
    fn all_completed_tasks_clear_after_hide_delay() {
        let mut panel = active_panel();
        let now = Instant::now();
        panel.apply_snapshot(snapshot_with(vec![]), now);
        panel.apply_snapshot(
            snapshot_with(vec![task("1", TaskStatusKind::InProgress)]),
            now,
        );
        assert!(panel.is_visible());
        panel.apply_snapshot(
            snapshot_with(vec![task("1", TaskStatusKind::Completed)]),
            now,
        );
        assert!(
            panel.is_visible(),
            "completed list lingers within grace period"
        );
        panel.apply_snapshot(
            snapshot_with(vec![task("1", TaskStatusKind::Completed)]),
            now + HIDE_DELAY,
        );
        assert!(
            !panel.is_visible(),
            "completed list hides after grace period"
        );
    }

    #[test]
    fn fallback_refresh_required_after_poll_interval() {
        let mut panel = TaskPanelState::new(Some("test-session"), true);
        let now = Instant::now();
        panel.apply_snapshot(snapshot_with(vec![task("1", TaskStatusKind::Pending)]), now);
        assert!(!panel.needs_refresh(now));
        assert!(panel.needs_refresh(now + POLL_FALLBACK));
    }

    #[test]
    fn task_tool_dispatch_table_covers_canonical_and_camel_case() {
        for name in [
            "TaskCreate",
            "task-create",
            "TaskUpdate",
            "task-update",
            "TaskList",
            "task-list",
            "TaskGet",
            "task-get",
        ] {
            assert!(
                task_tool_changes_panel(name),
                "{name} should trigger refresh"
            );
        }
        assert!(!task_tool_changes_panel("Bash"));
    }

    #[test]
    fn active_task_status_label_prefers_active_form() {
        let mut panel = active_panel();
        let mut active = task("2", TaskStatusKind::InProgress);
        active.subject = "Verify acceptance criteria".to_string();
        active.active_form = Some("Verifying acceptance criteria".to_string());
        panel.apply_snapshot(snapshot_with(vec![]), Instant::now());
        panel.apply_snapshot(
            snapshot_with(vec![task("1", TaskStatusKind::Completed), active]),
            Instant::now(),
        );

        assert_eq!(
            panel.active_task_status_label().as_deref(),
            Some("Verifying acceptance criteria…")
        );
    }

    #[test]
    fn active_task_status_label_falls_back_to_subject() {
        let mut panel = active_panel();
        let mut active = task("1", TaskStatusKind::InProgress);
        active.subject = "Run checks".to_string();
        panel.apply_snapshot(snapshot_with(vec![]), Instant::now());
        panel.apply_snapshot(snapshot_with(vec![active]), Instant::now());

        assert_eq!(
            panel.active_task_status_label().as_deref(),
            Some("Run checks…")
        );
    }
}
