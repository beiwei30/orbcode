use std::fmt::Write as _;

use anyhow::Result;
use orbcode_app_server_client::AppClient;
use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};
use tokio::sync::mpsc;

use crate::clipboard::copy_text_to_clipboard;
use crate::commands::async_local::{LocalCommandEnvelope, LocalCommandEvent};
use crate::commands::auth::{run_login_slash_command, run_logout_slash_command};
use crate::commands::compact::compact_restored_file_detail_lines;
use crate::commands::effort::run_effort_slash_command;
use crate::commands::plan::run_plan_slash_command;
use crate::commands::release_notes::run_release_notes_slash_command;
use crate::commands::utils::{
    clean_sandbox_exclude_pattern, parse_model_argument, short_session_id,
    slash_command_display_path, split_first_word,
};
use crate::editor_mode::editor_mode_next_setting;
use crate::external_editor::{ExternalEditorRequest, ExternalEditorTarget};
use crate::overlays::{AddDirPickerState, HelpOverlayState, OverlayState};
use crate::slash_commands::{AsyncLocalSlashCommand, TuiLocalSlashCommand};
use crate::state::TuiState;

impl TuiState {
    pub(crate) async fn run_tui_local_slash_command(
        &mut self,
        command: TuiLocalSlashCommand,
        args: &str,
        line: &str,
        app_server: &AppClient,
        local_command_tx: &mpsc::UnboundedSender<LocalCommandEnvelope>,
    ) -> Result<()> {
        match command {
            TuiLocalSlashCommand::Branch => {
                unreachable!("handled by extracted BranchCommand")
            }
            TuiLocalSlashCommand::Goal => {
                unreachable!("handled by extracted GoalCommand")
            }
            TuiLocalSlashCommand::AddDir => {
                if args.is_empty() {
                    self.overlay = Some(OverlayState::AddDirPicker(AddDirPickerState::new(
                        line, &self.cwd,
                    )));
                    self.set_status_line("Directory picker: Enter to add, Esc to cancel.");
                } else {
                    let candidate = app_server
                        .validate_add_directory(args)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    let result = app_server
                        .add_directory(&self.session_id, &candidate.path.display().to_string())
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    let message = format!(
                        "Added {} as a working directory for this session.",
                        result.path.display()
                    );
                    self.push_local_slash_command_output(
                        line,
                        message.clone(),
                        Some("Use /permissions to inspect.".to_string()),
                    );
                    self.set_status_line(message);
                }
            }
            TuiLocalSlashCommand::Help => {
                if !args.is_empty() {
                    return Err(anyhow::anyhow!("unknown slash command"));
                }
                self.overlay = Some(OverlayState::Help(HelpOverlayState::default()));
                self.push_local_slash_command_output(line, "Opened help.", None);
                self.set_status_line("Help: ↑↓ scroll, Esc close.");
            }
            TuiLocalSlashCommand::Clear => {
                if !args.is_empty() {
                    return Err(anyhow::anyhow!("unknown slash command"));
                }
                let previous_session_id = self.session_id.clone();
                let previous_usage = orbcode_protocol::get_current_usage(&self.messages);
                let bootstrap = app_server
                    .clear_session(&previous_session_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                *self = Self::new(self.client.clone(), bootstrap);
                self.refresh_permission_mode(app_server).await;
                self.clear_session_info = Some(crate::state::ClearSessionInfo {
                    session_id: previous_session_id,
                    usage: previous_usage,
                });
                self.transcript_ui.emission.needs_scrollback_clear = true;
                self.pending_history_flush = true;
                self.set_status_line("Conversation cleared.");
            }
            TuiLocalSlashCommand::Compact => {
                let force = match args {
                    "" => false,
                    "force" => true,
                    _ => return Err(anyhow::anyhow!("unknown slash command")),
                };
                self.start_compact_slash_command(line, force, app_server, local_command_tx);
            }
            TuiLocalSlashCommand::Config => match args {
                "" => {
                    self.open_config_picker(line, app_server).await?;
                }
                "model" => {
                    self.open_model_picker(line, app_server).await?;
                }
                "theme" => {
                    self.open_theme_picker(line, app_server).await?;
                }
                _ if split_first_word(args).is_some_and(|(name, _)| name == "effort") => {
                    let (_, effort_args) = split_first_word(args).expect("effort args");
                    let message =
                        run_effort_slash_command(app_server, &self.session_id, effort_args).await?;
                    self.refresh_status_effort(app_server).await;
                    self.push_local_slash_command_output(line, message.clone(), None);
                    self.set_status_line(message);
                }
                "editor-mode" | "editorMode" => {
                    let message = self
                        .set_editor_mode_setting(
                            app_server,
                            editor_mode_next_setting(self.editor_mode),
                        )
                        .await?;
                    self.push_local_slash_command_output(line, message.clone(), None);
                    self.set_status_line(message);
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "usage: /config [model|theme|effort <level>|editor-mode]"
                    ));
                }
            },
            TuiLocalSlashCommand::Copy => {
                if !args.is_empty() {
                    return Err(anyhow::anyhow!("unknown slash command"));
                }
                let text = latest_assistant_text(&self.messages).ok_or_else(|| {
                    anyhow::anyhow!("No assistant response available to copy yet.")
                })?;
                let char_count = text.chars().count();
                copy_text_to_clipboard(&text)?;
                let message = format!("Copied last assistant response ({char_count} chars).");
                self.push_local_slash_command_output(line, message.clone(), None);
                self.set_status_line(message);
            }
            TuiLocalSlashCommand::Effort => {
                let message = run_effort_slash_command(app_server, &self.session_id, args).await?;
                self.refresh_status_effort(app_server).await;
                self.push_local_slash_command_output(line, message.clone(), None);
                self.set_status_line(message);
            }
            TuiLocalSlashCommand::Files => {
                if !args.is_empty() {
                    return Err(anyhow::anyhow!("unknown slash command"));
                }
                let permissions = app_server
                    .permission_overview()
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let session_summary = app_server.list_sessions().await.ok().and_then(|summaries| {
                    summaries
                        .into_iter()
                        .find(|summary| summary.session_id == self.session_id)
                });
                let file_lines = compact_restored_file_detail_lines(&self.messages, &self.cwd);
                let file_count = file_lines.len();
                let mut detail = String::new();
                write!(detail, "cwd: {}", self.cwd.display())
                    .expect("writing to String cannot fail");
                if let Some(summary) = session_summary.as_ref() {
                    if let Some(branch) = summary.git_branch.as_deref() {
                        write!(detail, "\ngit branch: {branch}")
                            .expect("writing to String cannot fail");
                    }
                    if let Some(model) = summary.model.as_deref() {
                        write!(detail, "\nmodel: {model}").expect("writing to String cannot fail");
                    }
                }
                for directory in &permissions.configured_additional_directories {
                    write!(detail, "\nconfigured dir: {}", directory.display())
                        .expect("writing to String cannot fail");
                }
                for directory in &permissions.session_additional_directories {
                    write!(detail, "\nsession dir: {}", directory.display())
                        .expect("writing to String cannot fail");
                }
                if file_lines.is_empty() {
                    detail.push_str("\nNo files referenced in this session yet.");
                } else {
                    detail.push_str("\nRecent files:");
                    for line in &file_lines {
                        detail.push_str("\n  ");
                        detail.push_str(line);
                    }
                }
                let summary = if file_count == 0 {
                    "No files tracked.".to_string()
                } else {
                    format!("{file_count} recent file(s) tracked.")
                };
                self.push_local_slash_command_output(line, summary.clone(), Some(detail));
                self.set_status_line(summary);
            }
            TuiLocalSlashCommand::Fork => {
                let fork_result = app_server
                    .fork_session(
                        &self.session_id,
                        (!args.is_empty()).then(|| args.to_string()),
                        None,
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let fork_session_id = &fork_result.session_id;
                let bootstrap = app_server
                    .bootstrap(Some(fork_session_id))
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                *self = Self::new(self.client.clone(), bootstrap);
                self.refresh_permission_mode(app_server).await;
                let status = format!(
                    "Forked into session {}.",
                    short_session_id(&self.session_id)
                );
                self.push_local_slash_command_output(line, status.clone(), None);
                self.set_status_line(status);
            }
            TuiLocalSlashCommand::Keybindings => {
                if !args.is_empty() {
                    return Err(anyhow::anyhow!("unknown slash command"));
                }
                let result = app_server
                    .ensure_keybindings_file()
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let file_path = result.path;
                let file_created = result.created;
                self.external_editor_request = Some(ExternalEditorRequest {
                    command: line.to_string(),
                    path: file_path.clone(),
                    target: ExternalEditorTarget::Keybindings {
                        created: file_created,
                    },
                });
                self.set_status_line(format!(
                    "Opening keybindings file {}...",
                    file_path.display()
                ));
            }
            TuiLocalSlashCommand::Login => {
                let (summary, detail) =
                    run_login_slash_command(app_server, self.app_client_ref(), args).await?;
                self.push_local_slash_command_output(line, summary.clone(), detail);
                self.set_status_line(summary);
            }
            TuiLocalSlashCommand::Logout => {
                let (summary, detail) =
                    run_logout_slash_command(app_server, self.app_client_ref(), args).await?;
                self.push_local_slash_command_output(line, summary.clone(), detail);
                self.set_status_line(summary);
            }
            TuiLocalSlashCommand::Model => {
                if args.is_empty() {
                    self.open_model_picker(line, app_server).await?;
                    return Ok(());
                }
                self.finish_model_selection(app_server, line, parse_model_argument(args), None)
                    .await?;
            }
            TuiLocalSlashCommand::OutputStyle => {
                let requested = args.trim();
                if requested.is_empty() {
                    self.open_output_style_picker(line, app_server).await?;
                    return Ok(());
                }
                self.finish_output_style_selection(app_server, line, requested.to_string())
                    .await?;
            }
            TuiLocalSlashCommand::Permissions => {
                if !args.is_empty() {
                    return Err(anyhow::anyhow!(
                        "`/permissions` no longer accepts arguments. Choose a preset with `/permissions`; for advanced allow/ask/deny rules, edit the `permissions` key in the applicable settings.json."
                    ));
                }
                self.start_async_local_slash_command(
                    AsyncLocalSlashCommand::Permissions,
                    app_server,
                    local_command_tx,
                );
            }
            TuiLocalSlashCommand::Plan => {
                let client = self.app_client();
                let args = args.to_string();
                let session_id = self.session_id.clone();
                let local_command_tx = local_command_tx.clone();
                self.set_status_line("Loading plan...");
                // Detached plan command; completion is routed back through
                // LocalCommandEvent.
                let _plan_command_handle = tokio::spawn(async move {
                    let result = run_plan_slash_command(&client, args)
                        .await
                        .map_err(|error| error.to_string());
                    let _ = local_command_tx.send(LocalCommandEnvelope::new(
                        session_id,
                        LocalCommandEvent::PlanFinished(result),
                    ));
                });
            }
            TuiLocalSlashCommand::ReleaseNotes => {
                if !args.is_empty() {
                    return Err(anyhow::anyhow!("unknown slash command"));
                }
                let (summary, detail) =
                    run_release_notes_slash_command(&self.ui_version, app_server, &self.session_id)
                        .await?;
                self.push_local_slash_command_output(line, summary.clone(), detail);
                self.set_status_line(summary);
            }
            TuiLocalSlashCommand::Rename => {
                let title = args.trim();
                if title.is_empty() {
                    return Err(anyhow::anyhow!("usage: /rename <title>"));
                }
                app_server
                    .rename_session(&self.session_id, title)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let message = format!("Renamed session to \"{title}\".");
                self.push_local_slash_command_output(line, message.clone(), None);
                self.set_status_line(message);
            }
            TuiLocalSlashCommand::Resume => {
                if args.is_empty() {
                    self.open_session_picker(app_server, line, "Resume Session")
                        .await?;
                    return Ok(());
                }
                let bootstrap = match app_server.bootstrap(Some(args)).await {
                    Ok(bootstrap) => bootstrap,
                    Err(_) => {
                        let Some(session_id) = app_server
                            .session_id_for_exact_custom_title(args)
                            .await
                            .map_err(|e| anyhow::anyhow!("{e}"))?
                        else {
                            return Err(anyhow::anyhow!("session not found: {args}"));
                        };
                        app_server
                            .bootstrap(Some(&session_id))
                            .await
                            .map_err(|e| anyhow::anyhow!("{e}"))?
                    }
                };
                *self = Self::new(self.client.clone(), bootstrap);
                self.refresh_permission_mode(app_server).await;
                let status = format!("Session {} resumed.", short_session_id(&self.session_id));
                self.push_local_slash_command_output(line, status.clone(), None);
                self.set_status_line(status);
            }
            TuiLocalSlashCommand::Rewind => {
                if !args.is_empty() {
                    return Err(anyhow::anyhow!("unknown slash command"));
                }
                self.open_rewind_picker(line);
            }
            TuiLocalSlashCommand::Sandbox => {
                if args.is_empty() {
                    self.open_sandbox_picker(line, app_server).await?;
                    return Ok(());
                }
                let (subcommand, rest) = split_first_word(args).ok_or_else(|| {
                    anyhow::anyhow!("usage: /sandbox exclude \"command pattern\"")
                })?;
                if subcommand != "exclude" {
                    return Err(anyhow::anyhow!(
                        "unknown subcommand \"{subcommand}\". Available subcommand: exclude"
                    ));
                }
                let pattern = clean_sandbox_exclude_pattern(rest);
                if pattern.is_empty() {
                    return Err(anyhow::anyhow!(
                        "please provide a command pattern to exclude (e.g. /sandbox exclude \"npm run test:*\")"
                    ));
                }
                let excluded = app_server
                    .add_sandbox_excluded_command(&pattern)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let display_path = slash_command_display_path(&excluded.path, &self.cwd);
                let message = format!(
                    "Added \"{}\" to excluded commands in {display_path}",
                    excluded.pattern
                );
                self.push_local_slash_command_output(line, message.clone(), None);
                self.set_status_line(message);
            }
            TuiLocalSlashCommand::Sessions => {
                if !args.is_empty() {
                    return Err(anyhow::anyhow!("unknown slash command"));
                }
                self.open_session_picker(app_server, line, "Project Sessions")
                    .await?;
            }
            TuiLocalSlashCommand::Theme => {
                if !args.is_empty() {
                    return Err(anyhow::anyhow!("unknown slash command"));
                }
                self.open_theme_picker(line, app_server).await?;
            }
            TuiLocalSlashCommand::Vim => {
                if !args.is_empty() {
                    return Err(anyhow::anyhow!("unknown slash command"));
                }
                let message = self
                    .set_editor_mode_setting(app_server, editor_mode_next_setting(self.editor_mode))
                    .await?;
                self.push_local_slash_command_output(line, message.clone(), None);
                self.set_status_line(message);
            }
        }
        Ok(())
    }
}

pub(crate) fn latest_assistant_text(messages: &[TranscriptMessage]) -> Option<String> {
    for message in messages.iter().rev() {
        if !matches!(message.role, MessageRole::Assistant) {
            continue;
        }
        let mut buffer = String::new();
        for block in &message.blocks {
            if let TranscriptBlock::Text { text } = block {
                if !buffer.is_empty() {
                    buffer.push_str("\n\n");
                }
                buffer.push_str(text);
            }
        }
        let trimmed = buffer.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
        let visible = message.content.trim();
        if !visible.is_empty() {
            return Some(visible.to_string());
        }
    }
    None
}
