use anyhow::Result;
use orbcode_app_server_client::{AppClient, GoalContinuation};
use orbcode_protocol::{SessionGoalStatus, StreamEvent};
use tokio::sync::mpsc;

use crate::state::TuiState;

impl TuiState {
    pub(crate) async fn steer_followup(
        &mut self,
        app_server: &AppClient,
        content: String,
    ) -> Result<()> {
        app_server.steer_turn(&self.session_id, &content).await?;
        self.remember_prompt_history(&content);
        self.steered_followups.push_back(content);
        self.set_status_line("Message will be submitted after the next tool call.");
        Ok(())
    }

    pub(crate) fn queue_followup(&mut self, content: String) {
        self.remember_prompt_history(&content);
        self.queued_followups.push_back(content);
        self.set_status_line("Message queued for the next turn.");
    }

    pub(crate) fn pop_last_followup(&mut self) -> Option<String> {
        if let Some(content) = self.queued_followups.pop_back() {
            return Some(content);
        }
        self.steered_followups.pop_back()
    }

    pub(crate) fn remove_committed_steered_followup(&mut self, content: &str) {
        if let Some(index) = self
            .steered_followups
            .iter()
            .position(|pending| pending == content)
        {
            self.steered_followups.remove(index);
        }
    }

    pub(crate) fn prompt_followup_line_count(&self) -> usize {
        let mut lines = 0;
        if !self.steered_followups.is_empty() {
            lines += 1 + self.steered_followups.len();
        }
        if !self.queued_followups.is_empty() {
            lines += 1 + self.queued_followups.len() + 1; // +1 for hint line
        }
        lines
    }

    pub(crate) fn take_pending_followups_for_immediate_send(&mut self) -> Option<String> {
        let messages = self
            .steered_followups
            .drain(..)
            .chain(self.queued_followups.drain(..))
            .collect::<Vec<_>>();
        join_followups(messages)
    }

    pub(crate) async fn submit_queued_followups_if_idle(
        &mut self,
        app_server: &AppClient,
        turn_events: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
    ) -> Result<()> {
        if turn_events.is_some() {
            return Ok(());
        }
        let messages = self.queued_followups.drain(..).collect::<Vec<_>>();
        let Some(prompt) = join_followups(messages) else {
            return Ok(());
        };
        *turn_events = Some(
            app_server
                .submit_turn_stream(&self.session_id, prompt)
                .await?,
        );
        self.set_status_line("Starting queued follow-up...");
        Ok(())
    }

    pub(crate) async fn continue_goal_if_idle(
        &mut self,
        app_server: &AppClient,
        turn_events: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
    ) -> Result<()> {
        if turn_events.is_some() || !self.queued_followups.is_empty() {
            return Ok(());
        }
        let Some(goal) = app_server.get_goal(&self.session_id).await? else {
            return Ok(());
        };
        if goal.status != SessionGoalStatus::Active {
            return Ok(());
        }
        match app_server
            .continue_goal(&self.session_id, &goal.goal_id, goal.revision)
            .await?
        {
            GoalContinuation::Started { events, .. } => {
                *turn_events = Some(events);
                self.set_status_line("Continuing persistent goal...");
            }
            GoalContinuation::NotStarted { .. } => {}
        }
        Ok(())
    }
}

fn join_followups(messages: Vec<String>) -> Option<String> {
    if messages.is_empty() {
        None
    } else {
        Some(messages.join("\n\n"))
    }
}
