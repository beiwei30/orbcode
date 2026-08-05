use std::time::Duration as StdDuration;

use orbcode_config::AppConfigOverrides;
use orbcode_protocol::{ProviderToolDefinition, StreamEvent};
use serde_json::json;

use super::support::*;
use super::*;

#[tokio::test]
async fn e2e_ask_user_question_absent_from_provider_request() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut rx = manager
        .submit_turn(&session_id, "hello")
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

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert!(
        !snapshot.body_json.contains("AskUserQuestion"),
        "ask-user-question must not appear in provider request tools"
    );
    assert!(
        snapshot.body_json.contains("Bash"),
        "other foundation tools should still be present"
    );
}

#[tokio::test]
async fn e2e_ask_user_question_present_for_capable_turn() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut rx = manager
        .submit_turn_with_interaction(
            &session_id,
            "hello",
            crate::TurnInteractionContext::capable("tui-test"),
        )
        .await
        .expect("submit capable turn");
    tokio::time::timeout(StdDuration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::TurnFinished { .. }) {
                break;
            }
        }
    })
    .await
    .expect("turn should finish");

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert!(snapshot.body_json.contains("AskUserQuestion"));
    assert!(snapshot.body_json.contains("multi_select"));
    assert!(snapshot.body_json.contains("allow_annotation"));
}

#[tokio::test]
async fn e2e_ask_user_question_hidden_for_partial_capability() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn_with_interaction(
            &session_id,
            "hello",
            crate::TurnInteractionContext {
                owner_id: "acp-test".into(),
                capabilities: crate::InteractiveQuestionCapabilities {
                    single_select: true,
                    ..Default::default()
                },
            },
        )
        .await
        .expect("submit partial-capability turn");
    while let Some(event) = rx.recv().await {
        if matches!(event, StreamEvent::TurnFinished { .. }) {
            break;
        }
    }
    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert!(!snapshot.body_json.contains("AskUserQuestion"));
}

#[tokio::test]
async fn e2e_ask_user_disconnect_cancels_owned_active_turn() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn_with_interaction(
            &session_id,
            r#"#tool:bash {"command":"sleep 2"}"#,
            crate::TurnInteractionContext::capable("disconnect-owner"),
        )
        .await
        .expect("submit capable turn");
    tokio::time::sleep(StdDuration::from_millis(50)).await;
    let cancelled = manager
        .disconnect_interaction_owner("disconnect-owner")
        .await;
    assert_eq!(cancelled, vec![session_id.clone()]);
    tokio::time::timeout(StdDuration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::TurnCancelled { .. }) {
                return;
            }
        }
        panic!("turn stream closed without cancellation");
    })
    .await
    .expect("disconnect should cancel promptly");
}

#[tokio::test]
async fn e2e_ask_user_invalid_model_input_is_tool_error_not_turn_abort() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let mut rx = manager
        .submit_turn_with_interaction(
            &session.session_id,
            r#"#tool:AskUserQuestion {"questions":[]}"#,
            crate::TurnInteractionContext::capable("tui-invalid-input"),
        )
        .await
        .expect("submit capable turn");
    let mut saw_tool_error = false;
    let mut saw_finished = false;
    tokio::time::timeout(StdDuration::from_secs(5), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::ToolUseCompleted {
                    tool_name, kind, ..
                } if tool_name == "AskUserQuestion" => {
                    saw_tool_error =
                        kind == orbcode_protocol::ToolUseCompletionKind::ExecutionFailed;
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
    .expect("invalid tool input should not hang");
    assert!(saw_tool_error);
    assert!(saw_finished);
}

#[tokio::test]
async fn e2e_plugin_tools_absent_from_provider_request() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;

    manager
        .tools
        .set_dynamic_definitions(vec![ProviderToolDefinition {
            name: "plugin__demo__search".to_string(),
            description: "A plugin tool.".to_string(),
            input_schema: json!({"type": "object"}),
        }]);

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut rx = manager
        .submit_turn(&session_id, "hello")
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

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert!(
        !snapshot.body_json.contains("plugin__demo__search"),
        "plugin tools must not appear in provider request tools"
    );
}

#[tokio::test]
async fn steer_turn_requires_active_turn() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");

    let error = manager
        .steer_turn(&session.session_id, "late input")
        .await
        .expect_err("steer should require an active turn");
    assert!(matches!(error, CoreError::NoActiveTurn(_)));
}

#[tokio::test]
async fn steer_turn_injected_during_slow_tool_execution() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:bash {"command":"sleep 0.1 && printf done"}"#,
        )
        .await
        .expect("submit turn");

    let steer_manager = (*manager).clone();
    let steer_session_id = session_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(StdDuration::from_millis(50)).await;
        steer_manager
            .steer_turn(&steer_session_id, "steered mid-execution")
            .await
            .expect("steer active turn");
    });

    tokio::time::timeout(StdDuration::from_secs(5), async {
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
    assert!(
        saved
            .messages
            .iter()
            .any(|msg| msg.role == MessageRole::User
                && msg.content.contains("steered mid-execution"))
    );
}

#[tokio::test]
async fn queued_user_command_appears_in_next_provider_request_after_tool_round() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .enqueue_user_command(&session_id, "follow-up during tool execution".to_string())
        .await;

    let mut rx = manager
        .submit_turn(&session_id, r#"#tool:bash {"command":"printf hello"}"#)
        .await
        .expect("submit turn");

    tokio::time::timeout(StdDuration::from_secs(5), async {
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

    let has_queued_message = saved.messages.iter().any(|msg| {
        msg.role == MessageRole::User && msg.content.contains("follow-up during tool execution")
    });
    assert!(
        has_queued_message,
        "queued user command should appear in persisted transcript after tool round"
    );
}

#[tokio::test]
async fn queued_user_command_injected_during_slow_tool_execution() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:bash {"command":"sleep 0.1 && printf done"}"#,
        )
        .await
        .expect("submit turn");

    let enqueue_manager = (*manager).clone();
    let enqueue_session_id = session_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(StdDuration::from_millis(50)).await;
        enqueue_manager
            .enqueue_user_command(&enqueue_session_id, "injected mid-execution".to_string())
            .await;
    });

    tokio::time::timeout(StdDuration::from_secs(5), async {
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

    let has_injected = saved
        .messages
        .iter()
        .any(|msg| msg.role == MessageRole::User && msg.content.contains("injected mid-execution"));
    assert!(
        has_injected,
        "user command enqueued during tool execution should appear in transcript"
    );
}

#[tokio::test]
async fn drain_queued_user_commands_returns_commands_in_order() {
    let manager = test_manager().await;
    let session_id = "session-drain-test";

    manager
        .enqueue_user_command(session_id, "first command".to_string())
        .await;
    manager
        .enqueue_user_command(session_id, "second command".to_string())
        .await;

    let drained = manager.drain_queued_user_commands(session_id).await;
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].content, "first command");
    assert_eq!(drained[1].content, "second command");

    let second_drain = manager.drain_queued_user_commands(session_id).await;
    assert!(second_drain.is_empty(), "drain should empty the queue");
}

#[tokio::test]
async fn undispatchable_dynamic_tool_excluded_from_provider_request() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;

    manager.tools.set_dynamic_definitions(vec![ProviderToolDefinition {
        name: "SkillAlpha".to_string(),
        description: "A dynamically registered skill tool.".to_string(),
        input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
    }]);

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let mut rx = manager
        .submit_turn(&session_id, "hello")
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

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert!(
        !snapshot.body_json.contains("SkillAlpha"),
        "dynamic tool without dispatch route must not appear in provider request"
    );
    assert!(
        snapshot.body_json.contains("Bash"),
        "foundation tools should still be present"
    );
}

#[tokio::test]
async fn undispatchable_dynamic_tool_excluded_from_provider_definitions() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (_session, _) = manager.start_or_resume(None).await.expect("create session");

    assert!(manager.tools.dynamic_definitions().is_empty());

    manager
        .tools
        .set_dynamic_definitions(vec![ProviderToolDefinition {
            name: "NewPluginTool".to_string(),
            description: "Plugin tool added mid-session.".to_string(),
            input_schema: json!({"type": "object"}),
        }]);

    let defs = manager
        .tools
        .provider_definitions_with_mcp(true, true, &manager.mcp)
        .await;
    assert!(
        !defs.iter().any(|d| d.name == "NewPluginTool"),
        "dynamic tool without dispatch route must not appear in provider_definitions_with_mcp"
    );
}

#[tokio::test]
async fn feature_gated_tool_excluded_from_provider_request() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;

    let defs_before = manager
        .tools
        .provider_definitions_with_mcp(true, true, &manager.mcp)
        .await;
    assert!(
        defs_before.iter().any(|d| d.name == "Bash"),
        "Bash tool should be present by default"
    );

    manager
        .tools
        .set_feature_disabled_tools(["Bash".to_string()].into_iter().collect());

    let defs_after = manager
        .tools
        .provider_definitions_with_mcp(true, true, &manager.mcp)
        .await;
    assert!(
        !defs_after.iter().any(|d| d.name == "Bash"),
        "Bash tool should be excluded when feature-gated"
    );
    assert!(
        defs_after.iter().any(|d| d.name == "Read"),
        "other tools should still be present"
    );
}

#[tokio::test]
async fn feature_gate_excludes_foundation_tool() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;

    let defs_before = manager
        .tools
        .provider_definitions_with_mcp(true, true, &manager.mcp)
        .await;
    assert!(
        defs_before.iter().any(|d| d.name == "Glob"),
        "Glob tool should be present by default"
    );

    manager
        .tools
        .set_feature_disabled_tools(["Glob".to_string()].into_iter().collect());

    let defs_after = manager
        .tools
        .provider_definitions_with_mcp(true, true, &manager.mcp)
        .await;
    assert!(
        !defs_after.iter().any(|d| d.name == "Glob"),
        "feature-gated foundation tool should be excluded"
    );
    assert!(
        defs_after.iter().any(|d| d.name == "Bash"),
        "other foundation tools should still be present"
    );
}

#[tokio::test]
async fn mcp_tools_still_rebuild_per_request() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;

    let defs1 = manager
        .tools
        .provider_definitions_with_mcp(true, true, &manager.mcp)
        .await;
    let defs2 = manager
        .tools
        .provider_definitions_with_mcp(true, true, &manager.mcp)
        .await;

    assert_eq!(
        defs1.len(),
        defs2.len(),
        "consecutive calls should produce same tool count (no MCP servers configured in test)"
    );
    assert!(
        defs1
            .iter()
            .all(|d1| defs2.iter().any(|d2| d2.name == d1.name)),
        "tool names should be identical across calls"
    );
}

#[tokio::test]
async fn dynamic_tool_does_not_override_foundation_tool() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;

    manager
        .tools
        .set_dynamic_definitions(vec![ProviderToolDefinition {
            name: "Bash".to_string(),
            description: "Impostor bash should be ignored.".to_string(),
            input_schema: json!({"type": "object"}),
        }]);

    let defs = manager
        .tools
        .provider_definitions_with_mcp(true, true, &manager.mcp)
        .await;
    let bash_tools: Vec<_> = defs.iter().filter(|d| d.name == "Bash").collect();
    assert_eq!(bash_tools.len(), 1, "only one Bash tool should exist");
    assert!(
        !bash_tools[0].description.contains("Impostor"),
        "foundation tool should take precedence"
    );
}
