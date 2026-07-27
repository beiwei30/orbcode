use super::support::*;
use super::*;

#[tokio::test]
async fn stop_failure_hook_runs_for_oversized_turn() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE".to_string(),
        "1".to_string(),
    );
    let marker_path = manager.config.cwd.join("stop-failure-hook-input.json");
    manager.config.settings.hooks.insert(
        "StopFailure".to_string(),
        vec![HookMatcher {
            matcher: Some("prompt_too_long".to_string()),
            hooks: vec![HookCommand::Command {
                command: format!("cat > '{}'", marker_path.display()),
                r#if: None,
                timeout: Some(5.0),
            }],
        }],
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
        if let StreamEvent::Error { message, .. } = event {
            saw_prompt_too_long =
                message.contains("Prompt is too long") && message.contains("blocking limit of 1");
            break;
        }
    }

    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("stop failure hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");

    assert!(saw_prompt_too_long);
    assert_eq!(hook_input["hook_event_name"], "StopFailure");
    assert_eq!(hook_input["error"], "prompt_too_long");
    assert!(
        hook_input["error_details"]
            .as_str()
            .is_some_and(|details| details.contains("Prompt is too long")
                && details.contains("blocking limit of 1")),
        "{hook_input:#?}"
    );
    assert_eq!(hook_input["last_assistant_message"], Value::Null);
}

#[tokio::test]
async fn pre_tool_hook_denies_without_permission_prompt() {
    let mut manager = test_manager().await;
    manager.config.settings.hooks.insert(
            "PreToolUse".to_string(),
            vec![HookMatcher {
                matcher: Some("bash".to_string()),
                hooks: vec![HookCommand::Command {
                    command: r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"blocked by test hook"}}'"#.to_string(),
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
            TranscriptMessage::new(MessageRole::User, "hook deny test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-hook-deny",
            "bash",
            r#"{"command":"printf denied"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_permission_request = false;
    let mut saw_hook_denial = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::PermissionRequested { .. } => saw_permission_request = true,
            StreamEvent::UserMessage { message } => {
                saw_hook_denial = message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult { content, is_error, .. }
                            if content.contains("blocked by test hook") && *is_error
                    )
                });
            }
            _ => {}
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(!saw_permission_request);
    assert!(saw_hook_denial);
}

#[tokio::test]
async fn pre_tool_hook_invalid_stdout_denies_without_permission_prompt() {
    let mut manager = test_manager().await;
    manager.config.settings.hooks.insert(
        "PreToolUse".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: r"printf '%s' 'not json'".to_string(),
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
            TranscriptMessage::new(MessageRole::User, "hook invalid stdout test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-hook-invalid-stdout",
            "bash",
            r#"{"command":"printf denied"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_permission_request = false;
    let mut saw_schema_denial = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::PermissionRequested { .. } => saw_permission_request = true,
            StreamEvent::UserMessage { message } => {
                saw_schema_denial = message.blocks.iter().any(|block| {
                        matches!(
                            block,
                            TranscriptBlock::ToolResult { content, is_error, .. }
                                if content.contains("PreToolUse hook returned invalid JSON") && *is_error
                        )
                    });
            }
            _ => {}
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(!saw_permission_request);
    assert!(saw_schema_denial);
}

#[tokio::test]
async fn pre_tool_hook_failure_denies_with_stderr_without_permission_prompt() {
    let mut manager = test_manager().await;
    manager.config.settings.hooks.insert(
        "PreToolUse".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: r"printf '%s' 'hook crashed' >&2; exit 1".to_string(),
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
            TranscriptMessage::new(MessageRole::User, "hook failure test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-hook-failure",
            "bash",
            r#"{"command":"printf denied"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_permission_request = false;
    let mut saw_failure_denial = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::PermissionRequested { .. } => saw_permission_request = true,
            StreamEvent::UserMessage { message } => {
                saw_failure_denial = message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult { content, is_error, .. }
                            if content.contains("hook crashed") && *is_error
                    )
                });
            }
            _ => {}
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(!saw_permission_request);
    assert!(saw_failure_denial);
}

#[cfg(unix)]
#[tokio::test]
async fn pre_tool_hook_timeout_kills_hook_process() {
    let mut manager = test_manager().await;
    let marker_path = manager.config.cwd.join("timed-out-hook-marker");
    manager.config.settings.hooks.insert(
        "PreToolUse".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: format!("sleep 0.2; printf done > '{}'", marker_path.display()),
                r#if: None,
                timeout: Some(0.05),
            }],
        }],
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "hook timeout test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-hook-timeout",
            "bash",
            r#"{"command":"printf denied"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_timeout_denial = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            saw_timeout_denial = message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult { content, is_error, .. }
                        if content.contains("PreToolUse hook timed out") && *is_error
                )
            });
        }
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(saw_timeout_denial);
    assert!(!marker_path.exists());
}

#[tokio::test]
async fn pre_tool_hook_allows_without_permission_prompt() {
    let mut manager = test_manager().await;
    manager.config.settings.hooks.insert(
            "PreToolUse".to_string(),
            vec![HookMatcher {
                matcher: Some("bash".to_string()),
                hooks: vec![HookCommand::Command {
                    command: r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"allowed by test hook"}}'"#.to_string(),
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
            TranscriptMessage::new(MessageRole::User, "hook allow test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-hook-allow",
            "bash",
            r#"{"command":"printf allowed"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_permission_request = false;
    let mut saw_success = false;
    let mut saw_output = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::PermissionRequested { .. } => saw_permission_request = true,
            StreamEvent::ToolUseCompleted { kind, .. } => {
                saw_success = kind == ToolUseCompletionKind::Success;
            }
            StreamEvent::UserMessage { message } => {
                saw_output = message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult { content, is_error, .. }
                            if content.contains("allowed") && !*is_error
                    )
                });
            }
            _ => {}
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Continue);
    assert!(!saw_permission_request);
    assert!(saw_success);
    assert!(saw_output);
}

#[tokio::test]
async fn pre_tool_hook_additional_context_is_appended_for_next_turn() {
    let mut manager = test_manager().await;
    manager.config.settings.hooks.insert(
            "PreToolUse".to_string(),
            vec![HookMatcher {
                matcher: Some("bash".to_string()),
                hooks: vec![HookCommand::Command {
                    command: r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","additionalContext":"inspect output carefully"}}'"#.to_string(),
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
            TranscriptMessage::new(MessageRole::User, "pre hook context test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-pre-hook-context",
            "bash",
            r#"{"command":"printf pre-context-ok"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_context = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event
            && message.content == "PreToolUse hook context:\ninspect output carefully"
        {
            saw_context = true;
        }
    }
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");

    assert_eq!(outcome, ToolUseOutcome::Continue);
    assert!(saw_context);
    assert!(
        saved
            .messages
            .iter()
            .any(|message| message.content == "PreToolUse hook context:\ninspect output carefully")
    );
}

#[tokio::test]
async fn pre_tool_hook_can_update_input_before_allowing() {
    let mut manager = test_manager().await;
    manager.config.settings.hooks.insert(
            "PreToolUse".to_string(),
            vec![HookMatcher {
                matcher: Some("bash".to_string()),
                hooks: vec![HookCommand::Command {
                    command: r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"command":"printf updated"}}}'"#.to_string(),
                    r#if: Some("Bash(printf:*)".to_string()),
                    timeout: Some(5.0),
                }],
            }],
        );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "hook update test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-hook-update",
            "bash",
            r#"{"command":"printf original"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_updated_output = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            saw_updated_output = message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult { content, is_error, .. }
                            if content.contains("updated") && !content.contains("original") && !*is_error
                    )
                });
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Continue);
    assert!(saw_updated_output);
}

#[tokio::test]
async fn post_tool_hook_runs_after_successful_tool() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let marker_path = manager.config.cwd.join("post-tool-hook-input.json");
    manager.config.settings.hooks.insert(
        "PostToolUse".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: format!("cat > '{}'", marker_path.display()),
                r#if: Some("Bash(printf:*)".to_string()),
                timeout: Some(5.0),
            }],
        }],
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "post hook test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-post-hook",
            "bash",
            r#"{"command":"printf post-hook-ok"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_success = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::ToolUseCompleted { kind, .. } = event {
            saw_success = kind == ToolUseCompletionKind::Success;
        }
    }
    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("post hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");

    assert_eq!(outcome, ToolUseOutcome::Continue);
    assert!(saw_success);
    assert_eq!(hook_input["hook_event_name"], "PostToolUse");
    assert_eq!(hook_input["tool_name"], "bash");
    assert_eq!(hook_input["tool_use_id"], "tool-post-hook");
    assert_eq!(hook_input["tool_input"]["command"], "printf post-hook-ok");
    assert_eq!(hook_input["tool_response"]["success"], true);
    assert_eq!(hook_input["tool_response"]["output"], "post-hook-ok");
}

#[tokio::test]
async fn post_tool_hook_additional_context_is_appended_for_next_turn() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let marker_path = manager.config.cwd.join("post-tool-context-hook-input.json");
    manager.config.settings.hooks.insert(
            "PostToolUse".to_string(),
            vec![HookMatcher {
                matcher: Some("bash".to_string()),
                hooks: vec![HookCommand::Command {
                    command: format!(
                        "cat > '{}'; printf '%s' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"check generated output\"}}}}'",
                        marker_path.display()
                    ),
                    r#if: Some("Bash(printf:*)".to_string()),
                    timeout: Some(5.0),
                }],
            }],
        );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "post hook context test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-post-hook-context",
            "bash",
            r#"{"command":"printf context-ok"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_context = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event
            && message.content == "PostToolUse hook context:\ncheck generated output"
        {
            saw_context = true;
        }
    }
    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("post hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");

    assert_eq!(outcome, ToolUseOutcome::Continue);
    assert!(saw_context);
    assert_eq!(hook_input["hook_event_name"], "PostToolUse");
    assert_eq!(
        saved
            .messages
            .last()
            .map(|message| message.content.as_str()),
        Some("PostToolUse hook context:\ncheck generated output")
    );
}

#[tokio::test]
async fn post_tool_failure_hook_runs_after_failed_tool() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let marker_path = manager.config.cwd.join("post-tool-failure-hook-input.json");
    manager.config.settings.hooks.insert(
        "PostToolUseFailure".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: format!("cat > '{}'", marker_path.display()),
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
            TranscriptMessage::new(MessageRole::User, "post failure hook test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-post-failure-hook",
            "bash",
            r#"{"command":"printf fail >&2; exit 7"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_failure = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::ToolUseCompleted { kind, .. } = event {
            saw_failure = kind == ToolUseCompletionKind::ExecutionFailed;
        }
    }
    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("post failure hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");

    assert_eq!(outcome, ToolUseOutcome::Continue);
    assert!(saw_failure);
    assert_eq!(hook_input["hook_event_name"], "PostToolUseFailure");
    assert_eq!(hook_input["tool_name"], "bash");
    assert_eq!(hook_input["tool_use_id"], "tool-post-failure-hook");
    assert_eq!(
        hook_input["tool_input"]["command"],
        "printf fail >&2; exit 7"
    );
    assert_eq!(hook_input["is_interrupt"], false);
    assert!(
        hook_input["error"]
            .as_str()
            .is_some_and(|error| error.contains("fail") && error.contains("exit 7")),
        "{hook_input:#?}"
    );
}

#[tokio::test]
async fn post_tool_failure_hook_additional_context_is_appended_for_next_turn() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    manager.config.settings.hooks.insert(
            "PostToolUseFailure".to_string(),
            vec![HookMatcher {
                matcher: Some("bash".to_string()),
                hooks: vec![HookCommand::Command {
                    command: r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PostToolUseFailure","additionalContext":"explain the failure and retry differently"}}'"#.to_string(),
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
            TranscriptMessage::new(MessageRole::User, "post failure context test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-post-failure-context",
            "bash",
            r#"{"command":"printf bad >&2; exit 8"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_context = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event
            && message.content
                == "PostToolUseFailure hook context:\nexplain the failure and retry differently"
        {
            saw_context = true;
        }
    }
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");

    assert_eq!(outcome, ToolUseOutcome::Continue);
    assert!(saw_context);
    assert_eq!(
        saved
            .messages
            .last()
            .map(|message| message.content.as_str()),
        Some("PostToolUseFailure hook context:\nexplain the failure and retry differently")
    );
}

#[tokio::test]
async fn user_prompt_submit_hook_additional_context_is_appended_before_provider_request() {
    let mut manager = test_manager().await;
    let marker_path = manager
        .config
        .cwd
        .join("user-prompt-submit-hook-input.json");
    manager.config.settings.hooks.insert(
            "UserPromptSubmit".to_string(),
            vec![HookMatcher {
                matcher: None,
                hooks: vec![HookCommand::Command {
                    command: format!(
                        "cat > '{}'; printf '%s' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"UserPromptSubmit\",\"additionalContext\":\"prefer concise answers\"}}}}'",
                        marker_path.display()
                    ),
                    r#if: None,
                    timeout: Some(5.0),
                }],
            }],
        );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, "answer briefly")
        .await
        .expect("submit turn");

    let mut saw_context_before_request = false;
    let mut saw_hook_progress_before_request = false;
    let mut saw_request = false;
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::HookProgress {
                    hook_event_name,
                    progress,
                    ..
                } if hook_event_name == "UserPromptSubmit" => {
                    saw_hook_progress_before_request = !saw_request
                        && progress
                            .get("data")
                            .and_then(|data| data.get("result"))
                            .and_then(Value::as_str)
                            == Some("completed");
                }
                StreamEvent::UserMessage { message }
                    if message.content
                        == "UserPromptSubmit hook context:\nprefer concise answers" =>
                {
                    saw_context_before_request = !saw_request;
                }
                StreamEvent::RequestStarted { .. } => {
                    saw_request = true;
                }
                StreamEvent::TurnFinished { .. } => return,
                _ => {}
            }
        }
    })
    .await
    .expect("turn should finish");

    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("user prompt hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");

    assert!(saw_context_before_request);
    assert!(saw_hook_progress_before_request);
    assert_eq!(hook_input["hook_event_name"], "UserPromptSubmit");
    assert_eq!(hook_input["prompt"], "answer briefly");
    assert_eq!(
        saved
            .messages
            .get(1)
            .map(|message| message.content.as_str()),
        Some("UserPromptSubmit hook context:\nprefer concise answers")
    );
    let transcript = tokio::fs::read_to_string(manager.transcript_store.path(&session_id))
        .await
        .expect("read transcript");
    let saw_persisted_hook_progress = transcript.lines().any(|line| {
        let value = serde_json::from_str::<Value>(line).expect("parse transcript line");
        value.get("type").and_then(Value::as_str) == Some("progress")
            && value
                .get("data")
                .and_then(|data| data.get("type"))
                .and_then(Value::as_str)
                == Some("hook_progress")
            && value
                .get("data")
                .and_then(|data| data.get("hookEventName"))
                .and_then(Value::as_str)
                == Some("UserPromptSubmit")
    });
    assert!(saw_persisted_hook_progress);
}

#[tokio::test]
async fn user_prompt_submit_hook_exit_2_blocks_provider_request_and_erases_prompt() {
    let mut manager = test_manager().await;
    let marker_path = manager
        .config
        .cwd
        .join("user-prompt-submit-block-hook-input.json");
    manager.config.settings.hooks.insert(
        "UserPromptSubmit".to_string(),
        vec![HookMatcher {
            matcher: None,
            hooks: vec![HookCommand::Command {
                command: format!(
                    "cat > '{}'; printf '%s' 'blocked by prompt hook' >&2; exit 2",
                    marker_path.display()
                ),
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
            TranscriptMessage::new(MessageRole::User, "previous prompt"),
        )
        .await
        .expect("seed session");
    let mut rx = manager
        .submit_turn(&session_id, "blocked prompt")
        .await
        .expect("submit turn");

    let mut saw_request = false;
    let mut error_event = None;
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::RequestStarted { .. } => {
                    saw_request = true;
                }
                StreamEvent::Error {
                    provider, message, ..
                } => {
                    error_event = Some((provider, message));
                }
                _ => {}
            }
        }
    })
    .await
    .expect("turn should stop");

    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("user prompt hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");

    assert!(!saw_request);
    assert_eq!(hook_input["hook_event_name"], "UserPromptSubmit");
    assert_eq!(hook_input["prompt"], "blocked prompt");
    let (provider, error_message) = error_event.expect("expected hook block error");
    assert_eq!(provider, None);
    assert!(
        error_message.contains("blocked by prompt hook"),
        "{error_message}"
    );
    assert_eq!(
        saved
            .messages
            .last()
            .map(|message| message.content.as_str()),
        Some("previous prompt")
    );
    assert!(
        !saved
            .messages
            .iter()
            .any(|message| message.content == "blocked prompt")
    );
}

#[tokio::test]
async fn stop_hook_runs_after_final_assistant_message() {
    let mut manager = test_manager().await;
    let marker_path = manager.config.cwd.join("stop-hook-input.json");
    manager.config.settings.hooks.insert(
        "Stop".to_string(),
        vec![HookMatcher {
            matcher: None,
            hooks: vec![HookCommand::Command {
                command: format!("cat > '{}'", marker_path.display()),
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
            TranscriptMessage::new(MessageRole::User, "stop hook test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .handle_provider_response(
            &session_id,
            Uuid::new_v4(),
            "stop hook test",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: "Final answer done.".to_string(),
                blocks: vec![TranscriptBlock::Text {
                    text: "Final answer done.".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: TokenUsage::default(),
                deltas: chunk_response("Final answer done."),
            },
            0,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("handle response");

    let mut saw_completed = false;
    let mut saw_finished = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::AssistantMessageCompleted { message, .. } => {
                saw_completed = message.content == "Final answer done.";
            }
            StreamEvent::TurnFinished { .. } => {
                saw_finished = true;
            }
            _ => {}
        }
    }

    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("stop hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");

    assert_eq!(outcome, TurnLoopOutcome::Finished);
    assert!(saw_completed);
    assert!(saw_finished);
    assert_eq!(hook_input["hook_event_name"], "Stop");
    assert_eq!(hook_input["last_assistant_message"], "Final answer done.");
    assert_eq!(hook_input["stop_hook_active"], false);
}

#[tokio::test]
async fn stop_hook_block_decision_appends_feedback_and_continues() {
    let mut manager = test_manager().await;
    manager.config.settings.hooks.insert(
        "Stop".to_string(),
        vec![HookMatcher {
            matcher: None,
            hooks: vec![HookCommand::Command {
                command: r#"printf '%s' '{"decision":"block","reason":"Need more detail"}'"#
                    .to_string(),
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
            TranscriptMessage::new(MessageRole::User, "stop hook block test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .handle_provider_response(
            &session_id,
            Uuid::new_v4(),
            "stop hook block test",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: "Too brief.".to_string(),
                blocks: vec![TranscriptBlock::Text {
                    text: "Too brief.".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: TokenUsage::default(),
                deltas: chunk_response("Too brief."),
            },
            0,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("handle response");

    let mut saw_feedback = false;
    let mut saw_finished = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::UserMessage { message } => {
                saw_feedback = message.content == "Stop hook feedback:\nNeed more detail";
            }
            StreamEvent::TurnFinished { .. } => {
                saw_finished = true;
            }
            _ => {}
        }
    }
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("load session");

    assert_eq!(outcome, TurnLoopOutcome::StopHookContinue);
    assert!(saw_feedback);
    assert!(!saw_finished);
    assert_eq!(
        saved
            .messages
            .last()
            .map(|message| message.content.as_str()),
        Some("Stop hook feedback:\nNeed more detail")
    );
}

#[tokio::test]
async fn stop_hook_continue_false_finishes_without_feedback() {
    let mut manager = test_manager().await;
    manager.config.settings.hooks.insert(
        "Stop".to_string(),
        vec![HookMatcher {
            matcher: None,
            hooks: vec![HookCommand::Command {
                command: r#"printf '%s' '{"continue":false,"stopReason":"Done enough"}'"#
                    .to_string(),
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
            TranscriptMessage::new(MessageRole::User, "stop hook prevent test"),
        )
        .await
        .expect("seed session");
    let context = manager.context_preview().await;
    let request = manager
        .provider_request_for_session(
            &session_id,
            "stop hook prevent test",
            context,
            &[],
            true,
            true,
        )
        .await
        .expect("provider request");
    manager
        .provider_debug_trace
        .record(ProviderId::Anthropic, "turn", &request)
        .await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .handle_provider_response(
            &session_id,
            Uuid::new_v4(),
            "stop hook prevent test",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: "Final enough.".to_string(),
                blocks: vec![TranscriptBlock::Text {
                    text: "Final enough.".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: TokenUsage::default(),
                deltas: chunk_response("Final enough."),
            },
            0,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("handle response");

    let mut saw_feedback = false;
    let mut saw_finished = false;
    let mut saw_notice = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::UserMessage { message }
                if message.content.starts_with("Stop hook feedback:") =>
            {
                saw_feedback = true;
            }
            StreamEvent::HookNotice {
                hook_event_name,
                message,
                is_error,
                ..
            } => {
                saw_notice = hook_event_name == "Stop" && message == "Done enough" && !is_error;
            }
            StreamEvent::TurnFinished { .. } => {
                saw_finished = true;
            }
            _ => {}
        }
    }

    assert_eq!(outcome, TurnLoopOutcome::Finished);
    assert!(!saw_feedback);
    assert!(saw_notice);
    assert!(saw_finished);
    let snapshot = manager
        .last_provider_request_snapshot()
        .await
        .expect("last request snapshot");
    assert!(
        snapshot
            .recent_activity_json
            .contains("hook_notice_to_orbcode")
    );
    assert!(
        snapshot
            .recent_activity_json
            .contains("\"hook_event_name\": \"Stop\"")
    );
    assert!(snapshot.recent_activity_json.contains("Done enough"));
}

#[tokio::test]
async fn agent_loop_stop_hook_continue_false_stops_after_feedback_continuation() {
    let mut manager = test_manager().await;
    let marker_path = manager.config.cwd.join("stop-hook-seen-once");
    manager.config.settings.hooks.insert(
            "Stop".to_string(),
            vec![HookMatcher {
                matcher: None,
                hooks: vec![HookCommand::Command {
                    command: format!(
                        "sh -lc 'if test -f \"{}\"; then printf \"%s\" \"{{\\\"continue\\\":false,\\\"stopReason\\\":\\\"done after feedback\\\"}}\"; else touch \"{}\"; printf \"%s\" \"{{\\\"decision\\\":\\\"block\\\",\\\"reason\\\":\\\"need one more pass\\\"}}\"; fi'",
                        marker_path.display(),
                        marker_path.display()
                    ),
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
            TranscriptMessage::new(MessageRole::User, "stop hook full loop test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let first_outcome = manager
        .finish_provider_response(
            &session_id,
            Uuid::new_v4(),
            "stop hook full loop test",
            ToolRoundResponse::from_response(orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: "Too brief.".to_string(),
                blocks: vec![TranscriptBlock::Text {
                    text: "Too brief.".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: TokenUsage::default(),
                deltas: chunk_response("Too brief."),
            }),
            "Too brief.".to_string(),
            0,
            false,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("first response");
    let second_outcome = manager
        .finish_provider_response(
            &session_id,
            Uuid::new_v4(),
            "stop hook full loop test",
            ToolRoundResponse::from_response(orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: "Revised answer.".to_string(),
                blocks: vec![TranscriptBlock::Text {
                    text: "Revised answer.".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: TokenUsage::default(),
                deltas: chunk_response("Revised answer."),
            }),
            "Revised answer.".to_string(),
            0,
            true,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("second response");

    let mut feedback_count = 0;
    let mut saw_prevent_notice = false;
    let mut saw_finished = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::UserMessage { message } => {
                if message.content == "Stop hook feedback:\nneed one more pass" {
                    feedback_count += 1;
                }
            }
            StreamEvent::HookNotice {
                hook_event_name,
                message,
                is_error,
                ..
            } => {
                saw_prevent_notice |=
                    hook_event_name == "Stop" && message == "done after feedback" && !is_error;
            }
            StreamEvent::TurnFinished { .. } => {
                saw_finished = true;
                break;
            }
            _ => {}
        }
    }
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("load session");

    assert_eq!(first_outcome, TurnLoopOutcome::StopHookContinue);
    assert_eq!(second_outcome, TurnLoopOutcome::Finished);
    assert_eq!(feedback_count, 1);
    assert!(saw_prevent_notice);
    assert!(saw_finished);
    assert_eq!(
        saved
            .messages
            .iter()
            .filter(|message| message.content == "Stop hook feedback:\nneed one more pass")
            .count(),
        1
    );
}

#[tokio::test]
async fn subagent_start_hook_returns_additional_context() {
    let mut manager = test_manager().await;
    let marker_path = manager.config.cwd.join("subagent-start-hook-input.json");
    manager.config.settings.hooks.insert(
            "SubagentStart".to_string(),
            vec![HookMatcher {
                matcher: Some("Explore".to_string()),
                hooks: vec![HookCommand::Command {
                    command: format!(
                        "cat > '{}'; printf '%s' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"SubagentStart\",\"additionalContext\":\"use repo map\"}}}}'",
                        marker_path.display()
                    ),
                    r#if: None,
                    timeout: Some(5.0),
                }],
            }],
        );

    let (tx, _) = mpsc::unbounded_channel();
    let contexts = manager
        .run_subagent_start_hooks("session-1", "agent-1", "Explore", None, &tx)
        .await;

    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("subagent start hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");

    assert_eq!(contexts, vec!["use repo map".to_string()]);
    assert_eq!(hook_input["hook_event_name"], "SubagentStart");
    assert_eq!(hook_input["agent_id"], "agent-1");
    assert_eq!(hook_input["agent_type"], "Explore");
}

#[tokio::test]
async fn subagent_stop_hook_block_decision_returns_feedback() {
    let mut manager = test_manager().await;
    let marker_path = manager.config.cwd.join("subagent-stop-hook-input.json");
    manager.config.settings.hooks.insert(
            "SubagentStop".to_string(),
            vec![HookMatcher {
                matcher: None,
                hooks: vec![HookCommand::Command {
                    command: format!(
                        "cat > '{}'; printf '%s' '{{\"decision\":\"block\",\"reason\":\"finish the checklist\"}}'",
                        marker_path.display()
                    ),
                    r#if: None,
                    timeout: Some(5.0),
                }],
            }],
        );

    let (tx, _) = mpsc::unbounded_channel();
    let outcome = manager
        .run_subagent_stop_hooks(
            "session-1",
            "agent-1",
            "session-1:agent-1",
            "Explore",
            "partial answer",
            true,
            None,
            &tx,
        )
        .await;

    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("subagent stop hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");

    assert_eq!(outcome.blocking_errors, vec!["finish the checklist"]);
    assert!(!outcome.prevent_continuation);
    assert_eq!(hook_input["hook_event_name"], "SubagentStop");
    assert_eq!(hook_input["agent_id"], "agent-1");
    assert_eq!(hook_input["agent_type"], "Explore");
    assert_eq!(hook_input["last_assistant_message"], "partial answer");
    assert_eq!(hook_input["stop_hook_active"], true);
    assert!(
        hook_input["agent_transcript_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("session-1:agent-1.jsonl"))
    );
}

#[tokio::test]
async fn subagent_start_hook_overlay_from_agent_definition_runs_alongside_settings() {
    use std::collections::BTreeMap;

    let manager = test_manager().await;
    let marker_path = manager.config.cwd.join("subagent-start-overlay-input.json");
    let mut agent_hooks: BTreeMap<String, Vec<HookMatcher>> = BTreeMap::new();
    agent_hooks.insert(
            "SubagentStart".to_string(),
            vec![HookMatcher {
                matcher: None,
                hooks: vec![HookCommand::Command {
                    command: format!(
                        "cat > '{}'; printf '%s' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"SubagentStart\",\"additionalContext\":\"agent overlay context\"}}}}'",
                        marker_path.display()
                    ),
                    r#if: None,
                    timeout: Some(5.0),
                }],
            }],
        );

    let definition = orbcode_config::AgentDefinition {
        agent_type: "Explore".to_string(),
        description: "Read-only exploration agent.".to_string(),
        prompt: "do work".to_string(),
        tools: None,
        disallowed_tools: None,
        model: None,
        permission_mode: None,
        skills: Vec::new(),
        mcp_server_names: None,
        hooks: agent_hooks,
        source: orbcode_config::AgentSource::UserSettings,
        path: None,
    };

    let (tx, _) = mpsc::unbounded_channel();
    let contexts = manager
        .run_subagent_start_hooks("session-1", "agent-1", "Explore", Some(&definition), &tx)
        .await;

    assert!(
        contexts.iter().any(|c| c == "agent overlay context"),
        "expected overlay context, got {contexts:?}"
    );
    assert!(marker_path.exists(), "agent overlay hook should have run");
}

#[tokio::test]
async fn subagent_start_hook_overlay_skipped_for_untrusted_project_agent() {
    use std::collections::BTreeMap;

    let mut manager = test_manager().await;
    manager.config.trusted_project = false;
    let marker_path = manager
        .config
        .cwd
        .join("subagent-start-overlay-untrusted-input.json");
    let mut agent_hooks: BTreeMap<String, Vec<HookMatcher>> = BTreeMap::new();
    agent_hooks.insert(
            "SubagentStart".to_string(),
            vec![HookMatcher {
                matcher: None,
                hooks: vec![HookCommand::Command {
                    command: format!(
                        "cat > '{}'; printf '%s' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"SubagentStart\",\"additionalContext\":\"should not run\"}}}}'",
                        marker_path.display()
                    ),
                    r#if: None,
                    timeout: Some(5.0),
                }],
            }],
        );

    let definition = orbcode_config::AgentDefinition {
        agent_type: "Explore".to_string(),
        description: "Read-only exploration agent.".to_string(),
        prompt: "do work".to_string(),
        tools: None,
        disallowed_tools: None,
        model: None,
        permission_mode: None,
        skills: Vec::new(),
        mcp_server_names: None,
        hooks: agent_hooks,
        source: orbcode_config::AgentSource::ProjectSettings,
        path: None,
    };

    let (tx, _) = mpsc::unbounded_channel();
    let contexts = manager
        .run_subagent_start_hooks("session-1", "agent-1", "Explore", Some(&definition), &tx)
        .await;

    assert!(
        contexts.is_empty(),
        "untrusted project agent overlay should be filtered; got {contexts:?}"
    );
    assert!(
        !marker_path.exists(),
        "filtered hook should not have written its marker file"
    );
}

#[tokio::test]
async fn pre_tool_hook_updated_input_cannot_bypass_configured_deny() {
    let mut manager = test_manager().await;
    manager.config.disallowed_tools = vec!["Bash(printf updated)".to_string()];
    manager.config.settings.hooks.insert(
            "PreToolUse".to_string(),
            vec![HookMatcher {
                matcher: Some("bash".to_string()),
                hooks: vec![HookCommand::Command {
                    command: r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"command":"printf updated"}}}'"#.to_string(),
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
            TranscriptMessage::new(MessageRole::User, "hook deny precedence test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-hook-deny-precedence",
            "bash",
            r#"{"command":"printf original"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_config_deny = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            saw_config_deny = message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult { content, is_error, .. }
                            if content.contains("configured deny rule after PreToolUse input update") && *is_error
                    )
                });
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(saw_config_deny);
}

#[tokio::test]
async fn pre_tool_hook_updated_input_cannot_bypass_previous_user_denial() {
    let mut manager = test_manager().await;
    manager
        .permission_runtime
        .remember_denied_tool_call("bash", r#"{"command":"printf updated"}"#)
        .await;
    manager.config.settings.hooks.insert(
            "PreToolUse".to_string(),
            vec![HookMatcher {
                matcher: Some("bash".to_string()),
                hooks: vec![HookCommand::Command {
                    command: r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"command":"printf updated"}}}'"#.to_string(),
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
            TranscriptMessage::new(MessageRole::User, "hook user-denial precedence test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-hook-user-denial-precedence",
            "bash",
            r#"{"command":"printf original"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_user_denial = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            saw_user_denial = message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult { content, is_error, .. }
                            if content.contains("previous user denial after PreToolUse input update") && *is_error
                    )
                });
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(saw_user_denial);
}

#[tokio::test]
async fn permission_denied_hook_retry_appends_model_visible_hint() {
    let mut manager = test_manager().await;
    manager.config.disallowed_tools = vec!["Bash(printf denied)".to_string()];
    let marker_path = manager.config.cwd.join("permission-denied-hook-input.json");
    manager.config.settings.hooks.insert(
            "PermissionDenied".to_string(),
            vec![HookMatcher {
                matcher: Some("bash".to_string()),
                hooks: vec![HookCommand::Command {
                    command: format!(
                        "cat > '{}'; printf '%s' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"PermissionDenied\",\"retry\":true}}}}'",
                        marker_path.display()
                    ),
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
            TranscriptMessage::new(MessageRole::User, "permission denied hook test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-permission-denied-hook",
            "bash",
            r#"{"command":"printf denied"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_denied_result = false;
    let mut saw_hook_progress = false;
    let mut saw_retry_hint = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::UserMessage { message } => {
                if message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        TranscriptBlock::ToolResult { content, is_error, .. }
                            if content.contains("configured deny rule") && *is_error
                    )
                }) {
                    saw_denied_result = true;
                }
                if message.content == PERMISSION_DENIED_RETRY_MESSAGE {
                    saw_retry_hint = true;
                }
            }
            StreamEvent::ToolProgress { progress, .. } => {
                saw_hook_progress |= progress
                    .get("data")
                    .and_then(|data| data.get("type"))
                    .and_then(Value::as_str)
                    == Some("hook_progress")
                    && progress
                        .get("data")
                        .and_then(|data| data.get("hookEventName"))
                        .and_then(Value::as_str)
                        == Some("PermissionDenied");
            }
            _ => {}
        }
    }
    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("permission denied hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(saw_denied_result);
    assert!(saw_hook_progress);
    assert!(saw_retry_hint);
    assert_eq!(hook_input["hook_event_name"], "PermissionDenied");
    assert_eq!(hook_input["tool_name"], "bash");
    assert_eq!(hook_input["tool_use_id"], "tool-permission-denied-hook");
    assert_eq!(hook_input["tool_input"]["command"], "printf denied");
    assert!(
        hook_input["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("configured deny rule"))
    );
    assert!(
        saved
            .messages
            .iter()
            .any(|message| message.content == PERMISSION_DENIED_RETRY_MESSAGE)
    );
    let metadata = saved
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .find_map(|block| match block {
            TranscriptBlock::ToolResult {
                tool_use_id,
                metadata,
                ..
            } if tool_use_id == "tool-permission-denied-hook" => metadata.as_ref(),
            _ => None,
        })
        .expect("permission denied tool result metadata");
    let metadata = serde_json::from_str::<Value>(metadata).expect("parse metadata");
    let progress_messages = metadata
        .get("progressMessages")
        .and_then(Value::as_array)
        .expect("progress messages");
    assert!(progress_messages.iter().any(|progress| {
        progress
            .get("data")
            .and_then(|data| data.get("type"))
            .and_then(Value::as_str)
            == Some("hook_progress")
            && progress
                .get("data")
                .and_then(|data| data.get("result"))
                .and_then(Value::as_str)
                == Some("completed")
    }));
}

#[tokio::test]
async fn permission_denied_hook_invalid_stdout_does_not_append_retry_hint() {
    let mut manager = test_manager().await;
    manager.config.disallowed_tools = vec!["Bash(printf denied)".to_string()];
    manager.config.settings.hooks.insert(
            "PermissionDenied".to_string(),
            vec![HookMatcher {
                matcher: Some("bash".to_string()),
                hooks: vec![HookCommand::Command {
                    command: r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PermissionDenied","retry":"yes"}}'"#.to_string(),
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
            TranscriptMessage::new(MessageRole::User, "permission denied invalid hook test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-permission-denied-invalid-hook",
            "bash",
            r#"{"command":"printf denied"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_retry_hint = false;
    let mut saw_failed_hook_progress = false;
    let mut saw_hook_error_detail = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::UserMessage { message } => {
                saw_retry_hint |= message.content == PERMISSION_DENIED_RETRY_MESSAGE;
            }
            StreamEvent::ToolProgress { progress, .. } => {
                let data = progress.get("data");
                let is_permission_denied_hook = data
                    .and_then(|data| data.get("type"))
                    .and_then(Value::as_str)
                    == Some("hook_progress")
                    && data
                        .and_then(|data| data.get("hookEventName"))
                        .and_then(Value::as_str)
                        == Some("PermissionDenied");
                saw_failed_hook_progress |= is_permission_denied_hook
                    && data
                        .and_then(|data| data.get("result"))
                        .and_then(Value::as_str)
                        == Some("failed");
                saw_hook_error_detail |= is_permission_denied_hook
                    && data
                        .and_then(|data| data.get("error"))
                        .and_then(Value::as_str)
                        .is_some_and(|error| error.contains("retry"));
            }
            _ => {}
        }
    }
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(!saw_retry_hint);
    assert!(saw_failed_hook_progress);
    assert!(saw_hook_error_detail);
    let metadata = saved
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .find_map(|block| match block {
            TranscriptBlock::ToolResult {
                tool_use_id,
                metadata,
                ..
            } if tool_use_id == "tool-permission-denied-invalid-hook" => metadata.as_ref(),
            _ => None,
        })
        .expect("permission denied invalid hook metadata");
    let metadata = serde_json::from_str::<Value>(metadata).expect("parse metadata");
    let progress_messages = metadata
        .get("progressMessages")
        .and_then(Value::as_array)
        .expect("progress messages");
    assert!(progress_messages.iter().any(|progress| {
        progress
            .get("data")
            .and_then(|data| data.get("type"))
            .and_then(Value::as_str)
            == Some("hook_progress")
            && progress
                .get("data")
                .and_then(|data| data.get("error"))
                .and_then(Value::as_str)
                .is_some_and(|error| error.contains("retry"))
    }));
    assert!(
        !saved
            .messages
            .iter()
            .any(|message| message.content == PERMISSION_DENIED_RETRY_MESSAGE)
    );
}

#[tokio::test]
async fn cancellation_interrupts_long_running_bash_tool() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let marker_path = manager
        .config
        .cwd
        .join("post-tool-interrupt-hook-input.json");
    manager.config.settings.hooks.insert(
        "PostToolUseFailure".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: format!("sleep 1; cat > '{}'", marker_path.display()),
                r#if: Some("Bash(sleep:*)".to_string()),
                timeout: Some(3.0),
            }],
        }],
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let mut rx = manager
        .submit_turn(&session_id, r#"#tool:bash {"command":"sleep 10"}"#)
        .await
        .expect("submit turn");

    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::ToolUseStarted { .. } => {
                    assert!(manager.cancel_turn(&session_id).await);
                }
                StreamEvent::ToolUseCompleted {
                    kind: ToolUseCompletionKind::Interrupted,
                    ..
                } => {
                    return;
                }
                _ => {}
            }
        }
        panic!("stream ended before interrupted completion");
    })
    .await
    .expect("interrupted completion should be emitted before waiting on hooks");
    assert!(!tokio::fs::try_exists(&marker_path).await.unwrap_or(false));

    let saw_turn_cancelled = tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(event) = rx.recv().await {
            if let StreamEvent::TurnCancelled { kind, .. } = event {
                return kind == TurnCancellationKind::ToolStage;
            }
        }
        false
    })
    .await
    .expect("bash tool cancellation should finish after hooks");
    assert!(saw_turn_cancelled);

    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("post failure interrupt hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");
    assert_eq!(hook_input["hook_event_name"], "PostToolUseFailure");
    assert_eq!(hook_input["tool_name"], "bash");
    assert_eq!(hook_input["tool_input"]["command"], "sleep 10");
    assert_eq!(hook_input["error"], INTERRUPTED_TOOL_RESULT);
    assert_eq!(hook_input["is_interrupt"], true);

    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");
    let interrupted_tool_results = saved
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult {
                    content,
                    is_error,
                    ..
                } if content == INTERRUPTED_TOOL_RESULT && *is_error
            )
        })
        .count();
    assert_eq!(interrupted_tool_results, 1);
}

#[tokio::test]
async fn local_settings_pre_tool_hook_denial_labels_source() {
    use orbcode_config::HookSource;

    let mut manager = test_manager().await;
    assert!(manager.config.trusted_project);
    manager.config.settings.hooks.insert(
        "PreToolUse".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: r"printf '%s' 'local hook crashed' >&2; exit 1".to_string(),
                r#if: None,
                timeout: Some(5.0),
            }],
        }],
    );
    manager
        .config
        .settings
        .hook_sources
        .insert("PreToolUse".to_string(), vec![HookSource::LocalSettings]);

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "source labeling test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-local-source",
            "bash",
            r#"{"command":"echo labeled"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_labeled_denial = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            for block in &message.blocks {
                if let TranscriptBlock::ToolResult {
                    content, is_error, ..
                } = block
                    && *is_error
                    && content.contains("[settings.local.json]")
                    && content.contains("local hook crashed")
                {
                    saw_labeled_denial = true;
                }
            }
        }
    }
    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(
        saw_labeled_denial,
        "denial reason should be prefixed with [settings.local.json]"
    );
}

#[tokio::test]
async fn pre_tool_hook_allow_cannot_override_configured_deny_on_original_input() {
    let mut manager = test_manager().await;
    manager.config.disallowed_tools = vec!["Bash(printf original)".to_string()];
    let marker_path = manager
        .config
        .cwd
        .join("ordering-deny-original-hook-input.json");
    manager.config.settings.hooks.insert(
        "PreToolUse".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: format!(
                    "cat > '{}'; printf '%s' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"PreToolUse\",\"permissionDecision\":\"allow\"}}}}'",
                    marker_path.display()
                ),
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
            TranscriptMessage::new(MessageRole::User, "ordering deny original input test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-ordering-deny-original",
            "bash",
            r#"{"command":"printf original"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_config_deny = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            saw_config_deny |= message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult { content, is_error, .. }
                        if content.contains("configured deny rule") && *is_error
                )
            });
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(
        saw_config_deny,
        "tool should be denied by configured deny rule"
    );
    assert!(
        !marker_path.exists(),
        "PreToolUse hook must not run when deny rule matches original input"
    );
}

#[tokio::test]
async fn pre_tool_hook_allow_cannot_override_user_denial_on_original_input() {
    let mut manager = test_manager().await;
    manager
        .permission_runtime
        .remember_denied_tool_call("bash", r#"{"command":"printf original"}"#)
        .await;
    let marker_path = manager
        .config
        .cwd
        .join("ordering-user-denial-original-hook-input.json");
    manager.config.settings.hooks.insert(
        "PreToolUse".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: format!(
                    "cat > '{}'; printf '%s' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"PreToolUse\",\"permissionDecision\":\"allow\"}}}}'",
                    marker_path.display()
                ),
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
            TranscriptMessage::new(
                MessageRole::User,
                "ordering user denial original input test",
            ),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-ordering-user-denial-original",
            "bash",
            r#"{"command":"printf original"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_user_denial = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            saw_user_denial |= message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult { content, is_error, .. }
                        if content.contains("previous user denial") && *is_error
                )
            });
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(
        saw_user_denial,
        "tool should be denied by previous user denial"
    );
    assert!(
        !marker_path.exists(),
        "PreToolUse hook must not run when user denial matches original input"
    );
}

#[tokio::test]
async fn post_tool_failure_hook_retry_appends_model_visible_hint() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let marker_path = manager
        .config
        .cwd
        .join("post-tool-failure-retry-hook-input.json");
    manager.config.settings.hooks.insert(
        "PostToolUseFailure".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: format!(
                    "cat > '{}'; printf '%s' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"PostToolUseFailure\",\"retry\":true}}}}'",
                    marker_path.display()
                ),
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
            TranscriptMessage::new(MessageRole::User, "post tool failure retry test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "tool-post-failure-retry",
            "bash",
            r#"{"command":"exit 1"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_retry_hint = false;
    let mut saw_hook_progress = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::UserMessage { message } => {
                if message.content == POST_TOOL_FAILURE_RETRY_MESSAGE {
                    saw_retry_hint = true;
                }
            }
            StreamEvent::ToolProgress { progress, .. } => {
                saw_hook_progress |= progress
                    .get("data")
                    .and_then(|data| data.get("type"))
                    .and_then(Value::as_str)
                    == Some("hook_progress")
                    && progress
                        .get("data")
                        .and_then(|data| data.get("hookEventName"))
                        .and_then(Value::as_str)
                        == Some("PostToolUseFailure");
            }
            _ => {}
        }
    }

    let hook_input = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("post tool failure retry hook should capture stdin");
    let hook_input = serde_json::from_str::<Value>(&hook_input).expect("valid hook input");
    let saved = manager
        .load_session(&session_id)
        .await
        .expect("reload session");

    assert_eq!(outcome, ToolUseOutcome::Continue);
    assert!(saw_hook_progress);
    assert!(
        saw_retry_hint,
        "retry guidance should be injected as model-visible message"
    );
    assert_eq!(hook_input["hook_event_name"], "PostToolUseFailure");
    assert_eq!(hook_input["tool_name"], "bash");
    assert_eq!(hook_input["tool_use_id"], "tool-post-failure-retry");
    assert!(
        saved
            .messages
            .iter()
            .any(|message| message.content == POST_TOOL_FAILURE_RETRY_MESSAGE),
        "retry hint should be persisted in session transcript"
    );
}

#[tokio::test]
async fn post_tool_failure_hook_retry_false_does_not_append_hint() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    manager.config.settings.hooks.insert(
        "PostToolUseFailure".to_string(),
        vec![HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PostToolUseFailure","retry":false}}'"#.to_string(),
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
            TranscriptMessage::new(MessageRole::User, "post tool failure retry false test"),
        )
        .await
        .expect("seed session");
    let (tx, mut rx) = mpsc::unbounded_channel();

    manager
        .execute_tool_use(
            &session_id,
            "tool-post-failure-no-retry",
            "bash",
            r#"{"command":"exit 1"}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute tool");

    let mut saw_retry_hint = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            saw_retry_hint |= message.content == POST_TOOL_FAILURE_RETRY_MESSAGE;
        }
    }

    assert!(
        !saw_retry_hint,
        "retry:false should not inject retry guidance"
    );
}
