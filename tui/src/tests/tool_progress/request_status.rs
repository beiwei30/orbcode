use crate::tests::support::*;

#[test]
fn context_percent_updated_on_assistant_message_completed() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.model_display_name = "claude-sonnet-4-20250514".to_string();

    let usage = TokenUsage {
        input_tokens: 30_000,
        total_tokens: 30_000,
        ..TokenUsage::default()
    };
    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, "hello".to_string()),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage,
    });

    assert!(
        state.status.context_percent_left.is_some(),
        "AssistantMessageCompleted should update context_percent_left"
    );
}

#[test]
fn request_status_row_renders_live_request_progress() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.spinner_verb_index = 0;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(5_000));

    let rendered = plain_text_lines(&state.request_status_lines());

    assert_eq!(
        rendered,
        vec!["· Combobulating...(5s · ↑ 0 tokens)".to_string()]
    );
}

#[test]
fn request_status_row_uses_in_progress_task_active_form() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(5_000));
    state.task_panel.clear_awaiting_session_activity();
    state.task_panel.apply_snapshot(
        orbcode_tools::TaskListSnapshot {
            task_list_id: "test".to_string(),
            directory: PathBuf::from("/tmp/orbcode-task-status-test"),
            summary: orbcode_tools::TaskListSummary::default(),
            tasks: vec![],
            fingerprint: 0,
        },
        Instant::now(),
    );
    state.task_panel.apply_snapshot(
        orbcode_tools::TaskListSnapshot {
            task_list_id: "test".to_string(),
            directory: PathBuf::from("/tmp/orbcode-task-status-test"),
            summary: orbcode_tools::TaskListSummary {
                total: 1,
                completed: 0,
                in_progress: 1,
                pending: 0,
            },
            tasks: vec![orbcode_tools::TaskView {
                id: "1".to_string(),
                subject: "Verify acceptance criteria".to_string(),
                description: String::new(),
                active_form: Some("Verifying acceptance criteria".to_string()),
                owner: None,
                status: orbcode_tools::TaskStatusKind::InProgress,
                blocks: Vec::new(),
                blocked_by: Vec::new(),
                open_blockers: Vec::new(),
            }],
            fingerprint: 0,
        },
        Instant::now(),
    );

    let rendered = plain_text_lines(&state.request_status_lines());

    assert_eq!(
        rendered[0],
        "· Verifying acceptance criteria… (5s · ↑ 0 tokens)"
    );
    assert_eq!(rendered[1], "  └ ◼ Verify acceptance criteria");
}

#[test]
fn request_status_row_renders_compact_down_token_progress() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.spinner_verb_index = 0;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(280_000));
    state.streamed_response_chars = 28_400;
    state.request_token_direction = RequestTokenDirection::Down;

    let rendered = plain_text_lines(&state.request_status_lines()).join("\n");

    assert!(
        rendered.contains("Combobulating...(4m 40s · ↓ 7.1k tokens)"),
        "{rendered}"
    );
}

#[test]
fn request_status_row_renders_up_token_progress() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.spinner_verb_index = 0;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(31_000));
    state.streamed_response_chars = 4_936;
    state.request_token_direction = RequestTokenDirection::Up;

    let rendered = plain_text_lines(&state.request_status_lines()).join("\n");

    assert!(
        rendered.contains("Combobulating...(31s · ↑ 1.2k tokens)"),
        "{rendered}"
    );
}

#[test]
fn request_status_row_renders_down_arrow_during_thinking_before_delta() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(5_000));

    let finished = state.apply_stream_event(StreamEvent::ThinkingStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
    });
    let rendered = plain_text_lines(&state.request_status_lines()).join("\n");

    assert!(!finished);
    assert!(
        rendered.contains("Thinking...(5s · ↓ 0 tokens)"),
        "{rendered}"
    );
}

#[test]
fn request_status_row_ignores_completed_failed_tool_activity() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.spinner_frame = 2;
    state.spinner_verb_index = 0;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(4_000));
    state.streamed_response_chars = 380;
    state.in_progress_tool_use_ids.insert("read-1".to_string());
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "read-1".to_string(),
        tool_name: "Read".to_string(),
        tool_input: "{\"file_path\":\"orbcode/tools/src/lib.rs\"}".to_string(),
        status_line: "Running `Read`".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });

    state.apply_stream_event(StreamEvent::ToolUseCompleted {
        session_id: "session".to_string(),
        tool_use_id: "read-1".to_string(),
        tool_name: "Read".to_string(),
        kind: orbcode_protocol::ToolUseCompletionKind::ExecutionFailed,
    });

    let rendered = plain_text_lines(&state.request_status_lines()).join("\n");

    assert!(state.has_live_tool_activity());
    assert!(rendered.contains("Combobulating"), "{rendered}");
    assert!(rendered.contains("↓ 95 tokens"), "{rendered}");
    assert!(!rendered.contains("Failed during execution"), "{rendered}");
}

#[test]
fn compacting_row_uses_busy_spinner_style() {
    let lines = render_compacting_lines(
        '✶',
        Instant::now() - std::time::Duration::from_millis(13_000),
        None,
    );
    let spinner = &lines[0].spans[0];
    let label = &lines[0].spans[2];

    assert_eq!(spinner.style.fg, Some(CLAUDE_ORANGE));
    assert!(spinner.style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(label.style.fg, Some(CLAUDE_ORANGE));
    assert!(label.style.add_modifier.contains(Modifier::ITALIC));
    assert!(!label.style.add_modifier.contains(Modifier::DIM));
}

#[test]
fn waiting_assistant_lines_render_tip_as_tree_child() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.spinner_verb_index = 0;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(3_000));

    let rendered = state
        .render_waiting_assistant_lines(true)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered.len(), 2);
    assert!(rendered[0].contains("Combobulating...(3s · ↑ 0 tokens)"));
    assert_eq!(
        rendered[1],
        "  └ Tip: Press Esc to interrupt the active turn."
    );
}

#[test]
fn active_request_tips_include_permissions_tip() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.request_count = 4;

    assert_eq!(
        state.active_request_tip_text(),
        Some(
            "Tip: Use /permissions to pre-approve and pre-deny bash, edit, and MCP tools."
                .to_string()
        )
    );
}

#[test]
fn active_request_tips_include_extended_command_tip() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.request_count = 5;

    assert_eq!(
        state.active_request_tip_text(),
        Some("Tip: Press Ctrl+R to browse resumable sessions.".to_string())
    );
}
