use crossterm::event::KeyCode;

use crate::bottom_pane::vim::RecordedChange;
use crate::editor_mode::EditorMode;
use crate::state::TuiState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EscapeAction {
    StayInTui,
    CancelTurn,
}

impl TuiState {
    pub(crate) fn handle_escape_key(&mut self, turn_active: bool) -> EscapeAction {
        if self.request_in_flight || turn_active {
            EscapeAction::CancelTurn
        } else if self.editor_mode == EditorMode::Standard {
            EscapeAction::StayInTui
        } else {
            if self.editor_mode == EditorMode::Insert {
                match (
                    self.vim_state.pending_insert_change.take(),
                    self.vim_state.inserted_text.is_empty(),
                ) {
                    (Some(base), false) => {
                        self.vim_state.last_change = Some(RecordedChange::CompositeInsert {
                            base: Box::new(base),
                            text: self.vim_state.inserted_text.clone(),
                        });
                    }
                    (Some(base), true) => {
                        self.vim_state.last_change = Some(base);
                    }
                    (None, false) => {
                        self.vim_state.last_change =
                            Some(RecordedChange::Insert(self.vim_state.inserted_text.clone()));
                    }
                    (None, true) => {}
                }
                self.move_cursor_left_from_insert_exit();
            } else {
                self.normal_pending = None;
                self.normal_count = None;
            }
            self.vim_state.inserted_text.clear();
            self.enter_normal_mode();
            EscapeAction::StayInTui
        }
    }

    pub(crate) fn enter_insert_mode(&mut self) {
        self.editor_mode = EditorMode::Insert;
        self.normal_pending = None;
        self.normal_count = None;
        self.vim_state.inserted_text.clear();
    }

    pub(crate) fn enter_normal_mode(&mut self) {
        self.editor_mode = EditorMode::Normal;
    }

    pub(crate) fn is_vim_alt_escape_key(&self, code: &KeyCode) -> bool {
        matches!(
            code,
            KeyCode::Char(
                'h' | 'j'
                    | 'k'
                    | 'l'
                    | 'i'
                    | 'a'
                    | 'I'
                    | 'A'
                    | 'w'
                    | 'e'
                    | 'W'
                    | 'B'
                    | 'E'
                    | 'd'
                    | 'c'
                    | 'y'
                    | 'r'
                    | 'u'
                    | '.'
                    | 'o'
                    | 'O'
                    | 'p'
                    | 'P'
                    | 'J'
                    | 'D'
                    | 'C'
                    | 'Y'
                    | 's'
                    | 'S'
                    | '~'
                    | '>'
                    | '<'
                    | 'x'
                    | 'b'
                    | 'f'
                    | 'F'
                    | 't'
                    | 'T'
                    | 'g'
                    | 'G'
                    | ';'
                    | ','
                    | '%'
                    | '^'
                    | '1'
                    | '2'
                    | '3'
                    | '4'
                    | '5'
                    | '6'
                    | '7'
                    | '8'
                    | '9'
                    | '0'
                    | '$'
            ) | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Delete
        )
    }
}
