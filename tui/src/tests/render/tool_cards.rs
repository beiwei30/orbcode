use crate::tests::support::*;

#[test]
fn completed_bash_card_previews_stdout_lines_instead_of_metadata_summary() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "bash-1".to_string(),
                name: "Bash".to_string(),
                input: "{\"description\":\"pwd && ls -la\",\"command\":\"pwd && ls -la\"}"
                    .to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "bash-1".to_string(),
                content: "/Users/user/github/sample-workspace-main/crates/render-fixtures\n\
total 60\n\
drwxr-xr-x 16 user staff 512 Apr 22 14:40 ."
                    .to_string(),
                is_error: false,
                metadata: Some(
                    "{\"status\":\"completed\",\"summary\":\"Listed orbcode workspace files.\"}"
                        .to_string(),
                ),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = render_tool_cell_lines(&card, false, None, 90, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(rendered[0].contains("Bash(pwd && ls -la)"));
    assert_eq!(
        rendered[1],
        "  └ /Users/user/github/sample-workspace-main/crates/render-fixtures"
    );
    assert_eq!(rendered[2], "    total 60");
    assert_eq!(
        rendered[3],
        "    drwxr-xr-x 16 user staff 512 Apr 22 14:40 ."
    );
    assert!(
        !rendered.iter().any(|line| line == "(ctrl+o to expand)"),
        "{rendered:#?}"
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("Listed orbcode workspace files.")),
        "{rendered:#?}"
    );
}

#[test]
fn completed_bash_card_renders_cwd_reset_warning_as_dim_follow_up_line() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "bash-1".to_string(),
                name: "Bash".to_string(),
                input: "{\"command\":\"cd .. && pwd\"}".to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "bash-1".to_string(),
                content: "/Users/user/github/sample-workspace-main\n\
Shell cwd was reset to /Users/user/github/sample-workspace-main/crates/render-fixtures"
                    .to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let lines = render_tool_cell_lines(&card, false, None, 90, &cwd);
    let rendered = plain_text_lines(&lines);

    assert_eq!(
        rendered[2],
        "    Shell cwd was reset to /Users/user/github/sample-workspace-main/crates/render-fixtu…"
    );
    assert_eq!(
        lines[2].spans[1].style,
        subtle_style().add_modifier(Modifier::DIM)
    );
}

#[test]
fn completed_bash_card_uses_inline_expand_hint_for_truncated_output() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "bash-1".to_string(),
                    name: "Bash".to_string(),
                    input: "{\"description\":\"print lines\",\"command\":\"printf 'a\\\\nb\\\\nc\\\\nd\\\\ne\\\\n'\"}"
                        .to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "bash-1".to_string(),
                    content: "a\nb\nc\nd\ne".to_string(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = render_tool_cell_lines(&card, false, None, 90, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "⏺ Bash(printf 'a\\nb\\nc\\nd\\ne\\n')");
    assert_eq!(rendered[1], "  └ a");
    assert_eq!(rendered[2], "    b");
    assert_eq!(rendered[3], "    c");
    assert_eq!(rendered[4], "    … +2 lines (ctrl+o to expand)");
    assert!(
        !rendered.iter().any(|line| line == "(ctrl+o to expand)"),
        "{rendered:#?}"
    );
}

#[test]
fn expanded_completed_bash_card_omits_progress_and_uses_connector_first_only() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let metadata = serde_json::json!({
            "progressMessages": [
                { "data": { "type": "hook_progress", "status": "PreToolUse hook completed in 367 ms" } },
                { "data": { "type": "bash_progress", "status": "Running bash command" } },
                { "data": { "type": "bash_progress", "status": "Streaming stdout" } },
                { "data": { "type": "bash_progress", "status": "Bash command completed" } }
            ]
        })
        .to_string();
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "bash-1".to_string(),
                    name: "Bash".to_string(),
                    input: "{\"command\":\"printf paths\"}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "bash-1".to_string(),
                    content: "src/main.tsx\nsrc/tools/TaskGetTool/TaskGetTool.ts\nsrc/tools/TaskGetTool/prompt.ts".to_string(),
                    is_error: false,
                    metadata: Some(metadata),
                }],
            ),
        ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, true, None, 90, &cwd));

    assert_eq!(rendered[0], "⏺ Bash(printf paths)");
    assert_eq!(rendered[1], "  └ src/main.tsx");
    assert_eq!(rendered[2], "    src/tools/TaskGetTool/TaskGetTool.ts");
    assert_eq!(rendered[3], "    src/tools/TaskGetTool/prompt.ts");
    assert_eq!(rendered.len(), 4);
}

#[test]
fn expanded_bash_card_renders_runtime_metadata() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let metadata = serde_json::json!({
        "status": "completed",
        "summary": "Executed `printf ok`.",
        "bash": {
            "command": "printf ok",
            "cwd": cwd.display().to_string(),
            "timeoutMs": 5000,
            "durationMs": 42,
            "exitCode": 0,
            "signal": null,
            "interrupted": false,
            "timedOut": false,
            "outputTruncated": true,
            "omittedChars": 7
        }
    })
    .to_string();
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "bash-1".to_string(),
                name: "Bash".to_string(),
                input: "{\"command\":\"printf ok\"}".to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "bash-1".to_string(),
                content: "ok".to_string(),
                is_error: false,
                metadata: Some(metadata),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, true, None, 90, &cwd));

    assert_eq!(rendered[0], "⏺ Bash(printf ok)");
    assert_eq!(rendered[1], "  └ ok");
    assert!(!rendered.iter().any(|line| line.contains("Duration:")));
    assert!(!rendered.iter().any(|line| line.contains("Timeout:")));
    assert!(!rendered.iter().any(|line| line.contains("Exit code:")));
}

#[test]
fn expanded_bash_card_renders_git_workspace_impact() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let metadata = serde_json::json!({
        "status": "completed",
        "summary": "Executed `git checkout -b feature-x && touch new.rs`.",
        "bash": {
            "command": "git checkout -b feature-x && touch new.rs",
            "cwd": cwd.display().to_string(),
            "timeoutMs": 120000,
            "durationMs": 18,
            "exitCode": 0,
            "interrupted": false,
            "timedOut": false,
            "outputTruncated": false,
            "omittedChars": 0,
            "workspaceImpact": {
                "git": {
                    "preBranch": "main",
                    "postBranch": "feature-x",
                    "branchChanged": true,
                    "preHead": "abcdef0123456789abcdef0123456789abcdef01",
                    "postHead": "1234567abcdef89abcdef0123456789abcdef0123",
                    "headChanged": true,
                    "preDirtyFiles": 0,
                    "postDirtyFiles": 1,
                    "dirtyDelta": 1,
                    "workingTreeChanged": true
                }
            }
        }
    })
    .to_string();
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "bash-1".to_string(),
                name: "Bash".to_string(),
                input: "{\"command\":\"git checkout -b feature-x && touch new.rs\"}".to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "bash-1".to_string(),
                content: "Switched to a new branch 'feature-x'".to_string(),
                is_error: false,
                metadata: Some(metadata),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, true, None, 100, &cwd));

    assert_eq!(
        rendered[0],
        "⏺ Bash(git checkout -b feature-x && touch new.rs)"
    );
    assert_eq!(rendered[1], "  └ Switched to a new branch 'feature-x'");
    assert!(
        !rendered.iter().any(|line| line.contains("Branch:")),
        "metadata should be omitted from expanded bash: {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|line| line.contains("Commit:")),
        "metadata should be omitted from expanded bash: {rendered:?}"
    );
}

#[test]
fn completed_large_bash_card_keeps_truncation_diagnostic_and_bounded_output() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let bash_truncation_note = "[Bash output truncated for transcript safety. Re-run with a narrower command if you need the omitted portion. Omitted 30139 characters.]";
    let mut content = "line\n".repeat(5900);
    content.push('\n');
    content.push_str(bash_truncation_note);
    let streaming_progress = serde_json::json!({
        "data": {
            "type": "bash_progress",
            "status": "Streaming stdout"
        }
    });
    let metadata = serde_json::json!({
        "status": "completed",
        "summary": "Executed `python3 -c 'print(\"line\\n\" * 12000)'`.",
        "progressMessages": [
            {
                "data": {
                    "type": "bash_progress",
                    "status": "Running bash command"
                }
            },
            streaming_progress.clone(),
            streaming_progress,
            {
                "data": {
                    "type": "bash_progress",
                    "status": "Bash command completed"
                }
            }
        ],
        "bash": {
            "command": "python3 -c 'print(\"line\\n\" * 12000)'",
            "cwd": cwd.display().to_string(),
            "timeoutMs": 120000,
            "durationMs": 65,
            "exitCode": 0,
            "interrupted": false,
            "timedOut": false,
            "outputTruncated": true,
            "omittedChars": 30139
        }
    })
    .to_string();
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "bash-large".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({
                    "command": "python3 -c 'print(\"line\\n\" * 12000)'"
                })
                .to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "bash-large".to_string(),
                content,
                is_error: false,
                metadata: Some(metadata),
            }],
        ),
    ];

    let collapsed = plain_text_lines(&flatten_transcript_cells(
        &render_committed_transcript_cells(&messages, &cwd, false, 120, "subagent"),
    ))
    .join("\n");
    let expanded = plain_text_lines(&flatten_transcript_cells(
        &render_committed_transcript_cells(&messages, &cwd, true, 120, "subagent"),
    ))
    .join("\n");
    let rendered_output_line_count = |text: &str| {
        text.lines()
            .filter(|line| {
                let t = line.trim();
                matches!(t, "│ line" | "└ line" | "line")
            })
            .count()
    };

    assert!(
        collapsed.contains("Bash output truncated for transcript safety"),
        "{collapsed}"
    );
    assert!(collapsed.contains("(ctrl+o to expand)"), "{collapsed}");
    assert!(rendered_output_line_count(&collapsed) <= 3, "{collapsed}");
    assert!(
        expanded.contains("Bash output truncated for transcript safety"),
        "{expanded}"
    );
    assert!(
        expanded.contains("omitted from expanded Bash preview"),
        "{expanded}"
    );
    assert!(
        rendered_output_line_count(&expanded) <= BASH_EXPANDED_OUTPUT_DETAIL_LIMIT + 1,
        "{expanded}"
    );
    assert!(
        !expanded.contains("Running bash command..."),
        "progress lines should be omitted: {expanded}"
    );
    assert!(
        !expanded.contains("Bash command completed..."),
        "progress lines should be omitted: {expanded}"
    );
    assert!(
        !expanded.contains("Output truncated: omitted"),
        "metadata lines should be omitted: {expanded}"
    );
}

#[test]
fn failed_bash_card_keeps_expand_hint_on_last_tree_child() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "bash-err-1".to_string(),
                    name: "Bash".to_string(),
                    input: "{\"command\":\"bad command\"}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "bash-err-1".to_string(),
                    content: "tool execution failed: command `bad command` exited with 2\nBash command failed...".to_string(),
                    is_error: true,
                    metadata: None,
                }],
            ),
        ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = render_tool_cell_lines(&card, false, None, 90, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "⏺ Bash(bad command)");
    assert_eq!(
        rendered[1],
        "  │ tool execution failed: command `bad command` exited with 2"
    );
    assert_eq!(rendered[2], "  └ Bash command failed... (ctrl+o to expand)");
    assert!(
        !rendered.iter().any(|line| line == "(ctrl+o to expand)"),
        "{rendered:#?}"
    );
}

#[test]
fn failed_bash_card_dedupes_single_line_permission_denial() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "bash-denied-1".to_string(),
                    name: "Bash".to_string(),
                    input: "{\"command\":\"echo hello\"}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "bash-denied-1".to_string(),
                    content: "permission denied for tool `Bash` by PreToolUse hook: PreToolUse hook failed with exit status: 1: hook crashed from local settings".to_string(),
                    is_error: true,
                    metadata: None,
                }],
            ),
        ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = render_tool_cell_lines(&card, false, None, 84, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "⏺ Bash(echo hello)");
    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains("permission denied for tool"))
            .count(),
        1,
        "{rendered:#?}"
    );
    assert!(rendered[1].contains("(ctrl+o to expand)"), "{rendered:#?}");
    assert!(display_width_str(&rendered[1]) <= 84, "{rendered:#?}");
    assert!(
        !rendered.iter().any(|line| line == "(ctrl+o to expand)"),
        "{rendered:#?}"
    );
}

#[test]
fn failed_bash_card_truncates_all_collapsed_body_lines_to_width() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "bash-wide-err-1".to_string(),
                    name: "Bash".to_string(),
                    input: "{\"command\":\"x\"}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "bash-wide-err-1".to_string(),
                    content: "first line is intentionally much too long for this narrow transcript width\nsecond line is also intentionally much too long and carries the expand hint".to_string(),
                    is_error: true,
                    metadata: None,
                }],
            ),
        ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = render_tool_cell_lines(&card, false, None, 48, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    for line in rendered.iter().skip(1) {
        assert!(display_width_str(line) <= 48, "{rendered:#?}");
    }
    assert!(rendered[2].contains("(ctrl+o to expand)"), "{rendered:#?}");
    assert!(
        rendered[1].contains('…') && rendered[2].contains('…'),
        "{rendered:#?}"
    );
}

#[test]
fn expanded_active_bash_card_uses_running_state_without_detail_fields() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let rendered = render_live_tool_activity_lines(
        &LiveToolActivity {
            request_id: None,
            tool_use_id: "bash-1".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: "{\"description\":\"查看 orbcode 根目录结构\",\"command\":\"ls -la\"}"
                .to_string(),
            status_line: "Running".to_string(),
            progress_messages: Vec::new(),
            is_error: false,
        },
        true,
        &cwd,
        true,
        90,
    )
    .into_iter()
    .map(|line| {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    })
    .collect::<Vec<_>>();

    assert_eq!(rendered[0], "⏺ Bash(ls -la)");
    assert!(rendered[1].contains("Running"), "{rendered:#?}");
    assert!(
        !rendered.iter().any(|line| line.contains("Description:")),
        "{rendered:#?}"
    );
    assert!(
        !rendered.iter().any(|line| line.contains("$ ls -la")),
        "{rendered:#?}"
    );
}

#[test]
fn completed_read_card_uses_line_count_without_duplicate_path_details() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "read-1".to_string(),
                    name: "Read".to_string(),
                    input: "{\"file_path\":\"/Users/user/github/sample-workspace-main/crates/render-fixtures/README.md\"}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "read-1".to_string(),
                    content: "line 1\nline 2\nline 3".to_string(),
                    is_error: false,
                    metadata: Some("{\"summary\":\"Read /Users/user/github/sample-workspace-main/crates/render-fixtures/README.md.\"}".to_string()),
                }],
            ),
        ];

    let (tool_cells, _) =
        build_collapsible_tool_cells_from_message(&messages, 0, &cwd, false).unwrap();
    let rendered = render_tool_cell_lines(&tool_cells[0], true, None, 90, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "⏺ Read(README.md)");
    assert_eq!(rendered[1], "  └ Read 3 lines");
    assert!(
        !rendered.iter().any(|line| line.contains("Read /Users/")),
        "{rendered:#?}"
    );
    assert!(
        !rendered.iter().any(|line| line == "    README.md"),
        "{rendered:#?}"
    );
}

#[test]
fn completed_read_card_uses_metadata_for_empty_output_line_count() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let metadata = serde_json::json!({
        "summary": "Read /tmp/orbcode-line-offset-ui/no-final-newline.txt.",
        "content": [
            {
                "type": "text",
                "text": ""
            }
        ]
    })
    .to_string();
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "read-empty".to_string(),
                name: "Read".to_string(),
                input: r#"{"file_path":"/tmp/orbcode-line-offset-ui/no-final-newline.txt","start_line":3,"end_line":2}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "read-empty".to_string(),
                content: "Read /tmp/orbcode-line-offset-ui/no-final-newline.txt.".to_string(),
                is_error: false,
                metadata: Some(metadata),
            }],
        ),
    ];

    let (tool_cells, _) =
        build_collapsible_tool_cells_from_message(&messages, 0, &cwd, false).unwrap();
    let rendered = render_tool_cell_lines(&tool_cells[0], true, None, 90, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        rendered[0],
        "⏺ Read(/tmp/orbcode-line-offset-ui/no-final-newline.txt)"
    );
    assert_eq!(rendered[1], "  └ Read 0 lines");
}

#[test]
fn expanded_completed_read_card_renders_progress_in_body_tree() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let metadata = serde_json::json!({
        "progressMessages": [
            {
                "data": {
                    "type": "hook_progress",
                    "status": "PreToolUse hook completed in 112 ms"
                }
            }
        ]
    })
    .to_string();
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "read-1".to_string(),
                    name: "Read".to_string(),
                    input: "{\"file_path\":\"/Users/user/github/sample-workspace-main/crates/render-fixtures/mcp/src/lib.rs\"}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "read-1".to_string(),
                    content: "line 1\nline 2\nline 3".to_string(),
                    is_error: false,
                    metadata: Some(metadata),
                }],
            ),
        ];

    let (tool_cells, _) =
        build_collapsible_tool_cells_from_message(&messages, 0, &cwd, false).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(
        &tool_cells[0],
        true,
        None,
        90,
        &cwd,
    ));

    assert_eq!(rendered[0], "⏺ Read(mcp/src/lib.rs)");
    assert_eq!(rendered[1], "  └ Read 3 lines");
    assert_eq!(rendered[2], "    PreToolUse hook completed in 112 ms...");
    assert_eq!(rendered.len(), 3, "{rendered:#?}");
}

#[test]
fn expanded_failed_read_card_renders_details_and_progress_in_body_tree() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let failure = "tool execution failed: File content (1702182 bytes) exceeds maximum allowed size (262144 bytes). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file.";
    let metadata = serde_json::json!({
        "progressMessages": [
            {
                "data": {
                    "type": "hook_progress",
                    "status": "PreToolUse hook completed in 110 ms"
                }
            }
        ]
    })
    .to_string();
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "read-large".to_string(),
                name: "Read".to_string(),
                input: r#"{"file_path":"orbcode/tui/src/lib.rs"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "read-large".to_string(),
                content: format!("{failure}\norbcode/tui/src/lib.rs\n{failure}"),
                is_error: true,
                metadata: Some(metadata),
            }],
        ),
    ];

    let (tool_cells, _) =
        build_collapsible_tool_cells_from_message(&messages, 0, &cwd, false).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(
        &tool_cells[0],
        true,
        None,
        120,
        &cwd,
    ));

    assert_eq!(rendered[0], "⏺ Read(orbcode/tui/src/lib.rs)");
    assert!(
        rendered[1].starts_with("  │ tool execution failed: File content"),
        "{rendered:#?}"
    );
    assert_eq!(rendered[2], "  │ orbcode/tui/src/lib.rs");
    assert_eq!(rendered[3], "  └ PreToolUse hook completed in 110 ms...");
    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains("tool execution failed"))
            .count(),
        1,
        "{rendered:#?}"
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.starts_with("    ") || line.starts_with("      ")),
        "{rendered:#?}"
    );
}

#[test]
fn active_read_card_uses_reading_state_without_path_detail_fields() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let messages = vec![TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "read-1".to_string(),
                name: "Read".to_string(),
                input: "{\"file_path\":\"/Users/user/github/sample-workspace-main/crates/render-fixtures/tui/src/lib.rs\"}".to_string(),
            }],
        )];

    let (tool_cells, _) =
        build_collapsible_tool_cells_from_message(&messages, 0, &cwd, false).unwrap();
    let rendered = render_tool_cell_lines(&tool_cells[0], true, None, 90, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "⏺ Read(tui/src/lib.rs)");
    assert_eq!(rendered[1], "  └ Reading…");
    assert_eq!(rendered.len(), 2, "{rendered:#?}");
}

#[test]
fn expanded_agent_card_hides_empty_progress_transcript_section() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let metadata = serde_json::json!({
        "status": "completed",
        "totalToolUseCount": 1,
        "totalTokens": 2,
        "totalDurationMs": 1200,
        "content": [{ "type": "text", "text": "done" }],
        "progressMessages": [
            {
                "data": {
                    "type": "bash_progress",
                    "status": "streaming stdout"
                }
            },
            {
                "data": {
                    "type": "agent_progress",
                    "message": {
                        "type": "user",
                        "message": {
                            "role": "user",
                            "content": [
                                {
                                    "type": "tool_result",
                                    "tool_use_id": "file-read-1",
                                    "content": "Read 1204 lines",
                                    "is_error": false
                                }
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
                    id: "agent-tool".to_string(),
                    name: "Agent".to_string(),
                    input: "{\n  \"description\": \"Explore repo\",\n  \"prompt\": \"Check the CLI flow\",\n  \"subagent_type\": \"Explore\"\n}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "agent-tool".to_string(),
                    content: "done".to_string(),
                    is_error: false,
                    metadata: Some(metadata),
                }],
            ),
        ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = render_tool_cell_lines(&card, true, None, 80, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(!rendered.iter().any(|line| line.contains("Transcript:")));
}

#[test]
fn expanded_non_agent_tool_card_hides_raw_tool_progress_snapshots() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let metadata = serde_json::json!({
        "status": "completed",
        "summary": "Executed `pwd`.",
        "progressMessages": [
            {
                "data": {
                    "type": "agent_progress",
                    "message": {
                        "type": "assistant",
                        "message": {
                            "role": "assistant",
                            "content": [
                                {
                                    "type": "tool_use",
                                    "id": "bash-progress-1",
                                    "name": "Bash",
                                    "input": { "command": "pwd" }
                                }
                            ]
                        }
                    }
                }
            },
            {
                "data": {
                    "type": "agent_progress",
                    "message": {
                        "type": "system",
                        "content": "Captured shell output"
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
                id: "bash-tool".to_string(),
                name: "Bash".to_string(),
                input: "{\"command\":\"pwd\"}".to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "bash-tool".to_string(),
                content: "/Users/user/github/sample-repo-tui-render".to_string(),
                is_error: false,
                metadata: Some(metadata),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = render_tool_cell_lines(&card, true, None, 90, &cwd)
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
            .any(|line| line.contains("Captured shell output"))
    );
    assert!(
        !rendered.iter().any(|line| line.contains("Bash($ pwd)")),
        "{rendered:#?}"
    );
}

#[test]
fn collapsed_completed_agent_card_hides_preview_lines_but_keeps_expand_hint() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let metadata = serde_json::json!({
        "status": "completed",
        "totalToolUseCount": 2,
        "totalTokens": 87,
        "totalDurationMs": 12_000,
        "content": [{ "type": "text", "text": "Summary line" }]
    })
    .to_string();
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "agent-tool".to_string(),
                    name: "Agent".to_string(),
                    input: "{\n  \"description\": \"Explore repo\",\n  \"prompt\": \"Check the CLI flow\",\n  \"subagent_type\": \"Explore\"\n}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "agent-tool".to_string(),
                    content: "done".to_string(),
                    is_error: false,
                    metadata: Some(metadata),
                }],
            ),
        ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = render_tool_cell_lines(&card, false, None, 80, &cwd)
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
    assert!(!rendered.iter().any(|line| line.contains("Summary line")));
}

#[test]
fn collapsed_completed_agent_card_keeps_recent_progress_steps_visible() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let metadata = serde_json::json!({
            "status": "completed",
            "totalToolUseCount": 3,
            "totalTokens": 87,
            "totalDurationMs": 12_000,
            "content": [{ "type": "text", "text": "Summary line" }],
            "progressMessages": [
                {
                    "data": {
                        "type": "agent_progress",
                        "message": {
                            "type": "assistant",
                            "message": {
                                "role": "assistant",
                                "content": [
                                    { "type": "text", "text": "Checking the core flow now." }
                                ]
                            }
                        }
                    }
                },
                {
                    "data": {
                        "type": "agent_progress",
                        "message": {
                            "type": "assistant",
                            "message": {
                                "role": "assistant",
                                "content": [
                                    {
                                        "type": "tool_use",
                                        "id": "file-read-1",
                                        "name": "Read",
                                        "input": { "file_path": "/Users/user/github/sample-repo-tui-render/README.md" }
                                    }
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
                    id: "agent-tool".to_string(),
                    name: "Agent".to_string(),
                    input: "{\n  \"description\": \"Explore repo\",\n  \"prompt\": \"Check the CLI flow\",\n  \"subagent_type\": \"Explore\"\n}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "agent-tool".to_string(),
                    content: "done".to_string(),
                    is_error: false,
                    metadata: Some(metadata),
                }],
            ),
        ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = render_tool_cell_lines(&card, false, None, 90, &cwd)
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
            .any(|line| line.contains("Checking the core flow now."))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Read(") && line.contains("README.md"))
    );
}

#[test]
fn assistant_text_renders_right_aligned_metadata_line() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let mut message = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::Text {
            text: "Hi there!".to_string(),
        }],
    );
    message.created_at = Utc.with_ymd_and_hms(2026, 4, 10, 2, 40, 0).unwrap();

    let rendered = render_message_lines(&message, &cwd, false, None, 40, "qwen3.6-plus", true);
    let lines = rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with("qwen3.6-plus"));
    assert_eq!(lines[1], "● Hi there!");
}

#[test]
fn assistant_text_continuations_do_not_use_tree_glyphs() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo");
    let message = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::Text {
            text: "First line\nSecond line".to_string(),
        }],
    );

    let rendered = render_message_lines(&message, &cwd, false, None, 40, "qwen3.6-plus", true);
    let lines = rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(lines[1], "● First line");
    assert_eq!(lines[2], "  Second line");
    assert!(!lines[2].contains('└'));
}

#[test]
fn truncate_path_tail_preserves_path_suffix() {
    assert_eq!(
        truncate_path_tail("~/github/sample-repo-tui-render", 12),
        "…-tui-render"
    );
}

#[test]
fn header_info_lines_truncate_path_tail_on_narrow_width() {
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
        cwd_display: "~/github/sample-repo-tui-render".to_string(),
        model_display_name: "Qwen3.6-Plus".to_string(),
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
        .header_info_lines(20, 80)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(rendered[3], "~/github/sample-repo-tui-render");
}

#[test]
fn transcript_lines_do_not_render_assistant_metadata_rows() {
    let mut state = normal_state("", 0);
    let mut first = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::Text {
            text: "First step".to_string(),
        }],
    );
    let mut second = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::Text {
            text: "Second step".to_string(),
        }],
    );
    first.created_at = Utc.with_ymd_and_hms(2026, 4, 10, 15, 48, 0).unwrap();
    second.created_at = Utc.with_ymd_and_hms(2026, 4, 10, 15, 48, 20).unwrap();
    state.model_display_name = "Qwen3.6-Plus-DogFooding[1m]".to_string();
    state.messages = vec![first, second];
    state.history_flushed_message_count = state.stable_transcript_cells(90).len();

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

    let metadata_matches = rendered
        .iter()
        .filter(|line| line.contains("Qwen3.6-Plus-DogFooding[1m]"))
        .count();
    assert_eq!(metadata_matches, 0);
}

#[test]
fn assistant_markdown_wraps_before_right_padding() {
    let rendered = plain_text_lines(&render_text_block_lines(
        &MessageRole::Assistant,
        "The user asked for a very long response that should wrap before the edge.",
        32,
    ));

    assert!(rendered.len() > 1);
    assert!(
        rendered
            .iter()
            .all(|line| display_width_str(line) <= 32 - TRANSCRIPT_RIGHT_PADDING)
    );
}

#[test]
fn task_tool_completion_marks_panel_dirty() {
    use orbcode_app_server::ToolUseCompletionKind;

    let mut state = normal_state("", 0);
    state.task_panel.mark_dirty();
    assert!(state.task_panel.needs_refresh(Instant::now()));

    state.task_panel.apply_snapshot(
        orbcode_tools::TaskListSnapshot {
            task_list_id: "test".to_string(),
            directory: PathBuf::from("/tmp/orbcode-task-panel-tests"),
            tasks: Vec::new(),
            summary: orbcode_tools::TaskListSummary::default(),
            fingerprint: 0,
        },
        Instant::now(),
    );
    assert!(!state.task_panel.needs_refresh(Instant::now()));

    state.apply_stream_event(StreamEvent::ToolUseCompleted {
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "TaskCreate".to_string(),
        kind: ToolUseCompletionKind::Success,
    });
    assert!(
        state.task_panel.needs_refresh(Instant::now()),
        "task tool completion should mark the panel dirty"
    );
    assert!(task_tool_changes_panel("TaskCreate"));
}

#[test]
fn workflow_tool_completion_marks_background_task_panel_dirty() {
    use orbcode_app_server::ToolUseCompletionKind;

    let mut state = normal_state("", 0);
    state.background_agent_panel.set_session_id("session");
    state
        .background_agent_panel
        .apply_views_for_test(Vec::new(), Instant::now());
    assert!(!state.background_agent_panel.needs_refresh(Instant::now()));

    state.apply_stream_event(StreamEvent::ToolUseCompleted {
        session_id: "session".to_string(),
        tool_use_id: "workflow-tool-1".to_string(),
        tool_name: "Workflow".to_string(),
        kind: ToolUseCompletionKind::Success,
    });

    assert!(
        state.background_agent_panel.needs_refresh(Instant::now()),
        "Workflow tool completion should request a background task refresh"
    );
}

#[test]
fn local_task_progress_marks_background_task_panel_dirty() {
    let mut state = normal_state("", 0);
    state.background_agent_panel.set_session_id("session");
    state
        .background_agent_panel
        .apply_views_for_test(Vec::new(), Instant::now());
    assert!(!state.background_agent_panel.needs_refresh(Instant::now()));

    let finished = state.apply_stream_event(StreamEvent::LocalTaskProgress {
        session_id: "session".to_string(),
        task_id: "shell-1".to_string(),
        status: "running".to_string(),
        command: Some("sleep 1".to_string()),
        exit_code: None,
        signal: None,
    });

    assert!(
        !finished,
        "local task progress should not terminate the active turn stream"
    );
    assert!(
        state.background_agent_panel.needs_refresh(Instant::now()),
        "local task progress should request a background task refresh"
    );
}

#[test]
fn background_task_updated_renders_workflow_card_without_polling() {
    use orbcode_protocol::{
        BackgroundTaskProgressEvent, BackgroundTaskView, BackgroundTaskViewKind,
        BackgroundTaskViewStatus,
    };

    let mut state = normal_state("", 0);
    let timestamp = Utc.with_ymd_and_hms(2026, 5, 27, 0, 0, 0).unwrap();
    let task = BackgroundTaskView {
        task_id: "workflow-push-1".to_string(),
        session_id: "session".to_string(),
        kind: BackgroundTaskViewKind::Workflow,
        status: BackgroundTaskViewStatus::Running,
        description: "Generated deploy workflow".to_string(),
        cwd: "/tmp".to_string(),
        created_at: timestamp,
        updated_at: timestamp,
        started_at: Some(timestamp),
        finished_at: None,
        pid: None,
        exit_code: None,
        signal: None,
        error: None,
        model: None,
        provider: None,
        permission_mode: None,
        agent_type: None,
        child_session_id: None,
        cancellation_reason: None,
        label: Some("Generated deploy workflow".to_string()),
        log_tail: None,
        progress_events: Some(vec![BackgroundTaskProgressEvent {
            timestamp,
            event: "step_started".to_string(),
            step_key: Some("plan".to_string()),
            kind: Some("agent".to_string()),
            message: Some("Planning rollout".to_string()),
            output: None,
            child_session_id: None,
        }]),
        workflow_steps: None,
    };

    let finished = state.apply_stream_event(StreamEvent::BackgroundTaskUpdated {
        session_id: "session".to_string(),
        task,
    });

    assert!(
        !finished,
        "background task snapshots should not terminate the active turn stream"
    );
    assert!(
        !state.background_agent_panel.needs_refresh(Instant::now()),
        "pushed snapshot should update the panel state directly"
    );
    let transcript = plain_text_lines(&state.transcript_lines(100)).join("\n");
    assert!(transcript.contains("Background tasks"), "{transcript}");
    assert!(transcript.contains("workflow-push-1"), "{transcript}");
    assert!(transcript.contains("workflow"), "{transcript}");
    assert!(
        transcript.contains("plan started: Planning rollout"),
        "{transcript}"
    );
}

#[test]
fn task_list_tool_card_omits_full_output_when_collapsed() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "task-list-1".to_string(),
                name: "TaskList".to_string(),
                input: "{}".to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "task-list-1".to_string(),
                content: "#1 [pending] Design API\n#2 [pending] Implement (alice) [blocked by #1]"
                    .to_string(),
                is_error: false,
                metadata: Some(
                    "{\"status\":\"completed\",\"summary\":\"Listed 2 task(s).\"}".to_string(),
                ),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let lines = render_tool_cell_lines(&card, false, None, 80, &cwd);
    let rendered = lines
        .iter()
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
            .any(|line| line.contains("Listed 2 task(s)."))
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("[pending] Design API")),
        "collapsed task-list card should not duplicate the full output; got {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("ctrl+o to expand"))
    );
}

#[test]
fn task_list_tool_card_shows_full_output_when_expanded() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "task-list-2".to_string(),
                name: "TaskList".to_string(),
                input: "{}".to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "task-list-2".to_string(),
                content: "#1 [pending] Design API".to_string(),
                is_error: false,
                metadata: Some(
                    "{\"status\":\"completed\",\"summary\":\"Listed 1 task(s).\"}".to_string(),
                ),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let lines = render_tool_cell_lines(&card, true, None, 80, &cwd);
    let rendered = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert!(
        rendered.iter().any(|line| line.contains("Response:")),
        "expanded card should reveal a Response section: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("Design API")),
        "expanded card should expose the full task list body: {rendered:?}"
    );
}

// ── File-Edit tool card ──────────────────────────────────────────────

#[test]
fn collapsed_edit_card_shows_filename_and_change_summary() {
    let cwd = PathBuf::from("/Users/user/project");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "edit-1".to_string(),
                name: "Edit".to_string(),
                input: r#"{"file_path":"/Users/user/project/src/main.rs","old_string":"fn old() {\n    println!(\"old\");\n}","new_string":"fn new_fn() {\n    println!(\"new\");\n    eprintln!(\"debug\");\n}"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "edit-1".to_string(),
                content: "Successfully edited file.".to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, false, None, 90, &cwd));

    assert!(
        rendered[0].contains("Update(src/main.rs)"),
        "title should show relative path: {rendered:?}"
    );
    assert!(
        rendered[1].contains("Added") && rendered[1].contains("removed"),
        "collapsed status should show change summary: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("1 -fn old()")),
        "collapsed edit card should show removed preview lines: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("1 +fn new_fn()")),
        "collapsed edit card should show added preview lines: {rendered:?}"
    );
}

#[test]
fn collapsed_edit_card_uses_metadata_diff_context_and_line_numbers() {
    let cwd = PathBuf::from("/Users/user/project");
    let metadata = serde_json::json!({
        "diff": "@@ -1,5 +1,5 @@\n # orbcode Rust Refactor Plan\n \n-Last updated: 2026-06-03 (Slice 8 completed)\n+Last updated: 2026-06-04 (Slice 9 completed)\n \n ## Baseline",
        "linesAdded": 1,
        "linesRemoved": 1
    })
    .to_string();
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "edit-meta".to_string(),
                name: "file-edit".to_string(),
                input: r#"{"file_path":"/Users/user/project/plans/rust-refactor-plan.md","old_string":"Last updated: 2026-06-03 (Slice 8 completed)","new_string":"Last updated: 2026-06-04 (Slice 9 completed)"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "edit-meta".to_string(),
                content: "replaced the first occurrence".to_string(),
                is_error: false,
                metadata: Some(metadata),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, false, None, 100, &cwd));

    assert_eq!(rendered[0], "⏺ Update(plans/rust-refactor-plan.md)");
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Added 1 line, removed 1 line")),
        "collapsed status should use metadata counts: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("1  # orbcode Rust Refactor Plan")),
        "collapsed diff should include metadata context lines: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("3 -Last updated: 2026-06-03")),
        "collapsed diff should use old line numbers: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("3 +Last updated: 2026-06-04")),
        "collapsed diff should use new line numbers: {rendered:?}"
    );
}

#[test]
fn expanded_edit_card_shows_diff_preview() {
    let cwd = PathBuf::from("/Users/user/project");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "edit-1".to_string(),
                name: "Edit".to_string(),
                input: r#"{"file_path":"/Users/user/project/src/lib.rs","old_string":"old line 1\nold line 2\nold line 3","new_string":"new line A\nnew line B\nnew line C\nnew line D"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "edit-1".to_string(),
                content: "Successfully edited file.".to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, true, None, 90, &cwd));

    assert_eq!(rendered[0], "⏺ Update(src/lib.rs)");
    assert!(
        rendered.iter().any(|line| line.contains("1 -old line 1")),
        "expanded diff should show numbered removed lines: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("1 +new line A")),
        "expanded diff should show numbered added lines: {rendered:?}"
    );
}

#[test]
fn expanded_edit_card_shows_duration_from_metadata() {
    let cwd = PathBuf::from("/Users/user/project");
    let metadata = r#"{"durationMs":150}"#;
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "edit-dur".to_string(),
                name: "Edit".to_string(),
                input: r#"{"file_path":"/Users/user/project/src/lib.rs","old_string":"a","new_string":"b"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "edit-dur".to_string(),
                content: "Successfully edited file.".to_string(),
                is_error: false,
                metadata: Some(metadata.to_string()),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, true, None, 90, &cwd));

    assert!(
        !rendered.iter().any(|line| line.contains("Duration:")),
        "expanded edit card should not show duration: {rendered:?}"
    );
}

#[test]
fn expanded_edit_card_diff_lines_have_color() {
    let cwd = PathBuf::from("/Users/user/project");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "edit-color".to_string(),
                name: "Edit".to_string(),
                input: r#"{"file_path":"/Users/user/project/f.rs","old_string":"hello","new_string":"world"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "edit-color".to_string(),
                content: "Successfully edited file.".to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let lines = render_tool_cell_lines(&card, true, None, 90, &cwd);

    let removal_line = lines
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("1 -hello"))
        })
        .expect("should have a removal line");
    let removal_span = removal_line
        .spans
        .iter()
        .find(|span| span.content.contains("1 -hello"))
        .unwrap();
    assert_eq!(
        removal_span.style.fg,
        Some(ratatui::prelude::Color::Rgb(255, 107, 128)),
        "removal line should be pink: {removal_span:?}"
    );

    let addition_line = lines
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("1 +world"))
        })
        .expect("should have an addition line");
    let addition_span = addition_line
        .spans
        .iter()
        .find(|span| span.content.contains("1 +world"))
        .unwrap();
    assert_eq!(
        addition_span.style.fg,
        Some(ratatui::prelude::Color::Rgb(78, 186, 101)),
        "addition line should be green: {addition_span:?}"
    );
}

// ── Grep tool card ───────────────────────────────────────────────────

#[test]
fn collapsed_grep_card_shows_match_count() {
    let cwd = PathBuf::from("/Users/user/project");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "grep-1".to_string(),
                name: "Grep".to_string(),
                input: r#"{"pattern":"TODO","path":"src/"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "grep-1".to_string(),
                content: "src/main.rs:10:    // TODO: fix this\nsrc/main.rs:20:    // TODO: refactor\nsrc/lib.rs:5:    // TODO: add tests\nsrc/lib.rs:30:    // TODO: optimize\nsrc/utils.rs:8:    // TODO: clean up".to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (tool_cells, _) =
        build_collapsible_tool_cells_from_message(&messages, 0, &cwd, false).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(
        &tool_cells[0],
        false,
        None,
        90,
        &cwd,
    ));

    assert!(
        rendered[0].contains("Search("),
        "title should show Search: {rendered:?}"
    );
    assert!(
        rendered[1].contains("5 matches"),
        "collapsed grep should show match count: {rendered:?}"
    );
}

#[test]
fn expanded_grep_card_shows_match_previews() {
    let cwd = PathBuf::from("/Users/user/project");
    let content = "src/main.rs:10:    // TODO: fix this\n\
        src/main.rs:20:    // TODO: refactor\n\
        src/lib.rs:5:    // TODO: add tests\n\
        src/lib.rs:30:    // TODO: optimize\n\
        src/utils.rs:8:    // TODO: clean up\n\
        src/extra.rs:1:    // TODO: six\n\
        src/extra.rs:2:    // TODO: seven";
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "grep-2".to_string(),
                name: "Grep".to_string(),
                input: r#"{"pattern":"TODO","path":"src/"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "grep-2".to_string(),
                content: content.to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (tool_cells, _) =
        build_collapsible_tool_cells_from_message(&messages, 0, &cwd, false).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(
        &tool_cells[0],
        true,
        None,
        120,
        &cwd,
    ));

    assert!(
        rendered[1].contains("7 matches"),
        "expanded status should show total match count: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("src/main.rs:10")),
        "expanded grep should show first match: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("… +2 matches")),
        "expanded grep should show truncation hint: {rendered:?}"
    );
}

// ── Glob tool card ───────────────────────────────────────────────────

#[test]
fn collapsed_glob_card_shows_file_count() {
    let cwd = PathBuf::from("/Users/user/project");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "glob-1".to_string(),
                name: "Glob".to_string(),
                input: r#"{"pattern":"**/*.rs"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "glob-1".to_string(),
                content: "/Users/user/project/src/main.rs\n/Users/user/project/src/lib.rs\n/Users/user/project/src/utils.rs".to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (tool_cells, _) =
        build_collapsible_tool_cells_from_message(&messages, 0, &cwd, false).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(
        &tool_cells[0],
        false,
        None,
        90,
        &cwd,
    ));

    assert!(
        rendered[1].contains("3 files matched"),
        "collapsed glob should show file count: {rendered:?}"
    );
}

#[test]
fn expanded_glob_card_shows_file_previews_with_relative_paths() {
    let cwd = PathBuf::from("/Users/user/project");
    let content = "/Users/user/project/src/main.rs\n\
        /Users/user/project/src/lib.rs\n\
        /Users/user/project/src/utils.rs\n\
        /Users/user/project/src/config.rs\n\
        /Users/user/project/src/tests.rs\n\
        /Users/user/project/src/extra.rs\n\
        /Users/user/project/src/more.rs";
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "glob-2".to_string(),
                name: "Glob".to_string(),
                input: r#"{"pattern":"**/*.rs"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "glob-2".to_string(),
                content: content.to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (tool_cells, _) =
        build_collapsible_tool_cells_from_message(&messages, 0, &cwd, false).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(
        &tool_cells[0],
        true,
        None,
        120,
        &cwd,
    ));

    assert!(
        rendered[1].contains("7 files matched"),
        "expanded status should show total file count: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("src/main.rs")),
        "expanded glob should show relative paths: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("… +2 files")),
        "expanded glob should show truncation hint: {rendered:?}"
    );
}

// ── MCP tool card ────────────────────────────────────────────────────

#[test]
fn collapsed_mcp_card_shows_server_tool_title() {
    let cwd = PathBuf::from("/Users/user/project");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "mcp-1".to_string(),
                name: "CallMcpTool".to_string(),
                input: r#"{"server_id":"docs","tool_name":"search","query":"auth"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "mcp-1".to_string(),
                content: "Found 3 results for auth.".to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, false, None, 90, &cwd));

    assert!(
        rendered[0].contains("CallMcpTool(docs.search)"),
        "MCP card title should show server.tool: {rendered:?}"
    );
}

#[test]
fn collapsed_mcp_provider_tool_shows_server_colon_tool() {
    let cwd = PathBuf::from("/Users/user/project");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "mcp-prov-1".to_string(),
                name: "mcp__docs__search".to_string(),
                input: r#"{"query":"auth"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "mcp-prov-1".to_string(),
                content: "Found 3 results.".to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, false, None, 90, &cwd));

    assert!(
        rendered[0].contains("docs:search"),
        "MCP provider tool title should show server:tool: {rendered:?}"
    );
}

#[test]
fn expanded_mcp_card_shows_duration_and_error() {
    let cwd = PathBuf::from("/Users/user/project");
    let metadata = r#"{"durationMs":2500}"#;
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "mcp-dur".to_string(),
                name: "CallMcpTool".to_string(),
                input: r#"{"server_id":"db","tool_name":"query"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "mcp-dur".to_string(),
                content: "Query returned 42 rows.".to_string(),
                is_error: false,
                metadata: Some(metadata.to_string()),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, true, None, 90, &cwd));

    assert!(
        rendered[0].contains("CallMcpTool(db.query)"),
        "title should show server.tool: {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|line| line.contains("Duration:")),
        "expanded MCP card should not show duration: {rendered:?}"
    );
}

#[test]
fn expanded_mcp_card_shows_error_status() {
    let cwd = PathBuf::from("/Users/user/project");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "mcp-err".to_string(),
                name: "CallMcpTool".to_string(),
                input: r#"{"server_id":"db","tool_name":"query"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "mcp-err".to_string(),
                content: "Connection refused: server not running".to_string(),
                is_error: true,
                metadata: Some(r#"{"durationMs":50}"#.to_string()),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    assert!(card.is_error);
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, true, None, 90, &cwd));

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Connection refused")),
        "error MCP card should show error message: {rendered:?}"
    );
}

// ── Write tool card ──────────────────────────────────────────────────

#[test]
fn collapsed_write_card_shows_filename_and_line_count() {
    let cwd = PathBuf::from("/Users/user/project");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "write-1".to_string(),
                name: "Write".to_string(),
                input: r#"{"file_path":"/Users/user/project/new_file.rs","content":"fn main() {\n    println!(\"hello\");\n}\n"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "write-1".to_string(),
                content: "Successfully wrote to new_file.rs.".to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(&card, false, None, 90, &cwd));

    assert!(
        rendered[0].contains("Write(new_file.rs)"),
        "title should show relative path: {rendered:?}"
    );
    assert!(
        rendered[1].contains("Wrote"),
        "collapsed status should show line count: {rendered:?}"
    );
}

// ── Generic duration for non-Bash tools ──────────────────────────────

#[test]
fn expanded_grep_card_shows_duration_from_metadata() {
    let cwd = PathBuf::from("/Users/user/project");
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "grep-dur".to_string(),
                name: "Grep".to_string(),
                input: r#"{"pattern":"TODO"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "grep-dur".to_string(),
                content: "src/main.rs:10:    // TODO: fix".to_string(),
                is_error: false,
                metadata: Some(r#"{"durationMs":85}"#.to_string()),
            }],
        ),
    ];

    let (tool_cells, _) =
        build_collapsible_tool_cells_from_message(&messages, 0, &cwd, false).unwrap();
    let rendered = plain_text_lines(&render_tool_cell_lines(
        &tool_cells[0],
        true,
        None,
        90,
        &cwd,
    ));

    assert!(
        !rendered.iter().any(|line| line.contains("Duration:")),
        "expanded grep card should not show duration: {rendered:?}"
    );
}
