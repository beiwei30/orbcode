use crate::bottom_pane::vim::{
    IndentDirection, OpenLineDirection, RecordedChange, UndoState, current_line_end_boundary,
    current_line_start, first_non_blank_in_line, last_motion_offset, line_index_at_offset,
    next_char_boundary, offset_for_line, prev_char_boundary,
};
use crate::editor_mode::EditorMode;
use crate::state::TuiState;

impl TuiState {
    pub(crate) fn execute_x(&mut self, count: usize, record_change: bool) {
        if self.input_cursor >= self.input.len() {
            return;
        }
        self.push_undo_state();

        let start = self.input_cursor;
        let mut end = start;
        for _ in 0..count {
            let next = next_char_boundary(&self.input, end);
            if next == end {
                break;
            }
            end = next;
        }

        if end == start {
            return;
        }

        self.set_register(self.input[start..end].to_string(), false);
        self.input.replace_range(start..end, "");
        self.input_cursor = start.min(last_motion_offset(&self.input));

        if record_change {
            self.vim_state.last_change = Some(RecordedChange::X { count });
        }
    }

    pub(crate) fn push_undo_state(&mut self) {
        self.vim_state.undo_stack.push(UndoState {
            input: self.input.clone(),
            input_cursor: self.input_cursor,
        });
        if self.vim_state.undo_stack.len() > 256 {
            let overflow = self.vim_state.undo_stack.len() - 256;
            self.vim_state.undo_stack.drain(0..overflow);
        }
    }

    pub(crate) fn undo_last_change(&mut self) {
        let Some(previous) = self.vim_state.undo_stack.pop() else {
            return;
        };
        self.input = previous.input;
        self.input_cursor = previous.input_cursor.min(self.input.len());
        self.editor_mode = EditorMode::Normal;
        self.normal_pending = None;
        self.normal_count = None;
        self.vim_state.inserted_text.clear();
    }

    pub(crate) fn set_register(&mut self, content: String, linewise: bool) {
        self.vim_state.register = if linewise {
            content.trim_end_matches('\n').to_string()
        } else {
            content
        };
        self.vim_state.register_is_linewise = linewise;
    }

    pub(crate) fn execute_replace(&mut self, character: char, count: usize, record_change: bool) {
        if self.input_cursor >= self.input.len() {
            return;
        }
        self.push_undo_state();

        let mut offset = self.input_cursor;
        for _ in 0..count {
            if offset >= self.input.len() {
                break;
            }
            let end = next_char_boundary(&self.input, offset);
            self.input
                .replace_range(offset..end, &character.to_string());
            offset = next_char_boundary(&self.input, offset);
        }
        self.input_cursor = prev_char_boundary(&self.input, offset.min(self.input.len()));

        if record_change {
            self.vim_state.last_change = Some(RecordedChange::Replace { character, count });
        }
    }

    pub(crate) fn execute_toggle_case(&mut self, count: usize, record_change: bool) {
        if self.input_cursor >= self.input.len() {
            return;
        }
        self.push_undo_state();

        let mut offset = self.input_cursor;
        let mut toggled = 0;
        while offset < self.input.len() && toggled < count {
            let end = next_char_boundary(&self.input, offset);
            let Some(current) = self.input[offset..end].chars().next() else {
                break;
            };
            let replacement = if current.is_uppercase() {
                current.to_lowercase().to_string()
            } else {
                current.to_uppercase().to_string()
            };
            self.input.replace_range(offset..end, &replacement);
            offset = next_char_boundary(&self.input, offset);
            toggled += 1;
        }
        self.input_cursor = offset.min(self.input.len());

        if record_change {
            self.vim_state.last_change = Some(RecordedChange::ToggleCase { count });
        }
    }

    pub(crate) fn execute_join(&mut self, count: usize, record_change: bool) {
        let current_start = current_line_start(&self.input, self.input_cursor);
        let current_end = current_line_end_boundary(&self.input, self.input_cursor);
        if current_end >= self.input.len() {
            return;
        }
        self.push_undo_state();

        let original_cursor_col = self.input_cursor.saturating_sub(current_start);
        let mut lines = self
            .input
            .split('\n')
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let current_line_index = line_index_at_offset(&self.input, self.input_cursor);
        let lines_to_join = count.min(lines.len().saturating_sub(current_line_index + 1));
        if lines_to_join == 0 {
            return;
        }

        let mut joined = lines[current_line_index].clone();
        for _ in 0..lines_to_join {
            let next = lines.remove(current_line_index + 1);
            let trimmed = next.trim_start();
            if !trimmed.is_empty() {
                if !joined.ends_with(' ') && !joined.is_empty() {
                    joined.push(' ');
                }
                joined.push_str(trimmed);
            }
        }
        lines[current_line_index] = joined;
        self.input = lines.join("\n");
        let new_start = offset_for_line(&self.input, current_line_index);
        self.input_cursor = (new_start + original_cursor_col).min(self.input.len());

        if record_change {
            self.vim_state.last_change = Some(RecordedChange::Join { count });
        }
    }

    pub(crate) fn execute_indent(
        &mut self,
        direction: IndentDirection,
        count: usize,
        record_change: bool,
    ) {
        self.push_undo_state();
        let mut lines = self
            .input
            .split('\n')
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let current_line_index = line_index_at_offset(&self.input, self.input_cursor);
        let lines_to_affect = count.min(lines.len().saturating_sub(current_line_index));
        if lines_to_affect == 0 {
            return;
        }

        for line in &mut lines[current_line_index..current_line_index + lines_to_affect] {
            match direction {
                IndentDirection::Right => line.insert_str(0, "  "),
                IndentDirection::Left => {
                    if line.starts_with("  ") {
                        line.drain(..2);
                    } else if line.starts_with('\t') {
                        line.drain(..1);
                    } else {
                        while line.starts_with(' ') {
                            line.drain(..1);
                            if !line.starts_with(' ') {
                                break;
                            }
                        }
                    }
                }
            }
        }

        self.input = lines.join("\n");
        let new_line_start = offset_for_line(&self.input, current_line_index);
        let first_non_blank = first_non_blank_in_line(&self.input, new_line_start);
        self.input_cursor = first_non_blank;

        if record_change {
            self.vim_state.last_change = Some(RecordedChange::Indent { direction, count });
        }
    }

    pub(crate) fn execute_open_line(&mut self, direction: OpenLineDirection, record_change: bool) {
        self.push_undo_state();
        let inserted_at_eof;
        let insert_at = match direction {
            OpenLineDirection::Below => {
                let end = current_line_end_boundary(&self.input, self.input_cursor);
                if end >= self.input.len() {
                    inserted_at_eof = true;
                    self.input.len()
                } else {
                    inserted_at_eof = false;
                    end + 1
                }
            }
            OpenLineDirection::Above => {
                inserted_at_eof = false;
                current_line_start(&self.input, self.input_cursor)
            }
        };

        let insertion = if self.input.is_empty() { "" } else { "\n" };

        self.input.insert_str(insert_at, insertion);
        self.input_cursor = match direction {
            OpenLineDirection::Above => insert_at,
            OpenLineDirection::Below if inserted_at_eof => insert_at + insertion.len(),
            OpenLineDirection::Below => insert_at,
        };
        self.vim_state.pending_insert_change =
            record_change.then_some(RecordedChange::OpenLine { direction });
        self.enter_insert_mode();
    }

    pub(crate) fn execute_paste(&mut self, after: bool, count: usize, record_change: bool) {
        if self.vim_state.register.is_empty() {
            return;
        }
        self.push_undo_state();

        if self.vim_state.register_is_linewise {
            let lines = self
                .input
                .split('\n')
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let current_line_index = line_index_at_offset(&self.input, self.input_cursor);
            let insert_index = if after {
                current_line_index + 1
            } else {
                current_line_index
            };
            let mut repeated = Vec::new();
            for _ in 0..count {
                repeated.extend(self.vim_state.register.split('\n').map(ToOwned::to_owned));
            }
            let mut new_lines = Vec::new();
            new_lines.extend_from_slice(&lines[..insert_index.min(lines.len())]);
            new_lines.extend(repeated);
            new_lines.extend_from_slice(&lines[insert_index.min(lines.len())..]);
            self.input = new_lines.join("\n");
            self.input_cursor = offset_for_line(
                &self.input,
                insert_index.min(new_lines.len().saturating_sub(1)),
            );
            if record_change {
                self.vim_state.last_change = Some(RecordedChange::Paste { after, count });
            }
            return;
        }

        let text = self.vim_state.register.repeat(count);
        let insert_at = if after && self.input_cursor < self.input.len() {
            next_char_boundary(&self.input, self.input_cursor)
        } else {
            self.input_cursor
        };
        self.input.insert_str(insert_at, &text);
        let end = insert_at + text.len();
        self.input_cursor = prev_char_boundary(&self.input, end);
        if record_change {
            self.vim_state.last_change = Some(RecordedChange::Paste { after, count });
        }
    }
}
