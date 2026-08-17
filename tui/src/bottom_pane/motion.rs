use crate::bottom_pane::input_layout::{
    build_input_layout, input_cursor_for_column, input_cursor_for_display_column,
    input_display_column, input_inner_width,
};
use crate::bottom_pane::vim::{
    FindKind, LastFind, current_line_end_boundary, current_line_end_motion, current_line_start,
    end_big_word, end_prev_big_word, end_prev_vim_word, end_vim_word, find_character,
    find_matching_delimiter, first_non_blank_in_line, go_to_last_line as vim_go_to_last_line,
    go_to_line_from_command, next_big_word_start, next_char_boundary, next_vim_word_start,
    prev_big_word_start, prev_char_boundary, prev_vim_word_start,
};
use crate::state::TuiState;

impl TuiState {
    pub(crate) fn move_cursor_left_from_insert_exit(&mut self) {
        if self.input_cursor == 0 {
            return;
        }

        let previous = prev_char_boundary(&self.input, self.input_cursor);
        if &self.input[previous..self.input_cursor] != "\n" {
            self.input_cursor = previous;
        }
    }

    pub(crate) fn move_cursor_prev_vim_word(&mut self) {
        self.input_cursor = prev_vim_word_start(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_next_vim_word(&mut self) {
        self.input_cursor = next_vim_word_start(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_end_vim_word(&mut self) {
        self.input_cursor = end_vim_word(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_prev_big_word(&mut self) {
        self.input_cursor = prev_big_word_start(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_next_big_word(&mut self) {
        self.input_cursor = next_big_word_start(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_end_big_word(&mut self) {
        self.input_cursor = end_big_word(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_end_prev_vim_word(&mut self) {
        self.input_cursor = end_prev_vim_word(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_end_prev_big_word(&mut self) {
        self.input_cursor = end_prev_big_word(&self.input, self.input_cursor);
    }

    pub(crate) fn apply_find(&mut self, kind: FindKind, target: char, count: Option<usize>) {
        let count = count.unwrap_or(1);
        if let Some(index) = find_character(&self.input, self.input_cursor, target, kind, count) {
            self.input_cursor = index;
            self.last_find = Some(LastFind { kind, target });
        }
    }

    pub(crate) fn repeat_last_find(&mut self, reverse: bool, count: usize) {
        let Some(last_find) = self.last_find else {
            return;
        };

        let kind = if reverse {
            match last_find.kind {
                FindKind::Forward => FindKind::Backward,
                FindKind::Backward => FindKind::Forward,
                FindKind::TillForward => FindKind::TillBackward,
                FindKind::TillBackward => FindKind::TillForward,
            }
        } else {
            last_find.kind
        };

        let search_cursor = if reverse {
            self.input_cursor
        } else {
            match kind {
                FindKind::TillForward => next_char_boundary(&self.input, self.input_cursor),
                FindKind::TillBackward => prev_char_boundary(&self.input, self.input_cursor),
                FindKind::Forward | FindKind::Backward => self.input_cursor,
            }
        };

        if let Some(index) =
            find_character(&self.input, search_cursor, last_find.target, kind, count)
        {
            self.input_cursor = index;
        }
    }

    pub(crate) fn move_cursor_to_line_start(&mut self) {
        self.input_cursor = current_line_start(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_to_first_non_blank(&mut self) {
        self.input_cursor = first_non_blank_in_line(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_to_line_end(&mut self) {
        self.input_cursor = current_line_end_motion(&self.input, self.input_cursor);
    }

    pub(crate) fn move_cursor_to_line_end_insert(&mut self) {
        self.input_cursor = current_line_end_boundary(&self.input, self.input_cursor);
    }

    pub(crate) fn repeat_motion(&mut self, count: usize, mut motion: impl FnMut(&mut Self)) {
        for _ in 0..count {
            motion(self);
        }
    }

    pub(crate) fn repeat_logical_vertical_or_history(&mut self, count: usize, direction: isize) {
        for _ in 0..count {
            if self.move_cursor_logical_vertical(direction) {
                continue;
            }

            let moved_history = if direction < 0 {
                self.navigate_prompt_history_older()
            } else {
                self.navigate_prompt_history_newer()
            };
            if !moved_history {
                break;
            }
        }
    }

    pub(crate) fn move_cursor_vertical(&mut self, direction: isize) -> bool {
        let width = input_inner_width();
        let layout = build_input_layout(&self.input, self.input_cursor, width);
        if layout.lines.is_empty() {
            return false;
        }

        let target_row = if direction < 0 {
            layout.cursor_row.checked_sub(direction.unsigned_abs())
        } else {
            layout
                .cursor_row
                .checked_add(direction as usize)
                .filter(|row| *row < layout.lines.len())
        };

        let Some(target_row) = target_row else {
            return false;
        };

        if target_row == layout.cursor_row {
            return false;
        }

        let current_column = self.desired_column.unwrap_or(layout.cursor_col);
        self.input_tail_pinned = false;
        self.input_cursor = input_cursor_for_column(&layout.lines[target_row], current_column);
        self.desired_column = Some(current_column);
        true
    }

    pub(crate) fn move_cursor_logical_vertical(&mut self, direction: isize) -> bool {
        let current_start = current_line_start(&self.input, self.input_cursor);
        let current_end = current_line_end_boundary(&self.input, self.input_cursor);
        let current_column = self
            .desired_column
            .unwrap_or_else(|| input_display_column(&self.input, current_start, self.input_cursor));

        let (target_start, target_end) = if direction < 0 {
            if current_start == 0 {
                return false;
            }
            let previous_end = current_start.saturating_sub(1);
            (current_line_start(&self.input, previous_end), previous_end)
        } else {
            if current_end >= self.input.len() {
                return false;
            }
            let next_start = current_end + 1;
            (
                next_start,
                current_line_end_boundary(&self.input, next_start),
            )
        };

        self.input_tail_pinned = false;
        self.input_cursor =
            input_cursor_for_display_column(&self.input, target_start, target_end, current_column);
        self.desired_column = Some(current_column);
        true
    }

    pub(crate) fn repeat_visual_vertical_motion(&mut self, count: usize, direction: isize) {
        for _ in 0..count {
            if !self.move_cursor_vertical(direction) {
                break;
            }
        }
    }

    pub(crate) fn push_normal_count_digit(&mut self, digit: char) {
        let current = self.normal_count.unwrap_or(0);
        let next = current
            .saturating_mul(10)
            .saturating_add(digit.to_digit(10).unwrap_or(0) as usize);
        self.normal_count = Some(next);
    }

    pub(crate) fn take_normal_count(&mut self) -> Option<usize> {
        self.normal_count.take()
    }

    pub(crate) fn go_to_line(&mut self, requested_line: usize) {
        self.input_cursor = go_to_line_from_command(&self.input, requested_line);
    }

    pub(crate) fn go_to_last_line(&mut self) {
        self.input_cursor = vim_go_to_last_line(&self.input);
    }

    pub(crate) fn execute_match_pair(&mut self) {
        if let Some(offset) = find_matching_delimiter(&self.input, self.input_cursor) {
            self.input_cursor = offset;
        }
    }
}
