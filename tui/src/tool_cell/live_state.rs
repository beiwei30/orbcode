use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};
use serde_json::Value;

use crate::history_cell::agent_activity::AgentActivityGroup;
use crate::history_cell::cells::{
    is_pending_tool_tail_neutral_message, transcript_has_tool_result_in_messages,
    transcript_has_unresolved_tool_use_in_messages,
};
use crate::history_cell::collapsed_activity::CollapsedActivityGroup;
use crate::state::TuiState;
use crate::tool_cell::summary::merge_tool_result_progress_metadata;

pub(crate) const LIVE_TOOL_PROGRESS_MESSAGE_LIMIT: usize = 64;

/// Whether two live activities represent the same tool card. A non-empty
/// `tool_use_id` is the primary key; when it is empty (e.g. a permission request
/// that arrives before the tool_use id is known) fall back to `request_id` so
/// two distinct empty-id requests don't collapse into a single card.
fn live_tool_activities_match(a: &LiveToolActivity, b: &LiveToolActivity) -> bool {
    if !a.tool_use_id.is_empty() || !b.tool_use_id.is_empty() {
        return a.tool_use_id == b.tool_use_id;
    }
    match (a.request_id.as_deref(), b.request_id.as_deref()) {
        (Some(left), Some(right)) => left == right,
        // Both ids empty: treat as the same only if neither has a request_id
        // (preserves the prior single-empty-card behavior).
        (None, None) => true,
        _ => false,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LiveToolActivity {
    pub(crate) request_id: Option<String>,
    pub(crate) tool_use_id: String,
    pub(crate) tool_name: String,
    pub(crate) tool_input: String,
    pub(crate) status_line: String,
    pub(crate) progress_messages: Vec<Value>,
    pub(crate) is_error: bool,
}

impl LiveToolActivity {
    pub(crate) fn push_progress_message(&mut self, progress: Value) -> bool {
        if self
            .progress_messages
            .last()
            .is_some_and(|existing| existing == &progress)
        {
            return false;
        }
        self.progress_messages.push(progress);
        self.retain_recent_progress_messages();
        true
    }

    pub(crate) fn retain_recent_progress_messages(&mut self) {
        let overflow = self
            .progress_messages
            .len()
            .saturating_sub(LIVE_TOOL_PROGRESS_MESSAGE_LIMIT);
        if overflow > 0 {
            self.progress_messages.drain(..overflow);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LiveToolCells {
    pub(crate) activities: Vec<LiveToolActivity>,
}

impl LiveToolCells {
    #[cfg(test)]
    pub(crate) fn latest(&self) -> Option<&LiveToolActivity> {
        self.activities.last()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.activities.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.activities.len()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &LiveToolActivity> {
        self.activities.iter()
    }

    pub(crate) fn clear(&mut self) {
        self.activities.clear();
    }

    pub(crate) fn upsert(&mut self, mut activity: LiveToolActivity) {
        activity.retain_recent_progress_messages();
        if let Some(existing) = self
            .activities
            .iter_mut()
            .find(|existing| live_tool_activities_match(existing, &activity))
        {
            *existing = activity;
        } else {
            self.activities.push(activity);
        }
    }

    pub(crate) fn get_mut_or_push(
        &mut self,
        mut activity: LiveToolActivity,
    ) -> &mut LiveToolActivity {
        activity.retain_recent_progress_messages();
        if let Some(index) = self
            .activities
            .iter()
            .position(|existing| live_tool_activities_match(existing, &activity))
        {
            return &mut self.activities[index];
        }
        self.activities.push(activity);
        self.activities
            .last_mut()
            .expect("pushed live tool activity should be present")
    }

    pub(crate) fn find_by_tool_use_id_mut(
        &mut self,
        tool_use_id: &str,
    ) -> Option<&mut LiveToolActivity> {
        self.activities
            .iter_mut()
            .find(|activity| activity.tool_use_id == tool_use_id)
    }

    pub(crate) fn find_by_tool_use_id(&self, tool_use_id: &str) -> Option<&LiveToolActivity> {
        self.activities
            .iter()
            .find(|activity| activity.tool_use_id == tool_use_id)
    }

    pub(crate) fn find_by_request_id_mut(
        &mut self,
        request_id: &str,
    ) -> Option<&mut LiveToolActivity> {
        self.activities
            .iter_mut()
            .find(|activity| activity.request_id.as_deref() == Some(request_id))
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&LiveToolActivity) -> bool) {
        self.activities.retain(|activity| keep(activity));
    }
}

impl TuiState {
    pub(crate) fn transcript_has_unresolved_tool_use(&self, tool_use_id: &str) -> bool {
        let mut saw_tool_use = false;
        for message in &self.messages {
            for block in &message.blocks {
                match block {
                    TranscriptBlock::ToolUse { id, .. } if id == tool_use_id => {
                        saw_tool_use = true;
                    }
                    TranscriptBlock::ToolResult {
                        tool_use_id: id, ..
                    } if id == tool_use_id => {
                        return false;
                    }
                    _ => {}
                }
            }
        }

        saw_tool_use
    }

    pub(crate) fn transcript_has_pending_tail_tool_use(&self, tool_use_id: &str) -> bool {
        for message in self.messages.iter().rev() {
            for block in &message.blocks {
                match block {
                    TranscriptBlock::ToolResult {
                        tool_use_id: id, ..
                    } if id == tool_use_id => return false,
                    TranscriptBlock::ToolUse { id, .. } if id == tool_use_id => return true,
                    _ => {}
                }
            }

            if !is_pending_tool_tail_neutral_message(message) {
                return false;
            }
        }

        false
    }

    pub(crate) fn transcript_has_tool_result(&self, tool_use_id: &str) -> bool {
        self.messages.iter().any(|message| {
            message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult { tool_use_id: id, .. } if id == tool_use_id
                )
            })
        })
    }

    #[cfg(test)]
    pub(crate) fn latest_live_tool_activity(&self) -> Option<&LiveToolActivity> {
        self.live_tool_cells.latest()
    }

    pub(crate) fn latest_active_live_tool_activity(&self) -> Option<&LiveToolActivity> {
        self.live_tool_cells
            .activities
            .iter()
            .rev()
            .find(|activity| self.live_tool_activity_is_busy(activity))
    }

    fn live_tool_activity_is_busy(&self, activity: &LiveToolActivity) -> bool {
        self.in_progress_tool_use_ids
            .contains(&activity.tool_use_id)
            || activity.status_line == "Waiting for permission"
            || (self.request_in_flight
                && !activity.is_error
                && self.transcript_has_unresolved_tool_use(&activity.tool_use_id))
    }

    pub(crate) fn has_live_tool_activity(&self) -> bool {
        !self.live_tool_cells.is_empty()
    }

    pub(crate) fn live_tool_activities(&self) -> Vec<&LiveToolActivity> {
        self.live_tool_cells.iter().collect()
    }

    pub(crate) fn should_keep_live_tool_activity(&self, activity: &LiveToolActivity) -> bool {
        let has_committed_result = self.transcript_has_tool_result(&activity.tool_use_id);
        // Keep a permission-status cell only while its result has NOT committed.
        // Without the `!has_committed_result` guard, any activity whose status
        // merely contains "permission" (e.g. "permission denied") stayed pinned
        // as a stale live cell forever.
        let awaiting_permission = activity
            .status_line
            .to_ascii_lowercase()
            .contains("permission");
        ((awaiting_permission || activity.is_error) && !has_committed_result)
            || self
                .in_progress_tool_use_ids
                .contains(&activity.tool_use_id)
            || self.transcript_has_unresolved_tool_use(&activity.tool_use_id)
            || (self.pending_history_flush && !activity.is_error && has_committed_result)
    }

    pub(crate) fn clear_live_tool_activities(&mut self) {
        self.live_tool_cells.clear();
    }

    pub(crate) fn upsert_live_tool_activity(&mut self, activity: LiveToolActivity) {
        self.live_tool_cells.upsert(activity);
    }

    pub(crate) fn live_tool_activity_mut_or_insert(
        &mut self,
        activity: LiveToolActivity,
    ) -> &mut LiveToolActivity {
        self.live_tool_cells.get_mut_or_push(activity)
    }

    pub(crate) fn find_live_tool_activity_by_tool_use_id_mut(
        &mut self,
        tool_use_id: &str,
    ) -> Option<&mut LiveToolActivity> {
        self.live_tool_cells.find_by_tool_use_id_mut(tool_use_id)
    }

    pub(crate) fn find_live_tool_activity_by_tool_use_id(
        &self,
        tool_use_id: &str,
    ) -> Option<&LiveToolActivity> {
        self.live_tool_cells.find_by_tool_use_id(tool_use_id)
    }

    pub(crate) fn find_live_tool_activity_by_request_id_mut(
        &mut self,
        request_id: &str,
    ) -> Option<&mut LiveToolActivity> {
        self.live_tool_cells.find_by_request_id_mut(request_id)
    }

    pub(crate) fn enrich_tool_result_message_with_live_progress(
        &self,
        mut message: TranscriptMessage,
    ) -> TranscriptMessage {
        for block in &mut message.blocks {
            let TranscriptBlock::ToolResult {
                tool_use_id,
                metadata,
                ..
            } = block
            else {
                continue;
            };

            let Some(activity) = self.find_live_tool_activity_by_tool_use_id(tool_use_id) else {
                continue;
            };
            if activity.progress_messages.is_empty() {
                continue;
            }

            *metadata =
                merge_tool_result_progress_metadata(metadata.take(), &activity.progress_messages);
        }

        message
    }

    pub(crate) fn prune_completed_live_tool_activity(&mut self) {
        let pending_history_flush = self.pending_history_flush;
        let in_progress_tool_use_ids = self.in_progress_tool_use_ids.clone();
        let messages = &self.messages;
        self.live_tool_cells.retain(|activity| {
            let has_committed_result =
                transcript_has_tool_result_in_messages(messages, &activity.tool_use_id);
            activity
                .status_line
                .to_ascii_lowercase()
                .contains("permission")
                || (activity.is_error && !has_committed_result)
                || in_progress_tool_use_ids.contains(&activity.tool_use_id)
                || transcript_has_unresolved_tool_use_in_messages(messages, &activity.tool_use_id)
                || (pending_history_flush && !activity.is_error && has_committed_result)
        });
    }

    pub(crate) fn group_has_in_progress_tool_use(&self, group: &CollapsedActivityGroup) -> bool {
        group
            .tool_use_ids
            .iter()
            .any(|tool_use_id| self.in_progress_tool_use_ids.contains(tool_use_id))
    }

    pub(crate) fn agent_group_has_in_progress_tool_use(&self, group: &AgentActivityGroup) -> bool {
        group
            .agents
            .iter()
            .any(|agent| self.in_progress_tool_use_ids.contains(&agent.tool_use_id))
    }

    pub(crate) fn live_tool_activities_to_render(&self) -> Vec<&LiveToolActivity> {
        self.live_tool_activities()
            .into_iter()
            .filter(|activity| {
                self.should_keep_live_tool_activity(activity)
                    && !self.live_tool_activity_has_stable_cell(activity)
            })
            .collect()
    }

    fn live_tool_activity_has_stable_cell(&self, activity: &LiveToolActivity) -> bool {
        self.transcript_ui
            .cells
            .iter()
            .any(|cell| cell.has_tool_use_id(&activity.tool_use_id))
    }

    pub(crate) fn lookup_tool_use(&self, tool_use_id: &str) -> Option<(String, String)> {
        self.messages.iter().rev().find_map(|message| {
            if !matches!(message.role, MessageRole::Assistant) {
                return None;
            }

            message.blocks.iter().find_map(|block| match block {
                TranscriptBlock::ToolUse { id, name, input } if id == tool_use_id => {
                    Some((name.clone(), input.clone()))
                }
                _ => None,
            })
        })
    }
}
