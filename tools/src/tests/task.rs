use super::*;
use crate::TaskStatusKind;
use crate::load_task_list_snapshot;
use crate::plan_tools::{load_plan_mode_state, workspace_plan_file_path};
use crate::session_task_list_id;
use crate::task_tools::{BackgroundTaskStatus, write_background_task_record};
use orbcode_config::PermissionMode;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::process::Command;

#[tokio::test]
async fn task_tools_persist_workspace_scoped_records() {
    let registry = ToolRegistry::foundation();
    let context = test_context("tasks").await;

    let created = registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Implement store","description":"Persist workspace tasks"}"#,
            &context,
        )
        .await
        .expect("create first task");
    assert!(created.summary.contains("Created task #1"));

    registry
        .invoke(
            "task-create",
            r#"{"subject":"Wire tool","description":"Expose task CRUD"}"#,
            &context,
        )
        .await
        .expect("create second task");
    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"2","addBlockedBy":["1"]}"#,
            &context,
        )
        .await
        .expect("add blockedBy to second task");

    let listed = registry
        .invoke("TaskList", "{}", &context)
        .await
        .expect("list tasks");
    let listed_json: Value = serde_json::from_str(&listed.output).expect("parse list output");
    assert_eq!(listed_json["tasks"][0]["id"], "1");
    assert_eq!(listed_json["tasks"][0]["subject"], "Implement store");
    assert_eq!(listed_json["tasks"][1]["id"], "2");
    assert_eq!(listed_json["tasks"][1]["subject"], "Wire tool");

    let first_task = registry
        .invoke("TaskGet", r#"{"taskId":"1"}"#, &context)
        .await
        .expect("get first task");
    let first_task_json: Value =
        serde_json::from_str(&first_task.output).expect("parse first task");
    assert_eq!(first_task_json["blocks"], json!(["2"]));

    let updated = registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"2","status":"in_progress","owner":"copilot","metadata":{"area":"tools"},"addBlocks":["1"]}"#,
            &context,
        )
        .await
        .expect("update second task");
    assert!(
        updated.output.contains("Updated task #2"),
        "expected TS-style update output, got `{}`",
        updated.output
    );
    assert!(updated.output.contains("status"));
    assert!(updated.output.contains("owner"));

    let second_task = registry
        .invoke("TaskGet", r#"{"taskId":"2"}"#, &context)
        .await
        .expect("get second task after update");
    let second_task_json: Value =
        serde_json::from_str(&second_task.output).expect("parse second task");
    assert_eq!(second_task_json["status"], "in_progress");
    assert_eq!(second_task_json["owner"], "copilot");
    assert_eq!(second_task_json["metadata"]["area"], "tools");
    assert_eq!(second_task_json["blocks"], json!(["1"]));
    assert_eq!(second_task_json["blockedBy"], json!(["1"]));

    let first_task_after_update = registry
        .invoke("task-get", r#"{"taskId":"1"}"#, &context)
        .await
        .expect("get first task after update");
    let first_task_after_update_json: Value = serde_json::from_str(&first_task_after_update.output)
        .expect("parse first task after update");
    assert_eq!(first_task_after_update_json["blockedBy"], json!(["2"]));

    registry
        .invoke(
            "task-update",
            r#"{"taskId":"2","status":"deleted"}"#,
            &context,
        )
        .await
        .expect("delete second task");

    let list_after_delete = registry
        .invoke("task-list", "{}", &context)
        .await
        .expect("list after delete");
    let list_after_delete_json: Value =
        serde_json::from_str(&list_after_delete.output).expect("parse list after delete");
    assert_eq!(list_after_delete_json["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(list_after_delete_json["tasks"][0]["id"], "1");

    let first_task_after_delete = registry
        .invoke("task-get", r#"{"taskId":"1"}"#, &context)
        .await
        .expect("get first task after delete");
    let first_task_after_delete_json: Value = serde_json::from_str(&first_task_after_delete.output)
        .expect("parse first task after delete");
    assert_eq!(first_task_after_delete_json["blocks"], json!([]));
    assert_eq!(first_task_after_delete_json["blockedBy"], json!([]));

    let third = registry
        .invoke(
            "task-create",
            r#"{"subject":"Follow-up","description":"Ensure IDs are not reused"}"#,
            &context,
        )
        .await
        .expect("create third task");
    assert!(third.summary.contains("Created task #3"));
}

#[tokio::test]
async fn task_list_output_matches_typescript_format_and_filters_completed_blockers() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-list-format").await;

    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Design","description":"Design API"}"#,
            &context,
        )
        .await
        .expect("create first task");
    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Implement","description":"Wire it up"}"#,
            &context,
        )
        .await
        .expect("create second task");
    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"2","owner":"alice","addBlockedBy":["1"]}"#,
            &context,
        )
        .await
        .expect("set owner and blockedBy on second task");

    let listed = registry
        .invoke("TaskList", "{}", &context)
        .await
        .expect("list tasks");
    let listed_json: Value = serde_json::from_str(&listed.output).expect("parse list output");
    let task2 = &listed_json["tasks"][1];
    assert_eq!(task2["id"], "2");
    assert_eq!(task2["subject"], "Implement");
    assert_eq!(task2["status"], "pending");
    assert_eq!(task2["owner"], "alice");
    assert_eq!(task2["blockedBy"], json!(["1"]));

    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"completed"}"#,
            &context,
        )
        .await
        .expect("complete first task");

    let after_complete = registry
        .invoke("TaskList", "{}", &context)
        .await
        .expect("list after completion");
    let after_json: Value =
        serde_json::from_str(&after_complete.output).expect("parse list after complete");
    let task2_after = &after_json["tasks"][1];
    assert_eq!(task2_after["owner"], "alice");
    assert_eq!(
        task2_after["blockedBy"],
        json!([]),
        "completed blocker should be filtered from blockedBy"
    );
}

#[tokio::test]
async fn task_list_reports_no_tasks_when_empty() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-list-empty").await;
    let listed = registry
        .invoke("TaskList", "{}", &context)
        .await
        .expect("list empty");
    let listed_json: Value = serde_json::from_str(&listed.output).expect("parse empty list");
    assert_eq!(listed_json["tasks"], json!([]));
}

#[tokio::test]
async fn task_update_rejects_invalid_status() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-status-validation").await;
    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Subject","description":"Description"}"#,
            &context,
        )
        .await
        .expect("create task");

    let error = registry
        .invoke("TaskUpdate", r#"{"taskId":"1","status":"bogus"}"#, &context)
        .await
        .expect_err("invalid status should fail");
    assert!(
        error
            .to_string()
            .contains("pending, in_progress, completed")
    );
}

#[tokio::test]
async fn task_ids_never_recycle_after_delete() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-id-recycle").await;
    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"One","description":"First"}"#,
            &context,
        )
        .await
        .expect("create task one");
    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Two","description":"Second"}"#,
            &context,
        )
        .await
        .expect("create task two");
    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"2","status":"deleted"}"#,
            &context,
        )
        .await
        .expect("delete task two");
    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"deleted"}"#,
            &context,
        )
        .await
        .expect("delete task one");

    let next = registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Third","description":"After deletes"}"#,
            &context,
        )
        .await
        .expect("create next task");
    assert!(
        next.summary.contains("Created task #3"),
        "expected fresh id #3, got `{}`",
        next.summary
    );
}

#[tokio::test]
async fn load_task_list_snapshot_reads_workspace_records() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-snapshot").await;

    let empty = load_task_list_snapshot(
        &context.home_dir,
        &session_task_list_id(context.session_id.as_deref()),
    )
    .await
    .expect("load empty snapshot");
    assert!(empty.tasks.is_empty());
    assert_eq!(empty.summary.total, 0);

    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Snapshot","description":"Run it"}"#,
            &context,
        )
        .await
        .expect("create snapshot task");
    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Follow","description":"Depends on snapshot"}"#,
            &context,
        )
        .await
        .expect("create follow task");
    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"in_progress","owner":"alice"}"#,
            &context,
        )
        .await
        .expect("set in progress and owner");
    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"2","addBlockedBy":["1"]}"#,
            &context,
        )
        .await
        .expect("add blockedBy to follow task");

    let snapshot = load_task_list_snapshot(
        &context.home_dir,
        &session_task_list_id(context.session_id.as_deref()),
    )
    .await
    .expect("load populated snapshot");
    assert_eq!(snapshot.summary.total, 2);
    assert_eq!(snapshot.summary.in_progress, 1);
    assert_eq!(snapshot.summary.pending, 1);
    assert_eq!(snapshot.tasks[0].id, "1");
    assert_eq!(snapshot.tasks[0].status, TaskStatusKind::InProgress);
    assert_eq!(snapshot.tasks[0].owner.as_deref(), Some("alice"));
    assert_eq!(snapshot.tasks[1].id, "2");
    assert_eq!(snapshot.tasks[1].open_blockers, vec!["1".to_string()]);

    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"completed"}"#,
            &context,
        )
        .await
        .expect("complete snapshot task");

    let after_complete = load_task_list_snapshot(
        &context.home_dir,
        &session_task_list_id(context.session_id.as_deref()),
    )
    .await
    .expect("load snapshot after completion");
    assert_eq!(after_complete.summary.completed, 1);
    assert!(after_complete.tasks[1].open_blockers.is_empty());
}

#[tokio::test]
async fn task_runtime_tools_read_output_and_stop_background_jobs() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-runtime").await;

    seed_background_job(
        &context,
        "job-complete",
        BackgroundTaskStatus::Completed,
        None,
        "done\n",
    )
    .await;
    let completed = registry
        .invoke(
            "TaskOutput",
            r#"{"task_id":"job-complete","block":false}"#,
            &context,
        )
        .await
        .expect("read completed task output");
    let completed_json: Value =
        serde_json::from_str(&completed.output).expect("parse completed output");
    assert_eq!(completed_json["retrieval_status"], "success");
    assert_eq!(completed_json["task"]["status"], "completed");
    assert_eq!(completed_json["task"]["output"], "done\n");
    assert_eq!(completed_json["task"]["result"], "done\n");
    assert_eq!(
        completed_json["task"]["output_path"]
            .as_str()
            .expect("output path"),
        context
            .home_dir
            .join("background")
            .join("logs")
            .join("job-complete.log")
            .display()
            .to_string()
    );
    assert_eq!(completed_json["task"]["progress"]["active"], false);
    assert_eq!(completed_json["task"]["progress"]["log_bytes"], 5);

    seed_background_job(
        &context,
        "job-running",
        BackgroundTaskStatus::Running,
        None,
        "still-running\n",
    )
    .await;
    let running = registry
        .invoke(
            "task-output",
            r#"{"task_id":"job-running","block":true,"timeout":1}"#,
            &context,
        )
        .await
        .expect("read running task output");
    let running_json: Value = serde_json::from_str(&running.output).expect("parse running output");
    assert_eq!(running_json["retrieval_status"], "timeout");
    assert_eq!(running_json["task"]["status"], "running");
    assert_eq!(running_json["task"]["result"], Value::Null);
    assert_eq!(running_json["task"]["progress"]["active"], true);

    let mut child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn background sleep");
    let pid = child.id().expect("sleep pid");
    seed_background_job(
        &context,
        "job-stop",
        BackgroundTaskStatus::Running,
        Some(pid),
        "before-stop\n",
    )
    .await;

    let stop = registry
        .invoke("TaskStop", r#"{"task_id":"job-stop"}"#, &context)
        .await
        .expect("stop background job");
    let stop_json: Value = serde_json::from_str(&stop.output).expect("parse stop output");
    assert_eq!(stop_json["task_id"], "job-stop");

    timeout(Duration::from_secs(3), child.wait())
        .await
        .expect("wait for stopped child")
        .expect("child wait status");

    let stopped = registry
        .invoke(
            "task-output",
            r#"{"task_id":"job-stop","block":false}"#,
            &context,
        )
        .await
        .expect("read stopped task output");
    let stopped_json: Value = serde_json::from_str(&stopped.output).expect("parse stopped output");
    assert_eq!(stopped_json["retrieval_status"], "success");
    assert_eq!(stopped_json["task"]["status"], "cancelled");
    assert_eq!(stopped_json["task"]["error"], "Cancelled via TaskStop");
    assert_eq!(stopped_json["task"]["result"], "before-stop\n");
    assert_eq!(stopped_json["task"]["progress"]["active"], false);
}

#[tokio::test]
async fn task_output_polls_until_terminal_state_and_reports_progress() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("task-output-poll").await;
    let progress = Arc::new(RecordingProgressReporter::default());
    context.progress = Some(progress.clone());

    let mut record = seed_background_job(
        &context,
        "job-poll",
        BackgroundTaskStatus::Running,
        None,
        "starting\n",
    )
    .await;
    let home = context.home_dir.clone();
    let log_path = PathBuf::from(&record.log_path);
    let finisher = tokio::spawn(async move {
        sleep(Duration::from_millis(50)).await;
        tokio::fs::write(&log_path, "starting\nfinished\n")
            .await
            .expect("write final log");
        let now = Utc::now().to_rfc3339();
        record.status = BackgroundTaskStatus::Completed;
        record.updated_at = now.clone();
        record.finished_at = Some(now);
        write_background_task_record(&home, &record)
            .await
            .expect("write completed record");
    });

    let output = registry
        .invoke(
            "TaskOutput",
            r#"{"task_id":"job-poll","block":true,"timeout":1000}"#,
            &context,
        )
        .await
        .expect("poll task output");
    finisher.await.expect("finish background job");

    let output_json: Value = serde_json::from_str(&output.output).expect("parse output");
    assert_eq!(output_json["retrieval_status"], "success");
    assert_eq!(output_json["task"]["status"], "completed");
    assert_eq!(output_json["task"]["output"], "starting\nfinished\n");
    assert_eq!(output_json["task"]["result"], "starting\nfinished\n");
    assert_eq!(output_json["task"]["progress"]["active"], false);

    let progress_records = progress.records.lock().await;
    assert!(
        progress_records.iter().any(|record| {
            record["type"] == "waiting_for_task"
                && record["task_id"] == "job-poll"
                && record["taskType"] == "background_job"
        }),
        "expected waiting_for_task progress, got {progress_records:?}"
    );
}

#[tokio::test]
async fn task_output_reads_persisted_state_after_context_resume() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-output-resume").await;
    seed_background_job(
        &context,
        "job-resume",
        BackgroundTaskStatus::Completed,
        None,
        "resumed output\n",
    )
    .await;

    let mut resumed_context = context.clone();
    resumed_context.progress = None;
    resumed_context.cancellation = ToolCancellationToken::default();
    let output = registry
        .invoke(
            "task-output",
            r#"{"task_id":"job-resume","block":false}"#,
            &resumed_context,
        )
        .await
        .expect("read resumed task output");
    let output_json: Value = serde_json::from_str(&output.output).expect("parse output");
    assert_eq!(output_json["retrieval_status"], "success");
    assert_eq!(output_json["task"]["status"], "completed");
    assert_eq!(output_json["task"]["output"], "resumed output\n");
}

#[tokio::test]
async fn plan_mode_tools_manage_workspace_plan_lifecycle() {
    let registry = ToolRegistry::foundation();
    let context = test_context("plan-mode").await;

    let enter = registry
        .invoke("EnterPlanMode", "{}", &context)
        .await
        .expect("enter plan mode");
    let plan_path = workspace_plan_file_path(&context);
    assert!(enter.output.contains("Entered plan mode"));
    assert!(enter.output.contains(&plan_path.display().to_string()));
    assert!(std::fs::exists(&plan_path).expect("plan file exists check"));

    registry
        .invoke(
            "file-write",
            &json!({
                "file_path": plan_path.display().to_string(),
                "content": "# Plan\n\n## Problem\nNeed plan tools.\n\n## Steps\n1. Add tools.\n2. Test tools.\n"
            })
            .to_string(),
            &context,
        )
        .await
        .expect("write plan");

    let exit = registry
        .invoke("ExitPlanMode", "{}", &context)
        .await
        .expect("exit plan mode");
    assert!(exit.output.contains("Exited plan mode"));
    assert!(exit.output.contains("Need plan tools."));
    let state = load_plan_mode_state(&context)
        .await
        .expect("load plan state")
        .expect("plan state should exist");
    assert!(!state.in_plan_mode);

    let verify = registry
        .invoke("VerifyPlanExecution", "{}", &context)
        .await
        .expect("verify plan execution");
    assert!(verify.output.contains("Verification snapshot"));
    assert!(verify.output.contains("Plan present: yes"));
    assert!(verify.output.contains("Need plan tools."));
}

#[tokio::test]
async fn local_agent_background_record_round_trips_typed_fields() {
    use crate::{
        BackgroundTaskKind, BackgroundTaskRecord, ToolRegistry, background_log_path,
        read_background_task_record, write_background_task_record,
    };

    let context = test_context("local-agent-record").await;
    let log_path = background_log_path(&context.home_dir, "agent-job-1");
    std::fs::create_dir_all(log_path.parent().expect("logs dir")).expect("create logs dir");
    std::fs::write(&log_path, "interim subagent output\n").expect("write log");

    let record = BackgroundTaskRecord::new_local_agent(
        "agent-job-1".to_string(),
        "session-1".to_string(),
        "session-1:agent-aaa".to_string(),
        "toolu-parent".to_string(),
        "Explore".to_string(),
        "summarize the repo".to_string(),
        context.cwd.display().to_string(),
        Some("claude-haiku-4-5".to_string()),
        Some(PermissionMode::Plan),
        log_path.display().to_string(),
    );
    write_background_task_record(&context.home_dir, &record)
        .await
        .expect("write local-agent record");

    let loaded = read_background_task_record(&context.home_dir, "agent-job-1")
        .await
        .expect("read record")
        .expect("record present");
    assert_eq!(loaded.task_kind, BackgroundTaskKind::LocalAgent);
    assert_eq!(loaded.agent_type.as_deref(), Some("Explore"));
    assert_eq!(loaded.model.as_deref(), Some("claude-haiku-4-5"));
    assert_eq!(loaded.permission_mode, Some(PermissionMode::Plan));
    assert_eq!(
        loaded.child_session_id.as_deref(),
        Some("session-1:agent-aaa")
    );
    assert_eq!(loaded.tool_use_id.as_deref(), Some("toolu-parent"));
    assert_eq!(loaded.status, BackgroundTaskStatus::Running);

    let registry = ToolRegistry::foundation();
    let output = registry
        .invoke(
            "task-output",
            r#"{"task_id":"agent-job-1","block":false}"#,
            &context,
        )
        .await
        .expect("read agent task output");
    let payload: Value = serde_json::from_str(&output.output).expect("parse output json");
    assert_eq!(payload["task"]["task_type"], "local_agent");
    assert_eq!(payload["task"]["child_session_id"], "session-1:agent-aaa");
    assert_eq!(payload["task"]["agent_type"], "Explore");
    assert_eq!(payload["task"]["model"], "claude-haiku-4-5");
    assert_eq!(payload["task"]["permission_mode"], "plan");
    assert_eq!(payload["task"]["status"], "running");
}

#[tokio::test]
async fn task_output_progress_reports_local_agent_kind() {
    use crate::{
        BackgroundTaskRecord, ToolRegistry, background_log_path, write_background_task_record,
    };

    let mut context = test_context("local-agent-output-progress").await;
    let progress = Arc::new(RecordingProgressReporter::default());
    context.progress = Some(progress.clone());

    let log_path = background_log_path(&context.home_dir, "agent-job-progress");
    std::fs::create_dir_all(log_path.parent().expect("logs dir")).expect("create logs dir");
    std::fs::write(&log_path, "still running\n").expect("write log");
    let record = BackgroundTaskRecord::new_local_agent(
        "agent-job-progress".to_string(),
        "session-1".to_string(),
        "session-1:agent-progress".to_string(),
        "toolu-parent".to_string(),
        "general-purpose".to_string(),
        "background agent progress".to_string(),
        context.cwd.display().to_string(),
        None,
        None,
        log_path.display().to_string(),
    );
    write_background_task_record(&context.home_dir, &record)
        .await
        .expect("persist running local-agent record");

    let registry = ToolRegistry::foundation();
    let output = registry
        .invoke(
            "task-output",
            r#"{"task_id":"agent-job-progress","block":true,"timeout":1}"#,
            &context,
        )
        .await
        .expect("poll running local-agent output");
    let payload: Value = serde_json::from_str(&output.output).expect("parse output json");
    assert_eq!(payload["retrieval_status"], "timeout");
    assert_eq!(payload["task"]["task_type"], "local_agent");

    let progress_records = progress.records.lock().await;
    assert!(
        progress_records.iter().any(|record| {
            record["type"] == "waiting_for_task"
                && record["task_id"] == "agent-job-progress"
                && record["taskType"] == "local_agent"
        }),
        "expected waiting_for_task progress for local_agent, got {progress_records:?}"
    );
}

#[tokio::test]
async fn local_agent_finished_record_exposes_explicit_result() {
    use crate::{
        BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus, ToolRegistry,
        background_log_path, write_background_task_record,
    };

    let context = test_context("local-agent-result").await;
    let log_path = background_log_path(&context.home_dir, "agent-job-2");
    std::fs::create_dir_all(log_path.parent().expect("logs dir")).expect("create logs dir");
    std::fs::write(&log_path, "raw streamed log").expect("write log");

    let mut record = BackgroundTaskRecord::new_local_agent(
        "agent-job-2".to_string(),
        "session-1".to_string(),
        "session-1:agent-bbb".to_string(),
        "toolu-parent".to_string(),
        "general-purpose".to_string(),
        "explain auth flow".to_string(),
        context.cwd.display().to_string(),
        None,
        None,
        log_path.display().to_string(),
    );
    let now = Utc::now().to_rfc3339();
    record.status = BackgroundTaskStatus::Completed;
    record.updated_at = now.clone();
    record.finished_at = Some(now);
    record.result = Some("Final concise answer.".to_string());
    write_background_task_record(&context.home_dir, &record)
        .await
        .expect("persist completed record");

    let registry = ToolRegistry::foundation();
    let output = registry
        .invoke(
            "task-output",
            r#"{"task_id":"agent-job-2","block":false}"#,
            &context,
        )
        .await
        .expect("read agent task output");
    let payload: Value = serde_json::from_str(&output.output).expect("parse output json");
    assert_eq!(payload["retrieval_status"], "success");
    assert_eq!(payload["task"]["status"], "completed");
    assert_eq!(
        payload["task"]["task_type"],
        BackgroundTaskKind::LocalAgent.as_str()
    );
    assert_eq!(payload["task"]["result"], "Final concise answer.");
    assert_eq!(payload["task"]["output"], "raw streamed log");
}

#[tokio::test]
async fn list_local_agent_records_filters_by_kind_and_session() {
    use crate::{
        BackgroundTaskKind, BackgroundTaskRecord, background_log_path,
        list_local_agent_records_for_session, write_background_task_record,
    };

    let context = test_context("list-local-agent").await;
    let _now = Utc::now().to_rfc3339();

    let logs_dir = context.home_dir.join("background").join("logs");
    std::fs::create_dir_all(&logs_dir).expect("create logs dir");
    for id in ["agent-aa", "agent-bb", "agent-cc"] {
        std::fs::write(background_log_path(&context.home_dir, id), "").expect("log");
    }

    let mut keep_one = BackgroundTaskRecord::new_local_agent(
        "agent-aa".to_string(),
        "session-target".to_string(),
        "session-target:agent-1".to_string(),
        "toolu-1".to_string(),
        "Explore".to_string(),
        "first agent".to_string(),
        context.cwd.display().to_string(),
        None,
        None,
        background_log_path(&context.home_dir, "agent-aa")
            .display()
            .to_string(),
    );
    keep_one.created_at = "2026-05-27T00:00:01Z".to_string();
    write_background_task_record(&context.home_dir, &keep_one)
        .await
        .expect("persist keep_one");

    let mut keep_two = BackgroundTaskRecord::new_local_agent(
        "agent-bb".to_string(),
        "session-target".to_string(),
        "session-target:agent-2".to_string(),
        "toolu-2".to_string(),
        "general-purpose".to_string(),
        "second agent".to_string(),
        context.cwd.display().to_string(),
        None,
        None,
        background_log_path(&context.home_dir, "agent-bb")
            .display()
            .to_string(),
    );
    keep_two.created_at = "2026-05-27T00:00:02Z".to_string();
    write_background_task_record(&context.home_dir, &keep_two)
        .await
        .expect("persist keep_two");

    let other_parent = BackgroundTaskRecord::new_local_agent(
        "agent-cc".to_string(),
        "session-other".to_string(),
        "session-other:agent-3".to_string(),
        "toolu-3".to_string(),
        "Explore".to_string(),
        "other parent agent".to_string(),
        context.cwd.display().to_string(),
        None,
        None,
        background_log_path(&context.home_dir, "agent-cc")
            .display()
            .to_string(),
    );
    write_background_task_record(&context.home_dir, &other_parent)
        .await
        .expect("persist other_parent");

    let shell_job = seed_background_job(
        &context,
        "job-shell",
        BackgroundTaskStatus::Running,
        Some(42),
        "shell only",
    )
    .await;
    assert_eq!(shell_job.task_kind, BackgroundTaskKind::BackgroundJob);

    let records = list_local_agent_records_for_session(&context.home_dir, "session-target")
        .await
        .expect("list for target session");
    let ids: Vec<&str> = records.iter().map(|r| r.job_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["agent-aa", "agent-bb"],
        "only LocalAgent records belonging to session-target, oldest first"
    );

    let none = list_local_agent_records_for_session(&context.home_dir, "session-missing")
        .await
        .expect("list for unknown session");
    assert!(none.is_empty(), "unknown parent should match nothing");
}

#[tokio::test]
async fn task_stop_signals_in_process_cancel_flag_for_background_agent() {
    use crate::{
        BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus, ToolRegistry,
        background_log_path, cancel_background_task, has_background_task_cancel_flag,
        read_background_task_record, register_background_task_cancel_flag,
        unregister_background_task_cancel_flag, write_background_task_record,
    };

    let context = test_context("local-agent-stop").await;
    let log_path = background_log_path(&context.home_dir, "agent-job-stop");
    std::fs::create_dir_all(log_path.parent().expect("logs dir")).expect("create logs dir");
    std::fs::write(&log_path, "").expect("touch log");

    let record = BackgroundTaskRecord::new_local_agent(
        "agent-job-stop".to_string(),
        "session-1".to_string(),
        "session-1:agent-ccc".to_string(),
        "toolu-parent".to_string(),
        "general-purpose".to_string(),
        "long running agent".to_string(),
        context.cwd.display().to_string(),
        None,
        None,
        log_path.display().to_string(),
    );
    write_background_task_record(&context.home_dir, &record)
        .await
        .expect("persist record");
    assert_eq!(record.task_kind, BackgroundTaskKind::LocalAgent);

    let flag = Arc::new(AtomicBool::new(false));
    register_background_task_cancel_flag("agent-job-stop", flag.clone());
    assert!(has_background_task_cancel_flag("agent-job-stop"));

    let registry = ToolRegistry::foundation();
    let stop = registry
        .invoke("TaskStop", r#"{"task_id":"agent-job-stop"}"#, &context)
        .await
        .expect("stop background agent");
    let stop_json: Value = serde_json::from_str(&stop.output).expect("parse stop output");
    assert_eq!(stop_json["task_id"], "agent-job-stop");
    assert_eq!(stop_json["task_type"], "local_agent");
    assert!(
        flag.load(Ordering::SeqCst),
        "in-process cancel flag must be set after TaskStop"
    );

    let updated = read_background_task_record(&context.home_dir, "agent-job-stop")
        .await
        .expect("reload record")
        .expect("record present");
    assert_eq!(updated.status, BackgroundTaskStatus::Cancelled);

    unregister_background_task_cancel_flag("agent-job-stop");
    assert!(!cancel_background_task("agent-job-stop"));
}

#[tokio::test]
async fn task_output_reports_completed_workflow_result() {
    use crate::{
        BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus, ToolRegistry,
        background_log_path, write_background_task_record,
    };

    let context = test_context("workflow-output-completed").await;
    let log_path = background_log_path(&context.home_dir, "workflow-output");
    std::fs::create_dir_all(log_path.parent().expect("logs dir")).expect("create logs dir");
    std::fs::write(&log_path, "workflow log\n").expect("write log");

    let mut record = BackgroundTaskRecord::new_workflow(
        "workflow-output".to_string(),
        "session-1".to_string(),
        "Run workflow".to_string(),
        context.cwd.display().to_string(),
        log_path.display().to_string(),
    );
    let now = Utc::now().to_rfc3339();
    record.status = BackgroundTaskStatus::Completed;
    record.updated_at = now.clone();
    record.finished_at = Some(now);
    record.result = Some("final aggregate".to_string());
    write_background_task_record(&context.home_dir, &record)
        .await
        .expect("persist workflow record");

    let output = ToolRegistry::foundation()
        .invoke(
            "task-output",
            r#"{"task_id":"workflow-output","block":false}"#,
            &context,
        )
        .await
        .expect("read workflow output");
    let payload: Value = serde_json::from_str(&output.output).expect("parse output");

    assert_eq!(payload["retrieval_status"], "success");
    assert_eq!(
        payload["task"]["task_type"],
        BackgroundTaskKind::Workflow.as_str()
    );
    assert_eq!(payload["task"]["status"], "completed");
    assert_eq!(payload["task"]["output"], "workflow log\n");
    assert_eq!(payload["task"]["result"], "final aggregate");
}

#[tokio::test]
async fn task_output_reports_running_workflow_not_ready_and_timeout() {
    use crate::{
        BackgroundTaskRecord, ToolRegistry, background_log_path, write_background_task_record,
    };

    let mut context = test_context("workflow-output-running").await;
    let progress = Arc::new(RecordingProgressReporter::default());
    context.progress = Some(progress.clone());
    let log_path = background_log_path(&context.home_dir, "workflow-running");
    std::fs::create_dir_all(log_path.parent().expect("logs dir")).expect("create logs dir");
    std::fs::write(&log_path, "still running\n").expect("write log");
    let record = BackgroundTaskRecord::new_workflow(
        "workflow-running".to_string(),
        "session-1".to_string(),
        "Run slow workflow".to_string(),
        context.cwd.display().to_string(),
        log_path.display().to_string(),
    );
    write_background_task_record(&context.home_dir, &record)
        .await
        .expect("persist workflow record");

    let registry = ToolRegistry::foundation();
    let not_ready = registry
        .invoke(
            "task-output",
            r#"{"task_id":"workflow-running","block":false}"#,
            &context,
        )
        .await
        .expect("read running workflow output");
    let not_ready_json: Value = serde_json::from_str(&not_ready.output).expect("parse not_ready");
    assert_eq!(not_ready_json["retrieval_status"], "not_ready");
    assert_eq!(not_ready_json["task"]["task_type"], "workflow");
    assert_eq!(not_ready_json["task"]["result"], Value::Null);

    let timeout = registry
        .invoke(
            "task-output",
            r#"{"task_id":"workflow-running","block":true,"timeout":1}"#,
            &context,
        )
        .await
        .expect("poll running workflow output");
    let timeout_json: Value = serde_json::from_str(&timeout.output).expect("parse timeout");
    assert_eq!(timeout_json["retrieval_status"], "timeout");
    assert_eq!(timeout_json["task"]["task_type"], "workflow");

    let progress_records = progress.records.lock().await;
    assert!(
        progress_records.iter().any(|record| {
            record["type"] == "waiting_for_task"
                && record["task_id"] == "workflow-running"
                && record["taskType"] == "workflow"
        }),
        "expected waiting progress for workflow, got {progress_records:?}"
    );
}

#[tokio::test]
async fn task_stop_signals_in_process_cancel_flag_for_workflow() {
    use crate::{
        BackgroundTaskRecord, BackgroundTaskStatus, ToolRegistry, background_log_path,
        cancel_background_task, has_background_task_cancel_flag, read_background_task_record,
        register_background_task_cancel_flag, unregister_background_task_cancel_flag,
        write_background_task_record,
    };

    let context = test_context("workflow-stop").await;
    let log_path = background_log_path(&context.home_dir, "workflow-stop");
    std::fs::create_dir_all(log_path.parent().expect("logs dir")).expect("create logs dir");
    std::fs::write(&log_path, "").expect("touch log");
    let record = BackgroundTaskRecord::new_workflow(
        "workflow-stop".to_string(),
        "session-1".to_string(),
        "Long workflow".to_string(),
        context.cwd.display().to_string(),
        log_path.display().to_string(),
    );
    write_background_task_record(&context.home_dir, &record)
        .await
        .expect("persist workflow record");

    let flag = Arc::new(AtomicBool::new(false));
    register_background_task_cancel_flag("workflow-stop", flag.clone());
    assert!(has_background_task_cancel_flag("workflow-stop"));

    let stop = ToolRegistry::foundation()
        .invoke("TaskStop", r#"{"task_id":"workflow-stop"}"#, &context)
        .await
        .expect("stop workflow");
    let stop_json: Value = serde_json::from_str(&stop.output).expect("parse stop");
    assert_eq!(stop_json["task_id"], "workflow-stop");
    assert_eq!(stop_json["task_type"], "workflow");
    assert!(flag.load(Ordering::SeqCst));

    let updated = read_background_task_record(&context.home_dir, "workflow-stop")
        .await
        .expect("read updated workflow")
        .expect("workflow record");
    assert_eq!(updated.status, BackgroundTaskStatus::Cancelled);

    unregister_background_task_cancel_flag("workflow-stop");
    assert!(!cancel_background_task("workflow-stop"));
}

#[tokio::test]
async fn task_create_output_matches_typescript_format() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-create-ts-format").await;

    let result = registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Run migrations","description":"Apply pending DB migrations"}"#,
            &context,
        )
        .await
        .expect("create task");

    assert_eq!(
        result.output, "Task #1 created successfully: Run migrations",
        "TaskCreate output must match TS mapToolResultToToolResultBlockParam format"
    );
}

#[tokio::test]
async fn task_update_output_matches_typescript_format() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-update-ts-format").await;

    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Design","description":"Design API"}"#,
            &context,
        )
        .await
        .expect("create task");

    let status_update = registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"in_progress"}"#,
            &context,
        )
        .await
        .expect("update status");
    assert_eq!(
        status_update.output, "Updated task #1 status",
        "TaskUpdate output must match TS format: Updated task #{{id}} {{fields}}"
    );

    let multi_update = registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","subject":"Redesign","owner":"bob"}"#,
            &context,
        )
        .await
        .expect("update multiple fields");
    assert_eq!(
        multi_update.output, "Updated task #1 subject, owner",
        "TaskUpdate output should list changed fields comma-separated"
    );

    let no_change = registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","subject":"Redesign"}"#,
            &context,
        )
        .await
        .expect("no-op update");
    assert_eq!(
        no_change.output, "Updated task #1 (no changes)",
        "TaskUpdate with no actual changes should indicate no changes"
    );

    let not_found = registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"99","status":"completed"}"#,
            &context,
        )
        .await
        .expect("not found update");
    assert_eq!(
        not_found.output, "Task #99 not found",
        "TaskUpdate for missing task must match TS format"
    );

    let deleted = registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"deleted"}"#,
            &context,
        )
        .await
        .expect("delete task");
    assert_eq!(
        deleted.output, "Updated task #1 deleted",
        "TaskUpdate delete must match TS format"
    );
}

#[tokio::test]
async fn exit_plan_mode_rejects_when_not_in_plan_mode() {
    let registry = ToolRegistry::foundation();
    let context = test_context("exit-plan-not-active").await;

    let error = registry
        .invoke("ExitPlanMode", "{}", &context)
        .await
        .expect_err("ExitPlanMode should reject when not in plan mode");

    let message = error.to_string();
    assert!(
        message.contains("You are not in plan mode"),
        "expected TS-compatible rejection message, got: {message}"
    );
    assert!(
        message.contains("continue with implementation"),
        "expected guidance suffix in rejection message, got: {message}"
    );
}

#[tokio::test]
async fn exit_plan_mode_succeeds_after_enter() {
    let registry = ToolRegistry::foundation();
    let context = test_context("exit-plan-after-enter").await;

    registry
        .invoke("EnterPlanMode", "{}", &context)
        .await
        .expect("enter plan mode");

    let exit = registry
        .invoke("ExitPlanMode", "{}", &context)
        .await
        .expect("exit plan mode after entering should succeed");
    assert!(exit.output.contains("Exited plan mode"));
}

#[tokio::test]
async fn todo_write_backward_compat_alongside_task_tools() {
    let registry = ToolRegistry::foundation();
    let context = test_context("todo-compat").await;

    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Task one","description":"First task"}"#,
            &context,
        )
        .await
        .expect("create task");

    let todo = registry
        .invoke(
            "TodoWrite",
            r#"{"list":"sprint","items":["item alpha","item beta"]}"#,
            &context,
        )
        .await
        .expect("todo write should succeed alongside task tools");
    assert!(todo.output.contains("sprint.json"));

    let task_list = registry
        .invoke("TaskList", "{}", &context)
        .await
        .expect("task list unaffected by todo write");
    let task_list_json: Value =
        serde_json::from_str(&task_list.output).expect("parse task list JSON");
    assert_eq!(
        task_list_json["tasks"][0]["subject"], "Task one",
        "task list should be independent of todo list"
    );

    let todo_path = context.home_dir.join("todos/sprint.json");
    let todo_content = tokio::fs::read_to_string(&todo_path)
        .await
        .expect("read todo file");
    let todo_store: serde_json::Value =
        serde_json::from_str(&todo_content).expect("parse todo JSON");
    assert_eq!(todo_store["list_name"], "sprint");
    assert_eq!(
        todo_store["items"].as_array().expect("items array").len(),
        2
    );
    assert_eq!(todo_store["items"][0]["title"], "item alpha");
}

#[tokio::test]
async fn plan_file_discovery_uses_override_directory() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("plan-dir-override").await;
    let custom_plans_dir = context.cwd.join("plans");
    std::fs::create_dir_all(&custom_plans_dir).expect("create custom plans dir");
    context.plans_directory_override = Some(custom_plans_dir.clone());

    let enter = registry
        .invoke("EnterPlanMode", "{}", &context)
        .await
        .expect("enter plan mode with override dir");

    assert!(
        enter
            .output
            .contains(&custom_plans_dir.display().to_string()),
        "plan file should be in the override directory, got: {}",
        enter.output
    );

    let exit = registry
        .invoke("ExitPlanMode", "{}", &context)
        .await
        .expect("exit plan mode with override dir");
    assert!(exit.output.contains("Exited plan mode"));

    let plan_files: Vec<_> = std::fs::read_dir(&custom_plans_dir)
        .expect("read custom plans dir")
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext == "md" || ext == "json")
        })
        .collect();
    assert!(
        !plan_files.is_empty(),
        "plan files should be created in the override directory"
    );
}

#[test]
fn task_create_schema_matches_typescript() {
    use crate::catalog::tool_input_schema;
    let schema = tool_input_schema("task-create");
    let expected = json!({
        "type": "object",
        "properties": {
            "subject": { "description": "A brief title for the task", "type": "string" },
            "description": { "description": "What needs to be done", "type": "string" },
            "activeForm": { "description": "Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")", "type": "string" },
            "metadata": { "additionalProperties": {}, "description": "Arbitrary metadata to attach to the task", "propertyNames": { "type": "string" }, "type": "object" }
        },
        "required": ["subject", "description"],
        "additionalProperties": false,
    });
    assert_eq!(schema, expected, "TaskCreate schema must match TS SDK");
}

#[test]
fn task_update_schema_matches_typescript() {
    use crate::catalog::tool_input_schema;
    let schema = tool_input_schema("task-update");
    let props = schema["properties"].as_object().expect("properties object");
    assert!(props.contains_key("taskId"), "must have taskId");
    assert!(props.contains_key("status"), "must have status");
    assert!(props.contains_key("subject"), "must have subject");
    assert!(props.contains_key("description"), "must have description");
    assert!(props.contains_key("activeForm"), "must have activeForm");
    assert!(props.contains_key("owner"), "must have owner");
    assert!(props.contains_key("metadata"), "must have metadata");
    assert!(props.contains_key("addBlocks"), "must have addBlocks");
    assert!(props.contains_key("addBlockedBy"), "must have addBlockedBy");
    assert!(
        !props.contains_key("blocks"),
        "must NOT have blocks (TS uses addBlocks only)"
    );
    assert!(
        !props.contains_key("blockedBy"),
        "must NOT have blockedBy (TS uses addBlockedBy only)"
    );
    assert_eq!(
        schema["required"],
        json!(["taskId"]),
        "only taskId is required"
    );
    let status = &props["status"];
    assert!(
        status.get("anyOf").is_some(),
        "status must use anyOf format matching TS SDK"
    );
    assert_eq!(
        status["anyOf"],
        json!([
            { "enum": ["pending", "in_progress", "completed"], "type": "string" },
            { "const": "deleted", "type": "string" }
        ])
    );
}

#[tokio::test]
async fn task_list_json_structure_matches_typescript() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-list-json").await;

    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Alpha","description":"First task"}"#,
            &context,
        )
        .await
        .expect("create task");
    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"in_progress","owner":"agent-1"}"#,
            &context,
        )
        .await
        .expect("set in_progress");

    let listed = registry
        .invoke("TaskList", "{}", &context)
        .await
        .expect("list tasks");
    let output: Value = serde_json::from_str(&listed.output).expect("parse JSON output");
    let tasks = output["tasks"].as_array().expect("tasks array");
    assert_eq!(tasks.len(), 1);

    let task = &tasks[0];
    assert_eq!(task["id"], "1");
    assert_eq!(task["subject"], "Alpha");
    assert_eq!(task["status"], "in_progress");
    assert_eq!(task["owner"], "agent-1");
    assert_eq!(task["blockedBy"], json!([]));

    let keys: std::collections::BTreeSet<&str> = task
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let expected_keys: std::collections::BTreeSet<&str> =
        ["id", "subject", "status", "owner", "blockedBy"]
            .into_iter()
            .collect();
    assert_eq!(
        keys, expected_keys,
        "TaskList task objects must have exactly the TS fields"
    );
}

#[tokio::test]
async fn task_update_rejects_completed_to_in_progress() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-transition-ci").await;
    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Done","description":"Already completed"}"#,
            &context,
        )
        .await
        .expect("create task");
    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"completed"}"#,
            &context,
        )
        .await
        .expect("complete task");

    let err = registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"in_progress"}"#,
            &context,
        )
        .await
        .expect_err("completed→in_progress should fail");
    assert!(
        err.to_string()
            .contains("Cannot transition task from completed to in_progress"),
        "error message must match TS format, got: {err}"
    );
}

#[tokio::test]
async fn task_update_rejects_completed_to_pending() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-transition-cp").await;
    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Done","description":"Already completed"}"#,
            &context,
        )
        .await
        .expect("create task");
    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"completed"}"#,
            &context,
        )
        .await
        .expect("complete task");

    let err = registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"pending"}"#,
            &context,
        )
        .await
        .expect_err("completed→pending should fail");
    assert!(
        err.to_string()
            .contains("Cannot transition task from completed to pending"),
        "error message must match TS format, got: {err}"
    );
}

#[tokio::test]
async fn task_update_allows_valid_transitions() {
    let registry = ToolRegistry::foundation();
    let context = test_context("task-transition-valid").await;
    registry
        .invoke(
            "TaskCreate",
            r#"{"subject":"Flow","description":"Test transitions"}"#,
            &context,
        )
        .await
        .expect("create task");

    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"in_progress"}"#,
            &context,
        )
        .await
        .expect("pending→in_progress should succeed");

    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"pending"}"#,
            &context,
        )
        .await
        .expect("in_progress→pending should succeed");

    registry
        .invoke(
            "TaskUpdate",
            r#"{"taskId":"1","status":"completed"}"#,
            &context,
        )
        .await
        .expect("pending→completed should succeed");
}
