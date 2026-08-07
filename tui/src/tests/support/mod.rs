mod assertions;
mod fixtures;
mod helpers;
mod http_server;
mod render_backend;
mod vt100_backend;

pub(super) use assertions::*;
pub(super) use fixtures::*;
pub(super) use helpers::*;
pub(super) use http_server::*;
pub(super) use render_backend::*;
pub(super) use vt100_backend::*;

pub(super) use std::collections::{HashMap, HashSet};
pub(super) use std::io::{self, Read, Write};
pub(super) use std::net::TcpListener;
pub(super) use std::path::{Path, PathBuf};
pub(super) use std::sync::Arc;
pub(super) use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
pub(super) use std::thread;
pub(super) use std::time::{Duration as StdDuration, Instant};

pub(super) use chrono::{NaiveDate, TimeZone, Utc};
pub(super) use crossterm::{
    cursor::SetCursorStyle,
    event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
};
pub(super) use orbcode_app_server::{
    AppServer, CostSummary, ModelOption, ModelUsage, OutputStyleOption,
    suggested_bash_permission_rules,
};
pub(super) use orbcode_app_server::{
    BudgetOutcome, EffortLevel, MemorySourceStatus, MessageRole, PermissionRequest, ProviderId,
    SessionSummary, StreamErrorCategory, StreamEvent, TokenUsage, TranscriptBlock,
    TranscriptMessage, TurnContext,
};
pub(super) use orbcode_app_server_client::{
    AuthMethod, AuthOverview, AuthStatusEntry, ContextDiagnosticsReport, ContextOverview,
    ContextTokenSource, ContextUsageOverview, ContextWindowOptions, CostOverview, DoctorCheck,
    DoctorReport, DoctorStatus, MaxOutputTokenOptions, McpResourceSlashSuggestion,
    McpServerSlashSuggestion, McpSlashSuggestionCatalog, McpToolSlashSuggestion,
    MemoryFileOverview, MemoryOverview, PermissionOverview,
    PermissionRuleKind as PermissionRuleSettingKind, PermissionRuleOverview,
    ProviderRequestDebugSnapshot, SandboxFilesystemLocalSettings, SandboxLocalSettings,
    SandboxNetworkLocalSettings, StatsActivityDay, StatsOverview, StatusAuthOverview,
    StatusOverview, ThemeSetting, TokenWarningOptions, UsageOverview, WorkspaceDiff,
};
pub(super) use orbcode_protocol::{MemorySource, MemorySourceKind};
pub(super) use ratatui::{
    backend::{Backend, ClearType, WindowSize},
    prelude::*,
};
pub(super) use serde_json::Value;
pub(super) use tokio::sync::mpsc;
pub(super) use tokio::time::Duration;

pub(super) use crate::background_agent_panel::BackgroundAgentPanelState;
pub(super) use crate::bottom_pane::slash_suggestions::SlashSuggestionLinesCache;
pub(super) use crate::bottom_pane::{completion::*, input_layout::*, mode::EscapeAction, vim::*};
pub(super) use crate::chat::stream_events::{
    INTERRUPTED_TOOL_RESULT, detach_turn_event_stream, handle_stream_event_batch,
};
pub(super) use crate::clipboard::{
    is_transcript_copy_shortcut, take_test_clipboard_capture, test_clipboard_assertion_lock,
    transcript_copy_shortcut_label,
};
pub(super) use crate::commands::async_local::{
    LocalCommandEnvelope, LocalCommandEvent, apply_local_command_envelope_for_redraw,
    apply_local_command_event_for_redraw,
};
pub(super) use crate::commands::compact::compact_restored_file_detail_lines;
pub(super) use crate::commands::permissions::{PERMISSIONS_USAGE, PermissionRuleScope};
pub(super) use crate::commands::release_notes::CHANGELOG_URL;
pub(super) use crate::custom_terminal::Terminal;
pub(super) use crate::editor_mode::EditorMode;
pub(super) use crate::embedded_progress::embedded_progress_message_to_transcript;
pub(super) use crate::external_editor::{
    EditorLaunchInfo, ExternalEditorRequest, ExternalEditorTarget,
};
pub(super) use crate::history_cell::agent_activity::render_agent_activity_group_cell_lines;
pub(super) use crate::history_cell::cells::{
    ORPHANED_TOOL_RESULT, TranscriptCell, build_collapsible_tool_cells_from_message,
    build_tool_cell, transcript_cells_from_messages,
};
pub(super) use crate::history_cell::collapsed_activity::{
    CollapsedActivityGroup, build_collapsed_activity_group, collapsed_activity_summary_text,
    render_collapsed_activity_group_cell_lines, render_collapsed_activity_group_lines,
};
pub(super) use crate::history_cell::local_note::{
    LOCAL_TURN_DURATION_PREFIX, encode_slash_command_output_note, local_context_compacted_message,
    local_slash_command_output_message, parse_local_transcript_note,
};
pub(super) use crate::history_cell::state::{
    TranscriptUiState, flatten_transcript_cells, history_lines_for_cell_range,
};
pub(super) use crate::history_cell::viewport::{
    TranscriptSelectionPoint, TranscriptSelectionState, TranscriptViewportState,
    visible_transcript_lines,
};
pub(super) use crate::overlays::*;
pub(super) use crate::prompt_state::{ActiveThinkingState, NormalPending};
pub(super) use crate::render::active_transcript::render_compacting_lines;
pub(super) use crate::render::assistant::{
    render_assistant_markdown_lines, render_pending_assistant_lines,
};
pub(super) use crate::render::bash_warnings::destructive_bash_command_warning;
pub(super) use crate::render::footer::FOOTER_STATUS_TIMEOUT_MS;
pub(super) use crate::render::local_note::render_local_transcript_note_lines;
pub(super) use crate::render::message::{render_message_lines, render_text_block_lines};
pub(super) use crate::render::permission_labels::friendly_bash_permission_rule_label;
pub(super) use crate::render::slash::render_recent_activity_detail_lines;
pub(super) use crate::render::slash_output::{
    LAST_REQUEST_BODY_PREVIEW_CHARS, render_agent_definitions, render_auth_overview,
    render_context_overview, render_cost_overview, render_doctor_report, render_hook_discovery,
    render_last_provider_request_snapshot, render_memory_overview, render_permission_overview,
    render_provider_request_body_section, render_recent_activity_trace, render_skill_definitions,
    render_stats_overview, render_stats_summary, render_status_overview, render_turn_context,
    render_usage_overview, render_workspace_diff, workspace_diff_changed_path_count,
};
pub(super) use crate::render::styled_wrap::{
    TRANSCRIPT_RIGHT_PADDING, transcript_layout_constraint, wrap_styled_lines,
};
pub(super) use crate::render::text_utils::{
    StyledLine, display_width, display_width_str, styled_line_display_width, truncate_path_tail,
};
pub(super) use crate::render::thinking::{
    THINKING_RETENTION_MS, last_visible_thinking_block, render_active_thinking_lines,
};
pub(super) use crate::render::user::render_user_message_lines;
pub(super) use crate::render_metrics::RenderEventCounts;
pub(super) use crate::slash_commands::{
    AsyncLocalSlashCommand, LocalOutputSlashCommand, SLASH_COMMAND_VISIBLE_ROWS,
    SlashCommandExecution, TuiLocalSlashCommand, async_local_slash_command,
    canonicalize_slash_command_line, exact_slash_command, local_output_slash_command,
    record_slash_command_use, render_slash_command_help, slash_command_invocation,
    slash_command_scrollbar_active, slash_command_suggestion_view, slash_command_suggestions,
    slash_commands,
};
pub(super) use crate::state::{RequestTokenDirection, StatusLineState, TuiState};
pub(super) use crate::task_panel::{TaskPanelState, task_tool_changes_panel};
pub(super) use crate::tool_cell::live_state::{
    LIVE_TOOL_PROGRESS_MESSAGE_LIMIT, LiveToolActivity, LiveToolCells,
};
pub(super) use crate::tool_cell::render::{
    black_circle_glyph, render_live_tool_activity_lines, render_tool_cell_lines,
};
pub(super) use crate::tool_cell::summary::{
    BASH_EXPANDED_OUTPUT_DETAIL_LIMIT, format_tool_activity_title, format_tool_result_summary,
    tool_activity_detail_lines, tool_activity_progress_messages,
};
pub(super) use crate::tool_cell::{ToolCell, ToolResultIndex};
pub(super) use crate::transcript_task_cards::TranscriptTaskCardsState;
pub(super) use crate::tui_runtime::terminal_session::{
    HistoryLineWrapPolicy, TranscriptPagerTerminalMode, flush_pending_history_to_scrollback,
    initial_top_viewport_area, insert_history_lines, insert_history_lines_with_wrap_policy,
    prepare_draw_transaction, prepare_terminal_for_cli_output, resized_inline_viewport,
    sync_transcript_pager_terminal_mode, update_inline_viewport_generic,
};
pub(super) use crate::tui_theme::stats_heatmap_color;
pub(super) use crate::tui_theme::{
    CLAUDE_ORANGE, DIFF_ADDED_BG, DIFF_REMOVED_BG, TOOL_BLUE, USER_BAR_BG, active_palette,
    empty_transcript_placeholder_style, inactive_style, palette_for_theme, subtle_style,
};

pub(super) use crate::*;

pub(super) const MAX_INPUT_INNER_HEIGHT: usize = 5;

impl TuiState {
    pub(crate) fn stable_transcript_cells(
        &mut self,
        transcript_width: usize,
    ) -> Vec<Vec<StyledLine>> {
        self.refresh_transcript_ui_state();
        let mut cells = Vec::new();
        let banner = self.intro_banner_cell(transcript_width);
        if !banner.is_empty() {
            cells.push(banner);
        }
        cells.extend(self.render_committed_transcript_ui_cells(transcript_width));
        cells
    }

    pub(super) fn render_committed_transcript_ui_cells(
        &self,
        transcript_width: usize,
    ) -> Vec<Vec<StyledLine>> {
        let last_thinking_block = if self.expanded_tool_details {
            last_visible_thinking_block(&self.messages)
        } else {
            None
        };
        self.transcript_ui
            .cells
            .iter()
            .filter_map(|cell| {
                let rendered = render_transcript_cell_lines(
                    cell,
                    &self.cwd,
                    self.expanded_tool_details,
                    last_thinking_block.as_ref(),
                    transcript_width,
                    &self.model_display_name,
                );
                (!rendered.is_empty()).then_some(rendered)
            })
            .collect()
    }

    pub(crate) fn transcript_lines(&mut self, transcript_width: usize) -> Vec<StyledLine> {
        self.transcript_lines_for_messages(
            transcript_width,
            self.history_flushed_message_count == 0,
        )
    }
}

pub(super) fn render_committed_transcript_cells(
    messages: &[TranscriptMessage],
    cwd: &Path,
    expanded_tool_details: bool,
    transcript_width: usize,
    model_display_name: &str,
) -> Vec<Vec<StyledLine>> {
    let last_thinking_block = if expanded_tool_details {
        last_visible_thinking_block(messages)
    } else {
        None
    };
    transcript_cells_from_messages(messages, cwd)
        .iter()
        .filter_map(|cell| {
            let rendered = render_transcript_cell_lines(
                cell,
                cwd,
                expanded_tool_details,
                last_thinking_block.as_ref(),
                transcript_width,
                model_display_name,
            );
            (!rendered.is_empty()).then_some(rendered)
        })
        .collect()
}

pub(super) fn render_transcript_cell_lines(
    cell: &TranscriptCell,
    cwd: &Path,
    expanded_tool_details: bool,
    last_thinking_block: Option<&(String, usize)>,
    transcript_width: usize,
    model_display_name: &str,
) -> Vec<StyledLine> {
    match cell {
        TranscriptCell::AgentGroup(group) => render_agent_activity_group_cell_lines(
            &group.agents,
            expanded_tool_details,
            false,
            true,
            cwd,
            transcript_width,
        ),
        TranscriptCell::ActivityGroup(group) => render_collapsed_activity_group_cell_lines(
            group,
            expanded_tool_details,
            false,
            true,
            cwd,
            transcript_width,
            model_display_name,
            last_thinking_block,
        ),
        TranscriptCell::Tool(card) => {
            render_tool_cell_lines(card, expanded_tool_details, None, transcript_width, cwd)
        }
        TranscriptCell::Message(message) => render_message_lines(
            message,
            cwd,
            expanded_tool_details,
            last_thinking_block,
            transcript_width,
            model_display_name,
            false,
        ),
    }
}

pub(super) fn normal_state(input: &str, cursor: usize) -> TuiState {
    TuiState {
        client: None,
        session_id: "session".to_string(),
        cwd: PathBuf::from("/Users/user/github/sample-repo"),
        messages: Vec::new(),
        transcript_ui: TranscriptUiState::default(),
        input: input.to_string(),
        input_cursor: cursor,
        input_tail_pinned: false,
        input_area: Rect::ZERO,
        input_selection: None,
        desired_column: None,
        prompt_history: Vec::new(),
        prompt_history_index: None,
        slash_command_selected: 0,
        steered_followups: std::collections::VecDeque::new(),
        queued_followups: std::collections::VecDeque::new(),
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
        status_line: String::new(),
        status_line_set_at: None,
        ui_version: "2.1.888".to_string(),
        cwd_display: "~".to_string(),
        model_display_name: "model".to_string(),
        context_window_options: ContextWindowOptions::default(),
        max_output_token_options: MaxOutputTokenOptions::default(),
        token_warning_options: TokenWarningOptions::default(),
        default_provider_label: "anthropic".to_string(),
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
        editor_mode: EditorMode::Normal,
        normal_pending: None,
        last_find: None,
        normal_count: None,
        vim_state: VimRuntimeState::default(),
        external_editor_request: None,
        slash_suggestion_lines_cache: SlashSuggestionLinesCache::default(),
        mcp_slash_suggestions: McpSlashSuggestionCatalog::default(),
        mcp_slash_suggestion_revision: 0,
        mcp_slash_suggestion_refresh_key: None,
        task_panel: TaskPanelState::new(Some("test-session"), true),
        background_agent_panel: BackgroundAgentPanelState::new(),
        transcript_task_cards: TranscriptTaskCardsState::new(),
        status: StatusLineState::default(),
        statusline_command: None,
        statusline_refresh_interval: std::time::Duration::from_secs(30),
        clear_session_info: None,
    }
}
