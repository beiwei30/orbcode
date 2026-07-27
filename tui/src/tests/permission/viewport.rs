use crate::tests::support::*;

#[test]
fn render_metrics_fixture_covers_large_permission_panel() {
    let mut state = permission_render_metrics_state();
    let mut fixture = RenderMetricsFixture::new(100, 30);

    let first = fixture.draw(&mut state);
    let permission_area = match state.overlay.as_ref() {
        Some(OverlayState::PermissionRequest(permission)) => {
            assert!(
                permission.viewport.all_lines.len() > permission.viewport.lines.len(),
                "long Agent prompt should make the permission panel scrollable"
            );
            assert!(permission.viewport.max_scroll > 0);
            permission.viewport.area
        }
        _ => panic!("expected permission overlay"),
    };

    state.handle_mouse(mouse_event(
        MouseEventKind::ScrollUp,
        permission_area.x.saturating_add(1),
        permission_area.y,
    ));
    let scrolled = fixture.draw(&mut state);
    let panel_scroll = match state.overlay.as_ref() {
        Some(OverlayState::PermissionRequest(permission)) => permission.panel_scroll,
        _ => 0,
    };

    assert!(first.initial_frame);
    assert!(!scrolled.initial_frame);
    assert!(panel_scroll > 0);
    assert!(scrolled.output_bytes > 0);
    assert!(scrolled.output_bytes < first.output_bytes);
    assert!(scrolled.draw_command_count < scrolled.buffer_cell_count);
}

#[test]
fn mouse_drag_selects_transcript_while_permission_overlay_is_open() {
    let mut state = normal_state("", 0);
    state.overlay = Some(OverlayState::PermissionRequest(
        permission_overlay_with_viewport(Rect::new(1, 5, 20, 4)),
    ));
    state.transcript_ui.viewport.sync(
        Rect::new(0, 0, 20, 4),
        vec![
            Line::from("Thinking"),
            Line::from("about"),
            Line::from("Rust"),
        ],
        vec![
            Line::from("Thinking"),
            Line::from("about"),
            Line::from("Rust"),
        ],
        0,
        0,
        0,
    );

    state.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 0));
    state.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 3, 2));

    assert_eq!(
        state.transcript_ui.viewport.selected_text().as_deref(),
        Some("hinking\nabout\nRust")
    );
}

#[test]
fn mouse_scroll_outside_permission_panel_does_not_scroll_transcript() {
    let mut state = normal_state("", 0);
    state.overlay = Some(OverlayState::PermissionRequest(
        permission_overlay_with_viewport(Rect::new(1, 5, 20, 4)),
    ));
    state.transcript_ui.viewport.current_scroll = 0;
    state.transcript_ui.viewport.max_scroll = 12;

    state.handle_mouse(mouse_event(MouseEventKind::ScrollUp, 0, 0));

    assert_eq!(state.transcript_ui.viewport.current_scroll, 0);
}

#[test]
fn mouse_scroll_permission_panel_updates_scroll_and_clears_selection() {
    let mut state = normal_state("", 0);
    state.overlay = Some(OverlayState::PermissionRequest(
        permission_overlay_with_viewport(Rect::new(1, 1, 20, 4)),
    ));
    state.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 1));
    state.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 4, 3));

    state.handle_mouse(mouse_event(MouseEventKind::ScrollUp, 5, 2));

    let (panel_scroll, has_selection) = match &state.overlay {
        Some(OverlayState::PermissionRequest(permission)) => (
            permission.panel_scroll,
            permission.viewport.selection.is_some(),
        ),
        _ => (0, true),
    };
    assert_eq!(panel_scroll, 3);
    assert!(!has_selection);
    assert_eq!(
        state.transient_footer_status(),
        "Browsing permission details. End returns to options."
    );
}

#[test]
fn desired_viewport_height_includes_permission_panel_content() {
    let mut state = normal_state("", 0);
    let request = PermissionRequest {
            request_id: "req-1".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "tool-1".to_string(),
            tool_name: "Agent".to_string(),
            tool_input: serde_json::json!({
                "description": "Explore permission panel implementation",
                "prompt": "Please examine the permission panel implementation in orbcode/tui/src/lib.rs.\nFocus on:\n1. How the permission panel is structured and rendered\n2. What permission types are supported\n3. How user input/approval is handled\n4. The overall architecture of the permission system in the TUI\nProvide a detailed analysis of the permission panel code, including key functions, data structures, and the flow of permission requests.",
                "subagent_type": "Explore"
            })
            .to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        };
    state.apply_stream_event(StreamEvent::PermissionRequested { request });

    let height = state.desired_viewport_height(140, 100);
    let permission = match state.overlay.as_ref() {
        Some(OverlayState::PermissionRequest(permission)) => permission,
        _ => panic!("expected permission overlay"),
    };
    let panel = permission_panel_content(permission, 138);
    let panel_height = permission_panel_full_height(&panel.body, 138);
    let transcript_height = wrap_styled_lines(&state.transcript_lines_for_messages(140, true), 140)
        .len()
        .max(1) as u16;

    assert!(
        height
            >= panel_height
                .saturating_add(transcript_height)
                .saturating_add(3),
        "{height}"
    );
}

#[test]
fn permission_overlay_area_is_compact_and_within_host_area() {
    let host = Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };
    let body = vec![
        Line::from("Tool bash"),
        Line::default(),
        Line::from("This command requires approval"),
        Line::from("Needs bash"),
        Line::default(),
        Line::from("Input"),
        Line::from("{"),
        Line::from("  \"command\": \"ls\""),
        Line::from("}"),
        Line::default(),
        Line::from("Do you want to proceed?"),
        Line::default(),
        Line::from("› Yes"),
        Line::from("  No"),
    ];

    let area = permission_overlay_area(&body, host);

    assert!(area.width <= host.width);
    assert!(area.height <= host.height);
    assert!(area.height < host.height);
    assert!(area.width >= 48);
    assert!(area.height >= 10);
}

#[test]
fn permission_overlay_area_handles_tiny_host_area_without_panicking() {
    let host = Rect {
        x: 0,
        y: 0,
        width: 20,
        height: 5,
    };
    let body = vec![Line::from("This command requires approval")];

    let area = permission_overlay_area(&body, host);

    assert!(area.width <= host.width);
    assert!(area.height <= host.height);
    assert!(area.height >= 1);
}

#[test]
fn ctrl_o_permission_overlay_expands_request_and_transcript_details() {
    let mut state = normal_state("", 0);
    state.overlay = Some(OverlayState::PermissionRequest(
        PermissionOverlayState::new(PermissionRequest {
            request_id: "req-1".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "tool-1".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({
                "command": "printf hi",
                "description": "Print a greeting"
            })
            .to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        }),
    ));

    state.toggle_permission_request_details();

    assert!(state.expanded_tool_details);
    let permission = match &state.overlay {
        Some(OverlayState::PermissionRequest(permission)) => permission,
        _ => panic!("expected permission overlay"),
    };
    assert!(permission.details_expanded);
    assert!(permission.panel_scroll > 0);
    let expanded = plain_text_lines(&permission_panel_content(permission, 100).body).join("\n");
    assert!(expanded.contains("Request details"), "{expanded}");
    assert!(expanded.contains("Tool use ID tool-1"), "{expanded}");
    assert!(expanded.contains("Requires tools permission"), "{expanded}");

    state.toggle_permission_request_details();

    assert!(!state.expanded_tool_details);
    let permission = match &state.overlay {
        Some(OverlayState::PermissionRequest(permission)) => permission,
        _ => panic!("expected permission overlay"),
    };
    assert!(!permission.details_expanded);
    assert_eq!(permission.panel_scroll, 0);
}

#[test]
fn permission_overlay_preserves_visible_transcript_context() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "please inspect the permission panel",
    ));
    state.messages.push(TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "agent-tool".to_string(),
                name: "Agent".to_string(),
                input:
                    "{\"description\":\"Inspect permission panel\",\"prompt\":\"inspect orbcode/tui/src/lib.rs\",\"subagent_type\":\"Explore\"}"
                        .to_string(),
            }],
        ));
    let request = PermissionRequest {
            request_id: "req-read".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "read-tool".to_string(),
            tool_name: "Read".to_string(),
            tool_input: serde_json::json!({
                "file_path": "/Users/user/github/sample-workspace-main/crates/render-fixtures/tui/src/lib.rs"
            })
            .to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        };
    state.apply_stream_event(StreamEvent::PermissionRequested {
        request: request.clone(),
    });

    let area = Rect::new(0, 0, 140, 30);
    let input_view = InputView {
        lines: vec![String::new()],
        line_layouts: vec![InputLineLayout {
            start: 0,
            end: 0,
            text: String::new(),
        }],
        width: 137,
        cursor_row: 0,
        cursor_col: 0,
    };
    let layout = state.main_layout_regions(area, &input_view, 0);
    let permission = PermissionOverlayState::new(request);
    let body = permission_panel_content(
        &permission,
        layout[0].width.saturating_sub(2).max(1) as usize,
    );
    let panel_height = permission_panel_height_with_context(&body.body, layout[0]);
    let uncovered_rows = layout[0].height.saturating_sub(panel_height) as usize;
    let transcript_view =
        state.visible_transcript_lines_for_view(layout[0].width as usize, uncovered_rows, true);
    let visible_above_panel = plain_text_lines(&transcript_view.visible_lines)
        .into_iter()
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        visible_above_panel.contains("Orb Code"),
        "{visible_above_panel}"
    );
    assert!(
        visible_above_panel.contains("please inspect the permission panel"),
        "{visible_above_panel}"
    );
    assert!(
        visible_above_panel.contains("Explore(Inspect permission panel)"),
        "{visible_above_panel}"
    );
}

#[test]
fn ctrl_o_state_rerenders_active_tool_details() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "agent-tool".to_string(),
                name: "Agent".to_string(),
                input:
                    "{\"description\":\"Explore repo\",\"prompt\":\"check flow\",\"subagent_type\":\"Explore\"}"
                        .to_string(),
            }],
        ));
    state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "session".to_string(),
        tool_use_id: "agent-tool".to_string(),
        tool_name: "Agent".to_string(),
        tool_input: String::new(),
    });
    state.apply_stream_event(StreamEvent::ToolProgress {
        session_id: "session".to_string(),
        tool_use_id: "agent-tool".to_string(),
        tool_name: "Agent".to_string(),
        progress: serde_json::json!({
            "data": {
                "type": "agent_progress",
                "message": {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
                            { "type": "text", "text": "Checking the core flow now." },
                            {
                                "type": "tool_use",
                                "id": "file-read-1",
                                "name": "Read",
                                "input": { "file_path": "/Users/user/github/sample-repo/README.md" }
                            }
                        ]
                    }
                }
            }
        }),
    });

    let brief = plain_text_lines(&state.transcript_lines(90)).join("\n");

    assert!(brief.contains("(ctrl+o to expand)"), "{brief}");
    assert!(brief.contains("Checking the core flow now."), "{brief}");
    assert!(brief.contains("Read("), "{brief}");
}

#[test]
fn inserted_terminal_history_snapshot_cannot_follow_later_ctrl_o_state() {
    let mut state = normal_state("", 0);
    let metadata = serde_json::json!({
        "summary": "Done (1 tool use · 42 tokens · 1s)",
        "progressMessages": [
            {
                "data": {
                    "type": "agent_progress",
                    "message": {
                        "type": "assistant",
                        "message": {
                            "role": "assistant",
                            "content": [
                                { "type": "text", "text": "Snapshot-only progress detail." }
                            ]
                        }
                    }
                }
            }
        ]
    })
    .to_string();
    state.messages = vec![
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
                    content: "completed output".to_string(),
                    is_error: false,
                    metadata: Some(metadata),
                }],
            ),
        ];
    state.pending_history_flush = true;

    let inserted_snapshot = state.take_history_lines(90, 24);
    state.expanded_tool_details = true;
    let snapshot_text = plain_text_lines(&inserted_snapshot).join("\n");
    let rerendered_from_state = plain_text_lines(&flatten_transcript_cells(
        &state.stable_transcript_cells(90),
    ))
    .join("\n");

    assert!(
        snapshot_text.contains("(ctrl+o to expand)"),
        "{snapshot_text}"
    );
    assert!(
        snapshot_text.contains("Snapshot-only progress detail."),
        "{snapshot_text}"
    );
    assert!(!snapshot_text.contains("Prompt:"), "{snapshot_text}");
    assert!(
        rerendered_from_state.contains("Snapshot-only progress detail."),
        "{rerendered_from_state}"
    );
}
