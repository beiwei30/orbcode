use super::support::*;

#[test]
#[ignore = "manual stress test for scrolled visible transcript windowing"]
fn transcript_scrolled_window_stress_avoids_full_history_window_clone() {
    const MESSAGE_COUNT: usize = 1_200;
    const FRAME_COUNT: usize = 1_000;
    const VIEW_HEIGHT: usize = 30;
    const REQUESTED_SCROLL: usize = 200;

    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, MESSAGE_COUNT);

    let started = Instant::now();
    let mut last_line_count = 0;
    let mut last_window_len = 0;
    for frame in 0..FRAME_COUNT {
        state.pending_assistant = format!("streaming scrolled tail frame {frame}");
        let view = state.visible_transcript_lines_for_view(100, VIEW_HEIGHT, true);
        assert_eq!(view.actual_scroll, REQUESTED_SCROLL.min(view.max_scroll));
        assert!(view.visible_lines.len() <= VIEW_HEIGHT);
        assert_eq!(view.all_lines, view.visible_lines);
        assert!(view.all_line_count > view.all_lines.len());
        last_line_count = view.all_line_count;
        last_window_len = view.all_lines.len();
    }
    let duration = started.elapsed();

    assert_eq!(state.transcript_ui.render_cache.misses, 1);
    assert_eq!(
        state.transcript_ui.render_cache.hits,
        (FRAME_COUNT - 1) as u64
    );
    eprintln!(
        "messages={MESSAGE_COUNT} frames={FRAME_COUNT} requested_scroll={REQUESTED_SCROLL} total_visual_lines={last_line_count} window_lines={last_window_len} cache_hits={} cache_misses={} loop_us={}",
        state.transcript_ui.render_cache.hits,
        state.transcript_ui.render_cache.misses,
        duration.as_micros()
    );
}

#[test]
fn visible_transcript_lines_follow_latest_visual_rows() {
    let lines = vec![Line::from("123456789012"), Line::from("tail")];

    let transcript_view = visible_transcript_lines(&lines, 5, 3, 0);

    assert_eq!(transcript_view.visible_lines.len(), 3);
    assert_eq!(transcript_view.visible_lines[0], Line::from("67890"));
    assert_eq!(transcript_view.visible_lines[1], Line::from("12"));
    assert_eq!(transcript_view.visible_lines[2], Line::from("tail"));
    assert_eq!(transcript_view.actual_scroll, 0);
    assert_eq!(transcript_view.max_scroll, 1);
}

#[test]
fn visible_transcript_lines_use_visual_row_scrollback() {
    let lines = vec![Line::from("123456789012"), Line::from("tail")];

    let transcript_view = visible_transcript_lines(&lines, 5, 3, 1);

    assert_eq!(transcript_view.visible_lines.len(), 3);
    assert_eq!(transcript_view.visible_lines[0], Line::from("12345"));
    assert_eq!(transcript_view.visible_lines[1], Line::from("67890"));
    assert_eq!(transcript_view.visible_lines[2], Line::from("12"));
    assert_eq!(transcript_view.actual_scroll, 1);
    assert_eq!(transcript_view.max_scroll, 1);
}

#[test]
fn flatten_transcript_cells_inserts_blank_before_local_note() {
    let cells = vec![
        vec![Line::from("● Finished analysis")],
        vec![Line::from("✻ Baked for 12s")],
    ];

    let flattened = flatten_transcript_cells(&cells)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        flattened,
        vec![
            "● Finished analysis".to_string(),
            String::new(),
            "✻ Baked for 12s".to_string()
        ]
    );
}

#[test]
fn flatten_transcript_cells_inserts_blank_before_user_prompt() {
    let user_cell = render_user_message_lines(
        &TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::Text {
                text: "hello".to_string(),
            }],
        ),
        12,
    );
    let cells = vec![vec![Line::from("Orb Code v0.0.1")], user_cell];

    let flattened = flatten_transcript_cells(&cells)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(flattened.len(), 3);
    assert_eq!(flattened[0], "Orb Code v0.0.1");
    assert_eq!(flattened[1], "");
    assert!(flattened[2].starts_with("› hello"));
}

#[test]
fn flatten_transcript_cells_inserts_blank_after_local_note_before_user_prompt() {
    let user_cell = render_user_message_lines(
        &TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::Text {
                text: "评估一下这个项目".to_string(),
            }],
        ),
        40,
    );
    let cells = vec![vec![Line::from("✻ Combobulated for 12s")], user_cell];

    let flattened = flatten_transcript_cells(&cells)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(flattened[0], "✻ Combobulated for 12s");
    assert_eq!(flattened[1], "");
    assert!(flattened[2].starts_with("› 评估一下这个项目"));
}

#[test]
fn flatten_transcript_cells_inserts_blank_between_assistant_text_and_tool_card() {
    let assistant_cell =
        render_assistant_markdown_lines("I'll inspect the repo.", inactive_style(), 60);
    let tool_card = ToolCell {
        tool_use_id: "agent-tool".to_string(),
        tool_name: "Agent".to_string(),
        title: "Explore(Compare repo)".to_string(),
        title_style: Style::default()
            .fg(active_palette().success)
            .add_modifier(Modifier::BOLD),
        status_line: "Done (3 tool uses · 0 tokens · 15s)".to_string(),
        detail_lines: vec![],
        collapsed_preview_lines: vec![],
        prompt: None,
        progress_messages: vec![],
        response: None,
        collapsed_preview_limit: 0,
        is_error: false,
        is_active: false,
    };
    let tool_cell = render_tool_cell_lines(&tool_card, false, None, 80, Path::new("/tmp"));

    let flattened = flatten_transcript_cells(&[assistant_cell, tool_cell])
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(flattened.len(), 4);
    assert_eq!(flattened[0], "● I'll inspect the repo.");
    assert_eq!(flattened[1], "");
    assert!(flattened[2].contains("Explore(Compare repo)"));
    assert!(flattened[3].starts_with("  └ Done"));
}

#[test]
fn history_lines_for_cell_range_preserves_cross_flush_tool_separator() {
    let assistant_cell =
        render_assistant_markdown_lines("I'll inspect the repo.", inactive_style(), 60);
    let tool_card = ToolCell {
        tool_use_id: "agent-tool".to_string(),
        tool_name: "Agent".to_string(),
        title: "Explore(Compare repo)".to_string(),
        title_style: Style::default()
            .fg(active_palette().success)
            .add_modifier(Modifier::BOLD),
        status_line: "Done (3 tool uses · 0 tokens · 15s)".to_string(),
        detail_lines: vec![],
        collapsed_preview_lines: vec![],
        prompt: None,
        progress_messages: vec![],
        response: None,
        collapsed_preview_limit: 0,
        is_error: false,
        is_active: false,
    };
    let tool_cell = render_tool_cell_lines(&tool_card, false, None, 80, Path::new("/tmp"));
    let cells = vec![assistant_cell, tool_cell];

    let mut emitted = history_lines_for_cell_range(&cells, 0, 1);
    emitted.extend(history_lines_for_cell_range(&cells, 1, 2));
    let rendered = plain_text_lines(&emitted);
    let tool_index = rendered
        .iter()
        .position(|line| line.contains("Explore(Compare repo)"))
        .expect("tool card should render");

    assert_eq!(rendered[tool_index.saturating_sub(1)], "");
    assert_eq!(
        rendered[..tool_index]
            .iter()
            .filter(|line| line.is_empty())
            .count(),
        1,
        "{rendered:#?}"
    );
}

#[test]
fn flatten_transcript_cells_does_not_insert_blank_between_assistant_text_and_collapsed_activity() {
    let assistant_cell =
        render_assistant_markdown_lines("I'll inspect the repo.", inactive_style(), 60);
    let collapsed_group = CollapsedActivityGroup {
        search_count: 1,
        read_paths: vec!["orbcode/core/src/lib.rs".to_string()],
        read_operation_count: 0,
        read_tool_use_ids: HashSet::new(),
        failed_read_tool_use_ids: HashSet::new(),
        list_count: 0,
        latest_hint: Some("orbcode/core/src/lib.rs".to_string()),
        detail_lines: vec!["orbcode/core/src/lib.rs".to_string()],
        error_messages: Vec::new(),
        messages: Vec::new(),
        tool_use_ids: HashSet::new(),
        matched_tool_use_ids: HashSet::new(),
        tool_results: ToolResultIndex::new(),
    };
    let collapsed_cell =
        render_collapsed_activity_group_lines(&collapsed_group, false, false, true);

    let flattened = flatten_transcript_cells(&[assistant_cell, collapsed_cell])
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(flattened.len(), 2);
    assert_eq!(flattened[0], "● I'll inspect the repo.");
    assert!(
        flattened[1].starts_with("  Searched for 1 pattern, read 1 file"),
        "{flattened:#?}"
    );
    assert!(flattened[1].contains("Searched for 1 pattern, read 1 file (ctrl+o to expand)"));
}

#[test]
fn history_lines_for_cell_range_keeps_collapsed_activity_adjacent_across_flushes() {
    let assistant_cell =
        render_assistant_markdown_lines("I'll inspect the repo.", inactive_style(), 60);
    let collapsed_group = CollapsedActivityGroup {
        search_count: 1,
        read_paths: vec!["orbcode/core/src/lib.rs".to_string()],
        read_operation_count: 0,
        read_tool_use_ids: HashSet::new(),
        failed_read_tool_use_ids: HashSet::new(),
        list_count: 0,
        latest_hint: Some("orbcode/core/src/lib.rs".to_string()),
        detail_lines: vec!["orbcode/core/src/lib.rs".to_string()],
        error_messages: Vec::new(),
        messages: Vec::new(),
        tool_use_ids: HashSet::new(),
        matched_tool_use_ids: HashSet::new(),
        tool_results: ToolResultIndex::new(),
    };
    let collapsed_cell =
        render_collapsed_activity_group_lines(&collapsed_group, false, false, true);
    let cells = vec![assistant_cell, collapsed_cell];

    let mut emitted = history_lines_for_cell_range(&cells, 0, 1);
    emitted.extend(history_lines_for_cell_range(&cells, 1, 2));
    let rendered = plain_text_lines(&emitted);
    let activity_index = rendered
        .iter()
        .position(|line| line.starts_with("  Searched for 1 pattern, read 1 file"))
        .expect("collapsed activity should render");

    assert_eq!(activity_index, 1, "{rendered:#?}");
    assert!(
        !rendered[..activity_index]
            .iter()
            .any(|line| line.is_empty())
    );
}

#[test]
fn take_history_lines_flushes_committed_history_into_scrollback() {
    let mut state = normal_state("", 0);
    state.messages = (0..8)
        .map(|index| {
            TranscriptMessage::new(MessageRole::Assistant, format!("history line {index}"))
        })
        .collect();
    state.pending_history_flush = true;

    let lines = state.take_history_lines(80, 8);
    assert!(!lines.is_empty());
    assert!(state.history_flushed_message_count > 0);
    assert!(!state.pending_history_flush);
}

#[test]
fn queue_existing_history_flush_marks_loaded_transcript_for_scrollback() {
    let mut state = normal_state("", 0);
    state.messages = vec![TranscriptMessage::new(
        MessageRole::Assistant,
        "resumed history".to_string(),
    )];

    state.queue_existing_history_flush();

    assert!(state.pending_history_flush);
}

#[test]
fn queue_existing_history_flush_leaves_empty_session_idle() {
    let mut state = normal_state("", 0);

    state.queue_existing_history_flush();

    assert!(!state.pending_history_flush);
}

#[test]
fn active_cells_update_in_place_before_committing_history_once() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.active_thinking = Some(ActiveThinkingState {
        text: "first thought".to_string(),
        is_streaming: true,
        completed_at: None,
    });
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-live".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: r#"{"command":"pwd"}"#.to_string(),
        status_line: "Running `pwd`".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });
    state
        .in_progress_tool_use_ids
        .insert("tool-live".to_string());
    let initial_thinking = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert_eq!(initial_thinking.matches("(thinking)").count(), 1);
    state.active_thinking.as_mut().unwrap().text = "updated thought".to_string();
    let updated_thinking = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert_eq!(updated_thinking.matches("(thinking)").count(), 1);

    state.pending_assistant = "partial answer".to_string();
    let initial_live = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert_eq!(initial_live.matches("Bash(pwd)").count(), 1);
    assert_eq!(initial_live.matches("partial answer").count(), 1);

    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-live".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: r#"{"command":"pwd"}"#.to_string(),
        status_line: "Still running `pwd`".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });
    state.pending_assistant.push_str("\nsecond line");

    let updated_live = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert_eq!(updated_live.matches("Bash(pwd)").count(), 1);
    assert_eq!(updated_live.matches("partial answer").count(), 1);
    assert!(state.take_history_lines(80, 20).is_empty());

    state.request_in_flight = false;
    state.active_thinking = None;
    state.clear_live_tool_activities();
    state.pending_assistant.clear();
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "final answer".to_string(),
    ));
    state.pending_history_flush = true;

    let first_history = plain_text_lines(&state.take_history_lines(80, 20)).join("\n");
    let second_history = state.take_history_lines(80, 20);
    assert!(first_history.contains("final answer"), "{first_history}");
    assert!(second_history.is_empty());
}

#[test]
fn history_emission_excludes_prompt_status_and_footer_chrome() {
    let mut state = normal_state("draft input should stay live", 0);
    state.status_line = "status should stay live".to_string();
    state.messages = vec![TranscriptMessage::new(
        MessageRole::Assistant,
        "committed answer".to_string(),
    )];
    state.pending_history_flush = true;

    let history = plain_text_lines(&state.take_history_lines(80, 20)).join("\n");

    assert!(history.contains("committed answer"), "{history}");
    assert!(
        !history.contains("draft input should stay live"),
        "{history}"
    );
    assert!(!history.contains("status should stay live"), "{history}");
}

#[test]
fn take_history_lines_flushes_latest_message_even_when_it_fits() {
    let mut state = normal_state("", 0);
    state.messages = vec![
        TranscriptMessage::new(MessageRole::Assistant, "older log line 1".to_string()),
        TranscriptMessage::new(MessageRole::Assistant, "older log line 2".to_string()),
        TranscriptMessage::new(MessageRole::Assistant, "older log line 3".to_string()),
        TranscriptMessage::new(
            MessageRole::Assistant,
            "一、概览\n二、能力\n三、关键差距分析".to_string(),
        ),
    ];
    state.pending_history_flush = true;

    let history = state.take_history_lines(60, 10);
    let history_text = history
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let viewport_text = state
        .transcript_lines(60)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(history_text.contains("older log line 1"));
    assert!(history_text.contains("一、概览"));
    assert!(history_text.contains("三、关键差距分析"));
    assert!(!viewport_text.contains("一、概览"), "{viewport_text}");
    assert!(
        !viewport_text.contains("三、关键差距分析"),
        "{viewport_text}"
    );
}

#[test]
fn take_history_lines_flushes_long_finished_answer_into_scrollback() {
    let mut state = normal_state("", 0);
    state.messages = vec![
            TranscriptMessage::new(MessageRole::Assistant, "older log line".to_string()),
            TranscriptMessage::new(
                MessageRole::Assistant,
                "一、概览\n这是一段很长的总结，会占据多行。\n二、能力\n这里继续展开说明。\n三、关键差距分析\n这是应该还能通过滚动继续看到的后续内容。".to_string(),
            ),
        ];
    state.pending_history_flush = true;

    let history = state.take_history_lines(32, 8);
    let history_text = history
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let viewport_text = state
        .transcript_lines(32)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(history_text.contains("older log line"));
    assert!(history_text.contains("一、概览"));
    assert!(history_text.contains("三、关键差距分析"));
    assert!(!viewport_text.contains("一、概览"), "{viewport_text}");
    assert!(
        !viewport_text.contains("三、关键差距分析"),
        "{viewport_text}"
    );
    assert!(!state.pending_history_flush);
}

#[test]
fn take_history_lines_flushes_tool_output_with_long_finished_answer() {
    let mut state = normal_state("", 0);
    state.messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "tool-search".to_string(),
                    name: "glob".to_string(),
                    input: "{\"pattern\":\"**/*.ts\"}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-search".to_string(),
                    content: "20340 total".to_string(),
                    is_error: false,
                    metadata: None,
                }],
            ),
            TranscriptMessage::new(
                MessageRole::Assistant,
                "一、概览\n这是一段很长的总结，会占据多行。\n二、能力矩阵\n这里继续展开说明。\n三、关键差距分析\n这是应该通过真实 scrollback 查看的后续内容。".to_string(),
            ),
        ];
    state.pending_history_flush = true;

    let history = state.take_history_lines(36, 8);
    let viewport_text = state
        .transcript_lines(36)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    let history_text = history
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(
        history_text.contains("Searched for")
            || history_text.contains("20340 total")
            || history_text.contains("glob("),
        "{history_text}"
    );
    assert!(history_text.contains("一、概览"));
    assert!(history_text.contains("三、关键差距分析"));
    assert!(!viewport_text.contains("一、概览"), "{viewport_text}");
    assert!(
        !viewport_text.contains("三、关键差距分析"),
        "{viewport_text}"
    );
}

#[test]
fn update_inline_viewport_grows_downward_before_reaching_bottom() {
    let (area, scroll_up) = resized_inline_viewport(Rect::new(0, 5, 80, 4), Size::new(80, 24), 10);

    assert_eq!(area.y, 5);
    assert_eq!(area.height, 10);
    assert_eq!(scroll_up, 0);
}

#[test]
fn initial_top_viewport_grows_downward_from_window_top() {
    let initial = initial_top_viewport_area(Size::new(80, 24));
    let (area, scroll_up) = resized_inline_viewport(initial, Size::new(80, 24), 10);

    assert_eq!(area, Rect::new(0, 0, 80, 10));
    assert_eq!(scroll_up, 0);
}

#[test]
fn update_inline_viewport_keeps_position_when_shrinking() {
    let (area, scroll_up) = resized_inline_viewport(Rect::new(0, 14, 80, 10), Size::new(80, 24), 6);

    assert_eq!(
        area.y, 14,
        "viewport y should stay in place when content shrinks"
    );
    assert_eq!(area.height, 6);
    assert_eq!(scroll_up, 0);
}

#[test]
fn update_inline_viewport_stays_when_terminal_grows() {
    let current = Rect::new(0, 14, 80, 10);
    let (area, scroll_up) = resized_inline_viewport(current, Size::new(80, 30), 10);

    assert_eq!(area.y, 14, "viewport y stays in place when terminal grows");
    assert_eq!(area.height, 10);
    assert_eq!(scroll_up, 0);
}

#[test]
fn insert_history_lines_scrolls_before_writing_above_live_viewport() {
    let backend = RenderFixtureBackend::new(80, 12);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 3));
    let history = vec![
        Line::from("first history row"),
        Line::from("second history row"),
    ];

    insert_history_lines(&mut terminal, &history, 80).expect("insert history lines");

    let output = terminal.backend_mut().output_string();
    assert_history_insert_disables_line_wrap(&output);
    assert!(output.contains("first history row"), "{output:?}");
    assert!(output.contains("second history row"), "{output:?}");
    assert!(
        output.matches("\x1bM").count() >= 2,
        "history insertion should reserve rows by moving the live viewport down first: {output:?}"
    );
    assert_eq!(terminal.viewport_area.y, 6);
}

#[test]
fn insert_history_lines_at_top_reserves_rows_before_writing() {
    let backend = RenderFixtureBackend::new(80, 12);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    terminal.set_viewport_area(Rect::new(0, 0, 80, 8));
    let history = vec![
        Line::from("first history row"),
        Line::from("second history row"),
    ];

    insert_history_lines(&mut terminal, &history, 80).expect("insert history lines");

    let output = terminal.backend_mut().output_string();
    assert_history_insert_disables_line_wrap(&output);
    assert!(output.contains("first history row"), "{output:?}");
    assert!(output.contains("second history row"), "{output:?}");
    assert!(
        output.matches("\x1bM").count() >= 2,
        "top-aligned history insertion should move the live viewport down before writing: {output:?}"
    );
    assert_eq!(terminal.viewport_area.y, 2);
}

#[test]
fn insert_history_lines_with_bottom_gap_preserves_native_rows_below_viewport() {
    let backend = RenderFixtureBackend::new(80, 12);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    terminal.set_viewport_area(Rect::new(0, 1, 80, 3));
    let mut screen = TerminalScreenModel::new(80, 12);
    screen.process_bytes(
        b"\x1b[7;1Hnative shell launch command\x1b[8;1Hnative banner row before tui",
    );
    let history = vec![
        Line::from("first committed history row"),
        Line::from("second committed history row"),
    ];

    insert_history_lines(&mut terminal, &history, 80).expect("insert history lines");
    screen.process_bytes(terminal.backend_mut().output.as_slice());

    let full = screen.full_contents().join("\n");
    assert!(
        full.contains("native shell launch command"),
        "history insertion should not clear native rows below the old viewport\n{full}"
    );
    assert!(
        full.contains("native banner row before tui"),
        "history insertion should preserve terminal-owned rows while reserving bottom gap\n{full}"
    );
    assert_eq!(terminal.viewport_area.y, 3);
}

#[test]
fn insert_history_lines_clears_old_live_viewport_before_reserving_rows() {
    let backend = RenderFixtureBackend::new(80, 12);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 3));
    let mut screen = TerminalScreenModel::new(80, 12);
    screen.process_bytes(
        b"\x1b[5;1Hstale spinner must not move\x1b[6;1Hstale live row must not move\x1b[9;1Hnative row below viewport",
    );
    let history = vec![
        Line::from("first committed history row"),
        Line::from("second committed history row"),
    ];

    insert_history_lines(&mut terminal, &history, 80).expect("insert history lines");
    screen.process_bytes(terminal.backend_mut().output.as_slice());

    let full = screen.full_contents().join("\n");
    assert!(
        !full.contains("stale spinner must not move")
            && !full.contains("stale live row must not move"),
        "history insertion should clear stale live viewport rows before RI moves content\n{full}"
    );
    assert!(
        full.contains("native row below viewport"),
        "history insertion should not use a from-cursor-down clear that erases rows below the live viewport\n{full}"
    );
    assert_eq!(terminal.viewport_area.y, 6);
}

#[test]
fn first_drawn_history_flush_appends_through_native_scrollback() {
    let backend = RenderFixtureBackend::new(80, 12);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    terminal.set_viewport_area(Rect::new(0, 6, 80, 4));
    let mut state = normal_state("", 0);
    terminal
        .draw(|frame| {
            state.draw(frame);
        })
        .expect("draw initial live viewport");
    terminal.backend_mut().output.clear();
    terminal.set_viewport_area(Rect::new(0, 6, 80, 2));
    terminal.last_known_cursor_pos = Position::new(11, 9);
    state.push_local_slash_command_output(
        "/allow-all on",
        "Allow-all mode enabled.",
        Some(
            "Tool and network permission prompts are bypassed; configured deny rules still apply."
                .to_string(),
        ),
    );

    let flushed = flush_pending_history_to_scrollback(&mut terminal, &mut state, 80, 12)
        .expect("flush first history lines");

    assert!(flushed, "local slash output should flush history");
    let output = terminal.backend_mut().output_string();
    assert!(
        output.find("\x1b[?7l") < output.rfind("\x1b[?7h"),
        "history insert should disable and then restore line wrap: {output:?}"
    );
    assert!(
        !output.contains("\x1b[J"),
        "history insert should not clear below the live viewport: {output:?}"
    );
    assert!(
        output.contains("\x1b[2K"),
        "history insert should clear old live viewport rows: {output:?}"
    );
    assert!(
        !output.contains("\x1b[1;"),
        "first drawn history flush should use full-screen scrolling so native tmux history is preserved: {output:?}"
    );
    assert_eq!(
        output.matches("Orb Code").count(),
        1,
        "old live banner should be cleared instead of copied into scrollback: {output:?}"
    );
    assert_eq!(terminal.viewport_area.y, 10);
}

#[test]
fn first_user_message_history_flush_appends_through_native_scrollback() {
    // The first USER message flush must still take the native-append path (which
    // preserves pre-TUI scrollback) even though the intro banner has already been
    // emitted into the live viewport. Regression guard for the relaxed
    // first-flush trigger (emitted_cell_count == 0 alone).
    let backend = RenderFixtureBackend::new(83, 25);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    terminal.set_viewport_area(Rect::new(0, 14, 83, 11));
    let mut state = normal_state("", 0);
    terminal
        .draw(|frame| {
            state.draw(frame);
        })
        .expect("draw initial live viewport");
    terminal.backend_mut().output.clear();
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "找出代码行数排名前10的 .rs 文件".to_string(),
    ));
    state.pending_history_flush = true;

    let result = prepare_draw_transaction(&mut terminal, &mut state, false)
        .expect("flush first user message history");

    assert!(result.history_flushed, "first user message should flush");
    let output = terminal.backend_mut().output_string();
    assert!(
        !output.contains("\x1b[1;"),
        "first user message flush should not use a constrained scroll region: {output:?}"
    );
    assert_eq!(
        output.matches("Orb Code").count(),
        1,
        "intro banner should be appended once during the first history flush: {output:?}"
    );
    assert_eq!(
        terminal.viewport_area.bottom(),
        25,
        "first flush should leave the live viewport bottom-pinned"
    );
}

fn assert_history_insert_disables_line_wrap(output: &str) {
    let disable = output
        .find("\x1b[?7l")
        .unwrap_or_else(|| panic!("history insert should disable line wrap: {output:?}"));
    let enable = output
        .rfind("\x1b[?7h")
        .unwrap_or_else(|| panic!("history insert should restore line wrap: {output:?}"));
    assert!(
        disable < enable,
        "line wrap should be restored after it is disabled: {output:?}"
    );
    assert!(
        !output.contains("\x1b[J"),
        "history insert should not clear below the live viewport: {output:?}"
    );
    let clear_line = output
        .find("\x1b[2K")
        .unwrap_or_else(|| panic!("history insert should clear live viewport rows: {output:?}"));
    let reverse_index = output
        .find("\x1bM")
        .unwrap_or_else(|| panic!("history insert should reserve rows with RI: {output:?}"));
    assert!(
        disable < clear_line && clear_line < reverse_index && reverse_index < enable,
        "history insert should disable wrap, clear live rows, reserve rows, then restore wrap: {output:?}"
    );
}

#[test]
fn transcript_pager_terminal_mode_uses_alternate_screen_and_restores_inline_viewport() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    terminal.set_viewport_area(Rect::new(0, 10, 80, 8));
    let mut mode = TranscriptPagerTerminalMode::default();

    let entered = sync_transcript_pager_terminal_mode(&mut terminal, true, &mut mode)
        .expect("enter transcript pager terminal mode");

    assert!(entered);
    assert!(mode.is_active());
    assert_eq!(terminal.viewport_area, Rect::new(0, 0, 80, 24));
    let entered_output = terminal.backend_mut().output_string();
    assert!(
        entered_output.contains("\x1b[?1049h"),
        "pager should enter terminal alternate screen"
    );
    assert!(
        !entered_output.contains("\x1b[?1007h"),
        "pager should not enable alternate scroll mode"
    );
    assert!(
        !entered_output.contains("\x1b[?1000h")
            && !entered_output.contains("\x1b[?1002h")
            && !entered_output.contains("\x1b[?1003h"),
        "pager should not enable mouse capture: {entered_output:?}"
    );
    let entered_output_len = entered_output.len();

    let left = sync_transcript_pager_terminal_mode(&mut terminal, false, &mut mode)
        .expect("leave transcript pager terminal mode");

    assert!(left);
    assert!(!mode.is_active());
    assert_eq!(terminal.viewport_area, Rect::new(0, 10, 80, 8));
    let output = terminal.backend_mut().output_string();
    let left_output = &output[entered_output_len..];
    assert!(
        !left_output.contains("\x1b[?1007l"),
        "pager should not emit alternate scroll mode cleanup"
    );
    assert!(
        left_output.contains("\x1b[?1049l"),
        "pager should leave terminal alternate screen"
    );
    assert!(
        !output.contains("\x1b[?1000h")
            && !output.contains("\x1b[?1002h")
            && !output.contains("\x1b[?1003h"),
        "pager should not enable mouse capture: {output:?}"
    );
}

#[test]
fn update_inline_viewport_pins_to_bottom_when_growth_exceeds_screen_height() {
    let (area, scroll_up) =
        resized_inline_viewport(Rect::new(0, 14, 80, 10), Size::new(80, 24), 12);

    assert_eq!(area.y, 12);
    assert_eq!(area.height, 12);
    assert_eq!(area.bottom(), 24);
    assert_eq!(scroll_up, 2);
}

#[test]
fn update_inline_viewport_reserves_top_row_when_content_exceeds_terminal_height() {
    let size = Size::new(142, 38);
    let (area, scroll_up) = resized_inline_viewport(Rect::new(0, 1, 142, 37), size, 39);

    assert_eq!(
        area,
        Rect::new(0, 1, 142, 37),
        "inline viewport must not grow into top=0/full-screen mode"
    );
    assert_eq!(scroll_up, 0);

    let (shrunk, scroll_up) = resized_inline_viewport(area, size, 6);
    assert_eq!(
        shrunk,
        Rect::new(0, 1, 142, 6),
        "shrinking after a capped tall viewport must keep the reserved top row"
    );
    assert_eq!(scroll_up, 0);
}

#[test]
fn update_inline_viewport_repositions_and_clears_new_live_viewport() {
    let backend = RenderFixtureBackend::new(80, 12);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    terminal.set_viewport_area(Rect::new(0, 8, 80, 4));

    let changed = update_inline_viewport_generic(&mut terminal, 6).expect("update viewport");

    assert!(changed);
    assert_eq!(terminal.viewport_area, Rect::new(0, 6, 80, 6));
    let output = terminal.backend_mut().output_string();
    assert!(
        output.contains("\x1b[7;1H\x1b[0J"),
        "new live viewport should be cleared before redraw: {output:?}"
    );
    assert!(
        !output.contains("\x1b[1;8r"),
        "height-only viewport adjustment should not scroll native history: {output:?}"
    );
}

#[test]
fn active_streaming_viewport_growth_preserves_terminal_history() {
    let backend = RenderFixtureBackend::new(80, 12);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    terminal.set_viewport_area(Rect::new(0, 8, 80, 4));
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = (0..16)
        .map(|index| format!("streaming answer line {index}"))
        .collect::<Vec<_>>()
        .join("\n");

    let result = prepare_draw_transaction(&mut terminal, &mut state, false)
        .expect("prepare draw transaction");

    assert!(result.viewport_mutated);
    assert!(
        terminal.viewport_area.top() < 8,
        "active transcript should grow upward"
    );
    let output = terminal.backend_mut().output_string();
    assert!(
        output.contains("\x1b[2;1H\x1b[0J"),
        "active streaming growth should clear the new live viewport before redraw: {output:?}"
    );
    assert!(
        !output.contains("\x1b[1;8r"),
        "active streaming height growth should not scroll native history: {output:?}"
    );
}

#[test]
fn active_streaming_upward_growth_scrolls_history_to_native_scrollback() {
    // With visible history above the viewport, growing the active viewport
    // upward must NOT use a DECSTBM sub-region scroll (`SetScrollRegion`), which
    // discards rows scrolled off the region top — permanently losing history
    // such as the intro banner. It must use a full-screen scroll (ResetScrollRegion
    // + `\r\n` at the last row) so displaced rows reach native scrollback.
    let backend = RenderFixtureBackend::new(80, 12);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    terminal.set_viewport_area(Rect::new(0, 8, 80, 4));
    // Simulate visible history occupying the rows directly above the viewport.
    terminal.note_history_rows_inserted(8);
    assert!(terminal.visible_history_rows() > 0);

    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = (0..16)
        .map(|index| format!("streaming answer line {index}"))
        .collect::<Vec<_>>()
        .join("\n");

    let result = prepare_draw_transaction(&mut terminal, &mut state, false)
        .expect("prepare draw transaction");

    assert!(result.viewport_mutated);
    assert!(
        terminal.viewport_area.top() < 8,
        "active transcript should grow upward"
    );
    let output = terminal.backend_mut().output_string();
    assert!(
        !output.contains("\x1b[1;8r"),
        "upward growth must not use a discard-prone sub-region scroll: {output:?}"
    );
    assert!(
        output.contains("\x1b[r"),
        "upward growth should use a full-screen scroll (ResetScrollRegion) to reach scrollback: {output:?}"
    );
}

#[test]
fn idle_slash_suggestions_reserve_terminal_rows_before_first_flush() {
    // In the startup window (before the first flush), an idle slash-suggestion
    // panel that needs more rows must RESERVE terminal-owned rows — full-screen
    // scroll the pre-TUI history up into native scrollback (preserved/scrollable)
    // — instead of capping the growth or clearing the rows above (which would eat
    // the pre-TUI shell output). The old cap behavior ate history at idle.
    let width = 88;
    let height = 29;
    let mut idle_state = normal_state("", 0);
    let idle_height = idle_state.desired_viewport_height(width, height);
    let mut state = normal_state("/", 1);
    let suggestion_height = state.desired_viewport_height(width, height);
    assert!(suggestion_height > idle_height);

    let backend = RenderFixtureBackend::new(width, height);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    let initial_area = Rect::new(0, height - idle_height, width, idle_height);
    terminal.set_viewport_area(initial_area);

    let result = prepare_draw_transaction(&mut terminal, &mut state, false)
        .expect("prepare draw transaction");

    assert!(result.viewport_mutated);
    assert_eq!(terminal.viewport_area.height, suggestion_height);
    assert_eq!(terminal.viewport_area.bottom(), height);
    let output = terminal.backend_mut().output_string();
    let scroll_index = output.find("\r\n").unwrap_or_else(|| {
        panic!("suggestions should reserve rows by scrolling terminal-owned content\n{output:?}")
    });
    let clear_index = output.find("\x1b[0J").unwrap_or_else(|| {
        panic!("old live viewport should be cleared before reserving rows\n{output:?}")
    });
    assert!(
        clear_index < scroll_index,
        "old live viewport should be cleared before the full-screen reserve scroll\n{output:?}"
    );
    assert!(
        !output.contains("\x1b[1;"),
        "reserve should use a full-screen scroll, not a constrained scroll region\n{output:?}"
    );
}

#[test]
fn idle_task_panel_reserves_terminal_rows_before_first_flush() {
    let width = 88u16;
    let height = 29u16;
    let mut idle_state = normal_state("", 0);
    let idle_height = idle_state.desired_viewport_height(width, height);
    let mut state = normal_state("", 0);
    let now = Instant::now();
    state.task_panel.clear_awaiting_session_activity();
    state.task_panel.apply_snapshot(task_snapshot(vec![]), now);
    state.task_panel.apply_snapshot(
        task_snapshot(vec![
            task_view(
                "1",
                "Inspect terminal resize",
                orbcode_tools::TaskStatusKind::InProgress,
            ),
            task_view(
                "2",
                "Keep shell history",
                orbcode_tools::TaskStatusKind::Pending,
            ),
        ]),
        now,
    );
    assert!(
        !state
            .request_status_lines_for_width(width as usize)
            .is_empty()
    );
    let task_panel_height = state.desired_viewport_height(width, height);
    assert!(task_panel_height > idle_height);

    let backend = RenderFixtureBackend::new(width, height);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    let initial_area = Rect::new(0, height - idle_height, width, idle_height);
    terminal.set_viewport_area(initial_area);

    let result = prepare_draw_transaction(&mut terminal, &mut state, false)
        .expect("prepare draw transaction");

    assert!(result.viewport_mutated);
    assert_eq!(terminal.viewport_area.height, task_panel_height);
    assert_eq!(terminal.viewport_area.bottom(), height);
    let output = terminal.backend_mut().output_string();
    let scroll_index = output.find("\r\n").unwrap_or_else(|| {
        panic!(
            "idle task panel should reserve rows by scrolling terminal-owned content\n{output:?}"
        )
    });
    let clear_index = output.find("\x1b[0J").unwrap_or_else(|| {
        panic!("old live viewport should be cleared before reserving rows\n{output:?}")
    });
    assert!(
        clear_index < scroll_index,
        "old live viewport should be cleared before the full-screen reserve scroll\n{output:?}"
    );
    assert!(
        !output.contains("\x1b[1;"),
        "reserve should use a full-screen scroll, not a constrained scroll region\n{output:?}"
    );
}

fn task_snapshot(tasks: Vec<orbcode_tools::TaskView>) -> orbcode_tools::TaskListSnapshot {
    let mut summary = orbcode_tools::TaskListSummary::default();
    for task in &tasks {
        match task.status {
            orbcode_tools::TaskStatusKind::Pending => summary.pending += 1,
            orbcode_tools::TaskStatusKind::InProgress => summary.in_progress += 1,
            orbcode_tools::TaskStatusKind::Completed => summary.completed += 1,
        }
        summary.total += 1;
    }

    orbcode_tools::TaskListSnapshot {
        task_list_id: "test-session".to_string(),
        directory: PathBuf::from("/tmp/test-session"),
        tasks,
        summary,
        fingerprint: 0,
    }
}

fn task_view(
    id: &str,
    subject: &str,
    status: orbcode_tools::TaskStatusKind,
) -> orbcode_tools::TaskView {
    orbcode_tools::TaskView {
        id: id.to_string(),
        subject: subject.to_string(),
        description: String::new(),
        active_form: None,
        owner: None,
        status,
        blocks: Vec::new(),
        blocked_by: Vec::new(),
        open_blockers: Vec::new(),
    }
}

#[test]
fn history_flush_allows_committed_cells_during_active_turn() {
    let mut state = normal_state("", 0);
    state.messages = vec![TranscriptMessage::new(
        MessageRole::Assistant,
        "streamed chunk committed".to_string(),
    )];
    state.pending_history_flush = true;
    state.request_in_flight = true;

    assert!(state.should_flush_history());
    let history = state.take_history_lines(80, 20);
    let history_text = history
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(history_text.contains("streamed chunk committed"));
}

#[test]
fn transcript_lines_exclude_committed_messages_after_history_flush() {
    let mut state = normal_state("", 0);
    state.messages = vec![
        TranscriptMessage::new(MessageRole::User, "older message".to_string()),
        TranscriptMessage::new(MessageRole::Assistant, "latest message".to_string()),
    ];
    state.pending_history_flush = true;
    let history = state.take_history_lines(80, 20);
    let history_text = plain_text_lines(&history).join("\n");

    let text = state
        .transcript_lines(80)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(history_text.contains("older message"), "{history_text}");
    assert!(history_text.contains("latest message"), "{history_text}");
    assert!(!text.contains("older message"), "{text}");
    assert!(!text.contains("latest message"), "{text}");
}

#[test]
fn idle_viewport_does_not_duplicate_committed_messages_after_history_flush() {
    let mut state = normal_state("", 0);
    state.messages = vec![
        TranscriptMessage::new(
            MessageRole::Assistant,
            "older committed message".to_string(),
        ),
        TranscriptMessage::new(
            MessageRole::Assistant,
            "latest committed message".to_string(),
        ),
    ];
    state.pending_history_flush = true;

    let history = state.take_history_lines(80, 20);
    let history_text = history
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let viewport_text = state
        .transcript_lines(80)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(
        history_text.contains("latest committed message"),
        "{history_text}"
    );
    assert!(
        !viewport_text.contains("older committed message"),
        "{viewport_text}"
    );
    assert!(
        !viewport_text.contains("latest committed message"),
        "{viewport_text}"
    );
}

#[test]
fn transcript_lines_show_intro_banner_before_first_history_flush() {
    let mut state = normal_state("", 0);
    let text = state
        .transcript_lines(90)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("Orb Code"));
    assert!(text.contains("model"));
}

#[test]
fn transcript_lines_pad_intro_banner_vertically() {
    let mut state = normal_state("", 0);
    let rendered = plain_text_lines(&state.transcript_lines(90));

    assert_eq!(rendered.first().map(String::as_str), Some(""));
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("No transcript yet.")),
        "empty transcript placeholder should not appear"
    );

    state.messages = vec![TranscriptMessage::new(
        MessageRole::User,
        "first prompt".to_string(),
    )];
    let rendered_with_prompt = plain_text_lines(&state.transcript_lines(90));
    let prompt_index = rendered_with_prompt
        .iter()
        .position(|line| line.starts_with("› first prompt"))
        .expect("first prompt should render after the banner");
    assert_eq!(rendered_with_prompt[prompt_index.saturating_sub(1)], "");
}

#[test]
fn transcript_lines_pad_submitted_user_prompt_vertically() {
    let mut state = normal_state("", 0);
    state.messages = vec![TranscriptMessage::new(
        MessageRole::User,
        "评估一下这个仓库中各个 crate 的测试覆盖情况".to_string(),
    )];
    state.request_in_flight = true;
    state.active_thinking = Some(ActiveThinkingState {
        text: "让我开始探索项目结构。".to_string(),
        is_streaming: true,
        completed_at: None,
    });

    let rendered = plain_text_lines(&state.transcript_lines(90));
    let prompt_index = rendered
        .iter()
        .position(|line| line.starts_with("› 评估一下这个仓库"))
        .expect("submitted prompt should render");
    let thinking_index = rendered
        .iter()
        .position(|line| line.contains("(thinking)") || line.starts_with("∴ Thinking"))
        .expect("active thinking should render after the prompt");

    assert_eq!(rendered[prompt_index.saturating_sub(1)], "");
    assert_eq!(rendered[prompt_index + 1], "");
    assert_eq!(thinking_index, prompt_index + 2);
}

#[test]
fn allow_all_history_output_keeps_summary_without_long_detail() {
    // With the AllowAll command's feedback set to SUMMARY_HIDDEN_DEFERRED, the
    // committed transcript keeps the short summary but hides the long detail, so
    // /allow-all doesn't pollute the preserved scrollback.
    let mut state = normal_state("", 0);
    state.push_local_slash_command_output(
        "/allow-all on",
        "Allow-all mode enabled.",
        Some(
            "Tool and network permission prompts are bypassed; configured deny rules still apply."
                .to_string(),
        ),
    );

    let rendered = plain_text_lines(&state.transcript_lines(90)).join("\n");

    assert!(rendered.contains("❯ /allow-all on"), "{rendered}");
    assert!(rendered.contains("Allow-all mode enabled."), "{rendered}");
    assert!(
        !rendered.contains("Tool and network permission prompts"),
        "{rendered}"
    );
}

#[test]
fn intro_banner_flushes_as_first_history_cell() {
    let mut state = normal_state("", 0);
    state.pending_history_flush = true;

    let history = state.take_history_lines(90, 24);
    let history_text = history
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(history_text.contains("Orb Code"), "{history_text}");
    assert!(history_text.contains("model"), "{history_text}");
    assert_eq!(state.history_flushed_message_count, 1);
}

#[test]
fn banner_moves_to_history_after_history_flush() {
    let mut state = normal_state("", 0);
    state.messages = vec![
        TranscriptMessage::new(MessageRole::User, "inspect the workspace".to_string()),
        TranscriptMessage::new(
            MessageRole::Assistant,
            "I'll inspect the workspace.".to_string(),
        ),
    ];
    state.request_in_flight = true;
    state.pending_history_flush = true;
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: r#"{"command":"find . -name \"*.rs\""}"#.to_string(),
        status_line: "Searching for 1 pattern".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });
    state.in_progress_tool_use_ids.insert("tool-1".to_string());

    let history = state.take_history_lines(90, 24);
    let history_text = plain_text_lines(&history).join("\n");
    let viewport_text = plain_text_lines(&state.transcript_lines(90)).join("\n");

    assert!(history_text.contains("Orb Code"), "{history_text}");
    assert!(!viewport_text.contains("Orb Code"), "{viewport_text}");
    assert!(
        history_text.contains("inspect the workspace"),
        "{history_text}"
    );
    assert!(
        !viewport_text.contains("inspect the workspace"),
        "{viewport_text}"
    );
    assert!(viewport_text.contains("Bash"), "{viewport_text}");
}

#[test]
fn banner_stays_visible_in_transcript_after_first_user_message() {
    let mut state = normal_state("", 0);
    state.messages = vec![TranscriptMessage::new(
        MessageRole::User,
        "why did the banner disappear?".to_string(),
    )];

    let viewport_text = plain_text_lines(&state.transcript_lines(90)).join("\n");

    assert!(viewport_text.contains("Orb Code"), "{viewport_text}");
    assert!(
        viewport_text.contains("why did the banner disappear?"),
        "{viewport_text}"
    );
}

#[test]
fn pending_history_flush_does_not_change_runtime_viewport_height() {
    let mut state = normal_state("", 0);
    state.messages = (0..24)
        .map(|index| {
            TranscriptMessage::new(
                if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                format!("committed transcript line {index}"),
            )
        })
        .collect();
    let baseline_height = state.desired_viewport_height(80, 100);
    state.pending_history_flush = true;
    let flushed_height = state.desired_viewport_height(80, 100);

    assert_eq!(flushed_height, baseline_height);
}

#[test]
fn short_transcript_layout_keeps_request_panel_attached_to_content() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    let area = Rect::new(0, 0, 80, 30);
    let input_view = build_input_view(&state.input, state.input_cursor, 77, MAX_INPUT_INNER_HEIGHT);
    let request_status_height = state.request_status_lines().len() as u16;

    let layout = state.main_layout_regions(area, &input_view, request_status_height);
    let transcript_height = state.transcript_content_height(80, true);

    assert_eq!(layout[0].height, transcript_height);
    assert_eq!(layout[1].y, layout[0].y + layout[0].height);
    assert!(layout[7].height > 0);
}

#[test]
fn browsing_oldest_transcript_lines_shows_intro_banner() {
    let mut state = normal_state("", 0);
    state.messages = vec![
        TranscriptMessage::new(MessageRole::User, "inspect the workspace".to_string()),
        TranscriptMessage::new(
            MessageRole::Assistant,
            "I'll inspect the workspace.".to_string(),
        ),
    ];
    state.focus_latest_message_start = true;

    let transcript_view = state.visible_transcript_lines_for_view(90, 12, false);
    let text = plain_text_lines(&transcript_view.visible_lines).join("\n");

    assert!(text.contains("Orb Code"), "{text}");
    assert!(text.contains("inspect the workspace"), "{text}");
}

#[test]
fn transcript_cells_from_messages_preserve_tool_cells_as_structured_state() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
            TranscriptMessage::new(MessageRole::Assistant, "I'll inspect the repo.".to_string()),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "Agent".to_string(),
                    input:
                        "{\"description\":\"Inspect repo\",\"prompt\":\"inspect files\",\"subagent_type\":\"Explore\"}"
                            .to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "file list".to_string(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        ];

    let cells = transcript_cells_from_messages(&messages, &cwd);

    assert_eq!(cells.len(), 2);
    assert!(matches!(cells[0], TranscriptCell::Message(_)));
    let TranscriptCell::Tool(tool) = &cells[1] else {
        panic!("expected structured tool cell, got {:?}", cells[1]);
    };
    assert_eq!(tool.tool_use_id, "tool-1");
    assert!(tool.title.contains("Inspect repo"));
    assert!(!tool.is_active);
}

#[test]
fn transcript_cells_split_multi_tool_assistant_messages_into_tool_rows() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "Bash".to_string(),
                    input: "{\"command\":\"pwd\"}".to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "tool-2".to_string(),
                    name: "Bash".to_string(),
                    input: "{\"command\":\"ls -la\"}".to_string(),
                },
            ],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![
                TranscriptBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "/Users/user/github/sample-repo".to_string(),
                    is_error: false,
                    metadata: Some("{\"summary\":\"Executed `pwd`.\"}".to_string()),
                },
                TranscriptBlock::ToolResult {
                    tool_use_id: "tool-2".to_string(),
                    content: "total 60\n-rw-r--r-- Cargo.toml".to_string(),
                    is_error: false,
                    metadata: Some("{\"summary\":\"Executed `ls -la`.\"}".to_string()),
                },
            ],
        ),
    ];

    let transcript = committed_transcript_fixture_text(
        &TuiState {
            client: None,
            messages,
            cwd,
            cwd_display: "~".to_string(),
            ui_version: "2.1.119".to_string(),
            model_display_name: "model".to_string(),
            context_window_options: ContextWindowOptions::default(),
            max_output_token_options: MaxOutputTokenOptions::default(),
            token_warning_options: TokenWarningOptions::default(),
            default_provider_label: "anthropic".to_string(),
            ..normal_state("", 0)
        },
        80,
    );

    assert!(transcript.contains("Bash(pwd)"), "{transcript}");
    assert!(
        transcript.contains("Listed 1 directory (ctrl+o to expand)"),
        "{transcript}"
    );
    assert!(!transcript.contains("Executed `pwd`."), "{transcript}");
    assert!(!transcript.contains("Executed `ls -la`."), "{transcript}");
}

#[test]
fn intro_banner_lines_use_actual_logo_width_for_info_budget() {
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
        recent_denied_permissions: Vec::new(),
        status_line: String::new(),
        status_line_set_at: None,
        ui_version: "2.1.888".to_string(),
        cwd_display: "~/github/sample-workspace-main/crates/render-fixtures".to_string(),
        model_display_name: "glm-5:cloud(openai)".to_string(),
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
        .intro_banner_lines(56)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(rendered[1].contains("glm-5:cloud(openai)"));
    assert!(rendered[2].contains("render-fixtures"));
}

#[test]
fn shrinking_live_transcript_does_not_insert_large_blank_gap() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = "streaming answer line one\nline two\nline three".to_string();
    state.mark_transcript_bottom_pin_sticky();

    let tall_view = state.visible_transcript_lines_for_view(80, 20, true);
    let blank_prefix = tall_view
        .visible_lines
        .iter()
        .take_while(|line| {
            line.spans.is_empty()
                || plain_text_lines(std::slice::from_ref(line))
                    .join("")
                    .trim()
                    .is_empty()
        })
        .count();

    state.pending_assistant = "short".to_string();
    let shrunk_view = state.visible_transcript_lines_for_view(80, 20, true);
    let shrunk_blank_prefix = shrunk_view
        .visible_lines
        .iter()
        .take_while(|line| {
            line.spans.is_empty()
                || plain_text_lines(std::slice::from_ref(line))
                    .join("")
                    .trim()
                    .is_empty()
        })
        .count();

    assert!(
        shrunk_blank_prefix <= blank_prefix + 2,
        "shrinking content should not create disproportionate blank gap: \
         before={blank_prefix}, after={shrunk_blank_prefix}"
    );
}

#[test]
fn bottom_pin_padding_never_enters_history_emission() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "user prompt".to_string(),
    ));
    state.pending_history_flush = true;
    let _ = state.take_history_lines(80, 20);

    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "committed answer".to_string(),
    ));
    state.pending_history_flush = true;
    state.mark_transcript_bottom_pin_sticky();

    let history = state.take_history_lines(80, 20);
    let history_text = plain_text_lines(&history);

    let leading_blanks = history_text
        .iter()
        .take_while(|line| line.trim().is_empty())
        .count();
    assert!(
        leading_blanks <= 1,
        "history emission must not start with padding blank lines: {history_text:?}"
    );
    assert!(
        history_text
            .iter()
            .any(|line| line.contains("committed answer")),
        "history should contain the committed answer: {history_text:?}"
    );
}

#[test]
fn final_assistant_completion_does_not_interleave_stale_content() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "initial prompt".to_string(),
    ));
    state.pending_history_flush = true;
    let _ = state.take_history_lines(80, 20);

    state.request_in_flight = true;
    state.pending_assistant = "streaming answer chunk".to_string();

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(
            MessageRole::Assistant,
            "final answer from server".to_string(),
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    assert!(
        state.pending_assistant.is_empty(),
        "pending_assistant should be cleared after completion"
    );

    let history = plain_text_lines(&state.take_history_lines(80, 20)).join("\n");
    assert!(
        history.contains("final answer from server"),
        "history should contain the server's final answer: {history}"
    );
    assert!(
        !history.contains("streaming answer chunk"),
        "history must not contain stale streaming chunks: {history}"
    );
}

#[test]
fn final_assistant_commit_uses_source_backed_history_only() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_assistant = "live streaming content".to_string();

    let pre_commit_transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(
        pre_commit_transcript.contains("live streaming content"),
        "before commit, live content should be visible: {pre_commit_transcript}"
    );

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(
            MessageRole::Assistant,
            "committed final answer".to_string(),
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    let history = plain_text_lines(&state.take_history_lines(80, 20)).join("\n");
    assert!(
        history.contains("committed final answer"),
        "history should contain source-of-truth answer: {history}"
    );
    assert!(
        !history.contains("live streaming content"),
        "history must not contain streaming chunks: {history}"
    );

    let post_commit_transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(
        !post_commit_transcript.contains("live streaming content"),
        "after commit + flush, live content must not be in transcript: {post_commit_transcript}"
    );
}

#[test]
fn terminal_transition_history_flush_does_not_interleave_chrome() {
    let backend = RenderFixtureBackend::new(80, 24);
    let mut terminal = Terminal::with_options(backend).expect("create terminal fixture");
    terminal.set_viewport_area(Rect::new(0, 4, 80, 20));
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "initial prompt".to_string(),
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "first answer".to_string(),
    ));
    state.pending_history_flush = true;

    let history = state.take_pending_history_lines_for_emission(80, 24);
    assert!(!history.is_empty());
    insert_history_lines(&mut terminal, &history, 80).expect("insert history");

    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "second answer".to_string(),
    ));
    state.pending_history_flush = true;
    let history2 = state.take_pending_history_lines_for_emission(80, 24);
    insert_history_lines(&mut terminal, &history2, 80).expect("insert second batch");

    terminal.draw(|frame| state.draw(frame)).expect("draw");

    let output = terminal.backend_mut().output_string();
    let first_pos = output.find("first answer");
    let second_pos = output.find("second answer");
    assert!(
        first_pos.is_some() && second_pos.is_some(),
        "both answers should be in output"
    );
    if let (Some(first), Some(second)) = (first_pos, second_pos) {
        assert!(first < second, "first answer should appear before second");
        let between = &output[first..second];
        assert!(
            !between.contains("›"),
            "prompt chrome marker '›' must not appear between two committed answers: \
             ...{between}..."
        );
    }
}

#[test]
fn pager_open_defers_history_and_close_flushes() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "prompt before pager".to_string(),
    ));
    state.pending_history_flush = true;
    let _ = state.take_history_lines(80, 24);

    state.open_transcript_pager(80, 24);
    assert!(matches!(
        state.overlay,
        Some(OverlayState::TranscriptPager(_))
    ));

    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "answer during pager".to_string(),
    ));
    state.pending_history_flush = true;
    state.prepare_pending_history_emission(80, 24);
    assert!(
        !state.transcript_ui.emission.pending_lines.is_empty(),
        "pending lines should accumulate while pager is open"
    );

    state.overlay = None;
    let flushed = state.take_pending_history_lines_for_emission(80, 24);
    let flushed_text = plain_text_lines(&flushed).join("\n");
    assert!(
        flushed_text.contains("answer during pager"),
        "history accumulated during pager session should flush after close: {flushed_text}"
    );
}
