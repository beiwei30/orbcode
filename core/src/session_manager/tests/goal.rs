use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use orbcode_protocol::{
    AskUserQuestionSpec, MessageRole, SessionGoalStatus, SessionGoalTurnTerminalKind, StreamEvent,
    TokenUsage, ToolUseCompletionKind, TranscriptMessage, TurnContext,
};
use uuid::Uuid;

use super::support::*;
use super::*;

fn create_request(session_id: &str, objective: &str, token_budget: Option<u64>) -> GoalSetRequest {
    GoalSetRequest {
        session_id: session_id.to_string(),
        expected_revision: None,
        replace: false,
        objective: Some(objective.to_string()),
        status: None,
        token_budget: token_budget.map(Some),
        stop_reason: None,
        authority: GoalUpdateAuthority::User,
    }
}

fn update_request(
    session_id: &str,
    revision: u64,
    status: SessionGoalStatus,
    authority: GoalUpdateAuthority,
) -> GoalSetRequest {
    GoalSetRequest {
        session_id: session_id.to_string(),
        expected_revision: Some(revision),
        replace: false,
        objective: None,
        status: Some(status),
        token_budget: None,
        stop_reason: None,
        authority,
    }
}

async fn terminal_event(started: &mut StartedGoalTurn) -> StreamEvent {
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = started.events.recv().await {
            if event.is_terminal() {
                return event;
            }
        }
        panic!("goal stream closed without terminal event");
    })
    .await
    .expect("goal terminal timeout")
}

#[tokio::test]
async fn goal_mutations_validate_cas_authority_and_replacement() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id;

    let created = manager
        .set_goal(create_request(
            &session_id,
            "  ship the feature  ",
            Some(100),
        ))
        .await
        .expect("create goal");
    assert_eq!(created.objective, "ship the feature");
    assert_eq!(created.revision, 1);
    assert_eq!(created.status, SessionGoalStatus::Active);

    let stale = manager
        .set_goal(update_request(
            &session_id,
            99,
            SessionGoalStatus::Paused,
            GoalUpdateAuthority::User,
        ))
        .await
        .expect_err("stale update rejected");
    assert!(matches!(stale, GoalError::StaleRevision { .. }));

    let paused = manager
        .set_goal(update_request(
            &session_id,
            created.revision,
            SessionGoalStatus::Paused,
            GoalUpdateAuthority::User,
        ))
        .await
        .expect("pause goal");
    let model_resume = manager
        .set_goal(update_request(
            &session_id,
            paused.revision,
            SessionGoalStatus::Active,
            GoalUpdateAuthority::Model,
        ))
        .await
        .expect_err("model cannot resume goal");
    assert!(matches!(model_resume, GoalError::InvalidTransition { .. }));

    let resumed = manager
        .set_goal(update_request(
            &session_id,
            paused.revision,
            SessionGoalStatus::Active,
            GoalUpdateAuthority::User,
        ))
        .await
        .expect("resume goal");
    let unfinished_replace = manager
        .set_goal(GoalSetRequest {
            session_id: session_id.clone(),
            expected_revision: Some(resumed.revision),
            replace: true,
            objective: Some("new model goal".to_string()),
            status: None,
            token_budget: None,
            stop_reason: None,
            authority: GoalUpdateAuthority::Model,
        })
        .await
        .expect_err("model cannot replace unfinished goal");
    assert!(matches!(unfinished_replace, GoalError::UnfinishedGoal));

    let completed = manager
        .set_goal(update_request(
            &session_id,
            resumed.revision,
            SessionGoalStatus::Complete,
            GoalUpdateAuthority::Model,
        ))
        .await
        .expect("model completes goal");
    let replacement = manager
        .set_goal(GoalSetRequest {
            session_id: session_id.clone(),
            expected_revision: Some(completed.revision),
            replace: true,
            objective: Some("new model goal".to_string()),
            status: None,
            token_budget: None,
            stop_reason: None,
            authority: GoalUpdateAuthority::Model,
        })
        .await
        .expect("model replaces completed goal");
    assert_ne!(replacement.goal_id, completed.goal_id);
    assert_eq!(replacement.revision, 1);

    assert!(manager.clear_goal(&session_id).await.expect("clear goal"));
    assert!(
        !manager
            .clear_goal(&session_id)
            .await
            .expect("clear absent goal")
    );
    assert_eq!(manager.get_goal(&session_id).await.expect("get goal"), None);
}

#[tokio::test]
async fn clear_starts_goal_free_session_and_delete_removes_goal_transcript() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let old_session_id = session.session_id;
    let old_goal = manager
        .set_goal(create_request(&old_session_id, "survive clear only", None))
        .await
        .expect("create old goal");

    let fresh = manager
        .clear_session(&old_session_id)
        .await
        .expect("clear session");
    assert_ne!(fresh.session_id, old_session_id);
    assert_eq!(
        manager
            .get_goal(&fresh.session_id)
            .await
            .expect("get fresh goal"),
        None,
        "session/clear must never copy the previous goal into the new session"
    );
    assert_eq!(
        manager
            .get_goal(&old_session_id)
            .await
            .expect("reload resumable old goal"),
        Some(old_goal),
        "clear preserves the old resumable transcript"
    );

    let (deletable, _) = manager.start_or_resume(None).await.expect("new session");
    let deletable_id = deletable.session_id;
    manager
        .set_goal(create_request(
            &deletable_id,
            "delete with transcript",
            None,
        ))
        .await
        .expect("create deletable goal");
    let transcript_path = manager.transcript_store.path(&deletable_id);
    assert!(
        tokio::fs::try_exists(&transcript_path)
            .await
            .expect("stat transcript")
    );

    manager
        .delete_acp_visible_session(&deletable_id, manager.config.cwd.clone())
        .await
        .expect("delete goal session");
    assert!(
        !tokio::fs::try_exists(&transcript_path)
            .await
            .expect("stat deleted transcript")
    );
    assert!(
        manager
            .transcript_store
            .load_session_if_present(&deletable_id)
            .await
            .expect("load deleted transcript")
            .is_none()
    );
}

#[tokio::test]
async fn clear_during_active_goal_turn_cancels_old_work_and_starts_goal_free() {
    let mut manager = test_manager().await;
    manager.config.fallback_provider = None;
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=hang".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let old_session_id = session.session_id;
    let goal = manager
        .set_goal(create_request(&old_session_id, "cancel on clear", None))
        .await
        .expect("create goal");
    let mut started = match manager
        .continue_goal_if_eligible(
            &old_session_id,
            &goal.goal_id,
            goal.revision,
            true,
            crate::TurnInteractionContext::capable("clear-owner"),
        )
        .await
        .expect("start goal")
    {
        GoalContinuationOutcome::Started(started) => started,
        GoalContinuationOutcome::NotStarted { reason, .. } => {
            panic!("goal did not start: {reason:?}")
        }
    };

    let fresh = manager
        .clear_session(&old_session_id)
        .await
        .expect("clear active goal session");
    assert!(matches!(
        terminal_event(&mut started).await,
        StreamEvent::TurnCancelled { .. }
    ));
    assert_eq!(
        manager
            .get_goal(&fresh.session_id)
            .await
            .expect("get fresh goal"),
        None
    );
    let old = manager
        .get_goal(&old_session_id)
        .await
        .expect("get old goal")
        .expect("old goal remains resumable");
    assert_eq!(old.status, SessionGoalStatus::Paused);
    assert!(
        !manager
            .active_turns
            .has_active_session(&old_session_id)
            .await
    );
}

#[cfg(unix)]
#[tokio::test]
async fn goal_persistence_failure_keeps_previous_state_and_releases_turn_gate() {
    use std::os::unix::fs::PermissionsExt;

    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id;
    let created = manager
        .set_goal(create_request(&session_id, "persist atomically", None))
        .await
        .expect("create goal");
    let path = manager.transcript_store.path(&session_id);
    tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444))
        .await
        .expect("make transcript read-only");

    let mutation = manager
        .set_goal(update_request(
            &session_id,
            created.revision,
            SessionGoalStatus::Paused,
            GoalUpdateAuthority::User,
        ))
        .await;
    assert!(
        mutation.is_err(),
        "read-only transcript must reject mutation"
    );
    let continuation = manager
        .continue_goal_if_eligible(
            &session_id,
            &created.goal_id,
            created.revision,
            true,
            crate::TurnInteractionContext::capable("write-failure"),
        )
        .await;
    assert!(
        continuation.is_err(),
        "start checkpoint failure must not expose Started"
    );

    tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .await
        .expect("restore transcript permissions");
    assert_eq!(
        manager
            .get_goal(&session_id)
            .await
            .expect("reload unchanged goal"),
        Some(created)
    );
    assert!(!manager.active_turns.has_active_session(&session_id).await);
}

#[tokio::test]
async fn concurrent_goal_continuation_starts_once_and_checkpoints_before_terminal() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id;
    let goal = manager
        .set_goal(create_request(
            &session_id,
            "finish with a tiny budget",
            Some(1),
        ))
        .await
        .expect("create goal");

    let first_manager = manager.clone();
    let second_manager = manager.clone();
    let first_session_id = session_id.clone();
    let second_session_id = session_id.clone();
    let first_goal = goal.clone();
    let second_goal = goal.clone();
    let (first, second) = tokio::join!(
        first_manager.continue_goal_if_eligible(
            &first_session_id,
            &first_goal.goal_id,
            first_goal.revision,
            true,
            crate::TurnInteractionContext::capable("first"),
        ),
        second_manager.continue_goal_if_eligible(
            &second_session_id,
            &second_goal.goal_id,
            second_goal.revision,
            true,
            crate::TurnInteractionContext::capable("second"),
        )
    );

    let outcomes = [first.expect("first result"), second.expect("second result")];
    let mut started = None;
    let mut not_started = None;
    for outcome in outcomes {
        match outcome {
            GoalContinuationOutcome::Started(turn) => started = Some(turn),
            GoalContinuationOutcome::NotStarted { reason, .. } => not_started = Some(reason),
        }
    }
    let mut started = started.expect("exactly one continuation started");
    assert!(matches!(
        not_started,
        Some(GoalNotStartedReason::StaleRevision | GoalNotStartedReason::ActiveTurn)
    ));

    let terminal = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = started.events.recv().await {
            if event.is_terminal() {
                return event;
            }
        }
        panic!("goal stream closed without terminal event");
    })
    .await
    .expect("goal turn finishes");
    assert!(matches!(terminal, StreamEvent::TurnFinished { .. }));

    let checkpoint = manager
        .get_goal(&session_id)
        .await
        .expect("load checkpoint")
        .expect("goal remains");
    assert_eq!(checkpoint.status, SessionGoalStatus::BudgetLimited);
    assert!(checkpoint.tokens_used >= 1);
    assert_eq!(
        checkpoint.last_goal_turn_id.as_deref(),
        Some(started.turn_id.as_str())
    );
}

#[tokio::test]
async fn duplicate_goal_terminal_does_not_double_count_usage() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id;
    let goal = manager
        .set_goal(create_request(&session_id, "count once", None))
        .await
        .expect("create goal");
    let usage = TokenUsage {
        input_tokens: 3,
        output_tokens: 2,
        total_tokens: 5,
        ..TokenUsage::default()
    };

    for _ in 0..2 {
        manager
            .finish_goal_turn(
                &session_id,
                &goal.goal_id,
                goal.revision,
                "same-turn",
                SessionGoalTurnTerminalKind::Finished,
                &usage,
                4,
            )
            .await
            .expect("checkpoint terminal");
    }
    let checkpoint = manager
        .get_goal(&session_id)
        .await
        .expect("load checkpoint")
        .expect("goal remains");
    assert_eq!(checkpoint.tokens_used, 5);
    assert_eq!(checkpoint.elapsed_seconds, 4);
    assert_eq!(checkpoint.revision, goal.revision + 1);
}

#[tokio::test]
async fn queued_input_and_pending_question_block_goal_continuation() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id;
    let goal = manager
        .set_goal(create_request(&session_id, "wait for the user", None))
        .await
        .expect("create goal");

    manager
        .enqueue_user_command(&session_id, "user follow-up".to_string())
        .await;
    let queued = manager
        .continue_goal_if_eligible(
            &session_id,
            &goal.goal_id,
            goal.revision,
            true,
            crate::TurnInteractionContext::capable("queued"),
        )
        .await
        .expect("continuation decision");
    assert!(matches!(
        queued,
        GoalContinuationOutcome::NotStarted {
            reason: GoalNotStartedReason::PendingUserInput,
            ..
        }
    ));
    manager.drain_queued_user_commands(&session_id).await;

    let _pending = manager.register_pending_ask_user_for_test(
        &session_id,
        "pending-goal-question",
        vec![AskUserQuestionSpec {
            id: "answer".to_string(),
            question: "Continue?".to_string(),
            header: "Continue".to_string(),
            multi_select: false,
            options: Vec::new(),
            allow_free_text: true,
            allow_annotation: false,
        }],
    );
    let pending = manager
        .continue_goal_if_eligible(
            &session_id,
            &goal.goal_id,
            goal.revision,
            true,
            crate::TurnInteractionContext::capable("pending"),
        )
        .await
        .expect("continuation decision");
    assert!(matches!(
        pending,
        GoalContinuationOutcome::NotStarted {
            reason: GoalNotStartedReason::PendingUserInput,
            ..
        }
    ));
}

#[tokio::test]
async fn dropped_goal_stream_cancels_owned_turn_and_pauses_goal() {
    let mut manager = test_manager().await;
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=hang".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id;
    let goal = manager
        .set_goal(create_request(&session_id, "do not detach", None))
        .await
        .expect("create goal");
    let started = match manager
        .continue_goal_if_eligible(
            &session_id,
            &goal.goal_id,
            goal.revision,
            true,
            crate::TurnInteractionContext::capable("disconnect-owner"),
        )
        .await
        .expect("start continuation")
    {
        GoalContinuationOutcome::Started(started) => started,
        GoalContinuationOutcome::NotStarted { reason, .. } => {
            panic!("goal did not start: {reason:?}")
        }
    };
    drop(started.events);

    let paused = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let goal = manager
                .get_goal(&session_id)
                .await
                .expect("load goal")
                .expect("goal remains");
            if goal.status == SessionGoalStatus::Paused {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("disconnect pauses goal");
    assert_eq!(
        paused.stop_reason.as_deref(),
        Some("persistent goal turn cancelled")
    );
}

#[tokio::test]
async fn provider_error_and_usage_limit_leave_recoverable_goal_status() {
    for (scenario, expected_status, expected_reason) in [
        (
            "fatal",
            SessionGoalStatus::Paused,
            "persistent goal turn failed",
        ),
        (
            "ratelimit",
            SessionGoalStatus::UsageLimited,
            "provider usage limit reached",
        ),
    ] {
        let mut manager = test_manager().await;
        manager.config.fallback_provider = None;
        manager.config.max_retries = 0;
        manager.config.settings.env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            format!("mock://anthropic?scenario={scenario}"),
        );
        let (session, _) = manager.start_or_resume(None).await.expect("create session");
        let session_id = session.session_id;
        let goal = manager
            .set_goal(create_request(
                &session_id,
                "recover after provider failure",
                None,
            ))
            .await
            .expect("create goal");
        let mut started = match manager
            .continue_goal_if_eligible(
                &session_id,
                &goal.goal_id,
                goal.revision,
                true,
                crate::TurnInteractionContext::capable(format!("provider-{scenario}")),
            )
            .await
            .expect("continuation decision")
        {
            GoalContinuationOutcome::Started(started) => started,
            GoalContinuationOutcome::NotStarted { reason, .. } => {
                panic!("goal did not start: {reason:?}")
            }
        };
        assert!(matches!(
            terminal_event(&mut started).await,
            StreamEvent::Error { .. }
        ));
        let checkpoint = manager
            .get_goal(&session_id)
            .await
            .expect("load failure checkpoint")
            .expect("goal remains");
        assert_eq!(checkpoint.status, expected_status, "scenario {scenario}");
        assert_eq!(
            checkpoint.stop_reason.as_deref(),
            Some(expected_reason),
            "scenario {scenario}"
        );
    }
}

#[tokio::test]
async fn goal_tools_are_capability_scoped_and_persist_before_success() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id;
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "manage a persistent goal"),
        )
        .await
        .expect("persist initial prompt");
    let turn_id = Uuid::new_v4();
    manager
        .active_turns
        .insert(
            &session_id,
            turn_id,
            Arc::new(AtomicBool::new(false)),
            crate::TurnInteractionContext::capable("goal-tools").with_persistent_goals(),
        )
        .await
        .expect("register capable turn");

    let request = manager
        .provider_request_for_session(
            &session_id,
            "manage goal",
            TurnContext::default(),
            &[],
            false,
            false,
        )
        .await
        .expect("provider request");
    let names = request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"get_goal"));
    assert!(names.contains(&"create_goal"));
    assert!(names.contains(&"update_goal"));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    manager
        .execute_tool_use(
            &session_id,
            "create-goal-tool-use",
            "create_goal",
            r#"{"objective":"finish the model-owned task","token_budget":100}"#,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute create_goal");
    let created = manager
        .get_goal(&session_id)
        .await
        .expect("load created goal")
        .expect("goal exists before tool success is observed");
    assert_eq!(created.objective, "finish the model-owned task");

    manager
        .execute_tool_use(
            &session_id,
            "invalid-update-tool-use",
            "update_goal",
            &serde_json::json!({
                "goal_id": created.goal_id,
                "expected_revision": created.revision,
                "status": "active"
            })
            .to_string(),
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("invalid update is a tool result");
    let unchanged = manager
        .get_goal(&session_id)
        .await
        .expect("load unchanged goal")
        .expect("goal exists");
    assert_eq!(unchanged.revision, created.revision);

    manager
        .execute_tool_use(
            &session_id,
            "blocked-update-tool-use",
            "update_goal",
            &serde_json::json!({
                "goal_id": created.goal_id,
                "expected_revision": created.revision,
                "status": "blocked",
                "stop_reason": "same dependency failed in three consecutive goal turns"
            })
            .to_string(),
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute blocked update");
    let blocked = manager
        .get_goal(&session_id)
        .await
        .expect("load blocked goal")
        .expect("goal exists");
    assert_eq!(blocked.status, SessionGoalStatus::Blocked);
    assert_eq!(
        blocked.stop_reason.as_deref(),
        Some("same dependency failed in three consecutive goal turns")
    );

    let mut saw_invalid_failure = false;
    let mut saw_blocked_success = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::ToolUseCompleted {
                tool_use_id, kind, ..
            } if tool_use_id == "invalid-update-tool-use" => {
                saw_invalid_failure = kind == ToolUseCompletionKind::ExecutionFailed;
            }
            StreamEvent::ToolUseCompleted {
                tool_use_id, kind, ..
            } if tool_use_id == "blocked-update-tool-use" => {
                saw_blocked_success = kind == ToolUseCompletionKind::Success;
            }
            _ => {}
        }
    }
    assert!(saw_invalid_failure);
    assert!(saw_blocked_success);

    manager
        .active_turns
        .clear_if_matching(&session_id, turn_id)
        .await;
    let legacy_turn_id = Uuid::new_v4();
    manager
        .active_turns
        .insert(
            &session_id,
            legacy_turn_id,
            Arc::new(AtomicBool::new(false)),
            crate::TurnInteractionContext::capable("legacy"),
        )
        .await
        .expect("register legacy turn");
    let legacy_request = manager
        .provider_request_for_session(
            &session_id,
            "ordinary turn",
            TurnContext::default(),
            &[],
            true,
            true,
        )
        .await
        .expect("legacy provider request");
    assert!(legacy_request.tools.iter().all(|tool| !matches!(
        tool.name.as_str(),
        "get_goal" | "create_goal" | "update_goal"
    )));
}

#[tokio::test]
async fn update_goal_tool_reports_identity_mismatch_when_revision_matches() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id;
    let current = manager
        .set_goal(create_request(
            &session_id,
            "report the current identity",
            None,
        ))
        .await
        .expect("create goal");

    let completion = manager
        .invoke_persistent_goal_tool_and_buffer_result(
            &session_id,
            "mismatched-goal-id",
            "update_goal",
            &serde_json::json!({
                "goal_id": "replaced-goal-id",
                "expected_revision": current.revision,
                "status": "complete"
            })
            .to_string(),
        )
        .await;

    assert!(completion.result.is_error);
    let output: serde_json::Value =
        serde_json::from_str(&completion.result.content).expect("structured goal error");
    assert_eq!(output["error"]["code"], "goal_identity_mismatch");
    assert_eq!(
        output["error"]["message"],
        format!(
            "expected goal id replaced-goal-id, but current goal id is {}",
            current.goal_id
        )
    );
    let unchanged = manager
        .get_goal(&session_id)
        .await
        .expect("load unchanged goal")
        .expect("goal remains");
    assert_eq!(unchanged.goal_id, current.goal_id);
    assert_eq!(unchanged.revision, current.revision);
    assert_eq!(unchanged.objective, current.objective);
    assert_eq!(unchanged.status, current.status);
}

#[tokio::test]
async fn budgeted_goal_completion_tool_returns_final_usage() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id;
    let goal = manager
        .set_goal(create_request(&session_id, "report final usage", Some(100)))
        .await
        .expect("create budgeted goal");
    manager
        .finish_goal_turn(
            &session_id,
            &goal.goal_id,
            goal.revision,
            "usage-turn",
            SessionGoalTurnTerminalKind::Finished,
            &TokenUsage {
                input_tokens: 7,
                output_tokens: 3,
                total_tokens: 10,
                ..TokenUsage::default()
            },
            2,
        )
        .await
        .expect("checkpoint usage");
    let current = manager
        .get_goal(&session_id)
        .await
        .expect("load current goal")
        .expect("goal");
    let completion = manager
        .invoke_persistent_goal_tool_and_buffer_result(
            &session_id,
            "complete-goal",
            "update_goal",
            &serde_json::json!({
                "goal_id": current.goal_id,
                "expected_revision": current.revision,
                "status": "complete"
            })
            .to_string(),
        )
        .await;
    assert!(!completion.result.is_error);
    let output: serde_json::Value =
        serde_json::from_str(&completion.result.content).expect("structured goal result");
    assert_eq!(output["goal"]["status"], "complete");
    assert_eq!(output["final_usage"]["tokens_used"], 10);
    assert_eq!(output["final_usage"]["token_budget"], 100);
    assert_eq!(output["final_usage"]["elapsed_seconds"], 2);
}
