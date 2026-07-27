use crate::tests::support::*;

#[test]
fn tool_use_started_creates_live_activity_without_permission_prompt() {
    let mut state = normal_state("", 0);
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "glob".to_string(),
            input: "{\"pattern\":\"orbcode/**/*.rs\"}".to_string(),
        }],
    ));
    let finished = state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "glob".to_string(),
        tool_input: String::new(),
    });

    assert!(!finished);
    assert_eq!(
        state
            .latest_live_tool_activity()
            .map(|activity| activity.tool_name.as_str()),
        Some("glob")
    );
    assert_eq!(
        state
            .latest_live_tool_activity()
            .map(|activity| activity.tool_input.as_str()),
        Some("{\"pattern\":\"orbcode/**/*.rs\"}")
    );
}

#[test]
fn permission_requested_keeps_live_activity_and_shows_overlay() {
    let mut state = normal_state("", 0);

    let finished = state.apply_stream_event(StreamEvent::PermissionRequested {
        request: PermissionRequest {
            request_id: "req-1".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "tool-1".to_string(),
            tool_name: "bash".to_string(),
            tool_input: "{\"command\":\"ls\"}".to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        },
    });

    assert!(!finished);
    assert!(matches!(
        state.overlay,
        Some(OverlayState::PermissionRequest(_))
    ));
    assert_eq!(
        state
            .latest_live_tool_activity()
            .map(|activity| activity.tool_name.as_str()),
        Some("bash")
    );
}

#[test]
fn permission_blocked_later_tool_use_renders_as_queued() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![
            TranscriptBlock::ToolUse {
                id: "tool-waiting".to_string(),
                name: "Bash".to_string(),
                input: r#"{"command":"printf one"}"#.to_string(),
            },
            TranscriptBlock::ToolUse {
                id: "tool-queued".to_string(),
                name: "Bash".to_string(),
                input: r#"{"command":"printf two"}"#.to_string(),
            },
        ],
    ));
    state.apply_stream_event(StreamEvent::PermissionRequested {
        request: PermissionRequest {
            request_id: "req-1".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "tool-waiting".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: r#"{"command":"printf one"}"#.to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        },
    });

    let lines = plain_text_lines(&state.transcript_lines(120));
    let rendered = lines.join("\n");
    let queued_index = lines
        .iter()
        .position(|line| line.contains("Bash(printf two)"))
        .expect("queued tool row");
    let queued_row = lines[queued_index..lines.len().min(queued_index + 4)].join("\n");

    assert!(rendered.contains("Waiting for permission"), "{rendered}");
    assert!(
        queued_row.contains("Queued behind permission"),
        "{queued_row}"
    );
    assert!(!queued_row.contains("Running"), "{queued_row}");
}

#[test]
fn tool_use_started_keeps_live_activity_visible_until_result_arrives() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(2_000));
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            input: "{\"command\":\"ls\"}".to_string(),
        }],
    ));

    state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "bash".to_string(),
        tool_input: String::new(),
    });

    let rendered = state
        .transcript_lines(80)
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
            .any(|line| line.contains("Running bash") || line.contains("Running bash..."))
    );
}

#[test]
fn request_started_during_active_turn_preserves_live_tool_activity() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.in_progress_tool_use_ids.insert("tool-1".to_string());
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-1".to_string(),
        tool_name: "glob".to_string(),
        tool_input: "{\"pattern\":\"orbcode/**/*.rs\"}".to_string(),
        status_line: "Finished `glob`".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });

    let finished = state.apply_stream_event(StreamEvent::RequestStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_provider: None,
        context: TurnContext {
            cwd: "/tmp".to_string(),
            current_date: "2026-04-22".to_string(),
            ..Default::default()
        },
    });

    assert!(!finished);
    assert!(state.has_live_tool_activity());
    assert!(state.in_progress_tool_use_ids.contains("tool-1"));
}

#[test]
fn stale_hook_progress_is_cleared_by_regular_user_message() {
    let mut state = normal_state("", 0);

    state.apply_stream_event(StreamEvent::HookProgress {
        session_id: "session".to_string(),
        hook_event_name: "Stop".to_string(),
        progress: serde_json::json!({
            "data": {
                "type": "hook_progress",
                "hookEventName": "Stop",
                "result": "completed",
                "durationMs": 2
            }
        }),
    });
    state.apply_stream_event(StreamEvent::UserMessage {
        message: TranscriptMessage::new(MessageRole::User, "next prompt".to_string()),
    });

    assert!(state.pending_hook_progress.is_empty());
}

#[test]
fn successful_tool_completion_keeps_live_activity_visible() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.in_progress_tool_use_ids.insert("tool-1".to_string());
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-1".to_string(),
        tool_name: "glob".to_string(),
        tool_input: "{\"pattern\":\"orbcode/**/*.rs\"}".to_string(),
        status_line: "Running `glob`".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });

    let finished = state.apply_stream_event(StreamEvent::ToolUseCompleted {
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "glob".to_string(),
        kind: orbcode_protocol::ToolUseCompletionKind::Success,
    });

    assert!(!finished);
    assert_eq!(
        state
            .latest_live_tool_activity()
            .map(|activity| activity.status_line.as_str()),
        Some("Finished `glob`")
    );
}

#[test]
fn tool_result_message_commits_tool_result_without_history_flush() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "Bash".to_string(),
            input: "{\"command\":\"find . -maxdepth 2\"}".to_string(),
        }],
    ));
    state.in_progress_tool_use_ids.insert("tool-1".to_string());
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: "{\"command\":\"find . -maxdepth 2\"}".to_string(),
        status_line: "Finished `Bash`".to_string(),
        progress_messages: vec![serde_json::json!({
            "data": {
                "status": "Reading files",
                "message": {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
                            { "type": "text", "text": "Reading 2 files..." }
                        ]
                    }
                }
            }
        })],
        is_error: false,
    });

    let finished = state.apply_stream_event(StreamEvent::UserMessage {
        message: TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "file list".to_string(),
                is_error: false,
                metadata: None,
            }],
        ),
    });

    assert!(!finished);
    assert!(state.pending_history_flush);
    assert!(state.has_live_tool_activity());
    let history = state.take_history_lines(80, 20);
    let history_text = plain_text_lines(&history).join("\n");
    assert!(
        history_text.contains("Searched for 1 pattern"),
        "{history_text}"
    );
    assert!(!state.pending_history_flush);
    assert!(!state.has_live_tool_activity());

    assert!(state.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult { content, .. } if content == "file list"
            )
        })
    }));
    let committed_metadata = state
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .find_map(|block| match block {
            TranscriptBlock::ToolResult {
                tool_use_id,
                metadata,
                ..
            } if tool_use_id == "tool-1" => metadata.as_deref(),
            _ => None,
        });
    assert_eq!(tool_activity_progress_messages(committed_metadata).len(), 1);
}

#[test]
fn tool_progress_event_updates_live_activity_transcript() {
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

    let finished = state.apply_stream_event(StreamEvent::ToolProgress {
        session_id: "session".to_string(),
        tool_use_id: "agent-tool".to_string(),
        tool_name: "Agent".to_string(),
        progress: serde_json::json!({
            "data": {
                "type": "agent_progress",
                "status": "Reading 1 file",
                "message": {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
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

    assert!(!finished);
    assert_eq!(
        state
            .latest_live_tool_activity()
            .map(|activity| activity.progress_messages.len()),
        Some(1)
    );
    let footer_text = state.footer_right_text();
    assert!(
        !footer_text.contains("Reading") && !footer_text.contains("progress"),
        "footer should not show tool progress (it lives in request_status panel), got: {footer_text}"
    );
    let request_status = plain_text_lines(&state.request_status_lines());
    assert_eq!(request_status.len(), 1);
    assert!(
        request_status[0].starts_with("· Reading 1 file...(0s · ↓ "),
        "{request_status:?}"
    );
    assert!(
        !request_status[0].contains("↓ 0 tokens"),
        "{request_status:?}"
    );

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

    assert!(rendered.iter().any(|line| line.contains("Explore")));
    assert!(rendered.iter().any(|line| line.contains("Read(")));
}

#[test]
fn live_agent_progress_replaces_committed_initializing_card() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
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
                                {
                                    "type": "tool_use",
                                    "id": "file-read-1",
                                    "name": "Read",
                                    "input": { "file_path": "/Users/user/github/sample-repo/orbcode/tui/src/lib.rs" }
                                }
                            ]
                        }
                    }
                }
            }),
        });

    let rendered = plain_text_lines(&state.transcript_lines(90)).join("\n");

    assert_eq!(
        rendered
            .matches("Explore(Inspect permission panel)")
            .count(),
        1
    );
    assert!(!rendered.contains("Initializing"), "{rendered}");
    assert!(rendered.contains("Running Agent"), "{rendered}");
    assert!(rendered.contains("Read("), "{rendered}");
}

#[test]
fn live_agent_group_collapses_same_message_agents() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![
            TranscriptBlock::ToolUse {
                id: "agent-first".to_string(),
                name: "Agent".to_string(),
                input:
                    "{\"description\":\"Explore existing e2e test patterns\",\"prompt\":\"check tests\",\"subagent_type\":\"Explore\"}"
                        .to_string(),
            },
            TranscriptBlock::ToolUse {
                id: "agent-second".to_string(),
                name: "Agent".to_string(),
                input:
                    "{\"description\":\"Explore mock provider and stream events\",\"prompt\":\"check provider\",\"subagent_type\":\"Explore\"}"
                        .to_string(),
            },
        ],
    ));
    for (tool_use_id, status) in [
        ("agent-first", "Searching for 11 patterns, reading 7 files"),
        ("agent-second", "Searching for 6 patterns, reading 9 files"),
    ] {
        state.apply_stream_event(StreamEvent::ToolUseStarted {
            session_id: "session".to_string(),
            tool_use_id: tool_use_id.to_string(),
            tool_name: "Agent".to_string(),
            tool_input: String::new(),
        });
        state.apply_stream_event(StreamEvent::ToolProgress {
            session_id: "session".to_string(),
            tool_use_id: tool_use_id.to_string(),
            tool_name: "Agent".to_string(),
            progress: serde_json::json!({
                "data": {
                    "type": "agent_progress",
                    "status": status
                }
            }),
        });
    }

    let rendered = plain_text_lines(&state.transcript_lines(120)).join("\n");

    assert!(
        rendered.contains("Running 2 Explore agents... (ctrl+o to expand)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Explore existing e2e test patterns · Searching for 11 patterns"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Explore mock provider and stream events · Searching for 6 patterns"),
        "{rendered}"
    );
    assert_eq!(
        rendered.matches("Running 2 Explore agents").count(),
        1,
        "{rendered}"
    );
}

#[test]
fn overlapping_live_tool_activities_keep_earlier_details_visible() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.expanded_tool_details = true;
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
                "status": "Reading 1 file",
                "message": {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
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
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "bash-tool".to_string(),
            name: "Bash".to_string(),
            input: "{\"command\":\"git status --short\"}".to_string(),
        }],
    ));
    state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "session".to_string(),
        tool_use_id: "bash-tool".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: String::new(),
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
        rendered.iter().any(|line| line.contains("Read(README.md)")),
        "previous live tool details should remain visible after a second tool starts: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("git status --short")),
        "newer live tool should still be rendered as the current activity: {rendered:?}"
    );
    assert_eq!(state.live_tool_cells.len(), 2);
    assert_eq!(
        state
            .latest_live_tool_activity()
            .map(|activity| activity.tool_use_id.as_str()),
        Some("bash-tool")
    );
}

#[test]
fn live_tool_progress_updates_do_not_reorder_started_tools() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "session".to_string(),
        tool_use_id: "first-tool".to_string(),
        tool_name: "Agent".to_string(),
        tool_input: String::new(),
    });
    state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "session".to_string(),
        tool_use_id: "second-tool".to_string(),
        tool_name: "Bash".to_string(),
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
                            { "type": "text", "text": "First tool still running." }
                        ]
                    }
                }
            }
        }),
    });

    let ids = state
        .live_tool_activities()
        .into_iter()
        .map(|activity| activity.tool_use_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["first-tool", "second-tool"]);
}

#[test]
fn live_bash_progress_burst_stays_low_noise_before_and_after_result() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let command = "yes line | head -n 12000";
    let mut state = normal_state("", 0);
    state.cwd = cwd.clone();
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "bash-large-live".to_string(),
            name: "Bash".to_string(),
            input: serde_json::json!({ "command": command }).to_string(),
        }],
    ));
    state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "session".to_string(),
        tool_use_id: "bash-large-live".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: String::new(),
    });
    state.apply_stream_event(StreamEvent::ToolProgress {
        session_id: "session".to_string(),
        tool_use_id: "bash-large-live".to_string(),
        tool_name: "Bash".to_string(),
        progress: serde_json::json!({
            "data": {
                "type": "bash_progress",
                "status": "Running bash command"
            }
        }),
    });
    for bytes in [4096, 20480, 36864, 53248] {
        state.apply_stream_event(StreamEvent::ToolProgress {
            session_id: "session".to_string(),
            tool_use_id: "bash-large-live".to_string(),
            tool_name: "Bash".to_string(),
            progress: serde_json::json!({
                "data": {
                    "type": "bash_progress",
                    "status": "Streaming stdout",
                    "stream": "stdout",
                    "bytes": bytes
                }
            }),
        });
    }
    state.apply_stream_event(StreamEvent::ToolProgress {
        session_id: "session".to_string(),
        tool_use_id: "bash-large-live".to_string(),
        tool_name: "Bash".to_string(),
        progress: serde_json::json!({
            "data": {
                "type": "bash_progress",
                "status": "Bash command completed"
            }
        }),
    });

    let active_brief = plain_text_lines(&state.transcript_lines(120)).join("\n");
    assert!(active_brief.contains("Bash(yes line | head -n 12000)"));
    assert!(active_brief.contains("(ctrl+o to expand)"));
    assert_eq!(active_brief.matches("Bash command completed...").count(), 1);

    state.apply_stream_event(StreamEvent::ToolUseCompleted {
        session_id: "session".to_string(),
        tool_use_id: "bash-large-live".to_string(),
        tool_name: "Bash".to_string(),
        kind: orbcode_protocol::ToolUseCompletionKind::Success,
    });
    let bash_truncation_note = "[Bash output truncated for transcript safety. Re-run with a narrower command if you need the omitted portion. Omitted 30139 characters.]";
    let mut content = "line\n".repeat(BASH_EXPANDED_OUTPUT_DETAIL_LIMIT + 40);
    content.push('\n');
    content.push_str(bash_truncation_note);
    let metadata = serde_json::json!({
        "status": "completed",
        "summary": "Executed `yes line | head -n 12000`.",
        "bash": {
            "command": command,
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
    state.apply_stream_event(StreamEvent::UserMessage {
        message: TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "bash-large-live".to_string(),
                content,
                is_error: false,
                metadata: Some(metadata),
            }],
        ),
    });

    let completed_history = state.take_history_lines(120, 30);
    assert!(!state.has_live_tool_activity());

    let completed_brief = plain_text_lines(&completed_history).join("\n");

    assert!(
        completed_brief.contains("Bash(yes line | head -n 12000)"),
        "{completed_brief}"
    );
    assert!(
        completed_brief.contains("(ctrl+o to expand)"),
        "{completed_brief}"
    );
}

#[test]
fn post_large_bash_result_waiting_state_stays_low_noise() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let command = "yes line | head -n 12000";
    let bash_truncation_note = "[Bash output truncated for transcript safety. Re-run with a narrower command if you need the omitted portion. Omitted 30139 characters.]";
    let mut content = "line\n".repeat(5_900);
    content.push('\n');
    content.push_str(bash_truncation_note);
    let metadata = serde_json::json!({
        "status": "completed",
        "summary": "Executed `yes line | head -n 12000`.",
        "bash": {
            "command": command,
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
    let mut state = normal_state("", 0);
    state.cwd = cwd.clone();
    state.request_in_flight = true;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(5_000));
    state.messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "bash-large-wait".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({ "command": command }).to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "bash-large-wait".to_string(),
                content,
                is_error: false,
                metadata: Some(metadata),
            }],
        ),
    ];

    let collapsed = plain_text_lines(&state.transcript_lines(120)).join("\n");
    let status = plain_text_lines(&state.request_status_lines()).join("\n");
    let rendered_output_line_count = |text: &str| {
        text.lines()
            .filter(|line| matches!(line.trim(), "│ line" | "└ line" | "line"))
            .count()
    };

    assert!(collapsed.contains("Bash(yes line | head -n 12000)"));
    assert!(
        collapsed.contains("Bash output truncated for transcript safety"),
        "{collapsed}"
    );
    assert!(collapsed.contains("(ctrl+o to expand)"), "{collapsed}");
    assert!(rendered_output_line_count(&collapsed) <= 3, "{collapsed}");
    assert!(!collapsed.contains("Thought for"), "{collapsed}");
    assert!(status.contains("(5s · ↑ 0 tokens)"), "{status}");
    assert!(!status.contains("line"), "{status}");
}

#[test]
fn active_turn_orphan_tool_use_renders_as_running_until_result_arrives() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "tool-pending".to_string(),
            name: "Bash".to_string(),
            input:
                r#"{"command":"find orbcode -type f -name \"*.rs\" ! -path \"*/target/*\" | wc -l"}"#
                    .to_string(),
        }],
    ));

    let rendered = state
        .transcript_lines(120)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Bash(find orbcode"), "{rendered}");
    assert!(rendered.contains("Running"), "{rendered}");
    assert!(!rendered.contains(INTERRUPTED_TOOL_RESULT), "{rendered}");
}

#[test]
fn active_turn_pending_tool_use_with_trailing_hook_context_stays_running() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "tool-pending".to_string(),
            name: "Bash".to_string(),
            input:
                r#"{"command":"find orbcode -type f -name \"*.rs\" ! -path \"*/target/*\" | wc -l"}"#
                    .to_string(),
        }],
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "PreToolUse hook context:\ninspect output carefully",
    ));

    let rendered = state
        .transcript_lines(120)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Bash(find orbcode"), "{rendered}");
    assert!(rendered.contains("Running"), "{rendered}");
    assert!(!rendered.contains(INTERRUPTED_TOOL_RESULT), "{rendered}");
}

#[test]
fn active_turn_multi_tool_pending_after_other_result_stays_running() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::ToolUse {
                    id: "tool-complete".to_string(),
                    name: "Bash".to_string(),
                    input: r#"{"command":"find orbcode -type f -name \"*.rs\" ! -path \"*/target/*\" | wc -l"}"#
                        .to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "tool-pending".to_string(),
                    name: "Bash".to_string(),
                    input: r#"{"command":"find src -type f \\( -name \"*.ts\" -o -name \"*.tsx\" \\) | wc -l"}"#
                        .to_string(),
                },
            ],
        ));
    state.messages.push(TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-complete".to_string(),
                content: "  84913 total".to_string(),
                is_error: false,
                metadata: Some(
                    r#"{"summary":"Executed `find orbcode -type f -name \"*.rs\" ! -path \"*/target/*\" | wc -l`."}"#
                        .to_string(),
                ),
            }],
        ));

    let rendered = state
        .transcript_lines(120)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Bash(find src"), "{rendered}");
    assert!(rendered.contains("Running"), "{rendered}");
    assert!(!rendered.contains(INTERRUPTED_TOOL_RESULT), "{rendered}");
}

#[test]
fn active_turn_group_pending_with_trailing_hook_context_stays_active() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![
            TranscriptBlock::ToolUse {
                id: "tool-complete".to_string(),
                name: "Bash".to_string(),
                input: r#"{"command":"find . -type f -name \"*.rs\" | head -20"}"#.to_string(),
            },
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
    ));
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::User,
        vec![TranscriptBlock::ToolResult {
            tool_use_id: "tool-complete".to_string(),
            content: "./orbcode/Cargo.toml".to_string(),
            is_error: false,
            metadata: Some(
                r#"{"summary":"Executed `find . -type f -name \"*.rs\" | head -20`."}"#.to_string(),
            ),
        }],
    ));
    state.messages.push(TranscriptMessage::new(
        MessageRole::User,
        "PostToolUse hook context:\ncompleted first tool",
    ));

    let rendered = state
        .transcript_lines(120)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        rendered.contains("Searching for 1 pattern, listing 1 directory..."),
        "{rendered}"
    );
    assert!(
        !rendered.contains("Searched for 1 pattern, listed 1 directory"),
        "{rendered}"
    );
}

#[test]
fn active_turn_old_orphan_tool_use_stays_interrupted_after_user_message() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "tool-old".to_string(),
            name: "Bash".to_string(),
            input: r#"{"command":"printf hi"}"#.to_string(),
        }],
    ));
    state
        .messages
        .push(TranscriptMessage::new(MessageRole::User, "next prompt"));

    let rendered = state
        .transcript_lines(100)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Bash(printf hi)"), "{rendered}");
    assert!(rendered.contains(ORPHANED_TOOL_RESULT), "{rendered}");
    assert!(!rendered.contains(INTERRUPTED_TOOL_RESULT), "{rendered}");
}

#[test]
fn embedded_progress_message_content_omits_raw_tool_markers() {
    let message = embedded_progress_message_to_transcript(&serde_json::json!({
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
                        },
                        {
                            "type": "tool_result",
                            "tool_use_id": "file-read-1",
                            "content": "Read 10 lines",
                            "is_error": false
                        }
                    ]
                }
            }
        }
    }))
    .expect("embedded progress message should parse");

    assert_eq!(message.content, "Checking the core flow now.");
    assert!(!message.content.contains("[tool_use"));
    assert!(!message.content.contains("[tool_result"));
    assert_eq!(message.blocks.len(), 3);
}

#[test]
fn legacy_flattened_tool_messages_render_without_raw_markers() {
    let cwd = PathBuf::from("/Users/user/github/sample-repo-tui-render");
    let assistant = TranscriptMessage {
        id: "assistant-tool".to_string(),
        role: MessageRole::Assistant,
        content:
            "[tool_use Read]\n{\n  \"file_path\": \"/Users/user/github/sample-repo/README.md\"\n}"
                .to_string(),
        blocks: Vec::new(),
        stop_reason: None,
        usage: None,
        created_at: Utc::now(),
        is_synthetic: false,
    };
    let user = TranscriptMessage {
        id: "user-tool-result".to_string(),
        role: MessageRole::User,
        content: "[tool_result file-read-1]\nRead 10 lines".to_string(),
        blocks: Vec::new(),
        stop_reason: None,
        usage: None,
        created_at: Utc::now(),
        is_synthetic: false,
    };

    let rendered = [assistant, user]
        .into_iter()
        .flat_map(|message| render_message_lines(&message, &cwd, true, None, 90, "subagent", false))
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line.contains("README.md")));
    assert!(rendered.iter().any(|line| line.contains("Read 10 lines")));
    assert!(!rendered.iter().any(|line| line.contains("[tool_use")));
    assert!(!rendered.iter().any(|line| line.contains("[tool_result")));
}
