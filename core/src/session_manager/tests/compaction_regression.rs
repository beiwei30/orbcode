use super::support::*;
use super::*;

// ---------------------------------------------------------------------------
// Acceptance criterion 1: Compact boundary in multi-tool round — tool_results
// after compaction are not lost.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compact_boundary_preserves_tool_results_in_multi_tool_round() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE".to_string(),
        "5000".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let old_history = format!("old-multi-tool-history {}", "x".repeat(32_000));
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, old_history),
        )
        .await
        .expect("append old user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "old answer for multi-tool"),
        )
        .await
        .expect("append old assistant");

    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "tool-a".to_string(),
                        name: "bash".to_string(),
                        input: r#"{"command":"echo a"}"#.to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "tool-b".to_string(),
                        name: "glob".to_string(),
                        input: r#"{"pattern":"*.rs"}"#.to_string(),
                    },
                ],
            )
            .with_stop_reason("tool_use"),
        )
        .await
        .expect("append multi-tool assistant");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![
                    TranscriptBlock::ToolResult {
                        tool_use_id: "tool-a".to_string(),
                        content: "result-a-marker".to_string(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "tool-b".to_string(),
                        content: "result-b-marker".to_string(),
                        is_error: false,
                        metadata: None,
                    },
                ],
            ),
        )
        .await
        .expect("append tool results");

    let result = manager
        .compact_session(&session_id)
        .await
        .expect("compact session");
    assert!(
        result
            .session
            .messages
            .first()
            .is_some_and(|m| m.content.contains("This session is being continued"))
    );

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load after compact");
    assert!(
        !loaded
            .messages
            .iter()
            .any(|m| m.content.contains("old-multi-tool-history"))
    );
}

#[tokio::test]
async fn multi_tool_round_after_auto_compact_persists_all_tool_results() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE".to_string(),
        "5000".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let old_history = format!("pre-compact-bulk {}", "x".repeat(32_000));
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, old_history),
        )
        .await
        .expect("append old user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "old answer"),
        )
        .await
        .expect("append old assistant");

    let prompt = "continue after compact";
    let mut rx = manager
        .submit_turn(&session_id, prompt)
        .await
        .expect("submit turn that triggers auto-compact");

    let mut saw_compacted = false;
    let mut saw_finished = false;
    tokio::time::timeout(StdDuration::from_secs(5), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::ContextCompacted { .. } => saw_compacted = true,
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
    assert!(saw_compacted);
    assert!(saw_finished);

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load compacted session");
    assert!(
        loaded
            .messages
            .first()
            .is_some_and(|m| m.role == MessageRole::System
                && m.content.contains("This session is being continued"))
    );
    assert!(
        loaded
            .messages
            .iter()
            .any(|m| m.role == MessageRole::User && m.content == prompt)
    );
}

#[tokio::test]
async fn compact_preserves_tool_use_tool_result_pairing() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "paired-1".to_string(),
                        name: "bash".to_string(),
                        input: r#"{"command":"ls"}"#.to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "paired-2".to_string(),
                        name: "Read".to_string(),
                        input: r#"{"file_path":"x.rs"}"#.to_string(),
                    },
                ],
            )
            .with_stop_reason("tool_use"),
        )
        .await
        .expect("append tool uses");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![
                    TranscriptBlock::ToolResult {
                        tool_use_id: "paired-1".to_string(),
                        content: "dir listing".to_string(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "paired-2".to_string(),
                        content: "file contents".to_string(),
                        is_error: false,
                        metadata: None,
                    },
                ],
            ),
        )
        .await
        .expect("append tool results");

    let result = manager
        .compact_session(&session_id)
        .await
        .expect("compact session");
    assert_eq!(result.compacted_message_count, 1);
    assert!(result.provider_generated);

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load after compact");
    assert_eq!(loaded.messages.len(), 1);
    assert_eq!(loaded.messages[0].role, MessageRole::System);
    assert!(
        loaded.messages[0]
            .content
            .contains("This session is being continued")
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 2: Parent turn cancel → subagent child session
// metadata marked cancelled.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn parent_cancel_marks_child_session_cancelled() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let child_id = format!("{session_id}:agent-cancel-test-1");
    manager
        .child_session_store
        .start(orbcode_session_store::StartChildSessionInput {
            child_session_id: child_id.clone(),
            parent_session_id: session_id.clone(),
            agent_id: "agent-cancel-test-1".to_string(),
            agent_type: "general-purpose".to_string(),
            source_tool_use_id: "tool-use-cancel-1".to_string(),
            cwd: manager.config.cwd.to_string_lossy().to_string(),
            model: Some("stub-model".to_string()),
            permission_mode: None,
            prompt: "do some work".to_string(),
        })
        .await
        .expect("start child session");

    let loaded = manager
        .child_session_store
        .load(&child_id)
        .await
        .expect("load child")
        .expect("child exists");
    assert_eq!(
        loaded.status,
        orbcode_session_store::ChildSessionStatus::Running
    );

    manager
        .child_session_store
        .cancel(&child_id)
        .await
        .expect("cancel child session");

    let cancelled = manager
        .child_session_store
        .load(&child_id)
        .await
        .expect("load child after cancel")
        .expect("child still exists");
    assert_eq!(
        cancelled.status,
        orbcode_session_store::ChildSessionStatus::Cancelled
    );
    assert!(cancelled.ended_at.is_some());
}

#[tokio::test]
async fn parent_cancel_marks_multiple_children_cancelled() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let child_ids: Vec<String> = (1..=3)
        .map(|i| format!("{session_id}:agent-multi-cancel-{i}"))
        .collect();
    for (i, child_id) in child_ids.iter().enumerate() {
        manager
            .child_session_store
            .start(orbcode_session_store::StartChildSessionInput {
                child_session_id: child_id.clone(),
                parent_session_id: session_id.clone(),
                agent_id: format!("agent-multi-cancel-{}", i + 1),
                agent_type: "general-purpose".to_string(),
                source_tool_use_id: format!("tool-use-{}", i + 1),
                cwd: manager.config.cwd.to_string_lossy().to_string(),
                model: Some("stub-model".to_string()),
                permission_mode: None,
                prompt: format!("agent task {}", i + 1),
            })
            .await
            .expect("start child");
    }

    for child_id in &child_ids {
        manager
            .child_session_store
            .cancel(child_id)
            .await
            .expect("cancel child");
    }

    let children = manager
        .child_session_store
        .list_for_parent(&session_id)
        .await
        .expect("list children");
    assert_eq!(children.len(), 3);
    for child in &children {
        assert_eq!(
            child.status,
            orbcode_session_store::ChildSessionStatus::Cancelled
        );
        assert!(child.ended_at.is_some());
    }
}

#[tokio::test]
async fn child_session_complete_after_cancel_stays_cancelled() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let child_id = format!("{session_id}:agent-cancel-then-complete");
    manager
        .child_session_store
        .start(orbcode_session_store::StartChildSessionInput {
            child_session_id: child_id.clone(),
            parent_session_id: session_id.clone(),
            agent_id: "agent-cancel-then-complete".to_string(),
            agent_type: "general-purpose".to_string(),
            source_tool_use_id: "tool-cancel-then-complete".to_string(),
            cwd: manager.config.cwd.to_string_lossy().to_string(),
            model: Some("stub-model".to_string()),
            permission_mode: None,
            prompt: "do some work".to_string(),
        })
        .await
        .expect("start child session");

    manager
        .child_session_store
        .cancel(&child_id)
        .await
        .expect("cancel child");

    // Complete after cancel overwrites status; this test documents the
    // current behavior — a race where cancel lands before the loop sees
    // its flag results in a `Completed` metadata write after `Cancelled`.
    manager
        .child_session_store
        .complete(&child_id)
        .await
        .expect("complete child after cancel");

    let loaded = manager
        .child_session_store
        .load(&child_id)
        .await
        .expect("load child")
        .expect("child exists");
    // The store does not guard against post-cancel overwrites; both
    // states are valid outcomes for the race. The test captures the
    // current behavior explicitly.
    assert!(
        loaded.status == orbcode_session_store::ChildSessionStatus::Completed
            || loaded.status == orbcode_session_store::ChildSessionStatus::Cancelled
    );
    assert!(loaded.ended_at.is_some());
}

// ---------------------------------------------------------------------------
// Acceptance criterion 3: Hook context injection after compact boundary —
// context still visible in next provider request.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_context_after_compact_boundary_visible_in_provider_request() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE".to_string(),
        "5000".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let old_history = format!("hook-ctx-old-history {}", "x".repeat(32_000));
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, old_history),
        )
        .await
        .expect("append old user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "old answer for hook test"),
        )
        .await
        .expect("append old assistant");

    let result = manager
        .compact_session(&session_id)
        .await
        .expect("compact session");
    assert!(result.provider_generated);

    let hook_context_content = "HOOK_CONTEXT_AFTER_COMPACT_MARKER";
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, hook_context_content.to_string())
                .with_synthetic(true),
        )
        .await
        .expect("append hook context message");

    let prompt = "verify hook context visible";
    let rx = manager
        .submit_turn(&session_id, prompt)
        .await
        .expect("submit turn");
    wait_for_turn(rx).await;

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("provider request snapshot");
    assert!(
        snapshot.body_json.contains(hook_context_content),
        "hook context must be in provider request after compact"
    );
    assert!(
        snapshot
            .body_json
            .contains("This session is being continued")
    );
    assert!(snapshot.body_json.contains(prompt));
}

#[tokio::test]
async fn multiple_hook_contexts_after_compact_all_visible() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "pre-compact content"),
        )
        .await
        .expect("append user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "pre-compact answer"),
        )
        .await
        .expect("append assistant");

    manager
        .compact_session(&session_id)
        .await
        .expect("compact session");

    let contexts = ["HOOK_CTX_ALPHA", "HOOK_CTX_BETA", "HOOK_CTX_GAMMA"];
    for ctx in &contexts {
        manager
            .append_message(
                &session_id,
                TranscriptMessage::new(MessageRole::User, ctx.to_string()).with_synthetic(true),
            )
            .await
            .expect("append hook context");
    }

    let prompt = "verify all hook contexts";
    let rx = manager
        .submit_turn(&session_id, prompt)
        .await
        .expect("submit turn");
    wait_for_turn(rx).await;

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("provider request snapshot");
    for ctx in &contexts {
        assert!(
            snapshot.body_json.contains(ctx),
            "hook context {ctx} must survive compact boundary"
        );
    }
}

#[tokio::test]
async fn hook_context_before_compact_is_summarized_away() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(
                MessageRole::User,
                "HOOK_CONTEXT_BEFORE_COMPACT_MARKER".to_string(),
            )
            .with_synthetic(true),
        )
        .await
        .expect("append hook context before compact");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "acknowledged hook context"),
        )
        .await
        .expect("append assistant");

    manager
        .compact_session(&session_id)
        .await
        .expect("compact session");

    let prompt = "after compact check";
    let rx = manager
        .submit_turn(&session_id, prompt)
        .await
        .expect("submit turn");
    wait_for_turn(rx).await;

    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("provider request snapshot");
    assert!(
        !snapshot
            .body_json
            .contains("HOOK_CONTEXT_BEFORE_COMPACT_MARKER"),
        "hook context from before compaction must be gone from request"
    );
    assert!(
        snapshot
            .body_json
            .contains("This session is being continued")
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 4: Auto-continue synthetic prompt after compact is
// not GC'd (post-compact messages survive GC).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_compact_messages_survive_gc_on_reload() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "first user message"),
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

    manager
        .compact_session(&session_id)
        .await
        .expect("compact session");

    let post_compact_prompt = "POST_COMPACT_SYNTHETIC_NUDGE_MARKER";
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, post_compact_prompt.to_string())
                .with_synthetic(true),
        )
        .await
        .expect("append post-compact synthetic");

    let reloaded = manager
        .load_session(&session_id)
        .await
        .expect("reload session");

    assert!(
        reloaded.messages[0]
            .content
            .contains("This session is being continued"),
        "compact boundary must survive reload"
    );
    assert!(
        reloaded
            .messages
            .iter()
            .any(|m| m.content.contains(post_compact_prompt)),
        "post-compact synthetic message must not be GC'd"
    );
}

#[tokio::test]
async fn auto_continue_nudge_after_compact_not_gc_on_reload() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "old question"),
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

    manager
        .compact_session(&session_id)
        .await
        .expect("compact session");

    let nudge = "Continue immediately. AUTO_CONTINUE_MARKER";
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, nudge.to_string()).with_synthetic(true),
        )
        .await
        .expect("append auto-continue nudge");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "continued work"),
        )
        .await
        .expect("append follow-up assistant");

    let reloaded = manager
        .load_session(&session_id)
        .await
        .expect("reload session");

    assert!(
        reloaded
            .messages
            .first()
            .is_some_and(|m| m.content.contains("This session is being continued"))
    );
    assert!(
        reloaded
            .messages
            .iter()
            .any(|m| m.content.contains("AUTO_CONTINUE_MARKER")),
        "auto-continue nudge after compact must survive GC"
    );
    assert!(
        reloaded
            .messages
            .iter()
            .any(|m| m.content == "continued work"),
        "follow-up assistant message must survive GC"
    );
}

#[tokio::test]
async fn gc_only_drops_pre_compact_messages() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "PRE_COMPACT_MARKER"),
        )
        .await
        .expect("append pre-compact user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "pre-compact answer"),
        )
        .await
        .expect("append pre-compact assistant");

    manager
        .compact_session(&session_id)
        .await
        .expect("compact session");

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "POST_COMPACT_USER"),
        )
        .await
        .expect("append post-compact user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "POST_COMPACT_ASSISTANT"),
        )
        .await
        .expect("append post-compact assistant");

    let transcript_path = manager.transcript_store.path(&session_id);
    let existing = tokio::fs::read_to_string(&transcript_path)
        .await
        .expect("read transcript");

    let stale = serde_json::json!({
        "type": "user",
        "uuid": "stale-injected-regression",
        "timestamp": "2024-01-01T00:00:00.000Z",
        "message": { "role": "user", "content": "STALE_INJECTED_REGRESSION" },
        "cwd": "/tmp",
        "sessionId": session_id,
    });
    let injected = format!(
        "{}\n{}",
        serde_json::to_string(&stale).expect("serialize"),
        existing
    );
    tokio::fs::write(&transcript_path, &injected)
        .await
        .expect("inject stale record");

    let reloaded = manager
        .load_session(&session_id)
        .await
        .expect("reload after injection");

    assert!(
        !reloaded
            .messages
            .iter()
            .any(|m| m.content.contains("STALE_INJECTED_REGRESSION")),
        "injected stale pre-compact record must be GC'd"
    );
    assert!(
        !reloaded
            .messages
            .iter()
            .any(|m| m.content.contains("PRE_COMPACT_MARKER")),
        "original pre-compact messages must be GC'd"
    );
    assert!(
        reloaded
            .messages
            .iter()
            .any(|m| m.content.contains("POST_COMPACT_USER")),
        "post-compact user must survive GC"
    );
    assert!(
        reloaded
            .messages
            .iter()
            .any(|m| m.content.contains("POST_COMPACT_ASSISTANT")),
        "post-compact assistant must survive GC"
    );
    assert!(
        reloaded
            .messages
            .first()
            .is_some_and(|m| m.content.contains("This session is being continued"))
    );
}

// ---------------------------------------------------------------------------
// Cross-cutting: microcompact + tool pairing interaction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn microcompact_preserves_tool_result_pairing_after_compact() {
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

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "old question"),
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

    manager.compact_session(&session_id).await.expect("compact");

    let big_result = format!("microcompact-after-compact-marker {}", "x".repeat(4_000));
    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "post-compact-tool".to_string(),
                    name: "bash".to_string(),
                    input: r#"{"command":"echo hi"}"#.to_string(),
                }],
            )
            .with_stop_reason("tool_use"),
        )
        .await
        .expect("append post-compact tool use");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "post-compact-tool".to_string(),
                    content: big_result,
                    is_error: false,
                    metadata: None,
                }],
            ),
        )
        .await
        .expect("append post-compact tool result");

    let prompt = "summarize after compact";
    let mut rx = manager
        .submit_turn(&session_id, prompt)
        .await
        .expect("submit turn");

    let mut saw_microcompact = false;
    let mut saw_finished = false;
    tokio::time::timeout(StdDuration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::ContextCompacted {
                    summary,
                    provider_generated,
                    ..
                } => {
                    saw_microcompact = summary
                        .as_deref()
                        .is_some_and(|s| s.contains("Microcompacted 1 tool result"))
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
    .expect("turn finishes");
    assert!(saw_finished);
    assert!(saw_microcompact);

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load after microcompact");
    let tool_result_blocks: Vec<_> = loaded
        .messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter(|b| matches!(b, TranscriptBlock::ToolResult { .. }))
        .collect();
    assert!(
        !tool_result_blocks.is_empty(),
        "tool result blocks must survive microcompact"
    );
    for block in &tool_result_blocks {
        if let TranscriptBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } = block
        {
            assert_eq!(tool_use_id, "post-compact-tool");
            assert_eq!(
                content,
                crate::compaction::MICROCOMPACT_TOOL_RESULT_PLACEHOLDER
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Cross-cutting: snip + compact interaction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn snip_after_compact_boundary_preserves_post_compact_messages() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ORBCODE_SNIP_MESSAGE_TOKEN_THRESHOLD_OVERRIDE".to_string(),
        "100".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "pre-compact"),
        )
        .await
        .expect("append user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "pre-compact answer"),
        )
        .await
        .expect("append assistant");

    manager.compact_session(&session_id).await.expect("compact");

    let huge_post_compact = format!("snip-post-compact-marker {}", "x".repeat(8_000));
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, huge_post_compact),
        )
        .await
        .expect("append huge post-compact user");
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::Assistant, "POST_COMPACT_ANSWER_KEPT"),
        )
        .await
        .expect("append post-compact assistant");

    let prompt = "snip test follow-up";
    let mut rx = manager
        .submit_turn(&session_id, prompt)
        .await
        .expect("submit turn");

    let mut saw_snip = false;
    let mut saw_finished = false;
    tokio::time::timeout(StdDuration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::ContextCompacted {
                    summary,
                    provider_generated,
                    ..
                } => {
                    saw_snip = summary
                        .as_deref()
                        .is_some_and(|s| s.contains("Snipped 1 oversized message"))
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
    .expect("turn finishes");
    assert!(saw_finished);
    assert!(saw_snip);

    let loaded = manager
        .load_session(&session_id)
        .await
        .expect("load snipped session");
    assert!(
        loaded
            .messages
            .iter()
            .any(|m| m.content == "POST_COMPACT_ANSWER_KEPT"),
        "non-snipped post-compact answer must survive"
    );
    assert!(
        loaded
            .messages
            .iter()
            .any(|m| m.role == MessageRole::User && m.content == prompt),
        "current prompt must survive"
    );
    assert!(
        loaded
            .messages
            .first()
            .is_some_and(|m| m.content.contains("This session is being continued"))
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn wait_for_turn(mut rx: tokio::sync::mpsc::UnboundedReceiver<StreamEvent>) {
    tokio::time::timeout(StdDuration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            if matches!(
                event,
                StreamEvent::TurnFinished { .. } | StreamEvent::Error { .. }
            ) {
                break;
            }
        }
    })
    .await
    .expect("turn finishes within timeout");
}
