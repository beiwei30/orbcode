use orbcode_app_server_client::{
    AgentDefinition, AgentLoadWarning, AppClient, CompactSessionResult, ContextOverview,
    CostOverview, DoctorReport, HookDiscovery, MemoryOverview, PermissionOverview, SkillDefinition,
    StatsOverview, StatusOverview, UsageOverview, WorkspaceDiff,
};
use orbcode_protocol::StreamEvent;
use tokio::sync::mpsc;

use crate::chat::stream_events::mark_redraw;
use crate::commands::plan::PlanCommandResult;
use crate::history_cell::local_note::{
    LocalTranscriptNote, nonempty_detail, parse_local_transcript_note,
};
use crate::overlays::{DiffOverlayState, MemoryPickerState, OverlayState};
use crate::render::slash_output::{
    render_agent_definitions_with_warnings, render_context_overview, render_cost_overview,
    render_doctor_report, render_hook_discovery, render_skill_definitions, render_stats_overview,
    render_stats_summary, render_status_overview, render_usage_overview,
    workspace_diff_changed_path_count,
};
use crate::render_metrics::RenderEventCounts;
use crate::slash_commands::AsyncLocalSlashCommand;
use crate::state::TuiState;

/// A [`LocalCommandEvent`] tagged with the session it was launched for.
///
/// Detached local commands can complete after the user switched sessions:
/// `/clear`, `/fork`, and `/resume` replace the whole `TuiState` (`*self =
/// Self::new(...)`), but the command channel lives in the run loop and persists.
/// Without the origin tag, a stale result would be applied to — and rendered
/// in — the wrong session. The apply path drops any envelope whose origin no
/// longer matches the current `session_id`.
pub(crate) struct LocalCommandEnvelope {
    pub(crate) origin_session_id: String,
    pub(crate) event: LocalCommandEvent,
}

impl LocalCommandEnvelope {
    pub(crate) fn new(origin_session_id: impl Into<String>, event: LocalCommandEvent) -> Self {
        Self {
            origin_session_id: origin_session_id.into(),
            event,
        }
    }
}

#[allow(clippy::large_enum_variant, clippy::enum_variant_names)]
pub(crate) enum LocalCommandEvent {
    AgentsFinished(Result<(Vec<AgentDefinition>, Vec<AgentLoadWarning>), String>),
    CompactFinished {
        command: String,
        result: Result<CompactSessionResult, String>,
    },
    CompactNeedsConfirmation {
        command: String,
        context_percent_used: u32,
        threshold_percent: u32,
    },
    ContextFinished {
        command: String,
        full: bool,
        result: Result<ContextOverview, String>,
    },
    CostFinished(Result<CostOverview, String>),
    DiffFinished(Result<WorkspaceDiff, String>),
    DoctorFinished(Result<DoctorReport, String>),
    HooksFinished(Result<HookDiscovery, String>),
    InstructionsFinished(Result<String, String>),
    MemoryFinished(Result<MemoryOverview, String>),
    PermissionsFinished(Result<PermissionOverview, String>),
    PlanFinished(Result<PlanCommandResult, String>),
    SkillsFinished(Result<Vec<SkillDefinition>, String>),
    StatsFinished(Result<StatsOverview, String>),
    StatusFinished(Result<StatusOverview, String>),
    UsageFinished(Result<UsageOverview, String>),
}

pub(crate) async fn handle_local_command_event_batch(
    state: &mut TuiState,
    app_server: &AppClient,
    turn_events: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
    first_envelope: LocalCommandEnvelope,
    local_command_rx: &mut mpsc::UnboundedReceiver<LocalCommandEnvelope>,
    event_counts: &mut RenderEventCounts,
    needs_redraw: &mut bool,
    redraw_reasons: &mut Vec<&'static str>,
) -> anyhow::Result<()> {
    let mut pending_prompt = apply_local_command_envelope_for_redraw(
        state,
        first_envelope,
        event_counts,
        needs_redraw,
        redraw_reasons,
    );
    while let Ok(envelope) = local_command_rx.try_recv() {
        let prompt = apply_local_command_envelope_for_redraw(
            state,
            envelope,
            event_counts,
            needs_redraw,
            redraw_reasons,
        );
        if pending_prompt.is_none() {
            pending_prompt = prompt;
        }
    }

    if turn_events.is_none()
        && let Some(prompt) = pending_prompt
    {
        *turn_events = Some(
            app_server
                .submit_turn_stream(&state.session_id, prompt)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        );
        state.set_status_line("Starting provider request...");
    }

    Ok(())
}

/// Apply an envelope, dropping it if it was launched for a session the TUI has
/// since left (`/clear`, `/fork`, `/resume`). A dropped envelope still counts as
/// an event but produces no state mutation and no follow-up prompt.
pub(crate) fn apply_local_command_envelope_for_redraw(
    state: &mut TuiState,
    envelope: LocalCommandEnvelope,
    event_counts: &mut RenderEventCounts,
    needs_redraw: &mut bool,
    redraw_reasons: &mut Vec<&'static str>,
) -> Option<String> {
    if envelope.origin_session_id != state.session_id {
        // Stale result from a previous session; ignore so it does not land in
        // the current session's transcript.
        event_counts.local_command_events += 1;
        return None;
    }
    apply_local_command_event_for_redraw(
        state,
        envelope.event,
        event_counts,
        needs_redraw,
        redraw_reasons,
    )
}

pub(crate) fn apply_local_command_event_for_redraw(
    state: &mut TuiState,
    event: LocalCommandEvent,
    event_counts: &mut RenderEventCounts,
    needs_redraw: &mut bool,
    redraw_reasons: &mut Vec<&'static str>,
) -> Option<String> {
    event_counts.local_command_events += 1;
    let prompt = state.apply_local_command_event(event);
    mark_redraw(needs_redraw, redraw_reasons, "local_command_event");
    prompt
}

/// Deserialize a `serde_json::Value` into a typed `T`, converting errors to
/// `String` so the result fits the `LocalCommandEvent` error channel.
fn from_value<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| format!("protocol deserialization error: {e}"))
}

impl AsyncLocalSlashCommand {
    pub(crate) fn loading_status(self) -> &'static str {
        match self {
            Self::Agents => "Loading agent definitions...",
            Self::Context => "Loading context usage...",
            Self::Cost => "Loading cost...",
            Self::Diff => "Loading workspace diff...",
            Self::Doctor => "Running doctor...",
            Self::Hooks => "Loading hooks...",
            Self::Instructions => "Loading instructions...",
            Self::Memory => "Loading memory...",
            Self::Permissions => "Loading permissions...",
            Self::Skills => "Loading skill definitions...",
            Self::Stats => "Loading stats...",
            Self::Status => "Loading status...",
            Self::Usage => "Loading usage...",
        }
    }

    /// Execute the slash command asynchronously via the protocol-preserving
    /// [`AppClient`] facade. Each command goes through the full protocol
    /// round-trip (serialize -> MessageProcessor -> deserialize), ensuring
    /// in-process callers exercise the exact same path as out-of-process
    /// transports.
    ///
    /// When `client` is `Some` (all production paths), the client is used
    /// for every operation that has an `AppClient` equivalent. When `None`
    /// (unit-test fixtures only), `server` is used as a fallback.
    ///
    /// Execute via the `AppClient` protocol facade. All commands now route
    /// through protocol methods.
    pub(crate) async fn run(self, client: &AppClient, session_id: &str) -> LocalCommandEvent {
        self.run_via_client(client, session_id).await
    }

    /// Execute via the protocol-preserving [`AppClient`] facade (production path).
    pub(crate) async fn run_via_client(
        self,
        client: &AppClient,
        session_id: &str,
    ) -> LocalCommandEvent {
        match self {
            Self::Agents => {
                #[derive(serde::Deserialize)]
                struct AgentResult {
                    definitions: Vec<AgentDefinition>,
                    warnings: Vec<AgentLoadWarning>,
                }
                let result = client
                    .agent_definitions_with_warnings()
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value::<AgentResult>)
                    .map(|r| (r.definitions, r.warnings));
                LocalCommandEvent::AgentsFinished(result)
            }
            Self::Instructions => {
                let result = client
                    .pre_user_instructions_preview(session_id)
                    .await
                    .map_err(|e| e.to_string())
                    .map(|v| v["preview"].as_str().unwrap_or("").to_string());
                LocalCommandEvent::InstructionsFinished(result)
            }
            Self::Hooks => {
                let result = client
                    .hook_discovery()
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value);
                LocalCommandEvent::HooksFinished(result)
            }
            Self::Context => {
                let result = client
                    .context_overview(session_id)
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value);
                LocalCommandEvent::ContextFinished {
                    command: "/context".to_string(),
                    full: false,
                    result,
                }
            }
            Self::Cost => {
                let result = client
                    .cost_overview(session_id)
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value);
                LocalCommandEvent::CostFinished(result)
            }
            Self::Diff => {
                let result = client
                    .workspace_diff()
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value);
                LocalCommandEvent::DiffFinished(result)
            }
            Self::Doctor => {
                let result = client
                    .doctor_report()
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value);
                LocalCommandEvent::DoctorFinished(result)
            }
            Self::Memory => {
                let result = client
                    .memory_overview()
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value);
                LocalCommandEvent::MemoryFinished(result)
            }
            Self::Permissions => {
                let result = client
                    .permission_overview()
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value);
                LocalCommandEvent::PermissionsFinished(result)
            }
            Self::Skills => {
                let result = client
                    .skill_definitions_for_session(session_id)
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value);
                LocalCommandEvent::SkillsFinished(result)
            }
            Self::Stats => {
                let result = client
                    .stats_overview()
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value);
                LocalCommandEvent::StatsFinished(result)
            }
            Self::Status => {
                let result = client
                    .status_overview(session_id)
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value);
                LocalCommandEvent::StatusFinished(result)
            }
            Self::Usage => {
                let result = client
                    .usage_overview(session_id)
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(from_value);
                LocalCommandEvent::UsageFinished(result)
            }
        }
    }
}

impl TuiState {
    pub(crate) fn start_async_local_slash_command(
        &mut self,
        command: AsyncLocalSlashCommand,
        _app_server: &AppClient,
        local_command_tx: &mpsc::UnboundedSender<LocalCommandEnvelope>,
    ) {
        let client = self.app_client();
        let session_id = self.session_id.clone();
        let local_command_tx = local_command_tx.clone();
        self.set_status_line(command.loading_status());
        // Detached local command; completion is routed back through
        // LocalCommandEvent.
        let _local_command_handle = tokio::spawn(async move {
            let event = command.run(&client, &session_id).await;
            let _ = local_command_tx.send(LocalCommandEnvelope::new(session_id, event));
        });
    }

    pub(crate) fn start_context_slash_command(
        &mut self,
        command_line: String,
        full: bool,
        _app_server: &AppClient,
        local_command_tx: &mpsc::UnboundedSender<LocalCommandEnvelope>,
    ) {
        let client = self.app_client();
        let session_id = self.session_id.clone();
        let local_command_tx = local_command_tx.clone();
        self.set_status_line(AsyncLocalSlashCommand::Context.loading_status());
        self.push_local_slash_command_output(&command_line, "Loading context usage...", None);
        // Detached context command; completion is routed back through
        // LocalCommandEvent.
        let _context_command_handle = tokio::spawn(async move {
            let result = client
                .context_overview(&session_id)
                .await
                .map_err(|e| e.to_string())
                .and_then(from_value);
            let _ = local_command_tx.send(LocalCommandEnvelope::new(
                session_id,
                LocalCommandEvent::ContextFinished {
                    command: command_line,
                    full,
                    result,
                },
            ));
        });
    }

    pub(crate) fn apply_local_command_event(&mut self, event: LocalCommandEvent) -> Option<String> {
        match event {
            LocalCommandEvent::AgentsFinished(Ok((definitions, warnings))) => {
                let count = definitions.len();
                let warn_count = warnings.len();
                let summary = if warn_count > 0 {
                    format!("{count} agent definition(s) loaded, {warn_count} warning(s).")
                } else {
                    format!("{count} agent definition(s) loaded.")
                };
                let detail = render_agent_definitions_with_warnings(&definitions, &warnings);
                self.push_local_slash_command_output("/agents", summary.clone(), Some(detail));
                self.set_status_line(summary);
                None
            }
            LocalCommandEvent::AgentsFinished(Err(error)) => {
                let status = format!("Agents failed: {error}");
                self.push_local_slash_command_output(
                    "/agents",
                    "Agents failed.",
                    Some(error.clone()),
                );
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::CompactFinished {
                command,
                result: Ok(result),
            } => {
                self.apply_compact_result(&command, result);
                None
            }
            LocalCommandEvent::CompactFinished {
                command,
                result: Err(error),
            } => {
                self.compact_started_at = None;
                self.remove_pending_compact_output();
                let status = format!("Compact failed: {error}");
                self.push_local_slash_command_output(command, "Compact failed.", Some(error));
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::CompactNeedsConfirmation {
                command,
                context_percent_used,
                threshold_percent,
            } => {
                self.compact_started_at = None;
                self.remove_pending_compact_output();
                let summary = format!(
                    "Context is {context_percent_used}% used (threshold: {threshold_percent}%)."
                );
                self.push_local_slash_command_output(
                    command,
                    &summary,
                    Some("Run /compact force to compact anyway.".to_string()),
                );
                self.set_status_line(summary);
                None
            }
            LocalCommandEvent::ContextFinished {
                command,
                full,
                result: Ok(overview),
            } => {
                self.remove_pending_context_output(&command);
                self.push_local_slash_command_output(
                    &command,
                    "Context usage loaded.",
                    Some(render_context_overview(&overview, full)),
                );
                self.set_status_line("Context usage loaded.");
                None
            }
            LocalCommandEvent::ContextFinished {
                command,
                result: Err(error),
                ..
            } => {
                self.remove_pending_context_output(&command);
                let status = format!("Context usage failed: {error}");
                self.push_local_slash_command_output(
                    &command,
                    "Context usage failed.",
                    Some(error.clone()),
                );
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::CostFinished(Ok(overview)) => {
                self.push_local_slash_command_output(
                    "/cost",
                    "Cost loaded.",
                    Some(render_cost_overview(&overview)),
                );
                self.set_status_line("Cost loaded.");
                None
            }
            LocalCommandEvent::CostFinished(Err(error)) => {
                let status = format!("Cost failed: {error}");
                self.push_local_slash_command_output("/cost", "Cost failed.", Some(error.clone()));
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::InstructionsFinished(Ok(system_prompt)) => {
                self.push_local_slash_command_output(
                    "/instructions",
                    "Instructions loaded.",
                    Some(system_prompt),
                );
                self.set_status_line("Instructions loaded.");
                None
            }
            LocalCommandEvent::InstructionsFinished(Err(error)) => {
                let status = format!("Instructions failed: {error}");
                self.push_local_slash_command_output(
                    "/instructions",
                    "Instructions failed.",
                    Some(error.clone()),
                );
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::DoctorFinished(Ok(report)) => {
                let (pass, warn, fail) = report.counts();
                let has_failures = report.has_failures();
                let status = if has_failures {
                    format!("Doctor completed: {pass} pass, {warn} warn, {fail} fail.")
                } else {
                    format!("Doctor passed: {pass} pass, {warn} warn.")
                };
                self.push_local_slash_command_output(
                    "/doctor",
                    status.clone(),
                    Some(render_doctor_report(&report)),
                );
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::DoctorFinished(Err(error)) => {
                let status = format!("Doctor failed: {error}");
                self.push_local_slash_command_output(
                    "/doctor",
                    "Doctor failed.",
                    Some(error.clone()),
                );
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::HooksFinished(Ok(discovery)) => {
                let count = discovery.hooks.len();
                let warn_count = discovery.warnings.len();
                let summary = if warn_count > 0 {
                    format!("{count} hook(s) discovered, {warn_count} warning(s).")
                } else {
                    format!("{count} hook(s) discovered.")
                };
                self.push_local_slash_command_output(
                    "/hooks",
                    summary.clone(),
                    Some(render_hook_discovery(&discovery)),
                );
                self.set_status_line(summary);
                None
            }
            LocalCommandEvent::HooksFinished(Err(error)) => {
                let status = format!("Hooks failed: {error}");
                self.push_local_slash_command_output(
                    "/hooks",
                    "Hooks failed.",
                    Some(error.clone()),
                );
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::DiffFinished(Ok(diff)) => {
                let changed_paths = workspace_diff_changed_path_count(&diff);
                self.overlay = Some(OverlayState::Diff(DiffOverlayState::new(diff)));
                if changed_paths == 0 {
                    self.push_local_slash_command_output("/diff", "Workspace diff is clean.", None);
                    self.set_status_line("Workspace diff is clean. Press Esc to close.");
                } else {
                    let status = format!("Opened diff mode: {changed_paths} changed path(s).");
                    self.push_local_slash_command_output("/diff", status.clone(), None);
                    self.set_status_line(status);
                }
                None
            }
            LocalCommandEvent::DiffFinished(Err(error)) => {
                let status = format!("Diff failed: {error}");
                self.push_local_slash_command_output("/diff", "Diff failed.", Some(error));
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::MemoryFinished(Ok(overview)) => {
                self.overlay = Some(OverlayState::MemoryPicker(MemoryPickerState::new(
                    "/memory", overview,
                )));
                self.set_status_line("Memory selector: Enter confirm, Esc cancel.");
                None
            }
            LocalCommandEvent::MemoryFinished(Err(error)) => {
                let status = format!("Memory failed: {error}");
                self.push_local_slash_command_output("/memory", "Memory failed.", Some(error));
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::PermissionsFinished(Ok(overview)) => {
                self.open_permission_picker("/permissions", overview);
                None
            }
            LocalCommandEvent::PermissionsFinished(Err(error)) => {
                let status = format!("Permissions failed: {error}");
                self.push_local_slash_command_output(
                    "/permissions",
                    "Permissions failed.",
                    Some(error.clone()),
                );
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::PlanFinished(Ok(result)) => {
                self.push_local_slash_command_output(
                    result.command,
                    result.status.clone(),
                    nonempty_detail(result.output),
                );
                self.set_status_line(result.status);
                result.submit_prompt
            }
            LocalCommandEvent::PlanFinished(Err(error)) => {
                let status = format!("Plan failed: {error}");
                self.push_local_slash_command_output("/plan", "Plan failed.", Some(error.clone()));
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::SkillsFinished(Ok(definitions)) => {
                let count = definitions.len();
                let summary = format!("{count} skill(s) loaded.");
                self.push_local_slash_command_output(
                    "/skills",
                    summary.clone(),
                    Some(render_skill_definitions(&definitions)),
                );
                self.set_status_line(summary);
                None
            }
            LocalCommandEvent::SkillsFinished(Err(error)) => {
                let status = format!("Skills failed: {error}");
                self.push_local_slash_command_output(
                    "/skills",
                    "Skills failed.",
                    Some(error.clone()),
                );
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::StatusFinished(Ok(overview)) => {
                self.push_local_slash_command_output(
                    "/status",
                    "Status loaded.",
                    Some(render_status_overview(&overview)),
                );
                self.set_status_line("Status loaded.");
                None
            }
            LocalCommandEvent::StatusFinished(Err(error)) => {
                let status = format!("Status failed: {error}");
                self.push_local_slash_command_output(
                    "/status",
                    "Status failed.",
                    Some(error.clone()),
                );
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::StatsFinished(Ok(overview)) => {
                self.push_local_slash_command_output(
                    "/stats",
                    render_stats_summary(&overview),
                    Some(render_stats_overview(&overview)),
                );
                self.set_status_line("Stats loaded.");
                None
            }
            LocalCommandEvent::StatsFinished(Err(error)) => {
                let status = format!("Stats failed: {error}");
                self.push_local_slash_command_output(
                    "/stats",
                    "Stats failed.",
                    Some(error.clone()),
                );
                self.set_status_line(status);
                None
            }
            LocalCommandEvent::UsageFinished(Ok(overview)) => {
                self.push_local_slash_command_output(
                    "/usage",
                    "Usage loaded.",
                    Some(render_usage_overview(&overview)),
                );
                self.set_status_line("Usage loaded.");
                None
            }
            LocalCommandEvent::UsageFinished(Err(error)) => {
                let status = format!("Usage failed: {error}");
                self.push_local_slash_command_output(
                    "/usage",
                    "Usage failed.",
                    Some(error.clone()),
                );
                self.set_status_line(status);
                None
            }
        }
    }

    fn remove_pending_context_output(&mut self, command: &str) {
        let Some(index) = self.messages.iter().rposition(|message| {
            matches!(
                parse_local_transcript_note(message),
                Some(LocalTranscriptNote::SlashCommandOutput {
                    command: note_command,
                    summary,
                    ..
                }) if note_command == command && summary == "Loading context usage..."
            )
        }) else {
            return;
        };
        self.messages.remove(index);
    }
}
