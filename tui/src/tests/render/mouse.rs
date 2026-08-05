use crate::tests::support::*;
use orbcode_app_server_client::{AskUserOption, AskUserQuestionRequest, AskUserQuestionSpec};

#[test]
fn ask_user_mouse_focus_change_requests_redraw() {
    let request = AskUserQuestionRequest {
        session_id: "session-1".into(),
        turn_id: Some("turn-1".into()),
        tool_use_id: "tool-1".into(),
        request_id: "ask-1".into(),
        deadline: None,
        validation_error: None,
        questions: vec![AskUserQuestionSpec {
            id: "database".into(),
            question: "Which database?".into(),
            header: "Database".into(),
            multi_select: false,
            options: vec![AskUserOption {
                id: "postgres".into(),
                label: "PostgreSQL".into(),
                description: String::new(),
                preview: None,
            }],
            allow_free_text: false,
            allow_annotation: false,
        }],
        question: String::new(),
        options: Vec::new(),
    };
    let mut overlay = AskUserQuestionOverlayState::new(request);
    overlay.panel_area = Rect::new(0, 0, 40, 10);
    let mut state = normal_state("", 0);
    state.overlay = Some(OverlayState::AskUserQuestion(overlay));

    let changed = state.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 6));

    assert!(changed);
}

#[test]
fn mouse_scroll_up_ignores_events_at_top_boundary() {
    let mut state = normal_state("", 0);
    state.transcript_ui.viewport.current_scroll = 9;
    state.transcript_ui.viewport.max_scroll = 9;
    state.status_line = "sentinel".to_string();

    let changed = state.handle_mouse(mouse_event(MouseEventKind::ScrollUp, 0, 0));

    assert!(!changed);
    assert_eq!(state.transient_footer_status(), "sentinel");
}

#[test]
fn mouse_scroll_down_ignores_events_at_bottom_boundary() {
    let mut state = normal_state("", 0);
    state.transcript_ui.viewport.current_scroll = 0;
    state.transcript_ui.viewport.max_scroll = 9;
    state.status_line = "sentinel".to_string();

    let changed = state.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 0, 0));

    assert!(!changed);
    assert_eq!(state.transient_footer_status(), "sentinel");
}

#[test]
fn mouse_events_report_no_visible_change_for_overlay_and_permission_noops() {
    let mut model_state = state_with_status_overlay(OverlayState::ModelPicker(
        ModelPickerState::new("/model", synthetic_model_options(4, 0), None),
    ));

    assert!(!model_state.handle_mouse(mouse_event(MouseEventKind::ScrollUp, 0, 0)));

    let mut permission_state = normal_state("", 0);
    let mut permission = permission_overlay_with_viewport(Rect::new(0, 0, 40, 4));
    permission.panel_scroll = 0;
    permission.viewport.set_scroll(0);
    permission_state.overlay = Some(OverlayState::PermissionRequest(permission));
    permission_state.status_line =
        "Browsing permission details. End returns to options.".to_string();

    assert!(!permission_state.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 2, 1)));
}

#[test]
#[ignore = "manual stress test for mouse no-op redraw suppression"]
fn mouse_noop_redraw_skip_stress_filters_boundary_scroll_events() {
    const EVENT_COUNT: usize = 10_000;

    let mut state = normal_state("", 0);
    state.transcript_ui.viewport.current_scroll = 0;
    state.transcript_ui.viewport.max_scroll = 120;
    state.status_line = "sentinel".to_string();

    let mut changed_events = 0;
    let started = Instant::now();
    for _ in 0..EVENT_COUNT {
        if state.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 0, 0)) {
            changed_events += 1;
        }
    }
    let duration = started.elapsed();

    assert_eq!(changed_events, 0);
    assert_eq!(state.transient_footer_status(), "sentinel");
    eprintln!(
        "events={EVENT_COUNT} changed_events={changed_events} loop_us={}",
        duration.as_micros()
    );
}

#[test]
fn mouse_drag_auto_copies_selected_prompt_text_on_release() {
    let _clipboard_guard = test_clipboard_assertion_lock()
        .lock()
        .expect("test clipboard assertion mutex poisoned");
    let _ = take_test_clipboard_capture();
    let mut state = normal_state("hello world", "hello world".len());
    state.input_area = Rect::new(0, 3, 20, 1);

    state.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 3));
    state.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 7, 3));

    assert_eq!(state.selected_input_text().as_deref(), Some("hello"));

    state.handle_mouse(mouse_event(MouseEventKind::Up(MouseButton::Left), 7, 3));

    assert_eq!(take_test_clipboard_capture().as_deref(), Some("hello"));
    assert!(!state.has_input_selection());
    assert_eq!(state.transient_footer_status(), "Selected text copied.");
}

#[test]
fn mouse_click_does_not_auto_copy_collapsed_prompt_selection() {
    let _clipboard_guard = test_clipboard_assertion_lock()
        .lock()
        .expect("test clipboard assertion mutex poisoned");
    let _ = take_test_clipboard_capture();
    let mut state = normal_state("hello world", "hello world".len());
    state.input_area = Rect::new(0, 3, 20, 1);
    state.status_line = "sentinel".to_string();

    state.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 3));
    state.handle_mouse(mouse_event(MouseEventKind::Up(MouseButton::Left), 2, 3));

    assert_eq!(take_test_clipboard_capture(), None);
    assert!(!state.has_input_selection());
    assert_eq!(state.transient_footer_status(), "sentinel");
}

#[test]
fn mouse_drag_selects_visible_permission_text() {
    let mut state = normal_state("", 0);
    state.overlay = Some(OverlayState::PermissionRequest(
        permission_overlay_with_viewport(Rect::new(1, 1, 20, 4)),
    ));

    state.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 1));
    state.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 4, 3));

    let selected = match &state.overlay {
        Some(OverlayState::PermissionRequest(permission)) => permission.viewport.selected_text(),
        _ => None,
    };
    assert_eq!(selected.as_deref(), Some("lpha\nBeta\nGamm"));
}

#[test]
fn mouse_drag_selects_permission_row_outside_panel_columns() {
    let mut state = normal_state("", 0);
    state.overlay = Some(OverlayState::PermissionRequest(
        permission_overlay_with_viewport(Rect::new(5, 1, 10, 3)),
    ));

    state.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 0, 1));
    state.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 50, 3));

    let selected = match &state.overlay {
        Some(OverlayState::PermissionRequest(permission)) => permission.viewport.selected_text(),
        _ => None,
    };
    assert_eq!(selected.as_deref(), Some("Alpha\nBeta\nGamma"));
}

#[test]
fn mouse_drag_auto_copies_permission_text_on_release() {
    let _clipboard_guard = test_clipboard_assertion_lock()
        .lock()
        .expect("test clipboard assertion mutex poisoned");
    let _ = take_test_clipboard_capture();
    let mut state = normal_state("", 0);
    state.overlay = Some(OverlayState::PermissionRequest(
        permission_overlay_with_viewport(Rect::new(1, 1, 20, 4)),
    ));

    state.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 1));
    state.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 4, 3));
    state.handle_mouse(mouse_event(MouseEventKind::Up(MouseButton::Left), 4, 3));

    assert_eq!(
        take_test_clipboard_capture().as_deref(),
        Some("lpha\nBeta\nGamm")
    );
    assert!(!state.has_permission_selection());
    assert_eq!(state.transient_footer_status(), "Selected text copied.");
}

#[test]
fn mouse_drag_from_transcript_into_permission_selects_both_regions() {
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
    state.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 4, 6));

    assert_eq!(
        state.transcript_ui.viewport.selected_text().as_deref(),
        Some("hinking\nabout\nRust")
    );
    let selected_permission = match &state.overlay {
        Some(OverlayState::PermissionRequest(permission)) => permission.viewport.selected_text(),
        _ => None,
    };
    assert_eq!(selected_permission.as_deref(), Some("Alpha\nBeta"));
}

#[test]
fn mouse_drag_from_transcript_past_permission_selects_and_scrolls_permission() {
    let mut state = normal_state("", 0);
    let all_permission_lines = vec![
        Line::from("zero"),
        Line::from("one"),
        Line::from("two"),
        Line::from("three"),
        Line::from("four"),
    ];
    let mut permission = permission_overlay_with_viewport(Rect::new(1, 5, 20, 3));
    permission.panel_scroll = 1;
    permission.viewport.sync(
        Rect::new(1, 5, 20, 3),
        all_permission_lines[1..4].to_vec(),
        all_permission_lines,
        1,
        1,
        2,
    );
    state.overlay = Some(OverlayState::PermissionRequest(permission));
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
    state.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 50, 9));

    assert_eq!(
        state.transcript_ui.viewport.selected_text().as_deref(),
        Some("hinking\nabout\nRust")
    );
    let (selected_permission, panel_scroll) = match &state.overlay {
        Some(OverlayState::PermissionRequest(permission)) => {
            (permission.viewport.selected_text(), permission.panel_scroll)
        }
        _ => (None, usize::MAX),
    };
    assert_eq!(selected_permission.as_deref(), Some("one\ntwo\nthree"));
    assert_eq!(panel_scroll, 0);
}

#[test]
fn mouse_drag_from_permission_into_transcript_selects_both_regions() {
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

    state.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 4, 6));
    state.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 1, 0));

    assert_eq!(
        state.transcript_ui.viewport.selected_text().as_deref(),
        Some("hinking\nabout\nRust")
    );
    let selected_permission = match &state.overlay {
        Some(OverlayState::PermissionRequest(permission)) => permission.viewport.selected_text(),
        _ => None,
    };
    assert_eq!(selected_permission.as_deref(), Some("Alpha\nBeta"));
}

#[test]
fn mouse_drag_bottom_edge_autoscrolls_permission_selection() {
    let mut state = normal_state("", 0);
    let all_lines = vec![
        Line::from("zero"),
        Line::from("one"),
        Line::from("two"),
        Line::from("three"),
        Line::from("four"),
    ];
    let mut permission = permission_overlay_with_viewport(Rect::new(1, 1, 20, 3));
    permission.panel_scroll = 1;
    permission.viewport.sync(
        Rect::new(1, 1, 20, 3),
        all_lines[1..4].to_vec(),
        all_lines,
        1,
        1,
        2,
    );
    state.overlay = Some(OverlayState::PermissionRequest(permission));

    state.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 2));
    state.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 1, 3));

    let panel_scroll = match &state.overlay {
        Some(OverlayState::PermissionRequest(permission)) => permission.panel_scroll,
        _ => 0,
    };
    assert_eq!(panel_scroll, 0);
}

#[test]
fn transcript_selection_highlights_selected_cells() {
    let mut viewport = TranscriptViewportState::default();
    viewport.sync(
        Rect::new(0, 0, 10, 2),
        vec![Line::from("abcd")],
        vec![Line::from("abcd")],
        0,
        0,
        0,
    );
    viewport.selection = Some(TranscriptSelectionState {
        area: Rect::new(0, 0, 10, 2),
        anchor: TranscriptSelectionPoint { row: 0, column: 1 },
        focus: TranscriptSelectionPoint { row: 0, column: 2 },
    });

    let rendered = viewport.render_lines();
    let selected_style = rendered[0].spans[1].style;

    assert_eq!(rendered[0].spans[1].content.as_ref(), "bc");
    assert!(selected_style.add_modifier.contains(Modifier::REVERSED));
    assert!(
        rendered[0].spans[1]
            .style
            .add_modifier
            .contains(Modifier::REVERSED)
    );
    assert!(
        !rendered[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::REVERSED)
    );
}

#[test]
#[cfg(target_os = "macos")]
fn transcript_copy_shortcut_uses_cmd_c_on_macos() {
    let key_event = crossterm::event::KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER);
    assert!(is_transcript_copy_shortcut(&key_event));
    assert_eq!(transcript_copy_shortcut_label(), "Cmd+C");
}

#[test]
#[cfg(not(target_os = "macos"))]
fn transcript_copy_shortcut_uses_ctrl_shift_c_off_macos() {
    let key_event = crossterm::event::KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(is_transcript_copy_shortcut(&key_event));
    assert_eq!(transcript_copy_shortcut_label(), "Ctrl+Shift+C");
}
