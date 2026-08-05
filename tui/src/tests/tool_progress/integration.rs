use crate::tests::support::*;
use orbcode_app_server_client::AppClient;

#[test]
fn force_interrupt_active_turn_clears_loading_ui_and_keeps_partial_answer() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();
    state.pending_assistant = "partial answer".to_string();
    state.active_thinking = Some(ActiveThinkingState {
        text: "working".to_string(),
        is_streaming: true,
        completed_at: None,
    });
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: "{\"command\":\"sleep 10\"}".to_string(),
        status_line: "Running `Bash`".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });
    state.in_progress_tool_use_ids.insert("tool-1".to_string());

    state.force_interrupt_active_turn();

    assert!(!state.request_in_flight);
    assert!(state.request_started_at.is_none());
    assert!(state.pending_assistant.is_empty());
    assert!(state.active_thinking.is_none());
    assert!(state.live_tool_cells.is_empty());
    assert!(state.in_progress_tool_use_ids.is_empty());
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].content, "partial answer");
}

#[test]
fn force_interrupt_active_turn_marks_unresolved_live_tool_interrupted() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();
    state.messages.push(TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![TranscriptBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "Bash".to_string(),
            input: "{\"command\":\"sleep 10\"}".to_string(),
        }],
    ));
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: "{\"command\":\"sleep 10\"}".to_string(),
        status_line: "Running `Bash`".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });
    state.in_progress_tool_use_ids.insert("tool-1".to_string());

    state.force_interrupt_active_turn();

    assert!(!state.request_in_flight);
    assert!(state.live_tool_cells.is_empty());
    assert!(state.in_progress_tool_use_ids.is_empty());
    assert!(state.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    ..
                } if tool_use_id == "tool-1"
                    && content == INTERRUPTED_TOOL_RESULT
                    && *is_error
            )
        })
    }));

    let transcript = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(transcript.contains(INTERRUPTED_TOOL_RESULT), "{transcript}");
    assert!(!transcript.contains("Running…"), "{transcript}");
}

#[test]
fn interrupt_detaches_old_turn_event_stream() {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut turn_events = Some(rx);

    assert!(detach_turn_event_stream(&mut turn_events));
    assert!(turn_events.is_none());

    let late_send = tx.send(StreamEvent::ToolProgress {
        session_id: "session".to_string(),
        tool_use_id: "old-tool".to_string(),
        tool_name: "bash".to_string(),
        progress: serde_json::json!({"message": "late progress"}),
    });
    assert!(late_send.is_err());
}

#[tokio::test]
async fn tui_interrupt_smoke_allows_immediate_followup_without_stale_events() {
    let home_dir = test_temp_path("home");
    let cwd = test_temp_path("workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    tokio::fs::write(
        home_dir.join("settings.json"),
        serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "stub://anthropic",
                "ANTHROPIC_MODEL": "stub-model"
            }
        })
        .to_string(),
    )
    .await
    .expect("write settings");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            allow_tools: Some(true),
            env_overrides: orbcode_app_server::sealed_provider_env_overrides(),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let session_id = state.session_id.clone();
    let mut turn_events = Some(
        app_server
            .submit_turn_stream(&session_id, r#"#tool:bash {"command":"sleep 10"}"#)
            .await
            .expect("submit long bash turn"),
    );

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = turn_events
                .as_mut()
                .expect("first turn receiver")
                .recv()
                .await
                .expect("first turn event");
            let started = matches!(event, StreamEvent::ToolUseStarted { .. });
            state.apply_stream_event(event);
            if started {
                return;
            }
        }
    })
    .await
    .expect("bash tool should start");

    assert!(state.request_in_flight);
    assert!(!state.live_tool_cells.is_empty());

    state
        .interrupt_active_turn(&app_server, &mut turn_events)
        .await;

    assert!(turn_events.is_none());
    assert!(!state.request_in_flight);
    assert!(state.live_tool_cells.is_empty());
    assert!(state.in_progress_tool_use_ids.is_empty());
    assert_eq!(state.status_line, "Turn interrupted.");

    turn_events = Some(
        app_server
            .submit_turn_stream(&session_id, "new turn after tui interrupt")
            .await
            .expect("submit followup turn"),
    );

    let (saw_followup_user, followup_finished) =
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut saw_followup_user = false;
            loop {
                let event = turn_events
                    .as_mut()
                    .expect("followup turn receiver")
                    .recv()
                    .await
                    .expect("followup turn event");
                if matches!(
                    &event,
                    StreamEvent::UserMessage { message }
                        if message.content == "new turn after tui interrupt"
                ) {
                    saw_followup_user = true;
                }
                let finished = matches!(event, StreamEvent::TurnFinished { .. });
                state.apply_stream_event(event);
                if finished {
                    return (saw_followup_user, true);
                }
            }
        })
        .await
        .expect("followup turn should finish promptly");

    assert!(saw_followup_user);
    assert!(followup_finished);
    assert!(!state.request_in_flight);
    assert!(state.live_tool_cells.is_empty());

    let resumed = app_server
        .bootstrap(Some(&session_id))
        .await
        .expect("reload session");
    let resumed_messages = resumed
        .session
        .messages
        .iter()
        .map(|message| format!("{:?}: {}", message.role, message.content))
        .collect::<Vec<_>>();
    assert!(
        resumed
            .session
            .messages
            .iter()
            .any(|message| message.content == "new turn after tui interrupt"),
        "resumed messages: {resumed_messages:#?}"
    );
    assert!(
        !resumed.session.messages.iter().any(|message| {
            message.content == "[Request interrupted by user]"
                || message.content == "[Request interrupted by user for tool use]"
        }),
        "detached TUI interrupt should not persist stale interruption markers"
    );
}

#[tokio::test]
async fn tui_provider_interrupt_smoke_allows_immediate_followup() {
    let server = start_provider_interrupt_smoke_server();
    let base_url = server.base_url.clone();
    let accepted_requests = server.accepted.clone();
    let home_dir = test_temp_path("provider-home");
    let cwd = test_temp_path("provider-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    tokio::fs::write(
        home_dir.join("settings.json"),
        serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": base_url,
                "ANTHROPIC_API_KEY": "test-api-key",
                "ANTHROPIC_MODEL": "stub-model"
            }
        })
        .to_string(),
    )
    .await
    .expect("write settings");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            allow_tools: Some(true),
            env_overrides: orbcode_app_server::sealed_provider_env_overrides(),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let session_id = state.session_id.clone();
    let mut turn_events = Some(
        app_server
            .submit_turn_stream(&session_id, "provider wait before interrupt")
            .await
            .expect("submit provider wait turn"),
    );

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = turn_events
                .as_mut()
                .expect("first turn receiver")
                .recv()
                .await
                .expect("first turn event");
            let request_started = matches!(event, StreamEvent::RequestStarted { .. });
            state.apply_stream_event(event);
            if request_started {
                return;
            }
        }
    })
    .await
    .expect("first provider request should start");

    tokio::time::timeout(Duration::from_secs(2), async {
        while accepted_requests.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first provider request should reach the server");

    assert!(state.request_in_flight);
    state
        .interrupt_active_turn(&app_server, &mut turn_events)
        .await;

    assert!(turn_events.is_none());
    assert!(!state.request_in_flight);
    assert!(state.live_tool_cells.is_empty());
    assert_eq!(state.status_line, "Turn interrupted.");

    turn_events = Some(
        app_server
            .submit_turn_stream(&session_id, "new turn after provider interrupt")
            .await
            .expect("submit provider followup turn"),
    );

    let (saw_followup_user, saw_followup_answer, followup_finished) =
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut saw_followup_user = false;
            let mut saw_followup_answer = false;
            loop {
                let event = turn_events
                    .as_mut()
                    .expect("followup turn receiver")
                    .recv()
                    .await
                    .expect("followup turn event");
                match &event {
                    StreamEvent::UserMessage { message }
                        if message.content == "new turn after provider interrupt" =>
                    {
                        saw_followup_user = true;
                    }
                    StreamEvent::AssistantMessageCompleted { message, .. }
                        if message.content == "provider followup ok" =>
                    {
                        saw_followup_answer = true;
                    }
                    _ => {}
                }
                let finished = matches!(event, StreamEvent::TurnFinished { .. });
                state.apply_stream_event(event);
                if finished {
                    return (saw_followup_user, saw_followup_answer, true);
                }
            }
        })
        .await
        .expect("provider followup turn should finish promptly");

    assert!(saw_followup_user);
    assert!(saw_followup_answer);
    assert!(followup_finished);
    assert!(!state.request_in_flight);
    assert!(accepted_requests.load(Ordering::SeqCst) >= 2);
    server.shutdown_and_join();

    let resumed = app_server
        .bootstrap(Some(&session_id))
        .await
        .expect("reload session");
    assert!(
        resumed
            .session
            .messages
            .iter()
            .any(|message| message.content == "new turn after provider interrupt")
    );
    assert!(
        resumed
            .session
            .messages
            .iter()
            .any(|message| message.content == "provider followup ok")
    );
    assert!(
        !resumed
            .session
            .messages
            .iter()
            .any(|message| message.content == "[Request interrupted by user]"),
        "detached provider interrupt should not persist stale interruption markers"
    );
}

#[tokio::test]
async fn tui_large_bash_handoff_provider_snapshot_stays_bounded() {
    let home_dir = test_temp_path("large-bash-provider-home");
    let cwd = test_temp_path("large-bash-provider-workspace");
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    tokio::fs::write(
        home_dir.join("settings.json"),
        serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "stub://anthropic",
                "ANTHROPIC_MODEL": "stub-model"
            }
        })
        .to_string(),
    )
    .await
    .expect("write settings");

    let app_server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            allow_tools: Some(true),
            env_overrides: orbcode_app_server::sealed_provider_env_overrides(),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let app_server = Arc::new(AppClient::new(app_server).await.unwrap());
    let bootstrap = app_server.bootstrap(None).await.expect("bootstrap session");
    let mut state = TuiState::new(Some(Arc::clone(&app_server)), bootstrap);
    let session_id = state.session_id.clone();
    let prompt = format!(
        "#tool:bash {}",
        serde_json::json!({
            "command": "sh -c 'i=0; while [ \"$i\" -lt 20000 ]; do echo line; i=$((i+1)); done'"
        })
    );
    let mut turn_events = app_server
        .submit_turn_stream(&session_id, &prompt)
        .await
        .expect("submit large bash turn");

    let (saw_large_tool_result, saw_stub_followup) =
            tokio::time::timeout(Duration::from_secs(15), async {
                let mut saw_large_tool_result = false;
                let mut saw_stub_followup = false;
                while let Some(event) = turn_events.recv().await {
                    match &event {
                        StreamEvent::UserMessage { message } => {
                            saw_large_tool_result |= message.blocks.iter().any(|block| {
                                matches!(
                                    block,
                                    TranscriptBlock::ToolResult { content, .. }
                                        if content.contains("Bash output truncated for transcript safety")
                                            && content.contains("Omitted ")
                                )
                            });
                        }
                        StreamEvent::AssistantMessageCompleted { message, .. } => {
                            saw_stub_followup |= message.content.contains("Tool `bash` completed.");
                        }
                        _ => {}
                    }
                    let finished = matches!(event, StreamEvent::TurnFinished { .. });
                    state.apply_stream_event(event);
                    if finished {
                        return (saw_large_tool_result, saw_stub_followup);
                    }
                }
                (saw_large_tool_result, saw_stub_followup)
            })
            .await
            .expect("large bash turn should finish");

    assert!(saw_large_tool_result);
    assert!(saw_stub_followup);
    assert!(!state.request_in_flight);

    let snapshot = app_server
        .app_server()
        .unwrap()
        .last_provider_request_snapshot()
        .await
        .expect("last provider request snapshot");
    assert_eq!(snapshot.provider, ProviderId::Anthropic);
    assert_eq!(snapshot.source, "turn");
    assert_eq!(snapshot.session_id, session_id);
    let body = serde_json::from_str::<Value>(&snapshot.body_json).expect("provider body json");
    let tool_result = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .flat_map(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .find(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .expect("tool result block");
    let content = tool_result["content"]
        .as_str()
        .expect("tool result content should serialize as text");

    assert!(content.contains("Orb Code truncated an oversized earlier tool result"));
    assert!(content.contains("Bash output truncated for transcript safety"));
    assert!(content.contains("Omitted "));
    assert!(content.chars().count() < 9_000, "{content}");
    assert!(content.matches("line\n").count() < 2_000, "{content}");
    let tail = content
        .split("middle characters.]\n\n")
        .nth(1)
        .expect("provider preview tail");
    assert!(tail.starts_with("line\n"), "{tail}");
    assert!(!tail.lines().any(|line| line == "e"), "{tail}");
    assert!(!snapshot.body_json.contains("progressMessages"));
    assert!(!snapshot.body_json.contains("bash_progress"));
    assert!(snapshot.recent_activity_json.len() < 40_000);

    let body_section = render_provider_request_body_section(&snapshot);
    assert!(body_section.contains("● Provider request body"));
    assert!(body_section.contains("provider: anthropic"));
    assert!(body_section.contains("source: turn"));
    assert!(body_section.contains("model: stub-model"));
    assert!(body_section.contains("Bash output truncated for transcript safety"));
    assert!(body_section.contains("Orb Code truncated an oversized earlier tool result"));
    assert!(!body_section.contains("progressMessages"));
    assert!(!body_section.contains("bash_progress"));
    assert!(
        body_section.chars().count() < LAST_REQUEST_BODY_PREVIEW_CHARS + 1_000,
        "{body_section}"
    );
}

#[test]
fn request_started_restarts_waiting_animation_for_followup_rounds() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.spinner_frame = 4;
    state.request_count = 2;
    state.request_started_at = None;
    state.streamed_response_chars = 4_936;
    state.current_turn_total_tokens = 123;

    let finished = state.apply_stream_event(StreamEvent::RequestStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_provider: None,
        context: TurnContext {
            cwd: "/tmp".to_string(),
            current_date: "2025-01-01".to_string(),
            ..Default::default()
        },
    });

    assert!(!finished);
    assert!(state.request_in_flight);
    assert_eq!(state.spinner_frame, 0);
    assert!(state.request_started_at.is_some());
    assert_eq!(state.streamed_response_chars, 4_936);
    assert_eq!(state.request_token_direction, RequestTokenDirection::Up);
    assert_eq!(state.current_turn_total_tokens, 123);
    let rendered = plain_text_lines(&state.request_status_lines()).join("\n");
    assert!(rendered.contains("↑ 1.2k tokens"), "{rendered}");
}

#[test]
fn request_started_for_new_turn_resets_accumulated_turn_tokens() {
    let mut state = normal_state("", 0);
    state.current_turn_total_tokens = 123;
    state.streamed_response_chars = 4_936;

    let finished = state.apply_stream_event(StreamEvent::RequestStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_provider: None,
        context: TurnContext::default(),
    });

    assert!(!finished);
    assert_eq!(state.current_turn_total_tokens, 0);
    assert_eq!(state.streamed_response_chars, 0);
}

#[test]
fn request_started_renders_up_without_context_token_estimate() {
    let mut state = normal_state("", 0);

    assert!(!state.apply_stream_event(StreamEvent::UserMessage {
        message: TranscriptMessage::new(
            MessageRole::User,
            "Please analyze the current workspace and explain the request status token display.",
        ),
    }));
    let finished = state.apply_stream_event(StreamEvent::RequestStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_provider: None,
        context: TurnContext {
            cwd: "/tmp".to_string(),
            current_date: "2025-01-01".to_string(),
            ..Default::default()
        },
    });

    assert!(!finished);
    assert_eq!(state.request_token_direction, RequestTokenDirection::Up);
    let rendered = plain_text_lines(&state.request_status_lines()).join("\n");
    assert!(rendered.contains("↑ 0 tokens"), "{rendered}");
}

#[test]
fn assistant_delta_marks_request_token_direction_down() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();

    let finished = state.apply_stream_event(StreamEvent::AssistantDelta {
        session_id: "session".to_string(),
        delta: "hello".to_string(),
    });

    assert!(!finished);
    assert_eq!(state.request_token_direction, RequestTokenDirection::Down);
    assert_eq!(state.streamed_response_chars, 5);
}

#[test]
fn assistant_completed_usage_updates_request_token_estimate() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();
    state.streamed_response_chars = 208;

    let finished = state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "agent-tool".to_string(),
                name: "Agent".to_string(),
                input: "{\"description\":\"Explore repo\"}".to_string(),
            }],
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage {
            output_tokens: 760,
            total_tokens: 760,
            ..TokenUsage::default()
        },
    });

    assert!(!finished);
    assert_eq!(state.request_token_direction, RequestTokenDirection::Down);
    assert_eq!(state.streamed_response_chars, 3_040);
    let rendered = plain_text_lines(&state.request_status_lines()).join("\n");
    assert!(rendered.contains("↓ 760 tokens"), "{rendered}");
}

#[test]
fn agent_assistant_progress_advances_request_token_estimate() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();
    state.streamed_response_chars = 208;

    let finished = state.apply_stream_event(StreamEvent::ToolProgress {
        session_id: "session".to_string(),
        tool_use_id: "agent-tool".to_string(),
        tool_name: "Agent".to_string(),
        progress: serde_json::json!({
            "data": {
                "type": "agent_progress",
                "status": "Running Agent",
                "message": {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
                            {
                                "type": "text",
                                "text": "child agent produced additional analysis that should move the token estimate beyond the stale parent value"
                            }
                        ]
                    }
                }
            }
        }),
    });

    assert!(!finished);
    assert_eq!(state.request_token_direction, RequestTokenDirection::Down);
    assert!(state.streamed_response_chars > 208);
    let rendered = plain_text_lines(&state.request_status_lines()).join("\n");
    assert!(!rendered.contains("↓ 52 tokens"), "{rendered}");
}

#[test]
fn active_stream_events_mark_request_token_direction_down() {
    fn assert_down_after(event: StreamEvent) {
        let mut state = normal_state("", 0);
        state.begin_waiting_animation();

        let finished = state.apply_stream_event(event);

        assert!(!finished);
        assert_eq!(state.request_token_direction, RequestTokenDirection::Down);
    }

    assert_down_after(StreamEvent::AssistantMessageStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_from: None,
    });
    assert_down_after(StreamEvent::ThinkingStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
    });
    assert_down_after(StreamEvent::ThinkingDelta {
        session_id: "session".to_string(),
        delta: "thinking".to_string(),
    });
    assert_down_after(StreamEvent::AssistantDelta {
        session_id: "session".to_string(),
        delta: "answer".to_string(),
    });
    assert_down_after(StreamEvent::PermissionRequested {
        request: PermissionRequest {
            request_id: "request-1".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "tool-1".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: "{\"command\":\"echo hi\"}".to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        },
    });
    assert_down_after(StreamEvent::PermissionResolved {
        session_id: "session".to_string(),
        request_id: "request-1".to_string(),
        kind: orbcode_protocol::PermissionResolutionKind::Approved,
    });
    assert_down_after(StreamEvent::ToolUseStarted {
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: String::new(),
    });
    assert_down_after(StreamEvent::ToolProgress {
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        progress: serde_json::json!({"stdout": "hi"}),
    });
    assert_down_after(StreamEvent::ToolUseCompleted {
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        kind: orbcode_protocol::ToolUseCompletionKind::Success,
    });
}

#[test]
fn turn_finished_appends_turn_duration_note_to_transcript() {
    let mut state = normal_state("", 0);
    state.request_count = 1;
    state.messages.push(TranscriptMessage::new(
        MessageRole::Assistant,
        "Finished analysis.".to_string(),
    ));
    state.begin_waiting_animation();
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(34_000));

    let finished = state.apply_stream_event(StreamEvent::TurnFinished {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    assert!(finished);
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

    assert!(rendered.iter().any(|line| line.contains("Thought for 34s")));
    assert_eq!(rendered[rendered.len() - 2], "");
    assert!(
        rendered
            .last()
            .is_some_and(|line| line.starts_with("✻ Thought for ")),
        "{rendered:#?}"
    );
}

#[test]
fn turn_finished_status_uses_accumulated_turn_tokens() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "Bash".to_string(),
                input: "{\"command\":\"echo hi\"}".to_string(),
            }],
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage {
            input_tokens: 8,
            output_tokens: 2,
            total_tokens: 10,
            ..TokenUsage::default()
        },
    });
    state.apply_stream_event(StreamEvent::RequestStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_provider: None,
        context: TurnContext::default(),
    });
    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, "done"),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage {
            input_tokens: 15,
            output_tokens: 5,
            total_tokens: 20,
            ..TokenUsage::default()
        },
    });

    let finished = state.apply_stream_event(StreamEvent::TurnFinished {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage {
            input_tokens: 15,
            output_tokens: 5,
            total_tokens: 20,
            ..TokenUsage::default()
        },
    });

    assert!(finished);
    let rendered = plain_text_lines(&state.transcript_lines(80));
    assert!(
        rendered.iter().any(|line| line.contains("30 tokens")),
        "turn duration note should include accumulated token total: {rendered:#?}"
    );
}

#[test]
fn turn_finished_status_includes_agent_tool_result_tokens() {
    let mut state = normal_state("", 0);
    state.begin_waiting_animation();

    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "agent-tool".to_string(),
                name: "Agent".to_string(),
                input: "{\"description\":\"Explore repo\"}".to_string(),
            }],
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage {
            input_tokens: 80,
            output_tokens: 20,
            total_tokens: 100,
            ..TokenUsage::default()
        },
    });
    state.apply_stream_event(StreamEvent::UserMessage {
        message: TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "agent-tool".to_string(),
                content: "agent result".to_string().into(),
                is_error: false,
                metadata: Some(serde_json::json!({"totalTokens": 900}).to_string()),
            }],
        ),
    });
    state.apply_stream_event(StreamEvent::RequestStarted {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_provider: None,
        context: TurnContext::default(),
    });
    state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(MessageRole::Assistant, "done"),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage {
            input_tokens: 40,
            output_tokens: 10,
            total_tokens: 50,
            ..TokenUsage::default()
        },
    });

    let finished = state.apply_stream_event(StreamEvent::TurnFinished {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage {
            input_tokens: 40,
            output_tokens: 10,
            total_tokens: 50,
            ..TokenUsage::default()
        },
    });

    assert!(finished);
    let rendered = plain_text_lines(&state.transcript_lines(80));
    assert!(
        rendered.iter().any(|line| line.contains("1.1k tokens")),
        "turn duration note should include agent sub-turn tokens: {rendered:#?}"
    );
}

#[test]
fn context_compacted_event_appends_local_compact_note() {
    let mut state = normal_state("", 0);

    let finished = state.apply_stream_event(StreamEvent::ContextCompacted {
        session_id: "session".to_string(),
        duration_ms: 2_500,
        summary: Some("Summary:\n- Old history was compacted.".to_string()),
        original_message_count: 3,
        compacted_message_count: 2,
        provider_generated: true,
        fallback_reason: None,
    });

    assert!(!finished);
    assert_eq!(state.status_line, "Conversation compacted automatically.");

    let collapsed = plain_text_lines(&state.transcript_lines(80)).join("\n");
    assert!(collapsed.contains("Conversation compacted"), "{collapsed}");

    let note = state
        .messages
        .iter()
        .find_map(parse_local_transcript_note)
        .expect("context compacted note");
    let expanded = plain_text_lines(&render_local_transcript_note_lines(note, 80, true)).join("\n");
    assert!(expanded.contains("Crunched for 3s"), "{expanded}");
    assert!(expanded.contains("Old history was compacted"), "{expanded}");
}

#[test]
fn error_event_without_provider_renders_as_red_local_error() {
    let mut state = normal_state("", 0);

    let finished = state.apply_stream_event(StreamEvent::Error {
        session_id: Some("session".to_string()),
        provider: None,
        category: None,
        message: "UserPromptSubmit operation blocked by hook:\n[hook]: blocked".to_string(),
        suggestion: None,
    });

    assert!(finished);
    assert_eq!(
        state.status_line,
        "UserPromptSubmit operation blocked by hook:\n[hook]: blocked"
    );
    let lines = state.transcript_lines(80);
    let rendered = plain_text_lines(&lines).join("\n");

    assert!(rendered.contains("UserPromptSubmit operation blocked by hook:"));
    assert!(rendered.contains("[hook]: blocked"));
    assert!(!rendered.contains("anthropic:"), "{rendered}");
    let first_error_line = lines
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref() == "UserPromptSubmit operation blocked by hook:")
        })
        .expect("rendered error line");
    assert_eq!(
        first_error_line.spans[0].style,
        Style::default().fg(active_palette().error)
    );
    assert_eq!(
        first_error_line.spans[2].style,
        Style::default().fg(active_palette().error)
    );
}

#[test]
fn turn_finished_shows_context_warning_status() {
    let mut state = normal_state("", 0);
    state.model_display_name = "gpt-4o(openai)".to_string();
    let usage = TokenUsage {
        input_tokens: 175_000,
        total_tokens: 175_000,
        ..TokenUsage::default()
    };

    let finished = state.apply_stream_event(StreamEvent::TurnFinished {
        session_id: "session".to_string(),
        provider: ProviderId::OpenAi,
        fallback_from: None,
        usage,
    });

    assert!(finished);
    assert!(
        state
            .status_line
            .starts_with("Auto-compact recommended: 175000 tokens"),
        "{}",
        state.status_line
    );
    assert!(state.status_line_should_persist());
}

#[test]
fn turn_finished_context_warning_uses_bootstrap_token_options() {
    let mut state = normal_state("", 0);
    state.model_display_name = "glm-4.7".to_string();
    state.context_window_options = ContextWindowOptions {
        auto_compact_window_override: Some(100),
        ..Default::default()
    };
    state.max_output_token_options = MaxOutputTokenOptions {
        max_output_tokens_override: Some(1),
    };
    state.token_warning_options = TokenWarningOptions {
        auto_compact_enabled: false,
        ..Default::default()
    };
    let usage = TokenUsage {
        input_tokens: 96,
        total_tokens: 96,
        ..TokenUsage::default()
    };

    let finished = state.apply_stream_event(StreamEvent::TurnFinished {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage,
    });

    assert!(finished);
    assert!(
        state
            .status_line
            .starts_with("Context limit reached: 96 tokens, 3% left"),
        "{}",
        state.status_line
    );
}

#[test]
fn turn_finished_commits_assistant_message_before_duration_note() {
    let mut state = normal_state("", 0);
    state.request_count = 1;
    state.begin_waiting_animation();
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(34_000));
    state.pending_assistant = "Final streamed answer".to_string();

    let finished = state.apply_stream_event(StreamEvent::AssistantMessageCompleted {
        message: TranscriptMessage::new(
            MessageRole::Assistant,
            "Final streamed answer".to_string(),
        ),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });
    assert!(!finished);
    assert!(state.pending_assistant.is_empty());
    assert_eq!(state.messages.len(), 1);

    let finished = state.apply_stream_event(StreamEvent::TurnFinished {
        session_id: "session".to_string(),
        provider: ProviderId::Anthropic,
        fallback_from: None,
        usage: TokenUsage::default(),
    });

    assert!(finished);
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
            .any(|line| line.contains("Final streamed answer"))
    );
    assert_eq!(rendered[rendered.len() - 2], "");
    assert!(
        rendered
            .last()
            .is_some_and(|line| line.starts_with("✻ Thought for ")),
        "{rendered:#?}"
    );
}

#[test]
fn transcript_lines_do_not_render_waiting_tip_during_active_tool_use() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(5_000));
    state.request_token_direction = RequestTokenDirection::Down;
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: "{\"command\":\"ls\"}".to_string(),
        status_line: "Waiting for permission".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
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
        !rendered
            .iter()
            .any(|line| line.contains("Tip: Press Ctrl+O to expand tool details."))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Waiting for permission"))
    );
    assert_eq!(
        plain_text_lines(&state.request_status_lines()),
        vec!["· Waiting for permission...(5s · ↓ 0 tokens)".to_string()]
    );
    let footer_text = state.footer_right_text();
    assert!(
        !footer_text.contains("Waiting for permission"),
        "footer should not show tool status when request_in_flight (shown in request_status panel), got: {footer_text}"
    );
}

#[test]
fn transcript_lines_show_thinking_and_tool_activity_together() {
    let mut state = normal_state("", 0);
    state.request_in_flight = true;
    state.spinner_frame = 3;
    state.spinner_verb_index = 0;
    state.request_count = 1;
    state.request_started_at = Some(Instant::now() - std::time::Duration::from_millis(5_000));
    state.active_thinking = Some(ActiveThinkingState {
        text: "draft reply".to_string(),
        is_streaming: true,
        completed_at: None,
    });
    state.upsert_live_tool_activity(LiveToolActivity {
        request_id: None,
        tool_use_id: "tool-1".to_string(),
        tool_name: "Grep".to_string(),
        tool_input: "{\"pattern\":\"alpha\"}".to_string(),
        status_line: "Searching for 1 pattern".to_string(),
        progress_messages: Vec::new(),
        is_error: false,
    });
    state.in_progress_tool_use_ids.insert("tool-1".to_string());

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
            .any(|line| line.contains("Pontificating… (thinking)"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Searching for 1 pattern"))
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("Combobulating...(5s)"))
    );
    assert!(
        plain_text_lines(&state.request_status_lines())
            .iter()
            .any(|line| line.contains("Searching for 1 pattern...(5s · "))
    );
}
