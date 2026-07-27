use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::bottom_pane::vim::{
    FindKind, IndentDirection, MotionKind, OpenLineDirection, OperatorKind, TextObjectScope,
    go_to_percent_of_buffer, operator_key, text_object_kind_from_key,
};
use crate::prompt_state::NormalPending;
use crate::state::TuiState;

impl TuiState {
    pub(crate) fn handle_normal_mode_key(&mut self, key_event: KeyEvent) -> Result<bool> {
        // Note: transcript-visual interception (vi-normal `v`, `Ctrl-B`,
        // `Ctrl-F`, plus `j`/`k`/Up/Down while a transcript selection is
        // active) happens earlier in `handle_key` so it can run before
        // the global selection-clearing fall-through. By the time we
        // reach this function those keys are already consumed.

        let saved_desired_column = self.desired_column.take();

        if let Some(pending) = self.normal_pending.take() {
            match pending {
                NormalPending::Find(kind) => {
                    if let KeyCode::Char(target) = key_event.code {
                        let count = self.take_normal_count();
                        self.apply_find(kind, target, count);
                    }
                    self.normal_count = None;
                    return Ok(true);
                }
                NormalPending::Go => {
                    let count = self.take_normal_count().unwrap_or(1);
                    match key_event.code {
                        KeyCode::Char('g') => {
                            self.go_to_line(count);
                            return Ok(true);
                        }
                        KeyCode::Char('j') => {
                            self.repeat_visual_vertical_motion(count, 1);
                            return Ok(true);
                        }
                        KeyCode::Char('k') => {
                            self.repeat_visual_vertical_motion(count, -1);
                            return Ok(true);
                        }
                        KeyCode::Char('e') => {
                            self.repeat_motion(count, Self::move_cursor_end_prev_vim_word);
                            return Ok(true);
                        }
                        KeyCode::Char('E') => {
                            self.repeat_motion(count, Self::move_cursor_end_prev_big_word);
                            return Ok(true);
                        }
                        _ => {}
                    }
                    self.normal_count = None;
                }
                NormalPending::Operator {
                    op,
                    count: op_count,
                } => {
                    if let KeyCode::Char(digit) = key_event.code
                        && digit.is_ascii_digit()
                        && (digit != '0' || self.normal_count.is_some())
                    {
                        self.push_normal_count_digit(digit);
                        self.normal_pending = Some(NormalPending::Operator {
                            op,
                            count: op_count,
                        });
                        return Ok(true);
                    }

                    let motion_count = self.take_normal_count().unwrap_or(1);
                    let count = op_count.saturating_mul(motion_count);
                    match key_event.code {
                        KeyCode::Char(character)
                            if operator_key(op).is_some_and(|expected| expected == character) =>
                        {
                            self.execute_line_op(op, count, true);
                        }
                        KeyCode::Char('i') => {
                            self.normal_pending = Some(NormalPending::OperatorTextObject {
                                op,
                                count,
                                scope: TextObjectScope::Inner,
                            });
                        }
                        KeyCode::Char('a') => {
                            self.normal_pending = Some(NormalPending::OperatorTextObject {
                                op,
                                count,
                                scope: TextObjectScope::Around,
                            });
                        }
                        KeyCode::Char('f') => {
                            self.normal_pending = Some(NormalPending::OperatorFind {
                                op,
                                count,
                                kind: FindKind::Forward,
                            });
                        }
                        KeyCode::Char('F') => {
                            self.normal_pending = Some(NormalPending::OperatorFind {
                                op,
                                count,
                                kind: FindKind::Backward,
                            });
                        }
                        KeyCode::Char('t') => {
                            self.normal_pending = Some(NormalPending::OperatorFind {
                                op,
                                count,
                                kind: FindKind::TillForward,
                            });
                        }
                        KeyCode::Char('T') => {
                            self.normal_pending = Some(NormalPending::OperatorFind {
                                op,
                                count,
                                kind: FindKind::TillBackward,
                            });
                        }
                        KeyCode::Char('g') => {
                            self.normal_pending = Some(NormalPending::OperatorGo { op, count });
                        }
                        KeyCode::Char('G') => self.execute_operator_g(op, count, true),
                        KeyCode::Char('h') | KeyCode::Left => {
                            self.execute_operator_motion(op, MotionKind::Left, count, true)
                        }
                        KeyCode::Backspace => {
                            self.execute_operator_motion(op, MotionKind::Left, count, true)
                        }
                        KeyCode::Char('l') | KeyCode::Right => {
                            self.execute_operator_motion(op, MotionKind::Right, count, true)
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            self.execute_operator_motion(op, MotionKind::Down, count, true)
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            self.execute_operator_motion(op, MotionKind::Up, count, true)
                        }
                        KeyCode::Char('w') => {
                            self.execute_operator_motion(op, MotionKind::Word, count, true)
                        }
                        KeyCode::Char('b') => {
                            self.execute_operator_motion(op, MotionKind::BackWord, count, true)
                        }
                        KeyCode::Char('e') => {
                            self.execute_operator_motion(op, MotionKind::EndWord, count, true)
                        }
                        KeyCode::Char('W') => {
                            self.execute_operator_motion(op, MotionKind::BigWord, count, true)
                        }
                        KeyCode::Char('B') => {
                            self.execute_operator_motion(op, MotionKind::BackBigWord, count, true)
                        }
                        KeyCode::Char('E') => {
                            self.execute_operator_motion(op, MotionKind::EndBigWord, count, true)
                        }
                        KeyCode::Char('0') | KeyCode::Home => {
                            self.execute_operator_motion(op, MotionKind::LineStart, count, true)
                        }
                        KeyCode::Char('^') => {
                            self.execute_operator_motion(op, MotionKind::FirstNonBlank, count, true)
                        }
                        KeyCode::Char('$') | KeyCode::End => {
                            self.execute_operator_motion(op, MotionKind::LineEnd, count, true)
                        }
                        KeyCode::Char('%') => {
                            self.execute_operator_motion(op, MotionKind::MatchPair, 1, true)
                        }
                        _ => {}
                    }
                    self.normal_count = None;
                    return Ok(true);
                }
                NormalPending::OperatorFind { op, count, kind } => {
                    if let KeyCode::Char(target) = key_event.code {
                        self.execute_operator_find(op, kind, target, count, true);
                    }
                    self.normal_count = None;
                    return Ok(true);
                }
                NormalPending::OperatorTextObject { op, count, scope } => {
                    if let Some(kind) = text_object_kind_from_key(key_event.code) {
                        self.execute_operator_text_object(op, scope, kind, count, true);
                    }
                    self.normal_count = None;
                    return Ok(true);
                }
                NormalPending::OperatorGo { op, count } => {
                    match key_event.code {
                        KeyCode::Char('g') => self.execute_operator_gg(op, count, true),
                        KeyCode::Char('j') => {
                            self.execute_operator_motion(op, MotionKind::VisualDown, count, true)
                        }
                        KeyCode::Char('k') => {
                            self.execute_operator_motion(op, MotionKind::VisualUp, count, true)
                        }
                        KeyCode::Char('e') => {
                            self.execute_operator_motion(op, MotionKind::PrevWordEnd, count, true)
                        }
                        KeyCode::Char('E') => self.execute_operator_motion(
                            op,
                            MotionKind::PrevBigWordEnd,
                            count,
                            true,
                        ),
                        _ => {}
                    }
                    self.normal_count = None;
                    return Ok(true);
                }
                NormalPending::Replace { count } => {
                    if let KeyCode::Char(character) = key_event.code {
                        self.execute_replace(character, count, true);
                    }
                    self.normal_count = None;
                    return Ok(true);
                }
                NormalPending::Indent { direction, count } => {
                    match key_event.code {
                        KeyCode::Char('>') if direction == IndentDirection::Right => {
                            self.execute_indent(direction, count, true);
                        }
                        KeyCode::Char('<') if direction == IndentDirection::Left => {
                            self.execute_indent(direction, count, true);
                        }
                        _ => {}
                    }
                    self.normal_count = None;
                    return Ok(true);
                }
            }
        }

        if let KeyCode::Char(digit) = key_event.code
            && digit.is_ascii_digit()
            && (digit != '0' || self.normal_count.is_some())
        {
            self.push_normal_count_digit(digit);
            return Ok(true);
        }

        let count_override = self.take_normal_count();
        let count = count_override.unwrap_or(1);

        match key_event.code {
            KeyCode::Char('d') => {
                self.normal_pending = Some(NormalPending::Operator {
                    op: OperatorKind::Delete,
                    count,
                });
            }
            KeyCode::Char('c') => {
                self.normal_pending = Some(NormalPending::Operator {
                    op: OperatorKind::Change,
                    count,
                });
            }
            KeyCode::Char('y') => {
                self.normal_pending = Some(NormalPending::Operator {
                    op: OperatorKind::Yank,
                    count,
                });
            }
            KeyCode::Char('r') => {
                self.normal_pending = Some(NormalPending::Replace { count });
            }
            KeyCode::Char('>') => {
                self.normal_pending = Some(NormalPending::Indent {
                    direction: IndentDirection::Right,
                    count,
                });
            }
            KeyCode::Char('<') => {
                self.normal_pending = Some(NormalPending::Indent {
                    direction: IndentDirection::Left,
                    count,
                });
            }
            KeyCode::Char('u') => self.undo_last_change(),
            KeyCode::Char('.') => self.repeat_last_change(),
            KeyCode::Char('i') => {
                self.vim_state.pending_insert_change = None;
                self.enter_insert_mode();
            }
            KeyCode::Char('a') => {
                self.move_cursor_right();
                self.vim_state.pending_insert_change = None;
                self.enter_insert_mode();
            }
            KeyCode::Char('I') => {
                self.move_cursor_to_first_non_blank();
                self.vim_state.pending_insert_change = None;
                self.enter_insert_mode();
            }
            KeyCode::Char('A') => {
                self.move_cursor_to_line_end_insert();
                self.vim_state.pending_insert_change = None;
                self.enter_insert_mode();
            }
            KeyCode::Char('o') => self.execute_open_line(OpenLineDirection::Below, true),
            KeyCode::Char('O') => self.execute_open_line(OpenLineDirection::Above, true),
            KeyCode::Char('s') => {
                self.execute_operator_motion(OperatorKind::Change, MotionKind::Right, count, true)
            }
            KeyCode::Char('S') => self.execute_line_op(OperatorKind::Change, count, true),
            KeyCode::Backspace => self.repeat_motion(count, Self::move_cursor_left),
            KeyCode::Char('h') | KeyCode::Left => self.repeat_motion(count, Self::move_cursor_left),
            KeyCode::Char('l') | KeyCode::Right => {
                self.repeat_motion(count, Self::move_cursor_right)
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.desired_column = saved_desired_column;
                self.repeat_logical_vertical_or_history(count, 1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.desired_column = saved_desired_column;
                self.repeat_logical_vertical_or_history(count, -1);
            }
            KeyCode::Char('w') => self.repeat_motion(count, Self::move_cursor_next_vim_word),
            KeyCode::Char('b') => self.repeat_motion(count, Self::move_cursor_prev_vim_word),
            KeyCode::Char('e') => self.repeat_motion(count, Self::move_cursor_end_vim_word),
            KeyCode::Char('W') => self.repeat_motion(count, Self::move_cursor_next_big_word),
            KeyCode::Char('B') => self.repeat_motion(count, Self::move_cursor_prev_big_word),
            KeyCode::Char('E') => self.repeat_motion(count, Self::move_cursor_end_big_word),
            KeyCode::Char('~') => self.execute_toggle_case(count, true),
            KeyCode::Char('f') => {
                self.normal_pending = Some(NormalPending::Find(FindKind::Forward))
            }
            KeyCode::Char('F') => {
                self.normal_pending = Some(NormalPending::Find(FindKind::Backward))
            }
            KeyCode::Char('t') => {
                self.normal_pending = Some(NormalPending::Find(FindKind::TillForward))
            }
            KeyCode::Char('T') => {
                self.normal_pending = Some(NormalPending::Find(FindKind::TillBackward))
            }
            KeyCode::Char('g') => {
                // The dispatch above already consumed `normal_count` into
                // `count_override`; restore it so a count like `5gg` survives to
                // the pending `Go` resolution instead of defaulting to line 1.
                self.normal_count = count_override;
                self.normal_pending = Some(NormalPending::Go);
            }
            KeyCode::Char('G') => {
                if let Some(line) = count_override {
                    self.go_to_line(line);
                } else {
                    self.go_to_last_line();
                }
            }
            KeyCode::Char(';') => self.repeat_last_find(false, count),
            KeyCode::Char(',') => self.repeat_last_find(true, count),
            KeyCode::Char('%') => {
                if let Some(percent) = count_override {
                    self.input_cursor = go_to_percent_of_buffer(&self.input, percent);
                } else {
                    self.execute_match_pair();
                }
            }
            KeyCode::Char('0') | KeyCode::Home => self.move_cursor_to_line_start(),
            KeyCode::Char('^') => self.move_cursor_to_first_non_blank(),
            KeyCode::Char('$') | KeyCode::End => self.move_cursor_to_line_end(),
            KeyCode::Char('x') | KeyCode::Delete => self.execute_x(count, true),
            KeyCode::Char('D') => {
                self.execute_operator_motion(OperatorKind::Delete, MotionKind::LineEnd, 1, true)
            }
            KeyCode::Char('C') => {
                self.execute_operator_motion(OperatorKind::Change, MotionKind::LineEnd, 1, true)
            }
            KeyCode::Char('Y') => self.execute_line_op(OperatorKind::Yank, count, true),
            KeyCode::Char('J') => self.execute_join(count, true),
            KeyCode::Char('p') => self.execute_paste(true, count, true),
            KeyCode::Char('P') => self.execute_paste(false, count, true),
            _ => {}
        }

        Ok(true)
    }
}
