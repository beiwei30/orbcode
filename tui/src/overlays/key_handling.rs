use anyhow::Result;
use crossterm::event::KeyEvent;
use orbcode_app_server_client::{AppClient, PermissionDecision};

use crate::clipboard::copy_text_to_clipboard;
use crate::commands::permissions::PermissionRuleAction;
use crate::commands::utils::short_session_id;
use crate::external_editor::{ExternalEditorRequest, ExternalEditorTarget, open_path_in_system};
use crate::state::TuiState;

use super::*;

impl TuiState {
    pub(crate) fn should_refresh_background_jobs_overlay(&self) -> bool {
        matches!(
            self.overlay,
            Some(OverlayState::BackgroundJobs(BackgroundJobsOverlayState {
                needs_refresh: true,
                ..
            }))
        )
    }

    pub(crate) async fn open_background_jobs_overlay(&mut self, app_server: &AppClient) {
        match app_server.list_background_jobs_summary().await {
            Ok(jobs) => {
                if jobs.is_empty() {
                    self.set_status_line("No background jobs.");
                    return;
                }
                self.overlay = Some(OverlayState::BackgroundJobs(
                    BackgroundJobsOverlayState::new(jobs.into_inner(), self.session_id.clone()),
                ));
                self.set_status_line(
                    "Background jobs: jk select, Enter detail, d cancel, q close.",
                );
            }
            Err(error) => {
                self.set_status_line(format!("Failed to load background jobs: {error}"));
            }
        }
    }

    pub(crate) async fn refresh_background_jobs_overlay(&mut self, app_server: &AppClient) {
        let Some(OverlayState::BackgroundJobs(state)) = self.overlay.as_mut() else {
            return;
        };
        state.needs_refresh = false;

        if state.view == BackgroundJobsView::Detail
            && let Some(job) = state.selected_job()
        {
            let job_id = job.task_id.clone();
            match app_server.background_job_detail(&job_id).await {
                Ok(detail) => {
                    state.set_detail(detail);
                }
                Err(error) => {
                    self.set_status_line(format!("Failed to load job detail: {error}"));
                }
            }
            return;
        }

        match app_server.list_background_jobs_summary().await {
            Ok(jobs) => {
                if let Some(OverlayState::BackgroundJobs(state)) = self.overlay.as_mut() {
                    state.update_jobs(jobs.into_inner());
                }
            }
            Err(error) => {
                self.set_status_line(format!("Failed to refresh background jobs: {error}"));
            }
        }
    }

    pub(crate) fn toggle_expanded_tool_details(&mut self) {
        if matches!(self.overlay, Some(OverlayState::TranscriptPager(_))) {
            self.overlay = None;
            self.set_status_line("Closed transcript pager.");
            return;
        }
        let area = self.transcript_ui.viewport.area;
        let width = area.width.max(1) as usize;
        let height = area.height.max(1) as usize;
        self.open_transcript_pager(width, height);
    }

    pub(crate) fn toggle_permission_request_details(&mut self) {
        let expanded = !self.expanded_tool_details;
        self.expanded_tool_details = expanded;
        if !expanded {
            self.clear_transcript_bottom_pin_sticky();
        }
        if let Some(OverlayState::PermissionRequest(permission)) = self.overlay.as_mut() {
            permission.details_expanded = expanded;
            permission.panel_scroll = if expanded { usize::MAX / 2 } else { 0 };
            permission.viewport.clear_selection();
        }
        self.set_status_line(if expanded {
            "Expanded permission and transcript details."
        } else {
            "Collapsed permission and transcript details."
        });
    }

    pub(crate) async fn handle_overlay_key(
        &mut self,
        app_server: &AppClient,
        key_event: KeyEvent,
    ) -> Result<bool> {
        let mut action = OverlayAction::None;

        if let Some(overlay) = self.overlay.as_mut() {
            match overlay {
                OverlayState::AddDirPicker(picker) => {
                    match apply_add_dir_picker_key(picker, &key_event) {
                        AddDirPickerKeyAction::None => {}
                        AddDirPickerKeyAction::Close => {
                            self.overlay = None;
                            self.set_status_line("Closed directory picker.");
                        }
                        AddDirPickerKeyAction::AddDirectory { command, path } => {
                            action = OverlayAction::AddDirectory { command, path };
                        }
                    }
                }
                OverlayState::SessionPicker(picker) => {
                    match apply_session_picker_key(picker, &key_event) {
                        SessionPickerKeyAction::None => {}
                        SessionPickerKeyAction::Close => {
                            self.overlay = None;
                            self.set_status_line("Closed session picker.");
                        }
                        SessionPickerKeyAction::Resume {
                            command,
                            session_id,
                        } => {
                            action = OverlayAction::Resume {
                                command,
                                session_id,
                            };
                        }
                        SessionPickerKeyAction::Fork {
                            command,
                            session_id,
                        } => {
                            action = OverlayAction::Fork {
                                command,
                                session_id,
                            };
                        }
                    }
                }
                OverlayState::ModelPicker(picker) => {
                    match apply_model_picker_key(picker, &key_event) {
                        ModelPickerKeyAction::None => {}
                        ModelPickerKeyAction::Close => {
                            self.overlay = None;
                            self.set_status_line("Closed model picker.");
                        }
                        ModelPickerKeyAction::SetModel {
                            command,
                            model,
                            effort,
                        } => {
                            action = OverlayAction::SetModel {
                                command,
                                model,
                                effort,
                            };
                        }
                    }
                }
                OverlayState::ThemePicker(picker) => {
                    match apply_theme_picker_key(picker, &key_event) {
                        ThemePickerKeyAction::None => {}
                        ThemePickerKeyAction::Close => {
                            self.overlay = None;
                            self.set_status_line("Theme picker dismissed.");
                        }
                        ThemePickerKeyAction::SetTheme { command, theme } => {
                            action = OverlayAction::SetTheme { command, theme };
                        }
                    }
                }
                OverlayState::OutputStylePicker(picker) => {
                    match apply_output_style_picker_key(picker, &key_event) {
                        OutputStylePickerKeyAction::None => {}
                        OutputStylePickerKeyAction::Close => {
                            self.overlay = None;
                            self.set_status_line("Output style picker dismissed.");
                        }
                        OutputStylePickerKeyAction::SetOutputStyle { command, style } => {
                            action = OverlayAction::SetOutputStyle { command, style };
                        }
                    }
                }
                OverlayState::ConfigPicker(picker) => {
                    match apply_config_picker_key(picker, &key_event) {
                        ConfigPickerKeyAction::None => {}
                        ConfigPickerKeyAction::Close => {
                            self.overlay = None;
                            self.set_status_line("Closed config.");
                        }
                        ConfigPickerKeyAction::Config {
                            command,
                            action: config_action,
                        } => {
                            action = OverlayAction::Config {
                                command,
                                action: config_action,
                            };
                        }
                    }
                }
                OverlayState::SandboxPicker(picker) => {
                    match apply_sandbox_picker_key(picker, &key_event) {
                        SandboxPickerKeyAction::None => {}
                        SandboxPickerKeyAction::Close => {
                            self.overlay = None;
                            self.set_status_line("Closed sandbox settings.");
                        }
                        SandboxPickerKeyAction::SetSandboxMode { command, choice } => {
                            action = OverlayAction::SetSandboxMode { command, choice };
                        }
                        SandboxPickerKeyAction::SetSandboxOverride { command, choice } => {
                            action = OverlayAction::SetSandboxOverride { command, choice };
                        }
                    }
                }
                OverlayState::MemoryPicker(picker) => {
                    match apply_memory_picker_key(picker, &key_event) {
                        MemoryPickerKeyAction::None => {}
                        MemoryPickerKeyAction::Close => {
                            self.overlay = None;
                            self.set_status_line("Closed memory selector.");
                        }
                        MemoryPickerKeyAction::EditMemory { command, path } => {
                            action = OverlayAction::EditMemory { command, path };
                        }
                        MemoryPickerKeyAction::OpenPath { command, path } => {
                            action = OverlayAction::OpenPath { command, path };
                        }
                    }
                }
                OverlayState::PermissionPicker(picker) => {
                    match apply_permission_picker_key(picker, &key_event) {
                        PermissionPickerKeyAction::None => {}
                        PermissionPickerKeyAction::Status(status) => {
                            self.set_status_line(status);
                        }
                        PermissionPickerKeyAction::Close { status } => {
                            self.overlay = None;
                            self.set_status_line(status);
                        }
                        PermissionPickerKeyAction::AddRule {
                            command,
                            scope,
                            kind,
                            rule,
                        } => {
                            action = OverlayAction::PermissionRuleUpdate {
                                command,
                                action: PermissionRuleAction::Add,
                                scope,
                                kind,
                                rule,
                            };
                        }
                        PermissionPickerKeyAction::RemoveRule {
                            command,
                            scope,
                            kind,
                            rule,
                        } => {
                            action = OverlayAction::PermissionRuleUpdate {
                                command,
                                action: PermissionRuleAction::Remove,
                                scope,
                                kind,
                                rule,
                            };
                        }
                    }
                }
                OverlayState::RewindPicker(picker) => {
                    match apply_rewind_picker_key(picker, &key_event) {
                        RewindPickerKeyAction::None => {}
                        RewindPickerKeyAction::Close => {
                            self.overlay = None;
                            self.set_status_line("Closed rewind picker.");
                        }
                        RewindPickerKeyAction::Rewind {
                            command,
                            session_id,
                            keep_messages,
                            anchor_id,
                            restore_prompt,
                        } => {
                            action = OverlayAction::Rewind {
                                command,
                                session_id,
                                keep_messages,
                                anchor_id,
                                restore_prompt,
                            };
                        }
                    }
                }
                OverlayState::Help(help) => match apply_help_overlay_key(help, &key_event) {
                    HelpOverlayAction::Close => {
                        self.overlay = None;
                        self.set_status_line("Closed help.");
                    }
                    HelpOverlayAction::OpenKeybindHelp => {
                        self.overlay =
                            Some(OverlayState::KeybindHelp(KeybindHelpOverlayState::default()));
                        self.set_status_line("Opened keybinding reference.");
                    }
                    HelpOverlayAction::None => {}
                },
                OverlayState::KeybindHelp(state) => {
                    if apply_keybind_help_overlay_key(state, &key_event)
                        == KeybindHelpOverlayAction::Close
                    {
                        self.overlay = None;
                        self.set_status_line("Closed keybinding help.");
                    }
                }
                OverlayState::Diff(diff) => {
                    if apply_diff_overlay_key(diff, &key_event) == DiffOverlayAction::Close {
                        self.overlay = None;
                        self.set_status_line("Closed diff.");
                    }
                }
                OverlayState::PermissionRequest(permission) => {
                    match apply_permission_request_key(permission, &key_event) {
                        PermissionRequestKeyAction::None => {}
                        PermissionRequestKeyAction::ToggleDetails => {
                            self.toggle_permission_request_details();
                        }
                        PermissionRequestKeyAction::Permission {
                            request_id,
                            decision,
                        } => {
                            action = OverlayAction::Permission {
                                request_id,
                                decision,
                            };
                        }
                    }
                }
                OverlayState::BackgroundJobs(state) => {
                    match apply_background_jobs_key(state, &key_event) {
                        BackgroundJobsOverlayAction::None => {}
                        BackgroundJobsOverlayAction::Close => {
                            self.overlay = None;
                            self.set_status_line("Closed background jobs.");
                        }
                        BackgroundJobsOverlayAction::CancelJob { job_index } => {
                            if let Some(job) = state.jobs.get(job_index) {
                                action = OverlayAction::CancelBackgroundJob {
                                    job_id: job.task_id.clone(),
                                };
                            }
                        }
                        BackgroundJobsOverlayAction::RequestRefresh => {
                            self.refresh_background_jobs_overlay(app_server).await;
                        }
                        BackgroundJobsOverlayAction::OpenChildSession { session_id } => {
                            match app_server.load_session_view(&session_id).await {
                                Ok(bootstrap) => {
                                    state.set_child_session(bootstrap.session);
                                    self.set_status_line(
                                        "Opened agent step output. Esc returns to workflow detail.",
                                    );
                                }
                                Err(error) => {
                                    self.set_status_line(format!(
                                        "Failed to open agent step output: {error}"
                                    ));
                                }
                            }
                        }
                        BackgroundJobsOverlayAction::CopyWorkflowStepOutput { output } => {
                            let char_count = output.chars().count();
                            match copy_text_to_clipboard(&output) {
                                Ok(()) => self.set_status_line(format!(
                                    "Copied workflow step output ({char_count} chars)."
                                )),
                                Err(error) => self.set_status_line(format!("Copy failed: {error}")),
                            }
                        }
                        BackgroundJobsOverlayAction::SetStatus { message } => {
                            self.set_status_line(message);
                        }
                    }
                }
                OverlayState::TranscriptPager(pager) => {
                    match super::transcript_pager::apply_transcript_pager_key(pager, &key_event) {
                        super::transcript_pager::TranscriptPagerAction::None => {}
                        super::transcript_pager::TranscriptPagerAction::Close => {
                            self.overlay = None;
                            self.set_status_line("Closed transcript pager.");
                        }
                    }
                }
            }
        }

        match action {
            OverlayAction::None => Ok(true),
            OverlayAction::AddDirectory { command, path } => {
                self.overlay = None;
                let path_str = path.to_string_lossy().to_string();
                match app_server.validate_add_directory(&path_str).await {
                    Ok(candidate) => {
                        let result = app_server
                            .add_directory(&self.session_id, &candidate.path.to_string_lossy())
                            .await?;
                        let message = format!(
                            "Added {} as a working directory for this session.",
                            result.path.display()
                        );
                        self.push_local_slash_command_output(
                            command,
                            message.clone(),
                            Some("Use /permissions to inspect.".to_string()),
                        );
                        self.set_status_line(message);
                    }
                    Err(error) => {
                        let message = format!("Cannot add directory: {error}");
                        self.push_local_slash_command_output(command, message.clone(), None);
                        self.set_status_line(message);
                    }
                }
                Ok(true)
            }
            OverlayAction::Resume {
                command,
                session_id,
            } => {
                let bootstrap = app_server.bootstrap(Some(&session_id)).await?;
                *self = Self::new(self.client.clone(), bootstrap);
                let status = format!("Session {} resumed.", short_session_id(&self.session_id));
                self.push_local_slash_command_output(command, status.clone(), None);
                self.set_status_line(status);
                Ok(true)
            }
            OverlayAction::Rewind {
                command,
                session_id,
                keep_messages,
                anchor_id,
                restore_prompt,
            } => {
                // Resolve the kept count against the *persisted* record: the
                // in-memory display list can diverge from the transcript the
                // server truncates, so an index into the display list would
                // rewind to the wrong point. Fall back to the display estimate
                // if the anchor can't be located.
                let keep = match app_server.load_session_view(&session_id).await {
                    Ok(view) => view
                        .session
                        .messages
                        .iter()
                        .position(|message| message.id == anchor_id)
                        .unwrap_or(keep_messages),
                    Err(_) => keep_messages,
                };
                let preserved_suggestions = self.mcp_slash_suggestions.clone();
                let bootstrap = app_server.rewind_session(&session_id, keep).await?;
                *self = Self::new(self.client.clone(), bootstrap);
                self.mcp_slash_suggestions = preserved_suggestions;
                self.input = restore_prompt;
                self.input_cursor = self.input.len();
                let status = format!("Rewound to an earlier turn ({keep} message(s) kept).");
                self.push_local_slash_command_output(command, status.clone(), None);
                self.set_status_line(status);
                Ok(true)
            }
            OverlayAction::Fork {
                command,
                session_id,
            } => {
                let fork = app_server.fork_session(&session_id, None, None).await?;
                let bootstrap = app_server.bootstrap(Some(&fork.session_id)).await?;
                *self = Self::new(self.client.clone(), bootstrap);
                let status = format!(
                    "Forked into session {}.",
                    short_session_id(&self.session_id)
                );
                self.push_local_slash_command_output(command, status.clone(), None);
                self.set_status_line(status);
                Ok(true)
            }
            OverlayAction::SetModel {
                command,
                model,
                effort,
            } => {
                self.finish_model_selection(app_server, command, model, effort)
                    .await?;
                Ok(true)
            }
            OverlayAction::SetTheme { command, theme } => {
                self.finish_theme_selection(app_server, command, theme)
                    .await?;
                Ok(true)
            }
            OverlayAction::SetOutputStyle { command, style } => {
                self.finish_output_style_selection(app_server, command, style)
                    .await?;
                Ok(true)
            }
            OverlayAction::Config { command, action } => {
                self.apply_config_action(app_server, command, action)
                    .await?;
                Ok(true)
            }
            OverlayAction::SetSandboxMode { command, choice } => {
                self.apply_sandbox_mode_choice(app_server, command, choice)
                    .await?;
                Ok(true)
            }
            OverlayAction::SetSandboxOverride { command, choice } => {
                self.apply_sandbox_override_choice(app_server, command, choice)
                    .await?;
                Ok(true)
            }
            OverlayAction::EditMemory { command, path } => {
                app_server
                    .ensure_memory_file(&path.to_string_lossy())
                    .await?;
                self.overlay = None;
                self.external_editor_request = Some(ExternalEditorRequest {
                    command: command.clone(),
                    path: path.clone(),
                    target: ExternalEditorTarget::Memory,
                });
                self.push_local_slash_command_output(
                    command,
                    format!("Opening memory file {}.", path.display()),
                    None,
                );
                self.set_status_line(format!("Opening memory file {}...", path.display()));
                Ok(true)
            }
            OverlayAction::OpenPath { command, path } => {
                tokio::fs::create_dir_all(&path).await?;
                self.overlay = None;
                match open_path_in_system(&path) {
                    Ok(()) => {
                        let status = format!("Opened {}.", path.display());
                        self.push_local_slash_command_output(command, status.clone(), None);
                        self.set_status_line(status);
                    }
                    Err(error) => {
                        let status = format!("Open path failed: {error}");
                        self.push_local_slash_command_output(command, status.clone(), None);
                        self.set_status_line(status);
                    }
                }
                Ok(true)
            }
            OverlayAction::PermissionRuleUpdate {
                command,
                action,
                scope,
                kind,
                rule,
            } => {
                self.apply_permission_rule_update(app_server, command, action, scope, kind, rule)
                    .await?;
                Ok(true)
            }
            OverlayAction::CancelBackgroundJob { job_id } => {
                match app_server.cancel_background_job(&job_id).await {
                    Ok(record) => {
                        let status = format!(
                            "Cancelled background job {}.",
                            short_session_id(&record.job_id)
                        );
                        self.set_status_line(&status);
                        self.refresh_background_jobs_overlay(app_server).await;
                    }
                    Err(error) => {
                        self.set_status_line(format!("Cancel failed: {error}"));
                    }
                }
                Ok(true)
            }
            OverlayAction::Permission {
                request_id,
                decision,
            } => {
                let denied_request = if matches!(decision, PermissionDecision::Deny) {
                    match &self.overlay {
                        Some(OverlayState::PermissionRequest(permission)) => {
                            Some(permission.request.clone())
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                if app_server
                    .respond_to_pending_permission_request(&request_id, decision.clone())
                    .await
                {
                    if let Some(request) = denied_request {
                        self.record_recent_denied_permission(request);
                    }
                    if let PermissionDecision::ApproveAlways(ref rule) = decision {
                        self.set_status_line(format!("Remembered `{rule}` for this session."));
                    } else if let PermissionDecision::ApproveAlwaysMany(ref rules) = decision {
                        self.set_status_line(format!(
                            "Remembered {} rules for this session.",
                            rules.len()
                        ));
                    }
                } else {
                    self.set_status_line("Permission request expired.");
                    self.overlay = None;
                }
                Ok(true)
            }
        }
    }
}
