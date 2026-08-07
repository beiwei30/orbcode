use crate::tests::support::*;

#[test]
fn help_overlay_lines_use_grouped_two_column_layout() {
    let lines = plain_text_lines(&help_overlay_lines(120));
    assert!(lines.iter().any(|line| line.trim() == "GLOBAL"));
    assert!(lines.iter().any(|line| line.trim() == "INPUT"));
    assert!(lines.iter().any(|line| line.trim() == "AGENT ENVIRONMENT"));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("/help") && line.contains("show full help"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("ctrl+o") && line.contains("expand or collapse"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("/allow-all on|off") && line.contains("YOLO permissions"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("/mcp call <server> <tool> [input]"))
    );
}

#[test]
fn help_overlay_lines_cache_reuses_content_across_scroll() {
    let mut help = HelpOverlayState::default();
    let content_height = 10;
    let line_count = help.cached_lines(120).len();
    help.max_scroll = line_count.saturating_sub(content_height);

    let first = plain_text_lines(&help.cached_visible_lines(120, content_height)).join("\n");
    help.scroll = 5;
    let second = plain_text_lines(&help.cached_visible_lines(120, content_height)).join("\n");

    assert_ne!(first, second);
    assert_eq!(help.lines_cache.misses, 1);
    assert_eq!(help.lines_cache.hits, 2);

    let _ = help.cached_visible_lines(100, content_height);
    assert_eq!(help.lines_cache.misses, 2);
}

#[test]
#[ignore = "manual stress test for help overlay line caching"]
fn help_overlay_lines_cache_stress_reuses_content_across_scroll() {
    const FRAME_COUNT: usize = 1_000;
    const WIDTH: usize = 120;
    const CONTENT_HEIGHT: usize = 20;

    let mut help = HelpOverlayState::default();
    let line_count = help.cached_lines(WIDTH).len();
    help.max_scroll = line_count.saturating_sub(CONTENT_HEIGHT);
    let started = Instant::now();
    let mut last_visible_len = 0;
    for frame in 0..FRAME_COUNT {
        help.scroll = frame % help.max_scroll.max(1);
        let lines = help.cached_visible_lines(WIDTH, CONTENT_HEIGHT);
        assert!(lines.len() <= CONTENT_HEIGHT);
        last_visible_len = lines.len();
    }
    let duration = started.elapsed();

    assert_eq!(help.lines_cache.misses, 1);
    assert_eq!(help.lines_cache.hits, FRAME_COUNT as u64);
    eprintln!(
        "frames={FRAME_COUNT} total_lines={line_count} visible_lines={last_visible_len} cache_hits={} cache_misses={} loop_us={}",
        help.lines_cache.hits,
        help.lines_cache.misses,
        duration.as_micros()
    );
}

#[test]
fn help_overlay_keys_follow_less_style_scrolling() {
    let mut help = HelpOverlayState {
        scroll: 4,
        max_scroll: 20,
        ..HelpOverlayState::default()
    };

    assert_eq!(
        apply_help_overlay_key(
            &mut help,
            &crossterm::event::KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        ),
        HelpOverlayAction::None
    );
    assert_eq!(help.scroll, 3);

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, 4);

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, 4 + HELP_OVERLAY_PAGE_STEP);

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, 4);

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, 0);

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, help.max_scroll);

    assert_eq!(
        apply_help_overlay_key(
            &mut help,
            &crossterm::event::KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        ),
        HelpOverlayAction::Close
    );
}

#[test]
fn help_overlay_scroll_stays_clamped_at_boundaries() {
    let mut help = HelpOverlayState {
        scroll: 0,
        max_scroll: 3,
        ..HelpOverlayState::default()
    };

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, 0);

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, 0);

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, 1);

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, 3);

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, 3);

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, 3);

    apply_help_overlay_key(
        &mut help,
        &crossterm::event::KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
    );
    assert_eq!(help.scroll, 2);
}

// ---------------------------------------------------------------------------
// Visual regression fixtures — overlay resize + keybind help
// ---------------------------------------------------------------------------

#[test]
fn help_overlay_lines_at_narrow_40_col_preserve_content() {
    let lines = plain_text_lines(&help_overlay_lines(40));
    assert!(lines.iter().any(|l| l.trim() == "GLOBAL"));
    assert!(lines.iter().any(|l| l.trim() == "INPUT"));
    assert!(lines.iter().any(|l| l.trim() == "AGENT ENVIRONMENT"));
    assert!(lines.iter().any(|l| l.contains("/help")));
    assert!(lines.iter().any(|l| l.contains("ctrl+c")));
    assert!(lines.iter().any(|l| l.contains("ctrl+o")));
}

#[test]
fn help_overlay_lines_at_very_narrow_20_col() {
    let lines = plain_text_lines(&help_overlay_lines(20));
    assert!(!lines.is_empty());
    assert!(lines.iter().any(|l| l.trim() == "GLOBAL"));
    assert!(lines.iter().any(|l| l.trim() == "INPUT"));
    assert!(lines.iter().any(|l| l.trim() == "AGENT ENVIRONMENT"));
}

#[test]
fn help_overlay_render_at_wide_200_col() {
    let lines = plain_text_lines(&help_overlay_lines(200));
    assert!(lines.iter().any(|l| l.trim() == "GLOBAL"));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("/help") && l.contains("show full help"))
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("ctrl+c") && l.contains("cancel"))
    );
}

#[test]
fn model_picker_lines_at_narrow_40_col() {
    let picker = ModelPickerState::new(
        "/model",
        vec![
            ModelOption {
                value: Some("sonnet".to_string()),
                label: "Sonnet 4.6".to_string(),
                description: "Best for everyday coding tasks".to_string(),
                current: true,
            },
            ModelOption {
                value: Some("opus".to_string()),
                label: "Opus 4.6".to_string(),
                description: "Most capable for complex work".to_string(),
                current: false,
            },
        ],
        Some(EffortLevel::High),
    );

    let lines = plain_text_lines(&model_picker_lines(&picker, 40));
    assert!(lines[0].contains("Select model"));
    assert!(display_width_str(&lines[1]) <= 40);
    assert!(lines.iter().any(|l| l.contains("Sonnet 4.6")));
    assert!(lines.iter().any(|l| l.contains("Opus 4.6")));
}

#[test]
fn permission_picker_lines_at_narrow_40_col() {
    let overview = PermissionOverview {
        permissions: orbcode_app_server_client::PermissionContext {
            cwd: PathBuf::from("/tmp/project"),
            allow_network: false,
            provider_allow_network: false,
            allow_tools: false,
            allowed_rules: Vec::new(),
            denied_rules: Vec::new(),
            ask_rules: Vec::new(),
            additional_directories: Vec::new(),
        },
        allow_all: false,
        effective_rules: Default::default(),
        settings_allowed_rules: vec!["Bash(cargo test:*)".to_string()],
        settings_denied_rules: Vec::new(),
        startup_allowed_rules: Vec::new(),
        startup_denied_rules: Vec::new(),
        edited_allowed_rules: Vec::new(),
        edited_denied_rules: Vec::new(),
        runtime_allowed_rules: Vec::new(),
        runtime_denied_rules: Vec::new(),
        configured_additional_directories: Vec::new(),
        session_additional_directories: Vec::new(),
    };
    let mut picker = PermissionPickerState::new("/permissions", overview, Vec::new());
    let lines = picker.cached_lines(40);
    assert_eq!(lines.len(), PERMISSION_PICKER_PANEL_HEIGHT);
    assert_eq!(plain_text_line(&lines[0]), "Permissions");
}

#[test]
fn permission_panel_content_at_narrow_40_col() {
    let mut permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({"command": "ls -la"}).to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });
    let cached = permission.cached_panel_content(38);
    assert!(!cached.wrapped_body.is_empty());
}

#[test]
fn keybind_help_overlay_shows_all_contexts() {
    let lines = plain_text_lines(&keybind_help_overlay_lines(120));
    assert!(lines.iter().any(|l| l.contains("KEYBINDINGS")));
    assert!(lines.iter().any(|l| l.trim() == "GLOBAL"));
    assert!(lines.iter().any(|l| l.trim() == "CHAT"));
    assert!(lines.iter().any(|l| l.trim() == "OVERLAY NAVIGATION"));
    assert!(lines.iter().any(|l| l.trim() == "VIM NORMAL MODE"));
    assert!(lines.iter().any(|l| l.contains("ctrl+")));
}

#[test]
fn keybind_help_overlay_scroll_and_close() {
    let mut state = KeybindHelpOverlayState::default();
    let _ = state.cached_lines(120);
    state.max_scroll = 20;
    state.scroll = 5;

    apply_keybind_help_overlay_key(
        &mut state,
        &crossterm::event::KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
    );
    assert_eq!(state.scroll, 6);

    apply_keybind_help_overlay_key(
        &mut state,
        &crossterm::event::KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
    );
    assert_eq!(state.scroll, 5);

    apply_keybind_help_overlay_key(
        &mut state,
        &crossterm::event::KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
    );
    assert_eq!(state.scroll, 5 + HELP_OVERLAY_PAGE_STEP);

    apply_keybind_help_overlay_key(
        &mut state,
        &crossterm::event::KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
    );
    assert_eq!(state.scroll, 0);

    apply_keybind_help_overlay_key(
        &mut state,
        &crossterm::event::KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
    );
    assert_eq!(state.scroll, state.max_scroll);

    assert_eq!(
        apply_keybind_help_overlay_key(
            &mut state,
            &crossterm::event::KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        ),
        KeybindHelpOverlayAction::Close
    );
}

#[test]
fn render_metrics_narrow_40_col_help_overlay() {
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 40);
    state.overlay = Some(OverlayState::Help(HelpOverlayState::default()));
    let mut fixture = RenderMetricsFixture::new(40, 24);
    let metrics = fixture.draw(&mut state);
    assert!(metrics.initial_frame);
    assert!(metrics.output_bytes > 0);

    if let Some(OverlayState::Help(help)) = state.overlay.as_mut() {
        help.scroll = help.max_scroll.min(4);
    }
    let second = fixture.draw(&mut state);
    assert!(!second.initial_frame);
    assert!(second.output_bytes > 0);
}

#[test]
fn render_metrics_wide_200_col_help_overlay() {
    let mut state = normal_state("", 0);
    fill_long_transcript(&mut state, 40);
    state.overlay = Some(OverlayState::Help(HelpOverlayState::default()));
    let mut fixture = RenderMetricsFixture::new(200, 30);
    let metrics = fixture.draw(&mut state);
    assert!(metrics.initial_frame);
    assert!(metrics.output_bytes > 0);

    if let Some(OverlayState::Help(help)) = state.overlay.as_mut() {
        help.scroll = help.max_scroll.min(8);
    }
    let second = fixture.draw(&mut state);
    assert!(!second.initial_frame);
    assert!(second.output_bytes > 0);
}

#[test]
fn help_overlay_resize_120_to_40_preserves_sections() {
    let wide = plain_text_lines(&help_overlay_lines(120));
    let narrow = plain_text_lines(&help_overlay_lines(40));
    let sections = ["GLOBAL", "INPUT", "AGENT ENVIRONMENT", "MCP"];
    for section in &sections {
        assert!(
            wide.iter().any(|l| l.trim() == *section),
            "wide missing {section}"
        );
        assert!(
            narrow.iter().any(|l| l.trim() == *section),
            "narrow missing {section}"
        );
    }
}
