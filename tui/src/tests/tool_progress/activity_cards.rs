use crate::tests::support::*;

#[test]
fn live_tool_activity_titles_use_muted_style_not_bright_white() {
    let rendered = render_live_tool_activity_lines(
        &LiveToolActivity {
            request_id: None,
            tool_use_id: "tool-1".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: "{\"command\":\"ls -la\"}".to_string(),
            status_line: "Running `Bash`".to_string(),
            progress_messages: Vec::new(),
            is_error: false,
        },
        false,
        Path::new("/tmp"),
        true,
        80,
    );
    let name_span = &rendered[0].spans[2];
    let suffix_span = &rendered[0].spans[3];
    let body_span = &rendered[1].spans[1];

    assert_eq!(name_span.content.as_ref(), "Bash");
    assert_eq!(suffix_span.content.as_ref(), "(ls -la)");
    assert_ne!(name_span.style.fg, Some(Color::White));
    assert!(name_span.style.add_modifier.contains(Modifier::BOLD));
    assert!(!suffix_span.style.add_modifier.contains(Modifier::BOLD));
    assert!(body_span.style.add_modifier.contains(Modifier::DIM));
}

#[test]
fn assistant_message_completed_commits_virtual_transcript_once() {
    let mut state = normal_state("", 0);
    let answer = "亮点\n\n1. 第一行\n2. 第二行".to_string();
    state.pending_assistant = answer.clone();

    let finished = state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, answer),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    assert!(!finished);
    assert!(state.pending_assistant.is_empty());
    assert_eq!(state.messages.len(), 1);
    let viewport_text = state
        .transcript_lines(80)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(viewport_text.contains("亮点"), "{viewport_text}");
    assert!(viewport_text.contains("第二行"), "{viewport_text}");
    assert_eq!(viewport_text.matches("亮点").count(), 1, "{viewport_text}");
}

#[test]
fn assistant_message_discarded_clears_primary_stream_without_committing() {
    let mut state = normal_state("", 0);
    state.apply_stream_event(StreamEvent::AssistantDelta {
        session_id: "session".to_string(),
        delta: "before error".to_string(),
    });
    state.pending_assistant.push_str("older streamed chunk");
    state.active_thinking = Some(ActiveThinkingState {
        text: "partial thinking".to_string(),
        is_streaming: true,
        completed_at: None,
    });

    let finished = state.apply_stream_event(StreamEvent::AssistantMessageDiscarded {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_provider: ProviderId::OpenAi,
        reason: "server overloaded after content".to_string(),
    });

    assert!(!finished);
    assert!(state.messages.is_empty());
    assert!(state.pending_assistant.is_empty());
    assert!(state.active_thinking.is_none());
    assert!(state.deferred_assistant_message.is_none());
}

#[test]
fn active_turn_keeps_committed_and_live_activity_visible_after_history_flush() {
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
    state.request_in_flight = true;
    state.pending_history_flush = true;
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: r#"{"command":"pwd","description":"Print working directory"}"#.to_string(),
        status_line: "Running `pwd`".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });
    state.in_progress_tool_use_ids.insert("tool-1".to_string());

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

    assert!(history_text.contains("older committed message"));
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
    assert!(viewport_text.contains("pwd"), "{viewport_text}");
}

#[test]
fn tool_blink_visibility_matches_typescript_cadence() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;

    state.spinner_frame = 0;
    assert!(state.current_tool_blink_visible());

    state.spinner_frame = 4;
    assert!(state.current_tool_blink_visible());

    state.spinner_frame = 5;
    assert!(!state.current_tool_blink_visible());

    state.spinner_frame = 9;
    assert!(!state.current_tool_blink_visible());
}

#[test]
fn tool_activity_card_uses_agent_metadata_summary() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "agent-tool".to_string(),
                    name: "Agent".to_string(),
                    input: "{\n  \"description\": \"Explore orbcode Rust codebase\",\n  \"prompt\": \"Search broadly\",\n  \"subagent_type\": \"Explore\"\n}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "agent-tool".to_string(),
                    content: "API Error: 400 模型提供方限流".to_string(),
                    is_error: false,
                    metadata: Some("{\"status\":\"completed\",\"totalToolUseCount\":3,\"totalTokens\":0,\"totalDurationMs\":14941,\"content\":[{\"type\":\"text\",\"text\":\"API Error: 400 模型提供方限流\"}]}".to_string()),
                }],
            ),
        ];

    let (card, next_index) = build_tool_cell(&messages, 0, &cwd).unwrap();
    assert_eq!(next_index, 1);
    assert_eq!(card.title, "Explore(Explore orbcode Rust codebase)");
    assert_eq!(card.status_line, "Done (3 tool uses · 0 tokens · 15s)");
    assert!(!card.is_active);

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
    assert!(rendered[0].contains("Explore(Explore orbcode Rust codebase)"));
    assert!(rendered[1].contains("Done (3 tool uses · 0 tokens · 15s)"));
    assert_eq!(rendered[2], "(ctrl+o to expand)");
    assert_eq!(rendered.len(), 3);
}

#[test]
fn tool_activity_card_prefers_tool_summary_metadata_and_structured_details() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "task-update".to_string(),
                    name: "TaskUpdate".to_string(),
                    input: "{\n  \"task_id\": \"transcript-rendering\",\n  \"status\": \"in_progress\",\n  \"title\": \"Polish transcript rendering\"\n}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "task-update".to_string(),
                    content: "{\"id\":\"transcript-rendering\",\"status\":\"in_progress\"}".to_string(),
                    is_error: false,
                    metadata: Some("{\"status\":\"completed\",\"summary\":\"Updated task `transcript-rendering`.\",\"changedPaths\":[\"/tmp/tasks/transcript-rendering.json\"],\"content\":[{\"type\":\"text\",\"text\":\"{\\\"id\\\":\\\"transcript-rendering\\\"}\"}]}".to_string()),
                }],
            ),
        ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    assert_eq!(card.title, "TaskUpdate(transcript-rendering)");
    assert_eq!(card.status_line, "Updated task `transcript-rendering`.");
    assert!(
        card.detail_lines
            .iter()
            .any(|line| line == "Task: transcript-rendering")
    );
    assert!(
        card.detail_lines
            .iter()
            .any(|line| line == "Title: Polish transcript rendering")
    );
    assert!(
        card.detail_lines
            .iter()
            .any(|line| line == "Status: in_progress")
    );
    assert!(
        card.detail_lines
            .iter()
            .any(|line| line == "/tmp/tasks/transcript-rendering.json")
    );
}

#[test]
fn tool_activity_titles_and_details_cover_lsp_skill_and_mcp_calls() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");

    assert_eq!(
        format_tool_activity_title(
            "LSP",
            "{\"operation\":\"goToDefinition\",\"path\":\"src/main.rs\",\"symbol\":\"main\"}",
            &cwd,
        ),
        "LSP(goToDefinition)"
    );
    assert_eq!(
        format_tool_activity_title(
            "Skill",
            "{\"name\":\"rust-review\",\"arguments\":\"focus on tui\"}",
            &cwd,
        ),
        "Skill(rust-review)"
    );
    assert_eq!(
        format_tool_activity_title(
            "CallMcpTool",
            "{\"server_id\":\"docs\",\"tool_name\":\"inspect\"}",
            &cwd,
        ),
        "CallMcpTool(docs.inspect)"
    );
    assert_eq!(
        format_tool_activity_title(
            "Read",
            "{\"file_path\":\"/Users/user/github/sample-repo-tui-render/src/main.rs\",\"offset\":12,\"limit\":5}",
            &cwd,
        ),
        "Read(src/main.rs · lines 12-16)"
    );
    assert_eq!(
        format_tool_activity_title("Glob", "{\"pattern\":\"**/*.rs\",\"path\":\"src\"}", &cwd),
        "Search(pattern: \"**/*.rs\", path: \"src\")"
    );
    assert_eq!(
        format_tool_activity_title(
            "Grep",
            r#"{"pattern":"ToolSpec\\s*\\{","path":"orbcode/tools/src","output_mode":"content"}"#,
            &cwd,
        ),
        r"Search(regex: ToolSpec\s*\{, in: orbcode/tools/src)"
    );
    assert_eq!(
        format_tool_activity_title(
            "Bash",
            "{\"description\":\"Show repo structure\",\"command\":\"ls -la\"}",
            &cwd,
        ),
        "Bash(ls -la)"
    );

    let detail_lines = tool_activity_detail_lines(
        "CallMcpTool",
        "{\"server_id\":\"docs\",\"tool_name\":\"inspect\"}",
        &cwd,
    );
    assert!(detail_lines.iter().any(|line| line == "Server: docs"));
    assert!(detail_lines.iter().any(|line| line == "Tool: inspect"));

    let grep_detail_lines = tool_activity_detail_lines(
        "Grep",
        r#"{"pattern":"ToolSpec\\s*\\{","path":"orbcode/tools/src","output_mode":"content"}"#,
        &cwd,
    );
    assert!(
        grep_detail_lines
            .iter()
            .any(|line| line == r"Regex ToolSpec\s*\{")
    );
}

#[test]
fn tool_activity_card_matches_delayed_result_after_intervening_tool_message() {
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
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-2".to_string(),
                name: "Bash".to_string(),
                input: r#"{"command":"git status --short"}"#.to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: String::new(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (card, next_index) = build_tool_cell(&messages, 0, &cwd).unwrap();

    assert_eq!(next_index, 1);
    assert!(!card.is_active);
    assert_eq!(card.status_line, "Done");
}

#[test]
fn tool_activity_card_matches_result_after_hook_context() {
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
        TranscriptMessage::new(
            MessageRole::User,
            "PostToolUse hook context:\nwrite allowed",
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: String::new(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let (card, next_index) = build_tool_cell(&messages, 0, &cwd).unwrap();

    assert_eq!(next_index, 1);
    assert!(!card.is_active);
    assert_eq!(card.status_line, "Done");
}

#[test]
fn expanded_agent_card_renders_embedded_progress_transcript() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-alt");
    let metadata = serde_json::json!({
            "status": "completed",
            "totalToolUseCount": 2,
            "totalTokens": 18,
            "totalDurationMs": 9200,
            "content": [
                { "type": "text", "text": "## Summary\nThe CLI flow has two entry points." }
            ],
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
                                        "id": "file-read-1",
                                        "name": "Read",
                                        "input": { "file_path": "/Users/user/github/sample-workspace-alt/crates/render-fixtures/core/src/context.rs" }
                                    }
                                ]
                            }
                        }
                    }
                },
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
                            "type": "system",
                            "content": "API Error: 400 模型提供方限流"
                        }
                    }
                },
                {
                    "data": {
                        "type": "agent_progress",
                        "message": {
                            "type": "user",
                            "toolUseResult": {
                                "kind": "file_read",
                                "lines": 1204
                            },
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
                    input: "{\n  \"description\": \"Explore repo\",\n  \"prompt\": \"Check the CLI flow\\nThen summarize the gaps\",\n  \"subagent_type\": \"Explore\"\n}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "agent-tool".to_string(),
                    content: "## Summary\nThe CLI flow has two entry points.".to_string(),
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

    assert!(rendered.iter().any(|line| line.contains("Prompt:")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Read(crates/render-fixtures/core/src/context.rs)"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("API Error: 400 模型提供方限流"))
    );
    assert!(rendered.iter().any(|line| line.contains("Response:")));
    assert!(rendered.iter().any(|line| line.contains("Read 1204 lines")));
}

#[test]
fn permission_denied_card_renders_hook_progress_status() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let metadata = serde_json::json!({
        "progressMessages": [
            {
                "data": {
                    "type": "hook_progress",
                    "hookEventName": "PermissionDenied",
                    "status": "PermissionDenied hook completed in 3 ms",
                    "result": "completed",
                    "durationMs": 3
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
                input: "{\"command\":\"echo denied\"}".to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "bash-tool".to_string(),
                content: "permission denied for tool `Bash` by configured deny rule".to_string(),
                is_error: true,
                metadata: Some(metadata),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let collapsed = render_tool_cell_lines(&card, false, None, 90, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let expanded = render_tool_cell_lines(&card, true, None, 90, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("PermissionDenied hook completed in 3 ms")),
        "{collapsed:#?}"
    );
    assert!(
        expanded
            .iter()
            .any(|line| line.contains("PermissionDenied hook completed in 3 ms")),
        "{expanded:#?}"
    );
}

#[test]
fn permission_denied_card_renders_hook_progress_error_detail() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let metadata = serde_json::json!({
        "progressMessages": [
            {
                "data": {
                    "type": "hook_progress",
                    "hookEventName": "PermissionDenied",
                    "status": "PermissionDenied hook failed in 3 ms",
                    "result": "failed",
                    "durationMs": 3,
                    "error": "PermissionDenied hookSpecificOutput.retry must be a boolean"
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
                input: "{\"command\":\"echo denied\"}".to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "bash-tool".to_string(),
                content: "permission denied for tool `Bash` by configured deny rule".to_string(),
                is_error: true,
                metadata: Some(metadata),
            }],
        ),
    ];

    let (card, _) = build_tool_cell(&messages, 0, &cwd).unwrap();
    let collapsed = render_tool_cell_lines(&card, false, None, 120, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let expanded = render_tool_cell_lines(&card, true, None, 120, &cwd)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("retry must be a bool")),
        "{collapsed:#?}"
    );
    assert!(
        expanded
            .iter()
            .any(|line| line.contains("retry must be a boolean")),
        "{expanded:#?}"
    );
}

#[test]
fn tool_result_summary_normalizes_input_validation_errors() {
    assert_eq!(
        format_tool_result_summary("InputValidationError: missing required field", true),
        "Invalid tool parameters"
    );
    assert_eq!(
        format_tool_result_summary("invalid tool input: expected object", true),
        "Invalid tool parameters"
    );
}
