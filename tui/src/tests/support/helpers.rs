use super::*;

pub fn test_temp_path(name: &str) -> PathBuf {
    let timestamp = Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| Utc::now().timestamp_micros());
    std::env::temp_dir().join(format!(
        "orbcode-tui-{name}-{}-{timestamp}",
        std::process::id()
    ))
}

pub fn run_git_test_command(cwd: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .expect("run git command");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

pub fn permission_overlay_with_viewport(area: Rect) -> PermissionOverlayState {
    let mut permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Agent".to_string(),
        tool_input: serde_json::json!({
            "description": "Explore",
            "prompt": "Inspect permission panel",
            "subagent_type": "Explore"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });
    let lines = vec![Line::from("Alpha"), Line::from("Beta"), Line::from("Gamma")];
    permission
        .viewport
        .sync(area, lines.clone(), lines, 0, 0, 0);
    permission
}

pub fn fill_long_transcript(state: &mut TuiState, count: usize) {
    state.messages = (0..count)
            .map(|index| {
                let role = if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                };
                TranscriptMessage::new(
                    role,
                    format!(
                        "render metrics transcript row {index:03}: stable context for overlay and selection measurement"
                    ),
                )
            })
            .collect();
}

pub fn long_agent_permission_request() -> PermissionRequest {
    let prompt = (0..90)
            .map(|index| {
                format!(
                    "Step {index:02}: inspect render path details, preserve transcript context, and report bounded output."
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

    PermissionRequest {
        request_id: "req-render-metrics-agent".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "agent-render-metrics".to_string(),
        tool_name: "Agent".to_string(),
        tool_input: serde_json::json!({
            "description": "Audit render performance",
            "prompt": prompt,
            "subagent_type": "explorer"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    }
}

pub fn progress_statuses(hidden_count: usize) -> Vec<Value> {
    (0..hidden_count)
        .map(|index| {
            serde_json::json!({
                "status": format!("Hidden cumulative progress step {index}")
            })
        })
        .chain([
            serde_json::json!({"status": "Checked render loop"}),
            serde_json::json!({"status": "Ready with bounded progress"}),
        ])
        .collect()
}

pub fn stream_progress_event(status: &str) -> StreamEvent {
    StreamEvent::ToolProgress {
        session_id: "session".to_string(),
        tool_use_id: "agent-render-metrics".to_string(),
        tool_name: "Agent".to_string(),
        progress: serde_json::json!({ "status": status }),
    }
}

pub fn hook_notice_event(event_name: &str) -> StreamEvent {
    StreamEvent::HookNotice {
        session_id: "session".to_string(),
        hook_event_name: event_name.to_string(),
        message: "hook progress visible".to_string(),
        is_error: false,
    }
}

pub fn hook_progress_event(event_name: &str, duration_ms: u64) -> StreamEvent {
    StreamEvent::HookProgress {
        session_id: "session".to_string(),
        hook_event_name: event_name.to_string(),
        progress: serde_json::json!({
            "data": {
                "type": "hook_progress",
                "hookEventName": event_name,
                "result": "completed",
                "durationMs": duration_ms,
                "exitCode": 0
            }
        }),
    }
}

pub fn apply_long_session_tool_run(state: &mut TuiState, index: usize, progress_count: usize) {
    let tool_use_id = format!("long-session-tool-{index}");
    let tool_name = "Agent";
    state.push_message_and_flush_history(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: tool_use_id.clone(),
            name: tool_name.to_string(),
            input: serde_json::json!({
                "description": format!("Long session tool {index}"),
                "prompt": "exercise long-session memory growth budgets",
                "subagent_type": "explorer"
            })
            .to_string(),
        }],
    ));

    assert!(!state.apply_stream_event(StreamEvent::ToolUseStarted {
        session_id: "session".to_string(),
        tool_use_id: tool_use_id.clone(),
        tool_name: tool_name.to_string(),
        tool_input: String::new(),
    }));

    for progress_index in 0..progress_count {
        assert!(!state.apply_stream_event(StreamEvent::ToolProgress {
            session_id: "session".to_string(),
            tool_use_id: tool_use_id.clone(),
            tool_name: tool_name.to_string(),
            progress: serde_json::json!({
                "status": format!("tool {index} progress {progress_index}")
            }),
        }));
    }

    assert_eq!(
        state
            .find_live_tool_activity_by_tool_use_id(&tool_use_id)
            .map(|activity| activity.progress_messages.len()),
        Some(progress_count.min(LIVE_TOOL_PROGRESS_MESSAGE_LIMIT))
    );

    assert!(!state.apply_stream_event(StreamEvent::ToolUseCompleted {
        session_id: "session".to_string(),
        tool_use_id: tool_use_id.clone(),
        tool_name: tool_name.to_string(),
        kind: orbcode_protocol::ToolUseCompletionKind::Success,
    }));
    assert!(!state.apply_stream_event(StreamEvent::UserMessage {
        message: TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id,
                content: format!("long session tool result {index}"),
                is_error: false,
                metadata: None,
            }],
        ),
    }));
}

pub fn completed_tool_result_progress_lengths(state: &TuiState) -> Vec<usize> {
    state
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            TranscriptBlock::ToolResult { metadata, .. } => {
                Some(tool_activity_progress_messages(metadata.as_deref()).len())
            }
            _ => None,
        })
        .collect()
}

pub fn attached_hook_progress_count(state: &TuiState) -> usize {
    state
        .hook_progress_by_message_id
        .values()
        .map(Vec::len)
        .sum()
}
