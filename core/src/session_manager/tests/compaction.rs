use super::super::session_compaction::CompactDecision;
use super::support::*;
use super::*;

const OLD_HISTORY_MARKER: &str = "old-high-usage-history-marker";
const HIGH_PRE_COMPACT_INPUT_TOKENS: u32 = 60_000;

#[tokio::test]
async fn oversized_history_auto_compacts_before_provider_request() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE".to_string(),
        "5000".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let old_history = format!("old-history-marker {}", "x".repeat(32_000));
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, old_history),
        )
        .await
        .expect("append old user message");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "old answer"),
        )
        .await
        .expect("append old assistant message");

    let prompt = "continue with the next small step";
    let mut rx = manager
        .submit_turn(&session_id, prompt)
        .await
        .expect("submit turn");

    let mut saw_finished = false;
    let mut saw_prompt_too_long = false;
    let mut saw_context_compacted = false;
    tokio::time::timeout(StdDuration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Error { message, .. } => {
                    saw_prompt_too_long |= message.contains("Prompt is too long");
                }
                StreamEvent::ContextCompacted {
                    summary,
                    original_message_count,
                    compacted_message_count,
                    provider_generated,
                    ..
                } => {
                    saw_context_compacted = summary
                        .as_deref()
                        .is_some_and(|summary| summary.contains("This session is being continued"))
                        && original_message_count == 3
                        && compacted_message_count == 2
                        && provider_generated;
                }
                StreamEvent::TurnFinished { .. } => {
                    saw_finished = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("turn finishes after auto-compact");

    assert!(saw_finished);
    assert!(saw_context_compacted);
    assert!(!saw_prompt_too_long);

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load compacted session");
    assert!(
        loaded
            .messages
            .first()
            .is_some_and(|message| message.role == MessageRole::System)
    );
    assert!(
        loaded.messages[0]
            .content
            .contains("This session is being continued")
    );
    assert!(
        loaded
            .messages
            .iter()
            .any(|message| message.role == MessageRole::User && message.content == prompt)
    );
    assert!(
        !loaded
            .messages
            .iter()
            .any(|message| message.content.contains("old-history-marker"))
    );

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert!(snapshot.body_json.contains(prompt));
    assert!(
        snapshot
            .body_json
            .contains("This session is being continued")
    );
    assert!(!snapshot.body_json.contains("old-history-marker"));
}

#[tokio::test]
async fn manual_compact_resume_next_turn_drops_pre_compact_usage_anchor() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE".to_string(),
        "5000".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    append_high_usage_old_history(&manager, &session_id).await;

    let result = manager
        .compact_session(&session_id)
        .await
        .expect("compact session");
    assert_eq!(result.original_message_count, 2);
    assert_eq!(result.compacted_message_count, 1);
    assert!(
        result
            .session
            .messages
            .iter()
            .all(|message| message.usage.is_none())
    );

    let (resumed, loaded_event) = manager
        .start_or_resume(Some(&session_id))
        .await
        .expect("resume compacted session");
    assert!(matches!(loaded_event, StreamEvent::SessionLoaded { .. }));
    assert_eq!(resumed.messages.len(), 1);
    assert_eq!(resumed.messages[0].role, MessageRole::System);
    assert_compacted_estimate_drops_old_usage(&manager, &session_id).await;

    let next_prompt = "manual compact follow-up";
    let rx = manager
        .submit_turn(&session_id, next_prompt)
        .await
        .expect("submit next turn");
    wait_for_finished_turn(rx).await;

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("next turn provider request snapshot");
    assert_compacted_turn_request(&snapshot.body_json, &[next_prompt]);
}

#[tokio::test]
async fn auto_compact_follow_up_turn_drops_pre_compact_usage_anchor() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE".to_string(),
        "5000".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    append_high_usage_old_history(&manager, &session_id).await;

    let compacting_prompt = "trigger auto compact with this small prompt";
    let rx = manager
        .submit_turn(&session_id, compacting_prompt)
        .await
        .expect("submit compacting turn");
    let events = wait_for_finished_turn(rx).await;
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ContextCompacted {
            original_message_count: 3,
            compacted_message_count: 2,
            ..
        }
    )));
    assert_compacted_estimate_drops_old_usage(&manager, &session_id).await;

    let next_prompt = "auto compact follow-up";
    let rx = manager
        .submit_turn(&session_id, next_prompt)
        .await
        .expect("submit next turn");
    wait_for_finished_turn(rx).await;

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("next turn provider request snapshot");
    assert_compacted_turn_request(&snapshot.body_json, &[compacting_prompt, next_prompt]);
}

#[tokio::test]
async fn provider_prompt_too_long_error_reactive_compacts_and_retries_once() {
    let (base_url, request_rx, server_handle) = start_reactive_compaction_anthropic_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        fallback_provider: None,
        max_retries: Some(0),
        ..AppConfigOverrides::default()
    })
    .await;
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

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, OLD_HISTORY_MARKER),
        )
        .await
        .expect("append old user message");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "old answer marker")
                .with_usage(high_pre_compact_usage()),
        )
        .await
        .expect("append old assistant message");

    let prompt = "keep this current prompt";
    let mut rx = manager
        .submit_turn(&session_id, prompt)
        .await
        .expect("submit turn");

    let mut saw_context_compacted = false;
    let mut saw_finished = false;
    tokio::time::timeout(StdDuration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::ContextCompacted {
                    summary,
                    original_message_count,
                    compacted_message_count,
                    provider_generated,
                    ..
                } => {
                    saw_context_compacted = summary
                        .as_deref()
                        .is_some_and(|summary| summary.contains("reactive compact summary marker"))
                        && original_message_count == 3
                        && compacted_message_count == 2
                        && provider_generated;
                }
                StreamEvent::Error { message, .. } => {
                    panic!("reactive compaction should recover from provider error: {message}");
                }
                StreamEvent::TurnFinished { .. } => {
                    saw_finished = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("turn finishes after reactive compaction");
    server_handle
        .join()
        .expect("reactive compaction server joins");

    assert!(saw_context_compacted);
    assert!(saw_finished);

    let requests = request_rx.try_iter().collect::<Vec<_>>();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].contains(OLD_HISTORY_MARKER));
    assert!(requests[0].contains(prompt));
    assert!(requests[2].contains("reactive compact summary marker"));
    assert!(requests[2].contains(prompt));
    assert!(!requests[2].contains(OLD_HISTORY_MARKER));

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load compacted session");
    assert_eq!(
        loaded
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::User && message.content == prompt)
            .count(),
        1
    );
    assert_eq!(
        loaded
            .messages
            .iter()
            .filter(|message| {
                message.role == MessageRole::Assistant
                    && message.content == "final answer after reactive compaction"
            })
            .count(),
        1
    );
    assert!(!loaded.messages.iter().any(|message| matches!(
        message.role,
        MessageRole::User | MessageRole::Assistant
    )
        && message.content.contains(OLD_HISTORY_MARKER)));
    assert!(
        loaded
            .messages
            .iter()
            .all(|message| message.blocks.iter().all(|block| {
                !matches!(
                    block,
                    TranscriptBlock::ToolUse { .. } | TranscriptBlock::ToolResult { .. }
                )
            }))
    );

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert!(
        snapshot
            .body_json
            .contains("reactive compact summary marker")
    );
    assert!(snapshot.body_json.contains(prompt));
    assert!(!snapshot.body_json.contains(OLD_HISTORY_MARKER));

    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "stub://anthropic".to_string(),
    );
    manager.config.settings.env.insert(
        "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE".to_string(),
        "5000".to_string(),
    );
    assert_compacted_estimate_drops_old_usage(&manager, &session_id).await;

    let next_prompt = "reactive compact follow-up";
    let rx = manager
        .submit_turn(&session_id, next_prompt)
        .await
        .expect("submit post-reactive turn");
    wait_for_finished_turn(rx).await;

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("post-reactive provider request snapshot");
    assert_compacted_turn_request(&snapshot.body_json, &[prompt, next_prompt]);
}

#[tokio::test]
async fn oversized_turn_errors_before_provider_request() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE".to_string(),
        "1".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(
            &session_id,
            "this prompt should be too large for the test limit",
        )
        .await
        .expect("submit turn");

    let mut saw_prompt_too_long = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Error { message, .. } => {
                saw_prompt_too_long = message.contains("Prompt is too long")
                    && message.contains("blocking limit of 1");
                break;
            }
            StreamEvent::AssistantMessageCompleted { message, .. } => {
                panic!("provider should not run: {}", message.content);
            }
            StreamEvent::TurnFinished { .. } => {
                panic!("oversized prompt should not finish");
            }
            _ => {}
        }
    }

    assert!(saw_prompt_too_long);
}

#[tokio::test]
async fn glm_4_7_preflight_reports_default_177k_blocking_limit() {
    let mut manager = test_manager().await;
    manager
        .config
        .settings
        .env
        .insert("ANTHROPIC_MODEL".to_string(), "glm-4.7".to_string());
    let request = ProviderRequest {
        session_id: "session".to_string(),
        prompt: String::new(),
        context: TurnContext {
            cwd: "/repo".to_string(),
            current_date: "2026-05-06".to_string(),
            ..Default::default()
        },
        messages: vec![TranscriptMessage::new(
            MessageRole::User,
            "x".repeat(708_000),
        )],
        system_prompt: String::new(),
        tools: Vec::new(),
        model: "glm-4.7".to_string(),
        base_url: "stub://anthropic".to_string(),
        api_key: None,
        auth_token: None,
        disable_thinking: false,
        effort: None,
        options: ProviderRequestOptions::default(),
    };

    let message = manager
        .prompt_too_long_preflight_error(&request, &manager.config)
        .await
        .expect("prompt should be at the default blocking limit");

    assert!(message.contains("estimated 177000 context tokens"));
    assert!(message.contains("blocking limit of 177000"));
    assert!(message.contains("model `glm-4.7`"));
}

#[tokio::test]
async fn compact_session_rewrites_transcript_to_modeled_summary() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "please inspect the repo"),
        )
        .await
        .expect("append user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "I found the TUI entrypoint."),
        )
        .await
        .expect("append assistant");
    let result = manager
        .compact_session(&session_id)
        .await
        .expect("compact session");

    assert_eq!(result.original_message_count, 2);
    assert_eq!(result.compacted_message_count, 1);
    assert!(result.provider_generated);
    assert_eq!(result.fallback_reason, None);
    assert!(result.usage.is_some());
    assert_eq!(result.session.messages.len(), 1);
    assert_eq!(result.session.messages[0].role, MessageRole::System);
    let summary = &result.session.messages[0].content;
    assert!(summary.contains("This session is being continued"));
    assert!(summary.contains("Anthropic compatibility stub response"));
    assert!(summary.contains("read the full transcript at:"));
    assert!(!summary.contains("local modeled compaction placeholder"));

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load compacted session");
    assert_eq!(loaded.messages.len(), 1);
    assert_eq!(loaded.messages[0].role, MessageRole::System);
    assert_eq!(
        loaded.messages[0].content,
        result.session.messages[0].content
    );
}

#[tokio::test]
async fn compact_session_falls_back_to_modeled_summary_when_provider_unavailable() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        provider_allow_network: Some(false),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "please inspect the repo"),
        )
        .await
        .expect("append user");

    let result = manager
        .compact_session(&session_id)
        .await
        .expect("compact session");

    assert!(!result.provider_generated);
    assert_eq!(result.usage, None);
    assert!(
        result
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("requires network access"))
    );
    assert!(
        result.session.messages[0]
            .content
            .contains("local modeled compaction placeholder")
    );
    assert!(
        result.session.messages[0]
            .content
            .contains("please inspect the repo")
    );
}

#[tokio::test]
async fn microcompact_clears_old_tool_results_before_provider_request() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ORBCODE_MICROCOMPACT_TOKEN_THRESHOLD_OVERRIDE".to_string(),
        "1".to_string(),
    );
    manager.config.settings.env.insert(
        "ORBCODE_MICROCOMPACT_KEEP_RECENT_OVERRIDE".to_string(),
        "0".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let big_result = format!("microcompact-clearme {}", "x".repeat(4_000));
    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "Read".to_string(),
                    input: r#"{"file_path":"big.txt"}"#.to_string(),
                }],
            )
            .with_stop_reason("tool_use"),
        )
        .await
        .expect("append tool_use");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: big_result.clone(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        )
        .await
        .expect("append tool_result");

    let prompt = "summarize what you found";
    let mut rx = manager
        .submit_turn(&session_id, prompt)
        .await
        .expect("submit turn");

    let mut saw_finished = false;
    let mut saw_microcompact = false;
    let mut saw_prompt_too_long = false;
    let mut request_started_count = 0_usize;
    tokio::time::timeout(StdDuration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::RequestStarted { .. } => {
                    request_started_count += 1;
                }
                StreamEvent::Error { message, .. } => {
                    saw_prompt_too_long |= message.contains("Prompt is too long");
                }
                StreamEvent::ContextCompacted {
                    summary,
                    provider_generated,
                    ..
                } => {
                    saw_microcompact = summary
                        .as_deref()
                        .is_some_and(|summary| summary.contains("Microcompacted 1 tool result"))
                        && !provider_generated;
                }
                StreamEvent::TurnFinished { .. } => {
                    saw_finished = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("turn finishes after microcompact");

    assert!(saw_finished);
    assert!(saw_microcompact);
    assert!(!saw_prompt_too_long);
    // The lightweight-compaction round loops back without issuing a provider
    // request, so it must NOT emit a `RequestStarted`. Exactly one request is
    // made (the post-compaction round); a count of 2 is the pre-fix regression.
    assert_eq!(
        request_started_count, 1,
        "compaction round must not emit a spurious RequestStarted event"
    );

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load compacted session");
    assert!(loaded.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult { content, .. }
                    if content == crate::compaction::MICROCOMPACT_TOOL_RESULT_PLACEHOLDER
            )
        })
    }));
    assert!(
        !loaded
            .messages
            .iter()
            .any(|message| message.content.contains("microcompact-clearme"))
    );

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert!(!snapshot.body_json.contains("microcompact-clearme"));
    assert!(snapshot.body_json.contains(prompt));

    // Resume: the cleared history must survive a reload so the rebuilt request
    // never re-expands the tool result.
    let (resumed, loaded_event) = manager
        .start_or_resume(Some(&session_id))
        .await
        .expect("resume compacted session");
    assert!(matches!(loaded_event, StreamEvent::SessionLoaded { .. }));
    assert!(
        !resumed
            .messages
            .iter()
            .any(|message| message.content.contains("microcompact-clearme"))
    );

    let next_prompt = "microcompact resume follow-up";
    let rx = manager
        .submit_turn(&session_id, next_prompt)
        .await
        .expect("submit resume turn");
    wait_for_finished_turn(rx).await;
    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("resume request snapshot");
    assert!(!snapshot.body_json.contains("microcompact-clearme"));
    assert!(snapshot.body_json.contains(next_prompt));
}

#[tokio::test]
async fn snip_truncates_oversized_message_before_provider_request() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ORBCODE_SNIP_MESSAGE_TOKEN_THRESHOLD_OVERRIDE".to_string(),
        "100".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let huge = format!("snip-oversized-marker {}", "x".repeat(8_000));
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, huge.clone()),
        )
        .await
        .expect("append huge user message");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "kept-answer-marker"),
        )
        .await
        .expect("append kept assistant message");

    let prompt = "continue please";
    let mut rx = manager
        .submit_turn(&session_id, prompt)
        .await
        .expect("submit turn");

    let mut saw_finished = false;
    let mut saw_snip = false;
    tokio::time::timeout(StdDuration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Error { message, .. } => {
                    panic!("snip turn should not error: {message}");
                }
                StreamEvent::ContextCompacted {
                    summary,
                    provider_generated,
                    ..
                } => {
                    saw_snip = summary
                        .as_deref()
                        .is_some_and(|summary| summary.contains("Snipped 1 oversized message"))
                        && !provider_generated;
                }
                StreamEvent::TurnFinished { .. } => {
                    saw_finished = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("turn finishes after snip");

    assert!(saw_finished);
    assert!(saw_snip);

    let big_run = "x".repeat(8_000);
    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load snipped session");
    assert!(
        loaded
            .messages
            .iter()
            .any(|message| message.role == MessageRole::System
                && message.content == crate::compaction::SNIP_BOUNDARY_TEXT)
    );
    assert!(
        !loaded
            .messages
            .iter()
            .any(|message| message.content.contains(&big_run))
    );
    // Remaining messages keep their pairing/content.
    assert!(
        loaded
            .messages
            .iter()
            .any(|message| message.content == "kept-answer-marker")
    );
    assert!(
        loaded
            .messages
            .iter()
            .any(|message| message.role == MessageRole::User && message.content == prompt)
    );

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert!(!snapshot.body_json.contains(&big_run));
    assert!(
        snapshot
            .body_json
            .contains(crate::compaction::SNIP_BOUNDARY_TEXT)
    );
    assert!(snapshot.body_json.contains("kept-answer-marker"));
    assert!(snapshot.body_json.contains(prompt));
}

async fn append_high_usage_old_history(manager: &SessionManager, session_id: &str) {
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::User, OLD_HISTORY_MARKER),
        )
        .await
        .expect("append old user message");
    manager
        .append_message(
            session_id,
            TranscriptMessage::new(MessageRole::Assistant, "old high-usage answer")
                .with_usage(high_pre_compact_usage()),
        )
        .await
        .expect("append old assistant message");
}

fn high_pre_compact_usage() -> TokenUsage {
    TokenUsage {
        input_tokens: HIGH_PRE_COMPACT_INPUT_TOKENS,
        output_tokens: 12,
        total_tokens: HIGH_PRE_COMPACT_INPUT_TOKENS + 12,
        ..TokenUsage::default()
    }
}

async fn wait_for_finished_turn(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
) -> Vec<StreamEvent> {
    tokio::time::timeout(StdDuration::from_secs(3), async {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::Error { .. }) {
                panic!("turn should not error after compaction: {event:?}");
            }
            let finished = matches!(event, StreamEvent::TurnFinished { .. });
            events.push(event);
            if finished {
                break;
            }
        }
        events
    })
    .await
    .expect("turn finishes")
}

async fn assert_compacted_estimate_drops_old_usage(manager: &SessionManager, session_id: &str) {
    let loaded = manager
        .load_session(session_id)
        .await
        .expect("load compacted session");
    assert!(
        orbcode_protocol::token_count_with_estimation(&loaded.messages)
            < HIGH_PRE_COMPACT_INPUT_TOKENS / 2
    );

    let overview = manager
        .context_usage_overview(session_id, manager.context_preview().await)
        .await
        .expect("context usage overview");
    assert_eq!(
        overview.token_source,
        crate::ContextTokenSource::RoughEstimateFallback
    );
    assert!(overview.estimated_tokens < HIGH_PRE_COMPACT_INPUT_TOKENS / 2);
}

/// End-to-end snip smoke mirroring the manual two-turn CLI check (`stub://`
/// provider + low snip threshold). Unlike
/// `snip_truncates_oversized_message_before_provider_request`, which injects
/// history via `append_message`, here the oversized history is produced by a
/// real first turn — so a regression anywhere in the turn pipeline (not just
/// the compaction helper) trips it.
#[tokio::test]
async fn snip_two_turn_smoke_compacts_oversized_history() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ORBCODE_SNIP_MESSAGE_TOKEN_THRESHOLD_OVERRIDE".to_string(),
        "50".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    // Turn 1: a genuinely oversized prompt. snip never touches the live prompt
    // (the trailing message), so this turn must not compact.
    let huge = format!("snip-smoke-marker {}", "x".repeat(8_000));
    let compacted_turn1 = run_turn_collecting_compaction(&manager, &session_id, &huge).await;
    assert!(!compacted_turn1, "turn 1 must not snip the live prompt");

    // Turn 2: the oversized turn-1 prompt is now history, so snip fires before
    // the provider request — exactly the CLI smoke's second turn.
    let second = "second turn please";
    let compacted_turn2 = run_turn_collecting_compaction(&manager, &session_id, second).await;
    assert!(compacted_turn2, "turn 2 should snip oversized history");

    let big_run = "x".repeat(8_000);
    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load snipped session");
    assert!(
        loaded
            .messages
            .iter()
            .any(|message| message.role == MessageRole::System
                && message.content == crate::compaction::SNIP_BOUNDARY_TEXT)
    );
    assert!(
        !loaded
            .messages
            .iter()
            .any(|message| message.content.contains(&big_run))
    );
    assert!(
        loaded
            .messages
            .iter()
            .any(|message| message.role == MessageRole::User && message.content == second)
    );
}

/// Submit one turn, drain its events, and return whether a non-provider
/// `ContextCompacted` (snip/microcompact) event arrived before the turn
/// finished. Panics if the turn errors or never finishes.
async fn run_turn_collecting_compaction(
    manager: &SessionManager,
    session_id: &str,
    prompt: &str,
) -> bool {
    let mut rx = manager
        .submit_turn(session_id, prompt)
        .await
        .expect("submit turn");
    let mut saw_compacted = false;
    let mut saw_finished = false;
    tokio::time::timeout(StdDuration::from_secs(5), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Error { message, .. } => panic!("turn should not error: {message}"),
                StreamEvent::ContextCompacted {
                    provider_generated, ..
                } => {
                    saw_compacted |= !provider_generated;
                }
                StreamEvent::TurnFinished { .. } => {
                    saw_finished = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("turn finishes");
    assert!(saw_finished);
    saw_compacted
}

fn assert_compacted_turn_request(body_json: &str, expected_prompts: &[&str]) {
    let body: Value = serde_json::from_str(body_json).expect("provider request body parses");
    let system = body
        .get("system")
        .and_then(Value::as_str)
        .expect("Anthropic request has system prompt");
    assert!(system.contains("This session is being continued"));
    assert!(!system.contains(OLD_HISTORY_MARKER));

    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .expect("Anthropic request has messages");
    let messages_json = serde_json::to_string(messages).expect("serialize messages");
    for prompt in expected_prompts {
        assert!(messages_json.contains(prompt));
    }
    assert!(!messages_json.contains(OLD_HISTORY_MARKER));
    assert!(!body_json.contains(&HIGH_PRE_COMPACT_INPUT_TOKENS.to_string()));
}

/// Full lifecycle: create → turns → /compact → inject stale pre-compact
/// records into the JSONL → reload → verify stale records are GC'd.
///
/// This exercises the real compaction path (provider-generated summary via
/// stub provider) followed by the transcript GC that fires on load. The
/// injected stale records simulate the scenario where a previous runtime
/// (e.g. the TypeScript CLI) appended a compact summary without rewriting
/// the file, leaving pre-compact records orphaned.
#[tokio::test]
async fn compact_then_inject_stale_records_gc_on_reload() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "first question"),
        )
        .await
        .expect("append user message");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "first answer"),
        )
        .await
        .expect("append assistant message");

    let compact_result = manager
        .compact_session(&session_id)
        .await
        .expect("compact session");
    assert!(
        compact_result.original_message_count >= 2,
        "at least the user+assistant pair"
    );
    assert_eq!(
        compact_result.compacted_message_count, 1,
        "full compact produces a single summary message"
    );

    let after_compact = manager
        .load_session(&session_id)
        .await
        .expect("load after compact");
    assert_eq!(
        after_compact.messages.len(),
        1,
        "only the compact summary message remains"
    );
    assert!(
        after_compact.messages[0]
            .content
            .starts_with("This session is being continued")
    );

    let transcript_path = manager.transcript_store.path(&session_id);
    let existing = tokio::fs::read_to_string(&transcript_path)
        .await
        .expect("read compacted transcript");

    let stale_records = [
        serde_json::json!({
            "type": "user",
            "uuid": "stale-user-injected",
            "timestamp": "2025-01-01T00:00:00.000Z",
            "message": { "role": "user", "content": "STALE_PRE_COMPACT_USER_MARKER" },
            "cwd": "/tmp/stale",
            "sessionId": session_id,
        }),
        serde_json::json!({
            "type": "assistant",
            "uuid": "stale-assistant-injected",
            "timestamp": "2025-01-01T00:00:01.000Z",
            "message": {
                "role": "assistant",
                "content": [{ "type": "text", "text": "STALE_PRE_COMPACT_ASSISTANT_MARKER" }],
                "model": "stub-model",
            },
        }),
    ];
    let stale_lines: String = stale_records
        .iter()
        .map(|r| serde_json::to_string(r).expect("serialize stale record"))
        .collect::<Vec<_>>()
        .join("\n");
    let injected = format!("{stale_lines}\n{existing}");
    tokio::fs::write(&transcript_path, &injected)
        .await
        .expect("write injected transcript");

    let reloaded = manager
        .load_session(&session_id)
        .await
        .expect("reload after stale injection");

    assert!(
        !reloaded
            .messages
            .iter()
            .any(|m| m.content.contains("STALE_PRE_COMPACT_USER_MARKER")
                || m.content.contains("STALE_PRE_COMPACT_ASSISTANT_MARKER")),
        "stale pre-compact messages must be GC'd on reload, got: {:?}",
        reloaded
            .messages
            .iter()
            .map(|m| format!("[{:?}] {}", m.role, &m.content[..m.content.len().min(80)]))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        reloaded.messages.len(),
        1,
        "only the compact summary survives after GC"
    );
    assert!(
        reloaded.messages[0]
            .content
            .starts_with("This session is being continued")
    );
}

#[tokio::test]
async fn manual_compact_needs_confirmation_when_context_low() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "small prompt"),
        )
        .await
        .expect("append user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "short answer"),
        )
        .await
        .expect("append assistant");

    let decision = manager
        .evaluate_manual_compact_decision(&session_id)
        .await
        .expect("evaluate decision");

    assert!(
        matches!(
            decision,
            CompactDecision::NeedsConfirmation {
                context_percent_used,
                threshold_percent: 50,
            } if context_percent_used < 50
        ),
        "low context should need confirmation, got: {decision:?}"
    );
}

#[tokio::test]
async fn manual_compact_proceeds_when_context_above_threshold() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ORBCODE_MANUAL_COMPACT_THRESHOLD_PERCENT_OVERRIDE".to_string(),
        "1".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let big_content = "x".repeat(32_000);
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, big_content),
        )
        .await
        .expect("append user");

    let decision = manager
        .evaluate_manual_compact_decision(&session_id)
        .await
        .expect("evaluate decision");

    assert_eq!(decision, CompactDecision::Proceed);
}

#[tokio::test]
async fn manual_compact_proceeds_when_session_has_only_system_messages() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::System, "session context note"),
        )
        .await
        .expect("append system message");

    let decision = manager
        .evaluate_manual_compact_decision(&session_id)
        .await
        .expect("evaluate decision");

    assert!(
        matches!(
            decision,
            CompactDecision::NeedsConfirmation { context_percent_used, .. } if context_percent_used < 50
        ),
        "minimal content should need confirmation, got: {decision:?}"
    );
}

#[tokio::test]
async fn autocompact_skipped_when_recently_compacted() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "old prompt"),
        )
        .await
        .expect("append user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "old answer"),
        )
        .await
        .expect("append assistant");

    let _result = manager
        .compact_session(&session_id)
        .await
        .expect("compact session");

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "first turn after compact"),
        )
        .await
        .expect("append user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "first answer"),
        )
        .await
        .expect("append assistant");

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load session");
    let decision = manager.evaluate_autocompact_recent_guard(&loaded.messages);

    assert!(
        matches!(
            decision,
            CompactDecision::SkippedRecentManual {
                turns_since_compact: 1,
            }
        ),
        "should skip autocompact 1 turn after manual compact, got: {decision:?}"
    );
}

#[tokio::test]
async fn autocompact_proceeds_when_enough_turns_since_compact() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "old prompt"),
        )
        .await
        .expect("append user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "old answer"),
        )
        .await
        .expect("append assistant");

    let _result = manager
        .compact_session(&session_id)
        .await
        .expect("compact session");

    for i in 0..4 {
        manager
            .append_message(
                &session_id,
                TranscriptMessage::new(MessageRole::User, format!("turn {i}")),
            )
            .await
            .expect("append user");
        manager
            .append_message(
                &session_id,
                TranscriptMessage::new(MessageRole::Assistant, format!("answer {i}")),
            )
            .await
            .expect("append assistant");
    }

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load session");
    let decision = manager.evaluate_autocompact_recent_guard(&loaded.messages);

    assert_eq!(
        decision,
        CompactDecision::Proceed,
        "should proceed after 4 turns (> default 3 guard)"
    );
}

#[tokio::test]
async fn autocompact_proceeds_when_no_prior_compact() {
    let manager = test_manager().await;
    let messages = vec![
        TranscriptMessage::new(MessageRole::User, "hello"),
        TranscriptMessage::new(MessageRole::Assistant, "world"),
    ];

    let decision = manager.evaluate_autocompact_recent_guard(&messages);

    assert_eq!(
        decision,
        CompactDecision::Proceed,
        "should proceed when no compact summary in history"
    );
}
