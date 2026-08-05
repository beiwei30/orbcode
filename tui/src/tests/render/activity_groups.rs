use crate::tests::support::*;

#[test]
fn collapsed_activity_group_summarizes_search_and_read_sequences() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-search".to_string(),
                name: "Glob".to_string(),
                input: "{\n  \"pattern\": \"src/**/*\"\n}".to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-search".to_string(),
                content: "src/main.tsx".to_string().into(),
                is_error: false,
                metadata: None,
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-read".to_string(),
                name: "Read".to_string(),
                input: "{\n  \"file_path\": \"/Users/user/github/sample-repo/package.json\"\n}"
                    .to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-read".to_string(),
                content: "{\n  \"name\": \"sample-repo\"\n}".to_string().into(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (group, next_index) = build_collapsed_activity_group(&messages, 0, &cwd).unwrap();
    assert_eq!(next_index, 4);
    assert_eq!(group.search_count, 1);
    assert_eq!(group.read_count(), 1);
    assert_eq!(group.latest_hint.as_deref(), Some("package.json"));

    let lines = render_collapsed_activity_group_lines(&group, false, false, true);
    let rendered = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert_eq!(rendered.len(), 1);
    assert!(
        rendered[0].starts_with("  Searched for 1 pattern, read 1 file"),
        "{rendered:#?}"
    );
    assert!(rendered[0].contains("Searched for 1 pattern, read 1 file (ctrl+o to expand)"));
}

#[test]
fn collapsed_activity_group_distinguishes_failed_reads_in_summary() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::ToolUse {
                    id: "tool-read-ok".to_string(),
                    name: "Read".to_string(),
                    input: "{\"file_path\":\"/Users/user/github/sample-repo/package.json\"}"
                        .to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "tool-read-failed".to_string(),
                    name: "Read".to_string(),
                    input: "{\"file_path\":\"/Users/user/github/sample-repo/big.log\"}".to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "tool-list".to_string(),
                    name: "LS".to_string(),
                    input: "{\"path\":\"/Users/user/github/sample-repo/src\"}".to_string(),
                },
            ],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![
                TranscriptBlock::ToolResult {
                    tool_use_id: "tool-read-ok".to_string(),
                    content: "{\n  \"name\": \"sample-repo\"\n}".to_string().into(),
                    is_error: false,
                    metadata: None,
                },
                TranscriptBlock::ToolResult {
                    tool_use_id: "tool-read-failed".to_string(),
                    content: "File content (50513 tokens) exceeds maximum allowed tokens (25000)."
                        .to_string()
                        .into(),
                    is_error: true,
                    metadata: None,
                },
                TranscriptBlock::ToolResult {
                    tool_use_id: "tool-list".to_string(),
                    content: "main.tsx".to_string().into(),
                    is_error: false,
                    metadata: None,
                },
            ],
        ),
    ];

    let (group, next_index) = build_collapsed_activity_group(&messages, 0, &cwd).unwrap();
    assert_eq!(next_index, 2);
    assert_eq!(group.read_count(), 2);
    assert_eq!(group.failed_read_count(), 1);

    let lines = render_collapsed_activity_group_lines(&group, false, false, true);
    let rendered = plain_text_lines(&lines);
    assert_eq!(
        rendered[0],
        "  Read 1 file, 1 file failed, listed 1 directory (ctrl+o to expand)"
    );
    assert!(
        rendered[1].contains("File content (50513 tokens) exceeds maximum allowed tokens (25000).")
    );
}

#[test]
fn collapsed_activity_group_labels_grep_regex_without_json_quotes() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "tool-grep".to_string(),
            name: "Grep".to_string(),
            input:
                r#"{"pattern":"ToolSpec\\s*\\{","path":"orbcode/tools/src","output_mode":"content"}"#
                    .to_string(),
        }],
    )];

    let (group, next_index) = build_collapsed_activity_group(&messages, 0, &cwd).unwrap();
    assert_eq!(next_index, 1);
    assert_eq!(group.search_count, 1);
    assert_eq!(group.latest_hint.as_deref(), Some(r"Regex ToolSpec\s*\{"));
    assert!(
        group
            .detail_lines
            .iter()
            .any(|line| line == r"Regex ToolSpec\s*\{ in orbcode/tools/src")
    );

    let lines = render_collapsed_activity_group_lines(&group, false, true, true);
    let rendered = plain_text_lines(&lines);
    assert!(
        rendered
            .iter()
            .any(|line| line.contains(r"└ Regex ToolSpec\s*\{"))
    );
    assert!(!rendered.iter().any(|line| line.contains(r#""ToolSpec"#)));
}

#[test]
fn collapsed_activity_group_uses_spinner_when_active() {
    let group = CollapsedActivityGroup {
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

    let lines = render_collapsed_activity_group_lines(&group, false, true, true);
    let rendered = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(rendered[0].starts_with(&platform_tool_line(
        "Searching for 1 pattern, reading 1 file..."
    )));
}

#[test]
fn collapsed_activity_group_hides_dot_on_blink_off_frame() {
    let group = CollapsedActivityGroup {
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

    let lines = render_collapsed_activity_group_lines(&group, false, true, false);
    let rendered = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(rendered[0].starts_with("  Searching for 1 pattern, reading 1 file..."));
}

#[test]
fn completed_collapsed_activity_group_keeps_summary_and_errors_muted() {
    let group = CollapsedActivityGroup {
        search_count: 2,
        read_paths: vec!["README.md".to_string()],
        read_operation_count: 0,
        read_tool_use_ids: HashSet::new(),
        failed_read_tool_use_ids: HashSet::new(),
        list_count: 1,
        latest_hint: None,
        detail_lines: Vec::new(),
        error_messages: vec![
            "tool execution failed: command `tree ...`".to_string(),
            "tool execution failed: command `for dir ...`".to_string(),
        ],
        messages: Vec::new(),
        tool_use_ids: HashSet::new(),
        matched_tool_use_ids: HashSet::new(),
        tool_results: ToolResultIndex::new(),
    };

    let lines = render_collapsed_activity_group_lines(&group, false, false, true);

    assert_eq!(lines[0].spans[2].style, inactive_style());
    assert_eq!(
        lines[1].spans[1].style,
        inactive_style().add_modifier(Modifier::DIM)
    );
    assert_eq!(
        lines[2].spans[1].style,
        inactive_style().add_modifier(Modifier::DIM)
    );
    assert_eq!(
        plain_text_line(&lines[1]),
        "  └ tool execution failed: command `tree ...`"
    );
    assert_eq!(
        plain_text_line(&lines[2]),
        "    tool execution failed: command `for dir ...`"
    );
}

#[test]
fn active_collapsed_activity_group_uses_tree_prefixes_for_hint_and_errors() {
    let group = CollapsedActivityGroup {
        search_count: 1,
        read_paths: vec!["src/main.rs".to_string()],
        read_operation_count: 0,
        read_tool_use_ids: HashSet::new(),
        failed_read_tool_use_ids: HashSet::new(),
        list_count: 0,
        latest_hint: Some("src/main.rs".to_string()),
        detail_lines: vec!["src/main.rs".to_string()],
        error_messages: vec![
            "tool execution failed: command `grep ...`".to_string(),
            "tool execution failed: command `rg ...`".to_string(),
        ],
        messages: Vec::new(),
        tool_use_ids: HashSet::new(),
        matched_tool_use_ids: HashSet::new(),
        tool_results: ToolResultIndex::new(),
    };

    let lines = render_collapsed_activity_group_lines(&group, false, true, true);

    assert_eq!(plain_text_line(&lines[1]), "  └ src/main.rs");
    assert_eq!(
        plain_text_line(&lines[2]),
        "    tool execution failed: command `grep ...`"
    );
    assert_eq!(
        plain_text_line(&lines[3]),
        "    tool execution failed: command `rg ...`"
    );
}

#[test]
fn tool_result_user_messages_render_as_results_not_prompts() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let message = TranscriptMessage::from_blocks(
        MessageRole::User,
        vec![TranscriptBlock::ToolResult {
            tool_use_id: "tool-1".to_string(),
            content: "completed".to_string().into(),
            is_error: false,
            metadata: None,
        }],
    );

    let rendered = render_message_lines(&message, &cwd, false, None, 80, "qwen3.6-plus", true);
    let first_line = rendered[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(!first_line.starts_with("❯ "));
    assert!(first_line.contains("completed"));
}

#[test]
fn collapsed_activity_group_stays_active_when_some_tool_uses_are_still_unresolved() {
    let mut tool_use_ids = HashSet::new();
    tool_use_ids.insert("tool-read-1".to_string());
    tool_use_ids.insert("tool-read-2".to_string());

    let mut matched_tool_use_ids = HashSet::new();
    matched_tool_use_ids.insert("tool-read-1".to_string());

    let group = CollapsedActivityGroup {
        search_count: 0,
        read_paths: vec![
            "orbcode/cli/src/lib.rs".to_string(),
            "orbcode/cli/src/main.rs".to_string(),
        ],
        read_operation_count: 0,
        read_tool_use_ids: HashSet::new(),
        failed_read_tool_use_ids: HashSet::new(),
        list_count: 0,
        latest_hint: Some("orbcode/cli/src/main.rs".to_string()),
        detail_lines: vec!["orbcode/cli/src/main.rs".to_string()],
        error_messages: Vec::new(),
        messages: Vec::new(),
        tool_use_ids,
        matched_tool_use_ids,
        tool_results: ToolResultIndex::new(),
    };

    assert!(group.has_unresolved_tool_uses());
}

#[test]
fn mixed_assistant_text_and_tool_use_preserves_text_and_result_card() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::Text {
                    text: "I will update the file.".to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "Write".to_string(),
                    input: r#"{"file_path":"src/main.rs"}"#.to_string(),
                },
            ],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: String::new().into(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let cells = render_committed_transcript_cells(&messages, &cwd, true, 120, "model");
    let rendered = plain_text_lines(&flatten_transcript_cells(&cells)).join("\n");

    assert!(rendered.contains("I will update the file."), "{rendered}");
    assert!(rendered.contains("Write"), "{rendered}");
    assert!(rendered.contains("Done"), "{rendered}");
    assert!(!rendered.contains("tool_use"), "{rendered}");
}

#[test]
fn assistant_text_after_tool_use_does_not_orphan_result_card() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::Text {
                    text: "Starting the edit.".to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "Write".to_string(),
                    input: r#"{"file_path":"src/main.rs"}"#.to_string(),
                },
                TranscriptBlock::Text {
                    text: "The edit is queued.".to_string(),
                },
            ],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: String::new().into(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let cells = render_committed_transcript_cells(&messages, &cwd, true, 120, "model");
    let rendered = plain_text_lines(&flatten_transcript_cells(&cells)).join("\n");

    assert!(rendered.contains("Starting the edit."), "{rendered}");
    assert!(rendered.contains("The edit is queued."), "{rendered}");
    assert!(rendered.contains("Done"), "{rendered}");
    assert!(!rendered.contains(ORPHANED_TOOL_RESULT), "{rendered}");
}

#[test]
fn assistant_text_after_collapsible_tool_use_does_not_orphan_activity_group() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::Text {
                        text: "Searching now.".to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "tool-search".to_string(),
                        name: "Grep".to_string(),
                        input: r#"{"pattern":"impl.*Tool.*for","path":"orbcode/tools/src/lib.rs","output_mode":"content"}"#
                            .to_string(),
                    },
                    TranscriptBlock::Text {
                        text: "Search was queued.".to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-search".to_string(),
                    content: "4594:    impl ToolProgressReporter for RecordingProgressReporter {"
                        .to_string().into(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        ];

    let cells = render_committed_transcript_cells(&messages, &cwd, true, 120, "model");
    let rendered = plain_text_lines(&flatten_transcript_cells(&cells)).join("\n");

    assert!(rendered.contains("Searching now."), "{rendered}");
    assert!(rendered.contains("Search was queued."), "{rendered}");
    assert!(
        rendered.contains("Search(regex: impl.*Tool.*for, in: orbcode/tools/src/lib.rs)"),
        "{rendered}"
    );
    assert!(!rendered.contains(ORPHANED_TOOL_RESULT), "{rendered}");
}

#[test]
fn mixed_user_text_and_tool_result_preserves_text_and_result_card() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "Write".to_string(),
                input: r#"{"file_path":"src/main.rs"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![
                TranscriptBlock::Text {
                    text: "Tool finished successfully.".to_string(),
                },
                TranscriptBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: String::new().into(),
                    is_error: false,
                    metadata: None,
                },
            ],
        ),
    ];

    let cells = render_committed_transcript_cells(&messages, &cwd, true, 120, "model");
    let rendered = plain_text_lines(&flatten_transcript_cells(&cells)).join("\n");

    assert!(
        rendered.contains("Tool finished successfully."),
        "{rendered}"
    );
    assert!(rendered.contains("Write"), "{rendered}");
    assert!(rendered.contains("Done"), "{rendered}");
    assert!(!rendered.contains(ORPHANED_TOOL_RESULT), "{rendered}");
}

#[test]
fn committed_transcript_suppresses_orphan_scalar_tool_results_after_handled_tools() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-list".to_string(),
                name: "Bash".to_string(),
                input: r#"{"command":"ls -la"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-status".to_string(),
                name: "Bash".to_string(),
                input: r#"{"command":"git status --short"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-list".to_string(),
                content: "55".to_string().into(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let cells = render_committed_transcript_cells(&messages, &cwd, false, 80, "qwen3.6-plus");
    let rendered = plain_text_lines(&flatten_transcript_cells(&cells));

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Listed 1 directory (ctrl+o to expand)"))
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.trim() == "● 55" || line.trim() == "⏺ 55"),
        "orphan scalar tool results should not leak as standalone transcript rows"
    );
}

#[test]
fn local_turn_duration_notes_render_as_highlighted_system_lines() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let message = TranscriptMessage::new(
        MessageRole::System,
        format!("{LOCAL_TURN_DURATION_PREFIX}1:34000"),
    );

    let rendered = render_message_lines(&message, &cwd, false, None, 80, "qwen3.6-plus", true);
    let lines = rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(lines, vec!["✻ Thought for 34s"]);
    assert_eq!(rendered[0].spans[0].style.fg, Some(Color::White));
    assert_eq!(rendered[0].spans[2].style.fg, Some(Color::White));
    assert!(
        !rendered[0].spans[2]
            .style
            .add_modifier
            .contains(Modifier::DIM)
    );
}

#[test]
fn grouped_failed_reads_do_not_reappear_after_later_permission_queue() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![
            TranscriptBlock::ToolUse {
                id: "read-core".to_string(),
                name: "Read".to_string(),
                input: r#"{"file_path":"orbcode/core/src/lib.rs"}"#.to_string(),
            },
            TranscriptBlock::ToolUse {
                id: "read-tools".to_string(),
                name: "Read".to_string(),
                input: r#"{"file_path":"orbcode/tools/src/lib.rs"}"#.to_string(),
            },
            TranscriptBlock::ToolUse {
                id: "read-tui".to_string(),
                name: "Read".to_string(),
                input: r#"{"file_path":"orbcode/tui/src/lib.rs"}"#.to_string(),
            },
            TranscriptBlock::ToolUse {
                id: "read-mcp".to_string(),
                name: "Read".to_string(),
                input: r#"{"file_path":"orbcode/mcp/src/lib.rs"}"#.to_string(),
            },
        ],
    ));
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::User,
        vec![
            TranscriptBlock::ToolResult {
                tool_use_id: "read-core".to_string(),
                content: "mod core;".to_string().into(),
                is_error: false,
                metadata: None,
            },
            TranscriptBlock::ToolResult {
                tool_use_id: "read-tools".to_string(),
                content: "tool execution failed: too many tokens".to_string().into(),
                is_error: true,
                metadata: None,
            },
            TranscriptBlock::ToolResult {
                tool_use_id: "read-tui".to_string(),
                content: "tool execution failed: file too large".to_string().into(),
                is_error: true,
                metadata: None,
            },
            TranscriptBlock::ToolResult {
                tool_use_id: "read-mcp".to_string(),
                content: "mod mcp;".to_string().into(),
                is_error: false,
                metadata: None,
            },
        ],
    ));
    for (tool_use_id, path) in [
        ("read-tools", "orbcode/tools/src/lib.rs"),
        ("read-tui", "orbcode/tui/src/lib.rs"),
    ] {
        state.upsert_live_tool_activity(LiveToolActivity {
            request_id: None,
            tool_use_id: tool_use_id.to_string(),
            tool_name: "Read".to_string(),
            tool_input: format!(r#"{{"file_path":"{path}"}}"#),
            status_line: "Failed during execution".to_string(),
            progress_messages: Vec::new(),
            is_error: true,
        });
    }
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![
            TranscriptBlock::ToolUse {
                id: "bash-waiting".to_string(),
                name: "Bash".to_string(),
                input: r#"{"command":"cargo test -p orbcode-tui"}"#.to_string(),
            },
            TranscriptBlock::ToolUse {
                id: "bash-queued".to_string(),
                name: "Bash".to_string(),
                input: r#"{"command":"bun test"}"#.to_string(),
            },
            TranscriptBlock::ToolUse {
                id: "read-agents".to_string(),
                name: "Read".to_string(),
                input: r#"{"file_path":"orbcode/AGENTS.md"}"#.to_string(),
            },
        ],
    ));
    state.apply_stream_event(StreamEvent::PermissionRequested {
        request: PermissionRequest {
            request_id: "req-1".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "bash-waiting".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: r#"{"command":"cargo test -p orbcode-tui"}"#.to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        },
    });

    let rendered = plain_text_lines(&state.transcript_lines(120)).join("\n");

    assert!(rendered.contains("Waiting for permission"), "{rendered}");
    assert!(rendered.contains("Queued behind permission"), "{rendered}");
    assert!(
        !rendered.contains("Read(orbcode/tools/src/lib.rs)"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("Read(orbcode/tui/src/lib.rs)"),
        "{rendered}"
    );
}

#[test]
fn committed_failed_live_activity_does_not_render_after_later_thinking() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.pending_history_flush = true;
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::User,
        vec![TranscriptBlock::ToolResult {
            tool_use_id: "read-missing".to_string(),
            content: "io error: No such file or directory (os error 2)"
                .to_string()
                .into(),
            is_error: true,
            metadata: None,
        }],
    ));
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "read-missing".to_string(),
        tool_name: "Read".to_string(),
        tool_input: r#"{"file_path":"orbcode/docs/README.md"}"#.to_string(),
        status_line: "Failed during execution".to_string(),
        progress_messages: Vec::new(),
        is_error: true,
    });
    state.active_thinking = Some(ActiveThinkingState {
        text: "让我用不同的方式统计 Rust 各个模块的行数。".to_string(),
        is_streaming: true,
        completed_at: None,
    });

    let lines = plain_text_lines(&state.transcript_lines(120));
    let thinking_index = lines
        .iter()
        .position(|line| line.contains("(thinking)") || line.contains("Thinking"))
        .expect("active thinking should render");
    let tail = lines[thinking_index..].join("\n");

    assert!(tail.contains("Rust 各个模块"), "{tail}");
    assert!(!tail.contains("Read(orbcode/docs/README.md)"), "{tail}");
    assert!(!tail.contains("Failed during execution"), "{tail}");
}

#[test]
fn collapsed_live_tool_activity_shows_progress_preview_lines() {
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

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("(ctrl+o to expand)"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Checking the core flow now."))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Read(") && line.contains("README.md"))
    );
}

#[test]
fn collapsed_simultaneous_live_tools_show_preview_for_each_tool() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "first-tool".to_string(),
                name: "Agent".to_string(),
                input:
                    "{\"description\":\"Explore repo\",\"prompt\":\"check flow\",\"subagent_type\":\"Explore\"}"
                        .to_string(),
            }],
        ));
    state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "session".to_string(),
        tool_use_id: "first-tool".to_string(),
        tool_name: "Agent".to_string(),
        tool_input: String::new(),
    });
    state.apply_stream_event(StreamEvent::ToolProgress {
        session_id: "session".to_string(),
        tool_use_id: "first-tool".to_string(),
        tool_name: "Agent".to_string(),
        progress: serde_json::json!({
            "data": {
                "type": "agent_progress",
                "message": {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
                            { "type": "text", "text": "First tool progress line." }
                        ]
                    }
                }
            }
        }),
    });
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "second-tool".to_string(),
            name: "Bash".to_string(),
            input: "{\"command\":\"git status --short\"}".to_string(),
        }],
    ));
    state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "session".to_string(),
        tool_use_id: "second-tool".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: String::new(),
    });
    state.apply_stream_event(StreamEvent::ToolProgress {
        session_id: "session".to_string(),
        tool_use_id: "second-tool".to_string(),
        tool_name: "Bash".to_string(),
        progress: serde_json::json!({
            "data": {
                "type": "agent_progress",
                "message": {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
                            { "type": "text", "text": "Second tool progress line." }
                        ]
                    }
                }
            }
        }),
    });

    let collapsed = plain_text_lines(&state.transcript_lines(90)).join("\n");

    assert!(
        collapsed.contains("First tool progress line."),
        "{collapsed}"
    );
    assert!(
        collapsed.contains("Second tool progress line."),
        "{collapsed}"
    );
    assert!(collapsed.contains("Explore(Explore repo)"), "{collapsed}");
    assert!(collapsed.contains("Bash"), "{collapsed}");
}

#[test]
fn completed_tool_history_cell_can_rerender_collapsed_and_expanded_from_structured_state() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
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
                                { "type": "text", "text": "Completed progress detail." }
                            ]
                        }
                    }
                }
            }
        ]
    })
    .to_string();
    let messages = vec![
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
                    content: "completed output".to_string().into(),
                    is_error: false,
                    metadata: Some(metadata),
                }],
            ),
        ];

    let collapsed = plain_text_lines(&flatten_transcript_cells(
        &render_committed_transcript_cells(&messages, &cwd, false, 90, "subagent"),
    ))
    .join("\n");
    let expanded = plain_text_lines(&flatten_transcript_cells(
        &render_committed_transcript_cells(&messages, &cwd, true, 90, "subagent"),
    ))
    .join("\n");

    assert!(collapsed.contains("(ctrl+o to expand)"), "{collapsed}");
    assert!(
        collapsed.contains("Completed progress detail."),
        "{collapsed}"
    );
    assert!(!collapsed.contains("Prompt:"), "{collapsed}");
    assert!(
        expanded.contains("Completed progress detail."),
        "{expanded}"
    );
}

#[test]
fn orphan_tool_use_renders_as_interrupted_on_load() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "tool-missing".to_string(),
            name: "Bash".to_string(),
            input: r#"{"command":"printf hi"}"#.to_string(),
        }],
    )];

    let cells = render_committed_transcript_cells(&messages, &cwd, false, 80, "qwen3.6-plus");
    let rendered = plain_text_lines(&flatten_transcript_cells(&cells)).join("\n");

    assert!(rendered.contains("Bash(printf hi)"), "{rendered}");
    assert!(rendered.contains(ORPHANED_TOOL_RESULT), "{rendered}");
    assert!(!rendered.contains(INTERRUPTED_TOOL_RESULT), "{rendered}");
    assert!(!rendered.contains("Running…"), "{rendered}");
}

#[test]
fn activity_group_matches_results_across_post_tool_hook_context() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::ToolUse {
                    id: "tool-find".to_string(),
                    name: "Bash".to_string(),
                    input: r#"{"command":"rg TODO src"}"#.to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "tool-list".to_string(),
                    name: "Bash".to_string(),
                    input: r#"{"command":"ls -la"}"#.to_string(),
                },
            ],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-find".to_string(),
                content: "src/main.rs".to_string().into(),
                is_error: false,
                metadata: Some(r#"{"summary":"Executed `rg TODO src`."}"#.to_string()),
            }],
        ),
        TranscriptMessage::new(MessageRole::User, "PostToolUse hook context:\nfirst done"),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-list".to_string(),
                content: "total 8\n-rw-r--r-- Cargo.toml".to_string().into(),
                is_error: false,
                metadata: Some(r#"{"summary":"Executed `ls -la`."}"#.to_string()),
            }],
        ),
    ];

    let (group, next_index) = build_collapsed_activity_group(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_collapsed_activity_group_cell_lines(
        &group, true, false, true, &cwd, 100, "model", None,
    ))
    .join("\n");

    assert_eq!(next_index, messages.len());
    assert!(!group.has_unresolved_tool_uses());
    assert!(rendered.contains("Bash(ls -la)"), "{rendered}");
    assert!(!rendered.contains(INTERRUPTED_TOOL_RESULT), "{rendered}");
    assert!(!rendered.contains(ORPHANED_TOOL_RESULT), "{rendered}");
}

#[test]
fn activity_group_matches_result_after_unrelated_tool_results() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "tool-rust-lines".to_string(),
                        name: "Bash".to_string(),
                        input:
                            r#"{"command":"find orbcode -name \"*.rs\" -type f | xargs wc -l | tail -1"}"#
                                .to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "tool-ts-lines".to_string(),
                        name: "Bash".to_string(),
                        input:
                            r#"{"command":"find src -name \"*.ts*\" -type f | xargs wc -l | tail -1"}"#
                                .to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "tool-list".to_string(),
                        name: "Bash".to_string(),
                        input: r#"{"command":"ls -la src/tools/ | head -30"}"#.to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-rust-lines".to_string(),
                    content: "  88991 total".to_string().into(),
                    is_error: false,
                    metadata: Some(
                        r#"{"summary":"Executed `find orbcode -name \"*.rs\" -type f | xargs wc -l | tail -1`."}"#
                            .to_string(),
                    ),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-ts-lines".to_string(),
                    content: "  524312 total".to_string().into(),
                    is_error: false,
                    metadata: Some(
                        r#"{"summary":"Executed `find src -name \"*.ts*\" -type f | xargs wc -l | tail -1`."}"#
                            .to_string(),
                    ),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-list".to_string(),
                    content: "total 4\ndrwxr-xr-x 58 user staff 1856 May 17 12:17 ."
                        .to_string().into(),
                    is_error: false,
                    metadata: Some(
                        r#"{"summary":"Executed `ls -la src/tools/ | head -30`."}"#
                            .to_string(),
                    ),
                }],
            ),
        ];

    let cells = render_committed_transcript_cells(&messages, &cwd, true, 120, "model");
    let rendered = plain_text_lines(&flatten_transcript_cells(&cells)).join("\n");

    assert!(
        rendered.contains("Bash(ls -la src/tools/ | head -30)"),
        "{rendered}"
    );
    assert!(rendered.contains("Listed 1 directory"), "{rendered}");
    assert!(!rendered.contains(INTERRUPTED_TOOL_RESULT), "{rendered}");
    assert!(!rendered.contains(ORPHANED_TOOL_RESULT), "{rendered}");
}

#[test]
fn activity_group_matches_result_after_unrelated_tool_use() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "tool-search".to_string(),
                        name: "Grep".to_string(),
                        input: r#"{"pattern":"impl.*Tool.*for","path":"orbcode/tools/src/lib.rs","output_mode":"content","-n":true}"#
                            .to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "tool-lines".to_string(),
                        name: "Bash".to_string(),
                        input: r#"{"command":"find src -name \"*.ts\" -o -name \"*.tsx\" | xargs wc -l | tail -1"}"#
                            .to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-search".to_string(),
                    content: "4594:    impl ToolProgressReporter for RecordingProgressReporter {"
                        .to_string().into(),
                    is_error: false,
                    metadata: None,
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-lines".to_string(),
                    content: "  524312 total".to_string().into(),
                    is_error: false,
                    metadata: Some(
                        r#"{"summary":"Executed `find src -name \"*.ts\" -o -name \"*.tsx\" | xargs wc -l | tail -1`."}"#
                            .to_string(),
                    ),
                }],
            ),
        ];

    let cells = render_committed_transcript_cells(&messages, &cwd, true, 120, "model");
    let rendered = plain_text_lines(&flatten_transcript_cells(&cells)).join("\n");

    assert!(
        rendered.contains("Search(regex: impl.*Tool.*for, in: orbcode/tools/src/lib.rs)"),
        "{rendered}"
    );
    assert!(rendered.contains("Bash(find src"), "{rendered}");
    assert!(!rendered.contains(INTERRUPTED_TOOL_RESULT), "{rendered}");
    assert!(!rendered.contains(ORPHANED_TOOL_RESULT), "{rendered}");
}

#[test]
fn activity_group_matches_all_results_after_unrelated_tool_use() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "read-large-tui".to_string(),
                        name: "Read".to_string(),
                        input: r#"{"file_path":"orbcode/tui/src/lib.rs"}"#.to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-cli".to_string(),
                        name: "Read".to_string(),
                        input: r#"{"file_path":"orbcode/cli/src/main.rs"}"#.to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-large-tools".to_string(),
                        name: "Read".to_string(),
                        input: r#"{"file_path":"orbcode/tools/src/lib.rs"}"#.to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "count-ts".to_string(),
                        name: "Bash".to_string(),
                        input: r#"{"command":"find src -name \"*.ts*\" -type f | wc -l"}"#
                            .to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "read-large-tui".to_string(),
                    content: "tool execution failed: File content (1699496 bytes) exceeds maximum allowed size (262144 bytes). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file.".to_string().into(),
                    is_error: true,
                    metadata: None,
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "read-cli".to_string(),
                    content: "use std::io::{self, Write};\nuse anyhow::Result;".to_string().into(),
                    is_error: false,
                    metadata: None,
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "read-large-tools".to_string(),
                    content: "tool execution failed: File content (55926 tokens) exceeds maximum allowed tokens (25000). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file.".to_string().into(),
                    is_error: true,
                    metadata: None,
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "count-ts".to_string(),
                    content: "2799".to_string().into(),
                    is_error: false,
                    metadata: Some(
                        r#"{"summary":"Executed `find src -name \"*.ts*\" -type f | wc -l`."}"#
                            .to_string(),
                    ),
                }],
            ),
        ];

    let cells = render_committed_transcript_cells(&messages, &cwd, true, 120, "model");
    let rendered = plain_text_lines(&flatten_transcript_cells(&cells)).join("\n");

    assert!(
        rendered.contains("Read(orbcode/cli/src/main.rs)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Read 1 file, 2 files failed"),
        "{rendered}"
    );
    assert!(rendered.contains("Bash(find src"), "{rendered}");
    assert!(!rendered.contains(INTERRUPTED_TOOL_RESULT), "{rendered}");
    assert!(!rendered.contains(ORPHANED_TOOL_RESULT), "{rendered}");
}

#[test]
fn expanded_detail_rendering_reveals_absorbed_group_steps() {
    let mut state = transcript_research_fixture_state();
    state.refresh_transcript_ui_state();

    let detail = state
        .transcript_ui
        .cells
        .iter()
        .flat_map(|cell| {
            render_transcript_cell_lines(
                cell,
                &state.cwd,
                true,
                None,
                90,
                &state.model_display_name,
            )
        })
        .collect::<Vec<_>>();
    let transcript = plain_text_lines(&detail).join("\n");

    assert!(
        transcript.contains("Searched for 1 pattern, listed 1 directory (ctrl+o to collapse)"),
        "{transcript}"
    );
    assert!(
        transcript.contains("Search(pattern: \"**/Cargo.toml\")"),
        "{transcript}"
    );
    assert!(transcript.contains("Bash(ls -la)"), "{transcript}");
    assert!(
        !transcript.contains("Glob(\"**/Cargo.toml\")"),
        "{transcript}"
    );
    assert!(!transcript.contains("Bash($ ls -la)"), "{transcript}");
}

#[test]
fn expanded_inactive_group_hides_stale_grouped_progress_bodies() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let mut tool_use_ids = HashSet::new();
    tool_use_ids.insert("bash-1".to_string());
    tool_use_ids.insert("read-1".to_string());
    let group = CollapsedActivityGroup {
            search_count: 0,
            read_paths: vec!["Cargo.toml".to_string()],
            read_operation_count: 0,
            read_tool_use_ids: HashSet::new(),
            failed_read_tool_use_ids: HashSet::new(),
            list_count: 1,
            latest_hint: None,
            detail_lines: Vec::new(),
            error_messages: Vec::new(),
            messages: vec![TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "bash-1".to_string(),
                        name: "Bash".to_string(),
                        input: "{\"command\":\"ls -la\"}".to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-1".to_string(),
                        name: "Read".to_string(),
                        input: "{\"file_path\":\"/Users/user/github/sample-workspace-main/crates/render-fixtures/Cargo.toml\"}".to_string(),
                    },
                ],
            )],
            tool_use_ids,
            matched_tool_use_ids: HashSet::new(),
            tool_results: ToolResultIndex::new(),
        };

    let rendered = plain_text_lines(&render_collapsed_activity_group_cell_lines(
        &group, true, false, true, &cwd, 90, "model", None,
    ));
    let transcript = rendered.join("\n");

    assert!(transcript.contains("Read 1 file, listed 1 directory (ctrl+o to collapse)"));
    assert!(transcript.contains("Bash(ls -la)"), "{transcript}");
    assert!(transcript.contains("Read(Cargo.toml)"), "{transcript}");
    assert!(!transcript.contains("Running…"), "{transcript}");
    assert!(!transcript.contains("Reading…"), "{transcript}");
}

#[test]
fn expanded_historical_orphan_activity_group_renders_interrupted_tools() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let messages = vec![TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![
            TranscriptBlock::ToolUse {
                id: "tool-list".to_string(),
                name: "Bash".to_string(),
                input: r#"{"command":"ls -la"}"#.to_string(),
            },
            TranscriptBlock::ToolUse {
                id: "tool-glob".to_string(),
                name: "Glob".to_string(),
                input: r#"{"pattern":"**/Cargo.toml"}"#.to_string(),
            },
        ],
    )];

    let cells = render_committed_transcript_cells(&messages, &cwd, true, 90, "qwen3.6-plus");
    let rendered = plain_text_lines(&flatten_transcript_cells(&cells)).join("\n");

    assert!(
        rendered.contains("Searched for 1 pattern, listed 1 directory"),
        "{rendered}"
    );
    assert_eq!(
        rendered.matches(ORPHANED_TOOL_RESULT).count(),
        2,
        "{rendered}"
    );
    assert!(!rendered.contains(INTERRUPTED_TOOL_RESULT), "{rendered}");
    assert!(!rendered.contains("Running…"), "{rendered}");
    assert!(!rendered.contains("Reading…"), "{rendered}");
}

#[test]
fn expanded_group_inserts_blank_lines_between_summary_and_nested_items() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let mut tool_use_ids = HashSet::new();
    tool_use_ids.insert("bash-1".to_string());
    tool_use_ids.insert("read-1".to_string());
    let matched_tool_use_ids = tool_use_ids.clone();
    let group = CollapsedActivityGroup {
            search_count: 0,
            read_paths: vec!["Cargo.toml".to_string()],
            read_operation_count: 0,
            read_tool_use_ids: HashSet::new(),
            failed_read_tool_use_ids: HashSet::new(),
            list_count: 1,
            latest_hint: None,
            detail_lines: Vec::new(),
            error_messages: Vec::new(),
            messages: vec![TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "bash-1".to_string(),
                        name: "Bash".to_string(),
                        input: "{\"command\":\"ls -la\"}".to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "read-1".to_string(),
                        name: "Read".to_string(),
                        input: "{\"file_path\":\"/Users/user/github/sample-workspace-main/crates/render-fixtures/Cargo.toml\"}".to_string(),
                    },
                ],
            )],
            tool_use_ids,
            matched_tool_use_ids,
            tool_results: ToolResultIndex::new(),
        };

    let rendered = plain_text_lines(&render_collapsed_activity_group_cell_lines(
        &group, true, false, true, &cwd, 90, "model", None,
    ));

    assert_eq!(
        rendered[0],
        "  Read 1 file, listed 1 directory (ctrl+o to collapse)"
    );
    assert_eq!(rendered[1], "");
    assert_eq!(rendered[2], platform_tool_line("Bash(ls -la)"));
    assert_eq!(rendered[3], "");
    assert_eq!(rendered[4], platform_tool_line("Read(Cargo.toml)"));
}

#[test]
fn expanded_thinking_renders_with_heading_and_indented_body() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let message = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![
            TranscriptBlock::Thinking {
                text: "plan the reply\nkeep it short".to_string(),
                signature: None,
            },
            TranscriptBlock::Text {
                text: "Hello!".to_string(),
            },
        ],
    );
    let last_thinking = (message.id.clone(), 0usize);

    let rendered = render_message_lines(
        &message,
        &cwd,
        true,
        Some(&last_thinking),
        80,
        "qwen3.6-plus",
        true,
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
    assert_eq!(lines[1], "  plan the reply");
    assert_eq!(lines[2], "  keep it short");
    assert_eq!(lines[3], "");
    assert!(lines[4].contains("qwen3.6-plus"));
    assert_eq!(lines[5], "● Hello!");
}

#[test]
fn collapsed_committed_thinking_renders_preview_line() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let message = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::Thinking {
            text: "plan the reply\nkeep it short".to_string(),
            signature: None,
        }],
    );

    let rendered = render_message_lines(&message, &cwd, false, None, 80, "qwen3.6-plus", true);
    let lines = rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(lines[0], "∴ Thinking (ctrl+o to expand)");
    assert!(lines[1].contains("keep it short"));
}

#[test]
fn expanded_transcript_only_shows_last_thinking_block_for_mixed_messages() {
    let first = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![
            TranscriptBlock::Thinking {
                text: "old plan".to_string(),
                signature: None,
            },
            TranscriptBlock::Text {
                text: "First answer".to_string(),
            },
        ],
    );
    let second = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![
            TranscriptBlock::Thinking {
                text: "new plan".to_string(),
                signature: None,
            },
            TranscriptBlock::Text {
                text: "Second answer".to_string(),
            },
        ],
    );
    let last = last_visible_thinking_block(&[first.clone(), second.clone()]).unwrap();
    assert_eq!(last, (second.id.clone(), 0));

    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let first_rendered =
        render_message_lines(&first, &cwd, true, Some(&last), 80, "qwen3.6-plus", true);
    let second_rendered =
        render_message_lines(&second, &cwd, true, Some(&last), 80, "qwen3.6-plus", true);
    let first_lines = first_rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let second_lines = second_rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(first_lines.iter().any(|line| line.contains("First answer")));
    assert!(!first_lines.iter().any(|line| line.contains("∴ Thinking")));
    assert!(second_lines.iter().any(|line| line.contains("∴ Thinking")));
    assert!(
        second_lines
            .iter()
            .any(|line| line.contains("Second answer"))
    );
}
