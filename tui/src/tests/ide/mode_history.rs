use crate::tests::support::*;

#[test]
fn escape_enters_normal_mode_when_idle() {
    let mut state = TuiState {
        client: None,
        session_id: "session".to_string(),
        cwd: PathBuf::from("/Users/user/github/sample-repo"),
        messages: Vec::new(),
        transcript_ui: TranscriptUiState::default(),
        input: "hello".to_string(),
        input_cursor: "hello".len(),
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

    assert_eq!(state.handle_escape_key(false), EscapeAction::StayInTui);
    assert_eq!(state.editor_mode, EditorMode::Normal);
    assert_eq!(state.input_cursor, "hell".len());
}

#[test]
fn escape_is_noop_when_vim_mode_is_disabled() {
    let mut state = normal_state("hello", "hello".len());
    state.editor_mode = EditorMode::Standard;

    assert_eq!(state.handle_escape_key(false), EscapeAction::StayInTui);
    assert_eq!(state.editor_mode, EditorMode::Standard);
    assert_eq!(state.input_cursor, "hello".len());
}

#[test]
fn cursor_style_follows_editor_mode_conventions() {
    let mut state = normal_state("", 0);

    state.editor_mode = EditorMode::Standard;
    assert_eq!(
        state.cursor_style().to_string(),
        SetCursorStyle::BlinkingBar.to_string()
    );

    state.editor_mode = EditorMode::Insert;
    assert_eq!(
        state.cursor_style().to_string(),
        SetCursorStyle::BlinkingBar.to_string()
    );

    state.editor_mode = EditorMode::Normal;
    assert_eq!(
        state.cursor_style().to_string(),
        SetCursorStyle::SteadyBlock.to_string()
    );
}

#[test]
fn normal_mode_j_k_navigate_prompt_history_when_vertical_motion_is_unavailable() {
    let mut state = normal_state("", 0);
    state.prompt_history = vec!["recent".to_string(), "older".to_string()];

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input, "recent");
    assert_eq!(state.prompt_history_index, Some(0));

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input, "older");
    assert_eq!(state.prompt_history_index, Some(1));

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input, "recent");
    assert_eq!(state.prompt_history_index, Some(0));

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert!(state.input.is_empty());
    assert_eq!(state.prompt_history_index, None);
}

#[test]
fn submitted_multiline_prompt_is_immediately_browsable_from_history() {
    let mut state = normal_state("", 0);
    let prompt = "Use Write, Edit, and Read\n\nCreate ambiguous.txt";
    state.prompt_history = vec!["older".to_string()];

    state.remember_prompt_history(prompt);
    assert!(state.navigate_prompt_up());

    assert_eq!(state.input, prompt);
    assert_eq!(state.prompt_history_index, Some(0));
}

#[test]
fn submitted_prompt_history_dedupes_to_most_recent() {
    let mut state = normal_state("", 0);
    state.prompt_history = vec![
        "recent".to_string(),
        "duplicate".to_string(),
        "older".to_string(),
    ];

    state.remember_prompt_history("duplicate");

    assert_eq!(
        state.prompt_history,
        vec![
            "duplicate".to_string(),
            "recent".to_string(),
            "older".to_string()
        ]
    );
}

#[test]
fn normal_mode_j_k_keep_multiline_prompt_navigation_before_history() {
    let mut state = normal_state("one\ntwo", "one\ntwo".len());
    state.prompt_history = vec!["recent".to_string()];

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.input, "one\ntwo");
    assert_eq!(state.prompt_history_index, None);
    assert_eq!(state.input_cursor, "one".len());
}

#[test]
fn standard_mode_up_uses_logical_lines_before_history() {
    let mut state = normal_state("one\ntwo", "one\ntwo".len());
    state.editor_mode = EditorMode::Standard;
    state.prompt_history = vec!["recent".to_string()];

    assert!(state.navigate_prompt_up());
    assert_eq!(state.input, "one\ntwo");
    assert_eq!(state.prompt_history_index, None);
    assert_eq!(state.input_cursor, "one".len());

    assert!(state.navigate_prompt_up());
    assert_eq!(state.input, "recent");
    assert_eq!(state.prompt_history_index, Some(0));
}

#[test]
fn standard_mode_wrapped_single_line_up_browses_history() {
    let mut state = normal_state(&"word ".repeat(80), "word ".repeat(80).len());
    state.editor_mode = EditorMode::Standard;
    state.prompt_history = vec!["recent".to_string()];

    assert!(state.navigate_prompt_up());

    assert_eq!(state.input, "recent");
    assert_eq!(state.prompt_history_index, Some(0));
}

#[test]
fn escape_requests_cancel_when_turn_is_active() {
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

    assert_eq!(state.handle_escape_key(false), EscapeAction::CancelTurn);
    assert_eq!(state.editor_mode, EditorMode::Insert);
}

#[test]
fn normal_mode_i_returns_to_insert_mode() {
    let mut state = TuiState {
        client: None,
        session_id: "session".to_string(),
        cwd: PathBuf::from("/Users/user/github/sample-repo"),
        messages: Vec::new(),
        transcript_ui: TranscriptUiState::default(),
        input: "hello".to_string(),
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

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('i'),
            KeyModifiers::NONE,
        ))
        .unwrap();

    assert_eq!(state.editor_mode, EditorMode::Insert);
}

#[test]
fn normal_mode_h_and_l_move_cursor() {
    let mut state = TuiState {
        client: None,
        session_id: "session".to_string(),
        cwd: PathBuf::from("/Users/user/github/sample-repo"),
        messages: Vec::new(),
        transcript_ui: TranscriptUiState::default(),
        input: "hello".to_string(),
        input_cursor: 3,
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

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input_cursor, 2);

    state
        .handle_normal_mode_key(crossterm::event::KeyEvent::new(
            KeyCode::Char('l'),
            KeyModifiers::NONE,
        ))
        .unwrap();
    assert_eq!(state.input_cursor, 3);
}
