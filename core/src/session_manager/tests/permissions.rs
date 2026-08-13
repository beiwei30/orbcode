use super::support::*;
use super::*;
use crate::approval_review::{ApprovalReviewOutcome, review_permission_boundary};
use crate::permissions::PermissionBoundaryReason;
use orbcode_protocol::{ApprovalReviewResolutionKind, ModelPermissionPreset};

#[tokio::test]
async fn provider_request_refreshes_mcp_tools_between_requests() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        allow_network: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    manager
        .append_message(
            &session.session_id,
            TranscriptMessage::new(MessageRole::User, "inspect docs"),
        )
        .await
        .expect("persist session");
    let context = manager.context_preview().await;
    let before = manager
        .provider_request_for_session(
            &session.session_id,
            "inspect docs",
            context.clone(),
            &[],
            true,
            true,
        )
        .await
        .expect("provider request before mcp server");
    assert!(
        !before
            .tools
            .iter()
            .any(|tool| tool.name == "mcp__docs__inspect")
    );

    manager
        .mcp
        .upsert_server(orbcode_mcp::McpServerConfig {
            id: "docs".to_string(),
            transport: orbcode_mcp::McpTransport::WebSocket,
            endpoint: "modeled://example.com".to_string(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            cwd: None,
            headers: std::collections::BTreeMap::new(),
            enabled: true,
            status: orbcode_mcp::McpServerStatus::Ready,
            error: None,
            summary: "Docs".to_string(),
            auth: orbcode_mcp::McpAuth::None,
            trust: orbcode_mcp::McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("add docs server");

    let after = manager
        .provider_request_for_session(
            &session.session_id,
            "inspect docs",
            context,
            &[],
            true,
            true,
        )
        .await
        .expect("provider request after mcp server");
    assert!(
        after
            .tools
            .iter()
            .any(|tool| tool.name == "mcp__docs__inspect")
    );
}

async fn run_auto_review_scenario(
    scenario: &str,
) -> (Vec<StreamEvent>, bool, orbcode_protocol::CostSummary) {
    let mut manager = test_manager().await;
    manager.config.fallback_provider = None;
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        format!("mock://anthropic?scenario={scenario}"),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    manager
        .append_message(
            &session.session_id,
            TranscriptMessage::new(MessageRole::User, "write the requested external file"),
        )
        .await
        .expect("persist task");
    manager
        .set_session_permission_preset(&session.session_id, ModelPermissionPreset::ApproveForMe)
        .await
        .expect("set auto-review preset");

    let outside = manager
        .config
        .cwd
        .parent()
        .expect("workspace parent")
        .join(format!("orbcode-review-outside-{}.txt", Uuid::new_v4()));
    let input = serde_json::json!({ "file_path": outside, "content": "reviewed" }).to_string();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner = (*manager).clone();
    let session_id = session.session_id.clone();
    let handle = tokio::spawn(async move {
        runner
            .execute_tool_use(
                &session_id,
                "tool-review",
                "file-write",
                &input,
                &tx,
                Arc::new(AtomicBool::new(false)),
            )
            .await
    });

    let mut events = Vec::new();
    let mut saw_user_request = false;
    while let Some(event) = rx.recv().await {
        if let StreamEvent::PermissionRequested { request } = &event {
            saw_user_request = true;
            assert!(
                manager
                    .respond_to_permission_request(
                        &request.request_id,
                        PermissionDecision::Approve,
                    )
                    .await
            );
        }
        events.push(event);
    }
    handle
        .await
        .expect("review task join")
        .expect("reviewed tool execution");
    let cost = manager
        .cost_overview(&session.session_id)
        .await
        .expect("review cost overview")
        .cost;
    let persisted = manager
        .load_session(&session.session_id)
        .await
        .expect("persisted review usage");
    let reviewer_reported_usage = cost
        .model_usage
        .values()
        .any(|usage| usage.input_tokens > 0 || usage.output_tokens > 0);
    if reviewer_reported_usage {
        assert!(persisted.messages.iter().any(|message| {
            message.is_synthetic
                && message.usage.is_some()
                && message.content.is_empty()
                && message.blocks.is_empty()
        }));
    }
    manager.reset_live_cost(&session.session_id).await;
    let restored_cost = manager
        .cost_overview(&session.session_id)
        .await
        .expect("restored review cost")
        .cost;
    assert!((restored_cost.total_cost_usd - cost.total_cost_usd).abs() < 1e-12);
    (events, saw_user_request, cost)
}

#[tokio::test]
async fn approve_for_me_executes_only_after_structured_reviewer_approval() {
    let (events, saw_user_request, cost) = run_auto_review_scenario("review_approve").await;
    assert!(!saw_user_request);
    assert!(cost.total_cost_usd > 0.0);
    assert!(
        cost.model_usage
            .values()
            .any(|usage| { usage.input_tokens > 0 || usage.output_tokens > 0 })
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::ApprovalReviewStarted { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ApprovalReviewCompleted {
            kind: ApprovalReviewResolutionKind::Approved,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ToolUseCompleted {
            kind: ToolUseCompletionKind::Success,
            ..
        }
    )));
}

#[tokio::test]
async fn approve_for_me_does_not_issue_a_reviewer_request_at_the_spend_cap() {
    let mut manager = test_manager().await;
    manager.config.fallback_provider = None;
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=review_approve".to_string(),
    );
    manager.config.settings.env.insert(
        "ANTHROPIC_MODEL".to_string(),
        "claude-sonnet-4-6".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    manager
        .append_message(
            &session.session_id,
            TranscriptMessage::new(MessageRole::User, "write the requested external file"),
        )
        .await
        .expect("persist task");
    manager
        .append_message(
            &session.session_id,
            TranscriptMessage::new(MessageRole::Assistant, "boundary tool requested").with_usage(
                TokenUsage {
                    input_tokens: 200_000,
                    output_tokens: 50_000,
                    ..TokenUsage::default()
                },
            ),
        )
        .await
        .expect("persist paid main-model usage");
    let (total, pricing_known) = manager.live_cost_total(&session.session_id).await;
    assert!(total > 0.0);
    assert!(pricing_known);
    manager.config.settings.max_budget_usd = Some(total);
    manager
        .set_session_permission_preset(&session.session_id, ModelPermissionPreset::ApproveForMe)
        .await
        .expect("set auto-review preset");

    let outside = manager
        .config
        .cwd
        .parent()
        .expect("workspace parent")
        .join(format!("orbcode-budget-review-{}.txt", Uuid::new_v4()));
    let input = serde_json::json!({ "file_path": outside, "content": "reviewed" }).to_string();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner = (*manager).clone();
    let session_id = session.session_id.clone();
    let handle = tokio::spawn(async move {
        runner
            .execute_tool_use(
                &session_id,
                "tool-budget-review",
                "file-write",
                &input,
                &tx,
                Arc::new(AtomicBool::new(false)),
            )
            .await
    });

    let mut saw_review_started = false;
    let mut saw_budget_escalation = false;
    let mut saw_user_request = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::ApprovalReviewStarted { .. } => saw_review_started = true,
            StreamEvent::ApprovalReviewCompleted {
                kind: ApprovalReviewResolutionKind::EscalatedToUser,
                rationale: Some(rationale),
                ..
            } => {
                saw_budget_escalation = rationale.contains("maxBudgetUsd");
            }
            StreamEvent::PermissionRequested { request } => {
                saw_user_request = true;
                assert!(
                    manager
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::Deny,
                        )
                        .await
                );
            }
            _ => {}
        }
    }
    handle
        .await
        .expect("budget review task join")
        .expect("budget review tool resolution");

    assert!(
        saw_review_started,
        "the review lifecycle must remain paired"
    );
    assert!(
        saw_budget_escalation,
        "the review must explain that maxBudgetUsd prevented the provider request"
    );
    assert!(saw_user_request, "a blocked auto-review must fail closed");
    let overview = manager
        .cost_overview(&session.session_id)
        .await
        .expect("cost overview");
    assert!((overview.cost.total_cost_usd - total).abs() < 1e-12);
    assert!(!outside.exists());
}

#[tokio::test]
async fn approve_for_me_escalates_risky_or_invalid_review_output_to_user() {
    for scenario in ["review_escalate", "review_invalid", "fatal"] {
        let (events, saw_user_request, _cost) = run_auto_review_scenario(scenario).await;
        assert!(saw_user_request, "scenario {scenario} must fail closed");
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::ApprovalReviewCompleted {
                kind: ApprovalReviewResolutionKind::EscalatedToUser
                    | ApprovalReviewResolutionKind::Failed,
                rationale: Some(_),
                ..
            }
        )));
    }
}

#[tokio::test]
async fn automatic_review_timeout_and_cancellation_fail_closed() {
    let mut manager = test_manager().await;
    manager.config.fallback_provider = None;
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=hang".to_string(),
    );
    let config = manager.effective_config();
    let timeout = review_permission_boundary(
        &config,
        &manager.auth,
        "session",
        "fetch the URL",
        "web-fetch",
        r#"{"url":"https://example.com"}"#,
        &PermissionBoundaryReason::Network,
        Arc::new(AtomicBool::new(false)),
        StdDuration::from_millis(50),
    )
    .await;
    assert!(matches!(
        timeout.outcome,
        ApprovalReviewOutcome::EscalateToUser {
            kind: ApprovalReviewResolutionKind::TimedOut,
            ..
        }
    ));

    let cancelled = review_permission_boundary(
        &config,
        &manager.auth,
        "session",
        "fetch the URL",
        "web-fetch",
        r#"{"url":"https://example.com"}"#,
        &PermissionBoundaryReason::Network,
        Arc::new(AtomicBool::new(true)),
        StdDuration::from_secs(1),
    )
    .await;
    assert_eq!(cancelled.outcome, ApprovalReviewOutcome::Cancelled);
}

#[tokio::test]
async fn provider_request_includes_plugin_mcp_tool() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        allow_network: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    manager.mcp = plugin_mcp_registry(&manager.config.home_dir).await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    manager
        .append_message(
            &session.session_id,
            TranscriptMessage::new(MessageRole::User, "inspect plugin docs"),
        )
        .await
        .expect("persist session");
    let context = manager.context_preview().await;

    let request = manager
        .provider_request_for_session(
            &session.session_id,
            "inspect plugin docs",
            context,
            &[],
            true,
            true,
        )
        .await
        .expect("provider request");

    let server_id = orbcode_mcp::scoped_plugin_server_id("demo@market", "docs");
    assert!(
        request
            .tools
            .iter()
            .any(|tool| tool.name == format!("mcp__{server_id}__inspect"))
    );
}

#[tokio::test]
async fn provider_request_hides_denied_plugin_mcp_tool() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        allow_network: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    manager.mcp = plugin_mcp_registry(&manager.config.home_dir).await;
    let server_id = orbcode_mcp::scoped_plugin_server_id("demo@market", "docs");
    manager.config.disallowed_tools = vec![format!("mcp__{server_id}__*")];
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    manager
        .append_message(
            &session.session_id,
            TranscriptMessage::new(MessageRole::User, "inspect plugin docs"),
        )
        .await
        .expect("persist session");
    let context = manager.context_preview().await;

    let request = manager
        .provider_request_for_session(
            &session.session_id,
            "inspect plugin docs",
            context,
            &[],
            true,
            true,
        )
        .await
        .expect("provider request");

    assert!(
        !request
            .tools
            .iter()
            .any(|tool| tool.name == format!("mcp__{server_id}__inspect"))
    );
}

async fn plugin_mcp_registry(home_dir: &std::path::Path) -> orbcode_mcp::McpRegistry {
    let registry = orbcode_mcp::McpRegistry::load(home_dir, home_dir)
        .await
        .expect("plugin mcp registry");
    registry
        .upsert_server(orbcode_mcp::McpServerConfig {
            id: orbcode_mcp::scoped_plugin_server_id("demo@market", "docs"),
            transport: orbcode_mcp::McpTransport::WebSocket,
            endpoint: "modeled://docs.example/mcp".to_string(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            cwd: None,
            headers: std::collections::BTreeMap::new(),
            enabled: true,
            status: orbcode_mcp::McpServerStatus::Ready,
            error: None,
            summary: "Docs".to_string(),
            auth: orbcode_mcp::McpAuth::None,
            trust: orbcode_mcp::McpServerTrust::Trusted,
            transport_type_hint: None,
            source: Some(orbcode_mcp::McpServerSource::Plugin(
                orbcode_mcp::McpPluginSource {
                    plugin_id: "demo@market".to_string(),
                    plugin_name: "demo".to_string(),
                    server_name: "docs".to_string(),
                    source: "test plugin mcp".to_string(),
                },
            )),
        })
        .await
        .expect("seed plugin mcp server");
    registry
}

#[tokio::test]
async fn executes_tool_use_after_permission_approval() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:bash {"command":"printf hi","sandbox_permissions":"require_escalated"}"#,
        )
        .await
        .expect("submit turn");

    let mut saw_permission_request = false;
    let mut saw_permission_resolved = false;
    let mut saw_tool_started = false;
    let mut saw_tool_result = false;
    let mut saw_finished = false;
    let mut request_started_count = 0;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::RequestStarted { .. } => {
                request_started_count += 1;
            }
            StreamEvent::PermissionRequested { request } => {
                saw_permission_request = true;
                assert_eq!(request.tool_name, "bash");
                assert!(
                    manager
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::Approve,
                        )
                        .await
                );
            }
            StreamEvent::ToolUseStarted { tool_name, .. } => {
                saw_tool_started = true;
                assert_eq!(tool_name, "bash");
            }
            StreamEvent::PermissionResolved { kind, .. } => {
                saw_permission_resolved = true;
                assert_eq!(kind, PermissionResolutionKind::Approved);
            }
            StreamEvent::ToolUseCompleted {
                tool_name, kind, ..
            } => {
                assert_eq!(tool_name, "bash");
                assert_eq!(kind, ToolUseCompletionKind::Success);
            }
            StreamEvent::UserMessage { message } => {
                if message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                            ..
                        } if tool_use_id == &format!("toolu-{session_id}")
                            && content.contains("hi")
                            && !is_error
                    )
                }) {
                    saw_tool_result = true;
                }
            }
            StreamEvent::TurnFinished { .. } => {
                saw_finished = true;
                break;
            }
            _ => {}
        }
    }

    assert!(saw_permission_request);
    assert!(saw_permission_resolved);
    assert!(saw_tool_started);
    assert!(saw_tool_result);
    assert!(saw_finished);
    assert!(
        request_started_count >= 2,
        "expected a follow-up provider request after the tool result"
    );

    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    assert_eq!(saved.messages.len(), 4);
    assert!(saved.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(
                block,
                TranscriptBlock::ToolUse { name, .. } if name == "bash"
            )
        })
    }));
    assert!(saved.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult { content, is_error, .. }
                    if content.contains("hi") && !is_error
            )
        })
    }));
}

#[tokio::test]
async fn provider_request_hides_tools_blocked_by_permissions() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(false),
        allow_tools: Some(false),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    manager
        .append_message(
            &session.session_id,
            TranscriptMessage::new(MessageRole::User, "inspect repo"),
        )
        .await
        .expect("persist session");
    let context = manager.context_preview().await;
    let request = manager
        .provider_request_for_session(
            &session.session_id,
            "inspect repo",
            context,
            &[],
            manager.config.allow_tools,
            manager.config.allow_network,
        )
        .await
        .expect("provider request");

    assert!(
        request
            .tools
            .iter()
            .any(|tool| tool.name == "EnterPlanMode")
    );
    assert!(!request.tools.iter().any(|tool| tool.name == "Bash"));
    assert!(!request.tools.iter().any(|tool| tool.name == "WebFetch"));
}

#[tokio::test]
async fn skips_permission_prompt_for_preapproved_tools() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(true),
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, r#"#tool:bash {"command":"printf hi"}"#)
        .await
        .expect("submit turn");

    let completed = tokio::time::timeout(Duration::from_secs(3), async {
        let mut saw_permission_request = false;
        let mut saw_tool_result = false;

        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::PermissionRequested { .. } => {
                    saw_permission_request = true;
                }
                StreamEvent::UserMessage { message } => {
                    if message.blocks.iter().any(|block| {
                        matches!(
                            block,
                            TranscriptBlock::ToolResult { content, is_error, .. }
                                if content.contains("hi") && !is_error
                        )
                    }) {
                        saw_tool_result = true;
                    }
                }
                StreamEvent::TurnFinished { .. } => {
                    return (saw_permission_request, saw_tool_result);
                }
                _ => {}
            }
        }

        (saw_permission_request, saw_tool_result)
    })
    .await
    .expect("turn should complete without interactive approval");

    assert!(!completed.0);
    assert!(completed.1);
}

#[tokio::test]
async fn permission_denial_emits_terminal_tool_event() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:bash {"command":"printf hi","sandbox_permissions":"require_escalated"}"#,
        )
        .await
        .expect("submit turn");

    let mut saw_completed_error = false;
    let mut saw_denied_tool_result = false;
    let mut saw_permission_resolved = false;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::PermissionRequested { request } => {
                assert!(
                    manager
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::Deny,
                        )
                        .await
                );
            }
            StreamEvent::ToolUseCompleted {
                tool_name, kind, ..
            } => {
                if tool_name == "bash" && kind == ToolUseCompletionKind::PermissionDenied {
                    saw_completed_error = true;
                }
            }
            StreamEvent::PermissionResolved { kind, .. } => {
                saw_permission_resolved = kind == PermissionResolutionKind::Denied;
            }
            StreamEvent::UserMessage { message } => {
                if message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult { content, is_error, .. }
                            if content.contains("permission denied") && *is_error
                    )
                }) {
                    saw_denied_tool_result = true;
                }
            }
            StreamEvent::TurnFinished { .. } => break,
            _ => {}
        }
    }

    assert!(saw_completed_error);
    assert!(saw_denied_tool_result);
    assert!(saw_permission_resolved);
}

#[tokio::test]
async fn permission_denial_finishes_turn_without_provider_continuation() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let response = ProviderResponse {
        provider: ProviderId::Anthropic,
        fallback_from: None,
        content: String::new(),
        blocks: vec![TranscriptBlock::ToolUse {
            id: "tool-denied".to_string(),
            name: "Write".to_string(),
            input: serde_json::json!({
                "file_path": "../hello.rs",
                "content": "fn main() {}\n"
            })
            .to_string(),
        }],
        stop_reason: Some("tool_use".to_string()),
        usage: TokenUsage::default(),
        deltas: Vec::new(),
    };
    let worker = {
        let manager = manager.clone();
        let session_id = session_id.clone();
        tokio::spawn(async move {
            manager
                .finish_provider_response(
                    &session_id,
                    Uuid::new_v4(),
                    "write hello.rs",
                    ToolRoundResponse::from_response(response),
                    String::new(),
                    0,
                    false,
                    &tx,
                    Arc::new(AtomicBool::new(false)),
                )
                .await
        })
    };

    let mut saw_turn_finished = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::PermissionRequested { request } => {
                assert_eq!(request.tool_name, "Write");
                assert!(
                    manager
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::Deny,
                        )
                        .await
                );
            }
            StreamEvent::TurnFinished { .. } => {
                saw_turn_finished = true;
                break;
            }
            _ => {}
        }
    }

    let outcome = worker
        .await
        .expect("join finish task")
        .expect("finish provider response");
    assert_eq!(outcome, TurnLoopOutcome::Finished);
    assert!(saw_turn_finished);
}

#[tokio::test]
async fn session_denied_tool_call_skips_later_identical_request() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let tool_input = r#"{"command":"printf denied"}"#;
    manager
        .permission_runtime
        .remember_denied_tool_call_for_session(&session_id, "bash", tool_input)
        .await;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-repeat-denied",
            "bash",
            tool_input,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_permission_request = false;
    let mut saw_denied_tool_result = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::PermissionRequested { .. } => {
                saw_permission_request = true;
            }
            StreamEvent::UserMessage { message }
                if message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult { content, is_error, .. }
                            if content.contains("previous user denial") && *is_error
                    )
                }) =>
            {
                saw_denied_tool_result = true;
            }
            _ => {}
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(!saw_permission_request);
    assert!(saw_denied_tool_result);
}

#[tokio::test]
async fn session_denied_tool_call_does_not_block_different_request() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .permission_runtime
        .remember_denied_tool_call_for_session(
            &session_id,
            "bash",
            r#"{"command":"printf denied"}"#,
        )
        .await;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let worker = {
        let manager = manager.clone();
        let session_id = session_id.clone();
        tokio::spawn(async move {
            manager
                .execute_tool_use(
                    &session_id,
                    "tool-different",
                    "bash",
                    r#"{"command":"printf different","sandbox_permissions":"require_escalated"}"#,
                    &tx,
                    Arc::new(AtomicBool::new(false)),
                )
                .await
        })
    };

    let mut saw_permission_request = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::PermissionRequested { request } => {
                saw_permission_request = true;
                assert!(
                    manager
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::Deny,
                        )
                        .await
                );
            }
            StreamEvent::ToolUseCompleted { .. } => break,
            _ => {}
        }
    }

    let outcome = worker.await.expect("join tool task").expect("execute tool");
    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(saw_permission_request);
}

#[tokio::test]
async fn configured_allow_rule_skips_permission_prompt() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allowed_tools: vec!["Bash(printf:*)".to_string()],
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, r#"#tool:bash {"command":"printf hi"}"#)
        .await
        .expect("submit turn");

    let mut saw_permission_request = false;
    let mut saw_success = false;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::PermissionRequested { .. } => {
                saw_permission_request = true;
            }
            StreamEvent::ToolUseCompleted { kind, .. } => {
                saw_success = kind == ToolUseCompletionKind::Success;
            }
            StreamEvent::TurnFinished { .. } => break,
            _ => {}
        }
    }

    assert!(!saw_permission_request);
    assert!(saw_success);

    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    let metadata = saved
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .find_map(|block| match block {
            TranscriptBlock::ToolResult {
                metadata: Some(metadata),
                ..
            } => Some(metadata),
            _ => None,
        })
        .expect("bash result metadata");
    let metadata: serde_json::Value =
        serde_json::from_str(metadata).expect("parse bash result metadata");
    assert_eq!(
        metadata
            .pointer("/sandbox/mode")
            .and_then(|value| value.as_str()),
        Some("workspace-write"),
        "a matching allow rule must not disable the workspace sandbox"
    );
}

#[tokio::test]
async fn configured_file_path_allow_rule_skips_permission_prompt() {
    let mut manager = test_manager().await;
    let outside = manager
        .config
        .cwd
        .parent()
        .expect("workspace parent")
        .join(format!("orbcode-explicit-allow-{}.txt", Uuid::new_v4()));
    manager.config.allowed_tools = vec![format!("File({})", outside.display())];
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let prompt = format!(
        "#tool:Write {}",
        serde_json::json!({"file_path": outside, "content": "hello\n"})
    );
    let mut rx = manager
        .submit_turn(&session_id, &prompt)
        .await
        .expect("submit turn");

    let mut saw_permission_request = false;
    let mut saw_success = false;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::PermissionRequested { .. } => {
                saw_permission_request = true;
            }
            StreamEvent::ToolUseCompleted { kind, .. } => {
                saw_success = kind == ToolUseCompletionKind::Success;
            }
            StreamEvent::TurnFinished { .. } => break,
            _ => {}
        }
    }

    assert!(!saw_permission_request);
    assert!(saw_success);
    assert_eq!(
        tokio::fs::read_to_string(&outside)
            .await
            .expect("written file"),
        "hello\n"
    );
}

#[tokio::test]
async fn ask_preset_executes_workspace_write_and_sandboxed_bash_without_prompt() {
    let manager = test_manager().await;
    let file_name = format!("ask-safe-{}.txt", Uuid::new_v4());
    let write_prompt = format!(
        "#tool:Write {}",
        serde_json::json!({"file_path": file_name, "content": "workspace safe\n"})
    );

    for prompt in [
        write_prompt,
        r#"#tool:bash {"command":"printf sandboxed"}"#.to_string(),
    ] {
        let (session, _) = manager.start_or_resume(None).await.expect("create session");
        let mut rx = manager
            .submit_turn(&session.session_id, &prompt)
            .await
            .expect("submit safe turn");
        let mut saw_permission_request = false;
        let mut saw_success = false;
        let mut completion_kinds = Vec::new();
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::PermissionRequested { .. } => saw_permission_request = true,
                StreamEvent::ToolUseCompleted { kind, .. } => {
                    saw_success |= kind == ToolUseCompletionKind::Success;
                    completion_kinds.push(kind);
                }
                StreamEvent::TurnFinished { .. } => break,
                _ => {}
            }
        }
        assert!(
            !saw_permission_request,
            "safe prompt unexpectedly asked: {prompt}"
        );
        assert!(
            saw_success,
            "safe prompt did not complete: {prompt}; completions={completion_kinds:?}"
        );
    }

    assert_eq!(
        tokio::fs::read_to_string(manager.config.cwd.join(file_name))
            .await
            .expect("workspace file written"),
        "workspace safe\n"
    );
}

#[tokio::test]
async fn configured_deny_rule_blocks_before_permission_prompt() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allowed_tools: vec!["Bash".to_string()],
        disallowed_tools: vec!["Bash(printf:*)".to_string()],
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, r#"#tool:bash {"command":"printf hi"}"#)
        .await
        .expect("submit turn");

    let mut saw_permission_request = false;
    let mut saw_denied = false;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::PermissionRequested { .. } => {
                saw_permission_request = true;
            }
            StreamEvent::ToolUseCompleted { kind, .. } => {
                saw_denied = kind == ToolUseCompletionKind::PermissionDenied;
            }
            StreamEvent::TurnFinished { .. } => break,
            _ => {}
        }
    }

    assert!(!saw_permission_request);
    assert!(saw_denied);
}

#[tokio::test]
async fn full_access_enables_session_tool_boundaries_without_provider_coupling() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(false),
        provider_allow_network: Some(false),
        allow_tools: Some(false),
        disallowed_tools: vec!["Bash(echo denied)".to_string()],
        ..AppConfigOverrides::default()
    })
    .await;

    let (session, _) = manager.start_or_resume(None).await.expect("session");
    manager
        .set_session_permission_preset(&session.session_id, ModelPermissionPreset::FullAccess)
        .await
        .expect("set Full Access");
    let after = manager.permission_context_for_session(&session.session_id);
    assert!(after.allow_tools);
    assert!(after.allow_network);
    assert!(!after.provider_allow_network);
    assert!(
        after
            .tool_denied("Bash", r#"{"command":"echo denied"}"#)
            .is_some()
    );
}

#[tokio::test]
async fn session_permission_rule_skips_later_bash_prompt() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut first_rx = manager
        .submit_turn(&session_id, r#"#tool:bash {"command":"wc -l README.md"}"#)
        .await
        .expect("submit first turn");
    while let Some(event) = first_rx.recv().await {
        match event {
            StreamEvent::PermissionRequested { request } => {
                assert!(
                    manager
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::ApproveAlways("wc:*".to_string()),
                        )
                        .await
                );
            }
            StreamEvent::TurnFinished { .. } => break,
            _ => {}
        }
    }

    let mut second_rx = manager
        .submit_turn(&session_id, r#"#tool:bash {"command":"wc -c README.md"}"#)
        .await
        .expect("submit second turn");
    // Timeout is a safety net for real bugs; under default cargo-test
    // parallelism (one tokio runtime per test thread on N cores) the
    // second turn's many `yield_now`-paced stub provider deltas can take
    // several seconds when the OS schedules the runtime late. 10s gives
    // realistic headroom without masking actual deadlocks.
    let completed = tokio::time::timeout(Duration::from_secs(10), async {
        let mut saw_permission_request = false;
        while let Some(event) = second_rx.recv().await {
            match event {
                StreamEvent::PermissionRequested { .. } => {
                    saw_permission_request = true;
                }
                StreamEvent::TurnFinished { .. } => return saw_permission_request,
                _ => {}
            }
        }
        saw_permission_request
    })
    .await
    .expect("second turn should finish");

    assert!(!completed);
}

#[tokio::test]
async fn session_permission_rule_edits_affect_permission_context_without_settings() {
    let manager = test_manager().await;

    let added = manager
        .add_session_permission_rule(PermissionRuleSettingKind::Allow, "Read(notes/**)")
        .await
        .expect("add session allow");
    assert!(added.changed);
    assert!(
        manager
            .permission_context()
            .tool_allowed_without_prompt("Read", r#"{"file_path":"notes/hello.txt"}"#)
    );
    assert_eq!(
        manager.session_permission_rules(PermissionRuleSettingKind::Allow),
        vec!["Read(notes/**)"]
    );

    let denied = manager
        .add_session_permission_rule(PermissionRuleSettingKind::Deny, "Bash(rm:*)")
        .await
        .expect("add session deny");
    assert!(denied.changed);
    assert!(
        manager
            .permission_context()
            .tool_denied("bash", r#"{"command":"rm -rf /tmp/example"}"#)
            .is_some()
    );

    let removed = manager
        .remove_session_permission_rule(PermissionRuleSettingKind::Deny, "Bash(rm:*)")
        .await
        .expect("remove session deny");
    assert!(removed.changed);
    assert!(
        manager
            .permission_context()
            .tool_denied("bash", r#"{"command":"rm -rf /tmp/example"}"#)
            .is_none()
    );
}

#[tokio::test]
async fn managed_only_policy_rejects_session_permission_rule_mutation() {
    let mut manager = test_manager().await;
    manager.config.policy.allow_managed_permission_rules_only = true;
    let (session, _) = manager.start_or_resume(None).await.expect("session");

    let error = manager
        .add_session_permission_rule_for_session(
            &session.session_id,
            PermissionRuleSettingKind::Allow,
            "Read(notes/**)",
        )
        .await
        .expect_err("managed-only policy must reject session rules");
    assert!(matches!(error, CoreError::PermissionDenied(_)));
    assert!(
        manager
            .session_permission_rules(PermissionRuleSettingKind::Allow)
            .is_empty()
    );
}

#[tokio::test]
async fn managed_only_policy_does_not_remember_approve_always_runtime_rule() {
    let mut manager = test_manager().await;
    manager.config.policy.allow_managed_permission_rules_only = true;
    let (session, _) = manager.start_or_resume(None).await.expect("session");
    let mut rx = manager
        .submit_turn(
            &session.session_id,
            r#"#tool:bash {"command":"printf managed-only"}"#,
        )
        .await
        .expect("submit turn");

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::PermissionRequested { request } => {
                assert!(
                    manager
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::ApproveAlways("printf:*".to_string()),
                        )
                        .await
                );
            }
            StreamEvent::TurnFinished { .. } => break,
            _ => {}
        }
    }

    assert!(
        manager
            .runtime_permission_rules(&session.session_id)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn removing_displayed_remembered_grant_is_scoped_to_target_session() {
    let manager = test_manager().await;
    let (first, _) = manager.start_or_resume(None).await.expect("first session");
    let (second, _) = manager.start_or_resume(None).await.expect("second session");
    for session_id in [&first.session_id, &second.session_id] {
        manager
            .permission_runtime
            .remember_permission_rule_for_session(session_id, "bash", "printf:*")
            .await;
    }

    let removed = manager
        .remove_session_permission_rule_for_session(
            &first.session_id,
            PermissionRuleSettingKind::Allow,
            "printf:*",
        )
        .await
        .expect("remove remembered grant");
    assert!(removed.changed);
    assert!(
        manager
            .runtime_permission_rules(&first.session_id)
            .await
            .is_empty()
    );
    assert_eq!(
        manager.runtime_permission_rules(&second.session_id).await,
        vec!["printf:*"]
    );
    assert!(
        !manager
            .permission_runtime
            .matches_permission_rule_for_session(
                &first.session_id,
                "bash",
                r#"{"command":"printf hi"}"#,
            )
            .await
    );
    assert!(
        manager
            .permission_runtime
            .matches_permission_rule_for_session(
                &second.session_id,
                "bash",
                r#"{"command":"printf hi"}"#,
            )
            .await
    );
}

#[tokio::test]
async fn cancellation_appends_interrupted_tool_result_for_pending_tool_use() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:bash {"command":"printf hi","sandbox_permissions":"require_escalated"}"#,
        )
        .await
        .expect("submit turn");

    let mut saw_interrupted_tool_result = false;
    let mut saw_interrupt_marker = false;
    let mut saw_completed_error = false;
    let mut saw_cancel_kind = false;
    let mut saw_permission_interrupted = false;
    let mut saw_turn_cancelled = false;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::PermissionRequested { .. } => {
                assert!(manager.cancel_turn(&session_id).await);
            }
            StreamEvent::UserMessage { message } => {
                if message.content == INTERRUPTED_TURN_MESSAGE_FOR_TOOL_USE {
                    saw_interrupt_marker = true;
                }
                if message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult { content, is_error, .. }
                            if content == INTERRUPTED_TOOL_RESULT && *is_error
                    )
                }) {
                    saw_interrupted_tool_result = true;
                }
            }
            StreamEvent::ToolUseCompleted {
                tool_name, kind, ..
            } => {
                if tool_name == "bash" && kind == ToolUseCompletionKind::Interrupted {
                    saw_completed_error = true;
                }
            }
            StreamEvent::PermissionResolved { kind, .. } => {
                saw_permission_interrupted = kind == PermissionResolutionKind::Interrupted;
            }
            StreamEvent::TurnCancelled { kind, .. } => {
                saw_cancel_kind = kind == TurnCancellationKind::ToolStage;
                saw_turn_cancelled = true;
                break;
            }
            _ => {}
        }
    }

    assert!(saw_interrupted_tool_result);
    assert!(saw_interrupt_marker);
    assert!(saw_completed_error);
    assert!(saw_permission_interrupted);
    assert!(saw_cancel_kind);
    assert!(saw_turn_cancelled);

    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    assert!(saved.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult { content, is_error, .. }
                    if content == INTERRUPTED_TOOL_RESULT && *is_error
            )
        })
    }));
    assert!(
        saved
            .messages
            .iter()
            .any(|message| message.content == INTERRUPTED_TURN_MESSAGE_FOR_TOOL_USE)
    );
}

#[tokio::test]
async fn denies_provider_calls_when_network_disabled() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        provider_allow_network: Some(false),
        ..Default::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "should fail")
        .await
        .expect("submit turn");

    let mut saw_permission_error = false;
    while let Some(event) = rx.recv().await {
        if let StreamEvent::Error { message, .. } = event {
            saw_permission_error = message.contains("requires network access");
            break;
        }
    }

    assert!(saw_permission_error);
}

#[tokio::test]
async fn mcp_provider_tool_name_invokes_stored_tool_directly() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    seed_fake_mcp_server(&manager).await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, r#"#tool:mcp__fake__echo {"foo":"bar"}"#)
        .await
        .expect("submit turn");

    let saw_tool_result = tokio::time::timeout(Duration::from_secs(3), async {
        let mut tool_started = false;
        let mut permission_approved = false;
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::PermissionRequested { request } => {
                    assert_eq!(request.tool_name, "mcp__fake__echo");
                    permission_approved = manager
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::Approve,
                        )
                        .await;
                }
                StreamEvent::ToolUseStarted { tool_name, .. } => {
                    assert_eq!(tool_name, "mcp__fake__echo");
                    tool_started = true;
                }
                StreamEvent::UserMessage { message } => {
                    if message.blocks.iter().any(|block| {
                        matches!(
                            block,
                            TranscriptBlock::ToolResult { content, is_error, .. }
                                if content.contains("server=fake") && !is_error
                        )
                    }) {
                        return tool_started && permission_approved;
                    }
                }
                StreamEvent::TurnFinished { .. } => return false,
                _ => {}
            }
        }
        false
    })
    .await
    .expect("turn should complete");

    assert!(saw_tool_result);
}

#[tokio::test]
async fn explicit_mcp_allow_rule_cannot_bypass_untrusted_server_state() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allowed_tools: vec!["mcp__fake__echo".to_string()],
        ..AppConfigOverrides::default()
    })
    .await;
    seed_fake_mcp_server(&manager).await;
    manager
        .mcp
        .set_server_trust("fake", orbcode_mcp::McpServerTrust::Unknown)
        .await
        .expect("set untrusted");
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let execute = manager.execute_tool_use(
        &session.session_id,
        "tool-untrusted-mcp",
        "mcp__fake__echo",
        r#"{"foo":"bar"}"#,
        &tx,
        Arc::new(AtomicBool::new(false)),
    );
    let resolve_trust = async {
        let mut saw_permission_request = false;
        let mut saw_trust_request = false;
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::PermissionRequested { request } => {
                    saw_permission_request = true;
                    manager
                        .respond_to_permission_request(
                            &request.request_id,
                            PermissionDecision::Deny,
                        )
                        .await;
                }
                StreamEvent::McpTrustApprovalRequested { request } => {
                    saw_trust_request = true;
                    manager
                        .mcp
                        .set_server_trust_for_session(
                            &session.session_id,
                            &request.server_id,
                            orbcode_mcp::McpServerTrust::Denied,
                        )
                        .await
                        .expect("deny trust request");
                }
                StreamEvent::ToolUseCompleted { .. }
                | StreamEvent::McpTrustApprovalResolved { .. } => break,
                _ => {}
            }
        }
        (saw_permission_request, saw_trust_request)
    };
    let (result, (saw_permission_request, saw_trust_request)) =
        tokio::join!(execute, resolve_trust);
    let _ = result;
    assert!(saw_trust_request, "untrusted server must request trust");
    assert!(
        !saw_permission_request,
        "explicit allow rule should satisfy only the permission layer"
    );
}

#[tokio::test]
async fn mcp_provider_tool_name_honors_server_deny_rule() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        disallowed_tools: vec!["mcp__fake__*".to_string()],
        ..AppConfigOverrides::default()
    })
    .await;
    seed_fake_mcp_server(&manager).await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, r#"#tool:mcp__fake__echo {"foo":"bar"}"#)
        .await
        .expect("submit turn");

    let saw_denial = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::PermissionRequested { .. } => return false,
                StreamEvent::UserMessage { message } => {
                    if message.blocks.iter().any(|block| {
                        matches!(
                            block,
                            TranscriptBlock::ToolResult { content, is_error, .. }
                                if content.contains(
                                    "permission denied for tool `mcp__fake__echo` by configured deny rule"
                                ) && *is_error
                        )
                    }) {
                        return true;
                    }
                }
                StreamEvent::TurnFinished { .. } => return false,
                _ => {}
            }
        }
        false
    })
    .await
    .expect("turn should complete");

    assert!(saw_denial);
}

async fn seed_fake_mcp_server(manager: &TestSessionManager) {
    manager
        .mcp
        .upsert_server(orbcode_mcp::McpServerConfig {
            id: "fake".to_string(),
            transport: orbcode_mcp::McpTransport::WebSocket,
            endpoint: "modeled://fake.local".to_string(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            cwd: None,
            headers: std::collections::BTreeMap::new(),
            enabled: true,
            status: orbcode_mcp::McpServerStatus::Ready,
            error: None,
            summary: "Fake stored MCP for tests".to_string(),
            auth: orbcode_mcp::McpAuth::None,
            trust: orbcode_mcp::McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("seed fake MCP server");
}

#[tokio::test]
async fn allow_network_false_does_not_block_provider_calls() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_network: Some(false),
        ..Default::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "provider should still run")
        .await
        .expect("submit turn");

    let mut saw_provider_start = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::AssistantMessageStarted { .. } => {
                saw_provider_start = true;
                break;
            }
            StreamEvent::Error { message, .. } if message.contains("requires network access") => {
                panic!("provider was blocked by allow_network=false: {message}");
            }
            StreamEvent::TurnFinished { .. } => break,
            _ => {}
        }
    }

    assert!(saw_provider_start);
}
