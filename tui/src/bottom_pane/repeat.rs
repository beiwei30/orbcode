use crate::bottom_pane::vim::RecordedChange;
use crate::editor_mode::EditorMode;
use crate::state::TuiState;

impl TuiState {
    pub(crate) fn replay_inserted_text(&mut self, text: &str, push_undo: bool) {
        if push_undo {
            self.push_undo_state();
        }

        if !text.is_empty() {
            self.input.insert_str(self.input_cursor, text);
            self.input_cursor += text.len();
        }

        if self.editor_mode == EditorMode::Insert {
            self.vim_state.inserted_text.clear();
            self.vim_state.pending_insert_change = None;
            let _ = self.handle_escape_key(false);
        } else if !text.is_empty() {
            self.move_cursor_left_from_insert_exit();
        }
    }

    pub(crate) fn repeat_last_change(&mut self) {
        let Some(change) = self.vim_state.last_change.clone() else {
            return;
        };

        match change {
            RecordedChange::Insert(text) => self.replay_inserted_text(&text, true),
            RecordedChange::CompositeInsert { base, text } => {
                match *base {
                    RecordedChange::OpenLine { direction } => {
                        self.execute_open_line(direction, false)
                    }
                    RecordedChange::LineOp { op, count } => self.execute_line_op(op, count, false),
                    RecordedChange::OperatorMotion { op, motion, count } => {
                        self.execute_operator_motion(op, motion, count, false)
                    }
                    RecordedChange::OperatorFind {
                        op,
                        kind,
                        target,
                        count,
                    } => self.execute_operator_find(op, kind, target, count, false),
                    RecordedChange::OperatorTextObject {
                        op,
                        scope,
                        kind,
                        count,
                    } => self.execute_operator_text_object(op, scope, kind, count, false),
                    _ => return,
                }
                if self.editor_mode == EditorMode::Insert {
                    self.replay_inserted_text(&text, false);
                }
            }
            RecordedChange::X { count } => self.execute_x(count, false),
            RecordedChange::Replace { character, count } => {
                self.execute_replace(character, count, false)
            }
            RecordedChange::ToggleCase { count } => self.execute_toggle_case(count, false),
            RecordedChange::Indent { direction, count } => {
                self.execute_indent(direction, count, false)
            }
            RecordedChange::Join { count } => self.execute_join(count, false),
            RecordedChange::OpenLine { direction } => self.execute_open_line(direction, false),
            RecordedChange::Paste { after, count } => self.execute_paste(after, count, false),
            RecordedChange::LineOp { op, count } => self.execute_line_op(op, count, false),
            RecordedChange::OperatorMotion { op, motion, count } => {
                self.execute_operator_motion(op, motion, count, false)
            }
            RecordedChange::OperatorFind {
                op,
                kind,
                target,
                count,
            } => self.execute_operator_find(op, kind, target, count, false),
            RecordedChange::OperatorTextObject {
                op,
                scope,
                kind,
                count,
            } => self.execute_operator_text_object(op, scope, kind, count, false),
        }
    }
}
