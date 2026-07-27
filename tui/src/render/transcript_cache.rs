use crate::history_cell::cells::TranscriptCell;
use crate::history_cell::state::{
    TranscriptRenderCacheKey, flatten_transcript_cells, hash_string_value,
};
use crate::render::text_utils::StyledLine;
use crate::render::transcript_cell::{CellRenderMode, render_committed_transcript_cell_lines};
use crate::state::TuiState;

impl TuiState {
    pub(crate) fn visible_stable_transcript_lines(
        &mut self,
        transcript_width: usize,
    ) -> Vec<StyledLine> {
        if self.history_flushed_message_count > 0 {
            return self.unemitted_stable_transcript_lines(transcript_width);
        }

        self.ensure_stable_transcript_render_cache(transcript_width);
        let mut lines = self.transcript_ui.render_cache.lines.clone();
        lines.extend(self.dynamic_stable_transcript_lines(transcript_width));
        lines
    }

    pub(crate) fn ensure_stable_transcript_render_cache(&mut self, transcript_width: usize) {
        self.refresh_transcript_ui_state();
        let static_cell_count = self.static_transcript_cell_count();
        let key = self.stable_transcript_render_cache_key(transcript_width, static_cell_count);
        if self.transcript_ui.render_cache.is_current(&key) {
            return;
        }

        let lines =
            flatten_transcript_cells(&self.current_message_transcript_cells_from_state_range(
                transcript_width,
                0,
                static_cell_count,
                true,
            ));
        self.transcript_ui.render_cache.store(key, lines);
    }

    pub(crate) fn committed_history_cells_for_emission_with_message_ids(
        &mut self,
        transcript_width: usize,
    ) -> Vec<(Vec<StyledLine>, Option<String>)> {
        self.refresh_transcript_ui_state();
        let mut cells = Vec::new();
        let banner = self.intro_banner_cell(transcript_width);
        if !banner.is_empty() {
            cells.push((banner, None));
        }
        let last_thinking_block = None;
        for cell in self
            .transcript_ui
            .cells
            .iter()
            .take(self.static_transcript_cell_count())
        {
            let rendered = render_committed_transcript_cell_lines(
                cell,
                &self.cwd,
                CellRenderMode::Brief,
                last_thinking_block.as_ref(),
                transcript_width,
                &self.model_display_name,
            );
            if !rendered.is_empty() {
                let message_id = match cell {
                    TranscriptCell::Message(message) => Some(message.id.clone()),
                    _ => None,
                };
                cells.push((rendered, message_id));
            }
        }
        cells
    }

    fn unemitted_stable_transcript_lines(&mut self, transcript_width: usize) -> Vec<StyledLine> {
        self.refresh_transcript_ui_state();
        let static_cell_count = self.static_transcript_cell_count();
        let emitted_cell_count = self.history_flushed_message_count;
        let banner_cell_count = usize::from(!self.intro_banner_cell(transcript_width).is_empty());
        // `history_flushed_message_count` counts cells in EMISSION space, which
        // skips cells that render empty (see
        // `committed_history_cells_for_emission_with_message_ids`). It is NOT a
        // raw index into `transcript_ui.cells`: an empty-rendering committed
        // cell before the boundary would make a raw index too small, so the
        // trailing already-emitted cell gets re-rendered into the live viewport
        // (visible duplication). Map the emission count back to a raw cell
        // index through the same empty-skip filter.
        let emitted_committed_cells = emitted_cell_count.saturating_sub(banner_cell_count);
        let start_raw = self.raw_cell_index_after_emitted(
            transcript_width,
            static_cell_count,
            emitted_committed_cells,
        );

        let mut lines =
            flatten_transcript_cells(&self.current_message_transcript_cells_from_state_range(
                transcript_width,
                start_raw,
                static_cell_count,
                false,
            ));
        lines.extend(self.dynamic_stable_transcript_lines(transcript_width));
        lines
    }

    /// Maps an emission-space count of already-flushed committed cells to a raw
    /// index into `transcript_ui.cells`, skipping cells that render empty (which
    /// the emission list also skips). Returns the raw index just past the
    /// `emitted_committed_cells`-th non-empty committed cell.
    fn raw_cell_index_after_emitted(
        &self,
        transcript_width: usize,
        static_cell_count: usize,
        emitted_committed_cells: usize,
    ) -> usize {
        if emitted_committed_cells == 0 {
            return 0;
        }
        let last_thinking_block = None;
        let mut non_empty_seen = 0usize;
        for (index, cell) in self
            .transcript_ui
            .cells
            .iter()
            .take(static_cell_count)
            .enumerate()
        {
            let rendered = render_committed_transcript_cell_lines(
                cell,
                &self.cwd,
                CellRenderMode::Brief,
                last_thinking_block.as_ref(),
                transcript_width,
                &self.model_display_name,
            );
            if !rendered.is_empty() {
                non_empty_seen += 1;
                if non_empty_seen == emitted_committed_cells {
                    return index + 1;
                }
            }
        }
        static_cell_count
    }

    pub(crate) fn current_message_transcript_cells_from_state_range(
        &self,
        transcript_width: usize,
        start: usize,
        end: usize,
        include_banner: bool,
    ) -> Vec<Vec<StyledLine>> {
        let mut cells = Vec::new();
        let banner = self.intro_banner_cell(transcript_width);
        if include_banner && !banner.is_empty() {
            cells.push(banner);
        }
        let last_thinking_block = None;

        let end = end.min(self.transcript_ui.cells.len());
        for (index, cell) in self
            .transcript_ui
            .cells
            .iter()
            .enumerate()
            .skip(start)
            .take(end.saturating_sub(start))
        {
            let is_last_cell = index + 1 >= self.transcript_ui.cells.len();
            let rendered = self.render_current_transcript_cell_lines(
                cell,
                CellRenderMode::Brief,
                is_last_cell,
                last_thinking_block.as_ref(),
                transcript_width,
            );
            if !rendered.is_empty() {
                cells.push(rendered);
            }
        }

        cells
    }

    pub(crate) fn dynamic_stable_transcript_lines(
        &self,
        transcript_width: usize,
    ) -> Vec<StyledLine> {
        let start = self.static_transcript_cell_count();
        if start >= self.transcript_ui.cells.len() {
            return Vec::new();
        }
        flatten_transcript_cells(&self.current_message_transcript_cells_from_state_range(
            transcript_width,
            start,
            self.transcript_ui.cells.len(),
            false,
        ))
    }

    pub(crate) fn static_transcript_cell_count(&self) -> usize {
        self.dynamic_transcript_cell_start_index()
            .unwrap_or(self.transcript_ui.cells.len())
    }

    fn dynamic_transcript_cell_start_index(&self) -> Option<usize> {
        self.transcript_ui
            .cells
            .iter()
            .enumerate()
            .find_map(|(index, cell)| {
                self.transcript_cell_needs_dynamic_render(cell, index)
                    .then_some(index)
            })
    }

    fn transcript_cell_needs_dynamic_render(&self, cell: &TranscriptCell, index: usize) -> bool {
        let is_last_cell = index + 1 >= self.transcript_ui.cells.len();
        match cell {
            TranscriptCell::AgentGroup(group) => {
                self.agent_group_has_in_progress_tool_use(group)
                    || group.agents.iter().any(|agent| {
                        self.find_live_tool_activity_by_tool_use_id(&agent.tool_use_id)
                            .is_some_and(|activity| {
                                self.should_keep_live_tool_activity(activity)
                                    && !self.transcript_has_tool_result(&agent.tool_use_id)
                            })
                    })
                    || self.committed_agent_group_is_pending_active_turn(group)
                    || (self.request_in_flight && is_last_cell && group.has_unresolved_tool_uses())
            }
            TranscriptCell::ActivityGroup(group) => {
                self.group_has_in_progress_tool_use(group)
                    || group.tool_use_ids.iter().any(|tool_use_id| {
                        self.find_live_tool_activity_by_tool_use_id(tool_use_id)
                            .is_some_and(|activity| {
                                self.should_keep_live_tool_activity(activity)
                                    && !self.transcript_has_tool_result(tool_use_id)
                            })
                    })
                    || self.committed_activity_group_is_pending_active_turn(group)
                    || (self.request_in_flight && is_last_cell && group.has_unresolved_tool_uses())
            }
            TranscriptCell::Tool(card) => {
                self.find_live_tool_activity_by_tool_use_id(&card.tool_use_id)
                    .is_some_and(|activity| {
                        self.should_keep_live_tool_activity(activity)
                            && !self.transcript_has_tool_result(&card.tool_use_id)
                    })
                    || self.committed_tool_cell_is_pending_active_turn(card)
                    || (card.is_active
                        && (self.in_progress_tool_use_ids.contains(&card.tool_use_id)
                            || (self.request_in_flight && is_last_cell)))
            }
            TranscriptCell::Message(message) => !self.hook_progress_for_message(message).is_empty(),
        }
    }

    fn stable_transcript_render_cache_key(
        &self,
        transcript_width: usize,
        static_cell_count: usize,
    ) -> TranscriptRenderCacheKey {
        TranscriptRenderCacheKey {
            transcript_width,
            static_cell_count,
            model_signature: hash_string_value(&self.model_display_name),
            cwd_signature: hash_string_value(&self.cwd.to_string_lossy()),
            source_signature: self.transcript_ui.source_signature,
            active_thinking_visible: self.is_active_thinking_visible(),
            blink_visible: self
                .stable_transcript_has_blinking_cell(static_cell_count)
                .then(|| self.current_tool_blink_visible()),
        }
    }

    fn stable_transcript_has_blinking_cell(&self, static_cell_count: usize) -> bool {
        self.transcript_ui
            .cells
            .iter()
            .take(static_cell_count)
            .enumerate()
            .any(|(index, cell)| {
                let is_last_cell = index + 1 >= self.transcript_ui.cells.len();
                match cell {
                    TranscriptCell::AgentGroup(group) => {
                        self.agent_group_has_in_progress_tool_use(group)
                            || self.committed_agent_group_is_pending_active_turn(group)
                            || (self.request_in_flight
                                && is_last_cell
                                && group.has_unresolved_tool_uses())
                    }
                    TranscriptCell::ActivityGroup(group) => {
                        self.group_has_in_progress_tool_use(group)
                            || self.committed_activity_group_is_pending_active_turn(group)
                            || (self.request_in_flight
                                && is_last_cell
                                && group.has_unresolved_tool_uses())
                    }
                    TranscriptCell::Tool(card) => {
                        let has_live_activity = self
                            .find_live_tool_activity_by_tool_use_id(&card.tool_use_id)
                            .is_some_and(|activity| self.should_keep_live_tool_activity(activity));
                        (card.is_active
                            || has_live_activity
                            || self.committed_tool_cell_is_pending_active_turn(card))
                            && (self.in_progress_tool_use_ids.contains(&card.tool_use_id)
                                || (self.request_in_flight
                                    && (is_last_cell
                                        || self.committed_tool_cell_is_pending_active_turn(card))))
                    }
                    TranscriptCell::Message(_) => false,
                }
            })
    }
}
