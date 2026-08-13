use std::sync::{Arc, atomic::AtomicBool};

use orbcode_config::AppConfigOverrides;
use orbcode_protocol::{
    BackgroundTaskViewKind, BackgroundTaskViewStatus, MessageRole, StreamEvent,
    ToolUseCompletionKind, TranscriptBlock, TranscriptMessage,
};
use orbcode_tools::{BackgroundTaskStatus, read_background_task_record};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::support::*;
use super::*;

async fn use_full_access_for_workflow(manager: &SessionManager, session_id: &str) {
    manager
        .set_session_permission_preset(session_id, ModelPermissionPreset::FullAccess)
        .await
        .expect("set Full Access for workflow behavior test");
}

#[tokio::test]
async fn workflow_tool_requires_approval_before_creating_durable_work() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner = manager.clone();
    let runner_session_id = session_id.clone();
    let handle = tokio::spawn(async move {
        runner
            .execute_tool_use(
                &runner_session_id,
                "workflow-needs-approval",
                "Workflow",
                &json!({
                    "name": "dynamic:approval-boundary",
                    "spec": {
                        "schema_version": 1,
                        "steps": [{ "log": { "message": "must not start" } }]
                    }
                })
                .to_string(),
                &tx,
                Arc::new(AtomicBool::new(false)),
            )
            .await
    });

    let mut saw_permission_request = false;
    while let Some(event) = rx.recv().await {
        if let StreamEvent::PermissionRequested { request } = event {
            saw_permission_request = true;
            assert!(
                !manager.config.home_dir.join("workflow-runs").exists(),
                "Workflow must not create durable state before approval"
            );
            assert!(
                manager
                    .respond_to_permission_request(&request.request_id, PermissionDecision::Deny)
                    .await
            );
        }
    }

    assert!(saw_permission_request);
    assert_eq!(
        handle
            .await
            .expect("join workflow permission task")
            .expect("execute Workflow tool"),
        ToolUseOutcome::Denied
    );
    assert!(!manager.config.home_dir.join("workflow-runs").exists());
}

#[tokio::test]
async fn workflow_tool_dispatch_starts_dynamic_workflow() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        allow_network: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    use_full_access_for_workflow(&manager, &session_id).await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "workflow-tool-1",
            "Workflow",
            &json!({
                "name": "dynamic:tool",
                "arguments": "ok",
                "spec": {
                    "schema_version": 1,
                    "description": "Workflow tool check",
                    "steps": [
                        { "agent": { "description": "Task 1", "prompt": "Output MARKER." } },
                        { "log": { "message": "tool $1" } }
                    ]
                }
            })
            .to_string(),
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute Workflow tool");

    assert_eq!(outcome, ToolUseOutcome::Continue);

    let mut task_id = None;
    let mut pushed_task = None;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::BackgroundTaskUpdated { task, .. } => {
                pushed_task = Some(task);
            }
            StreamEvent::UserMessage { message } => {
                for block in message.blocks {
                    if let TranscriptBlock::ToolResult {
                        content, is_error, ..
                    } = block
                    {
                        assert!(!is_error, "Workflow tool should succeed: {content}");
                        let value: Value =
                            serde_json::from_str(&content).expect("tool result json");
                        task_id = value["task_id"].as_str().map(str::to_string);
                    }
                }
            }
            _ => {}
        }
    }
    let task_id = task_id.expect("Workflow tool result includes task_id");
    let pushed_task = pushed_task.expect("Workflow tool pushes background task snapshot");
    assert_eq!(pushed_task.task_id, task_id);
    assert_eq!(pushed_task.kind, BackgroundTaskViewKind::Workflow);

    let mut record = None;
    for _ in 0..50 {
        record = read_background_task_record(&manager.config.home_dir, &task_id)
            .await
            .expect("read workflow record");
        if record
            .as_ref()
            .is_some_and(|record| record.status == BackgroundTaskStatus::Completed)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let record = record.expect("workflow record");
    assert_eq!(record.status, BackgroundTaskStatus::Completed);
    // Result joins the agent step's stub output and the log step's interpolated
    // message ("tool $1" with $1 == "ok"); the log runs last so it ends the join.
    let result = record.result.as_deref().expect("workflow result");
    assert!(
        result.contains("local compatibility stub"),
        "agent step output should be in the result: {result}"
    );
    assert!(
        result.ends_with("tool ok"),
        "log step should interpolate $1 into the result tail: {result}"
    );

    let mut saw_step_progress = false;
    let mut saw_completed_snapshot = false;
    for _ in 0..50 {
        while let Ok(event) = rx.try_recv() {
            if let StreamEvent::BackgroundTaskUpdated { task, .. } = event {
                saw_step_progress |= task.progress_events.as_ref().is_some_and(|events| {
                    events.iter().any(|event| {
                        event.step_key.as_deref() == Some("step.0")
                            && event.event == "step_completed"
                    })
                });
                saw_completed_snapshot |= task.status == BackgroundTaskViewStatus::Completed;
            }
        }
        if saw_step_progress && saw_completed_snapshot {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        saw_step_progress,
        "Workflow runner should push step progress"
    );
    assert!(
        saw_completed_snapshot,
        "Workflow runner should push terminal task snapshot"
    );
}

#[tokio::test]
async fn workflow_tool_tolerates_misplaced_subagent_type_on_agent_step() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    use_full_access_for_workflow(&manager, &session_id).await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    manager
        .execute_tool_use(
            &session_id,
            "workflow-misplaced-subagent-type",
            "Workflow",
            &json!({
                "name": "dynamic:misplaced-subagent-type",
                "spec": {
                    "schema_version": 1,
                    "description": "Tolerate misplaced subagent_type",
                    "steps": [
                        {
                            "agent": {
                                "description": "Task 1",
                                "prompt": "Output MISPLACED_SUBAGENT_TYPE_MARKER."
                            },
                            "subagent_type": null
                        }
                    ]
                }
            })
            .to_string(),
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute Workflow tool");

    let mut task_id = None;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            for block in message.blocks {
                if let TranscriptBlock::ToolResult {
                    content, is_error, ..
                } = block
                {
                    assert!(
                        !is_error,
                        "Workflow tool should accept old model shape: {content}"
                    );
                    let value: Value = serde_json::from_str(&content).expect("tool result json");
                    task_id = value["task_id"].as_str().map(str::to_string);
                }
            }
        }
    }
    let task_id = task_id.expect("Workflow tool result includes task_id");

    for _ in 0..100 {
        let completed = read_background_task_record(&manager.config.home_dir, &task_id)
            .await
            .expect("read workflow record")
            .is_some_and(|record| record.status == BackgroundTaskStatus::Completed);
        if completed {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("workflow should complete after accepting misplaced subagent_type");
}

#[tokio::test]
async fn workflow_agent_step_persists_child_transcript() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        allow_network: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    use_full_access_for_workflow(&manager, &session_id).await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    manager
        .execute_tool_use(
            &session_id,
            "workflow-agent-transcript",
            "Workflow",
            &json!({
                "name": "dynamic:child-transcript",
                "spec": {
                    "schema_version": 1,
                    "description": "Persist workflow child transcript",
                    "steps": [
                        {
                            "agent": {
                                "description": "Persist child",
                                "prompt": "Output WORKFLOW_CHILD_TRANSCRIPT_MARKER."
                            }
                        }
                    ]
                }
            })
            .to_string(),
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute Workflow tool");

    let mut task_id = None;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            for block in message.blocks {
                if let TranscriptBlock::ToolResult {
                    content, is_error, ..
                } = block
                {
                    assert!(!is_error, "Workflow tool should succeed: {content}");
                    let value: Value = serde_json::from_str(&content).expect("tool result json");
                    task_id = value["task_id"].as_str().map(str::to_string);
                }
            }
        }
    }
    let task_id = task_id.expect("Workflow tool result includes task_id");

    for _ in 0..100 {
        let completed = read_background_task_record(&manager.config.home_dir, &task_id)
            .await
            .expect("read workflow record")
            .is_some_and(|record| record.status == BackgroundTaskStatus::Completed);
        if completed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let record = read_background_task_record(&manager.config.home_dir, &task_id)
        .await
        .expect("read workflow record")
        .expect("workflow record");
    assert_eq!(record.status, BackgroundTaskStatus::Completed);

    let children = manager
        .child_sessions_for(&session_id)
        .await
        .expect("list children");
    assert_eq!(
        children.len(),
        1,
        "workflow should create one child metadata"
    );
    let child = &children[0];
    assert_eq!(child.parent_session_id, session_id);
    assert_eq!(
        child.status,
        orbcode_session_store::ChildSessionStatus::Completed
    );
    assert_eq!(
        child.source_tool_use_id,
        format!("workflow:{task_id}:step.0")
    );

    let child_transcript_path = manager
        .child_session_store
        .transcript_path_for(&child.child_session_id);
    assert!(
        tokio::fs::try_exists(&child_transcript_path)
            .await
            .expect("stat child transcript"),
        "workflow child transcript should be persisted under child-session storage"
    );
    assert!(
        !tokio::fs::try_exists(
            manager
                .config
                .current_project_dir
                .join(format!("{}.jsonl", child.child_session_id))
        )
        .await
        .expect("stat project transcript"),
        "workflow child transcript must not be written to the top-level project transcript directory"
    );

    let (persisted, _) = manager
        .start_or_resume(Some(&child.child_session_id))
        .await
        .expect("normal resume should load persisted child transcript");
    assert_eq!(persisted.session_id, child.child_session_id);
    assert!(
        persisted
            .messages
            .iter()
            .any(|message| message.role == MessageRole::User
                && message.content.contains("WORKFLOW_CHILD_TRANSCRIPT_MARKER")),
        "persisted child transcript should include agent prompt"
    );
    assert!(
        persisted
            .messages
            .iter()
            .any(|message| message.role == MessageRole::Assistant
                && message.content.contains("local compatibility stub")),
        "persisted child transcript should include agent assistant output"
    );
    assert!(
        persisted
            .messages
            .iter()
            .all(|message| !message.is_synthetic),
        "new persisted workflow child transcript should not be fallback-synthetic"
    );

    let summaries = manager.list_sessions().await.expect("list sessions");
    assert!(
        summaries
            .iter()
            .any(|summary| summary.session_id == session_id),
        "parent remains visible in top-level session list"
    );
    assert!(
        !summaries
            .iter()
            .any(|summary| summary.session_id == child.child_session_id),
        "persisted workflow child transcript must not appear as a top-level session"
    );
    let (latest, _) = manager
        .continue_latest()
        .await
        .expect("continue latest should ignore workflow child transcripts");
    assert_eq!(latest.session_id, session_id);
}

#[tokio::test]
async fn workflow_agent_failure_keeps_persisted_child_prompt_transcript() {
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        allow_network: Some(true),
        fallback_provider: None,
        max_retries: Some(0),
        ..AppConfigOverrides::default()
    })
    .await;
    manager.config.settings.env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=fatal".to_string(),
    );
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    use_full_access_for_workflow(&manager, &session_id).await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    manager
        .execute_tool_use(
            &session_id,
            "workflow-agent-failed-transcript",
            "Workflow",
            &json!({
                "name": "dynamic:failed-child-transcript",
                "spec": {
                    "schema_version": 1,
                    "description": "Persist failed workflow child transcript",
                    "steps": [
                        {
                            "agent": {
                                "description": "Fail child",
                                "prompt": "Output WORKFLOW_CHILD_FAILURE_MARKER."
                            }
                        }
                    ]
                }
            })
            .to_string(),
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute Workflow tool");

    let mut task_id = None;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            for block in message.blocks {
                if let TranscriptBlock::ToolResult {
                    content, is_error, ..
                } = block
                {
                    assert!(!is_error, "Workflow tool should start: {content}");
                    let value: Value = serde_json::from_str(&content).expect("tool result json");
                    task_id = value["task_id"].as_str().map(str::to_string);
                }
            }
        }
    }
    let task_id = task_id.expect("Workflow tool result includes task_id");

    for _ in 0..100 {
        let failed = read_background_task_record(&manager.config.home_dir, &task_id)
            .await
            .expect("read workflow record")
            .is_some_and(|record| record.status == BackgroundTaskStatus::Failed);
        if failed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let record = read_background_task_record(&manager.config.home_dir, &task_id)
        .await
        .expect("read workflow record")
        .expect("workflow record");
    assert_eq!(record.status, BackgroundTaskStatus::Failed);

    let children = manager
        .child_sessions_for(&session_id)
        .await
        .expect("list children");
    assert_eq!(
        children.len(),
        1,
        "workflow should create one child metadata"
    );
    let child = &children[0];
    assert_eq!(
        child.status,
        orbcode_session_store::ChildSessionStatus::Failed
    );

    let child_transcript_path = manager
        .child_session_store
        .transcript_path_for(&child.child_session_id);
    assert!(
        tokio::fs::try_exists(&child_transcript_path)
            .await
            .expect("stat child transcript"),
        "failed workflow child should still persist the initial prompt transcript"
    );

    let (persisted, _) = manager
        .start_or_resume(Some(&child.child_session_id))
        .await
        .expect("resume failed child transcript");
    assert!(
        persisted
            .messages
            .iter()
            .any(|message| message.role == MessageRole::User
                && message.content.contains("WORKFLOW_CHILD_FAILURE_MARKER")),
        "failed child transcript should include the agent prompt"
    );
}

#[tokio::test]
async fn provider_workflow_tool_use_starts_generated_multi_step_spec() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    use_full_access_for_workflow(&manager, &session_id).await;
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "plan workflow"),
        )
        .await
        .expect("seed session transcript");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .handle_provider_response(
            &session_id,
            uuid::Uuid::new_v4(),
            "plan workflow",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: String::new(),
                blocks: vec![TranscriptBlock::ToolUse {
                    id: "workflow-provider-1".to_string(),
                    name: "Workflow".to_string(),
                    input: json!({
                        "name": "dynamic:provider",
                        "arguments": "ok",
                        "spec": {
                            "schema_version": 1,
                            "description": "Provider multi-step workflow check",
                            "steps": [
                                {
                                    "phase": {
                                        "name": "Phase $1",
                                        "steps": [
                                            {
                                                "parallel": {
                                                    "steps": [
                                                        {
                                                            "agent": {
                                                                "description": "Inspect $1",
                                                                "prompt": "Inspect the first branch for $1 and report concise findings."
                                                            }
                                                        },
                                                        {
                                                            "pipeline": {
                                                                "steps": [
                                                                    {
                                                                        "agent": {
                                                                            "description": "Gather $1",
                                                                            "prompt": "Gather evidence for $1 and output the facts needed by the next step."
                                                                        }
                                                                    },
                                                                    {
                                                                        "agent": {
                                                                            "description": "Synthesize $1",
                                                                            "prompt": "Synthesize the prior pipeline output for $1."
                                                                        }
                                                                    }
                                                                ]
                                                            }
                                                        }
                                                    ]
                                                }
                                            },
                                            { "log": { "message": "done $1" } }
                                        ]
                                    }
                                }
                            ]
                        }
                    })
                    .to_string(),
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
        .expect("handle provider Workflow tool_use");

    assert_eq!(outcome, TurnLoopOutcome::Continue);

    let mut saw_tool_start = false;
    let mut saw_tool_success = false;
    let mut task_id = None;
    let mut pushed_task = None;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::ToolUseStarted { tool_name, .. } => {
                saw_tool_start |= tool_name == "Workflow";
            }
            StreamEvent::ToolUseCompleted {
                tool_name, kind, ..
            } => {
                saw_tool_success |=
                    tool_name == "Workflow" && kind == ToolUseCompletionKind::Success;
            }
            StreamEvent::UserMessage { message } => {
                for block in message.blocks {
                    if let TranscriptBlock::ToolResult {
                        content, is_error, ..
                    } = block
                    {
                        assert!(!is_error, "Workflow tool should succeed: {content}");
                        let value: Value =
                            serde_json::from_str(&content).expect("tool result json");
                        task_id = value["task_id"].as_str().map(str::to_string);
                    }
                }
            }
            StreamEvent::BackgroundTaskUpdated { task, .. } => {
                pushed_task = Some(task);
            }
            _ => {}
        }
    }

    assert!(saw_tool_start);
    assert!(saw_tool_success);
    let task_id = task_id.expect("Workflow tool result includes task_id");
    let pushed_task = pushed_task.expect("provider Workflow pushes background task snapshot");
    assert_eq!(pushed_task.task_id, task_id);
    assert_eq!(pushed_task.kind, BackgroundTaskViewKind::Workflow);
}

#[tokio::test]
async fn provider_malformed_workflow_input_returns_repairable_diagnostic() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    use_full_access_for_workflow(&manager, &session_id).await;
    manager
        .append_message(
            &session_id,
            TranscriptMessage::new(MessageRole::User, "plan workflow"),
        )
        .await
        .expect("seed session transcript");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let malformed_input = r#"{"name":"dynamic:ui-verify","spec":{"description":"Verify UI workflow","schema_version":1,"steps":[{"agent":{"description":"Task 1","prompt":"task1"}},{"parallel":{"steps":[{"agent":{"description":"Task 2","prompt":"task2"}},{"agent":{"description":"Task 3","prompt":}}]}},{"agent":{"description":"Task 4","prompt":"task4"}}]}}"#;
    let outcome = manager
        .handle_provider_response(
            &session_id,
            uuid::Uuid::new_v4(),
            "plan workflow",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: String::new(),
                blocks: vec![TranscriptBlock::ToolUse {
                    id: "workflow-malformed".to_string(),
                    name: "Workflow".to_string(),
                    input: malformed_input.to_string(),
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
        .expect("handle malformed provider Workflow tool_use");

    assert_eq!(outcome, TurnLoopOutcome::Continue);
    let mut diagnostic = None;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            for block in message.blocks {
                if let TranscriptBlock::ToolResult {
                    content, is_error, ..
                } = block
                    && is_error
                {
                    diagnostic = Some(content);
                }
            }
        }
    }
    let diagnostic = diagnostic.expect("malformed Workflow emits tool_result diagnostic");
    assert!(diagnostic.contains("invalid Workflow input"));
    assert!(diagnostic.contains("expected a valid JSON object"));
    assert!(diagnostic.contains("Near error:"));
    assert!(diagnostic.contains("Task 3"));
    assert!(diagnostic.contains("For parallel agent steps"));
    assert!(
        !manager.config.home_dir.join("workflow-runs").exists(),
        "malformed Workflow tool call must not create a workflow run"
    );

    let request = manager
        .provider_request_for_session(
            &session_id,
            "plan workflow",
            manager.context_preview().await,
            &[],
            true,
            true,
        )
        .await
        .expect("provider repair request");
    assert!(
        request.messages.iter().any(|message| {
            message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult { content, is_error, .. }
                        if *is_error
                            && content.contains("expected a valid JSON object")
                            && content.contains("Task 3")
                )
            })
        }),
        "next provider request should include the repairable Workflow diagnostic"
    );

    let corrected_input = json!({
        "name": "dynamic:ui-verify",
        "spec": {
            "description": "Verify UI workflow",
            "schema_version": 1,
            "steps": [
                { "agent": { "description": "Task 1", "prompt": "Output TASK-1." } },
                {
                    "parallel": {
                        "steps": [
                            { "agent": { "description": "Task 2", "prompt": "Output TASK-2." } },
                            { "agent": { "description": "Task 3", "prompt": "Output TASK-3." } }
                        ]
                    }
                },
                { "agent": { "description": "Task 4", "prompt": "Output TASK-4." } }
            ]
        }
    })
    .to_string();
    let (tx, mut rx) = mpsc::unbounded_channel();
    manager
        .handle_provider_response(
            &session_id,
            uuid::Uuid::new_v4(),
            "plan workflow",
            orbcode_model_provider::ProviderResponse {
                provider: ProviderId::Anthropic,
                fallback_from: None,
                content: String::new(),
                blocks: vec![TranscriptBlock::ToolUse {
                    id: "workflow-corrected".to_string(),
                    name: "Workflow".to_string(),
                    input: corrected_input,
                }],
                stop_reason: Some("tool_use".to_string()),
                usage: TokenUsage::default(),
                deltas: Vec::new(),
            },
            1,
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("handle corrected provider Workflow tool_use");

    let mut task_id = None;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            for block in message.blocks {
                if let TranscriptBlock::ToolResult {
                    content, is_error, ..
                } = block
                {
                    assert!(!is_error, "corrected Workflow should succeed: {content}");
                    let value: Value = serde_json::from_str(&content).expect("tool result json");
                    task_id = value["task_id"].as_str().map(str::to_string);
                }
            }
        }
    }
    let task_id = task_id.expect("corrected Workflow returns task id");
    assert!(task_id.starts_with("workflow-"));
}

#[tokio::test]
async fn provider_request_exposes_workflow_when_tools_allowed() {
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
            TranscriptMessage::new(MessageRole::User, "plan work"),
        )
        .await
        .expect("seed session");

    let request = manager
        .provider_request_for_session(
            &session.session_id,
            "plan work",
            manager.context_preview().await,
            &[],
            true,
            true,
        )
        .await
        .expect("provider request");

    assert!(
        request.tools.iter().any(|tool| tool.name == "Workflow"),
        "Workflow should be provider-visible once tools permission is available"
    );
    assert!(
        request.system_prompt.contains("Dynamic workflow planning"),
        "Workflow planning prompt section should be present when Workflow is visible"
    );
    assert!(
        request
            .system_prompt
            .contains("call Workflow in this same turn"),
        "Workflow planning prompt should direct same-turn tool use"
    );
}

#[tokio::test]
async fn provider_request_hides_workflow_when_tools_not_allowed() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(false),
        allow_network: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    manager
        .append_message(
            &session.session_id,
            TranscriptMessage::new(MessageRole::User, "plan work"),
        )
        .await
        .expect("seed session");

    let request = manager
        .provider_request_for_session(
            &session.session_id,
            "plan work",
            manager.context_preview().await,
            &[],
            false,
            true,
        )
        .await
        .expect("provider request");

    assert!(
        !request.tools.iter().any(|tool| tool.name == "Workflow"),
        "Workflow should stay hidden when tools permission is unavailable"
    );
    assert!(
        !request.system_prompt.contains("Dynamic workflow planning"),
        "Workflow planning prompt section should be absent when Workflow is hidden"
    );
}

#[tokio::test]
async fn provider_request_omits_workflow_planning_when_workflow_denied() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        allow_network: Some(true),
        disallowed_tools: vec!["Workflow".to_string()],
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    manager
        .append_message(
            &session.session_id,
            TranscriptMessage::new(MessageRole::User, "plan work"),
        )
        .await
        .expect("seed session");

    let request = manager
        .provider_request_for_session(
            &session.session_id,
            "plan work",
            manager.context_preview().await,
            &[],
            true,
            true,
        )
        .await
        .expect("provider request");

    assert!(!request.tools.iter().any(|tool| tool.name == "Workflow"));
    assert!(
        !request.system_prompt.contains("Dynamic workflow planning"),
        "Workflow planning prompt section should be absent when Workflow is denied"
    );
}

#[tokio::test]
async fn workflow_tool_respects_configured_deny_rule_before_task_creation() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        disallowed_tools: vec!["Workflow".to_string()],
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let (tx, mut rx) = mpsc::unbounded_channel();

    let outcome = manager
        .execute_tool_use(
            &session_id,
            "workflow-tool-denied",
            "Workflow",
            &json!({
                "name": "dynamic:denied",
                "spec": {
                    "schema_version": 1,
                    "description": "Denied workflow",
                    "steps": [
                        { "log": { "message": "should not run" } }
                    ]
                }
            })
            .to_string(),
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("execute Workflow tool");

    let mut saw_denied_result = false;
    while let Ok(event) = rx.try_recv() {
        if let StreamEvent::UserMessage { message } = event {
            saw_denied_result |= message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult { content, is_error, .. }
                        if content.contains("configured deny rule") && *is_error
                )
            });
        }
    }

    assert_eq!(outcome, ToolUseOutcome::Denied);
    assert!(saw_denied_result);
    assert!(
        !manager.config.home_dir.join("workflow-runs").exists(),
        "denied Workflow tool call must not create a workflow run"
    );
}
