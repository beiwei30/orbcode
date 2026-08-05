use super::support::*;
use super::*;

#[tokio::test]
async fn forwards_live_bash_progress_from_tool_runtime() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:bash {"command":"printf alpha && sleep 0.05 && printf beta >&2"}"#,
        )
        .await
        .expect("submit turn");

    let mut statuses = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::ToolProgress {
                tool_name,
                progress,
                ..
            } if tool_name == "bash" => {
                if let Some(status) = progress
                    .get("data")
                    .and_then(|data| data.get("status"))
                    .and_then(Value::as_str)
                {
                    statuses.push(status.to_string());
                }
            }
            StreamEvent::TurnFinished { .. } => break,
            _ => {}
        }
    }

    assert!(
        statuses
            .iter()
            .any(|status| status == "Running bash command")
    );
    assert!(statuses.iter().any(|status| status == "Streaming stdout"));
    assert!(statuses.iter().any(|status| status == "Streaming stderr"));
    assert!(
        statuses
            .iter()
            .any(|status| status == "Bash command completed")
    );
}

#[tokio::test]
async fn local_agent_tool_forwards_nested_progress_and_metadata() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
            .submit_turn(
                &session_id,
                r##"#tool:Agent {"description":"Explore repo","prompt":"#tool:bash {\"command\":\"printf nested && sleep 0.05 && printf err >&2\"}","subagent_type":"Explore"}"##,
            )
            .await
            .expect("submit turn");

    let mut saw_agent_progress = false;
    let mut saw_nested_tool_use = false;
    let mut saw_nested_tool_result = false;
    let mut saw_nested_bash_progress = false;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::ToolProgress {
                tool_name,
                progress,
                ..
            } if tool_name == "Agent" => {
                let data = progress.get("data");
                saw_agent_progress |= data
                    .and_then(|value| value.get("type"))
                    .and_then(Value::as_str)
                    == Some("agent_progress");
                saw_nested_bash_progress |= data
                    .and_then(|value| value.get("type"))
                    .and_then(Value::as_str)
                    == Some("bash_progress");
                saw_nested_tool_use |= data
                    .and_then(|value| value.get("message"))
                    .and_then(|message| message.get("type"))
                    .and_then(Value::as_str)
                    == Some("assistant");
                saw_nested_tool_result |= data
                    .and_then(|value| value.get("message"))
                    .and_then(|message| message.get("type"))
                    .and_then(Value::as_str)
                    == Some("user");
            }
            StreamEvent::TurnFinished { .. } => break,
            _ => {}
        }
    }

    assert!(saw_agent_progress);
    assert!(saw_nested_tool_use);
    assert!(saw_nested_tool_result);
    assert!(saw_nested_bash_progress);

    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    let metadata = saved
        .messages
        .iter()
        .find_map(|message| {
            message.blocks.iter().find_map(|block| match block {
                TranscriptBlock::ToolResult {
                    tool_use_id,
                    metadata,
                    ..
                } if tool_use_id == &format!("toolu-{session_id}") => metadata.clone(),
                _ => None,
            })
        })
        .expect("agent metadata");
    let parsed = serde_json::from_str::<Value>(&metadata).expect("parse agent metadata");

    assert_eq!(
        parsed.get("totalToolUseCount").and_then(Value::as_u64),
        Some(1)
    );
    let progress_messages = parsed
        .get("progressMessages")
        .and_then(Value::as_array)
        .expect("progress messages array");
    assert!(progress_messages.iter().any(|progress| {
        progress
            .get("data")
            .and_then(|data| data.get("type"))
            .and_then(Value::as_str)
            == Some("agent_progress")
    }));
    assert!(progress_messages.iter().any(|progress| {
        progress
            .get("data")
            .and_then(|data| data.get("type"))
            .and_then(Value::as_str)
            == Some("bash_progress")
    }));
}

#[tokio::test]
async fn local_agent_tool_keeps_final_text_out_of_progress() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
            .submit_turn(
                &session_id,
                r#"#tool:Agent {"description":"Summarize repo","prompt":"summarize the workspace","subagent_type":"Explore"}"#,
            )
            .await
            .expect("submit turn");

    let mut saw_assistant_text_progress = false;
    let mut saw_final_tool_result_text = false;
    let mut agent_progress_count = 0;
    let mut agent_completed = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::ToolProgress {
                tool_name,
                progress,
                ..
            } if tool_name == "Agent" && !agent_completed => {
                if progress
                    .get("data")
                    .and_then(|data| data.get("type"))
                    .and_then(Value::as_str)
                    == Some("agent_progress")
                {
                    agent_progress_count += 1;
                }
                let text = progress
                    .get("data")
                    .and_then(|data| data.get("message"))
                    .and_then(|message| message.get("message"))
                    .and_then(|message| message.get("content"))
                    .and_then(Value::as_array)
                    .and_then(|content| {
                        content.iter().find_map(|block| {
                            (block.get("type").and_then(Value::as_str) == Some("text"))
                                .then(|| block.get("text").and_then(Value::as_str))
                                .flatten()
                        })
                    })
                    .unwrap_or_default();
                saw_assistant_text_progress |= text.contains("Anthropic");
            }
            StreamEvent::UserMessage { message } => {
                saw_final_tool_result_text |= message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult { content, .. }
                            if content.contains("Anthropic")
                    )
                });
            }
            StreamEvent::ToolUseCompleted {
                tool_name, kind, ..
            } if tool_name == "Agent" => {
                agent_completed = kind == ToolUseCompletionKind::Success;
            }
            StreamEvent::TurnFinished { .. } => break,
            _ => {}
        }
    }

    assert!(!saw_assistant_text_progress);
    assert!(saw_final_tool_result_text);
    assert_eq!(agent_progress_count, 1);
    assert!(agent_completed);
}

#[tokio::test]
async fn agent_loop_sequential_tool_round_preserves_provider_tool_use_order() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "run tool order test"),
        )
        .await
        .expect("seed session transcript");
    let tool_uses = vec![
        ToolRoundToolUse::new(
            "tool-first".to_string(),
            "bash".to_string(),
            r#"{"command":"printf first"}"#.to_string(),
        ),
        ToolRoundToolUse::new(
            "tool-second".to_string(),
            "bash".to_string(),
            r#"{"command":"printf second"}"#.to_string(),
        ),
    ];
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_sequential_tool_round(
            &session_id,
            ToolRoundScheduler::from_tool_uses(tool_uses),
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool round");

    let mut started = Vec::new();
    let mut completed = Vec::new();
    let mut results = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::ToolUseStarted { tool_use_id, .. } => {
                started.push(tool_use_id);
            }
            StreamEvent::ToolUseCompleted {
                tool_use_id, kind, ..
            } => {
                assert_eq!(kind, ToolUseCompletionKind::Success);
                completed.push(tool_use_id);
            }
            StreamEvent::UserMessage { message } => {
                for block in message.blocks {
                    if let TranscriptBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } = block
                    {
                        results.push((tool_use_id, content, is_error));
                    }
                }
            }
            _ => {}
        }
    }

    assert_eq!(outcome, SequentialToolRoundOutcome::Continue);
    assert_eq!(started, vec!["tool-first", "tool-second"]);
    assert_eq!(completed, vec!["tool-first", "tool-second"]);
    assert_eq!(
        results
            .iter()
            .map(|(tool_use_id, _, _)| tool_use_id.as_str())
            .collect::<Vec<_>>(),
        vec!["tool-first", "tool-second"]
    );
    assert!(results[0].1.contains("first"));
    assert!(!results[0].2);
    assert!(results[1].1.contains("second"));
    assert!(!results[1].2);
}

#[tokio::test]
async fn agent_loop_queues_pre_tool_context_until_after_tool_result_for_next_request() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    manager.config.settings.hooks.insert(
        "PreToolUse".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","additionalContext":"queued pre-tool context"}}'"#.to_string(),
                r#if: None,
                timeout: Some(5.0),
            }],
        }],
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let prompt = r#"#tool:bash {"command":"printf queued"}"#;
    let tool_use_id = format!("toolu-{session_id}");
    let context_message = "PreToolUse hook context:\nqueued pre-tool context";
    let mut rx = manager
        .submit_turn(&session_id, prompt)
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
    .expect("turn should finish");

    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    let assistant_index = saved
        .messages
        .iter()
        .position(|message| {
            message.blocks.iter().any(
                |block| matches!(block, TranscriptBlock::ToolUse { id, .. } if id == &tool_use_id),
            )
        })
        .expect("assistant tool use should be persisted");
    let result_index = saved
        .messages
        .iter()
        .position(|message| {
            message.blocks.iter().any(|block| {
                matches!(block, TranscriptBlock::ToolResult { tool_use_id: id, .. } if id == &tool_use_id)
            })
        })
        .expect("tool result should be persisted");
    let context_indexes = saved
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message.content == context_message).then_some(index))
        .collect::<Vec<_>>();

    assert_eq!(context_indexes, vec![result_index + 1]);
    assert!(assistant_index < result_index);

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    let markers = provider_request_tool_context_markers(&snapshot.body_json, context_message);
    assert_ordered_markers(
        &markers,
        &[
            format!("tool_use:{tool_use_id}"),
            format!("tool_result:{tool_use_id}"),
            "context".to_string(),
        ],
    );
}

#[tokio::test]
async fn agent_loop_multi_tool_round_flushes_queued_context_after_all_tool_results() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    manager.config.settings.hooks.insert(
        "PreToolUse".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","additionalContext":"queued multi-tool context"}}'"#.to_string(),
                r#if: None,
                timeout: Some(5.0),
            }],
        }],
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "run multi queued context test"),
        )
        .await
        .expect("seed user message");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "tool-first".to_string(),
                        name: "bash".to_string(),
                        input: r#"{"command":"printf first"}"#.to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "tool-second".to_string(),
                        name: "bash".to_string(),
                        input: r#"{"command":"printf second"}"#.to_string(),
                    },
                ],
            ),
        )
        .await
        .expect("seed assistant tool round");
    let tool_uses = vec![
        ToolRoundToolUse::new("tool-first", "bash", r#"{"command":"printf first"}"#),
        ToolRoundToolUse::new("tool-second", "bash", r#"{"command":"printf second"}"#),
    ];
    let (tx, _rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_sequential_tool_round(
            &session_id,
            ToolRoundScheduler::from_tool_uses(tool_uses),
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool round");

    assert_eq!(outcome, SequentialToolRoundOutcome::Continue);
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    let markers = saved
        .messages
        .iter()
        .filter_map(|message| {
            if message.content == "PreToolUse hook context:\nqueued multi-tool context" {
                return Some("context".to_string());
            }
            message.blocks.iter().find_map(|block| match block {
                TranscriptBlock::ToolResult { tool_use_id, .. } => {
                    Some(format!("tool_result:{tool_use_id}"))
                }
                _ => None,
            })
        })
        .collect::<Vec<_>>();

    assert_eq!(
        markers,
        vec![
            "tool_result:tool-first",
            "tool_result:tool-second",
            "context",
            "context",
        ]
    );
}

fn provider_request_tool_context_markers(body_json: &str, context_message: &str) -> Vec<String> {
    let body = serde_json::from_str::<Value>(body_json).expect("provider request body is json");
    let mut markers = Vec::new();
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .expect("provider request has messages");
    for message in messages {
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        markers.push(format!("tool_use:{id}"));
                    }
                }
                Some("tool_result") => {
                    if let Some(tool_use_id) = block.get("tool_use_id").and_then(Value::as_str) {
                        markers.push(format!("tool_result:{tool_use_id}"));
                    }
                }
                Some("text")
                    if block.get("text").and_then(Value::as_str) == Some(context_message) =>
                {
                    markers.push("context".to_string());
                }
                _ => {}
            }
        }
    }
    markers
}

fn assert_ordered_markers(markers: &[String], expected: &[String]) {
    let mut cursor = 0;
    for marker in markers {
        if expected.get(cursor) == Some(marker) {
            cursor += 1;
            if cursor == expected.len() {
                return;
            }
        }
    }
    panic!("expected marker order {expected:?} in {markers:?}");
}

#[tokio::test]
async fn agent_loop_streamed_tool_use_starts_before_message_stop_and_commits_after_assistant() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "run streamed tool"),
        )
        .await
        .expect("seed session transcript");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let mut stream_sink = SessionProviderStreamSink::new(
        manager.clone(),
        &session_id,
        ProviderId::Anthropic,
        &tx,
        cancel_flag.clone(),
    );

    stream_sink
        .emit(ProviderStreamEvent::MessageStart {
            provider: ProviderId::Anthropic,
            fallback_from: None,
            usage: TokenUsage::default(),
        })
        .await
        .expect("message start");
    stream_sink
        .emit(ProviderStreamEvent::ContentBlockStart {
            index: 0,
            block: ProviderContentBlockStart::ToolUse {
                id: "tool-streamed".to_string(),
                name: "bash".to_string(),
                input: String::new(),
            },
        })
        .await
        .expect("tool start block");
    stream_sink
        .emit(ProviderStreamEvent::ContentBlockDelta {
            index: 0,
            delta: ProviderContentBlockDelta::InputJson(
                r#"{"command":"printf streamed"}"#.to_string(),
            ),
        })
        .await
        .expect("tool input delta");
    stream_sink
        .emit(ProviderStreamEvent::ContentBlockStop { index: 0 })
        .await
        .expect("tool stop block");

    let started_before_message_stop = tokio::time::timeout(StdDuration::from_secs(2), async {
        while let Some(event) = rx.recv().await {
            if let StreamEvent::ToolUseStarted { tool_use_id, .. } = event {
                return tool_use_id;
            }
        }
        panic!("event stream ended before streamed tool start");
    })
    .await
    .expect("streamed tool should start before message stop");
    assert_eq!(started_before_message_stop, "tool-streamed");

    stream_sink
        .emit(ProviderStreamEvent::MessageDelta {
            stop_reason: Some("tool_use".to_string()),
            usage: TokenUsage::default(),
        })
        .await
        .expect("message delta");
    stream_sink
        .emit(ProviderStreamEvent::MessageStop)
        .await
        .expect("message stop");
    let session_stream_result = stream_sink.into_session_provider_stream_result();
    let tool_round_response = session_stream_result
        .tool_round_stream
        .into_tool_round_response();

    let outcome = manager
        .finish_provider_response_with_streamed_tools(
            &session_id,
            Uuid::new_v4(),
            "run streamed tool",
            tool_round_response,
            String::new(),
            0,
            false,
            &tx,
            cancel_flag,
            session_stream_result.streamed_tool_executions,
        )
        .await
        .expect("finish provider response");
    assert_eq!(outcome, TurnLoopOutcome::Continue);

    let mut saw_assistant_completed = false;
    let mut result_after_assistant = None;
    let mut completed = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::AssistantMessageCompleted { .. } => {
                saw_assistant_completed = true;
            }
            StreamEvent::UserMessage { message } => {
                for block in message.blocks {
                    if let TranscriptBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } = block
                    {
                        result_after_assistant =
                            Some((saw_assistant_completed, tool_use_id, content, is_error));
                    }
                }
            }
            StreamEvent::ToolUseCompleted {
                tool_use_id, kind, ..
            } => {
                completed.push((tool_use_id, kind));
            }
            _ => {}
        }
    }

    let (after_assistant, tool_use_id, content, is_error) =
        result_after_assistant.expect("tool result event");
    assert!(after_assistant);
    assert_eq!(tool_use_id, "tool-streamed");
    assert!(content.contains("streamed"));
    assert!(!is_error);
    assert_eq!(
        completed,
        vec![("tool-streamed".to_string(), ToolUseCompletionKind::Success)]
    );
}

#[tokio::test]
async fn agent_loop_streamed_agent_tool_uses_start_before_message_stop() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "run streamed agents"),
        )
        .await
        .expect("seed session transcript");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let mut stream_sink = SessionProviderStreamSink::new(
        manager.clone(),
        &session_id,
        ProviderId::Anthropic,
        &tx,
        cancel_flag.clone(),
    );

    stream_sink
        .emit(ProviderStreamEvent::MessageStart {
            provider: ProviderId::Anthropic,
            fallback_from: None,
            usage: TokenUsage::default(),
        })
        .await
        .expect("message start");

    for (index, (tool_use_id, description)) in [
        ("agent-first", "Explore existing tests"),
        ("agent-second", "Explore provider streams"),
    ]
    .into_iter()
    .enumerate()
    {
        stream_sink
            .emit(ProviderStreamEvent::ContentBlockStart {
                index,
                block: ProviderContentBlockStart::ToolUse {
                    id: tool_use_id.to_string(),
                    name: "Agent".to_string(),
                    input: String::new(),
                },
            })
            .await
            .expect("agent tool start block");
        stream_sink
            .emit(ProviderStreamEvent::ContentBlockDelta {
                index,
                delta: ProviderContentBlockDelta::InputJson(
                    serde_json::json!({
                        "description": description,
                        "prompt": format!("summarize {description}"),
                        "subagent_type": "Explore"
                    })
                    .to_string(),
                ),
            })
            .await
            .expect("agent tool input delta");
        stream_sink
            .emit(ProviderStreamEvent::ContentBlockStop { index })
            .await
            .expect("agent tool stop block");
    }

    let started_before_message_stop = tokio::time::timeout(StdDuration::from_secs(2), async {
        let mut started = Vec::new();
        while let Some(event) = rx.recv().await {
            if let StreamEvent::ToolUseStarted { tool_use_id, .. } = event {
                started.push(tool_use_id);
                if started.len() == 2 {
                    return started;
                }
            }
        }
        panic!("event stream ended before both streamed agents started");
    })
    .await
    .expect("streamed agents should start before message stop");
    assert_eq!(
        started_before_message_stop,
        vec!["agent-first".to_string(), "agent-second".to_string()]
    );

    stream_sink
        .emit(ProviderStreamEvent::MessageDelta {
            stop_reason: Some("tool_use".to_string()),
            usage: TokenUsage::default(),
        })
        .await
        .expect("message delta");
    stream_sink
        .emit(ProviderStreamEvent::MessageStop)
        .await
        .expect("message stop");
    let session_stream_result = stream_sink.into_session_provider_stream_result();
    let tool_round_response = session_stream_result
        .tool_round_stream
        .into_tool_round_response();

    let outcome = manager
        .finish_provider_response_with_streamed_tools(
            &session_id,
            Uuid::new_v4(),
            "run streamed agents",
            tool_round_response,
            String::new(),
            0,
            false,
            &tx,
            cancel_flag,
            session_stream_result.streamed_tool_executions,
        )
        .await
        .expect("finish provider response");
    assert_eq!(outcome, TurnLoopOutcome::Continue);

    let mut completed = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::ToolUseCompleted {
            tool_use_id, kind, ..
        } = event
        {
            completed.push((tool_use_id, kind));
        }
    }
    assert_eq!(
        completed,
        vec![
            ("agent-first".to_string(), ToolUseCompletionKind::Success),
            ("agent-second".to_string(), ToolUseCompletionKind::Success),
        ]
    );
}

#[tokio::test]
async fn agent_loop_discarded_streamed_tool_use_emits_interrupted_completion() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "run discarded streamed tool"),
        )
        .await
        .expect("seed session transcript");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let mut stream_sink = SessionProviderStreamSink::new(
        manager.clone(),
        &session_id,
        ProviderId::Anthropic,
        &tx,
        cancel_flag.clone(),
    );

    stream_sink
        .emit(ProviderStreamEvent::MessageStart {
            provider: ProviderId::Anthropic,
            fallback_from: None,
            usage: TokenUsage::default(),
        })
        .await
        .expect("message start");
    stream_sink
        .emit(ProviderStreamEvent::ContentBlockStart {
            index: 0,
            block: ProviderContentBlockStart::ToolUse {
                id: "tool-streamed".to_string(),
                name: "bash".to_string(),
                input: String::new(),
            },
        })
        .await
        .expect("tool start block");
    stream_sink
        .emit(ProviderStreamEvent::ContentBlockDelta {
            index: 0,
            delta: ProviderContentBlockDelta::InputJson(
                r#"{"command":"printf discarded"}"#.to_string(),
            ),
        })
        .await
        .expect("tool input delta");
    stream_sink
        .emit(ProviderStreamEvent::ContentBlockStop { index: 0 })
        .await
        .expect("tool stop block");

    tokio::time::timeout(StdDuration::from_secs(2), async {
        while let Some(event) = rx.recv().await {
            if matches!(
                event,
                StreamEvent::ToolUseStarted { tool_use_id, .. }
                if tool_use_id == "tool-streamed"
            ) {
                return;
            }
        }
        panic!("event stream ended before discarded streamed tool start");
    })
    .await
    .expect("discarded streamed tool should start");
    let session_stream_result = stream_sink.into_session_provider_stream_result();
    let tool_round_response = ToolRoundResponse::from_response_and_streamed_tool_uses(
        ProviderResponse {
            provider: ProviderId::Anthropic,
            fallback_from: None,
            content: String::new(),
            blocks: vec![TranscriptBlock::ToolUse {
                id: "tool-final".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"printf final"}"#.to_string(),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: TokenUsage::default(),
            deltas: Vec::new(),
        },
        vec![ToolRoundToolUse::new(
            "tool-streamed",
            "bash",
            r#"{"command":"printf discarded"}"#,
        )],
    );

    let outcome = manager
        .finish_provider_response_with_streamed_tools(
            &session_id,
            Uuid::new_v4(),
            "run discarded streamed tool",
            tool_round_response,
            String::new(),
            0,
            false,
            &tx,
            cancel_flag,
            session_stream_result.streamed_tool_executions,
        )
        .await
        .expect("finish provider response");
    assert_eq!(outcome, TurnLoopOutcome::Continue);

    let mut completed = Vec::new();
    let mut results = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::ToolUseCompleted {
                tool_use_id, kind, ..
            } => completed.push((tool_use_id, kind)),
            StreamEvent::UserMessage { message } => {
                for block in message.blocks {
                    if let TranscriptBlock::ToolResult { tool_use_id, .. } = block {
                        results.push(tool_use_id);
                    }
                }
            }
            _ => {}
        }
    }

    assert!(completed.contains(&(
        "tool-streamed".to_string(),
        ToolUseCompletionKind::Interrupted
    )));
    assert!(completed.contains(&("tool-final".to_string(), ToolUseCompletionKind::Success)));
    assert_eq!(results, vec!["tool-final".to_string()]);
}

#[tokio::test]
async fn agent_loop_cancellation_synthesizes_remaining_tool_results_in_order() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allowed_tools: vec!["Bash(printf first)".to_string()],
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "run cancel synthesis test"),
        )
        .await
        .expect("seed session transcript");
    let response = ProviderResponse {
        provider: ProviderId::Anthropic,
        fallback_from: None,
        content: String::new(),
        blocks: vec![
            TranscriptBlock::ToolUse {
                id: "tool-first".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"printf first"}"#.to_string(),
            },
            TranscriptBlock::ToolUse {
                id: "tool-second".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"printf second"}"#.to_string(),
            },
            TranscriptBlock::ToolUse {
                id: "tool-third".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"printf third"}"#.to_string(),
            },
        ],
        stop_reason: Some("tool_use".to_string()),
        usage: TokenUsage::default(),
        deltas: Vec::new(),
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let worker = {
        let manager = manager.clone();
        let session_id = session_id.clone();
        let cancel_flag = cancel_flag.clone();
        tokio::spawn(async move {
            manager
                .finish_provider_response(
                    &session_id,
                    Uuid::new_v4(),
                    "multi tool cancellation",
                    ToolRoundResponse::from_response(response),
                    String::new(),
                    0,
                    false,
                    &tx,
                    cancel_flag,
                )
                .await
        })
    };

    let mut completed = Vec::new();
    let mut results = Vec::new();
    let mut saw_permission_interrupted = false;
    let cancel_kind = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::PermissionRequested { request } => {
                    assert_eq!(request.tool_use_id, "tool-second");
                    cancel_flag.store(true, Ordering::SeqCst);
                }
                StreamEvent::PermissionResolved { kind, .. } => {
                    saw_permission_interrupted |= kind == PermissionResolutionKind::Interrupted;
                }
                StreamEvent::ToolUseCompleted {
                    tool_use_id, kind, ..
                } => {
                    completed.push((tool_use_id, kind));
                }
                StreamEvent::UserMessage { message } => {
                    for block in message.blocks {
                        if let TranscriptBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                            ..
                        } = block
                        {
                            results.push((tool_use_id, content, is_error));
                        }
                    }
                }
                StreamEvent::TurnCancelled { kind, .. } => return kind,
                _ => {}
            }
        }
        panic!("stream ended before turn cancellation");
    })
    .await
    .expect("tool cancellation should finish");
    let outcome = worker
        .await
        .expect("join finish task")
        .expect("finish provider response");

    assert_eq!(outcome, TurnLoopOutcome::Cancelled);
    assert_eq!(cancel_kind, TurnCancellationKind::ToolStage);
    assert!(saw_permission_interrupted);
    assert_eq!(
        completed,
        vec![
            ("tool-first".to_string(), ToolUseCompletionKind::Success),
            (
                "tool-second".to_string(),
                ToolUseCompletionKind::Interrupted
            ),
            ("tool-third".to_string(), ToolUseCompletionKind::Interrupted),
        ]
    );
    assert_eq!(
        results
            .iter()
            .map(|(tool_use_id, _, _)| tool_use_id.as_str())
            .collect::<Vec<_>>(),
        vec!["tool-first", "tool-second", "tool-third"]
    );
    assert!(results[0].1.contains("first"));
    assert!(!results[0].2);
    assert_eq!(results[1].1, INTERRUPTED_TOOL_RESULT);
    assert!(results[1].2);
    assert_eq!(results[2].1, INTERRUPTED_TOOL_RESULT);
    assert!(results[2].2);
}

#[tokio::test]
async fn agent_loop_streamed_tool_error_interrupts_pending_streamed_executions() {
    use crate::tool_flow::{
        BufferedToolResult, BufferedToolUseCompletion, StreamedToolUseExecution, ToolUseOutcome,
    };
    use orbcode_protocol::TokenUsage;

    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "run streamed error test"),
        )
        .await
        .expect("seed session transcript");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));

    let failing_handle = tokio::spawn(async {
        Err::<BufferedToolUseCompletion, CoreError>(CoreError::Tool(
            "forced streamed tool failure".to_string(),
        ))
    });
    let pending_handle = tokio::spawn(async {
        tokio::time::sleep(StdDuration::from_secs(30)).await;
        Ok(BufferedToolUseCompletion {
            outcome: ToolUseOutcome::Continue,
            result: BufferedToolResult {
                tool_use_id: "tool-pending".to_string(),
                tool_name: "bash".to_string(),
                content: "unreached".to_string(),
                is_error: false,
                metadata: None,
                completion_kind: ToolUseCompletionKind::Success,
            },
        })
    });
    let streamed_tool_executions = vec![
        StreamedToolUseExecution::new("tool-fail".to_string(), "bash".to_string(), failing_handle),
        StreamedToolUseExecution::new(
            "tool-pending".to_string(),
            "bash".to_string(),
            pending_handle,
        ),
    ];

    let tool_round_response = ToolRoundResponse::from_response_and_streamed_tool_uses(
        ProviderResponse {
            provider: ProviderId::Anthropic,
            fallback_from: None,
            content: String::new(),
            blocks: vec![
                TranscriptBlock::ToolUse {
                    id: "tool-fail".to_string(),
                    name: "bash".to_string(),
                    input: r#"{"command":"printf fail"}"#.to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "tool-pending".to_string(),
                    name: "bash".to_string(),
                    input: r#"{"command":"printf pending"}"#.to_string(),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: TokenUsage::default(),
            deltas: Vec::new(),
        },
        vec![
            ToolRoundToolUse::new("tool-fail", "bash", r#"{"command":"printf fail"}"#),
            ToolRoundToolUse::new("tool-pending", "bash", r#"{"command":"printf pending"}"#),
        ],
    );

    let result = manager
        .finish_provider_response_with_streamed_tools(
            &session_id,
            Uuid::new_v4(),
            "run streamed error test",
            tool_round_response,
            String::new(),
            0,
            false,
            &tx,
            cancel_flag,
            streamed_tool_executions,
        )
        .await;

    let error = result.expect_err("finish_provider_response should propagate streamed tool error");
    assert!(
        error.to_string().contains("forced streamed tool failure"),
        "unexpected error message: {error}",
    );

    let mut completed = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::ToolUseCompleted {
            tool_use_id, kind, ..
        } = event
        {
            completed.push((tool_use_id, kind));
        }
    }

    assert!(
        completed.contains(&(
            "tool-pending".to_string(),
            ToolUseCompletionKind::Interrupted
        )),
        "pending streamed tool should be interrupted, saw {completed:?}",
    );
}

#[tokio::test]
async fn unknown_tool_appends_error_result_and_completes_unknown() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-unknown",
            "DefinitelyNotATool",
            "{}",
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute unknown tool");

    let mut saw_error_result = false;
    let mut saw_unknown_completion = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::UserMessage { message } => {
                saw_error_result |= message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                            ..
                        } if tool_use_id == "tool-unknown"
                            && content.contains("unknown tool requested by provider")
                            && *is_error
                    )
                });
            }
            StreamEvent::ToolUseCompleted {
                tool_use_id, kind, ..
            } => {
                saw_unknown_completion |=
                    tool_use_id == "tool-unknown" && kind == ToolUseCompletionKind::UnknownTool;
            }
            _ => {}
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Continue);
    assert!(saw_error_result);
    assert!(saw_unknown_completion);
}

#[tokio::test]
async fn tool_runtime_skill_loads_trusted_mcp_prompt_skill() {
    use crate::tool_flow::ToolUseOutcome;
    use orbcode_mcp::{McpAuth, McpServerConfig, McpServerStatus, McpServerTrust, McpTransport};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    struct FakeHttpResponse {
        body: String,
    }

    fn fake_ok(body: serde_json::Value) -> FakeHttpResponse {
        FakeHttpResponse {
            body: body.to_string(),
        }
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).expect("read fake MCP request");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end + 4]);
            let content_length = headers
                .lines()
                .find_map(|line| line.split_once(':'))
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let expected = header_end + 4 + content_length;
            while bytes.len() < expected {
                let read = stream.read(&mut buffer).expect("read fake MCP body");
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..read]);
            }
            break;
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn spawn_fake_http_mcp_server(
        requests: usize,
        handler: impl Fn(String) -> FakeHttpResponse + Send + 'static,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake MCP server");
        let endpoint = format!("http://{}/mcp", listener.local_addr().expect("local addr"));
        thread::spawn(move || {
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().expect("accept fake MCP request");
                stream
                    .set_read_timeout(Some(StdDuration::from_secs(2)))
                    .expect("set read timeout");
                let request = read_http_request(&mut stream);
                let response = handler(request);
                let payload = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.body.len(),
                    response.body
                );
                let _ = stream.write_all(payload.as_bytes());
            }
        });
        endpoint
    }

    fn json_rpc_id(request: &str) -> serde_json::Value {
        request
            .split("\r\n\r\n")
            .nth(1)
            .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
            .and_then(|value| value.get("id").cloned())
            .unwrap_or_else(|| json!(1))
    }

    let endpoint = spawn_fake_http_mcp_server(4, |request| {
        let id = json_rpc_id(&request);
        let result = if request.contains(r#""method":"initialize""#) {
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "prompts": {} },
                "serverInfo": { "name": "fake-skills", "version": "0.1.0" }
            })
        } else if request.contains(r#""method":"prompts/list""#) {
            json!({
                "prompts": [{
                    "name": "guide",
                    "description": "List description",
                    "skill": true,
                    "arguments": []
                }]
            })
        } else if request.contains(r#""method":"prompts/get""#) {
            json!({
                "description": "Rendered description",
                "messages": [{
                    "role": "user",
                    "content": {
                        "type": "text",
                        "text": "---\ndescription: Rendered Docs Guide\n---\nUse $ARGUMENTS from the MCP skill."
                    }
                }]
            })
        } else {
            json!({})
        };
        fake_ok(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    });

    let manager = test_manager().await;
    manager
        .mcp
        .upsert_server(McpServerConfig {
            id: "docs".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Docs".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert mcp");
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "load docs skill"),
        )
        .await
        .expect("seed session transcript");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-skill",
            "Skill",
            r#"{"skill":"docs:guide","args":"runtime args"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute skill tool");

    let mut saw_skill_result = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            saw_skill_result |= message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } if tool_use_id == "tool-skill"
                        && content.contains("Skill: docs:guide")
                        && content.contains("Use runtime args from the MCP skill.")
                        && !*is_error
                )
            });
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Continue);
    assert!(
        saw_skill_result,
        "tool runtime should load MCP prompt skills for provider-visible Skill calls"
    );
}

struct RuntimeMcpSkillFakeHttpResponse {
    body: String,
}

fn runtime_mcp_skill_fake_ok(body: serde_json::Value) -> RuntimeMcpSkillFakeHttpResponse {
    RuntimeMcpSkillFakeHttpResponse {
        body: body.to_string(),
    }
}

fn read_runtime_mcp_skill_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = std::io::Read::read(stream, &mut buffer).expect("read fake MCP request");
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end + 4]);
        let content_length = headers
            .lines()
            .find_map(|line| line.split_once(':'))
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let expected = header_end + 4 + content_length;
        while bytes.len() < expected {
            let read = std::io::Read::read(stream, &mut buffer).expect("read fake MCP body");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
        break;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn spawn_runtime_mcp_skill_http_server() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake MCP server");
    let endpoint = format!("http://{}/mcp", listener.local_addr().expect("local addr"));
    std::thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().expect("accept fake MCP request");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .expect("set read timeout");
            let request = read_runtime_mcp_skill_http_request(&mut stream);
            let id = request
                .split("\r\n\r\n")
                .nth(1)
                .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
                .and_then(|value| value.get("id").cloned())
                .unwrap_or_else(|| serde_json::json!(1));
            let result = if request.contains(r#""method":"initialize""#) {
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "prompts": {} },
                    "serverInfo": { "name": "fake-skills", "version": "0.1.0" }
                })
            } else if request.contains(r#""method":"prompts/list""#) {
                serde_json::json!({
                    "prompts": [{
                        "name": "guide",
                        "description": "List description",
                        "skill": true,
                        "arguments": []
                    }]
                })
            } else if request.contains(r#""method":"prompts/get""#) {
                serde_json::json!({
                    "description": "Rendered description",
                    "messages": [{
                        "role": "user",
                        "content": {
                            "type": "text",
                            "text": "---\ndescription: Rendered Docs Guide\n---\nUse $ARGUMENTS from the MCP skill."
                        }
                    }]
                })
            } else {
                serde_json::json!({})
            };
            let response = runtime_mcp_skill_fake_ok(
                serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}),
            );
            let payload = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.body.len(),
                response.body
            );
            let _ = std::io::Write::write_all(&mut stream, payload.as_bytes());
        }
    });
    endpoint
}

async fn seed_runtime_mcp_skill_server(manager: &super::super::SessionManager) {
    use orbcode_mcp::{McpAuth, McpServerConfig, McpServerStatus, McpServerTrust, McpTransport};
    use std::collections::BTreeMap;

    manager
        .mcp
        .upsert_server(McpServerConfig {
            id: "docs".to_string(),
            transport: McpTransport::Http,
            endpoint: spawn_runtime_mcp_skill_http_server(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Docs".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert mcp");
}

#[tokio::test]
async fn streamed_tool_runtime_skill_loads_trusted_mcp_prompt_skill() {
    use crate::agent_loop::tool_round::{ToolRoundScheduler, ToolRoundToolUse};

    let manager = test_manager().await;
    seed_runtime_mcp_skill_server(&manager).await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut scheduler = ToolRoundScheduler::new();
    let ready_item = scheduler.accept_tool_use(ToolRoundToolUse::new(
        "tool-streamed-skill",
        "Skill",
        r#"{"skill":"docs:guide","args":"runtime args"}"#,
    ));

    let execution = manager
        .start_streamed_tool_execution(
            &session.session_id,
            ready_item,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("streamed Skill execution should start");
    let completion = execution.finish().await.expect("finish streamed Skill");

    assert_eq!(
        completion.result.completion_kind,
        ToolUseCompletionKind::Success
    );
    assert!(!completion.result.is_error);
    assert!(
        completion
            .result
            .content
            .contains("Use runtime args from the MCP skill."),
        "streamed tool runtime should load MCP prompt skills for Skill calls: {}",
        completion.result.content
    );
}

#[tokio::test]
async fn nested_agent_skill_tool_loads_trusted_mcp_prompt_skill() {
    let manager = test_manager().await;
    seed_runtime_mcp_skill_server(&manager).await;
    let (tx, _rx) = mpsc::unbounded_channel();

    let outcome = manager
        .invoke_nested_agent_tool(
            "session-1",
            "parent-tool-use",
            "child-agent-1",
            "Skill",
            r#"{"skill":"docs:guide","args":"runtime args"}"#,
            None,
            true,
            true,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("nested Skill should load MCP skill");

    assert!(
        outcome
            .output
            .contains("Use runtime args from the MCP skill."),
        "nested agent Skill calls should load MCP prompt skills: {}",
        outcome.output
    );
}

#[tokio::test]
async fn large_tool_results_are_persisted_as_model_visible_previews() {
    let manager = test_manager().await;
    let session_id = "large-tool-result-session";
    let original = "line\n".repeat(12_000);

    let preview = manager
        .maybe_persist_large_tool_result(session_id, "tool-1", "web-fetch", original.clone())
        .await
        .expect("persist large result");

    assert!(preview.starts_with(PERSISTED_OUTPUT_TAG));
    assert!(preview.contains("Full output saved to:"));
    assert!(preview.contains("Preview (first"));
    assert!(preview.len() < original.len());
    let path = manager
        .config
        .current_project_dir
        .join(session_id)
        .join("tool-results")
        .join("tool-1.txt");
    assert_eq!(
        tokio::fs::read_to_string(path)
            .await
            .expect("read persisted result"),
        original
    );
}

#[tokio::test]
async fn persisted_large_bash_preview_retains_truncation_diagnostic() {
    let manager = test_manager().await;
    let session_id = "large-bash-result-session";
    let bash_note = "[Bash output truncated for transcript safety. Re-run with a narrower command if you need the omitted portion. Omitted 70000 characters.]";
    let original = format!("{}\n\n{bash_note}", "line\n".repeat(12_000));

    let preview = manager
        .maybe_persist_large_tool_result(session_id, "tool-1", "bash", original.clone())
        .await
        .expect("persist large bash result");

    assert!(preview.starts_with(PERSISTED_OUTPUT_TAG));
    assert!(preview.contains("Preview (first"));
    assert!(preview.contains(bash_note), "{preview}");
    assert_eq!(preview.matches(bash_note).count(), 1, "{preview}");
    assert!(preview.len() < original.len());
}

#[tokio::test]
async fn read_tool_results_skip_large_result_persistence() {
    let manager = test_manager().await;
    let session_id = "read-large-result-session";
    let original = "line\n".repeat(12_000);

    let content = manager
        .maybe_persist_large_tool_result(session_id, "tool-1", "Read", original.clone())
        .await
        .expect("skip read persistence");

    assert_eq!(content, original);
    assert!(
        !manager
            .config
            .current_project_dir
            .join(session_id)
            .join("tool-results")
            .exists()
    );
}

#[tokio::test]
async fn aggregate_tool_result_budget_replaces_largest_group_members() {
    let manager = test_manager().await;
    let session_id = "aggregate-tool-result-session";
    let assistant = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        (0..5)
            .map(|index| TranscriptBlock::ToolUse {
                id: format!("tool-{index}"),
                name: "web-fetch".to_string(),
                input: "{}".to_string(),
            })
            .collect(),
    );
    let mut messages = vec![assistant];
    for index in 0..5 {
        messages.push(TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: format!("tool-{index}"),
                content: "x".repeat(45_000).into(),
                is_error: false,
                metadata: None,
            }],
        ));
    }

    let visible = manager
        .model_visible_messages_with_tool_result_budget(session_id, messages)
        .await
        .expect("apply aggregate budget");
    let replaced = visible
        .iter()
        .flat_map(|message| &message.blocks)
        .filter(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult { content, .. }
                    if content.starts_with(PERSISTED_OUTPUT_TAG)
            )
        })
        .count();

    assert_eq!(replaced, 1);
    assert!(
        manager
            .config
            .current_project_dir
            .join(session_id)
            .join("tool-results")
            .join("tool-0.txt")
            .exists()
    );
}

#[tokio::test]
async fn aggregate_tool_result_budget_skips_read_results() {
    let manager = test_manager().await;
    let session_id = "aggregate-read-skip-session";
    let messages = vec![
        TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::ToolUse {
                    id: "read-tool".to_string(),
                    name: "Read".to_string(),
                    input: "{}".to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "bash-tool".to_string(),
                    name: "bash".to_string(),
                    input: "{}".to_string(),
                },
            ],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "read-tool".to_string(),
                content: "r".repeat(190_000).into(),
                is_error: false,
                metadata: None,
            }],
        ),
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "bash-tool".to_string(),
                content: "b".repeat(20_000).into(),
                is_error: false,
                metadata: None,
            }],
        ),
    ];

    let visible = manager
        .model_visible_messages_with_tool_result_budget(session_id, messages)
        .await
        .expect("apply aggregate budget");
    let replaced = visible
        .iter()
        .flat_map(|message| &message.blocks)
        .any(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult { content, .. }
                    if content.starts_with(PERSISTED_OUTPUT_TAG)
            )
        });

    assert!(!replaced);
    assert!(
        !manager
            .config
            .current_project_dir
            .join(session_id)
            .join("tool-results")
            .exists()
    );
}

#[tokio::test]
async fn provider_request_for_session_repairs_and_summarizes_tool_round_contract() {
    let manager = test_manager().await;
    let session_id = "provider-request-tool-round-contract";
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::User, "run the checks"),
        )
        .await
        .expect("append initial prompt");
    manager
        .append_message(
            session_id,
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "bash".to_string(),
                        input: r#"{"command":"printf missing"}"#.to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "tool-2".to_string(),
                        name: "glob".to_string(),
                        input: r#"{"pattern":"*.rs"}"#.to_string(),
                    },
                ],
            ),
        )
        .await
        .expect("append assistant tool uses");
    manager
        .append_message(
            session_id,
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-2".to_string(),
                    content: "src/lib.rs".into(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        )
        .await
        .expect("append one tool result");
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "next response"),
        )
        .await
        .expect("append next assistant response");
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::User, "follow up"),
        )
        .await
        .expect("append current prompt");

    let request = manager
        .provider_request_for_session(
            session_id,
            "follow up",
            manager.context_preview().await,
            &[],
            true,
            true,
        )
        .await
        .expect("provider request");

    assert_eq!(request.session_id, session_id);
    assert_eq!(request.prompt, "follow up");

    let missing_index = request
        .messages
        .iter()
        .position(|message| {
            message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } if tool_use_id == "tool-1"
                        && content == MISSING_TOOL_RESULT
                        && *is_error
                )
            })
        })
        .expect("missing tool result should be repaired");
    let actual_index = request
        .messages
        .iter()
        .position(|message| {
            message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } if tool_use_id == "tool-2" && content == "src/lib.rs" && !*is_error
                )
            })
        })
        .expect("existing tool result should remain visible");
    let summary_index = request
        .messages
        .iter()
        .position(|message| {
            message
                .content
                .starts_with("Tool round summary: 2 tool results.")
        })
        .expect("tool round summary should be model-visible");
    let next_response_index = request
        .messages
        .iter()
        .position(|message| {
            message.role == MessageRole::Assistant && message.content == "next response"
        })
        .expect("next assistant response should remain visible");
    let follow_up_index = request
        .messages
        .iter()
        .position(|message| message.role == MessageRole::User && message.content == "follow up")
        .expect("current prompt should remain visible");

    assert!(missing_index < actual_index);
    assert!(actual_index < summary_index);
    assert!(summary_index < next_response_index);
    assert!(next_response_index < follow_up_index);
    assert_eq!(
        request
            .messages
            .iter()
            .filter(|message| message.content.starts_with("Tool round summary:"))
            .count(),
        1
    );
    let summary = &request.messages[summary_index].content;
    assert!(summary.contains("bash `tool-1`: failed"), "{summary}");
    assert!(summary.contains("glob `tool-2`: completed"), "{summary}");
    assert!(summary.contains(MISSING_TOOL_RESULT), "{summary}");
    assert!(summary.contains("src/lib.rs"), "{summary}");
}

#[tokio::test]
async fn agent_loop_provider_request_strips_fallback_orphan_tool_results() {
    let manager = test_manager().await;
    let session_id = "provider-request-fallback-orphan";
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::User, "start"),
        )
        .await
        .expect("append initial prompt");
    manager
        .append_message(
            session_id,
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "tool-current".to_string(),
                    name: "bash".to_string(),
                    input: r#"{"command":"printf current"}"#.to_string(),
                }],
            ),
        )
        .await
        .expect("append assistant tool use");
    manager
        .append_message(
            session_id,
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![
                    TranscriptBlock::ToolResult {
                        tool_use_id: "tool-fallback-old".to_string(),
                        content: "discarded fallback attempt".into(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "tool-current".to_string(),
                        content: "current result".into(),
                        is_error: false,
                        metadata: None,
                    },
                ],
            ),
        )
        .await
        .expect("append mixed tool results");
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::User, "follow up"),
        )
        .await
        .expect("append current prompt");

    let request = manager
        .provider_request_for_session(
            session_id,
            "follow up",
            manager.context_preview().await,
            &[],
            true,
            true,
        )
        .await
        .expect("provider request");

    let tool_results = request
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            TranscriptBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => Some((tool_use_id.as_str(), content.as_str(), *is_error)),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        tool_results,
        vec![("tool-current", "current result", false)]
    );
}

#[tokio::test]
async fn loads_progress_records_into_tool_result_metadata() {
    let manager = test_manager().await;
    let session_id = "progress-session";
    let transcript_path = manager.transcript_store.path(session_id);
    let payload = [
        json!({
            "type": "assistant",
            "uuid": "assistant-1",
            "timestamp": "2026-04-10T00:00:00.000Z",
            "message": {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "agent-tool",
                        "name": "Agent",
                        "input": {
                            "description": "Explore repo",
                            "prompt": "check CLI flow"
                        }
                    }
                ]
            }
        }),
        json!({
            "type": "progress",
            "uuid": "progress-1",
            "timestamp": "2026-04-10T00:00:01.000Z",
            "parentToolUseID": "agent-tool",
            "data": {
                "type": "agent_progress",
                "prompt": "check CLI flow",
                "agentId": "agent-1",
                "message": {
                    "type": "assistant",
                    "uuid": "nested-assistant-1",
                    "message": {
                        "role": "assistant",
                        "content": [
                            {
                                "type": "tool_use",
                                "id": "file-read-1",
                                "name": "Read",
                                "input": { "file_path": "/tmp/context.rs" }
                            }
                        ]
                    }
                }
            }
        }),
        json!({
            "type": "user",
            "uuid": "user-1",
            "timestamp": "2026-04-10T00:00:02.000Z",
            "toolUseResult": {
                "status": "completed",
                "prompt": "check CLI flow",
                "content": [
                    { "type": "text", "text": "Done reading files." }
                ],
                "totalToolUseCount": 1,
                "totalDurationMs": 1500,
                "totalTokens": 9,
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 2,
                    "cache_creation_input_tokens": null,
                    "cache_read_input_tokens": null,
                    "server_tool_use": null,
                    "service_tier": null,
                    "cache_creation": null
                }
            },
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "agent-tool",
                        "content": "Done reading files.",
                        "is_error": false
                    }
                ]
            }
        }),
    ]
    .into_iter()
    .map(|value| serde_json::to_string(&value).expect("serialize transcript line"))
    .collect::<Vec<_>>()
    .join("\n");
    tokio::fs::write(&transcript_path, format!("{payload}\n"))
        .await
        .expect("write transcript");

    let session = manager
        .load_session(session_id)
        .await
        .expect("load transcript");

    let metadata = match &session.messages[1].blocks[0] {
        TranscriptBlock::ToolResult { metadata, .. } => metadata.clone(),
        other => panic!("expected tool result, got {other:?}"),
    }
    .expect("tool result metadata");
    let parsed: Value = serde_json::from_str(&metadata).expect("parse metadata");
    let progress = parsed
        .get("progressMessages")
        .and_then(Value::as_array)
        .expect("progress messages array");
    assert_eq!(progress.len(), 1);
    assert_eq!(
        progress[0]
            .get("data")
            .and_then(|data| data.get("agentId"))
            .and_then(Value::as_str),
        Some("agent-1")
    );
}

#[tokio::test]
async fn persists_progress_messages_as_separate_transcript_entries() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "agent-tool".to_string(),
                    name: "Agent".to_string(),
                    input: "{\"description\":\"Explore repo\",\"prompt\":\"check CLI flow\"}"
                        .to_string(),
                }],
            ),
        )
        .await
        .expect("append assistant message");

    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "agent-tool".to_string(),
                    content: "Done reading files.".into(),
                    is_error: false,
                    metadata: Some(
                        json!({
                            "status": "completed",
                            "prompt": "check CLI flow",
                            "content": [
                                { "type": "text", "text": "Done reading files." }
                            ],
                            "progressMessages": [
                                {
                                    "uuid": "progress-1",
                                    "timestamp": "2026-04-10T00:00:01.000Z",
                                    "parentToolUseID": "agent-tool",
                                    "data": {
                                        "type": "agent_progress",
                                        "agentId": "agent-1",
                                        "message": {
                                            "type": "assistant",
                                            "message": {
                                                "role": "assistant",
                                                "content": [
                                                    {
                                                        "type": "tool_use",
                                                        "id": "file-read-1",
                                                        "name": "Read",
                                                        "input": { "file_path": "/tmp/context.rs" }
                                                    }
                                                ]
                                            }
                                        }
                                    }
                                }
                            ]
                        })
                        .to_string(),
                    ),
                }],
            ),
        )
        .await
        .expect("append tool result");

    let transcript = tokio::fs::read_to_string(manager.transcript_store.path(&session_id))
        .await
        .expect("read transcript");
    let entries = transcript
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("parse transcript line"))
        .collect::<Vec<_>>();

    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries[1].get("type").and_then(Value::as_str),
        Some("progress")
    );
    assert_eq!(
        entries[1].get("parentToolUseID").and_then(Value::as_str),
        Some("agent-tool")
    );
    assert_eq!(
        entries[2]
            .get("toolUseResult")
            .and_then(|value| value.get("progressMessages")),
        None
    );

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load persisted session");
    let metadata = match &loaded.messages[1].blocks[0] {
        TranscriptBlock::ToolResult { metadata, .. } => metadata.clone(),
        other => panic!("expected tool result, got {other:?}"),
    }
    .expect("tool result metadata");
    let parsed: Value = serde_json::from_str(&metadata).expect("parse metadata");
    let progress = parsed
        .get("progressMessages")
        .and_then(Value::as_array)
        .expect("progress messages array");
    assert_eq!(progress.len(), 1);
    assert_eq!(
        progress[0]
            .get("data")
            .and_then(|data| data.get("agentId"))
            .and_then(Value::as_str),
        Some("agent-1")
    );
}

#[tokio::test]
async fn live_tool_progress_events_merge_into_subsequent_tool_result_metadata() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let (tx, mut rx) = mpsc::unbounded_channel();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "agent-tool".to_string(),
                    name: "Agent".to_string(),
                    input: "{\"description\":\"Explore repo\",\"prompt\":\"check CLI flow\"}"
                        .to_string(),
                }],
            ),
        )
        .await
        .expect("append assistant tool use");

    manager
        .append_tool_progress_event(
            &session_id,
            "agent-tool",
            "Agent",
            json!({
                "uuid": "progress-live-1",
                "timestamp": "2026-04-10T00:00:01.000Z",
                "data": {
                    "type": "agent_progress",
                    "status": "Reading 1 file",
                    "agentId": "agent-1",
                    "message": {
                        "type": "assistant",
                        "message": {
                            "role": "assistant",
                            "content": [
                                {
                                    "type": "tool_use",
                                    "id": "file-read-1",
                                    "name": "Read",
                                    "input": { "file_path": "/tmp/context.rs" }
                                }
                            ]
                        }
                    }
                }
            }),
            &tx,
        )
        .await
        .expect("append tool progress");

    match rx.recv().await.expect("tool progress event") {
        StreamEvent::ToolProgress {
            tool_use_id,
            tool_name,
            progress,
            ..
        } => {
            assert_eq!(tool_use_id, "agent-tool");
            assert_eq!(tool_name, "Agent");
            assert_eq!(
                progress.get("parentToolUseID").and_then(Value::as_str),
                Some("agent-tool")
            );
        }
        other => panic!("expected tool progress, got {other:?}"),
    }

    manager
        .append_tool_result_message(
            &session_id,
            "agent-tool",
            "Done reading files.",
            false,
            Some(
                json!({
                    "status": "completed",
                    "content": [{ "type": "text", "text": "Done reading files." }]
                })
                .to_string(),
            ),
            &tx,
        )
        .await
        .expect("append tool result");

    let transcript = tokio::fs::read_to_string(manager.transcript_store.path(&session_id))
        .await
        .expect("read transcript");
    let progress_entry_count = transcript
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("parse transcript line"))
        .filter(|entry| {
            entry.get("type").and_then(Value::as_str) == Some("progress")
                && entry.get("parentToolUseID").and_then(Value::as_str) == Some("agent-tool")
        })
        .count();
    assert_eq!(progress_entry_count, 1);

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load session");
    let metadata = match &loaded.messages[1].blocks[0] {
        TranscriptBlock::ToolResult { metadata, .. } => metadata.clone(),
        other => panic!("expected tool result, got {other:?}"),
    }
    .expect("tool result metadata");
    let parsed: Value = serde_json::from_str(&metadata).expect("parse metadata");
    let progress = parsed
        .get("progressMessages")
        .and_then(Value::as_array)
        .expect("progress messages");
    assert_eq!(progress.len(), 1);
    assert_eq!(
        progress[0].get("uuid").and_then(Value::as_str),
        Some("progress-live-1")
    );
}

#[tokio::test]
async fn loads_large_bash_transcript_shape_without_progress_or_echo_growth() {
    let manager = test_manager().await;
    let session_id = "large-bash-transcript-shape";
    let tool_use_id = "toolu-large-bash";
    let transcript_path = manager.transcript_store.path(session_id);
    let bash_truncation_note = "[Bash output truncated for transcript safety. Re-run with a narrower command if you need the omitted portion. Omitted 30139 characters.]";
    let progress_record = |uuid: &str, timestamp: &str, data: Value| {
        json!({
            "type": "progress",
            "uuid": uuid,
            "timestamp": timestamp,
            "parentToolUseID": tool_use_id,
            "data": data
        })
    };
    let progress_records = vec![
        progress_record(
            "progress-running",
            "2026-05-14T02:43:46.523Z",
            json!({
                "status": "Running bash command",
                "type": "bash_progress"
            }),
        ),
        progress_record(
            "progress-stream-1",
            "2026-05-14T02:43:46.587Z",
            json!({
                "bytes": 4096,
                "status": "Streaming stdout",
                "stream": "stdout",
                "type": "bash_progress"
            }),
        ),
        progress_record(
            "progress-stream-2",
            "2026-05-14T02:43:46.588Z",
            json!({
                "bytes": 53248,
                "status": "Streaming stdout",
                "stream": "stdout",
                "type": "bash_progress"
            }),
        ),
        progress_record(
            "progress-completed",
            "2026-05-14T02:43:46.590Z",
            json!({
                "exitCode": 0,
                "status": "Bash command completed",
                "type": "bash_progress"
            }),
        ),
    ];
    let tool_result_content = format!("{}\n\n{bash_truncation_note}", "line\n".repeat(5_900),);
    let assistant_echo = format!(
        "Tool `bash` completed.\n\n{}\n[Stub tool result preview truncated for interactive responsiveness. Transcript retains the original tool result. Omitted 28000 middle characters.]\n\n{bash_truncation_note}",
        "line\n".repeat(24),
    );

    let mut lines = vec![json!({
        "type": "assistant",
        "uuid": "assistant-tool-use",
        "timestamp": "2026-05-14T02:43:46.522Z",
        "message": {
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": tool_use_id,
                    "name": "bash",
                    "input": { "command": "yes line | head -n 12000" }
                }
            ]
        }
    })];
    lines.extend(progress_records.iter().cloned());
    lines.extend([
        json!({
            "type": "user",
            "uuid": "user-tool-result",
            "timestamp": "2026-05-14T02:43:46.592Z",
            "toolUseResult": {
                "bash": {
                    "command": "yes line | head -n 12000",
                    "exitCode": 0,
                    "outputChars": 59999,
                    "outputTruncated": true,
                    "stdoutChars": 59999,
                    "stderrChars": 0
                },
                "changedPaths": [],
                "content": [
                    { "type": "text", "text": tool_result_content }
                ],
                "progressMessages": progress_records,
                "status": "completed",
                "summary": "Executed `yes line | head -n 12000`.",
                "toolName": "bash"
            },
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": tool_result_content,
                        "is_error": false
                    }
                ]
            }
        }),
        json!({
            "type": "assistant",
            "uuid": "assistant-echo",
            "timestamp": "2026-05-14T02:43:46.715Z",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": assistant_echo }
                ],
                "stop_reason": "end_turn"
            }
        }),
    ]);
    let payload = lines
        .into_iter()
        .map(|value| serde_json::to_string(&value).expect("serialize transcript line"))
        .collect::<Vec<_>>()
        .join("\n");
    tokio::fs::write(&transcript_path, format!("{payload}\n"))
        .await
        .expect("write transcript");

    let session = manager
        .load_session(session_id)
        .await
        .expect("load transcript");

    assert_eq!(session.messages.len(), 3);
    let metadata = match &session.messages[1].blocks[0] {
        TranscriptBlock::ToolResult {
            tool_use_id,
            content,
            metadata,
            ..
        } => {
            assert_eq!(tool_use_id, "toolu-large-bash");
            assert_eq!(content.matches("line\n").count(), 5_900);
            metadata.clone()
        }
        other => panic!("expected tool result, got {other:?}"),
    }
    .expect("tool result metadata");
    let parsed: Value = serde_json::from_str(&metadata).expect("parse metadata");
    let progress = parsed
        .get("progressMessages")
        .and_then(Value::as_array)
        .expect("progress messages");
    let progress_uuids = progress
        .iter()
        .filter_map(|record| record.get("uuid").and_then(Value::as_str))
        .collect::<HashSet<_>>();
    assert_eq!(progress.len(), 4);
    assert_eq!(progress_uuids.len(), 4);
    assert_eq!(
        parsed
            .get("bash")
            .and_then(|bash| bash.get("outputTruncated"))
            .and_then(Value::as_bool),
        Some(true)
    );

    let assistant = &session.messages[2];
    assert!(
        assistant
            .content
            .contains("Stub tool result preview truncated")
    );
    assert!(
        assistant
            .content
            .contains("Bash output truncated for transcript safety")
    );
    assert!(
        assistant.content.len() < 2_000,
        "assistant echo should stay bounded, got {} bytes",
        assistant.content.len()
    );
    assert!(
        assistant.content.matches("line\n").count() < 100,
        "assistant echo should not replay the full Bash output"
    );
}
