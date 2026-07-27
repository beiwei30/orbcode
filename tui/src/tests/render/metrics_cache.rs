use crate::tests::support::*;

#[test]
fn render_metrics_fixture_covers_long_transcript() {
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 180);
    let mut fixture = RenderMetricsFixture::new(90, 24);

    let first = fixture.draw(&mut state);
    let second = fixture.draw(&mut state);

    assert!(first.initial_frame);
    assert!(!second.initial_frame);
    assert_eq!(first.buffer_cell_count, 90 * 24);
    assert!(first.output_bytes > 0);
    assert!(second.output_bytes < first.output_bytes);
    assert!(state.transcript_ui.viewport.all_line_count > 24);
    assert_eq!(state.transcript_ui.viewport.lines.len(), 20);
}

#[test]
fn render_metrics_fixture_covers_streaming_assistant_tail() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.request_started_at = Some(Instant::now());
    state.active_thinking = Some(ActiveThinkingState {
        text: "checking render loop and progress state".to_string(),
        is_streaming: true,
        completed_at: None,
    });
    state.pending_assistant = (0..40)
        .map(|index| format!("streamed assistant line {index:02}\n"))
        .collect::<String>();
    let mut fixture = RenderMetricsFixture::new(90, 24);

    let first = fixture.draw(&mut state);
    state
        .pending_assistant
        .push_str("streamed assistant line 40\n");
    let second = fixture.draw(&mut state);

    assert!(first.initial_frame);
    assert!(!second.initial_frame);
    assert_eq!(second.buffer_cell_count, first.buffer_cell_count);
    assert!(second.draw_command_count < second.buffer_cell_count);
    assert!(state.transcript_ui.viewport.all_line_count > 24);
}

#[test]
fn transcript_render_cache_reuses_stable_lines_while_pending_assistant_changes() {
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 160);

    let first = state.transcript_lines_for_messages(90, true);
    state.pending_assistant = "streaming tail frame 1".to_string();
    let second = state.transcript_lines_for_messages(90, true);
    state.pending_assistant = "streaming tail frame 2".to_string();
    let third = state.transcript_lines_for_messages(90, true);

    assert_eq!(state.transcript_ui.render_cache.misses, 1);
    assert_eq!(state.transcript_ui.render_cache.hits, 2);
    assert!(
        plain_text_lines(&second)
            .join("\n")
            .contains("streaming tail frame 1")
    );
    assert!(
        plain_text_lines(&third)
            .join("\n")
            .contains("streaming tail frame 2")
    );
    assert!(first.len() < second.len());
}

#[test]
fn transcript_render_cache_reuses_stable_prefix_while_active_thinking_changes() {
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 160);
    state.active_thinking = Some(ActiveThinkingState {
        text: "thinking frame 0".to_string(),
        is_streaming: true,
        completed_at: None,
    });

    let first = state.transcript_lines_for_messages(90, true);
    if let Some(thinking) = state.active_thinking.as_mut() {
        thinking.text = "thinking frame 1".to_string();
    }
    let second = state.transcript_lines_for_messages(90, true);
    if let Some(thinking) = state.active_thinking.as_mut() {
        thinking.text = "thinking frame 2".to_string();
    }
    let third = state.transcript_lines_for_messages(90, true);

    assert_eq!(state.transcript_ui.render_cache.misses, 1);
    assert_eq!(state.transcript_ui.render_cache.hits, 2);
    assert!(
        plain_text_lines(&first)
            .join("\n")
            .contains("thinking frame 0")
    );
    assert!(
        plain_text_lines(&second)
            .join("\n")
            .contains("thinking frame 1")
    );
    assert!(
        plain_text_lines(&third)
            .join("\n")
            .contains("thinking frame 2")
    );
}

#[test]
fn transcript_render_cache_reuses_stable_prefix_while_live_tool_progress_changes() {
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 160);
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "agent-render-metrics".to_string(),
            name: "Agent".to_string(),
            input: serde_json::json!({
                "description": "Render cache",
                "prompt": "Measure live card cache behavior",
                "subagent_type": "explorer"
            })
            .to_string(),
        }],
    ));
    state.request_in_flight = true;

    state.apply_stream_event(stream_progress_event("live progress frame 0"));
    let first = state.transcript_lines_for_messages(90, true);
    state.apply_stream_event(stream_progress_event("live progress frame 1"));
    let second = state.transcript_lines_for_messages(90, true);
    state.apply_stream_event(stream_progress_event("live progress frame 2"));
    let third = state.transcript_lines_for_messages(90, true);

    assert_eq!(state.transcript_ui.render_cache.misses, 1);
    assert_eq!(state.transcript_ui.render_cache.hits, 2);
    assert!(
        plain_text_lines(&first)
            .join("\n")
            .contains("live progress frame 0")
    );
    assert!(
        plain_text_lines(&second)
            .join("\n")
            .contains("live progress frame 1")
    );
    assert!(
        plain_text_lines(&third)
            .join("\n")
            .contains("live progress frame 2")
    );
}

#[test]
fn transcript_render_cache_invalidates_after_message_append() {
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 24);

    let first = state.transcript_lines_for_messages(90, true);
    let second = state.transcript_lines_for_messages(90, true);
    state.push_message_and_flush_history(TranscriptMessage::new(
        MessageRole::Assistant,
        "new committed assistant message",
    ));
    let third = state.transcript_lines_for_messages(90, true);

    assert_eq!(state.transcript_ui.render_cache.misses, 2);
    assert_eq!(state.transcript_ui.render_cache.hits, 1);
    assert_eq!(first, second);
    assert!(
        plain_text_lines(&third)
            .join("\n")
            .contains("new committed assistant message")
    );
}

#[test]
fn visible_tail_fast_path_uses_partial_all_lines_window() {
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 180);
    state.pending_assistant = "visible streaming tail".to_string();

    let view = state.visible_transcript_lines_for_view(90, 12, true);

    assert_eq!(state.transcript_ui.render_cache.misses, 1);
    assert!(view.all_line_count > view.all_lines.len());
    assert_eq!(view.all_lines, view.visible_lines);
    assert_eq!(view.all_lines_start, view.visible_row_start);
    assert!(view.visible_lines.len() <= 12);
    assert_eq!(view.actual_scroll, 0);
    assert!(view.max_scroll > 0);
    assert!(
        plain_text_lines(&view.visible_lines)
            .join("\n")
            .contains("visible streaming tail")
    );
}

#[test]
fn scrolled_visible_window_fast_path_matches_full_view() {
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 180);
    state.pending_assistant = "hidden while browsing earlier output".to_string();

    let windowed = state.visible_transcript_lines_for_view(90, 12, true);
    let full = state.visible_transcript_lines_for_view_without_window_fast_path(90, 12, true);

    assert_eq!(windowed.visible_lines, full.visible_lines);
    assert_eq!(windowed.visible_row_start, full.visible_row_start);
    assert_eq!(windowed.actual_scroll, full.actual_scroll);
    assert_eq!(windowed.max_scroll, full.max_scroll);
    assert_eq!(windowed.all_line_count, full.all_line_count);
    assert_eq!(windowed.all_lines, windowed.visible_lines);
    assert_eq!(windowed.all_lines_start, windowed.visible_row_start);
    assert!(windowed.all_line_count > windowed.all_lines.len());
}

#[test]
fn selected_scrolled_window_fast_path_matches_full_view_and_selected_text() {
    let area = Rect::new(0, 0, 90, 12);
    let mut probe_state = normal_state("", 0);
    fill_long_transcript(&mut probe_state, 180);
    let probe = probe_state.visible_transcript_lines_for_view_without_window_fast_path(
        area.width as usize,
        area.height as usize,
        true,
    );
    let selection = TranscriptSelectionState {
        area,
        anchor: TranscriptSelectionPoint {
            row: probe.visible_row_start.saturating_sub(3),
            column: 0,
        },
        focus: TranscriptSelectionPoint {
            row: probe.visible_row_start + 4,
            column: 8,
        },
    };

    let mut windowed_state = normal_state("", 0);
    fill_long_transcript(&mut windowed_state, 180);
    windowed_state.pending_assistant = "hidden while selecting earlier output".to_string();
    windowed_state.transcript_ui.viewport.selection = Some(selection.clone());
    let windowed = windowed_state.visible_transcript_lines_for_view(
        area.width as usize,
        area.height as usize,
        true,
    );
    windowed_state.transcript_ui.viewport.sync_with_window(
        area,
        windowed.visible_lines.clone(),
        windowed.all_lines.clone(),
        windowed.all_lines_start,
        windowed.all_line_count,
        windowed.selection_lines.clone(),
        windowed.selection_lines_start,
        windowed.visible_row_start,
        windowed.actual_scroll,
        windowed.max_scroll,
    );

    let mut full_state = normal_state("", 0);
    fill_long_transcript(&mut full_state, 180);
    full_state.pending_assistant = "hidden while selecting earlier output".to_string();
    full_state.transcript_ui.viewport.selection = Some(selection.clone());
    let full = full_state.visible_transcript_lines_for_view_without_window_fast_path(
        area.width as usize,
        area.height as usize,
        true,
    );
    full_state.transcript_ui.viewport.sync_with_window(
        area,
        full.visible_lines.clone(),
        full.all_lines.clone(),
        full.all_lines_start,
        full.all_line_count,
        full.selection_lines.clone(),
        full.selection_lines_start,
        full.visible_row_start,
        full.actual_scroll,
        full.max_scroll,
    );

    assert_eq!(windowed.visible_lines, full.visible_lines);
    assert_eq!(windowed.visible_row_start, full.visible_row_start);
    assert_eq!(windowed.actual_scroll, full.actual_scroll);
    assert_eq!(windowed.max_scroll, full.max_scroll);
    assert_eq!(windowed.all_line_count, full.all_line_count);
    assert!(windowed.all_lines.len() <= area.height as usize + 2);
    assert!(windowed.all_line_count > windowed.all_lines.len());
    assert!(selection.anchor.row < windowed.all_lines_start);
    assert_eq!(windowed.selection_lines_start, selection.anchor.row);
    assert_eq!(
        windowed_state.transcript_ui.viewport.selected_text(),
        full_state.transcript_ui.viewport.selected_text()
    );
    assert!(
        windowed_state
            .transcript_ui
            .viewport
            .selected_text()
            .is_some()
    );
}

#[test]
#[ignore = "manual stress test for stable transcript render cache reuse"]
fn transcript_render_cache_stress_reuses_long_history_during_streaming_tail() {
    const MESSAGE_COUNT: usize = 1_200;
    const FRAME_COUNT: usize = 1_000;

    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, MESSAGE_COUNT);

    let first_started = Instant::now();
    let first = state.transcript_lines_for_messages(100, true);
    let first_duration = first_started.elapsed();

    let cached_started = Instant::now();
    for frame in 0..FRAME_COUNT {
        state.pending_assistant = format!("streaming cached tail frame {frame}");
        let rendered = state.transcript_lines_for_messages(100, true);
        assert!(
            plain_text_lines(&rendered)
                .join("\n")
                .contains(&state.pending_assistant)
        );
    }
    let cached_duration = cached_started.elapsed();

    assert_eq!(state.transcript_ui.render_cache.misses, 1);
    assert_eq!(state.transcript_ui.render_cache.hits, FRAME_COUNT as u64);
    eprintln!(
        "messages={MESSAGE_COUNT} frames={FRAME_COUNT} stable_lines={} cache_hits={} cache_misses={} first_us={} cached_loop_us={}",
        first.len(),
        state.transcript_ui.render_cache.hits,
        state.transcript_ui.render_cache.misses,
        first_duration.as_micros(),
        cached_duration.as_micros()
    );
}

#[test]
#[ignore = "manual stress test for dynamic live tail cache reuse"]
fn transcript_dynamic_tail_cache_stress_reuses_stable_prefix_for_live_changes() {
    const MESSAGE_COUNT: usize = 1_200;
    const FRAME_COUNT: usize = 1_000;
    const VIEW_HEIGHT: usize = 30;

    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, MESSAGE_COUNT);
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "agent-render-metrics".to_string(),
            name: "Agent".to_string(),
            input: serde_json::json!({
                "description": "Render cache",
                "prompt": "Measure stable prefix reuse under live changes",
                "subagent_type": "explorer"
            })
            .to_string(),
        }],
    ));
    state.request_in_flight = true;
    state.active_thinking = Some(ActiveThinkingState {
        text: "thinking dynamic tail frame 0".to_string(),
        is_streaming: true,
        completed_at: None,
    });

    let started = Instant::now();
    let mut last_line_count = 0;
    let mut last_window_len = 0;
    for frame in 0..FRAME_COUNT {
        if let Some(thinking) = state.active_thinking.as_mut() {
            thinking.text = format!("thinking dynamic tail frame {frame}");
        }
        state.apply_stream_event(stream_progress_event(&format!(
            "live progress dynamic tail frame {frame}"
        )));
        let view = state.visible_transcript_lines_for_view(100, VIEW_HEIGHT, true);
        assert_eq!(view.actual_scroll, 0);
        assert!(view.visible_lines.len() <= VIEW_HEIGHT);
        assert_eq!(view.all_lines, view.visible_lines);
        assert!(view.all_line_count > view.all_lines.len());
        let visible = plain_text_lines(&view.visible_lines).join("\n");
        assert!(visible.contains(&format!("thinking dynamic tail frame {frame}")));
        assert!(visible.contains(&format!("live progress dynamic tail frame {frame}")));
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
        "messages={MESSAGE_COUNT} frames={FRAME_COUNT} total_visual_lines={last_line_count} window_lines={last_window_len} cache_hits={} cache_misses={} loop_us={}",
        state.transcript_ui.render_cache.hits,
        state.transcript_ui.render_cache.misses,
        duration.as_micros()
    );
}

#[test]
#[ignore = "manual stress test for bottom-pinned visible transcript tail windowing"]
fn transcript_visible_tail_stress_avoids_full_history_window_clone() {
    const MESSAGE_COUNT: usize = 1_200;
    const FRAME_COUNT: usize = 1_000;
    const VIEW_HEIGHT: usize = 30;

    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, MESSAGE_COUNT);

    let started = Instant::now();
    let mut last_line_count = 0;
    let mut last_window_len = 0;
    for frame in 0..FRAME_COUNT {
        state.pending_assistant = format!("streaming visible tail frame {frame}");
        let view = state.visible_transcript_lines_for_view(100, VIEW_HEIGHT, true);
        assert_eq!(view.actual_scroll, 0);
        assert!(view.visible_lines.len() <= VIEW_HEIGHT);
        assert_eq!(view.all_lines, view.visible_lines);
        assert!(view.all_line_count > view.all_lines.len());
        assert!(
            plain_text_lines(&view.visible_lines)
                .join("\n")
                .contains(&state.pending_assistant)
        );
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
        "messages={MESSAGE_COUNT} frames={FRAME_COUNT} total_visual_lines={last_line_count} window_lines={last_window_len} cache_hits={} cache_misses={} loop_us={}",
        state.transcript_ui.render_cache.hits,
        state.transcript_ui.render_cache.misses,
        duration.as_micros()
    );
}

#[test]
#[ignore = "manual stress test for selected visible transcript windowing"]
fn transcript_selected_window_stress_preserves_selected_text_without_full_history_clone() {
    const MESSAGE_COUNT: usize = 1_200;
    const FRAME_COUNT: usize = 1_000;
    const VIEW_WIDTH: usize = 100;
    const VIEW_HEIGHT: usize = 30;
    const REQUESTED_SCROLL: usize = 200;

    let area = Rect::new(0, 0, VIEW_WIDTH as u16, VIEW_HEIGHT as u16);
    let mut probe_state = normal_state("", 0);
    fill_long_transcript(&mut probe_state, MESSAGE_COUNT);
    probe_state.pending_assistant = "streaming selected tail frame 0".to_string();
    let probe = probe_state.visible_transcript_lines_for_view_without_window_fast_path(
        VIEW_WIDTH,
        VIEW_HEIGHT,
        true,
    );
    let selection_start = probe.visible_row_start.saturating_sub(20);
    let selection_end =
        (probe.visible_row_start + VIEW_HEIGHT + 20).min(probe.all_line_count.saturating_sub(1));
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

    let mut full_state = normal_state("", 0);
    fill_long_transcript(&mut full_state, MESSAGE_COUNT);
    full_state.pending_assistant = "streaming selected tail frame 0".to_string();
    full_state.transcript_ui.viewport.selection = Some(selection.clone());
    let full = full_state.visible_transcript_lines_for_view_without_window_fast_path(
        VIEW_WIDTH,
        VIEW_HEIGHT,
        true,
    );
    full_state.transcript_ui.viewport.sync_with_window(
        area,
        full.visible_lines,
        full.all_lines,
        full.all_lines_start,
        full.all_line_count,
        full.selection_lines,
        full.selection_lines_start,
        full.visible_row_start,
        full.actual_scroll,
        full.max_scroll,
    );
    let expected_selected = full_state
        .transcript_ui
        .viewport
        .selected_text()
        .expect("full selected text");

    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, MESSAGE_COUNT);
    state.transcript_ui.viewport.selection = Some(selection);

    let started = Instant::now();
    let mut last_line_count = 0;
    let mut last_window_len = 0;
    let mut last_selection_window_len = 0;
    for frame in 0..FRAME_COUNT {
        state.pending_assistant = format!("streaming selected tail frame {frame}");
        let view = state.visible_transcript_lines_for_view(VIEW_WIDTH, VIEW_HEIGHT, true);
        assert_eq!(view.actual_scroll, REQUESTED_SCROLL.min(view.max_scroll));
        assert!(view.visible_lines.len() <= VIEW_HEIGHT);
        assert!(view.all_lines.len() <= VIEW_HEIGHT + 2);
        assert!(view.all_line_count > view.all_lines.len());
        assert!(view.all_line_count > view.selection_lines.len());
        last_line_count = view.all_line_count;
        last_window_len = view.all_lines.len();
        last_selection_window_len = view.selection_lines.len();
        state.transcript_ui.viewport.sync_with_window(
            area,
            view.visible_lines,
            view.all_lines,
            view.all_lines_start,
            view.all_line_count,
            view.selection_lines,
            view.selection_lines_start,
            view.visible_row_start,
            view.actual_scroll,
            view.max_scroll,
        );
        assert_eq!(
            state.transcript_ui.viewport.selected_text().as_deref(),
            Some(expected_selected.as_str())
        );
    }
    let duration = started.elapsed();

    assert_eq!(state.transcript_ui.render_cache.misses, 1);
    assert_eq!(
        state.transcript_ui.render_cache.hits,
        (FRAME_COUNT - 1) as u64
    );
    eprintln!(
        "messages={MESSAGE_COUNT} frames={FRAME_COUNT} requested_scroll={REQUESTED_SCROLL} total_visual_lines={last_line_count} window_lines={last_window_len} selection_window_lines={last_selection_window_len} cache_hits={} cache_misses={} loop_us={}",
        state.transcript_ui.render_cache.hits,
        state.transcript_ui.render_cache.misses,
        duration.as_micros()
    );
}

#[test]
fn render_metrics_fixture_keeps_hidden_live_progress_output_bounded() {
    let mut short_state = live_progress_metric_state(progress_statuses(0));
    let mut long_state = live_progress_metric_state(progress_statuses(120));
    let mut short_fixture = RenderMetricsFixture::new(90, 24);
    let mut long_fixture = RenderMetricsFixture::new(90, 24);

    let short = short_fixture.draw(&mut short_state);
    let long = long_fixture.draw(&mut long_state);

    assert_eq!(
        short.output_bytes, long.output_bytes,
        "hidden cumulative progress should not change rendered output bytes"
    );
    assert_eq!(
        short.draw_command_count, long.draw_command_count,
        "hidden cumulative progress should not change rendered command count"
    );
    assert_eq!(
        long_state
            .live_tool_cells
            .activities
            .iter()
            .map(|activity| activity.progress_messages.len())
            .sum::<usize>(),
        LIVE_TOOL_PROGRESS_MESSAGE_LIMIT
    );
}

#[test]
#[ignore = "manual stress test for long-session live-render memory budgets"]
fn long_session_memory_budget_stress_keeps_live_render_state_bounded() {
    const TOOL_RUNS: usize = 200;
    const HOOK_NOTES: usize = 200;
    const PROGRESS_PER_TOOL: usize = LIVE_TOOL_PROGRESS_MESSAGE_LIMIT * 2;
    const VIEW_HEIGHT: usize = 30;

    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    for index in 0..TOOL_RUNS {
        apply_long_session_tool_run(&mut state, index, PROGRESS_PER_TOOL);
    }
    for index in 0..HOOK_NOTES {
        let event_name = format!("LongSessionStressHook{index}");
        for _ in 0..3 {
            assert!(!state.apply_stream_event(hook_progress_event(&event_name, index as u64)));
        }
        assert!(!state.apply_stream_event(hook_notice_event(&event_name)));
    }

    let progress_lengths = completed_tool_result_progress_lengths(&state);
    let view = state.visible_transcript_lines_for_view(100, VIEW_HEIGHT, true);
    let transcript_cell_count = state.transcript_ui.cells.len();
    let cache_line_count = state.transcript_ui.render_cache.lines.len();
    let cache_visual_line_count = state.transcript_ui.render_cache.visual_lines.len();
    let attached_hook_progress = attached_hook_progress_count(&state);

    assert!(state.live_tool_cells.is_empty());
    assert!(state.in_progress_tool_use_ids.is_empty());
    assert!(state.pending_hook_progress.is_empty());
    assert_eq!(state.hook_progress_by_message_id.len(), HOOK_NOTES);
    assert_eq!(attached_hook_progress, HOOK_NOTES);
    assert_eq!(progress_lengths.len(), TOOL_RUNS);
    assert!(
        progress_lengths
            .iter()
            .all(|len| *len == LIVE_TOOL_PROGRESS_MESSAGE_LIMIT)
    );
    assert!(view.visible_lines.len() <= VIEW_HEIGHT);
    assert_eq!(view.all_lines, view.visible_lines);
    assert!(view.all_line_count > view.all_lines.len());

    eprintln!(
        "tool_runs={TOOL_RUNS} hook_notes={HOOK_NOTES} messages={} transcript_cells={transcript_cell_count} cache_lines={cache_line_count} cache_visual_lines={cache_visual_line_count} window_lines={} total_visual_lines={} result_progress_messages={} live_tool_cells={} pending_hook_progress={} attached_hook_progress={attached_hook_progress}",
        state.messages.len(),
        view.visible_lines.len(),
        view.all_line_count,
        progress_lengths.iter().sum::<usize>(),
        state.live_tool_cells.len(),
        state.pending_hook_progress.len(),
    );
}

#[test]
fn render_metrics_fixture_covers_permission_mouse_drag_selection() {
    let mut state = permission_render_metrics_state();
    let mut fixture = RenderMetricsFixture::new(100, 30);

    let _ = fixture.draw(&mut state);
    let transcript_area = state.transcript_ui.viewport.area;
    let permission_area = match state.overlay.as_ref() {
        Some(OverlayState::PermissionRequest(permission)) => permission.viewport.area,
        _ => panic!("expected permission overlay"),
    };
    assert!(transcript_area.height > 0);
    assert!(permission_area.height > 2);

    state.handle_mouse(mouse_event(
        MouseEventKind::ScrollUp,
        permission_area.x.saturating_add(1),
        permission_area.y,
    ));
    let _ = fixture.draw(&mut state);
    let scrolled_panel_scroll = match state.overlay.as_ref() {
        Some(OverlayState::PermissionRequest(permission)) => permission.panel_scroll,
        _ => 0,
    };
    assert!(scrolled_panel_scroll > 0);

    state.handle_mouse(mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        transcript_area.x.saturating_add(1),
        transcript_area.y,
    ));
    state.handle_mouse(mouse_event(
        MouseEventKind::Drag(MouseButton::Left),
        permission_area.x.saturating_add(2),
        permission_area.bottom().saturating_sub(1),
    ));
    let drag = fixture.draw(&mut state);
    let (permission_selected, panel_scroll) = match state.overlay.as_ref() {
        Some(OverlayState::PermissionRequest(permission)) => (
            permission.viewport.selection.is_some(),
            permission.panel_scroll,
        ),
        _ => (false, 0),
    };

    assert!(state.transcript_ui.viewport.selection.is_some());
    assert!(permission_selected);
    assert!(panel_scroll < scrolled_panel_scroll);
    assert!(!drag.initial_frame);
    assert!(drag.output_bytes > 0);
    assert!(drag.draw_command_count < drag.buffer_cell_count);
}

#[test]
fn render_metrics_fixture_covers_cached_picker_and_slash_views() {
    let model_state = state_with_status_overlay(OverlayState::ModelPicker(ModelPickerState::new(
        "/model",
        synthetic_model_options(120, 24),
        Some(EffortLevel::Medium),
    )));
    assert_render_metrics_update_bounded("model picker", model_state, |state| {
        if let Some(OverlayState::ModelPicker(picker)) = state.overlay.as_mut() {
            picker.selected = 25;
        } else {
            panic!("expected model picker overlay");
        }
    });

    let theme_state = state_with_status_overlay(OverlayState::ThemePicker(ThemePickerState::new(
        "/theme",
        ThemeSetting::Auto,
    )));
    assert_render_metrics_update_bounded("theme picker", theme_state, |state| {
        if let Some(OverlayState::ThemePicker(picker)) = state.overlay.as_mut() {
            picker.selected = 1;
        } else {
            panic!("expected theme picker overlay");
        }
    });

    let output_style_state = state_with_status_overlay(OverlayState::OutputStylePicker(
        OutputStylePickerState::new(
            "/output-style",
            synthetic_output_style_options(80, 0),
            false,
        ),
    ));
    assert_render_metrics_update_bounded("output style picker", output_style_state, |state| {
        if let Some(OverlayState::OutputStylePicker(picker)) = state.overlay.as_mut() {
            picker.selected = 1;
        } else {
            panic!("expected output style picker overlay");
        }
    });

    let config_options = synthetic_config_options(140);
    let config_state = state_with_status_overlay(OverlayState::ConfigPicker(ConfigPickerState {
        command: "/config".to_string(),
        output_style: "default".to_string(),
        all_options: config_options.clone(),
        options: config_options,
        selected: 32,
        query: String::new(),
        searching: true,
        lines_cache: ConfigPickerLinesCache::default(),
    }));
    assert_render_metrics_update_bounded("config picker", config_state, |state| {
        if let Some(OverlayState::ConfigPicker(picker)) = state.overlay.as_mut() {
            picker.selected = 33;
        } else {
            panic!("expected config picker overlay");
        }
    });

    let mut sandbox_picker = SandboxPickerState::new("/sandbox", large_sandbox_settings());
    sandbox_picker.tab = SandboxTab::Config;
    let sandbox_state = state_with_status_overlay(OverlayState::SandboxPicker(sandbox_picker));
    assert_render_metrics_update_bounded("sandbox picker", sandbox_state, |state| {
        if let Some(OverlayState::SandboxPicker(picker)) = state.overlay.as_mut() {
            picker.tab = SandboxTab::Overrides;
        } else {
            panic!("expected sandbox picker overlay");
        }
    });

    let cwd = PathBuf::from("/tmp/project");
    let mut memory_picker = MemoryPickerState::new("/memory", large_memory_overview(&cwd));
    memory_picker.selected = 32;
    let mut memory_state = state_with_status_overlay(OverlayState::MemoryPicker(memory_picker));
    memory_state.cwd = cwd;
    assert_render_metrics_update_bounded("memory picker", memory_state, |state| {
        if let Some(OverlayState::MemoryPicker(picker)) = state.overlay.as_mut() {
            picker.selected = 33;
        } else {
            panic!("expected memory picker overlay");
        }
    });

    let mut slash_state = normal_state("/", 1);
    fill_long_transcript(&mut slash_state, 80);
    assert_render_metrics_update_bounded("slash suggestions", slash_state, |state| {
        state.slash_command_selected = 1;
    });
}
