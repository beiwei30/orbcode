use super::support::*;

#[test]
fn hook_context_user_message_renders_as_local_note() {
    let message = TranscriptMessage::new(
        MessageRole::User,
        "UserPromptSubmit hook context:\nhook progress visible".to_string(),
    );

    let styled = render_message_lines(
        &message,
        Path::new("."),
        false,
        None,
        80,
        "test-model",
        false,
    );
    let rendered = plain_text_lines(&styled);

    assert_eq!(rendered[0], "● UserPromptSubmit hook");
    assert_eq!(rendered[1], "  └ hook progress visible");
    assert!(!rendered.iter().any(|line| line.starts_with('›')));
    assert!(
        styled[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::DIM)
    );
    assert_eq!(styled[0].spans[0].style.fg, inactive_style().fg);
}

#[test]
fn hook_feedback_user_message_renders_as_local_note_generically() {
    let message = TranscriptMessage::new(
        MessageRole::User,
        "SomeFutureHook hook feedback:\nline one\nline two".to_string(),
    );

    let styled = render_message_lines(
        &message,
        Path::new("."),
        false,
        None,
        80,
        "test-model",
        false,
    );
    let rendered = plain_text_lines(&styled);

    assert_eq!(rendered[0], "● SomeFutureHook hook");
    assert_eq!(rendered[1], "  └ line one");
    assert_eq!(rendered[2], "    line two");
    assert!(!rendered.iter().any(|line| line.starts_with('›')));
    assert_eq!(styled[0].spans[0].style.fg, Some(active_palette().error));
}

#[test]
fn hook_note_is_separated_from_following_thinking() {
    let hook = render_message_lines(
        &TranscriptMessage::new(
            MessageRole::User,
            "UserPromptSubmit hook context:\nhook progress visible".to_string(),
        ),
        Path::new("."),
        false,
        None,
        80,
        "test-model",
        false,
    );
    let thinking = render_message_lines(
        &TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::Thinking {
                text: "Thinking about the request".to_string(),
                signature: None,
            }],
        ),
        Path::new("."),
        false,
        None,
        80,
        "test-model",
        false,
    );

    let rendered = plain_text_lines(&flatten_transcript_cells(&[hook, thinking]));

    assert!(
        rendered
            .windows(2)
            .any(|window| { window[0].is_empty() && window[1].starts_with("∴ Thinking") })
    );
}

#[test]
fn failed_read_card_hides_completed_post_tool_failure_hook_progress() {
    let cwd = PathBuf::from("/Users/user/github/sample-workspace-main/crates/render-fixtures");
    let metadata = serde_json::json!({
        "progressMessages": [
            {
                "data": {
                    "type": "hook_progress",
                    "hookEventName": "PostToolUseFailure",
                    "status": "PostToolUseFailure hook completed in 105 ms",
                    "result": "completed",
                    "durationMs": 105,
                    "exitCode": 0
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
                input: serde_json::json!({
                    "file_path": cwd.join("tools/src/lib.rs").display().to_string()
                })
                .to_string(),
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "read-large".to_string(),
                content: "Failed during execution".to_string().into(),
                is_error: true,
                metadata: Some(metadata),
            }],
        ),
    ];

    let (tool_cells, _) =
        build_collapsible_tool_cells_from_message(&messages, 0, &cwd, false).unwrap();
    let collapsed = plain_text_lines(&render_tool_cell_lines(
        &tool_cells[0],
        false,
        None,
        90,
        &cwd,
    ))
    .join("\n");
    let expanded = plain_text_lines(&render_tool_cell_lines(
        &tool_cells[0],
        true,
        None,
        90,
        &cwd,
    ))
    .join("\n");

    assert!(collapsed.contains("Read(tools/src/lib.rs)"), "{collapsed}");
    assert!(collapsed.contains("Failed during execution"), "{collapsed}");
    assert!(!collapsed.contains("PostToolUseFailure"), "{collapsed}");
    assert!(!expanded.contains("PostToolUseFailure"), "{expanded}");
}

#[test]
fn hook_progress_event_attaches_to_next_hook_note() {
    let mut state = normal_state("", 0);
    state.status_line = "Ready.".to_string();

    let finished = state.apply_stream_event(StreamEvent::HookProgress {
        session_id: "session".to_string(),
        hook_event_name: "UserPromptSubmit".to_string(),
        progress: serde_json::json!({
            "data": {
                "type": "hook_progress",
                "hookEventName": "UserPromptSubmit",
                "status": "UserPromptSubmit hook completed in 4 ms",
                "result": "completed",
                "durationMs": 4,
                "exitCode": 0
            }
        }),
    });

    assert!(!finished);
    assert_eq!(state.status_line, "Ready.");
    assert_eq!(state.pending_hook_progress.len(), 1);

    state.apply_stream_event(StreamEvent::UserMessage {
        message: TranscriptMessage::new(
            MessageRole::User,
            "UserPromptSubmit hook context:\nhook progress visible".to_string(),
        ),
    });

    let transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(transcript.contains("● UserPromptSubmit hook"));
    assert!(transcript.contains("  └ hook progress visible"));
    assert!(transcript.contains("    completed in 4 ms (exit 0)"));
    assert!(state.pending_hook_progress.is_empty());
}

#[test]
fn hook_notice_renders_as_local_hook_note() {
    let mut state = normal_state("", 0);

    let finished = state.apply_stream_event(StreamEvent::HookNotice {
        session_id: "session".to_string(),
        hook_event_name: "Stop".to_string(),
        message: "Done enough".to_string(),
        is_error: false,
    });

    let transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(!finished);
    assert!(transcript.contains("● Stop hook"));
    assert!(transcript.contains("  └ Done enough"));
    assert!(!transcript.contains("› Stop hook"));
}
