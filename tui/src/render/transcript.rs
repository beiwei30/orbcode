use crate::history_cell::viewport::{
    TranscriptViewLines, selection_line_window_from_sections, visible_transcript_lines,
    visible_transcript_lines_from_visual_sections,
};
use crate::overlays::OverlayState;
use crate::render::styled_wrap::wrap_styled_lines;
use crate::render::text_utils::StyledLine;
use crate::state::TuiState;

impl TuiState {
    pub(crate) fn refresh_transcript_ui_state(&mut self) {
        self.transcript_ui
            .refresh_from_messages(&self.messages, &self.cwd);
    }

    pub(crate) fn transcript_lines_for_messages(
        &mut self,
        transcript_width: usize,
        show_empty_placeholder: bool,
    ) -> Vec<StyledLine> {
        let mut lines = self.visible_stable_transcript_lines(transcript_width);
        self.append_active_transcript_lines(&mut lines, transcript_width, show_empty_placeholder);
        lines
    }

    pub(crate) fn visible_transcript_lines_for_view(
        &mut self,
        transcript_width: usize,
        transcript_height: usize,
        show_empty_placeholder: bool,
    ) -> TranscriptViewLines {
        let requested_scroll = if self.focus_latest_message_start {
            usize::MAX / 2
        } else {
            0
        };
        if let Some(view) = self.visible_transcript_window_fast_path(
            transcript_width,
            transcript_height,
            show_empty_placeholder,
            requested_scroll,
        ) {
            return view;
        }

        let transcript_lines =
            self.transcript_lines_for_messages(transcript_width, show_empty_placeholder);
        visible_transcript_lines(
            &transcript_lines,
            transcript_width,
            transcript_height,
            requested_scroll,
        )
    }

    fn visible_transcript_window_fast_path(
        &mut self,
        transcript_width: usize,
        transcript_height: usize,
        show_empty_placeholder: bool,
        requested_scroll: usize,
    ) -> Option<TranscriptViewLines> {
        if transcript_height == 0
            || self.focus_latest_message_start
            || self.history_flushed_message_count > 0
            || self.transcript_bottom_pin_is_sticky()
            || matches!(self.overlay, Some(OverlayState::PermissionRequest(_)))
        {
            return None;
        }

        self.ensure_stable_transcript_render_cache(transcript_width);
        let dynamic_stable_lines = self.dynamic_stable_transcript_lines(transcript_width);
        let dynamic_stable_visual_lines =
            wrap_styled_lines(&dynamic_stable_lines, transcript_width.max(1));
        let stable_has_lines =
            !self.transcript_ui.render_cache.lines.is_empty() || !dynamic_stable_lines.is_empty();
        let active_lines = self.active_transcript_lines(
            transcript_width,
            show_empty_placeholder,
            stable_has_lines,
        );
        let active_visual_lines = wrap_styled_lines(&active_lines, transcript_width.max(1));
        let stable_visual_lines = &self.transcript_ui.render_cache.visual_lines;
        Some(visible_transcript_lines_from_visual_sections(
            &[
                stable_visual_lines,
                &dynamic_stable_visual_lines,
                &active_visual_lines,
            ],
            transcript_height,
            requested_scroll,
            self.transcript_ui.viewport.selection.as_ref(),
        ))
    }

    pub(crate) fn refresh_transcript_selection_lines(&mut self) {
        if self.transcript_ui.viewport.selection.is_none() {
            self.transcript_ui
                .viewport
                .set_selection_lines(Vec::new(), 0);
            return;
        }
        let transcript_width = self.transcript_ui.viewport.area.width.max(1) as usize;
        self.ensure_stable_transcript_render_cache(transcript_width);
        let dynamic_stable_lines = self.dynamic_stable_transcript_lines(transcript_width);
        let dynamic_stable_visual_lines =
            wrap_styled_lines(&dynamic_stable_lines, transcript_width.max(1));
        let stable_has_lines =
            !self.transcript_ui.render_cache.lines.is_empty() || !dynamic_stable_lines.is_empty();
        let active_lines = self.active_transcript_lines(
            transcript_width,
            self.history_flushed_message_count == 0,
            stable_has_lines,
        );
        let active_visual_lines = wrap_styled_lines(&active_lines, transcript_width.max(1));
        let stable_visual_lines = &self.transcript_ui.render_cache.visual_lines;
        let total_line_count = stable_visual_lines
            .len()
            .saturating_add(dynamic_stable_visual_lines.len())
            .saturating_add(active_visual_lines.len());
        let (lines, start) = selection_line_window_from_sections(
            &[
                stable_visual_lines,
                &dynamic_stable_visual_lines,
                &active_visual_lines,
            ],
            total_line_count,
            self.transcript_ui.viewport.selection.as_ref(),
        )
        .unwrap_or_default();
        self.transcript_ui
            .viewport
            .set_selection_lines(lines, start);
    }

    #[cfg(test)]
    fn visible_transcript_tail_fast_path(
        &mut self,
        transcript_width: usize,
        transcript_height: usize,
        show_empty_placeholder: bool,
    ) -> Option<TranscriptViewLines> {
        self.visible_transcript_window_fast_path(
            transcript_width,
            transcript_height,
            show_empty_placeholder,
            0,
        )
    }

    #[cfg(test)]
    pub(crate) fn visible_transcript_lines_for_view_without_window_fast_path(
        &mut self,
        transcript_width: usize,
        transcript_height: usize,
        show_empty_placeholder: bool,
    ) -> TranscriptViewLines {
        let requested_scroll = if self.focus_latest_message_start {
            usize::MAX / 2
        } else {
            0
        };
        if requested_scroll == 0
            && let Some(view) = self.visible_transcript_tail_fast_path(
                transcript_width,
                transcript_height,
                show_empty_placeholder,
            )
        {
            return view;
        }

        let transcript_lines =
            self.transcript_lines_for_messages(transcript_width, show_empty_placeholder);
        visible_transcript_lines(
            &transcript_lines,
            transcript_width,
            transcript_height,
            requested_scroll,
        )
    }
}
