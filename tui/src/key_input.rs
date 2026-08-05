use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use orbcode_app_server_client::AppClient;
use orbcode_config::KeybindingContext;
use orbcode_protocol::StreamEvent;
use tokio::sync::mpsc;

use crate::bottom_pane::input_layout::prompt_input_submission_line;
use crate::bottom_pane::mode::EscapeAction;
use crate::clipboard::is_transcript_copy_shortcut;
use crate::commands::async_local::LocalCommandEnvelope;
use crate::commands::dispatch::SlashCommandOutcome;
use crate::editor_mode::EditorMode;
use crate::keybindings::{KeybindingAction, action_for, chord_from_key_event};
use crate::overlays::OverlayState;
use crate::slash_commands::record_slash_command_use;
use crate::slash_commands::{canonicalize_slash_command_line, exact_slash_command};
use crate::state::TuiState;

fn is_queue_followup_key(key_event: &KeyEvent) -> bool {
    key_event.code == KeyCode::Tab && key_event.modifiers.is_empty()
}

fn is_edit_last_followup_key(key_event: &KeyEvent) -> bool {
    key_event.code == KeyCode::Left && key_event.modifiers.contains(KeyModifiers::SHIFT)
}

fn is_active_turn_immediate_slash_command(line: &str) -> bool {
    let Some(command) = line
        .strip_prefix('/')
        .and_then(|rest| rest.split_whitespace().next())
    else {
        return false;
    };
    matches!(command, "jobs" | "background")
}

fn overlay_defers_ctrl_c_to_global_interrupt(overlay: &OverlayState) -> bool {
    matches!(
        overlay,
        OverlayState::Diff(_) | OverlayState::SandboxPicker(_)
    )
}

impl TuiState {
    fn clear_followup_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
        self.input_tail_pinned = false;
        self.prompt_history_index = None;
        self.slash_command_selected = 0;
    }

    pub(crate) async fn handle_key(
        &mut self,
        app_server: &AppClient,
        key_event: KeyEvent,
        turn_events: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
        local_command_tx: &mpsc::UnboundedSender<LocalCommandEnvelope>,
    ) -> Result<bool> {
        let saved_desired_column = self.desired_column.take();

        if matches!(self.overlay, Some(OverlayState::PermissionRequest(_)))
            && (self.has_transcript_selection() || self.has_permission_selection())
            && is_transcript_copy_shortcut(&key_event)
        {
            let result = self.copy_selected_screen_to_clipboard();
            self.report_transcript_copy_result(result);
            return Ok(true);
        }

        if matches!(self.overlay, Some(OverlayState::PermissionRequest(_)))
            && (self.has_transcript_selection() || self.has_permission_selection())
            && key_event.code == KeyCode::Esc
        {
            self.clear_screen_selection();
            self.set_status_line("Cleared selection.");
            return Ok(true);
        }

        if self.has_input_selection() && is_transcript_copy_shortcut(&key_event) {
            let result = self.copy_selected_input_to_clipboard();
            self.report_transcript_copy_result(result);
            return Ok(true);
        }

        if self.has_input_selection() && key_event.code == KeyCode::Esc {
            self.clear_input_selection();
            self.set_status_line("Cleared selection.");
            return Ok(true);
        }

        if self.has_input_selection() && key_event.code == KeyCode::Tab {
            self.indent_selected_lines();
            return Ok(true);
        }

        if self.has_input_selection() && key_event.code == KeyCode::BackTab {
            self.dedent_selected_lines();
            return Ok(true);
        }

        if self.has_transcript_selection() && is_transcript_copy_shortcut(&key_event) {
            let result = self.copy_selected_transcript_to_clipboard();
            self.report_transcript_copy_result(result);
            return Ok(true);
        }

        if self.has_permission_selection() && is_transcript_copy_shortcut(&key_event) {
            let result = self.copy_selected_permission_to_clipboard();
            self.report_transcript_copy_result(result);
            return Ok(true);
        }

        if self.has_transcript_selection() && key_event.code == KeyCode::Esc {
            self.clear_transcript_selection();
            self.set_status_line("Cleared transcript selection.");
            return Ok(true);
        }

        if self.has_permission_selection() && key_event.code == KeyCode::Esc {
            self.clear_screen_selection();
            self.set_status_line("Cleared selection.");
            return Ok(true);
        }

        if self.has_transcript_selection() {
            self.clear_transcript_selection();
        }

        if self.has_permission_selection() {
            self.clear_permission_selection();
        }

        if self.has_input_selection() {
            self.clear_input_selection();
        }

        if key_event.code == KeyCode::Char('c')
            && key_event.modifiers.contains(KeyModifiers::CONTROL)
            && turn_events.is_some()
            && self
                .overlay
                .as_ref()
                .is_some_and(overlay_defers_ctrl_c_to_global_interrupt)
        {
            self.interrupt_active_turn(app_server, turn_events).await;
            return Ok(true);
        }

        if self.overlay.is_some() {
            return self.handle_overlay_key(app_server, key_event).await;
        }

        if key_event.code == KeyCode::Char('c')
            && key_event.modifiers.contains(KeyModifiers::CONTROL)
        {
            if turn_events.is_some() {
                self.interrupt_active_turn(app_server, turn_events).await;
                return Ok(true);
            }
            return Ok(false);
        }

        if key_event.code == KeyCode::Esc {
            if self.handle_escape_key(turn_events.is_some()) == EscapeAction::CancelTurn {
                let immediate_followups = self.take_pending_followups_for_immediate_send();
                self.interrupt_active_turn(app_server, turn_events).await;
                if let Some(prompt) = immediate_followups {
                    *turn_events = Some(
                        app_server
                            .submit_turn_stream(&self.session_id, prompt)
                            .await?,
                    );
                    self.set_status_line("Sending pending follow-up after interrupt...");
                }
            }
            return Ok(true);
        }

        // Global-context keybindings fire in every editor mode (matching the
        // previous hard-coded ctrl+o / ctrl+t handling). The default keymap
        // reproduces those chords exactly, so behavior is unchanged unless the
        // user rebinds them in keybindings.json.
        if let Some(chord) = chord_from_key_event(&key_event) {
            match action_for(&[KeybindingContext::Global], chord) {
                Some(KeybindingAction::ToggleTranscript) => {
                    self.toggle_expanded_tool_details();
                    return Ok(true);
                }
                Some(KeybindingAction::ToggleTodos) => {
                    self.toggle_task_panel();
                    return Ok(true);
                }
                Some(KeybindingAction::ToggleBackgroundJobs) => {
                    if self.overlay.is_none() {
                        self.open_background_jobs_overlay(app_server).await;
                    }
                    return Ok(true);
                }
                _ => {}
            }
        }

        if self.editor_mode == EditorMode::Insert
            && key_event.modifiers.contains(KeyModifiers::ALT)
            && self.is_vim_alt_escape_key(&key_event.code)
            && !turn_events.is_some()
        {
            let _ = self.handle_escape_key(false);
            return self.handle_normal_mode_key(key_event);
        }

        if self.editor_mode == EditorMode::Normal && key_event.code != KeyCode::Enter {
            self.desired_column = saved_desired_column;
            return self.handle_normal_mode_key(key_event);
        }

        // Chat-context keybindings apply to the prompt editor in Standard /
        // Insert modes (Normal mode already routed to the vim handler above).
        // history:search lives in the Global context but is only actioned here
        // with the same turn-idle guard the previous ctrl+r handler used.
        if let Some(chord) = chord_from_key_event(&key_event) {
            match action_for(&[KeybindingContext::Chat, KeybindingContext::Global], chord) {
                Some(KeybindingAction::HistorySearch) if turn_events.is_none() => {
                    self.open_session_picker(app_server, "/sessions", "Project Sessions")
                        .await?;
                    return Ok(true);
                }
                Some(KeybindingAction::LineStart) => {
                    self.move_cursor_to_current_line_start();
                    return Ok(true);
                }
                Some(KeybindingAction::LineEnd) => {
                    self.move_cursor_to_current_line_end();
                    return Ok(true);
                }
                Some(KeybindingAction::ClearInput) => {
                    self.kill_to_line_start();
                    return Ok(true);
                }
                _ => {}
            }
        }

        // Tab queues the typed text as a follow-up during a streaming turn, but
        // only when it would not shadow a more specific Tab behavior: an open
        // completion popup (Tab completes), an empty input, or a caret sitting
        // in a leading-indent position (Tab indents). We also require a
        // submittable line so an empty buffer falls through.
        if is_queue_followup_key(&key_event)
            && turn_events.is_some()
            && !self.has_active_completion_popup()
            && !self.cursor_in_line_indent()
            && let Some(line) = prompt_input_submission_line(&self.input)
        {
            self.clear_followup_input();
            self.queue_followup(line);
            return Ok(true);
        }

        // Shift+Left recalls the last queued follow-up for editing, but only
        // when the input is empty — otherwise it would clobber typed text.
        // With text present (or no follow-up to recall) the key falls through
        // to normal cursor movement.
        if is_edit_last_followup_key(&key_event)
            && turn_events.is_some()
            && self.input.is_empty()
            && let Some(content) = self.pop_last_followup()
        {
            self.input = content;
            self.input_cursor = self.input.len();
            self.prompt_history_index = None;
            return Ok(true);
        }

        if key_event
            .modifiers
            .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
            && let KeyCode::Char('K' | 'k') = key_event.code
        {
            self.delete_current_line();
            self.prompt_history_index = None;
            return Ok(true);
        }

        if key_event.modifiers.contains(KeyModifiers::CONTROL) {
            match key_event.code {
                KeyCode::Char('k') => {
                    self.kill_to_line_end();
                    return Ok(true);
                }
                KeyCode::Char('w') => {
                    self.delete_prev_word();
                    self.prompt_history_index = None;
                    return Ok(true);
                }
                KeyCode::Char('b') => {
                    self.move_cursor_left();
                    return Ok(true);
                }
                KeyCode::Char('f') => {
                    self.move_cursor_right();
                    return Ok(true);
                }
                KeyCode::Left => {
                    self.move_cursor_word_left();
                    return Ok(true);
                }
                KeyCode::Right => {
                    self.move_cursor_word_right();
                    return Ok(true);
                }
                _ => {}
            }
        }

        match key_event.code {
            KeyCode::PageUp | KeyCode::PageDown => {}
            KeyCode::Home => self.move_cursor_to_current_line_start(),
            KeyCode::End => self.move_cursor_to_current_line_end(),
            KeyCode::Enter
                if key_event.modifiers.intersects(
                    KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL,
                ) =>
            {
                self.insert_text("\n");
                self.prompt_history_index = None;
            }
            KeyCode::Enter => {
                if turn_events.is_some() {
                    let Some(mut line) = prompt_input_submission_line(&self.input) else {
                        return Ok(true);
                    };
                    if line.starts_with('/') {
                        line = canonicalize_slash_command_line(&line);
                    }
                    if is_active_turn_immediate_slash_command(&line) {
                        self.clear_followup_input();
                        if let Some(name) = line
                            .strip_prefix('/')
                            .and_then(|rest| rest.split_whitespace().next())
                        {
                            record_slash_command_use(name);
                        }
                        match self
                            .handle_command(app_server, &line, local_command_tx)
                            .await
                        {
                            Ok(SlashCommandOutcome::Handled) => {}
                            Ok(SlashCommandOutcome::PromptToSubmit(prompt)) => {
                                self.queue_followup(prompt);
                                self.set_status_line("Command queued for next turn.");
                            }
                            Ok(SlashCommandOutcome::Exit) => return Ok(false),
                            Err(error) => {
                                self.set_status_line(format!("Command failed: {error}"));
                            }
                        }
                        return Ok(true);
                    }
                    self.clear_followup_input();
                    if let Err(error) = self.steer_followup(app_server, line.clone()).await {
                        self.queue_followup(line);
                        self.set_status_line(format!(
                            "Steer failed; queued for next turn: {error}"
                        ));
                    }
                    return Ok(true);
                }
                if turn_events.is_none() {
                    if self.complete_selected_add_dir_completion() {
                        return Ok(true);
                    }
                    let Some(mut line) = prompt_input_submission_line(&self.input) else {
                        return Ok(true);
                    };
                    if let Some(command) = exact_slash_command(&line) {
                        let requires_argument = command
                            .argument_hint
                            .is_some_and(|hint| !hint.starts_with('['));
                        let has_arguments = line
                            .strip_prefix('/')
                            .is_some_and(|body| body.trim().contains(char::is_whitespace));
                        if requires_argument && !has_arguments {
                            self.input = format!("/{} ", command.name);
                            self.input_cursor = self.input.len();
                            self.set_status_line(format!(
                                "/{} {}",
                                command.name,
                                command.argument_hint.unwrap_or_default()
                            ));
                            return Ok(true);
                        }
                    } else if self.complete_selected_slash_command() {
                        return Ok(true);
                    }
                    if line.starts_with('/') {
                        line = canonicalize_slash_command_line(&line);
                    }
                    self.input.clear();
                    self.input_cursor = 0;
                    self.input_tail_pinned = false;
                    self.prompt_history_index = None;
                    self.slash_command_selected = 0;
                    self.reset_pending_assistant_stream();
                    self.clear_live_tool_activities();
                    self.last_usage = None;

                    if line.starts_with('/') {
                        if let Some(name) = line
                            .strip_prefix('/')
                            .and_then(|rest| rest.split_whitespace().next())
                        {
                            record_slash_command_use(name);
                        }
                        match self
                            .handle_command(app_server, &line, local_command_tx)
                            .await
                        {
                            Ok(SlashCommandOutcome::Handled) => {
                                self.remember_prompt_history(&line);
                            }
                            Ok(SlashCommandOutcome::PromptToSubmit(prompt)) => {
                                self.remember_prompt_history(&line);
                                *turn_events = Some(
                                    app_server
                                        .submit_turn_stream(&self.session_id, prompt)
                                        .await?,
                                );
                            }
                            Ok(SlashCommandOutcome::Exit) => {
                                self.remember_prompt_history(&line);
                                return Ok(false);
                            }
                            Err(error) => {
                                self.set_status_line(format!("Command failed: {error}"));
                            }
                        }
                    } else {
                        self.remember_prompt_history(&line);
                        *turn_events = Some(
                            app_server
                                .submit_turn_stream(&self.session_id, line)
                                .await?,
                        );
                        self.set_status_line("Starting provider request...");
                    }
                }
            }
            KeyCode::Backspace => {
                self.delete_prev_char();
                self.prompt_history_index = None;
            }
            KeyCode::Delete => {
                self.delete_next_char();
                self.prompt_history_index = None;
            }
            KeyCode::Left => self.move_cursor_left(),
            KeyCode::Right => self.move_cursor_right(),
            KeyCode::Up => {
                if self.move_input_suggestion_selection(-1) {
                    return Ok(true);
                }
                self.desired_column = saved_desired_column;
                self.navigate_prompt_up();
            }
            KeyCode::Down => {
                if self.move_input_suggestion_selection(1) {
                    return Ok(true);
                }
                self.desired_column = saved_desired_column;
                self.navigate_prompt_down();
            }
            KeyCode::Tab if self.complete_selected_add_dir_completion() => {
                return Ok(true);
            }
            KeyCode::Tab if self.complete_selected_slash_argument_completion() => {
                return Ok(true);
            }
            KeyCode::Tab if self.complete_selected_slash_command() => {
                return Ok(true);
            }
            KeyCode::Char(character) if !key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_char(character);
                self.prompt_history_index = None;
            }
            KeyCode::Tab => self.insert_text("    "),
            _ => {}
        }

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_followup_key_accepts_tab() {
        assert!(is_queue_followup_key(&KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn queue_followup_key_rejects_shift_tab() {
        assert!(!is_queue_followup_key(&KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::SHIFT
        )));
    }

    #[test]
    fn edit_last_followup_key_accepts_shift_left() {
        assert!(is_edit_last_followup_key(&KeyEvent::new(
            KeyCode::Left,
            KeyModifiers::SHIFT
        )));
    }

    #[test]
    fn edit_last_followup_key_rejects_plain_left() {
        assert!(!is_edit_last_followup_key(&KeyEvent::new(
            KeyCode::Left,
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn active_turn_immediate_slash_command_matches_jobs_only() {
        assert!(is_active_turn_immediate_slash_command("/jobs"));
        assert!(is_active_turn_immediate_slash_command("/background"));
        assert!(is_active_turn_immediate_slash_command("/jobs running"));
        assert!(!is_active_turn_immediate_slash_command("jobs"));
        assert!(!is_active_turn_immediate_slash_command("/help"));
    }
}
