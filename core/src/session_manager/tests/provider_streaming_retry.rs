use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};
use tempfile::tempdir;

use super::support::*;
use super::*;

fn set_anthropic_server_env(manager: &mut SessionManager, base_url: String) {
    manager
        .config
        .settings
        .env
        .insert("ANTHROPIC_BASE_URL".to_string(), base_url);
    manager
        .config
        .settings
        .env
        .insert("ANTHROPIC_API_KEY".to_string(), "test-api-key".to_string());
}

fn set_openai_server_env(manager: &mut SessionManager, base_url: String) {
    manager
        .config
        .settings
        .env
        .insert("OPENAI_BASE_URL".to_string(), base_url);
    manager
        .config
        .settings
        .env
        .insert("OPENAI_MODEL".to_string(), "gpt-4o".to_string());
}

#[tokio::test]
async fn provider_interrupted_error_surfaces_as_turn_cancellation_without_fallback_or_error() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        fallback_provider: Some(ProviderId::OpenAi),
        max_retries: Some(3),
        ..AppConfigOverrides::default()
    })
    .await;
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=interrupted".to_string(),
    );
    manager.config.settings.env.insert(
        "OPENAI_BASE_URL".to_string(),
        "mock://openai?scenario=fatal".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let mut rx = manager
        .submit_turn(&session.session_id, "interrupt at provider boundary")
        .await
        .expect("submit turn");

    let mut cancellation = None;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TurnCancelled { kind, partial, .. } => {
                cancellation = Some((kind, partial));
                break;
            }
            StreamEvent::Error {
                message,
                suggestion,
                ..
            } => panic!(
                "provider interruption must not surface as an error: {message}; {suggestion:?}"
            ),
            StreamEvent::AssistantMessageStarted {
                provider: ProviderId::OpenAi,
                ..
            } => panic!("provider interruption must not trigger fallback"),
            _ => {}
        }
    }

    assert_eq!(
        cancellation,
        Some((TurnCancellationKind::BeforeResponse, None))
    );
}

#[tokio::test]
async fn provider_interrupted_after_text_preserves_partial_and_cancels() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=interrupt_after_text".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let mut rx = manager
        .submit_turn(&session.session_id, "interrupt after partial text")
        .await
        .expect("submit turn");

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TurnCancelled { kind, partial, .. } => {
                assert_eq!(kind, TurnCancellationKind::AssistantStreaming);
                assert!(
                    partial
                        .expect("partial assistant message")
                        .content
                        .contains("partial text before provider interruption")
                );
                return;
            }
            StreamEvent::Error {
                message,
                suggestion,
                ..
            } => panic!("interruption surfaced as error: {message}; {suggestion:?}"),
            _ => {}
        }
    }
    panic!("stream ended without cancellation");
}

#[tokio::test]
async fn fallback_interruption_stays_cancellation() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        fallback_provider: Some(ProviderId::OpenAi),
        max_retries: Some(0),
        ..AppConfigOverrides::default()
    })
    .await;
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=retryable".to_string(),
    );
    manager.config.settings.env.insert(
        "OPENAI_BASE_URL".to_string(),
        "mock://openai?scenario=interrupted".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let mut rx = manager
        .submit_turn(&session.session_id, "interrupt during fallback")
        .await
        .expect("submit turn");

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TurnCancelled { .. } => return,
            StreamEvent::Error {
                message,
                suggestion,
                ..
            } => panic!("fallback interruption surfaced as error: {message}; {suggestion:?}"),
            _ => {}
        }
    }
    panic!("stream ended without cancellation");
}

#[tokio::test]
async fn cancellation_during_retry_backoff_stays_cancellation() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        max_retries: Some(3),
        ..AppConfigOverrides::default()
    })
    .await;
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=ratelimit".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "interrupt retry backoff")
        .await
        .expect("submit turn");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(manager.interrupt_turn(&session_id).await);

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TurnCancelled { .. } => return,
            StreamEvent::Error {
                message,
                suggestion,
                ..
            } => panic!("backoff interruption surfaced as error: {message}; {suggestion:?}"),
            _ => {}
        }
    }
    panic!("stream ended without cancellation");
}

#[tokio::test]
async fn falls_back_when_primary_provider_retries_out() {
    let mut manager = test_manager().await;
    // Drive the primary failure from provider config (a `mock://` base URL),
    // not a prompt marker: the mock Anthropic provider returns a retryable
    // error on every attempt, so the turn exhausts retries and falls back to
    // the stub OpenAI provider.
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=retryable".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "use fallback")
        .await
        .expect("submit turn");

    let mut saw_started_fallback_from = None;
    let mut saw_completed_fallback_from = None;
    let mut saw_openai_finish = false;
    let mut saw_fallback_from = None;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::AssistantMessageStarted { fallback_from, .. } => {
                saw_started_fallback_from = fallback_from;
            }
            StreamEvent::AssistantMessageCompleted { fallback_from, .. } => {
                saw_completed_fallback_from = fallback_from;
            }
            StreamEvent::TurnFinished {
                provider,
                fallback_from,
                ..
            } => {
                saw_openai_finish = provider == ProviderId::OpenAi;
                saw_fallback_from = fallback_from;
                break;
            }
            _ => {}
        }
    }

    assert_eq!(saw_started_fallback_from, Some(ProviderId::Anthropic));
    assert_eq!(saw_completed_fallback_from, Some(ProviderId::Anthropic));
    assert!(saw_openai_finish);
    assert_eq!(saw_fallback_from, Some(ProviderId::Anthropic));
}

#[tokio::test]
async fn honors_retry_after_header_seconds_before_retrying() {
    // The server directs a 1-second wait via `Retry-After` on the first
    // `/v1/messages` attempt, then succeeds. Because the test config collapses
    // the exponential base backoff to 0ms (`CLAUDE_CODE_RETRY_BASE_DELAY_MS=0`),
    // any measured gap between the two attempts can only come from honoring the
    // header verbatim. Count-tokens preflight is answered separately by the
    // server so it never enters this timing window.
    let (base_url, instant_rx, server_handle) = start_retry_after_anthropic_server(1);
    let mut manager = test_manager().await;
    set_anthropic_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "trigger retry-after backoff")
        .await
        .expect("submit turn");

    let finished_ok = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Error { message, .. } => {
                    panic!("turn errored instead of recovering after retry: {message}");
                }
                StreamEvent::TurnFinished { .. } => return true,
                _ => {}
            }
        }
        false
    })
    .await
    .expect("turn should finish within the timeout");
    assert!(finished_ok, "turn stream closed without TurnFinished");

    let _ = server_handle.join();

    let first = instant_rx
        .recv()
        .expect("first /v1/messages attempt instant");
    let second = instant_rx
        .recv()
        .expect("second /v1/messages attempt instant");
    assert!(
        instant_rx.try_recv().is_err(),
        "exactly two /v1/messages attempts expected (retry succeeded on the second)"
    );

    let gap = second.duration_since(first);
    assert!(
        gap >= Duration::from_millis(950),
        "retry waited {gap:?}; expected >= ~1s from the Retry-After directive (base backoff is 0)"
    );
    assert!(
        gap < Duration::from_millis(3000),
        "retry waited {gap:?}; far longer than the 1s Retry-After directive"
    );
}

#[tokio::test]
async fn stream_error_after_content_falls_back_with_discarded_primary_tombstone() {
    let (base_url, server_handle) = start_error_after_content_anthropic_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        fallback_provider: Some(ProviderId::OpenAi),
        max_retries: Some(0),
        ..AppConfigOverrides::default()
    })
    .await;
    set_anthropic_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "stream error after content")
        .await
        .expect("submit turn");

    let (saw_delta, saw_tombstone, saw_fallback_started, completed, turn_finished) =
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut saw_delta = false;
            let mut saw_tombstone = false;
            let mut saw_fallback_started = false;
            let mut completed = None;
            while let Some(event) = rx.recv().await {
                match event {
                    StreamEvent::AssistantDelta { delta, .. } if delta == "before error" => {
                        saw_delta = true;
                    }
                    StreamEvent::AssistantMessageDiscarded {
                        provider,
                        fallback_provider,
                        reason,
                        ..
                    } => {
                        saw_tombstone = provider == ProviderId::Anthropic
                            && fallback_provider == ProviderId::OpenAi
                            && reason.contains("server overloaded after content");
                    }
                    StreamEvent::AssistantMessageStarted {
                        provider: ProviderId::OpenAi,
                        fallback_from,
                        ..
                    } => {
                        saw_fallback_started = fallback_from == Some(ProviderId::Anthropic);
                    }
                    StreamEvent::AssistantMessageCompleted {
                        message,
                        provider,
                        fallback_from,
                        ..
                    } => {
                        completed = Some((message, provider, fallback_from));
                    }
                    StreamEvent::Error { message, .. } => {
                        panic!(
                            "fallback should finish instead of surfacing stream error: {message}"
                        );
                    }
                    StreamEvent::TurnFinished {
                        provider,
                        fallback_from,
                        ..
                    } => {
                        return (
                            saw_delta,
                            saw_tombstone,
                            saw_fallback_started,
                            completed,
                            Some((provider, fallback_from)),
                        );
                    }
                    _ => {}
                }
            }
            (
                saw_delta,
                saw_tombstone,
                saw_fallback_started,
                completed,
                None,
            )
        })
        .await
        .expect("stream error fallback should finish promptly");

    server_handle.join().expect("stream error server joins");
    assert!(saw_delta);
    assert!(saw_tombstone);
    assert!(saw_fallback_started);
    let (completed_message, completed_provider, completed_fallback_from) =
        completed.expect("fallback assistant completion");
    assert_eq!(completed_provider, ProviderId::OpenAi);
    assert_eq!(completed_fallback_from, Some(ProviderId::Anthropic));
    assert!(
        completed_message
            .content
            .contains("OpenAI-compatible phase 2 response")
    );
    assert!(!completed_message.content.contains("before error"));
    assert_eq!(
        turn_finished,
        Some((ProviderId::OpenAi, Some(ProviderId::Anthropic)))
    );

    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    let assistant_messages = saved
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .collect::<Vec<_>>();
    assert_eq!(assistant_messages.len(), 1);
    assert_eq!(assistant_messages[0].content, completed_message.content);
    assert!(!assistant_messages[0].content.contains("before error"));

    let next_request = manager
        .provider_request_for_session(
            &session_id,
            "next provider-visible request",
            manager.context_preview().await,
            &[],
            true,
            true,
        )
        .await
        .expect("build next provider request");
    assert!(
        next_request
            .messages
            .iter()
            .all(|message| !message.content.contains("before error")),
        "failed primary partial assistant text must not be provider-visible"
    );
    assert!(
        next_request.messages.iter().all(|message| {
            !message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolUse { .. } | TranscriptBlock::ToolResult { .. }
                )
            })
        }),
        "failed primary attempt must not leave assistant/tool blocks for the next provider"
    );

    // The discarded primary attempt consumed tokens ("before error" was
    // streamed). Its usage must be preserved in LiveCostStore so
    // maxBudgetUsd enforcement counts the real provider spend even though
    // the partial response was thrown away.
    let (total_cost, _) = manager.live_cost_total(&session_id).await;
    assert!(
        total_cost > 0.0,
        "live cost must include both the discarded primary and the successful fallback"
    );
}

#[tokio::test]
async fn stream_error_after_thinking_before_tool_execution_still_falls_back() {
    let (base_url, server_handle) = start_error_after_thinking_anthropic_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        fallback_provider: Some(ProviderId::OpenAi),
        max_retries: Some(0),
        ..AppConfigOverrides::default()
    })
    .await;
    set_anthropic_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let mut rx = manager
        .submit_turn(&session.session_id, "stream error after thinking")
        .await
        .expect("submit turn");

    let mut saw_thinking = false;
    let mut saw_discard = false;
    let mut fallback_finish = None;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::ThinkingDelta { delta, .. } => {
                saw_thinking |= delta == "considering fallback";
            }
            StreamEvent::AssistantMessageDiscarded { reason, .. } => {
                saw_discard |= reason.contains("server overloaded after thinking");
            }
            StreamEvent::Error { message, .. } => {
                panic!("pre-effect thinking failure should fall back: {message}")
            }
            StreamEvent::TurnFinished {
                provider,
                fallback_from,
                ..
            } => {
                fallback_finish = Some((provider, fallback_from));
                break;
            }
            _ => {}
        }
    }

    server_handle.join().expect("thinking error server joins");
    assert!(saw_thinking);
    assert!(saw_discard);
    assert_eq!(
        fallback_finish,
        Some((ProviderId::OpenAi, Some(ProviderId::Anthropic)))
    );
}

#[tokio::test]
async fn stream_error_after_streamed_tool_use_suppresses_fallback_after_completed_side_effect() {
    let temp = tempdir().expect("tempdir");
    let marker_path = temp.path().join("provider-effect-marker");
    let command = format!("printf 'primary\\n' >> \"{}\"", marker_path.display());
    let (base_url, server_handle) =
        start_error_after_tool_use_anthropic_server(command, marker_path.clone());
    let (fallback_base_url, fallback_requests, fallback_shutdown, fallback_handle) =
        start_recording_openai_error_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        fallback_provider: Some(ProviderId::OpenAi),
        max_retries: Some(0),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    set_anthropic_server_env(&mut manager, base_url);
    set_openai_server_env(&mut manager, fallback_base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .set_session_permission_preset(&session_id, ModelPermissionPreset::FullAccess)
        .await
        .expect("enable full access for external side-effect test");
    let mut rx = manager
        .submit_turn(&session_id, "stream error after streamed tool")
        .await
        .expect("submit turn");

    let (started_index, terminal_events, tombstone_index, saw_tool_result, error_message) =
        tokio::time::timeout(StdDuration::from_secs(3), async {
            let mut index = 0_usize;
            let mut started_index = None;
            let mut terminal_events = Vec::new();
            let mut tombstone_index = None;
            let mut saw_tool_result = false;

            while let Some(event) = rx.recv().await {
                match event {
                    StreamEvent::ToolUseStarted { tool_use_id, .. }
                        if tool_use_id == "tool-stream-error" =>
                    {
                        started_index = Some(index);
                    }
                    StreamEvent::ToolUseCompleted {
                        tool_use_id, kind, ..
                    } if tool_use_id == "tool-stream-error" => {
                        terminal_events.push((index, kind));
                    }
                    StreamEvent::UserMessage { message } => {
                        saw_tool_result |= message.blocks.iter().any(|block| {
                            matches!(
                                block,
                                TranscriptBlock::ToolResult {
                                    tool_use_id,
                                    ..
                                } if tool_use_id == "tool-stream-error"
                            )
                        });
                    }
                    StreamEvent::AssistantMessageDiscarded {
                        provider,
                        fallback_provider,
                        reason,
                        ..
                    } => {
                        if provider == ProviderId::Anthropic
                            && fallback_provider == ProviderId::OpenAi
                            && reason.contains("server overloaded after tool use")
                        {
                            tombstone_index = Some(index);
                        }
                    }
                    StreamEvent::Error { message, .. } => {
                        return (
                            started_index,
                            terminal_events,
                            tombstone_index,
                            saw_tool_result,
                            Some(message),
                        );
                    }
                    StreamEvent::TurnFinished { .. } => panic!(
                        "a fallback turn must not finish after streamed tool execution started"
                    ),
                    _ => {}
                }
                index += 1;
            }

            (
                started_index,
                terminal_events,
                tombstone_index,
                saw_tool_result,
                None,
            )
        })
        .await
        .expect("post-effect provider failure should surface promptly");

    server_handle
        .join()
        .expect("tool stream error server joins");
    let _ = fallback_shutdown.send(());
    fallback_handle.join().expect("fallback recorder joins");
    let started_index = started_index.expect("streamed tool started");
    let tombstone_index = tombstone_index.expect("discarded primary tombstone emitted");
    assert_eq!(
        terminal_events.len(),
        1,
        "tool must have one terminal event"
    );
    let (terminal_index, terminal_kind) = terminal_events[0];
    assert_eq!(terminal_kind, ToolUseCompletionKind::Interrupted);
    assert!(started_index < terminal_index);
    assert!(terminal_index < tombstone_index);
    assert!(!saw_tool_result);
    assert_eq!(
        fallback_requests.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "fallback provider must not be contacted after the effect barrier"
    );
    assert!(
        error_message
            .expect("provider error")
            .contains("tool may already have produced side effects"),
        "the surfaced error must explain why fallback was suppressed"
    );
    assert_eq!(
        std::fs::read_to_string(&marker_path).expect("read side-effect marker"),
        "primary\n",
        "the completed external side effect must occur exactly once"
    );

    let next_request = manager
        .provider_request_for_session(
            &session_id,
            "next provider-visible request",
            manager.context_preview().await,
            &[],
            true,
            true,
        )
        .await
        .expect("build next provider request");
    assert!(
        next_request.messages.iter().all(|message| {
            !message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolUse { id, .. } if id == "tool-stream-error"
                )
            })
        }),
        "discarded primary tool_use must not be provider-visible"
    );
    assert!(
        next_request.messages.iter().all(|message| {
            !message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult {
                        tool_use_id,
                        ..
                    } if tool_use_id == "tool-stream-error"
                )
            })
        }),
        "discarded primary tool_result must not be provider-visible"
    );

    let (total_cost, _) = manager.live_cost_total(&session_id).await;
    assert!(
        total_cost > 0.0,
        "failed primary usage must remain accounted after fallback suppression"
    );
}

#[tokio::test]
async fn stream_error_after_started_slow_tool_interrupts_and_drains_without_fallback() {
    let temp = tempdir().expect("tempdir");
    let marker_path = temp.path().join("slow-tool-started");
    let command = format!(
        "printf 'started\\n' > \"{}\"; sleep 30",
        marker_path.display()
    );
    let (base_url, server_handle) =
        start_error_after_tool_use_anthropic_server(command, marker_path.clone());
    let (fallback_base_url, fallback_requests, fallback_shutdown, fallback_handle) =
        start_recording_openai_error_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        fallback_provider: Some(ProviderId::OpenAi),
        max_retries: Some(0),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    set_anthropic_server_env(&mut manager, base_url);
    set_openai_server_env(&mut manager, fallback_base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    manager
        .set_session_permission_preset(&session.session_id, ModelPermissionPreset::FullAccess)
        .await
        .expect("enable full access for external side-effect test");
    let mut rx = manager
        .submit_turn(&session.session_id, "stream error after slow tool")
        .await
        .expect("submit turn");

    let (started_index, terminal_indices, error_index) =
        tokio::time::timeout(StdDuration::from_secs(5), async {
            let mut index = 0_usize;
            let mut started_index = None;
            let mut terminal_indices = Vec::new();
            while let Some(event) = rx.recv().await {
                match event {
                    StreamEvent::ToolUseStarted { tool_use_id, .. }
                        if tool_use_id == "tool-stream-error" =>
                    {
                        started_index = Some(index);
                    }
                    StreamEvent::ToolUseCompleted {
                        tool_use_id, kind, ..
                    } if tool_use_id == "tool-stream-error" => {
                        assert_eq!(kind, ToolUseCompletionKind::Interrupted);
                        terminal_indices.push(index);
                    }
                    StreamEvent::Error { message, .. } => {
                        assert!(message.contains("tool may already have produced side effects"));
                        return (started_index, terminal_indices, Some(index));
                    }
                    StreamEvent::TurnFinished { .. } => {
                        panic!("slow post-effect tool failure must not fall back")
                    }
                    _ => {}
                }
                index += 1;
            }
            (started_index, terminal_indices, None)
        })
        .await
        .expect("slow streamed tool must be interrupted and drained promptly");

    server_handle.join().expect("primary server joins");
    let _ = fallback_shutdown.send(());
    fallback_handle.join().expect("fallback recorder joins");
    let started_index = started_index.expect("slow streamed tool started");
    let error_index = error_index.expect("provider error surfaced");
    assert_eq!(terminal_indices.len(), 1);
    assert!(started_index < terminal_indices[0]);
    assert!(terminal_indices[0] < error_index);
    assert_eq!(
        fallback_requests.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        std::fs::read_to_string(marker_path).expect("read slow tool marker"),
        "started\n"
    );
}

#[tokio::test]
async fn openai_stream_delta_is_emitted_before_message_completion() {
    let (base_url, server_handle) = start_openai_text_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        default_provider: Some(ProviderId::OpenAi),
        fallback_provider: None,
        ..AppConfigOverrides::default()
    })
    .await;
    set_openai_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "use openai")
        .await
        .expect("submit turn");

    let mut saw_delta_before_completion = false;
    let mut completed = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::AssistantDelta { delta, .. } if !completed => {
                saw_delta_before_completion |= delta == "openai " || delta == "live";
            }
            StreamEvent::AssistantMessageCompleted {
                provider, usage, ..
            } => {
                assert_eq!(provider, ProviderId::OpenAi);
                assert_eq!(usage.input_tokens, 4);
                assert_eq!(usage.output_tokens, 2);
                completed = true;
            }
            StreamEvent::TurnFinished { provider, .. } => {
                assert_eq!(provider, ProviderId::OpenAi);
                break;
            }
            _ => {}
        }
    }

    server_handle.join().expect("openai text server joins");
    assert!(saw_delta_before_completion);
    assert!(completed);
}

#[tokio::test]
async fn openai_tool_use_response_executes_existing_tool_flow() {
    let (base_url, server_handle) = start_openai_tool_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        default_provider: Some(ProviderId::OpenAi),
        fallback_provider: None,
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    set_openai_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "run a command")
        .await
        .expect("submit turn");

    let mut saw_tool_start = false;
    let mut saw_tool_success = false;
    let mut saw_final = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::ToolUseStarted { tool_name, .. } => {
                saw_tool_start |= tool_name == "bash";
            }
            StreamEvent::ToolUseCompleted {
                tool_name, kind, ..
            } => {
                saw_tool_success |= tool_name == "bash" && kind == ToolUseCompletionKind::Success;
            }
            StreamEvent::AssistantMessageCompleted { message, .. } => {
                saw_final |= message.content.contains("tool done");
            }
            StreamEvent::TurnFinished { provider, .. } => {
                assert_eq!(provider, ProviderId::OpenAi);
                break;
            }
            _ => {}
        }
    }

    server_handle.join().expect("openai tool server joins");
    assert!(saw_tool_start);
    assert!(saw_tool_success);
    assert!(saw_final);
}

#[tokio::test]
async fn openai_retryable_failure_before_content_falls_back() {
    let (base_url, server_handle) = start_openai_http_error_server(429);
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        default_provider: Some(ProviderId::OpenAi),
        fallback_provider: Some(ProviderId::Anthropic),
        max_retries: Some(0),
        ..AppConfigOverrides::default()
    })
    .await;
    set_openai_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "fallback please")
        .await
        .expect("submit turn");

    let mut saw_anthropic_finish = false;
    let mut fallback_from = None;
    while let Some(event) = rx.recv().await {
        if let StreamEvent::TurnFinished {
            provider,
            fallback_from: event_fallback_from,
            ..
        } = event
        {
            saw_anthropic_finish = provider == ProviderId::Anthropic;
            fallback_from = event_fallback_from;
            break;
        }
    }

    server_handle.join().expect("openai error server joins");
    assert!(saw_anthropic_finish);
    assert_eq!(fallback_from, Some(ProviderId::OpenAi));
}

#[tokio::test]
async fn openai_stream_error_after_content_does_not_fallback() {
    let (base_url, server_handle) = start_openai_error_after_content_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        default_provider: Some(ProviderId::OpenAi),
        fallback_provider: Some(ProviderId::Anthropic),
        max_retries: Some(0),
        ..AppConfigOverrides::default()
    })
    .await;
    set_openai_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "stream error")
        .await
        .expect("submit turn");

    let mut saw_delta = false;
    let mut saw_error = false;
    let mut saw_fallback_finish = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::AssistantDelta { delta, .. } if delta == "before error" => {
                saw_delta = true;
            }
            StreamEvent::Error { message, .. } => {
                saw_error = message.contains("invalid JSON");
                break;
            }
            StreamEvent::TurnFinished { provider, .. } => {
                saw_fallback_finish = provider == ProviderId::Anthropic;
                break;
            }
            _ => {}
        }
    }

    server_handle
        .join()
        .expect("openai stream error server joins");
    assert!(saw_delta);
    assert!(saw_error);
    assert!(!saw_fallback_finish);
}

#[tokio::test]
async fn openai_wait_cancellation_interrupts_inflight_http_request() {
    let (base_url, shutdown_tx, server_handle) = start_hanging_openai_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        default_provider: Some(ProviderId::OpenAi),
        fallback_provider: None,
        ..AppConfigOverrides::default()
    })
    .await;
    set_openai_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "wait for openai")
        .await
        .expect("submit turn");

    let cancelled_kind = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::RequestStarted { .. } => {
                    assert!(manager.cancel_turn(&session_id).await);
                }
                StreamEvent::TurnCancelled { kind, .. } => return kind,
                StreamEvent::Error { message, .. } => {
                    panic!("openai request errored before cancellation: {message}");
                }
                _ => {}
            }
        }
        panic!("stream ended before cancellation");
    })
    .await
    .expect("openai provider cancellation should be prompt");

    let _ = shutdown_tx.send(());
    let _ = server_handle.join();

    assert_eq!(cancelled_kind, TurnCancellationKind::BeforeResponse);
}

#[tokio::test]
async fn assistant_streaming_cancellation_appends_interrupt_marker() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "phase two")
        .await
        .expect("submit turn");

    let mut saw_delta = false;
    let mut saw_cancel_kind = false;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::AssistantDelta { .. } => {
                if !saw_delta {
                    saw_delta = true;
                    assert!(manager.cancel_turn(&session_id).await);
                }
            }
            StreamEvent::TurnCancelled { kind, .. } => {
                saw_cancel_kind = kind == TurnCancellationKind::AssistantStreaming;
                break;
            }
            _ => {}
        }
    }

    assert!(saw_delta);
    assert!(saw_cancel_kind);

    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    assert!(
        saved
            .messages
            .iter()
            .any(|message| message.content == INTERRUPTED_TURN_MESSAGE)
    );
}

#[tokio::test]
async fn provider_wait_cancellation_interrupts_inflight_http_request() {
    let (base_url, shutdown_tx, server_handle) = start_hanging_anthropic_server();
    let mut manager = test_manager().await;
    set_anthropic_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "wait for provider")
        .await
        .expect("submit turn");

    let cancelled_kind = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::RequestStarted { .. } => {
                    assert!(manager.cancel_turn(&session_id).await);
                }
                StreamEvent::TurnCancelled { kind, .. } => return kind,
                StreamEvent::Error { message, .. } => {
                    panic!("provider request errored before cancellation: {message}");
                }
                _ => {}
            }
        }
        panic!("stream ended before cancellation");
    })
    .await
    .expect("provider cancellation should be prompt");

    let _ = shutdown_tx.send(());
    let _ = server_handle.join();

    assert_eq!(cancelled_kind, TurnCancellationKind::BeforeResponse);

    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    assert!(
        saved
            .messages
            .iter()
            .any(|message| message.content == INTERRUPTED_TURN_MESSAGE)
    );
}

#[tokio::test]
async fn provider_stream_delta_is_emitted_before_message_completion() {
    let (base_url, shutdown_tx, server_handle) = start_partial_text_anthropic_server();
    let mut manager = test_manager().await;
    set_anthropic_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "stream partial text")
        .await
        .expect("submit turn");

    let saw_delta = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::AssistantDelta { delta, .. } if delta == "partial live" => {
                    assert!(manager.cancel_turn(&session_id).await);
                    return true;
                }
                StreamEvent::AssistantMessageCompleted { .. } => {
                    panic!("assistant completed before streaming the live delta");
                }
                StreamEvent::Error { message, .. } => {
                    panic!("provider errored before live delta: {message}");
                }
                _ => {}
            }
        }
        false
    })
    .await
    .expect("live delta should arrive before the hanging stream completes");

    let _ = shutdown_tx.send(());
    let _ = server_handle.join();
    assert!(saw_delta);
}

#[tokio::test]
async fn detached_interrupt_allows_next_turn_without_waiting_for_old_stream() {
    let (base_url, shutdown_tx, server_handle) = start_hanging_anthropic_server();
    let mut manager = test_manager().await;
    set_anthropic_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut first_rx = manager
        .submit_turn(&session_id, "old hanging turn")
        .await
        .expect("submit first turn");

    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = first_rx.recv().await {
            if matches!(event, StreamEvent::RequestStarted { .. }) {
                return;
            }
        }
        panic!("first turn ended before request start");
    })
    .await
    .expect("first request should start");

    assert!(manager.interrupt_turn(&session_id).await);
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "stub://anthropic".to_string(),
    );

    let mut second_rx = manager
        .submit_turn(&session_id, "new turn after interrupt")
        .await
        .expect("detached interrupt should free the active-turn slot");

    let second_finished = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = second_rx.recv().await {
            if matches!(event, StreamEvent::TurnFinished { .. }) {
                return true;
            }
        }
        false
    })
    .await
    .expect("second turn should not wait for the first hanging stream");

    let _ = shutdown_tx.send(());
    let _ = server_handle.join();

    assert!(second_finished);
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    assert!(
        !saved
            .messages
            .iter()
            .any(|message| message.content == INTERRUPTED_TURN_MESSAGE),
        "detached UI interrupt should not inject a stale interruption marker before the next user turn"
    );
}

#[tokio::test]
async fn fallback_succeeds_when_prior_turns_have_thinking_with_signatures() {
    let (base_url, server_handle) = start_error_after_content_anthropic_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        fallback_provider: Some(ProviderId::OpenAi),
        max_retries: Some(0),
        ..AppConfigOverrides::default()
    })
    .await;
    set_anthropic_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let prior_assistant = TranscriptMessage::from_blocks(
        MessageRole::Assistant,
        vec![
            TranscriptBlock::Thinking {
                text: "prior reasoning".to_string(),
                signature: Some("sig-from-prior-turn".to_string()),
            },
            TranscriptBlock::Text {
                text: "visible reply".to_string(),
            },
        ],
    );
    manager
        .append_message(&session_id, prior_assistant)
        .await
        .expect("append prior assistant");

    let mut rx = manager
        .submit_turn(&session_id, "trigger fallback with thinking in history")
        .await
        .expect("submit turn");

    let mut saw_tombstone = false;
    let mut saw_fallback_finish = false;
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::AssistantMessageDiscarded { .. } => {
                    saw_tombstone = true;
                }
                StreamEvent::TurnFinished {
                    provider,
                    fallback_from,
                    ..
                } => {
                    saw_fallback_finish = provider == ProviderId::OpenAi
                        && fallback_from == Some(ProviderId::Anthropic);
                    return;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("fallback should complete even with thinking blocks in history");

    server_handle.join().expect("server joins");
    assert!(saw_tombstone, "primary response should be discarded");
    assert!(
        saw_fallback_finish,
        "fallback to OpenAI should succeed despite thinking blocks in prior turns"
    );
}

#[tokio::test]
async fn fatal_stream_error_after_content_surfaces_with_category() {
    let (base_url, server_handle) = start_openai_error_after_content_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        default_provider: Some(ProviderId::OpenAi),
        fallback_provider: Some(ProviderId::Anthropic),
        max_retries: Some(0),
        ..AppConfigOverrides::default()
    })
    .await;
    set_openai_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "fatal error after content")
        .await
        .expect("submit turn");

    let mut error_category = None;
    let mut saw_fallback_finish = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Error { category, .. } => {
                error_category = category;
                break;
            }
            StreamEvent::TurnFinished { provider, .. } => {
                saw_fallback_finish = provider == ProviderId::Anthropic;
                break;
            }
            _ => {}
        }
    }

    server_handle.join().expect("server joins");
    assert!(
        !saw_fallback_finish,
        "fatal errors after content should not trigger fallback"
    );
    assert!(
        error_category.is_some(),
        "error event should carry a structured category"
    );
}

#[tokio::test]
async fn retryable_error_before_content_surfaces_with_retry_exhausted_category() {
    let (base_url, server_handle) = start_openai_http_error_server(429);
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        default_provider: Some(ProviderId::OpenAi),
        fallback_provider: None,
        max_retries: Some(0),
        ..AppConfigOverrides::default()
    })
    .await;
    set_openai_server_env(&mut manager, base_url);
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "rate limit no fallback")
        .await
        .expect("submit turn");

    let mut error_category = None;
    let mut error_suggestion = None;
    while let Some(event) = rx.recv().await {
        if let StreamEvent::Error {
            category,
            suggestion,
            ..
        } = event
        {
            error_category = category;
            error_suggestion = suggestion;
            break;
        }
    }

    server_handle.join().expect("server joins");
    assert_eq!(
        error_category,
        Some(StreamErrorCategory::RateLimit),
        "retryable error should carry its category when no fallback is available"
    );
    assert!(
        error_suggestion.is_some(),
        "rate-limit error should include a suggestion"
    );
}
