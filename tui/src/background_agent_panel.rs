use std::time::{Duration, Instant};

use orbcode_app_server_client::AppClient;
use orbcode_protocol::BackgroundTaskView;
#[cfg(test)]
use orbcode_tools::{BackgroundTaskRecord, task_record_to_view};

pub(crate) const FINISHED_TTL: Duration = Duration::from_secs(20);
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) struct BackgroundAgentPanelState {
    session_id: Option<String>,
    rows: Vec<BackgroundTaskView>,
    last_refreshed_at: Option<Instant>,
    refresh_in_flight: bool,
    finished_since: Option<Instant>,
    expiry_notified: bool,
    dirty: bool,
}

impl BackgroundAgentPanelState {
    pub(crate) fn new() -> Self {
        Self {
            session_id: None,
            rows: Vec::new(),
            last_refreshed_at: None,
            refresh_in_flight: false,
            finished_since: None,
            expiry_notified: false,
            dirty: true,
        }
    }

    pub(crate) fn set_session_id(&mut self, session_id: impl Into<String>) {
        let next = session_id.into();
        if self.session_id.as_deref() != Some(next.as_str()) {
            self.session_id = Some(next);
            self.rows.clear();
            self.finished_since = None;
            self.expiry_notified = false;
            self.dirty = true;
        }
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    #[cfg(test)]
    pub(crate) fn rows(&self) -> &[BackgroundTaskView] {
        &self.rows
    }

    pub(crate) fn apply_pushed_view(&mut self, view: BackgroundTaskView, now: Instant) -> bool {
        if self.session_id.as_deref() != Some(view.session_id.as_str()) {
            return false;
        }
        let mut next = self.rows.clone();
        match next.iter_mut().find(|row| row.task_id == view.task_id) {
            Some(row) => *row = view,
            None => next.push(view),
        }
        next.sort_by_key(|row| std::cmp::Reverse(row.updated_at));
        self.apply_views(next, now)
    }

    /// True while any agent is still queued/running OR we are still inside the
    /// `FINISHED_TTL` grace window after the last agent terminated. Outside of
    /// both, the panel collapses.
    #[cfg(test)]
    fn is_visible(&self, now: Instant) -> bool {
        if self.rows.is_empty() {
            return false;
        }
        if self.has_active() {
            return true;
        }
        match self.finished_since {
            Some(at) => now.duration_since(at) < FINISHED_TTL,
            None => false,
        }
    }

    pub(crate) fn has_active(&self) -> bool {
        self.rows.iter().any(|row| row.status.is_active())
    }

    pub(crate) fn needs_refresh(&self, now: Instant) -> bool {
        if self.refresh_in_flight {
            return false;
        }
        if self.session_id.is_none() {
            return false;
        }
        if self.dirty {
            return true;
        }
        match self.last_refreshed_at {
            Some(at) => now.duration_since(at) >= POLL_INTERVAL,
            None => true,
        }
    }

    pub(crate) async fn refresh(&mut self, app_server: &AppClient) -> bool {
        if self.refresh_in_flight {
            return false;
        }
        let Some(session_id) = self.session_id.clone() else {
            return false;
        };
        self.refresh_in_flight = true;
        let result = app_server.list_background_jobs_summary().await;
        self.refresh_in_flight = false;
        let views = match result {
            Ok(value) => value
                .into_iter()
                .filter(|view| view.session_id == session_id)
                .collect(),
            Err(_) => {
                self.last_refreshed_at = Some(Instant::now());
                self.dirty = false;
                return false;
            }
        };
        self.apply_views(views, Instant::now())
    }

    #[cfg(test)]
    pub(crate) fn apply_records(
        &mut self,
        records: Vec<BackgroundTaskRecord>,
        now: Instant,
    ) -> bool {
        let views: Vec<BackgroundTaskView> = records.iter().map(task_record_to_view).collect();
        self.apply_views(views, now)
    }

    #[cfg(test)]
    pub(crate) fn apply_views_for_test(
        &mut self,
        views: Vec<BackgroundTaskView>,
        now: Instant,
    ) -> bool {
        self.apply_views(views, now)
    }

    fn apply_views(&mut self, next: Vec<BackgroundTaskView>, now: Instant) -> bool {
        let changed = next != self.rows;
        let next_has_active = next.iter().any(|row| row.status.is_active());
        let has_new_finished_generation = !next_has_active
            && next.iter().any(|next_row| {
                !next_row.status.is_active()
                    && !self.rows.iter().any(|current_row| {
                        current_row.task_id == next_row.task_id
                            && current_row.status == next_row.status
                    })
            });
        self.rows = next;
        if next_has_active || self.rows.is_empty() {
            self.finished_since = None;
            self.expiry_notified = false;
        } else if self.finished_since.is_none() || has_new_finished_generation {
            self.finished_since = Some(now);
            self.expiry_notified = false;
        }
        self.last_refreshed_at = Some(now);
        self.dirty = false;
        changed
    }

    /// Called on the periodic UI tick to detect that we've crossed the
    /// FINISHED_TTL boundary and should hide the panel.
    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        let Some(at) = self.finished_since else {
            return false;
        };
        if !self.has_active() && !self.expiry_notified && now.duration_since(at) >= FINISHED_TTL {
            // Don't drop rows from memory — keep them so a final refresh still
            // works — but report that visibility flipped so the renderer
            // redraws.
            self.expiry_notified = true;
            return true;
        }
        false
    }
}

pub(crate) fn background_task_tool_changes_panel(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Agent" | "agent" | "Task" | "task" | "Workflow" | "workflow"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use orbcode_tools::{BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus};

    fn record(id: &str, status: BackgroundTaskStatus) -> BackgroundTaskRecord {
        BackgroundTaskRecord {
            job_id: id.to_string(),
            session_id: "session-1".to_string(),
            prompt: format!("prompt for {id}"),
            cwd: "/tmp".to_string(),
            status,
            created_at: "2026-05-27T00:00:00Z".to_string(),
            updated_at: "2026-05-27T00:00:00Z".to_string(),
            started_at: None,
            finished_at: None,
            pid: None,
            log_path: "/tmp/log".to_string(),
            error: None,
            task_kind: BackgroundTaskKind::LocalAgent,
            tool_use_id: None,
            child_session_id: Some(format!("session-1:{id}")),
            agent_type: Some("Explore".to_string()),
            model: None,
            permission_mode: None,
            result: None,
            exit_code: None,
            signal: None,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn empty_records_collapse_panel() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        assert!(!panel.is_visible(Instant::now()));
        panel.apply_records(Vec::new(), Instant::now());
        assert!(!panel.is_visible(Instant::now()));
    }

    #[test]
    fn active_record_makes_panel_visible() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        panel.apply_records(
            vec![record("agent-aa", BackgroundTaskStatus::Running)],
            Instant::now(),
        );
        assert!(panel.is_visible(Instant::now()));
        assert!(panel.has_active());
        assert_eq!(panel.rows().len(), 1);
        assert_eq!(panel.rows()[0].task_id, "agent-aa");
    }

    #[test]
    fn finished_records_linger_then_hide_after_ttl() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        let start = Instant::now();
        panel.apply_records(
            vec![record("agent-aa", BackgroundTaskStatus::Running)],
            start,
        );
        assert!(panel.is_visible(start));
        panel.apply_records(
            vec![record("agent-aa", BackgroundTaskStatus::Completed)],
            start,
        );
        assert!(
            panel.is_visible(start),
            "completed should linger within FINISHED_TTL"
        );
        assert!(
            !panel.is_visible(start + FINISHED_TTL),
            "completed should hide after FINISHED_TTL elapsed"
        );
    }

    #[test]
    fn finished_expiry_requests_exactly_one_redraw() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        let start = Instant::now();
        panel.apply_records(
            vec![record("agent-aa", BackgroundTaskStatus::Completed)],
            start,
        );

        assert!(!panel.tick(start + FINISHED_TTL - Duration::from_millis(1)));
        assert!(panel.tick(start + FINISHED_TTL));
        assert!(!panel.tick(start + FINISHED_TTL));
        assert!(!panel.tick(start + FINISHED_TTL + Duration::from_secs(10)));
        panel.apply_records(
            vec![record("agent-aa", BackgroundTaskStatus::Completed)],
            start + FINISHED_TTL + Duration::from_secs(11),
        );
        assert!(!panel.tick(start + FINISHED_TTL * 2 + Duration::from_secs(11)));
    }

    #[test]
    fn active_and_new_completed_generations_reset_expiry_latch() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        let start = Instant::now();
        panel.apply_records(
            vec![record("agent-aa", BackgroundTaskStatus::Completed)],
            start,
        );
        assert!(panel.tick(start + FINISHED_TTL));
        assert!(!panel.tick(start + FINISHED_TTL));

        let active_at = start + FINISHED_TTL + Duration::from_secs(1);
        panel.apply_records(
            vec![record("agent-bb", BackgroundTaskStatus::Running)],
            active_at,
        );
        assert!(!panel.tick(active_at + FINISHED_TTL));

        let completed_at = active_at + Duration::from_secs(1);
        panel.apply_records(
            vec![record("agent-bb", BackgroundTaskStatus::Completed)],
            completed_at,
        );
        assert!(!panel.tick(completed_at + FINISHED_TTL - Duration::from_millis(1)));
        assert!(panel.tick(completed_at + FINISHED_TTL));
        assert!(!panel.tick(completed_at + FINISHED_TTL));

        let next_generation_at = completed_at + FINISHED_TTL + Duration::from_secs(1);
        panel.apply_records(
            vec![
                record("agent-bb", BackgroundTaskStatus::Completed),
                record("agent-cc", BackgroundTaskStatus::Completed),
            ],
            next_generation_at,
        );
        assert!(!panel.tick(next_generation_at + FINISHED_TTL - Duration::from_millis(1)));
        assert!(panel.tick(next_generation_at + FINISHED_TTL));
        assert!(!panel.tick(next_generation_at + FINISHED_TTL));
    }

    #[test]
    fn empty_rows_and_session_reset_do_not_request_expiry_redraw() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        let start = Instant::now();
        panel.apply_records(
            vec![record("agent-aa", BackgroundTaskStatus::Completed)],
            start,
        );
        assert!(panel.tick(start + FINISHED_TTL));

        panel.apply_records(Vec::new(), start + FINISHED_TTL);
        assert!(!panel.tick(start + FINISHED_TTL + Duration::from_secs(1)));

        panel.apply_records(
            vec![record("agent-bb", BackgroundTaskStatus::Completed)],
            start + FINISHED_TTL,
        );
        panel.set_session_id("session-2");
        assert!(!panel.tick(start + FINISHED_TTL * 2));
    }

    #[test]
    fn changing_session_drops_previous_rows() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        panel.apply_records(
            vec![record("agent-aa", BackgroundTaskStatus::Running)],
            Instant::now(),
        );
        assert_eq!(panel.rows().len(), 1);
        panel.set_session_id("session-2");
        assert!(panel.rows().is_empty());
        assert!(panel.needs_refresh(Instant::now()));
    }

    #[test]
    fn pushed_view_upserts_matching_session_rows() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        let mut older = task_record_to_view(&record("agent-old", BackgroundTaskStatus::Running));
        older.updated_at = Utc.with_ymd_and_hms(2026, 5, 27, 0, 0, 0).unwrap();
        let mut newer = task_record_to_view(&record("agent-new", BackgroundTaskStatus::Running));
        newer.updated_at = Utc.with_ymd_and_hms(2026, 5, 27, 0, 0, 1).unwrap();

        assert!(panel.apply_pushed_view(older, Instant::now()));
        assert!(panel.apply_pushed_view(newer, Instant::now()));
        assert_eq!(panel.rows()[0].task_id, "agent-new");
        assert_eq!(panel.rows()[1].task_id, "agent-old");

        let mut updated = panel.rows()[1].clone();
        updated.status = orbcode_protocol::BackgroundTaskViewStatus::Completed;
        updated.updated_at = Utc.with_ymd_and_hms(2026, 5, 27, 0, 0, 2).unwrap();
        assert!(panel.apply_pushed_view(updated, Instant::now()));
        assert_eq!(panel.rows().len(), 2);
        assert_eq!(panel.rows()[0].task_id, "agent-old");
        assert_eq!(
            panel.rows()[0].status,
            orbcode_protocol::BackgroundTaskViewStatus::Completed
        );
    }

    #[test]
    fn pushed_view_ignores_other_sessions() {
        let mut panel = BackgroundAgentPanelState::new();
        panel.set_session_id("session-1");
        let mut view = task_record_to_view(&record("agent-aa", BackgroundTaskStatus::Running));
        view.session_id = "session-2".to_string();

        assert!(!panel.apply_pushed_view(view, Instant::now()));
        assert!(panel.rows().is_empty());
    }

    #[test]
    fn background_task_tool_dispatch_table_covers_known_background_tools() {
        for name in ["Agent", "agent", "Task", "task", "Workflow", "workflow"] {
            assert!(
                background_task_tool_changes_panel(name),
                "{name} should trigger background task refresh"
            );
        }
        assert!(!background_task_tool_changes_panel("Bash"));
        assert!(!background_task_tool_changes_panel("TaskCreate"));
    }
}
