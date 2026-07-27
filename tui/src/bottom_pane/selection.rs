use anyhow::Result;

use super::input_layout::{build_input_view_with_tail_pin, input_cursor_for_column};
use super::vim::snap_to_char_boundary_clamped;
use crate::clipboard::copy_text_to_clipboard;
use crate::prompt_state::InputSelectionState;
use crate::state::TuiState;

impl TuiState {
    pub(crate) fn has_input_selection(&self) -> bool {
        self.input_selection.is_some()
    }

    pub(crate) fn clear_input_selection(&mut self) {
        self.input_selection = None;
    }

    pub(crate) fn clamp_input_selection(&mut self) {
        let Some(selection) = self.input_selection.as_mut() else {
            return;
        };
        selection.anchor =
            snap_to_char_boundary_clamped(&self.input, 0, self.input.len(), selection.anchor);
        selection.focus =
            snap_to_char_boundary_clamped(&self.input, 0, self.input.len(), selection.focus);
    }

    pub(crate) fn selected_input_text(&self) -> Option<String> {
        let selection = self.input_selection?;
        if selection.is_collapsed() {
            return None;
        }
        let (start, end) = selection.normalized_range();
        let start = snap_to_char_boundary_clamped(&self.input, 0, self.input.len(), start);
        let end = snap_to_char_boundary_clamped(&self.input, 0, self.input.len(), end);
        if start == end {
            return None;
        }
        Some(self.input[start..end].to_string())
    }

    pub(crate) fn copy_selected_input_to_clipboard(&self) -> Result<usize> {
        let selected = self
            .selected_input_text()
            .ok_or_else(|| anyhow::anyhow!("No prompt text is selected."))?;
        copy_text_to_clipboard(&selected)?;
        Ok(selected.chars().count())
    }

    pub(crate) fn auto_copy_input_selection(&mut self) {
        if self
            .input_selection
            .is_none_or(|selection| selection.is_collapsed())
        {
            self.clear_input_selection();
            return;
        }
        let result = self.copy_selected_input_to_clipboard();
        self.clear_input_selection();
        self.report_transcript_copy_result(result);
    }

    pub(crate) fn input_cursor_from_mouse(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.input_area;
        if area.width == 0
            || area.height == 0
            || row < area.y
            || row >= area.y.saturating_add(area.height)
            || column < area.x
            || column >= area.x.saturating_add(area.width)
        {
            return None;
        }

        let row_index = row.saturating_sub(area.y) as usize;
        let input_width = area.width.saturating_sub(3).max(1) as usize;
        let input_view = build_input_view_with_tail_pin(
            &self.input,
            self.input_cursor,
            input_width,
            area.height as usize,
            self.input_tail_pinned,
        );
        let line = input_view.line_layouts.get(row_index)?;
        let text_column = column.saturating_sub(area.x.saturating_add(2)) as usize;
        Some(input_cursor_for_column(line, text_column.min(input_width)))
    }

    pub(crate) fn input_cursor_from_mouse_clamped(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.input_area;
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let max_x = area.x.saturating_add(area.width.saturating_sub(1));
        let max_y = area.y.saturating_add(area.height.saturating_sub(1));
        self.input_cursor_from_mouse(column.clamp(area.x, max_x), row.clamp(area.y, max_y))
    }

    pub(crate) fn begin_input_selection(&mut self, cursor: usize) {
        let cursor = snap_to_char_boundary_clamped(&self.input, 0, self.input.len(), cursor);
        self.input_selection = Some(InputSelectionState {
            anchor: cursor,
            focus: cursor,
        });
    }

    pub(crate) fn update_input_selection(&mut self, cursor: usize) {
        let Some(selection) = self.input_selection.as_mut() else {
            return;
        };
        selection.focus = snap_to_char_boundary_clamped(&self.input, 0, self.input.len(), cursor);
    }
}
