use super::support::*;

#[test]
fn footer_shows_non_error_status_messages() {
    let state = TuiState {
        client: None,
        session_id: "session".to_string(),
        cwd: PathBuf::from("/Users/user/github/sample-repo"),
        messages: Vec::new(),
        transcript_ui: TranscriptUiState::default(),
        input: String::new(),
        input_cursor: 0,
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
        status_line: "Session abc123 resumed.".to_string(),
        status_line_set_at: Some(Instant::now()),
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
        editor_mode: EditorMode::Insert,
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
    };

    assert_eq!(state.transient_footer_status(), "Session abc123 resumed.");
}

#[test]
fn streaming_events_do_not_set_status_line() {
    let mut state = normal_state("", 0);
    state.apply_stream_event(StreamEvent::RequestStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_provider: None,
        context: TurnContext::default(),
    });
    assert!(
        state.status_line.is_empty(),
        "RequestStarted should not set status_line: {:?}",
        state.status_line
    );
    state.apply_stream_event(StreamEvent::ThinkingStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
    });
    assert!(
        state.status_line.is_empty(),
        "ThinkingStarted should not set status_line: {:?}",
        state.status_line
    );
}

#[test]
fn footer_right_text_hides_live_request_progress() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.spinner_verb_index = 0;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(34_000));
    state.streamed_response_chars = 1_932;

    let text = state.footer_right_text();
    assert!(
        !text.contains("streaming") && !text.contains("progress"),
        "footer should not show transient request progress, got: {text}"
    );
    assert!(
        text.contains("model"),
        "footer should show status bar with model when no transient status: {text}"
    );
}

#[test]
fn footer_right_line_hides_live_request_progress() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.spinner_verb_index = 0;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(5_000));

    let text = plain_text_line(&state.footer_right_line());
    assert!(
        !text.contains("streaming") && !text.contains("progress"),
        "footer line should not show transient request progress, got: {text}"
    );
    assert!(
        text.contains("model"),
        "footer line should show status bar when no transient status: {text}"
    );
}

// ── Status bar rendering tests ──

// ── Status bar rendering tests ──

#[test]
fn status_bar_shows_model_name() {
    let state = normal_state("", 0);
    let text = state.footer_right_text();
    assert!(
        text.contains("model"),
        "status bar should show model name: {text}"
    );
}

#[test]
fn status_bar_shows_full_model_name_without_truncation() {
    let mut state = normal_state("", 0);
    state.model_display_name = "Peach-07-17-DogFooding-experimental".to_string();
    let text = state.footer_right_text();
    assert!(
        text.contains("Peach-07-17-DogFooding-experimental"),
        "status bar should show the full model name without an ellipsis: {text}"
    );
    assert!(
        !text.contains("..."),
        "status bar should not truncate long model names: {text}"
    );
}

#[test]
fn status_bar_cwd_shares_model_style_with_dimmed_separator() {
    let state = normal_state("", 0);
    let line = state.footer_right_line();
    // spans: [model, " · ", cwd, " · ", permission mode]
    assert_eq!(
        line.spans[0].style, line.spans[2].style,
        "cwd should use the same style as the model name"
    );
    assert_eq!(
        line.spans[0].style,
        inactive_style(),
        "model and cwd should use the inactive (non-dimmed) style"
    );
    assert_eq!(
        line.spans[1].style,
        subtle_style(),
        "the separator should be dimmed"
    );
    assert_eq!(line.spans[1].content.as_ref(), " \u{b7} ");
    assert_eq!(line.spans[4].style, inactive_style());
}

#[test]
fn status_bar_shows_working_directory() {
    let mut state = normal_state("", 0);
    state.cwd_display = "~/github/orbcode".to_string();
    let text = state.footer_right_text();
    assert_eq!(
        text, "model \u{b7} ~/github/orbcode \u{b7} Ask for approval",
        "default status bar should include model, cwd, and permission mode: {text}"
    );
}

#[test]
fn status_bar_shows_effort_next_to_model() {
    let mut state = normal_state("", 0);
    state.status.effort = Some(EffortLevel::High);
    let text = state.footer_right_text();
    assert!(
        text.contains("model high"),
        "status bar should show effort next to the model name: {text}"
    );
}

#[test]
fn status_bar_hides_effort_when_default() {
    let mut state = normal_state("", 0);
    state.status.effort = None;
    let text = state.footer_right_text();
    assert!(
        text.starts_with("model \u{b7}"),
        "status bar should omit effort when using the model default: {text}"
    );
}

#[test]
fn status_bar_hides_context_percent_when_ample() {
    let mut state = normal_state("", 0);
    // 90% left → well above the warning threshold, so nothing is shown.
    state.status.context_percent_left = Some(90);
    let text = state.footer_right_text();
    assert!(
        !text.contains("ctx:"),
        "status bar should hide context percent when context is ample: {text}"
    );
}

#[test]
fn status_bar_shows_context_percent_when_low() {
    let mut state = normal_state("", 0);
    // 20% left → below the 25% warning threshold, so it surfaces as a warning.
    state.status.context_percent_left = Some(20);
    let text = state.footer_right_text();
    assert!(
        text.contains("ctx:80%"),
        "status bar should show used% as a warning when context runs low: {text}"
    );
}

#[test]
fn status_bar_shows_rate_limit_warning() {
    let mut state = normal_state("", 0);
    state.status.has_rate_limit_warning = true;
    let text = state.footer_right_text();
    assert!(
        text.contains("rate-limit"),
        "status bar should show rate-limit warning: {text}"
    );
}

#[test]
fn status_bar_shows_auth_warning() {
    let mut state = normal_state("", 0);
    state.status.has_auth_warning = true;
    let text = state.footer_right_text();
    assert!(
        text.contains("auth-err"),
        "status bar should show auth error warning: {text}"
    );
}

// ── Error category warning flag tests ──

// ── Context % accuracy tests ──

#[test]
fn context_percent_matches_calculate_token_warning_state() {
    let mut state = normal_state("", 0);
    state.model_display_name = "claude-sonnet-4-20250514".to_string();

    let usage = TokenUsage {
        input_tokens: 80_000,
        total_tokens: 80_000,
        ..TokenUsage::default()
    };
    state.update_status_context_percent(&usage);

    let expected = crate::state::calculate_token_warning_state_from_protocol(
        usage.component_total_tokens(),
        &state.model_display_name,
        &state.context_window_options,
        &state.max_output_token_options,
        &state.token_warning_options,
    );

    assert_eq!(
        state.status.context_percent_left,
        Some(expected.percent_left),
        "status line percent_left must match calculate_token_warning_state"
    );
}

#[test]
fn context_percent_zero_usage_is_skipped() {
    let mut state = normal_state("", 0);
    let usage = TokenUsage::default();
    state.update_status_context_percent(&usage);

    assert_eq!(
        state.status.context_percent_left, None,
        "zero token usage should not set context_percent_left"
    );
}

#[test]
fn context_percent_updated_on_turn_finished() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.model_display_name = "claude-sonnet-4-20250514".to_string();

    let usage = TokenUsage {
        input_tokens: 120_000,
        total_tokens: 120_000,
        ..TokenUsage::default()
    };
    state.apply_stream_event(StreamEvent::TurnFinished {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage,
    });

    assert!(
        state.status.context_percent_left.is_some(),
        "TurnFinished should update context_percent_left"
    );
}

#[test]
fn status_bar_full_format_orders_model_cwd_mode_then_warnings() {
    let mut state = normal_state("", 0);
    state.model_display_name = "Claude Sonnet 4".to_string();
    state.cwd_display = "~/proj".to_string();
    state.status.effort = Some(EffortLevel::Max);
    state.status.context_percent_left = Some(5);
    state.status.has_rate_limit_warning = true;
    state.status.has_auth_warning = true;

    let text = state.footer_right_text();
    assert_eq!(
        text,
        "Sonnet 4 max \u{b7} ~/proj \u{b7} Ask for approval \u{b7} ctx:95% \u{b7} rate-limit \u{b7} auth-err",
        "status bar should order model+effort, cwd, permission mode, then active warnings: {text}"
    );
}

#[test]
fn footer_left_line_is_empty_by_default() {
    let state = TuiState {
        client: None,
        session_id: "session".to_string(),
        cwd: PathBuf::from("/Users/user/github/sample-repo"),
        messages: Vec::new(),
        transcript_ui: TranscriptUiState::default(),
        input: String::new(),
        input_cursor: 0,
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
        editor_mode: EditorMode::Insert,
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
    };

    let rendered = state
        .footer_left_line()
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(
        rendered, "",
        "footer left line should be empty when idle (no vim mode indicator)"
    );
}

#[test]
fn footer_left_line_shows_interrupt_hint_while_request_is_active() {
    let state = TuiState {
        client: None,
        session_id: "session".to_string(),
        cwd: PathBuf::from("/Users/user/github/sample-repo"),
        messages: Vec::new(),
        transcript_ui: TranscriptUiState::default(),
        input: String::new(),
        input_cursor: 0,
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
        request_in_flight: true,
        spinner_frame: 0,
        spinner_verb_index: 0,
        request_count: 0,
        request_started_at: None,
        streamed_response_chars: 0,
        request_token_direction: RequestTokenDirection::Up,
        current_turn_total_tokens: 0,
        last_provider: None,
        last_usage: None,
        editor_mode: EditorMode::Insert,
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
    };

    let rendered = state
        .footer_left_line()
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(rendered, "esc to interrupt");
}

#[test]
fn footer_left_line_shows_followup_hint_while_typing_during_request() {
    let mut state = normal_state("测试代码有多少", "测试代码有多少".len());
    state.request_in_flight = true;
    state.editor_mode = EditorMode::Insert;

    let rendered = state
        .footer_left_line()
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(rendered, "tab to queue message");
}

#[test]
fn non_error_footer_status_expires_after_timeout() {
    let mut state = TuiState {
        client: None,
        session_id: "session".to_string(),
        cwd: PathBuf::from("/Users/user/github/sample-repo"),
        messages: Vec::new(),
        transcript_ui: TranscriptUiState::default(),
        input: String::new(),
        input_cursor: 0,
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
        status_line: "Session abc123 resumed.".to_string(),
        status_line_set_at: Some(
            Instant::now() - std::time::Duration::from_millis(FOOTER_STATUS_TIMEOUT_MS + 5),
        ),
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
        editor_mode: EditorMode::Insert,
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
    };

    state.on_tick();

    assert_eq!(state.status_line, "");
    assert!(state.status_line_set_at.is_none());
}

#[test]
fn footer_left_line_hides_vim_normal_mode() {
    let state = TuiState {
        client: None,
        session_id: "session".to_string(),
        cwd: PathBuf::from("/Users/user/github/sample-repo"),
        messages: Vec::new(),
        transcript_ui: TranscriptUiState::default(),
        input: String::new(),
        input_cursor: 0,
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
    };

    let rendered = state
        .footer_left_line()
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(
        rendered, "",
        "vim NORMAL mode should no longer render a mode indicator"
    );
}

// ── Custom command output tests ──

#[test]
fn status_bar_shows_custom_command_output() {
    let mut state = normal_state("", 0);
    state.status.custom_command_output = Some("abc1234".to_string());
    let text = state.footer_right_text();
    assert!(
        text.contains("abc1234"),
        "status bar should show custom command output: {text}"
    );
}

#[test]
fn status_bar_hides_custom_command_when_none() {
    let state = normal_state("", 0);
    let text = state.footer_right_text();
    assert!(
        !text.contains("abc1234"),
        "status bar should not show custom command output when none: {text}"
    );
}

#[test]
fn status_bar_default_shows_model_cwd_and_permission_mode() {
    let state = normal_state("", 0);
    let text = state.footer_right_text();
    assert_eq!(
        text, "model \u{b7} ~ \u{b7} Ask for approval",
        "status bar should show model, cwd, and permission mode: {text}"
    );
}

#[test]
fn status_bar_tracks_each_interactive_permission_mode_as_third_field() {
    let mut state = normal_state("", 0);
    for mode in [
        InteractivePermissionMode::AskForApproval,
        InteractivePermissionMode::ApproveForMe,
        InteractivePermissionMode::FullAccess,
        InteractivePermissionMode::Plan,
    ] {
        state.status.permission_mode = mode;
        assert_eq!(
            state.footer_right_text(),
            format!("model \u{b7} ~ \u{b7} {}", mode.label())
        );
    }
}

#[test]
fn status_bar_custom_command_appears_in_styled_line() {
    let mut state = normal_state("", 0);
    state.status.custom_command_output = Some("v1.2.3".to_string());
    let text = plain_text_line(&state.footer_right_line());
    assert!(
        text.contains("v1.2.3"),
        "styled footer line should contain custom command output: {text}"
    );
}
