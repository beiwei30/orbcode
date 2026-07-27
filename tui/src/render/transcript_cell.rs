use crate::history_cell::agent_activity::{
    AgentActivityGroup, render_agent_activity_group_cell_lines,
};
use crate::history_cell::cells::{ORPHANED_TOOL_RESULT, TranscriptCell};
use crate::history_cell::collapsed_activity::{
    CollapsedActivityGroup, render_collapsed_activity_group_cell_lines,
};
use crate::render::message::render_message_lines_with_hook_progress;
use crate::render::text_utils::StyledLine;
use crate::state::TuiState;
use crate::tool_cell::ToolCell;
use crate::tool_cell::render::{
    active_tool_cell_from_committed_orphan, black_circle_glyph,
    queued_tool_cell_from_committed_orphan, render_tool_cell_lines, tool_cell_with_live_activity,
};
use crate::tui_theme::inactive_style;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CellRenderMode {
    Brief,
    Detail,
}

impl CellRenderMode {
    pub(crate) fn expanded(self) -> bool {
        matches!(self, CellRenderMode::Detail)
    }
}

pub(crate) fn render_committed_transcript_cell_lines(
    cell: &TranscriptCell,
    cwd: &std::path::Path,
    mode: CellRenderMode,
    last_thinking_block: Option<&(String, usize)>,
    transcript_width: usize,
    model_display_name: &str,
) -> Vec<StyledLine> {
    let expanded = mode.expanded();
    match cell {
        TranscriptCell::AgentGroup(group) => render_agent_activity_group_cell_lines(
            &group.agents,
            expanded,
            false,
            true,
            cwd,
            transcript_width,
        ),
        TranscriptCell::ActivityGroup(group) => render_collapsed_activity_group_cell_lines(
            group,
            expanded,
            false,
            true,
            cwd,
            transcript_width,
            model_display_name,
            last_thinking_block,
        ),
        TranscriptCell::Tool(card) => {
            render_tool_cell_lines(card, expanded, None, transcript_width, cwd)
        }
        TranscriptCell::Message(message) => render_message_lines_with_hook_progress(
            message,
            cwd,
            expanded,
            last_thinking_block,
            transcript_width,
            model_display_name,
            false,
            &[],
        ),
    }
}

impl TuiState {
    pub(crate) fn render_current_transcript_cell_lines(
        &self,
        cell: &TranscriptCell,
        mode: CellRenderMode,
        is_last_cell: bool,
        last_thinking_block: Option<&(String, usize)>,
        transcript_width: usize,
    ) -> Vec<StyledLine> {
        let expanded = mode.expanded();
        match cell {
            TranscriptCell::AgentGroup(group) => {
                let agents = self.agent_group_rendered_tool_cells(group, transcript_width);
                render_agent_activity_group_cell_lines(
                    &agents,
                    expanded,
                    self.agent_group_has_in_progress_tool_use(group)
                        || self.committed_agent_group_is_pending_active_turn(group)
                        || (self.request_in_flight
                            && is_last_cell
                            && group.has_unresolved_tool_uses()),
                    self.current_tool_blink_visible(),
                    &self.cwd,
                    transcript_width,
                )
            }
            TranscriptCell::ActivityGroup(group) => render_collapsed_activity_group_cell_lines(
                group,
                expanded,
                self.group_has_in_progress_tool_use(group)
                    || self.committed_activity_group_is_pending_active_turn(group)
                    || (self.request_in_flight && is_last_cell && group.has_unresolved_tool_uses()),
                self.current_tool_blink_visible(),
                &self.cwd,
                transcript_width,
                &self.model_display_name,
                last_thinking_block,
            ),
            TranscriptCell::Tool(card) => {
                let live_card = self
                    .find_live_tool_activity_by_tool_use_id(&card.tool_use_id)
                    .filter(|activity| self.should_keep_live_tool_activity(activity))
                    .map(|activity| {
                        tool_cell_with_live_activity(card, activity, &self.cwd, transcript_width)
                    });
                let pending_card =
                    self.committed_tool_cell_is_pending_active_turn(card)
                        .then(|| {
                            if self.tool_cell_is_queued_behind_permission(card) {
                                queued_tool_cell_from_committed_orphan(card)
                            } else {
                                active_tool_cell_from_committed_orphan(card)
                            }
                        });
                let rendered_card = live_card.as_ref().or(pending_card.as_ref()).unwrap_or(card);
                render_tool_cell_lines(
                    rendered_card,
                    expanded,
                    if (rendered_card.is_active
                        && self
                            .in_progress_tool_use_ids
                            .contains(&rendered_card.tool_use_id))
                        || (self.request_in_flight && is_last_cell && rendered_card.is_active)
                        || (self.committed_tool_cell_is_pending_active_turn(card)
                            && rendered_card.is_active)
                    {
                        Some((
                            if self.current_tool_blink_visible() {
                                black_circle_glyph().to_string()
                            } else {
                                " ".to_string()
                            },
                            inactive_style(),
                        ))
                    } else {
                        None
                    },
                    transcript_width,
                    &self.cwd,
                )
            }
            TranscriptCell::Message(message) => render_message_lines_with_hook_progress(
                message,
                &self.cwd,
                expanded,
                last_thinking_block,
                transcript_width,
                &self.model_display_name,
                false,
                self.hook_progress_for_message(message),
            ),
        }
    }

    pub(crate) fn committed_tool_cell_is_pending_active_turn(&self, card: &ToolCell) -> bool {
        self.request_in_flight
            && !card.is_active
            && card.is_error
            && card.status_line == ORPHANED_TOOL_RESULT
            && self.transcript_has_pending_tail_tool_use(&card.tool_use_id)
    }

    fn tool_cell_is_queued_behind_permission(&self, card: &ToolCell) -> bool {
        if self.in_progress_tool_use_ids.contains(&card.tool_use_id) {
            return false;
        }

        matches!(
            &self.overlay,
            Some(crate::overlays::OverlayState::PermissionRequest(permission))
                if permission.request.tool_use_id != card.tool_use_id
        )
    }

    pub(crate) fn committed_activity_group_is_pending_active_turn(
        &self,
        group: &CollapsedActivityGroup,
    ) -> bool {
        self.request_in_flight
            && group.has_unresolved_tool_uses()
            && group.tool_use_ids.iter().any(|tool_use_id| {
                !group.matched_tool_use_ids.contains(tool_use_id)
                    && self.transcript_has_pending_tail_tool_use(tool_use_id)
            })
    }

    pub(crate) fn committed_agent_group_is_pending_active_turn(
        &self,
        group: &AgentActivityGroup,
    ) -> bool {
        self.request_in_flight
            && group.has_unresolved_tool_uses()
            && group.agents.iter().any(|agent| {
                agent.is_active && self.transcript_has_pending_tail_tool_use(&agent.tool_use_id)
            })
    }

    fn agent_group_rendered_tool_cells(
        &self,
        group: &AgentActivityGroup,
        transcript_width: usize,
    ) -> Vec<ToolCell> {
        group
            .agents
            .iter()
            .map(|card| {
                self.find_live_tool_activity_by_tool_use_id(&card.tool_use_id)
                    .filter(|activity| self.should_keep_live_tool_activity(activity))
                    .map(|activity| {
                        tool_cell_with_live_activity(card, activity, &self.cwd, transcript_width)
                    })
                    .or_else(|| {
                        self.committed_tool_cell_is_pending_active_turn(card)
                            .then(|| {
                                if self.tool_cell_is_queued_behind_permission(card) {
                                    queued_tool_cell_from_committed_orphan(card)
                                } else {
                                    active_tool_cell_from_committed_orphan(card)
                                }
                            })
                    })
                    .unwrap_or_else(|| card.clone())
            })
            .collect()
    }
}
