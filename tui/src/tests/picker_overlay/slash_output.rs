use crate::tests::support::*;

#[test]
fn slash_command_output_note_hides_default_summary_and_strips_markdown_quote() {
    let message = TranscriptMessage::new(
            MessageRole::System,
            encode_slash_command_output_note(
                "/memory".to_string(),
                "Opened memory file at ./CLAUDE.md".to_string(),
                Some(
                    "> Using $EDITOR=\"nvim\". To change editor, set $EDITOR or $VISUAL environment variable."
                        .to_string(),
                ),
            ),
        );
    let note = parse_local_transcript_note(&message).expect("slash command note");

    let rendered = plain_text_lines(&render_local_transcript_note_lines(note, 62, false));
    let rendered_text = rendered.join("\n");

    assert_eq!(rendered[0], "❯ /memory");
    assert!(
        rendered.iter().any(|line| line.starts_with("  └  Tip: ")),
        "{rendered_text}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("   Using $EDITOR=\"nvim\"")),
        "{rendered_text}"
    );
    assert!(
        !rendered_text.contains("Opened memory file"),
        "{rendered_text}"
    );
    assert!(!rendered_text.contains("▎ Using"), "{rendered_text}");
    assert!(!rendered_text.contains("> Using"), "{rendered_text}");
}

#[test]
fn slash_command_output_note_hides_default_summary_and_renders_direct_detail() {
    let message = TranscriptMessage::new(
        MessageRole::System,
        encode_slash_command_output_note(
            "/status".to_string(),
            "Status loaded.".to_string(),
            Some("Status:\n\n## Plan\nsession: abc123\nmodel: `test-model`".to_string()),
        ),
    );
    let note = parse_local_transcript_note(&message).expect("slash command note");

    let styled_rendered = render_local_transcript_note_lines(note, 80, false);
    let rendered = plain_text_lines(&styled_rendered);
    let rendered_text = rendered.join("\n");

    assert_eq!(rendered[0], "❯ /status");
    assert!(
        rendered.iter().any(|line| line.starts_with("  └  Tip: ")),
        "{rendered_text}"
    );
    assert!(
        rendered.iter().any(|line| line == "   Status:"),
        "{rendered_text}"
    );
    assert!(
        rendered.iter().any(std::string::String::is_empty),
        "{rendered_text}"
    );
    assert!(
        rendered.iter().any(|line| line == "   Plan"),
        "{rendered_text}"
    );
    assert!(
        rendered.iter().any(|line| line == "   session: abc123"),
        "{rendered_text}"
    );
    assert!(
        rendered.iter().any(|line| line == "   model: test-model"),
        "{rendered_text}"
    );
    assert!(!rendered_text.contains("Status loaded."), "{rendered_text}");
    assert!(!rendered_text.contains("▎"), "{rendered_text}");
    let detail_line = styled_rendered
        .iter()
        .find(|line| plain_text_line(line) == "   Status:")
        .expect("detail line");
    assert_eq!(detail_line.spans[0].style, subtle_style());
}

#[test]
fn slash_command_output_note_can_render_extended_tip_pool() {
    let message = local_slash_command_output_message(
        "/permissions".to_string(),
        "Permissions loaded.".to_string(),
        None,
        slash_commands::SlashCommandDeferredFeedback::Direct,
    );
    let note = parse_local_transcript_note(&message).expect("slash command note");

    let rendered = plain_text_lines(&render_local_transcript_note_lines(note, 100, false));
    let rendered_text = rendered.join("\n");

    assert!(
        rendered_text.contains("  └  Tip: Use /allowed-tools as an alias for /permissions."),
        "{rendered_text}"
    );
    assert!(
        !rendered_text.contains("Permissions loaded."),
        "{rendered_text}"
    );
}

#[test]
fn slash_command_output_note_renders_dimmed_tip_when_summary_is_hidden() {
    let message = local_slash_command_output_message(
        "/help".to_string(),
        "Opened help.".to_string(),
        None,
        slash_commands::SlashCommandDeferredFeedback::Direct,
    );
    let note = parse_local_transcript_note(&message).expect("slash command note");

    let rendered = render_local_transcript_note_lines(note, 80, false);
    let rendered_text = plain_text_lines(&rendered).join("\n");

    assert!(
        rendered_text.contains("  └  Help: ↑↓ scroll, Esc close."),
        "{rendered_text}"
    );
    assert!(!rendered_text.contains("Opened help."), "{rendered_text}");
    let tip_line = rendered
        .iter()
        .find(|line| plain_text_line(line).starts_with("  └  Help:"))
        .expect("tip line");
    for span in &tip_line.spans {
        assert_eq!(span.style, subtle_style());
    }
}

#[test]
fn stats_slash_command_output_renders_summary_and_panel() {
    let detail = "     Jan\n     ▪ ░ ▒ ▓ ■\n  M  ▪ ▪ ▪ ▪ ▪\n     Less ▪ ░ ▒ ▓ ■ More";
    let message = local_slash_command_output_message(
        "/stats".to_string(),
        "Last 14 days · 8 messages.".to_string(),
        Some(detail.to_string()),
        slash_commands::SlashCommandDeferredFeedback::Direct,
    );
    let note = parse_local_transcript_note(&message).expect("stats slash command note");

    let styled_rendered = render_local_transcript_note_lines(note, 100, false);
    let rendered = plain_text_lines(&styled_rendered);
    let rendered_text = rendered.join("\n");

    assert_eq!(rendered.first().map(String::as_str), Some("❯ /stats"));
    assert!(
        rendered
            .iter()
            .any(|line| line == "  └  Last 14 days · 8 messages."),
        "{rendered_text}"
    );
    assert!(rendered_text.contains("     ■ ■ ■ ■ ■"), "{rendered_text}");

    let cell_line = styled_rendered
        .iter()
        .find(|line| plain_text_line(line).contains("■ ■ ■ ■ ■"))
        .expect("styled cell line");
    let cell_styles = cell_line
        .spans
        .iter()
        .filter(|span| span.content.as_ref() == "■")
        .map(|span| span.style.fg)
        .collect::<Vec<_>>();
    assert!(cell_styles.windows(2).any(|pair| pair[0] != pair[1]));
    assert_eq!(
        cell_styles,
        vec![
            Some(Color::Rgb(0x17, 0x39, 0x43)),
            Some(Color::Rgb(0x2a, 0x47, 0x50)),
            Some(Color::Rgb(0x57, 0x6d, 0x72)),
            Some(Color::Rgb(0x84, 0x94, 0x95)),
            Some(Color::Rgb(0x95, 0xa3, 0xa3)),
        ]
    );
}

#[test]
fn stats_heatmap_colors_are_relative_to_panel_background() {
    let colors = (0..5).map(stats_heatmap_color).collect::<Vec<_>>();

    assert_eq!(
        colors,
        vec![
            Color::Rgb(0x17, 0x39, 0x43),
            Color::Rgb(0x2a, 0x47, 0x50),
            Color::Rgb(0x57, 0x6d, 0x72),
            Color::Rgb(0x84, 0x94, 0x95),
            Color::Rgb(0x95, 0xa3, 0xa3),
        ]
    );
}

#[test]
fn context_compacted_note_renders_collapsed_and_expanded_summary() {
    let message = local_context_compacted_message(
        Some(3_000),
        Some("**Summary:**\n- Read `orbcode/tui/src/lib.rs`.".to_string()),
    );
    let note = parse_local_transcript_note(&message).expect("context compacted note");

    let rendered = plain_text_lines(&render_local_transcript_note_lines(note.clone(), 48, false));

    assert_eq!(rendered, vec!["✻ Conversation compacted"]);

    let expanded = plain_text_lines(&render_local_transcript_note_lines(note, 48, true));
    let expanded_text = expanded.join("\n");
    assert!(
        expanded_text.contains("✻ Crunched for 3s"),
        "{expanded_text}"
    );
    assert!(
        expanded_text.contains("⏺ Compact summary"),
        "{expanded_text}"
    );
    assert!(expanded_text.contains("Summary:"), "{expanded_text}");
    assert!(!expanded_text.contains("**Summary:**"), "{expanded_text}");
}

#[test]
fn compact_slash_output_renders_file_details_as_result_lines() {
    let message = TranscriptMessage::new(
        MessageRole::System,
        encode_slash_command_output_note(
            "/compact".to_string(),
            "Compacted (ctrl+o to see full summary)".to_string(),
            Some(
                "Read orbcode/core/src/lib.rs (54 lines)\nReferenced file orbcode/tui/src/lib.rs"
                    .to_string(),
            ),
        ),
    );
    let note = parse_local_transcript_note(&message).expect("slash command note");

    let rendered = plain_text_lines(&render_local_transcript_note_lines(note, 80, false));
    let rendered_text = rendered.join("\n");

    assert_eq!(rendered.first().map(String::as_str), Some("❯ /compact"));
    assert!(
        !rendered_text.contains("Compacted (ctrl+o to see full summary)"),
        "{rendered_text}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line == "  └  Read orbcode/core/src/lib.rs (54 lines)"),
        "{rendered_text}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line == "  └  Referenced file orbcode/tui/src/lib.rs"),
        "{rendered_text}"
    );
    assert!(!rendered_text.contains("▎"), "{rendered_text}");
}

#[test]
fn slash_command_summary_line_uses_dim_style_when_enabled() {
    let message = local_slash_command_output_message(
        "/stats".to_string(),
        "Last 14 days · 8 messages.".to_string(),
        None,
        slash_commands::SlashCommandDeferredFeedback::Direct,
    );
    let note = parse_local_transcript_note(&message).expect("slash command note");

    let rendered = render_local_transcript_note_lines(note, 80, false);
    let summary_line = rendered
        .iter()
        .find(|line| plain_text_line(line).contains("Last 14 days"))
        .expect("stats summary line");
    let summary_span = summary_line
        .spans
        .iter()
        .find(|span| span.content.contains("Last 14 days"))
        .expect("stats summary span");

    assert!(summary_span.style.add_modifier.contains(Modifier::DIM));
}

#[test]
fn compact_restored_file_detail_lines_include_recent_read_results() {
    let cwd = PathBuf::from("/tmp/project");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::ToolUse {
                    id: "read-1".to_string(),
                    name: "Read".to_string(),
                    input: r#"{"file_path":"/tmp/project/src/lib.rs"}"#.to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "read-2".to_string(),
                    name: "Read".to_string(),
                    input: r#"{"file_path":"/tmp/project/src/main.rs"}"#.to_string(),
                },
            ],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "read-1".to_string(),
                content: "one\ntwo\nthree\n".to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let lines = compact_restored_file_detail_lines(&messages, &cwd);

    assert_eq!(
        lines,
        vec![
            "Read src/lib.rs (3 lines)".to_string(),
            "Referenced file src/main.rs".to_string(),
        ]
    );
}

#[test]
fn memory_editor_result_uses_slash_command_output_note() {
    let mut state = normal_state("", 0);
    state.cwd = PathBuf::from("/tmp/project");

    state.report_external_editor_result(
        ExternalEditorRequest {
            command: "/memory".to_string(),
            path: PathBuf::from("/tmp/project/CLAUDE.md"),
            target: ExternalEditorTarget::Memory,
        },
        Ok(EditorLaunchInfo {
            source: "$EDITOR",
            value: "nvim".to_string(),
        }),
    );

    let transcript = plain_text_lines(&state.transcript_lines(72)).join("\n");

    assert!(transcript.contains("❯ /memory"), "{transcript}");
    assert!(
        !transcript.contains("└  Opened memory file at ./CLAUDE.md"),
        "{transcript}"
    );
    assert!(
        transcript.contains("   Using $EDITOR=\"nvim\""),
        "{transcript}"
    );
    assert!(!transcript.contains("▎ Using"), "{transcript}");
    assert!(!transcript.contains("> Using"), "{transcript}");
    assert_eq!(state.status_line, "Opened memory file at ./CLAUDE.md.");
}

#[test]
fn render_workspace_diff_includes_status_diff_and_untracked_files() {
    let diff = WorkspaceDiff {
        cwd: PathBuf::from("/tmp/project"),
        status: "AM src/main.rs\n?? scratch.txt".to_string(),
        staged_diff: "diff --git a/src/main.rs b/src/main.rs\n+fn main() {}".to_string(),
        unstaged_diff: "diff --git a/src/main.rs b/src/main.rs\n+println!(\"hi\");".to_string(),
        untracked_files: vec!["scratch.txt".to_string()],
    };

    let rendered = render_workspace_diff(&diff);

    assert!(rendered.contains("Workspace diff:"));
    assert!(rendered.contains("cwd: /tmp/project"));
    assert!(rendered.contains("Status:"));
    assert!(rendered.contains("AM src/main.rs"));
    assert!(rendered.contains("Staged changes:"));
    assert!(rendered.contains("Unstaged changes:"));
    assert!(rendered.contains("scratch.txt"));
    assert_eq!(workspace_diff_changed_path_count(&diff), 2);
}

#[test]
fn diff_overlay_parses_file_tabs_and_hunk_lines() {
    let diff_text = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -84,3 +84,5 @@ impl AppServer {
 pub async fn new() {
+    let cwd = PathBuf::new();
-    old_call();
     Ok(())
 }
";
    let files = parse_unified_diff_files(diff_text);

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/lib.rs");
    assert_eq!(files[0].added, 1);
    assert_eq!(files[0].removed, 1);
    assert!(
        files[0]
            .lines
            .iter()
            .any(|line| line.marker == '+' && line.new_line == Some(85))
    );

    let state = DiffOverlayState::new(WorkspaceDiff {
        cwd: PathBuf::from("/tmp/project"),
        status: " M src/lib.rs".to_string(),
        staged_diff: String::new(),
        unstaged_diff: diff_text.to_string(),
        untracked_files: Vec::new(),
    });
    let line_text = plain_text_lines(&diff_overlay_visible_lines(
        &state,
        &diff_files_for_overlay(&state),
        Rect::new(0, 0, 100, 4),
    ))
    .join("\n");
    assert!(!line_text.contains("Diff Mode"));
    assert!(line_text.contains("+"));
    assert!(line_text.contains("let cwd"));
}

#[test]
fn diff_overlay_file_content_cache_reuses_syntax_across_scroll() {
    let mut state = DiffOverlayState::new(large_workspace_diff());
    let area = Rect::new(0, 0, 100, 8);

    let first = plain_text_lines(&state.cached_visible_lines(area)).join("\n");
    state.line_scroll = 5;
    let second = plain_text_lines(&state.cached_visible_lines(area)).join("\n");

    assert_ne!(first, second);
    assert_eq!(state.files_cache.misses, 1);
    assert_eq!(state.files_cache.hits, 2);
    assert_eq!(state.file_content_cache.misses, 1);
    assert_eq!(state.file_content_cache.hits, 1);

    state.selected_file = 1;
    state.line_scroll = 0;
    let _ = state.cached_visible_lines(area);
    assert_eq!(state.files_cache.misses, 1);
    assert_eq!(state.file_content_cache.misses, 2);
}

#[test]
#[ignore = "manual stress test for diff overlay content caching"]
fn diff_overlay_file_content_cache_stress_reuses_large_diff_across_scroll() {
    const FRAME_COUNT: usize = 1_000;

    let mut state = DiffOverlayState::new(large_workspace_diff());
    let area = Rect::new(0, 0, 114, 20);
    let started = Instant::now();
    let mut last_visible_len = 0;
    for frame in 0..FRAME_COUNT {
        state.line_scroll = frame % 100;
        let lines = state.cached_visible_lines(area);
        assert!(lines.len() <= area.height as usize);
        last_visible_len = lines.len();
    }
    let duration = started.elapsed();
    let files = state.files_cache.unstaged_files.as_ref().unwrap();
    let file_count = files.len();
    let selected_line_count = files[state.selected_file].lines.len();

    assert_eq!(state.files_cache.misses, 1);
    assert_eq!(state.file_content_cache.misses, 1);
    assert_eq!(state.file_content_cache.hits, (FRAME_COUNT - 1) as u64);
    eprintln!(
        "frames={FRAME_COUNT} files={file_count} selected_lines={selected_line_count} visible_lines={last_visible_len} files_cache_hits={} files_cache_misses={} file_content_cache_hits={} file_content_cache_misses={} loop_us={}",
        state.files_cache.hits,
        state.files_cache.misses,
        state.file_content_cache.hits,
        state.file_content_cache.misses,
        duration.as_micros()
    );
}

#[test]
fn diff_overlay_tabs_follow_selection_and_disambiguate_duplicate_names() {
    let files = vec![
        DiffFile {
            path: "orbcode/app-server/src/lib.rs".to_string(),
            added: 1,
            removed: 0,
            lines: Vec::new(),
        },
        DiffFile {
            path: "orbcode/plan.md".to_string(),
            added: 1,
            removed: 0,
            lines: Vec::new(),
        },
        DiffFile {
            path: "orbcode/tui/src/lib.rs".to_string(),
            added: 1,
            removed: 0,
            lines: Vec::new(),
        },
        DiffFile {
            path: "orbcode/tui/src/slash_commands.rs".to_string(),
            added: 1,
            removed: 0,
            lines: Vec::new(),
        },
        DiffFile {
            path: "orbcode/.claude/settings.local.json".to_string(),
            added: 1,
            removed: 0,
            lines: Vec::new(),
        },
        DiffFile {
            path: "src/extra.rs".to_string(),
            added: 1,
            removed: 0,
            lines: Vec::new(),
        },
    ];
    let labels = diff_tab_labels(&files);
    assert!(labels.contains(&"app-server/src/lib.rs".to_string()));
    assert!(labels.contains(&"tui/src/lib.rs".to_string()));

    let state = DiffOverlayState {
        diff: WorkspaceDiff {
            cwd: PathBuf::from("/tmp/project"),
            status: String::new(),
            staged_diff: String::new(),
            unstaged_diff: String::new(),
            untracked_files: Vec::new(),
        },
        mode: DiffOverlayMode::Unstaged,
        selected_file: 5,
        line_scroll: 0,
        max_line_scroll: 0,
        files_cache: DiffOverlayFilesCache::default(),
        file_content_cache: DiffOverlayFileContentCache::default(),
    };
    let rendered = plain_text_lines(&[diff_overlay_tab_line(&state, &files, 52)]).join("\n");
    assert!(rendered.contains("extra.rs"));
    assert!(rendered.contains("[6/6]"));
}

#[test]
fn diff_overlay_added_and_removed_lines_use_distinct_backgrounds() {
    let added = render_diff_line(
        &DiffRenderLine {
            old_line: None,
            new_line: Some(1),
            marker: '+',
            content: "fn main() {}".to_string(),
            kind: DiffLineKind::Added,
        },
        true,
        2,
        "rs",
    );
    let removed = render_diff_line(
        &DiffRenderLine {
            old_line: Some(1),
            new_line: None,
            marker: '-',
            content: "fn old() {}".to_string(),
            kind: DiffLineKind::Removed,
        },
        false,
        2,
        "rs",
    );

    assert!(
        added
            .spans
            .iter()
            .any(|span| span.style.bg == Some(DIFF_ADDED_BG))
    );
    assert!(
        removed
            .spans
            .iter()
            .any(|span| span.style.bg == Some(DIFF_REMOVED_BG))
    );
}

#[test]
fn diff_overlay_separator_fits_available_width() {
    let separator = render_diff_line_with_syntax(
        &DiffRenderLine {
            old_line: None,
            new_line: None,
            marker: ' ',
            content: String::new(),
            kind: DiffLineKind::Separator,
        },
        false,
        2,
        24,
        None,
        "rs",
    );

    assert_eq!(styled_line_display_width(&separator), 24);
}

#[test]
fn diff_overlay_keys_support_vi_file_and_line_navigation() {
    let mut state = DiffOverlayState::new(WorkspaceDiff {
        cwd: PathBuf::from("/tmp/project"),
        status: String::new(),
        staged_diff: String::new(),
        unstaged_diff: String::new(),
        untracked_files: vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()],
    });
    state.max_line_scroll = 4;

    apply_diff_overlay_key(
        &mut state,
        &crossterm::event::KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
    );
    assert_eq!(state.selected_file, 1);

    apply_diff_overlay_key(
        &mut state,
        &crossterm::event::KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
    );
    assert_eq!(state.selected_file, 0);

    apply_diff_overlay_key(
        &mut state,
        &crossterm::event::KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
    );
    assert_eq!(state.line_scroll, 1);

    apply_diff_overlay_key(
        &mut state,
        &crossterm::event::KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
    );
    assert_eq!(state.line_scroll, 0);

    state.line_scroll = 2;
    apply_diff_overlay_key(
        &mut state,
        &crossterm::event::KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
    );
    assert_eq!(state.line_scroll, 0);
    assert_eq!(state.mode, DiffOverlayMode::Unstaged);

    apply_diff_overlay_key(
        &mut state,
        &crossterm::event::KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
    );
    assert_eq!(state.mode, DiffOverlayMode::Staged);

    assert_eq!(
        apply_diff_overlay_key(
            &mut state,
            &crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        ),
        DiffOverlayAction::Close
    );
}

#[test]
fn render_turn_context_includes_git_and_agents_context() {
    let context = TurnContext {
        cwd: "/tmp/project/src".to_string(),
        additional_directories: vec!["/tmp/other".to_string()],
        repo_root: Some("/tmp/project".to_string()),
        cwd_relative_to_repo: Some("src".to_string()),
        current_date: "2026-05-02".to_string(),
        git_branch: Some("main".to_string()),
        git_status: Some("M orbcode/tui/src/lib.rs".to_string()),
        claude_md: Some("Project instructions".to_string()),
        ..Default::default()
    };

    let rendered = render_turn_context(&context);

    assert!(rendered.contains("Context snapshot:"));
    assert!(rendered.contains("cwd: /tmp/project/src"));
    assert!(rendered.contains("repo root: /tmp/project"));
    assert!(rendered.contains("repo subdir: src"));
    assert!(rendered.contains("git branch: main"));
    assert!(rendered.contains("git status: M orbcode/tui/src/lib.rs"));
    assert!(rendered.contains("AGENTS.md: loaded"));
    assert!(rendered.contains("Project instructions"));
}

#[test]
fn render_doctor_report_includes_summary_and_checks() {
    let report = DoctorReport {
        checks: vec![
            DoctorCheck {
                name: "workspace".to_string(),
                status: DoctorStatus::Pass,
                detail: "/tmp/project".to_string(),
            },
            DoctorCheck {
                name: "auth".to_string(),
                status: DoctorStatus::Warn,
                detail: "missing token\nfallback available".to_string(),
            },
            DoctorCheck {
                name: "sandbox".to_string(),
                status: DoctorStatus::Fail,
                detail: "not enforced".to_string(),
            },
        ],
    };

    let rendered = render_doctor_report(&report);

    assert!(rendered.contains("Doctor summary: pass=1 warn=1 fail=1"));
    assert!(rendered.contains("PASS workspace"));
    assert!(rendered.contains("WARN auth"));
    assert!(rendered.contains("missing token fallback available"));
    assert!(rendered.contains("FAIL sandbox"));
}
