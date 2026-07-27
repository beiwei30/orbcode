use crate::tests::support::*;

#[test]
fn render_metrics_fixture_covers_large_help_and_diff_overlays() {
    let mut help_state = normal_state("", 0);
    fill_long_transcript(&mut help_state, 80);
    help_state.overlay = Some(OverlayState::Help(HelpOverlayState::default()));
    let mut help_fixture = RenderMetricsFixture::new(100, 24);

    let help_first = help_fixture.draw(&mut help_state);
    if let Some(OverlayState::Help(help)) = help_state.overlay.as_mut() {
        assert!(help.max_scroll > 0);
        help.scroll = help.max_scroll.min(12);
    } else {
        panic!("expected help overlay");
    }
    let help_second = help_fixture.draw(&mut help_state);

    assert!(help_first.initial_frame);
    assert!(!help_second.initial_frame);
    assert!(help_second.output_bytes > 0);
    assert!(help_second.draw_command_count < help_second.buffer_cell_count);

    let mut diff_state = normal_state("", 0);
    fill_long_transcript(&mut diff_state, 80);
    diff_state.overlay = Some(OverlayState::Diff(DiffOverlayState::new(
        large_workspace_diff(),
    )));
    let mut diff_fixture = RenderMetricsFixture::new(120, 32);

    let diff_first = diff_fixture.draw(&mut diff_state);
    if let Some(OverlayState::Diff(diff)) = diff_state.overlay.as_mut() {
        assert!(diff.max_line_scroll > 0);
        diff.scroll_lines(12);
    } else {
        panic!("expected diff overlay");
    }
    let diff_second = diff_fixture.draw(&mut diff_state);

    assert!(diff_first.initial_frame);
    assert!(!diff_second.initial_frame);
    assert!(diff_second.output_bytes > 0);
    assert!(diff_second.output_bytes < diff_first.output_bytes);
    assert!(diff_second.draw_command_count < diff_second.buffer_cell_count);
}

#[test]
fn session_picker_filters_by_title_and_id() {
    let sessions = vec![
        SessionSummary {
            session_id: "aaaa1111-9997".to_string(),
            title: Some("Fix tui renderer".to_string()),
            message_count: 12,
            created_at: Utc.with_ymd_and_hms(2026, 4, 10, 10, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 4, 10, 11, 0, 0).unwrap(),
            ..Default::default()
        },
        SessionSummary {
            session_id: "bbbb2222-0000".to_string(),
            title: Some("Provider smoke".to_string()),
            cwd: Some("/repo/worktrees/provider".to_string()),
            git_branch: Some("feature/provider-smoke".to_string()),
            message_count: 3,
            created_at: Utc.with_ymd_and_hms(2026, 4, 10, 12, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 4, 10, 13, 0, 0).unwrap(),
            ..Default::default()
        },
    ];

    let mut picker =
        SessionPickerState::new("/sessions", "Project Sessions", sessions, "aaaa1111-9997");
    assert_eq!(picker.sessions.len(), 2);
    assert_eq!(picker.selected, 0);

    picker.query = "provider".to_string();
    picker.refresh(None);
    assert_eq!(picker.sessions.len(), 1);
    assert_eq!(picker.sessions[0].session_id, "bbbb2222-0000");

    picker.query = "feature smoke".to_string();
    picker.refresh(None);
    assert_eq!(picker.sessions.len(), 1);
    assert_eq!(picker.sessions[0].session_id, "bbbb2222-0000");

    picker.query = "worktrees provider".to_string();
    picker.refresh(None);
    assert_eq!(picker.sessions.len(), 1);
    assert_eq!(picker.sessions[0].session_id, "bbbb2222-0000");

    picker.query = "aaaa".to_string();
    picker.refresh(None);
    assert_eq!(picker.sessions.len(), 1);
    assert_eq!(picker.sessions[0].session_id, "aaaa1111-9997");

    picker.query = "b22".to_string();
    picker.refresh(None);
    assert_eq!(picker.sessions.len(), 1);
    assert_eq!(picker.sessions[0].session_id, "bbbb2222-0000");

    picker.query = "fx rndr".to_string();
    picker.refresh(None);
    assert_eq!(picker.sessions.len(), 1);
    assert_eq!(picker.sessions[0].session_id, "aaaa1111-9997");

    picker.query = "97".to_string();
    picker.refresh(None);
    assert!(picker.sessions.is_empty());
}

#[test]
fn session_picker_lines_use_inline_suggestion_layout() {
    let sessions = vec![
        SessionSummary {
            session_id: "aaaa1111-0000".to_string(),
            title: Some("Fix tui renderer".to_string()),
            message_count: 12,
            created_at: Utc.with_ymd_and_hms(2026, 4, 10, 10, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 4, 10, 11, 0, 0).unwrap(),
            ..Default::default()
        },
        SessionSummary {
            session_id: "bbbb2222-0000".to_string(),
            title: Some("Provider smoke".to_string()),
            message_count: 3,
            created_at: Utc.with_ymd_and_hms(2026, 4, 10, 12, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 4, 10, 13, 0, 0).unwrap(),
            ..Default::default()
        },
    ];
    let picker = SessionPickerState::new("/resume", "Resume Session", sessions, "aaaa1111-0000");
    let lines = plain_text_lines(&session_picker_lines(&picker, 100));

    assert!(lines[0].starts_with(" Resume Session"));
    assert!(!lines[0].starts_with("│"));
    assert!(lines[0].contains("Resume Session"));
    assert!(lines[0].contains("2 / 2"));
    assert!(lines[1].starts_with(" ╭"));
    assert!(lines[2].starts_with(" │ ⌕ Search…"));
    assert!(lines[3].starts_with(" ╰"));
    assert!(lines[4].starts_with(" │"));
    assert!(lines[4].contains("› aaaa1111"));
    assert!(lines[4].contains("Fix tui renderer"));
    assert!(!lines.join("\n").contains("┌"));
}

#[test]
fn model_picker_lines_follow_config_model_layout() {
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

    let lines = plain_text_lines(&model_picker_lines(&picker, 100));
    let rendered = lines.join("\n");

    assert!(lines[0].contains("Select model"));
    assert!(rendered.contains("❯ 1. Sonnet 4.6 ✔"));
    assert!(rendered.contains("Best for everyday coding tasks"));
    assert!(rendered.contains("2. Opus 4.6"));
    assert!(rendered.contains("Most capable for complex work"));
    assert!(rendered.contains("◉ High effort ← → to adjust"));
    assert!(rendered.contains("Enter to confirm · Esc to cancel"));
}

#[test]
fn model_picker_lines_cache_reuses_content_until_selection_changes() {
    let mut picker = ModelPickerState::new(
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

    let first = plain_text_lines(picker.cached_lines(100)).join("\n");
    let second = plain_text_lines(picker.cached_lines(100)).join("\n");

    assert_eq!(first, second);
    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, 1);

    picker.selected = 1;
    let changed = plain_text_lines(picker.cached_lines(100)).join("\n");
    assert_ne!(first, changed);
    assert_eq!(picker.lines_cache.misses, 2);
}

#[test]
#[ignore = "manual stress test for model picker line caching"]
fn model_picker_lines_cache_stress_reuses_large_option_set() {
    const FRAME_COUNT: usize = 1_000;
    let options = (0..200)
        .map(|index| ModelOption {
            value: Some(format!("model-{index}")),
            label: format!("Model {index:03}"),
            description: format!("Synthetic model option {index} for render cache stress"),
            current: index == 42,
        })
        .collect::<Vec<_>>();
    let mut picker = ModelPickerState::new("/model", options, Some(EffortLevel::Medium));
    let started = Instant::now();
    let mut last_visible_len = 0;
    for _ in 0..FRAME_COUNT {
        let lines = picker.cached_lines(120);
        last_visible_len = lines.len();
    }
    let duration = started.elapsed();

    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, (FRAME_COUNT - 1) as u64);
    eprintln!(
        "frames={FRAME_COUNT} options={} visible_lines={last_visible_len} cache_hits={} cache_misses={} loop_us={}",
        picker.options.len(),
        picker.lines_cache.hits,
        picker.lines_cache.misses,
        duration.as_micros()
    );
}

#[test]
fn theme_picker_lines_match_terminal_theme_picker_shape() {
    let picker = ThemePickerState::new("/theme", ThemeSetting::Auto);
    let rendered = plain_text_lines(&theme_picker_lines(&picker, 100)).join("\n");

    assert!(rendered.contains("Theme"), "{rendered}");
    assert!(rendered.contains("Auto (match terminal) ✔"), "{rendered}");
    assert!(
        rendered.contains("Dark mode (ANSI colors only)"),
        "{rendered}"
    );
    assert!(rendered.contains("New custom theme…"), "{rendered}");
    assert!(
        rendered.contains("console.log(\"Hello, Claude!\");"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Enter to select · Esc to cancel"),
        "{rendered}"
    );
}

#[test]
fn theme_picker_preview_uses_focused_theme_palette() {
    let mut picker = ThemePickerState::new("/theme", ThemeSetting::Auto);
    let dark_lines = theme_picker_lines(&picker, 100);
    let dark_added_bg = dark_lines
        .iter()
        .find(|line| plain_text_line(line).contains("Hello, Claude"))
        .and_then(|line| line.spans.iter().find_map(|span| span.style.bg))
        .expect("dark preview added background");

    picker.selected = picker
        .options
        .iter()
        .position(|option| option.value == Some(ThemeSetting::Light))
        .expect("light option");
    let light_lines = theme_picker_lines(&picker, 100);
    let light_added_bg = light_lines
        .iter()
        .find(|line| plain_text_line(line).contains("Hello, Claude"))
        .and_then(|line| line.spans.iter().find_map(|span| span.style.bg))
        .expect("light preview added background");

    assert_ne!(dark_added_bg, light_added_bg);
    assert_eq!(
        light_added_bg,
        palette_for_theme(ThemeSetting::Light).diff_added_bg
    );
}

#[test]
fn theme_picker_lines_cache_reuses_content_until_selection_changes() {
    let mut picker = ThemePickerState::new("/theme", ThemeSetting::Auto);

    let first = plain_text_lines(picker.cached_lines(100)).join("\n");
    let second = plain_text_lines(picker.cached_lines(100)).join("\n");

    assert_eq!(first, second);
    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, 1);

    picker.selected = 1;
    let changed = plain_text_lines(picker.cached_lines(100)).join("\n");
    assert_ne!(first, changed);
    assert_eq!(picker.lines_cache.misses, 2);
}

#[test]
#[ignore = "manual stress test for theme picker line caching"]
fn theme_picker_lines_cache_stress_reuses_preview_lines() {
    const FRAME_COUNT: usize = 1_000;

    let mut picker = ThemePickerState::new("/theme", ThemeSetting::Auto);
    let started = Instant::now();
    let mut last_visible_len = 0;
    for _ in 0..FRAME_COUNT {
        let lines = picker.cached_lines(120);
        last_visible_len = lines.len();
    }
    let duration = started.elapsed();

    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, (FRAME_COUNT - 1) as u64);
    eprintln!(
        "frames={FRAME_COUNT} options={} visible_lines={last_visible_len} cache_hits={} cache_misses={} loop_us={}",
        picker.options.len(),
        picker.lines_cache.hits,
        picker.lines_cache.misses,
        duration.as_micros()
    );
}

#[test]
fn output_style_picker_lines_cache_reuses_content_until_selection_changes() {
    let mut picker = OutputStylePickerState::new(
        "/output-style",
        vec![
            OutputStyleOption {
                value: "default".to_string(),
                label: "Default".to_string(),
                description: "Balanced response style".to_string(),
                current: true,
            },
            OutputStyleOption {
                value: "learning".to_string(),
                label: "Learning".to_string(),
                description: "Explains steps more directly".to_string(),
                current: false,
            },
        ],
        false,
    );

    let first = plain_text_lines(picker.cached_lines(100)).join("\n");
    let second = plain_text_lines(picker.cached_lines(100)).join("\n");

    assert_eq!(first, second);
    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, 1);

    picker.selected = 1;
    let changed = plain_text_lines(picker.cached_lines(100)).join("\n");
    assert_ne!(first, changed);
    assert_eq!(picker.lines_cache.misses, 2);
}

#[test]
#[ignore = "manual stress test for output style picker line caching"]
fn output_style_picker_lines_cache_stress_reuses_large_option_set() {
    const FRAME_COUNT: usize = 1_000;
    let options = (0..100)
        .map(|index| OutputStyleOption {
            value: format!("style-{index}"),
            label: format!("Style {index:03}"),
            description: format!("Synthetic output style {index} for render cache stress"),
            current: index == 0,
        })
        .collect::<Vec<_>>();
    let mut picker = OutputStylePickerState::new("/output-style", options, false);
    let started = Instant::now();
    let mut last_visible_len = 0;
    for _ in 0..FRAME_COUNT {
        let lines = picker.cached_lines(120);
        last_visible_len = lines.len();
    }
    let duration = started.elapsed();

    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, (FRAME_COUNT - 1) as u64);
    eprintln!(
        "frames={FRAME_COUNT} options={} visible_lines={last_visible_len} cache_hits={} cache_misses={} loop_us={}",
        picker.options.len(),
        picker.lines_cache.hits,
        picker.lines_cache.misses,
        duration.as_micros()
    );
}

#[test]
fn config_picker_lines_cache_reuses_content_until_query_changes() {
    let options = config_options(
        "sonnet".to_string(),
        ThemeSetting::Auto,
        None,
        EditorMode::Standard,
        "default",
    );
    let mut picker = ConfigPickerState {
        command: "/config".to_string(),
        output_style: "default".to_string(),
        all_options: options.clone(),
        options,
        selected: 0,
        query: String::new(),
        searching: true,
        lines_cache: ConfigPickerLinesCache::default(),
    };

    let first = plain_text_lines(picker.cached_lines(100)).join("\n");
    let second = plain_text_lines(picker.cached_lines(100)).join("\n");

    assert_eq!(first, second);
    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, 1);

    picker.push_query_char('m');
    let changed = plain_text_lines(picker.cached_lines(100)).join("\n");
    assert_ne!(first, changed);
    assert_eq!(picker.lines_cache.misses, 2);
}

#[test]
#[ignore = "manual stress test for config picker line caching"]
fn config_picker_lines_cache_stress_reuses_large_option_set() {
    const FRAME_COUNT: usize = 1_000;
    let options = (0..200)
        .map(|index| ConfigOption {
            label: format!("Synthetic setting {index:03}"),
            value: format!("value-{index}"),
            description: format!("Synthetic config option {index} for render cache stress"),
            current: index % 17 == 0,
            action: ConfigAction::Readonly,
        })
        .collect::<Vec<_>>();
    let mut picker = ConfigPickerState {
        command: "/config".to_string(),
        output_style: "default".to_string(),
        all_options: options.clone(),
        options,
        selected: 42,
        query: String::new(),
        searching: true,
        lines_cache: ConfigPickerLinesCache::default(),
    };
    let started = Instant::now();
    let mut last_visible_len = 0;
    for _ in 0..FRAME_COUNT {
        let lines = picker.cached_lines(120);
        last_visible_len = lines.len();
    }
    let duration = started.elapsed();

    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, (FRAME_COUNT - 1) as u64);
    eprintln!(
        "frames={FRAME_COUNT} options={} visible_lines={last_visible_len} cache_hits={} cache_misses={} loop_us={}",
        picker.options.len(),
        picker.lines_cache.hits,
        picker.lines_cache.misses,
        duration.as_micros()
    );
}

#[test]
fn sandbox_picker_lines_cache_reuses_content_until_tab_changes() {
    let settings = SandboxLocalSettings {
        enabled: true,
        ..SandboxLocalSettings::default()
    };
    let mut picker = SandboxPickerState::new("/sandbox", settings);

    let first = plain_text_lines(picker.cached_lines(100)).join("\n");
    let second = plain_text_lines(picker.cached_lines(100)).join("\n");

    assert_eq!(first, second);
    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, 1);

    picker.next_tab();
    let changed = plain_text_lines(picker.cached_lines(100)).join("\n");
    assert_ne!(first, changed);
    assert_eq!(picker.lines_cache.misses, 2);
}

#[test]
#[ignore = "manual stress test for sandbox picker line caching"]
fn sandbox_picker_lines_cache_stress_reuses_large_config_tab() {
    const FRAME_COUNT: usize = 1_000;

    let settings = SandboxLocalSettings {
        enabled: true,
        excluded_commands: (0..120)
            .map(|index| format!("synthetic-command-{index}"))
            .collect(),
        filesystem: SandboxFilesystemLocalSettings {
            allow_write: (0..80)
                .map(|index| format!("/tmp/allow-write-{index}"))
                .collect(),
            ..Default::default()
        },
        network: SandboxNetworkLocalSettings {
            allowed_domains: (0..80)
                .map(|index| format!("example-{index}.test"))
                .collect(),
            ..Default::default()
        },
        ..SandboxLocalSettings::default()
    };
    let mut picker = SandboxPickerState::new("/sandbox", settings);
    picker.tab = SandboxTab::Config;

    let started = Instant::now();
    let mut last_visible_len = 0;
    for _ in 0..FRAME_COUNT {
        let lines = picker.cached_lines(120);
        last_visible_len = lines.len();
    }
    let duration = started.elapsed();

    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, (FRAME_COUNT - 1) as u64);
    eprintln!(
        "frames={FRAME_COUNT} excluded_commands={} visible_lines={last_visible_len} cache_hits={} cache_misses={} loop_us={}",
        picker.settings.excluded_commands.len(),
        picker.lines_cache.hits,
        picker.lines_cache.misses,
        duration.as_micros()
    );
}

#[test]
fn memory_picker_lines_cache_reuses_content_until_selection_changes() {
    let cwd = PathBuf::from("/tmp/project");
    let overview = MemoryOverview {
        user_memory: MemoryFileOverview {
            label: "User memory".to_string(),
            path: PathBuf::from("/tmp/user/CLAUDE.md"),
            exists: true,
            content: None,
            status: orbcode_protocol::MemorySourceStatus::Empty,
            writable: true,
            trust_boundary: None,
            scope: None,
            skipped_reason: None,
        },
        project_memories: vec![MemoryFileOverview {
            label: "Project memory".to_string(),
            path: cwd.join("CLAUDE.md"),
            exists: true,
            content: None,
            status: orbcode_protocol::MemorySourceStatus::Empty,
            writable: true,
            trust_boundary: None,
            scope: None,
            skipped_reason: None,
        }],
        auto_memory_enabled: true,
        auto_memory_dir: PathBuf::from("/tmp/auto-memory"),
    };
    let mut picker = MemoryPickerState::new("/memory", overview);

    let first = plain_text_lines(picker.cached_lines(&cwd, 100)).join("\n");
    let second = plain_text_lines(picker.cached_lines(&cwd, 100)).join("\n");

    assert_eq!(first, second);
    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, 1);

    picker.selected = 1;
    let changed = plain_text_lines(picker.cached_lines(&cwd, 100)).join("\n");
    assert_ne!(first, changed);
    assert_eq!(picker.lines_cache.misses, 2);
}

#[test]
fn memory_picker_does_not_edit_read_only_sources() {
    let cwd = PathBuf::from("/tmp/project");
    let overview = MemoryOverview {
        user_memory: MemoryFileOverview {
            label: "Managed memory".to_string(),
            path: PathBuf::from("/etc/claude-code/CLAUDE.md"),
            exists: true,
            content: None,
            status: MemorySourceStatus::Skipped,
            writable: false,
            trust_boundary: Some("managed policy".to_string()),
            scope: None,
            skipped_reason: Some("managed memory is read-only".to_string()),
        },
        project_memories: Vec::new(),
        auto_memory_enabled: false,
        auto_memory_dir: PathBuf::from("/tmp/auto-memory"),
    };
    let mut picker = MemoryPickerState::new("/memory", overview);

    let rendered = plain_text_lines(picker.cached_lines(&cwd, 200)).join("\n");
    assert!(rendered.contains("Managed memory [read-only, skipped]"));
    assert!(rendered.contains("managed memory is read-only"));
    assert_eq!(
        apply_memory_picker_key(
            &mut picker,
            &crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ),
        MemoryPickerKeyAction::None
    );
}

#[test]
#[ignore = "manual stress test for memory picker line caching"]
fn memory_picker_lines_cache_stress_reuses_large_memory_list() {
    const FRAME_COUNT: usize = 1_000;
    let cwd = PathBuf::from("/tmp/project");
    let overview = MemoryOverview {
        user_memory: MemoryFileOverview {
            label: "User memory".to_string(),
            path: PathBuf::from("/tmp/user/CLAUDE.md"),
            exists: true,
            content: None,
            status: orbcode_protocol::MemorySourceStatus::Empty,
            writable: true,
            trust_boundary: None,
            scope: None,
            skipped_reason: None,
        },
        project_memories: (0..200)
            .map(|index| MemoryFileOverview {
                label: format!("Project memory {index:03}"),
                path: cwd.join(format!("nested/{index}/CLAUDE.md")),
                exists: true,
                content: None,
                status: orbcode_protocol::MemorySourceStatus::Empty,
                writable: true,
                trust_boundary: None,
                scope: None,
                skipped_reason: None,
            })
            .collect(),
        auto_memory_enabled: true,
        auto_memory_dir: PathBuf::from("/tmp/auto-memory"),
    };
    let mut picker = MemoryPickerState::new("/memory", overview);
    picker.selected = 120;
    let started = Instant::now();
    let mut last_visible_len = 0;
    for _ in 0..FRAME_COUNT {
        let lines = picker.cached_lines(&cwd, 120);
        last_visible_len = lines.len();
    }
    let duration = started.elapsed();

    assert_eq!(picker.lines_cache.misses, 1);
    assert_eq!(picker.lines_cache.hits, (FRAME_COUNT - 1) as u64);
    eprintln!(
        "frames={FRAME_COUNT} items={} visible_lines={last_visible_len} cache_hits={} cache_misses={} loop_us={}",
        picker.items.len(),
        picker.lines_cache.hits,
        picker.lines_cache.misses,
        duration.as_micros()
    );
}

// ---------------------------------------------------------------------------
// Output style picker locked state
// ---------------------------------------------------------------------------

#[test]
fn output_style_picker_locked_shows_lock_badge_and_blocks_enter() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let options = vec![
        OutputStyleOption {
            value: "default".to_string(),
            label: "Default".to_string(),
            description: "Balanced response style".to_string(),
            current: true,
        },
        OutputStyleOption {
            value: "learning".to_string(),
            label: "Learning".to_string(),
            description: "Explains steps more directly".to_string(),
            current: false,
        },
    ];
    let mut picker = OutputStylePickerState::new("/output-style", options, true);

    let lines_text = plain_text_lines(picker.cached_lines(100)).join("\n");
    assert!(lines_text.contains("managed-locked"));
    assert!(lines_text.contains("cannot be changed"));
    assert!(lines_text.contains("Esc to close"));
    assert!(!lines_text.contains("Enter to select"));

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let action = apply_output_style_picker_key(&mut picker, &enter);
    assert!(matches!(action, OutputStylePickerKeyAction::None));
}

#[test]
fn output_style_picker_unlocked_allows_enter() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let options = vec![OutputStyleOption {
        value: "default".to_string(),
        label: "Default".to_string(),
        description: "Balanced response style".to_string(),
        current: true,
    }];
    let mut picker = OutputStylePickerState::new("/output-style", options, false);

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let action = apply_output_style_picker_key(&mut picker, &enter);
    assert!(matches!(
        action,
        OutputStylePickerKeyAction::SetOutputStyle { .. }
    ));
}

#[test]
fn output_style_picker_handles_namespaced_values() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let options = vec![
        OutputStyleOption {
            value: "default".to_string(),
            label: "Default".to_string(),
            description: "Balanced response style".to_string(),
            current: false,
        },
        OutputStyleOption {
            value: "demo:Concise".to_string(),
            label: "demo:Concise".to_string(),
            description: "Short plugin replies".to_string(),
            current: true,
        },
    ];
    let mut picker = OutputStylePickerState::new("/output-style", options, false);

    let lines_text = plain_text_lines(picker.cached_lines(100)).join("\n");
    assert!(lines_text.contains("demo:Concise ✔"), "{lines_text}");

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let action = apply_output_style_picker_key(&mut picker, &enter);
    assert!(matches!(
        action,
        OutputStylePickerKeyAction::SetOutputStyle { style, .. } if style == "demo:Concise"
    ));
}

// ---------------------------------------------------------------------------
// MCP auth rendering
// ---------------------------------------------------------------------------

#[test]
fn mcp_auth_render_overview_with_entries() {
    use orbcode_app_server::{McpOAuthOverview, McpOAuthStatusEntry};
    use std::path::PathBuf;

    use crate::commands::local_output::render_mcp_oauth_overview;

    let overview = McpOAuthOverview {
        store_path: PathBuf::from("/home/user/.claude/oauth_tokens.json"),
        entries: vec![McpOAuthStatusEntry {
            server_id: "my-server".to_string(),
            source_summary: "stdio".to_string(),
            usable: true,
            expired: false,
            has_refresh_token: true,
            has_token_endpoint: true,
            expires_at: Some(1700000000),
            scopes: vec!["read".to_string(), "write".to_string()],
            updated_at: Some(1699999000),
        }],
    };

    let rendered = render_mcp_oauth_overview(&overview);
    assert!(rendered.contains("my-server"));
    assert!(rendered.contains("status=ready"));
    assert!(rendered.contains("refresh=yes"));
    assert!(rendered.contains("scopes=read,write"));
    assert!(rendered.contains("oauth_tokens.json"));
}

#[test]
fn mcp_auth_render_overview_empty() {
    use orbcode_app_server::McpOAuthOverview;
    use std::path::PathBuf;

    use crate::commands::local_output::render_mcp_oauth_overview;

    let overview = McpOAuthOverview {
        store_path: PathBuf::from("/tmp/tokens.json"),
        entries: vec![],
    };

    let rendered = render_mcp_oauth_overview(&overview);
    assert_eq!(rendered, "No MCP OAuth tokens stored.");
}
