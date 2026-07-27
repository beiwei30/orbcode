use super::support::*;
use super::*;

#[tokio::test]
async fn last_provider_request_snapshot_records_stream_request_body() {
    let manager = test_manager().await;
    let mut rx = manager
        .submit_turn("session-last-request", "please answer hello")
        .await
        .expect("submit turn");

    tokio::time::timeout(StdDuration::from_secs(2), async {
        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::TurnFinished { .. }) {
                break;
            }
        }
    })
    .await
    .expect("turn finishes");

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert_eq!(snapshot.provider, ProviderId::Anthropic);
    assert_eq!(snapshot.source, "turn");
    assert_eq!(snapshot.session_id, "session-last-request");
    assert_eq!(snapshot.previous_turn_json, "[]");
    assert!(snapshot.body_json.contains("please answer hello"));
    assert!(snapshot.body_json.contains("\"system\""));
    assert!(!snapshot.body_json.contains("api_key"));
    assert!(!snapshot.body_json.contains("auth_token"));
}

#[tokio::test]
async fn last_provider_request_snapshot_keeps_tool_debug_activity_across_follow_up_request() {
    let mut manager = test_manager().await;
    manager.config.allow_tools = true;
    let mut rx = manager
        .submit_turn(
            "session-last-tool-request",
            r#"#tool:bash {"command":"printf hi"}"#,
        )
        .await
        .expect("submit turn");

    tokio::time::timeout(StdDuration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::TurnFinished { .. }) {
                break;
            }
        }
    })
    .await
    .expect("turn finishes");

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert!(
        snapshot
            .recent_activity_json
            .contains("assistant_response_from_llm")
    );
    assert!(
        snapshot
            .recent_activity_json
            .contains("\"type\": \"tool_use\"")
    );
    assert!(snapshot.recent_activity_json.contains("\"name\": \"bash\""));
    assert!(snapshot.recent_activity_json.contains("tool_result_to_llm"));
    assert!(snapshot.recent_activity_json.contains("hi"));
}

#[tokio::test]
async fn emits_assistant_deltas_for_text_before_tool_use() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    manager
        .handle_provider_response(
            &session_id,
            Uuid::new_v4(),
            "inspect repo",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: "Let me inspect the workspace.".to_string(),
                blocks: vec![
                    TranscriptBlock::Text {
                        text: "Let me inspect the workspace.".to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "glob".to_string(),
                        input: r#"{"pattern":"src/**/*"}"#.to_string(),
                    },
                ],
                stop_reason: Some("tool_use".to_string()),
                usage: TokenUsage::default(),
                deltas: chunk_response("Let me inspect the workspace."),
            },
            0,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("handle response");

    let mut saw_delta = false;
    let mut saw_tool_start = false;
    let mut saw_tool_progress = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::AssistantDelta { delta, .. } => {
                saw_delta |= delta.contains("Let me inspect");
            }
            StreamEvent::ToolUseStarted { tool_name, .. } => {
                saw_tool_start |= tool_name == "glob";
            }
            StreamEvent::ToolProgress {
                tool_name,
                progress,
                ..
            } => {
                saw_tool_progress |= tool_name == "glob"
                    && progress
                        .get("data")
                        .and_then(|data| data.get("status"))
                        .and_then(Value::as_str)
                        == Some("Searching for 1 pattern");
            }
            _ => {}
        }
    }

    assert!(saw_delta);
    assert!(saw_tool_start);
    assert!(saw_tool_progress);
}

#[tokio::test]
async fn tool_only_response_does_not_emit_raw_marker_deltas() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    manager
        .handle_provider_response(
            &session_id,
            Uuid::new_v4(),
            "inspect repo",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: String::new(),
                blocks: vec![TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "glob".to_string(),
                    input: r#"{"pattern":"src/**/*"}"#.to_string(),
                }],
                stop_reason: Some("tool_use".to_string()),
                usage: TokenUsage::default(),
                deltas: Vec::new(),
            },
            0,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("handle response");

    let mut saw_assistant_delta = false;
    let mut saw_tool_start = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::AssistantDelta { delta, .. } => {
                saw_assistant_delta = true;
                assert!(
                    !delta.contains("[tool_use"),
                    "tool-only responses should not stream raw tool markers: {delta}"
                );
            }
            StreamEvent::ToolUseStarted { tool_name, .. } => {
                saw_tool_start |= tool_name == "glob";
            }
            _ => {}
        }
    }

    assert!(!saw_assistant_delta);
    assert!(saw_tool_start);
}

#[tokio::test]
async fn rejects_empty_final_assistant_response() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let error = manager
        .handle_provider_response(
            &session_id,
            Uuid::new_v4(),
            "inspect repo",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: String::new(),
                blocks: Vec::new(),
                stop_reason: Some("end_turn".to_string()),
                usage: TokenUsage::default(),
                deltas: Vec::new(),
            },
            0,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect_err("empty response should fail");

    assert!(matches!(
        error,
        CoreError::ProviderFailed(failure) if failure.message.contains("empty assistant response")
    ));
}

#[tokio::test]
async fn empty_response_after_successful_workflow_tool_result_finishes_turn() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "workflow-tool".to_string(),
                    name: "Workflow".to_string(),
                    input: "{}".to_string(),
                }],
            ),
        )
        .await
        .expect("append workflow tool use");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "workflow-tool".to_string(),
                    content: serde_json::json!({
                        "task_id": "workflow-541b91f4a2de475e9a377a4d26e795c3",
                        "status": "started"
                    })
                    .to_string(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        )
        .await
        .expect("append workflow tool result");

    let outcome = manager
        .handle_provider_response(
            &session_id,
            Uuid::new_v4(),
            "run a workflow",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: String::new(),
                blocks: Vec::new(),
                stop_reason: Some("end_turn".to_string()),
                usage: TokenUsage::default(),
                deltas: Vec::new(),
            },
            0,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("empty response after workflow result should finish");

    assert_eq!(outcome, TurnLoopOutcome::Finished);
    let mut saw_finished = false;
    while let Ok(event) = rx.try_recv() {
        saw_finished |= matches!(event, StreamEvent::TurnFinished { .. });
    }
    assert!(saw_finished);
}

#[tokio::test]
async fn planning_only_repo_reply_triggers_auto_continue() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let outcome = manager
        .handle_provider_response(
            &session_id,
            Uuid::new_v4(),
            "评估一下这个仓库中各个 crate 的测试覆盖情况",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: "我来先查看一下项目结构。".to_string(),
                blocks: vec![TranscriptBlock::Text {
                    text: "我来先查看一下项目结构。".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: TokenUsage::default(),
                deltas: chunk_response("我来先查看一下项目结构。"),
            },
            0,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("handle response");

    assert!(matches!(
        outcome,
        TurnLoopOutcome::AutoContinue(
            NoToolTurnReason::PlanningCue | NoToolTurnReason::ThinPlanningReply
        )
    ));
}

#[tokio::test]
async fn max_tokens_response_appends_partial_and_auto_continues() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let partial = "This answer is incomplete at the truncation point.";

    let outcome = manager
        .handle_provider_response(
            &session_id,
            Uuid::new_v4(),
            "Explain Rust ownership briefly.",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: partial.to_string(),
                blocks: vec![TranscriptBlock::Text {
                    text: partial.to_string(),
                }],
                stop_reason: Some("max_tokens".to_string()),
                usage: TokenUsage::default(),
                deltas: chunk_response(partial),
            },
            0,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("handle response");

    assert!(matches!(
        outcome,
        TurnLoopOutcome::AutoContinue(NoToolTurnReason::MaxOutput)
    ));
    let mut saw_completion = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(
            event,
            StreamEvent::AssistantMessageCompleted { message, .. }
                if message.content == partial
        ) {
            saw_completion = true;
        }
    }
    assert!(saw_completion);
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    assert!(saved.messages.iter().any(|message| {
        matches!(message.role, MessageRole::Assistant) && message.content == partial
    }));
}
