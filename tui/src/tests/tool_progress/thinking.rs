use crate::tests::support::*;

#[test]
fn pending_assistant_stream_keeps_live_tool_activity_visible() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = "Working through the remaining files.".to_string();
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-1".to_string(),
        tool_name: "glob".to_string(),
        tool_input: "{\"pattern\":\"orbcode/**/*.rs\"}".to_string(),
        status_line: "Running `glob`".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });
    state.in_progress_tool_use_ids.insert("tool-1".to_string());

    let rendered = state
        .transcript_lines(90)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line.contains("glob")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Working through the remaining files."))
    );
}

#[test]
fn pending_assistant_stream_hides_completed_active_thinking() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant =
        "工具返回的内容是空字符串。\n\n因为 start_line=3 大于 end_line=2。".to_string();
    state.active_thinking = Some(ActiveThinkingState {
        text: "check whether the reversed range is empty".to_string(),
        is_streaming: false,
        completed_at: Some(Instant::now()),
    });

    let rendered = plain_text_lines(&state.transcript_lines(90));

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("工具返回的内容是空字符串")),
        "{rendered:#?}"
    );
    assert!(
        !rendered.iter().any(|line| line.contains("∴ Thinking")),
        "{rendered:#?}"
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("check whether the reversed range is empty")),
        "{rendered:#?}"
    );
    assert!(state.active_thinking.is_some());
}

#[test]
fn assistant_message_with_tool_use_keeps_request_active() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();

    let finished = state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "glob".to_string(),
                input: "{\"pattern\":\"orbcode/**/*.rs\"}".to_string(),
            }],
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    assert!(!finished);
    assert!(state.request_in_flight);
    assert!(state.needs_periodic_tick());
}

#[test]
fn assistant_message_without_tool_use_stops_request_animation() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();

    let finished = state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, "done"),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    assert!(!finished);
    assert!(!state.request_in_flight);
}

#[test]
fn assistant_message_completion_clears_active_thinking() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();
    state.active_thinking = Some(ActiveThinkingState {
        text: "draft thinking".to_string(),
        is_streaming: false,
        completed_at: Some(Instant::now()),
    });

    let finished = state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, "done"),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    assert!(!finished);
    assert!(state.active_thinking.is_none());
}

#[test]
fn assistant_message_completion_commits_completed_thinking_before_final_answer() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();
    state.active_thinking = Some(ActiveThinkingState {
        text: "inspect the repo first".to_string(),
        is_streaming: false,
        completed_at: Some(Instant::now()),
    });

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, "final answer"),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    let rendered = plain_text_lines(&state.transcript_lines(80));
    let thinking_index = rendered
        .iter()
        .position(|line| line.contains("∴ Thinking"))
        .expect("collapsed thinking step should remain in transcript");
    let answer_index = rendered
        .iter()
        .position(|line| line.contains("final answer"))
        .expect("final answer should remain in transcript");

    assert!(thinking_index < answer_index);
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("inspect the repo first"))
    );
}

#[test]
fn assistant_message_completion_deduplicates_streamed_thinking_from_completed_message() {
    let mut state = normal_state("", 0);
    state.expanded_tool_details = true;
    state.begin_waiting_animation();
    state.active_thinking = Some(ActiveThinkingState {
        text: "inspect the repo first".to_string(),
        is_streaming: false,
        completed_at: Some(Instant::now()),
    });

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::Thinking {
                    text: "inspect the repo first".to_string(),
                    signature: None,
                },
                TranscriptBlock::Text {
                    text: "final answer".to_string(),
                },
            ],
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    let rendered = plain_text_lines(&state.transcript_lines(80));
    let thinking_heading_count = rendered
        .iter()
        .filter(|line| line.contains("∴ Thinking"))
        .count();
    let thinking_body_count = rendered
        .iter()
        .filter(|line| line.contains("inspect the repo first"))
        .count();

    assert_eq!(thinking_heading_count, 1, "{rendered:#?}");
    assert_eq!(thinking_body_count, 1, "{rendered:#?}");
    assert!(
        rendered.iter().any(|line| line.contains("final answer")),
        "{rendered:#?}"
    );
}

#[test]
fn thinking_only_completion_commits_one_thinking_block() {
    let mut state = normal_state("", 0);
    state.expanded_tool_details = true;
    state.begin_waiting_animation();
    state.active_thinking = Some(ActiveThinkingState {
        text: "thinking-only marker".to_string(),
        is_streaming: false,
        completed_at: Some(Instant::now()),
    });

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::Thinking {
                text: "thinking-only marker".to_string(),
                signature: None,
            }],
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    let rendered = plain_text_lines(&state.transcript_lines(80));
    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains("thinking-only marker"))
            .count(),
        1,
        "{rendered:#?}"
    );
}

#[test]
fn transcript_lines_keep_active_tool_step_visible_during_tool_execution() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.spinner_verb_index = 3;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(5_000));
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "glob".to_string(),
            input: "{\"pattern\":\"orbcode/**/*.rs\"}".to_string(),
        }],
    ));
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-1".to_string(),
        tool_name: "glob".to_string(),
        tool_input: "{\"pattern\":\"orbcode/**/*.rs\"}".to_string(),
        status_line: "Running `glob`".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });

    let rendered = state
        .transcript_lines(80)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Searching for 1 pattern...(ctrl+o to expand)"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("\"orbcode/**/*.rs\""))
    );
    assert!(
        plain_text_lines(&state.request_status_lines())
            .iter()
            .any(|line| line.contains("Running glob...(5s · "))
    );
}

#[test]
fn committed_tool_step_stays_visible_with_trailing_messages_when_tool_is_in_progress() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.spinner_frame = 0;
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "tool-search".to_string(),
            name: "glob".to_string(),
            input: "{\"pattern\":\"orbcode/**/*.rs\"}".to_string(),
        }],
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::System,
        "anthropic: provider returned an error",
    ));
    state
        .in_progress_tool_use_ids
        .insert("tool-search".to_string());

    let rendered = state
        .transcript_lines(80)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Searching for 1 pattern...(ctrl+o to expand)"))
    );
}

#[test]
fn collapsed_active_thinking_renders_expand_hint() {
    let rendered = render_active_thinking_lines(
        &ActiveThinkingState {
            text: "draft reply".to_string(),
            is_streaming: true,
            completed_at: None,
        },
        false,
        '✽',
        "Pontificating",
        120,
    );
    let line = rendered[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(line, "✽ Pontificating… (thinking)");
    let preview = rendered[1]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(preview, "  └ draft reply");
}

#[test]
fn expanded_active_thinking_reuses_thinking_block_layout() {
    let rendered = render_active_thinking_lines(
        &ActiveThinkingState {
            text: "first line\nsecond line".to_string(),
            is_streaming: false,
            completed_at: Some(Instant::now()),
        },
        true,
        '✽',
        "Pontificating",
        120,
    );
    let lines = rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert_eq!(lines[0], "∴ Thinking...");
    assert_eq!(lines[1], "  first line");
    assert_eq!(lines[2], "  second line");
}

#[test]
fn active_thinking_visibility_expires_after_retention_window() {
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
        active_thinking: Some(ActiveThinkingState {
            text: "done".to_string(),
            is_streaming: false,
            completed_at: Some(
                Instant::now() - std::time::Duration::from_millis(THINKING_RETENTION_MS + 5),
            ),
        }),
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

    assert!(state.active_thinking.is_none());
}

// ── Error category warning flag tests ──

#[test]
fn error_event_rate_limit_sets_warning_flag() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;

    state.apply_stream_event(StreamEvent::Error {
        session_id: Some("session".to_string()),
        provider: Some(ProviderId::Anthropic),
        category: Some(StreamErrorCategory::RateLimit),
        message: "rate limited".to_string(),
        suggestion: None,
    });

    assert!(
        state.status.has_rate_limit_warning,
        "rate limit error should set has_rate_limit_warning"
    );
    assert!(
        !state.status.has_auth_warning,
        "rate limit error should not set has_auth_warning"
    );
}

#[test]
fn error_event_overload_sets_rate_limit_warning() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;

    state.apply_stream_event(StreamEvent::Error {
        session_id: Some("session".to_string()),
        provider: Some(ProviderId::Anthropic),
        category: Some(StreamErrorCategory::Overload),
        message: "overloaded".to_string(),
        suggestion: None,
    });

    assert!(
        state.status.has_rate_limit_warning,
        "overload error should set has_rate_limit_warning"
    );
}

#[test]
fn error_event_auth_sets_auth_warning() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;

    state.apply_stream_event(StreamEvent::Error {
        session_id: Some("session".to_string()),
        provider: Some(ProviderId::Anthropic),
        category: Some(StreamErrorCategory::Auth),
        message: "unauthorized".to_string(),
        suggestion: None,
    });

    assert!(
        state.status.has_auth_warning,
        "auth error should set has_auth_warning"
    );
    assert!(
        !state.status.has_rate_limit_warning,
        "auth error should not set has_rate_limit_warning"
    );
}

#[test]
fn error_event_other_category_does_not_set_warnings() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;

    state.apply_stream_event(StreamEvent::Error {
        session_id: Some("session".to_string()),
        provider: None,
        category: Some(StreamErrorCategory::Network),
        message: "network error".to_string(),
        suggestion: None,
    });

    assert!(!state.status.has_rate_limit_warning);
    assert!(!state.status.has_auth_warning);
}

#[test]
fn turn_finished_clears_warning_flags() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.status.has_rate_limit_warning = true;
    state.status.has_auth_warning = true;

    let usage = TokenUsage {
        input_tokens: 1000,
        total_tokens: 1000,
        ..TokenUsage::default()
    };
    state.apply_stream_event(StreamEvent::TurnFinished {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage,
    });

    assert!(
        !state.status.has_rate_limit_warning,
        "TurnFinished should clear rate_limit warning"
    );
    assert!(
        !state.status.has_auth_warning,
        "TurnFinished should clear auth warning"
    );
}

// ── Context % accuracy tests ──
