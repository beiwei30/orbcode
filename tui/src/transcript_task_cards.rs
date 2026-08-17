use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use orbcode_protocol::BackgroundTaskView;

pub(crate) const FINISHED_TTL: Duration = Duration::from_secs(20);

pub(crate) struct TranscriptTaskCardsState {
    session_id: Option<String>,
    rows: Vec<BackgroundTaskView>,
    finished_since: Option<Instant>,
    expiry_notified: bool,
    subscribed_task_ids: HashSet<String>,
    pending_subscriptions: VecDeque<String>,
}

impl TranscriptTaskCardsState {
    pub(crate) fn new() -> Self {
        Self {
            session_id: None,
            rows: Vec::new(),
            finished_since: None,
            expiry_notified: false,
            subscribed_task_ids: HashSet::new(),
            pending_subscriptions: VecDeque::new(),
        }
    }

    pub(crate) fn set_session_id(&mut self, session_id: impl Into<String>) {
        let next = session_id.into();
        if self.session_id.as_deref() != Some(next.as_str()) {
            self.session_id = Some(next);
            self.rows.clear();
            self.finished_since = None;
            self.expiry_notified = false;
            self.subscribed_task_ids.clear();
            self.pending_subscriptions.clear();
        }
    }

    pub(crate) fn rows(&self) -> &[BackgroundTaskView] {
        &self.rows
    }

    pub(crate) fn is_visible(&self, now: Instant) -> bool {
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

    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        let Some(at) = self.finished_since else {
            return false;
        };
        if !self.has_active() && !self.expiry_notified && now.duration_since(at) >= FINISHED_TTL {
            self.expiry_notified = true;
            return true;
        }
        false
    }

    pub(crate) fn apply_pushed_view(&mut self, view: BackgroundTaskView, now: Instant) -> bool {
        if self.session_id.as_deref() != Some(view.session_id.as_str()) {
            return false;
        }
        if view.status.is_active() && self.subscribed_task_ids.insert(view.task_id.clone()) {
            self.pending_subscriptions.push_back(view.task_id.clone());
        }
        let mut next = self.rows.clone();
        match next.iter_mut().find(|row| row.task_id == view.task_id) {
            Some(row) => *row = view,
            None => next.push(view),
        }
        next.sort_by_key(|row| std::cmp::Reverse(row.updated_at));
        self.apply_views(next, now)
    }

    pub(crate) fn drain_subscription_requests(&mut self) -> Vec<String> {
        self.pending_subscriptions.drain(..).collect()
    }

    fn has_active(&self) -> bool {
        self.rows.iter().any(|row| row.status.is_active())
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
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use orbcode_protocol::{BackgroundTaskView, BackgroundTaskViewKind, BackgroundTaskViewStatus};

    fn task(task_id: &str, status: BackgroundTaskViewStatus) -> BackgroundTaskView {
        let timestamp = Utc::now();
        BackgroundTaskView {
            task_id: task_id.to_string(),
            session_id: "session".to_string(),
            kind: BackgroundTaskViewKind::Workflow,
            status,
            description: "workflow".to_string(),
            cwd: "/tmp".to_string(),
            created_at: timestamp,
            updated_at: timestamp,
            started_at: Some(timestamp),
            finished_at: None,
            pid: None,
            exit_code: None,
            signal: None,
            error: None,
            model: None,
            provider: None,
            permission_mode: None,
            agent_type: None,
            child_session_id: None,
            cancellation_reason: None,
            label: Some("workflow".to_string()),
            log_tail: None,
            progress_events: None,
            workflow_steps: None,
        }
    }

    #[test]
    fn queues_each_active_task_subscription_once() {
        let mut state = TranscriptTaskCardsState::new();
        state.set_session_id("session");
        let now = Instant::now();

        state.apply_pushed_view(task("workflow-1", BackgroundTaskViewStatus::Running), now);
        state.apply_pushed_view(task("workflow-1", BackgroundTaskViewStatus::Running), now);

        assert_eq!(
            state.drain_subscription_requests(),
            vec!["workflow-1".to_string()]
        );
        assert!(state.drain_subscription_requests().is_empty());
    }

    #[test]
    fn finished_expiry_requests_exactly_one_redraw() {
        let mut state = TranscriptTaskCardsState::new();
        state.set_session_id("session");
        let start = Instant::now();
        state.apply_pushed_view(
            task("workflow-1", BackgroundTaskViewStatus::Completed),
            start,
        );

        assert!(state.is_visible(start));
        assert!(!state.tick(start + FINISHED_TTL - Duration::from_millis(1)));
        assert!(state.tick(start + FINISHED_TTL));
        assert!(!state.tick(start + FINISHED_TTL));
        assert!(!state.tick(start + FINISHED_TTL + Duration::from_secs(10)));
        assert!(!state.is_visible(start + FINISHED_TTL));
        state.apply_pushed_view(
            task("workflow-1", BackgroundTaskViewStatus::Completed),
            start + FINISHED_TTL + Duration::from_secs(11),
        );
        assert!(!state.tick(start + FINISHED_TTL * 2 + Duration::from_secs(11)));
    }

    #[test]
    fn active_and_new_completed_generations_reset_expiry_latch() {
        let mut state = TranscriptTaskCardsState::new();
        state.set_session_id("session");
        let start = Instant::now();
        state.apply_pushed_view(
            task("workflow-1", BackgroundTaskViewStatus::Completed),
            start,
        );
        assert!(state.tick(start + FINISHED_TTL));
        assert!(!state.tick(start + FINISHED_TTL));

        let active_at = start + FINISHED_TTL + Duration::from_secs(1);
        state.apply_pushed_view(
            task("workflow-2", BackgroundTaskViewStatus::Running),
            active_at,
        );
        assert_eq!(
            state.drain_subscription_requests(),
            vec!["workflow-2".to_string()]
        );
        assert!(!state.tick(active_at + FINISHED_TTL));

        let completed_at = active_at + Duration::from_secs(1);
        state.apply_pushed_view(
            task("workflow-2", BackgroundTaskViewStatus::Completed),
            completed_at,
        );
        assert!(state.tick(completed_at + FINISHED_TTL));
        assert!(!state.tick(completed_at + FINISHED_TTL));

        let next_generation_at = completed_at + FINISHED_TTL + Duration::from_secs(1);
        state.apply_pushed_view(
            task("workflow-3", BackgroundTaskViewStatus::Completed),
            next_generation_at,
        );
        assert!(!state.tick(next_generation_at + FINISHED_TTL - Duration::from_millis(1)));
        assert!(state.tick(next_generation_at + FINISHED_TTL));
        assert!(!state.tick(next_generation_at + FINISHED_TTL));
    }

    #[test]
    fn empty_rows_and_session_reset_do_not_request_expiry_redraw() {
        let mut state = TranscriptTaskCardsState::new();
        state.set_session_id("session");
        let start = Instant::now();
        assert!(!state.tick(start + FINISHED_TTL));

        state.apply_pushed_view(
            task("workflow-1", BackgroundTaskViewStatus::Completed),
            start,
        );
        state.set_session_id("other-session");
        assert!(state.rows().is_empty());
        assert!(!state.tick(start + FINISHED_TTL));
    }
}
