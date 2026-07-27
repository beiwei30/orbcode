use crate::tests::support::*;

#[test]
fn stream_event_batch_coalesces_progress_before_redraw() {
    let mut state = normal_state("", 0);
    let (tx, rx) = mpsc::unbounded_channel();
    tx.send(stream_progress_event("second progress"))
        .expect("queue second progress");
    tx.send(stream_progress_event("third progress"))
        .expect("queue third progress");
    let mut turn_events = Some(rx);
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    handle_stream_event_batch(
        &mut state,
        &mut turn_events,
        Some(stream_progress_event("first progress")),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );

    assert!(turn_events.is_some());
    assert!(needs_redraw);
    assert_eq!(event_counts.stream_events, 3);
    assert_eq!(redraw_reasons, vec!["stream_event"]);
    assert!(
        state
            .latest_live_tool_activity()
            .is_some_and(|a| a.status_line.contains("third progress"))
    );
    assert_eq!(
        state
            .live_tool_cells
            .activities
            .iter()
            .map(|activity| activity.progress_messages.len())
            .sum::<usize>(),
        3
    );
    drop(tx);
}

#[test]
fn request_started_breaks_stream_batch_for_upload_status_render() {
    let mut state = normal_state("", 0);
    state.apply_stream_event(StreamEvent::UserMessage {
        message: TranscriptMessage::new(
            MessageRole::User,
            "Compare the spinner upload and download token status.",
        ),
    });
    let (tx, rx) = mpsc::unbounded_channel();
    tx.send(StreamEvent::ThinkingStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
    })
    .expect("queue thinking start");
    let mut turn_events = Some(rx);
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    handle_stream_event_batch(
        &mut state,
        &mut turn_events,
        Some(StreamEvent::RequestStarted {
            session_id: "session".to_string(),
            provider: ProviderId::Anthropic,
            fallback_provider: None,
            context: TurnContext::default(),
        }),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );

    assert!(turn_events.is_some());
    assert!(needs_redraw);
    assert_eq!(event_counts.stream_events, 1);
    assert_eq!(redraw_reasons, vec!["stream_event"]);
    assert_eq!(state.request_token_direction, RequestTokenDirection::Up);
    let rendered = plain_text_lines(&state.request_status_lines()).join("\n");
    assert!(rendered.contains("↑ "), "{rendered}");
    assert!(!rendered.contains("↓ "), "{rendered}");
    drop(tx);
}

#[test]
fn live_tool_progress_retains_recent_messages_only() {
    let mut state = normal_state("", 0);
    for index in 0..(LIVE_TOOL_PROGRESS_MESSAGE_LIMIT + 12) {
        let finished =
            state.apply_stream_event(stream_progress_event(&format!("bounded progress {index}")));
        assert!(!finished);
    }

    let progress_messages = &state
        .latest_live_tool_activity()
        .expect("live activity should exist")
        .progress_messages;

    assert_eq!(progress_messages.len(), LIVE_TOOL_PROGRESS_MESSAGE_LIMIT);
    assert_eq!(
        progress_messages
            .first()
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str),
        Some("bounded progress 12")
    );
    assert_eq!(
        progress_messages
            .last()
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str),
        Some("bounded progress 75")
    );
}

#[test]
fn duplicate_tool_progress_does_not_request_redraw() {
    let mut state = normal_state("", 0);
    state.apply_stream_event(stream_progress_event("stable progress"));
    let (tx, rx) = mpsc::unbounded_channel();
    let mut turn_events = Some(rx);
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    handle_stream_event_batch(
        &mut state,
        &mut turn_events,
        Some(stream_progress_event("stable progress")),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );

    assert!(turn_events.is_some());
    assert!(!needs_redraw);
    assert_eq!(event_counts.stream_events, 1);
    assert!(redraw_reasons.is_empty());
    assert_eq!(
        state
            .latest_live_tool_activity()
            .map(|activity| activity.progress_messages.len()),
        Some(1)
    );
    drop(tx);
}

#[test]
fn duplicate_then_changed_tool_progress_requests_one_redraw() {
    let mut state = normal_state("", 0);
    state.apply_stream_event(stream_progress_event("stable progress"));
    let (tx, rx) = mpsc::unbounded_channel();
    tx.send(stream_progress_event("new progress"))
        .expect("queue changed progress");
    let mut turn_events = Some(rx);
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    handle_stream_event_batch(
        &mut state,
        &mut turn_events,
        Some(stream_progress_event("stable progress")),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );

    assert!(needs_redraw);
    assert_eq!(event_counts.stream_events, 2);
    assert_eq!(redraw_reasons, vec!["stream_event"]);
    assert!(
        state
            .latest_live_tool_activity()
            .is_some_and(|a| a.status_line.contains("new progress"))
    );
    assert_eq!(
        state
            .latest_live_tool_activity()
            .map(|activity| activity.progress_messages.len()),
        Some(2)
    );
    drop(tx);
}

#[test]
fn duplicate_hook_progress_does_not_request_redraw() {
    let mut state = normal_state("", 0);
    state.apply_stream_event(hook_notice_event("Stop"));
    state.apply_stream_event(hook_progress_event("Stop", 4));
    let (tx, rx) = mpsc::unbounded_channel();
    let mut turn_events = Some(rx);
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    handle_stream_event_batch(
        &mut state,
        &mut turn_events,
        Some(hook_progress_event("Stop", 4)),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );

    assert!(turn_events.is_some());
    assert!(!needs_redraw);
    assert_eq!(event_counts.stream_events, 1);
    assert!(redraw_reasons.is_empty());
    assert_eq!(attached_hook_progress_count(&state), 1);
    drop(tx);
}

#[test]
fn duplicate_then_changed_hook_progress_requests_one_redraw() {
    let mut state = normal_state("", 0);
    state.apply_stream_event(hook_notice_event("Stop"));
    state.apply_stream_event(hook_progress_event("Stop", 4));
    let (tx, rx) = mpsc::unbounded_channel();
    tx.send(hook_progress_event("Stop", 5))
        .expect("queue changed hook progress");
    let mut turn_events = Some(rx);
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    handle_stream_event_batch(
        &mut state,
        &mut turn_events,
        Some(hook_progress_event("Stop", 4)),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );

    assert!(needs_redraw);
    assert_eq!(event_counts.stream_events, 2);
    assert_eq!(redraw_reasons, vec!["stream_event"]);
    assert_eq!(attached_hook_progress_count(&state), 2);
    let transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(transcript.contains("completed in 5 ms (exit 0)"));
    drop(tx);
}

#[test]
fn stream_event_batch_drains_until_turn_finished() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.request_started_at = Some(Instant::now());
    let (tx, rx) = mpsc::unbounded_channel();
    tx.send(StreamEvent::TurnFinished {
        session_id: "session".to_string(),
        provider: ProviderId::OpenAi,
        fallback_from: None,
        usage: TokenUsage::default(),
    })
    .expect("queue turn finished");
    tx.send(stream_progress_event("ignored after finish"))
        .expect("queue post-finish progress");
    let mut turn_events = Some(rx);
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    handle_stream_event_batch(
        &mut state,
        &mut turn_events,
        Some(stream_progress_event("last progress before finish")),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );

    assert!(turn_events.is_none());
    assert!(needs_redraw);
    assert_eq!(event_counts.stream_events, 2);
    assert_eq!(redraw_reasons, vec!["stream_event", "stream_finished"]);
    assert!(!state.request_in_flight);
    drop(tx);
}

#[test]
#[ignore = "manual stress test for bursty stream progress redraw coalescing"]
fn stream_event_batch_stress_coalesces_large_progress_burst() {
    const EVENT_COUNT: usize = 10_000;

    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.request_started_at = Some(Instant::now());
    let (tx, rx) = mpsc::unbounded_channel();
    for index in 1..EVENT_COUNT {
        tx.send(stream_progress_event(&format!("stress progress {index}")))
            .expect("queue stress progress");
    }
    let mut turn_events = Some(rx);
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    let batch_started = Instant::now();
    handle_stream_event_batch(
        &mut state,
        &mut turn_events,
        Some(stream_progress_event("stress progress 0")),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );
    let batch_duration = batch_started.elapsed();

    let mut fixture = RenderMetricsFixture::new(100, 30);
    let draw_started = Instant::now();
    let first_draw = fixture.draw(&mut state);
    let draw_duration = draw_started.elapsed();
    let second_draw = fixture.draw(&mut state);
    let retained_progress = state
        .live_tool_cells
        .activities
        .iter()
        .map(|activity| activity.progress_messages.len())
        .sum::<usize>();

    assert!(turn_events.is_some());
    assert!(needs_redraw);
    assert_eq!(event_counts.stream_events, EVENT_COUNT as u64);
    assert_eq!(redraw_reasons, vec!["stream_event"]);
    assert!(
        state
            .latest_live_tool_activity()
            .is_some_and(|a| a.status_line.contains("stress progress 9999"))
    );
    assert_eq!(retained_progress, LIVE_TOOL_PROGRESS_MESSAGE_LIMIT);
    assert!(first_draw.initial_frame);
    assert_eq!(first_draw.buffer_cell_count, 100 * 30);
    assert!(first_draw.output_bytes > 0);
    assert!(!second_draw.initial_frame);
    assert!(second_draw.draw_command_count < second_draw.buffer_cell_count);
    eprintln!(
        "events={EVENT_COUNT} retained_progress={retained_progress} batch_us={} draw_wall_us={} draw_total_us={} first_commands={} first_bytes={} second_commands={} second_bytes={}",
        batch_duration.as_micros(),
        draw_duration.as_micros(),
        first_draw.total_duration_us,
        first_draw.draw_command_count,
        first_draw.output_bytes,
        second_draw.draw_command_count,
        second_draw.output_bytes
    );
    drop(tx);
}

#[test]
#[ignore = "manual stress test for duplicate progress redraw suppression"]
fn stream_event_batch_stress_skips_duplicate_progress_redraws() {
    const EVENT_COUNT: usize = 10_000;

    let mut state = normal_state("", 0);
    state.apply_stream_event(stream_progress_event("unchanged progress"));
    let (tx, rx) = mpsc::unbounded_channel();
    for _ in 1..EVENT_COUNT {
        tx.send(stream_progress_event("unchanged progress"))
            .expect("queue duplicate progress");
    }
    let mut turn_events = Some(rx);
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    let batch_started = Instant::now();
    handle_stream_event_batch(
        &mut state,
        &mut turn_events,
        Some(stream_progress_event("unchanged progress")),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );
    let batch_duration = batch_started.elapsed();
    let retained_progress = state
        .live_tool_cells
        .activities
        .iter()
        .map(|activity| activity.progress_messages.len())
        .sum::<usize>();

    assert!(turn_events.is_some());
    assert!(!needs_redraw);
    assert_eq!(event_counts.stream_events, EVENT_COUNT as u64);
    assert!(redraw_reasons.is_empty());
    assert_eq!(retained_progress, 1);
    eprintln!(
        "events={EVENT_COUNT} retained_progress={retained_progress} redraw=false batch_us={}",
        batch_duration.as_micros()
    );
    drop(tx);
}

#[test]
#[ignore = "manual stress test for duplicate hook progress redraw suppression"]
fn hook_progress_stress_skips_duplicate_redraws() {
    const EVENT_COUNT: usize = 10_000;

    let mut state = normal_state("", 0);
    state.apply_stream_event(hook_notice_event("Stop"));
    state.apply_stream_event(hook_progress_event("Stop", 4));
    let (tx, rx) = mpsc::unbounded_channel();
    for _ in 1..EVENT_COUNT {
        tx.send(hook_progress_event("Stop", 4))
            .expect("queue duplicate hook progress");
    }
    let mut turn_events = Some(rx);
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    let batch_started = Instant::now();
    handle_stream_event_batch(
        &mut state,
        &mut turn_events,
        Some(hook_progress_event("Stop", 4)),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );
    let batch_duration = batch_started.elapsed();
    let retained_progress = attached_hook_progress_count(&state);

    assert!(turn_events.is_some());
    assert!(!needs_redraw);
    assert_eq!(event_counts.stream_events, EVENT_COUNT as u64);
    assert!(redraw_reasons.is_empty());
    assert_eq!(retained_progress, 1);
    eprintln!(
        "events={EVENT_COUNT} retained_hook_progress={retained_progress} redraw=false batch_us={}",
        batch_duration.as_micros()
    );
    drop(tx);
}

#[test]
fn local_command_event_for_redraw_counts_without_duplicate_reason() {
    let mut state = normal_state("", 0);
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    let first_prompt = apply_local_command_event_for_redraw(
        &mut state,
        LocalCommandEvent::InstructionsFinished(Ok("system prompt".to_string())),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );
    let second_prompt = apply_local_command_event_for_redraw(
        &mut state,
        LocalCommandEvent::InstructionsFinished(Ok("updated prompt".to_string())),
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );

    assert!(first_prompt.is_none());
    assert!(second_prompt.is_none());
    assert!(needs_redraw);
    assert_eq!(event_counts.local_command_events, 2);
    assert_eq!(redraw_reasons, vec!["local_command_event"]);
}

#[test]
fn stale_session_local_command_envelope_is_dropped() {
    let mut state = normal_state("", 0);
    // `normal_state` sets session_id = "session".
    let mut event_counts = RenderEventCounts::default();
    let mut needs_redraw = false;
    let mut redraw_reasons = Vec::new();

    // A result launched for a different (now-left) session must be ignored so it
    // does not land in the current session's transcript.
    let stale = LocalCommandEnvelope::new(
        "other-session",
        LocalCommandEvent::InstructionsFinished(Ok("stale prompt".to_string())),
    );
    let prompt = apply_local_command_envelope_for_redraw(
        &mut state,
        stale,
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );
    assert!(
        prompt.is_none(),
        "a stale-session envelope produces no prompt"
    );
    assert!(
        !needs_redraw,
        "a dropped stale envelope must not force a redraw"
    );
    assert!(redraw_reasons.is_empty());

    // A matching-origin envelope is applied normally.
    let current = LocalCommandEnvelope::new(
        state.session_id.clone(),
        LocalCommandEvent::InstructionsFinished(Ok("current prompt".to_string())),
    );
    let _ = apply_local_command_envelope_for_redraw(
        &mut state,
        current,
        &mut event_counts,
        &mut needs_redraw,
        &mut redraw_reasons,
    );
    assert!(
        needs_redraw,
        "a matching-origin envelope applies and redraws"
    );
    assert_eq!(redraw_reasons, vec!["local_command_event"]);
}

#[test]
fn regression_budget_long_transcript_visible_windows_stay_bounded() {
    const MESSAGE_COUNT: usize = 420;
    const VIEW_WIDTH: usize = 90;
    const VIEW_HEIGHT: usize = 16;
    let mut bottom_state = normal_state("", 0);
    fill_long_transcript(&mut bottom_state, MESSAGE_COUNT);
    bottom_state.pending_assistant = "visible streaming tail budget".to_string();
    let bottom = bottom_state.visible_transcript_lines_for_view(VIEW_WIDTH, VIEW_HEIGHT, true);

    assert_eq!(bottom.actual_scroll, 0);
    assert!(bottom.visible_lines.len() <= VIEW_HEIGHT);
    assert_eq!(bottom.all_lines, bottom.visible_lines);
    assert!(bottom.all_line_count > bottom.all_lines.len());

    let mut scrolled_state = normal_state("", 0);
    fill_long_transcript(&mut scrolled_state, MESSAGE_COUNT);
    scrolled_state.pending_assistant = "hidden streaming tail budget".to_string();
    let scrolled = scrolled_state.visible_transcript_lines_for_view(VIEW_WIDTH, VIEW_HEIGHT, true);

    assert_eq!(scrolled.actual_scroll, 0);
    assert!(scrolled.visible_lines.len() <= VIEW_HEIGHT);
    assert_eq!(scrolled.all_lines, scrolled.visible_lines);
    assert!(scrolled.all_line_count > scrolled.all_lines.len());

    let area = Rect::new(0, 0, VIEW_WIDTH as u16, VIEW_HEIGHT as u16);
    let mut probe_state = normal_state("", 0);
    fill_long_transcript(&mut probe_state, MESSAGE_COUNT);
    let probe = probe_state.visible_transcript_lines_for_view_without_window_fast_path(
        VIEW_WIDTH,
        VIEW_HEIGHT,
        true,
    );
    let selection_start = probe.visible_row_start.saturating_sub(8);
    let selection_end =
        (probe.visible_row_start + VIEW_HEIGHT + 8).min(probe.all_line_count.saturating_sub(1));
    let selection = TranscriptSelectionState {
        area,
        anchor: TranscriptSelectionPoint {
            row: selection_start,
            column: 0,
        },
        focus: TranscriptSelectionPoint {
            row: selection_end,
            column: 8,
        },
    };
    let selection_span = selection_end.saturating_sub(selection_start) + 1;

    let mut selected_state = normal_state("", 0);
    fill_long_transcript(&mut selected_state, MESSAGE_COUNT);
    selected_state.pending_assistant = "hidden selected tail budget".to_string();
    selected_state.transcript_ui.viewport.selection = Some(selection);
    let selected = selected_state.visible_transcript_lines_for_view(VIEW_WIDTH, VIEW_HEIGHT, true);

    assert_eq!(selected.actual_scroll, 0);
    assert!(selected.visible_lines.len() <= VIEW_HEIGHT);
    assert!(selected.all_lines.len() <= VIEW_HEIGHT + 2);
    assert!(selected.selection_lines.len() <= selection_span);
    assert!(selected.all_line_count > selected.all_lines.len());
}

#[test]
fn regression_budget_cumulative_live_progress_output_and_state_stay_bounded() {
    let progress_counts = [
        0,
        LIVE_TOOL_PROGRESS_MESSAGE_LIMIT,
        LIVE_TOOL_PROGRESS_MESSAGE_LIMIT * 8,
    ];
    let mut expected_output = None;

    for hidden_count in progress_counts {
        let mut state = live_progress_metric_state(progress_statuses(hidden_count));
        let mut fixture = RenderMetricsFixture::new(90, 24);
        let draw = fixture.draw(&mut state);
        let retained_progress = state
            .live_tool_cells
            .activities
            .iter()
            .map(|activity| activity.progress_messages.len())
            .sum::<usize>();

        assert!(retained_progress <= LIVE_TOOL_PROGRESS_MESSAGE_LIMIT);
        if hidden_count >= LIVE_TOOL_PROGRESS_MESSAGE_LIMIT {
            assert_eq!(retained_progress, LIVE_TOOL_PROGRESS_MESSAGE_LIMIT);
        }

        if let Some((output_bytes, draw_command_count)) = expected_output {
            assert_eq!(
                draw.output_bytes, output_bytes,
                "hidden_count={hidden_count} changed terminal output bytes"
            );
            assert_eq!(
                draw.draw_command_count, draw_command_count,
                "hidden_count={hidden_count} changed terminal draw command count"
            );
        } else {
            expected_output = Some((draw.output_bytes, draw.draw_command_count));
        }
    }
}

#[test]
fn regression_budget_long_session_completed_tool_state_is_pruned() {
    const TOOL_RUNS: usize = 48;
    const PROGRESS_PER_TOOL: usize = LIVE_TOOL_PROGRESS_MESSAGE_LIMIT * 2 + 7;
    const VIEW_HEIGHT: usize = 24;

    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    for index in 0..TOOL_RUNS {
        apply_long_session_tool_run(&mut state, index, PROGRESS_PER_TOOL);
        let _history = state.take_history_lines(100, VIEW_HEIGHT as u16);
        assert!(state.live_tool_cells.is_empty());
        assert!(state.in_progress_tool_use_ids.is_empty());
    }

    let progress_lengths = completed_tool_result_progress_lengths(&state);
    assert_eq!(progress_lengths.len(), TOOL_RUNS);
    assert!(
        progress_lengths
            .iter()
            .all(|len| *len == LIVE_TOOL_PROGRESS_MESSAGE_LIMIT)
    );
    assert_eq!(
        progress_lengths.iter().sum::<usize>(),
        TOOL_RUNS * LIVE_TOOL_PROGRESS_MESSAGE_LIMIT
    );

    let view = state.visible_transcript_lines_for_view(100, VIEW_HEIGHT, true);
    assert!(view.visible_lines.len() <= VIEW_HEIGHT);
    assert_eq!(view.all_lines, view.visible_lines);

    let cached_line_count = state.transcript_ui.render_cache.lines.len();
    let cached_visual_line_count = state.transcript_ui.render_cache.visual_lines.len();
    let transcript_cell_count = state.transcript_ui.cells.len();
    let second = state.visible_transcript_lines_for_view(100, VIEW_HEIGHT, true);

    assert_eq!(state.transcript_ui.render_cache.misses, 0);
    assert_eq!(state.transcript_ui.render_cache.hits, 0);
    assert_eq!(
        state.transcript_ui.render_cache.lines.len(),
        cached_line_count
    );
    assert_eq!(
        state.transcript_ui.render_cache.visual_lines.len(),
        cached_visual_line_count
    );
    assert_eq!(state.transcript_ui.cells.len(), transcript_cell_count);
    assert_eq!(second.all_lines, second.visible_lines);
    assert!(second.visible_lines.len() <= VIEW_HEIGHT);
}

#[test]
fn regression_budget_long_session_hook_progress_state_is_bounded_per_note() {
    const HOOK_NOTES: usize = 72;
    const DUPLICATE_PROGRESS_EVENTS: usize = 5;
    const VIEW_HEIGHT: usize = 24;

    let mut state = normal_state("", 0);
    for index in 0..HOOK_NOTES {
        let event_name = format!("LongSessionHook{index}");
        for _ in 0..DUPLICATE_PROGRESS_EVENTS {
            assert!(!state.apply_stream_event(hook_progress_event(&event_name, index as u64)));
            assert_eq!(state.pending_hook_progress.len(), 1);
        }

        assert!(!state.apply_stream_event(hook_notice_event(&event_name)));
        assert!(state.pending_hook_progress.is_empty());
        assert_eq!(state.hook_progress_by_message_id.len(), index + 1);
        assert_eq!(attached_hook_progress_count(&state), index + 1);

        assert!(!state.apply_stream_event(hook_progress_event(&event_name, index as u64)));
        assert_eq!(attached_hook_progress_count(&state), index + 1);
    }

    assert!(state.pending_hook_progress.is_empty());
    assert_eq!(state.hook_progress_by_message_id.len(), HOOK_NOTES);
    assert!(
        state
            .hook_progress_by_message_id
            .values()
            .all(|progress| progress.len() == 1)
    );

    let view = state.visible_transcript_lines_for_view(100, VIEW_HEIGHT, true);
    assert!(view.visible_lines.len() <= VIEW_HEIGHT);
    assert_eq!(view.all_lines, view.visible_lines);
    assert!(view.all_line_count > view.all_lines.len());
}
