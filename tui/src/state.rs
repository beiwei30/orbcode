use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crossterm::cursor::SetCursorStyle;
use orbcode_app_server_client::{AppClient, BootstrapState, McpSlashSuggestionCatalog};
use orbcode_config::{
    ContextWindowOptions, MaxOutputTokenOptions, PermissionMode, TokenWarningOptions,
    calculate_token_warning_state,
};
use orbcode_protocol::{EffortLevel, ProviderId, StreamEvent, TokenUsage, TranscriptMessage};
use ratatui::prelude::Rect;
use serde_json::Value;

use crate::background_agent_panel::BackgroundAgentPanelState;
use crate::bottom_pane::slash_suggestions::SlashSuggestionLinesCache;
use crate::bottom_pane::vim::{LastFind, VimRuntimeState};
use crate::editor_mode::{EditorMode, editor_mode_from_setting};
use crate::external_editor::ExternalEditorRequest;
use crate::history_cell::state::TranscriptUiState;
use crate::overlays::{OverlayState, RecentlyDeniedPermission, overlay_cursor_style};
use crate::prompt_state::{
    ActiveThinkingState, DeferredAssistantMessage, InputSelectionState, NormalPending,
};
use crate::task_panel::TaskPanelState;
use crate::tool_cell::live_state::LiveToolCells;
use crate::transcript_task_cards::TranscriptTaskCardsState;
use crate::tui_theme::set_active_theme;
use crate::workspace_display::shorten_display_path;

#[cfg(test)]
pub(crate) type TuiClientHandle = Option<Arc<AppClient>>;
#[cfg(not(test))]
pub(crate) type TuiClientHandle = Arc<AppClient>;

#[derive(Clone, Debug)]
pub(crate) struct ClearSessionInfo {
    pub(crate) session_id: String,
    pub(crate) usage: Option<TokenUsage>,
}

#[derive(Clone, Debug)]
pub(crate) struct StatusLineState {
    pub(crate) context_percent_left: Option<u32>,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) allow_all: bool,
    pub(crate) sandbox_mode: String,
    pub(crate) bg_job_count: usize,
    pub(crate) has_rate_limit_warning: bool,
    pub(crate) has_auth_warning: bool,
    pub(crate) git_branch: Option<String>,
    pub(crate) custom_command_output: Option<String>,
    /// Runtime reasoning-effort override, shown next to the model name. `None`
    /// means the model's default effort (nothing is displayed).
    pub(crate) effort: Option<EffortLevel>,
}

impl Default for StatusLineState {
    fn default() -> Self {
        Self {
            context_percent_left: None,
            permission_mode: PermissionMode::Default,
            allow_all: false,
            sandbox_mode: String::new(),
            bg_job_count: 0,
            has_rate_limit_warning: false,
            has_auth_warning: false,
            git_branch: None,
            custom_command_output: None,
            effort: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RequestTokenDirection {
    #[default]
    Up,
    Down,
}

impl RequestTokenDirection {
    pub(crate) fn glyph(self) -> char {
        match self {
            Self::Up => '↑',
            Self::Down => '↓',
        }
    }
}

pub(crate) struct TuiState {
    /// Protocol-preserving async client for app-server operations. Production
    /// TUI state always carries a client; test fixtures may omit it.
    pub(crate) client: TuiClientHandle,
    pub(crate) session_id: String,
    pub(crate) cwd: PathBuf,
    pub(crate) messages: Vec<TranscriptMessage>,
    pub(crate) transcript_ui: TranscriptUiState,
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) input_tail_pinned: bool,
    pub(crate) input_area: Rect,
    pub(crate) input_selection: Option<InputSelectionState>,
    pub(crate) desired_column: Option<usize>,
    pub(crate) prompt_history: Vec<String>,
    pub(crate) prompt_history_index: Option<usize>,
    pub(crate) slash_command_selected: usize,
    pub(crate) steered_followups: VecDeque<String>,
    pub(crate) queued_followups: VecDeque<String>,
    pub(crate) pending_assistant: String,
    pub(crate) compact_started_at: Option<Instant>,
    pub(crate) deferred_assistant_message: Option<DeferredAssistantMessage>,
    pub(crate) active_thinking: Option<ActiveThinkingState>,
    pub(crate) live_tool_cells: LiveToolCells,
    pub(crate) in_progress_tool_use_ids: HashSet<String>,
    pub(crate) pending_hook_progress: Vec<Value>,
    pub(crate) hook_progress_by_message_id: HashMap<String, Vec<Value>>,
    pub(crate) history_flushed_message_count: usize,
    pub(crate) retained_visible_transcript_cells: usize,
    pub(crate) focus_latest_message_start: bool,
    pub(crate) pending_history_flush: bool,
    pub(crate) overlay: Option<OverlayState>,
    pub(crate) recent_denied_permissions: Vec<RecentlyDeniedPermission>,
    pub(crate) status_line: String,
    pub(crate) status_line_set_at: Option<Instant>,
    pub(crate) ui_version: String,
    pub(crate) cwd_display: String,
    pub(crate) model_display_name: String,
    pub(crate) context_window_options: ContextWindowOptions,
    pub(crate) max_output_token_options: MaxOutputTokenOptions,
    pub(crate) token_warning_options: TokenWarningOptions,
    pub(crate) default_provider_label: String,
    pub(crate) show_update_notice: bool,
    pub(crate) expanded_tool_details: bool,
    pub(crate) request_in_flight: bool,
    pub(crate) spinner_frame: usize,
    pub(crate) spinner_verb_index: usize,
    pub(crate) request_count: usize,
    pub(crate) request_started_at: Option<Instant>,
    pub(crate) streamed_response_chars: usize,
    pub(crate) request_token_direction: RequestTokenDirection,
    pub(crate) current_turn_total_tokens: u64,
    pub(crate) last_provider: Option<ProviderId>,
    pub(crate) last_usage: Option<TokenUsage>,
    pub(crate) editor_mode: EditorMode,
    pub(crate) normal_pending: Option<NormalPending>,
    pub(crate) last_find: Option<LastFind>,
    pub(crate) normal_count: Option<usize>,
    pub(crate) vim_state: VimRuntimeState,
    pub(crate) external_editor_request: Option<ExternalEditorRequest>,
    pub(crate) slash_suggestion_lines_cache: SlashSuggestionLinesCache,
    pub(crate) mcp_slash_suggestions: McpSlashSuggestionCatalog,
    pub(crate) mcp_slash_suggestion_revision: u64,
    pub(crate) mcp_slash_suggestion_refresh_key: Option<String>,
    pub(crate) task_panel: TaskPanelState,
    pub(crate) background_agent_panel: BackgroundAgentPanelState,
    pub(crate) transcript_task_cards: TranscriptTaskCardsState,
    pub(crate) status: StatusLineState,
    pub(crate) statusline_command: Option<String>,
    pub(crate) statusline_refresh_interval: std::time::Duration,
    pub(crate) clear_session_info: Option<ClearSessionInfo>,
}

impl TuiState {
    pub(crate) fn new(client: TuiClientHandle, bootstrap: BootstrapState) -> Self {
        set_active_theme(bootstrap.theme);
        let keybinding_warnings = crate::keybindings::keybinding_warnings();
        // Orb Code's own version, from the workspace Cargo version — the single
        // source of truth shared with `--version` and the doctor build_info row.
        let ui_version = env!("CARGO_PKG_VERSION").to_string();
        let cwd_display = shorten_display_path(&bootstrap.cwd);
        let cwd = bootstrap.cwd;
        let is_new_session = matches!(
            bootstrap.bootstrap_event,
            StreamEvent::SessionStarted { .. }
        );
        let task_panel =
            TaskPanelState::new(Some(bootstrap.session.session_id.as_str()), is_new_session);
        let mut background_agent_panel = BackgroundAgentPanelState::new();
        background_agent_panel.set_session_id(bootstrap.session.session_id.clone());
        let mut transcript_task_cards = TranscriptTaskCardsState::new();
        transcript_task_cards.set_session_id(bootstrap.session.session_id.clone());
        let messages = bootstrap.session.messages;
        let transcript_ui = TranscriptUiState::from_messages(&messages, &cwd);
        let status_line = if keybinding_warnings.is_empty() {
            match bootstrap.bootstrap_event {
                StreamEvent::SessionStarted { .. } => {
                    "New session ready. Enter submits. /help shows shell commands.".to_string()
                }
                StreamEvent::SessionLoaded { .. } => {
                    "Session resumed. Enter submits. /help shows shell commands.".to_string()
                }
                _ => "Ready.".to_string(),
            }
        } else {
            format!(
                "{} keybinding warning(s); run /keybindings to review.",
                keybinding_warnings.len()
            )
        };

        Self {
            client,
            session_id: bootstrap.session.session_id,
            cwd,
            messages,
            transcript_ui,
            input: String::new(),
            input_cursor: 0,
            input_tail_pinned: false,
            input_area: Rect::ZERO,
            input_selection: None,
            desired_column: None,
            prompt_history: bootstrap.prompt_history,
            prompt_history_index: None,
            slash_command_selected: 0,
            steered_followups: VecDeque::new(),
            queued_followups: VecDeque::new(),
            pending_assistant: String::new(),
            compact_started_at: None,
            deferred_assistant_message: None,
            active_thinking: None,
            live_tool_cells: LiveToolCells::default(),
            in_progress_tool_use_ids: HashSet::new(),
            pending_hook_progress: Vec::new(),
            hook_progress_by_message_id: HashMap::new(),
            history_flushed_message_count: 0,
            retained_visible_transcript_cells: 0,
            focus_latest_message_start: false,
            pending_history_flush: false,
            overlay: None,
            recent_denied_permissions: Vec::new(),
            status_line,
            status_line_set_at: None,
            ui_version,
            cwd_display,
            model_display_name: bootstrap.model_display_name,
            context_window_options: bootstrap.context_window_options,
            max_output_token_options: bootstrap.max_output_token_options,
            token_warning_options: bootstrap.token_warning_options,
            default_provider_label: bootstrap.default_provider.as_str().to_string(),
            show_update_notice: false,
            expanded_tool_details: false,
            request_in_flight: false,
            spinner_frame: 0,
            spinner_verb_index: 0,
            request_count: 0,
            request_started_at: None,
            streamed_response_chars: 0,
            request_token_direction: RequestTokenDirection::Up,
            current_turn_total_tokens: 0,
            last_provider: None,
            last_usage: None,
            editor_mode: editor_mode_from_setting(bootstrap.editor_mode),
            normal_pending: None,
            last_find: None,
            normal_count: None,
            vim_state: VimRuntimeState::default(),
            external_editor_request: None,
            slash_suggestion_lines_cache: SlashSuggestionLinesCache::default(),
            mcp_slash_suggestions: bootstrap.mcp_slash_suggestions,
            mcp_slash_suggestion_revision: 0,
            mcp_slash_suggestion_refresh_key: None,
            task_panel,
            background_agent_panel,
            transcript_task_cards,
            status: StatusLineState::default(),
            statusline_command: bootstrap.statusline_command,
            statusline_refresh_interval: std::time::Duration::from_secs(
                bootstrap.statusline_refresh_interval_secs,
            ),
            clear_session_info: None,
        }
    }

    pub(crate) fn app_client(&self) -> Arc<AppClient> {
        #[cfg(test)]
        {
            self.client
                .clone()
                .expect("AppClient must be set in TuiState")
        }
        #[cfg(not(test))]
        {
            Arc::clone(&self.client)
        }
    }

    pub(crate) fn app_client_ref(&self) -> Option<&AppClient> {
        #[cfg(test)]
        {
            self.client.as_deref()
        }
        #[cfg(not(test))]
        {
            Some(self.client.as_ref())
        }
    }

    /// Refresh the cached runtime effort override from the server so the status
    /// line stays in sync after `/effort`, the model picker, or the config
    /// picker change it. Best-effort: leaves the cached value untouched on error.
    pub(crate) async fn refresh_status_effort(&mut self, app_server: &AppClient) {
        if let Ok(value) = app_server.effort_level().await {
            self.status.effort = serde_json::from_value(value["effort"].clone())
                .ok()
                .flatten();
        }
    }

    pub(crate) fn update_mcp_slash_suggestions(&mut self, catalog: McpSlashSuggestionCatalog) {
        if self.mcp_slash_suggestions != catalog {
            self.mcp_slash_suggestions = catalog;
            self.mcp_slash_suggestion_revision =
                self.mcp_slash_suggestion_revision.saturating_add(1);
        }
    }

    pub(crate) fn mcp_slash_suggestion_refresh_key(&self) -> Option<String> {
        if self.input_cursor != self.input.len() {
            return None;
        }
        let rest = self.input.strip_prefix('/')?;
        let command_len = rest.find(char::is_whitespace)?;
        let command = &rest[..command_len];
        let raw_args = rest[command_len..].trim_start();
        match command {
            "tool" => Some("tool".to_string()),
            "mcp" => {
                let tokens = raw_args.split_whitespace().collect::<Vec<_>>();
                let subcommand = tokens.first().copied().unwrap_or("");
                if !matches!(subcommand, "resources" | "tools" | "read" | "call") {
                    return None;
                }
                let trailing_space = self.input.ends_with(char::is_whitespace);
                let argument_index = if trailing_space {
                    tokens.len()
                } else {
                    tokens.len().saturating_sub(1)
                };
                if argument_index < 1 {
                    return None;
                }
                let server = if argument_index >= 2 {
                    tokens.get(1).copied().unwrap_or("")
                } else {
                    ""
                };
                Some(format!("mcp {subcommand} {server}"))
            }
            _ => None,
        }
    }

    pub(crate) fn toggle_task_panel(&mut self) {
        let was_visible = self.task_panel.is_visible();
        let now_visible = self.task_panel.toggle();
        let message = if !was_visible && !now_visible {
            "Task panel is empty.".to_string()
        } else if self.task_panel.is_expanded() {
            "Task panel expanded.".to_string()
        } else {
            "Task panel collapsed.".to_string()
        };
        self.set_status_line(message);
    }

    #[cfg(test)]
    pub(crate) fn should_flush_history(&self) -> bool {
        self.pending_history_flush || !self.transcript_ui.emission.pending_lines.is_empty()
    }

    /// Mimics tmux copy-mode: while the user is scrolling up or has an
    /// active text selection, suppress redraws coming from streaming
    /// content so the terminal's own scrollback / mouse selection is
    /// not disrupted by the TUI repainting.
    pub(crate) fn rendering_paused(&self) -> bool {
        self.has_input_selection() || self.has_permission_selection()
    }

    pub(crate) fn cursor_style(&self) -> SetCursorStyle {
        if let Some(style) = overlay_cursor_style(self.overlay.as_ref()) {
            return style;
        }

        match self.editor_mode {
            EditorMode::Normal => SetCursorStyle::SteadyBlock,
            EditorMode::Standard | EditorMode::Insert => SetCursorStyle::BlinkingBar,
        }
    }

    pub(crate) fn update_status_context_percent(&mut self, usage: &orbcode_protocol::TokenUsage) {
        let token_usage = usage.component_total_tokens();
        if token_usage == 0 {
            return;
        }
        let warning_state = calculate_token_warning_state(
            token_usage,
            &self.model_display_name,
            &self.context_window_options,
            &self.max_output_token_options,
            &self.token_warning_options,
        );
        self.status.context_percent_left = Some(warning_state.percent_left);
    }
}
