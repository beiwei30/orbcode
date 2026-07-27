use orbcode_core::CoreError;

use super::{AppServer, WorkflowCommand};

impl AppServer {
    pub async fn list_workflows(&self) -> Result<Vec<WorkflowCommand>, CoreError> {
        self.sessions.list_workflows().await
    }

    pub async fn start_workflow(
        &self,
        session_id: &str,
        name: &str,
        arguments: &str,
    ) -> Result<String, CoreError> {
        self.sessions
            .start_workflow(session_id, name, arguments)
            .await
    }

    pub async fn start_dynamic_workflow(
        &self,
        session_id: &str,
        name: &str,
        spec: serde_json::Value,
        arguments: &str,
    ) -> Result<String, CoreError> {
        self.sessions
            .start_dynamic_workflow(session_id, name, spec, arguments)
            .await
    }

    pub async fn resume_workflow(&self, run_id: &str) -> Result<String, CoreError> {
        self.sessions.resume_workflow(run_id).await
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use orbcode_app_server_protocol::{ClientRequestEnvelope, ResponseResult, method};
    use orbcode_core::WorkflowSource;
    use orbcode_protocol::{BackgroundTaskViewKind, WorkflowStepViewStatus};
    use orbcode_tools::{
        BackgroundTaskRecord, BackgroundTaskStatus, read_background_task_record,
        write_background_task_record,
    };
    use serde_json::{Value, json};

    use super::*;
    use crate::AppConfigOverrides;

    fn test_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "orbcode-workflow-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    async fn wait_for_completed_record(
        home: &std::path::Path,
        task_id: &str,
    ) -> BackgroundTaskRecord {
        let mut record = None;
        for _ in 0..50 {
            record = read_background_task_record(home, task_id)
                .await
                .expect("read record");
            if record
                .as_ref()
                .is_some_and(|record| record.status == BackgroundTaskStatus::Completed)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        record.expect("workflow record")
    }

    async fn completed_step_count(home: &std::path::Path, task_id: &str) -> usize {
        let journal = tokio::fs::read_to_string(
            home.join("workflow-runs")
                .join(task_id)
                .join("journal.jsonl"),
        )
        .await
        .expect("journal");
        journal
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|event| event["event"] == "step_completed")
            .count()
    }

    #[tokio::test]
    async fn workflow_list_and_start_log_workflow() {
        let home = test_path("home");
        let cwd = test_path("cwd");
        tokio::fs::create_dir_all(home.join("workflows"))
            .await
            .expect("home workflows");
        tokio::fs::create_dir_all(cwd.join(".claude/workflows/acp"))
            .await
            .expect("project workflows");
        tokio::fs::write(
            cwd.join(".claude/workflows/acp/check.json"),
            r#"{"schema_version":1,"description":"Run ACP check","steps":[{"log":{"message":"done $1"}}]}"#,
        )
        .await
        .expect("workflow");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home.clone()),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app");

        let workflows = app.list_workflows().await.expect("list workflows");
        assert_eq!(workflows.len(), 1);
        assert_eq!(workflows[0].name, "acp:check");
        assert_eq!(workflows[0].source, WorkflowSource::Project);

        let task_id = app
            .start_workflow("session-1", "acp:check", "ok")
            .await
            .expect("start workflow");
        assert!(task_id.starts_with("workflow-"));

        let record = wait_for_completed_record(&home, &task_id).await;
        assert_eq!(record.status, BackgroundTaskStatus::Completed);
        assert_eq!(record.result.as_deref(), Some("done ok"));

        let resumed_parent = app
            .bootstrap(Some("session-1"))
            .await
            .expect("workflow parent session remains resumable");
        assert_eq!(resumed_parent.session.session_id, "session-1");
        assert!(resumed_parent.session.messages.is_empty());

        let jobs = app.list_background_jobs().await.expect("list jobs");
        let workflow_job = jobs
            .iter()
            .find(|job| job.task_id == task_id)
            .expect("workflow job view");
        assert_eq!(workflow_job.kind, BackgroundTaskViewKind::Workflow);
        assert_eq!(workflow_job.description, "Run ACP check");
        let summary_progress_events = workflow_job
            .progress_events
            .as_ref()
            .expect("summary progress events");
        assert!(summary_progress_events.iter().any(|event| {
            event.event == "step_completed"
                && event.step_key.as_deref() == Some("step.0")
                && event.output.as_deref() == Some("done ok")
        }));

        let detail = app
            .background_job_detail(&task_id)
            .await
            .expect("workflow detail");
        assert_eq!(detail.kind, BackgroundTaskViewKind::Workflow);
        assert_eq!(
            detail.log_tail.as_deref(),
            Some(&["done ok".to_string()][..])
        );
        let progress_events = detail.progress_events.as_ref().expect("progress events");
        assert!(progress_events.iter().any(|event| {
            event.event == "step_started"
                && event.step_key.as_deref() == Some("step.0")
                && event.kind.as_deref() == Some("log")
        }));
        assert!(progress_events.iter().any(|event| {
            event.event == "step_completed"
                && event.step_key.as_deref() == Some("step.0")
                && event.output.as_deref() == Some("done ok")
        }));

        assert_eq!(completed_step_count(&home, &task_id).await, 1);

        let resumed_task_id = app
            .resume_workflow(&task_id)
            .await
            .expect("resume workflow");
        assert_eq!(resumed_task_id, task_id);

        let resumed_record = wait_for_completed_record(&home, &task_id).await;
        assert_eq!(resumed_record.status, BackgroundTaskStatus::Completed);
        assert_eq!(resumed_record.result.as_deref(), Some("done ok"));
        assert_eq!(completed_step_count(&home, &task_id).await, 1);
    }

    #[tokio::test]
    async fn dynamic_workflow_starts_inline_spec_without_file() {
        let home = test_path("dynamic-home");
        let cwd = test_path("dynamic-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let mut env = crate::sealed_provider_env_overrides();
        env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            "mock://anthropic?scenario=hello".to_string(),
        );
        env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());

        let app = AppServer::new(
            cwd.clone(),
            AppConfigOverrides {
                home_dir: Some(home.clone()),
                env_overrides: env,
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app");

        let task_id = app
            .start_dynamic_workflow(
                "session-1",
                "dynamic:check",
                json!({
                    "schema_version": 1,
                    "description": "Generated check",
                    // Dynamic workflows must include at least one agent-work
                    // step (a log-only dynamic workflow is a no-op and is
                    // rejected at validation).
                    "steps": [
                        { "agent": { "description": "inspect", "prompt": "inspect" } },
                        { "log": { "message": "dynamic $1" } }
                    ]
                }),
                "ok",
            )
            .await
            .expect("start dynamic workflow");

        let record = wait_for_completed_record(&home, &task_id).await;
        assert_eq!(record.status, BackgroundTaskStatus::Completed);
        assert!(
            record
                .result
                .as_deref()
                .is_some_and(|result| result.contains("dynamic ok")),
            "workflow result should include the final log output: {:?}",
            record.result
        );
        assert_eq!(record.prompt, "Generated check");
        assert!(!cwd.join(".claude/workflows/dynamic/check.json").exists());

        let persisted_spec = tokio::fs::read_to_string(
            home.join("workflow-runs")
                .join(&task_id)
                .join("workflow.json"),
        )
        .await
        .expect("persisted dynamic spec");
        let persisted_spec: Value = serde_json::from_str(&persisted_spec).expect("spec json");
        assert_eq!(persisted_spec["description"], "Generated check");
    }

    #[tokio::test]
    async fn workflow_detail_projects_nested_step_tree() {
        let home = test_path("step-tree-home");
        let cwd = test_path("step-tree-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(cwd.join(".claude/workflows/check"))
            .await
            .expect("project workflows");
        tokio::fs::write(
            cwd.join(".claude/workflows/check/step-tree.json"),
            r#"{
              "schema_version": 1,
              "description": "Step tree check",
              "steps": [
                {
                  "phase": {
                    "name": "Phase $1",
                    "steps": [
                      { "log": { "message": "first $1" } },
                      {
                        "parallel": {
                          "steps": [
                            { "log": { "message": "left $1" } },
                            { "log": { "message": "right $1" } }
                          ]
                        }
                      },
                      {
                        "pipeline": {
                          "steps": [
                            { "log": { "message": "pipe one $1" } },
                            { "log": { "message": "pipe two $1" } }
                          ]
                        }
                      }
                    ]
                  }
                }
              ]
            }"#,
        )
        .await
        .expect("workflow");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home.clone()),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app");

        let task_id = app
            .start_workflow("session-1", "check:step-tree", "ok")
            .await
            .expect("start workflow");

        let record = wait_for_completed_record(&home, &task_id).await;
        assert_eq!(record.status, BackgroundTaskStatus::Completed);

        let detail = app
            .background_job_detail(&task_id)
            .await
            .expect("workflow detail");
        let steps = detail.workflow_steps.expect("workflow steps");
        let keys = steps
            .iter()
            .map(|step| step.step_key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "step.0",
                "step.0.0",
                "step.0.1",
                "step.0.1.0",
                "step.0.1.1",
                "step.0.2",
                "step.0.2.0",
                "step.0.2.1"
            ]
        );

        let phase = &steps[0];
        assert_eq!(phase.kind, "phase");
        assert_eq!(phase.label, "Phase ok");
        assert_eq!(phase.depth, 0);
        assert_eq!(phase.status, WorkflowStepViewStatus::Completed);
        assert!(
            phase.output.as_deref().is_some_and(|output| {
                output.contains("first ok") && output.contains("right ok")
            })
        );

        let parallel = &steps[2];
        assert_eq!(parallel.kind, "parallel");
        assert_eq!(parallel.label, "2 steps");
        assert_eq!(parallel.depth, 1);
        assert_eq!(parallel.parent_key.as_deref(), Some("step.0"));
        assert!(parallel.started_at.is_some());
        assert!(parallel.finished_at.is_some());
        assert!(
            parallel.output.as_deref().is_some_and(|output| {
                output.contains("left ok") && output.contains("right ok")
            })
        );

        let right = &steps[4];
        assert_eq!(right.kind, "log");
        assert_eq!(right.label, "right ok");
        assert_eq!(right.depth, 2);
        assert_eq!(right.parent_key.as_deref(), Some("step.0.1"));
        assert_eq!(right.output.as_deref(), Some("right ok"));
    }

    #[tokio::test]
    async fn workflow_detail_projects_agent_child_session_ids() {
        let home = test_path("step-child-home");
        let cwd = test_path("step-child-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        let run_id = "workflow-step-child";
        let run_dir = home.join("workflow-runs").join(run_id);
        tokio::fs::create_dir_all(&run_dir).await.expect("run dir");
        tokio::fs::write(
            run_dir.join("workflow.json"),
            r#"{
              "schema_version": 1,
              "description": "Step child check",
              "steps": [
                { "agent": { "description": "new agent", "prompt": "do new" } },
                {
                  "parallel": {
                    "steps": [
                      { "agent": { "description": "old agent", "prompt": "do old" } }
                    ]
                  }
                }
              ]
            }"#,
        )
        .await
        .expect("workflow spec");
        let child_session_id = "session-1:workflow-step-child:agent-new";
        tokio::fs::write(
            run_dir.join("journal.jsonl"),
            format!(
                "{}\n{}\n{}\n{}\n",
                json!({
                    "timestamp": "2026-06-19T08:00:00Z",
                    "event": "agent_started",
                    "step_key": "step.0",
                    "kind": "agent",
                    "message": "new agent",
                    "child_session_id": child_session_id,
                }),
                json!({
                    "timestamp": "2026-06-19T08:00:01Z",
                    "event": "step_completed",
                    "step_key": "step.0",
                    "output": "new done",
                }),
                json!({
                    "timestamp": "2026-06-19T08:00:02Z",
                    "event": "parallel_started",
                    "step_key": "step.1",
                    "kind": "parallel",
                    "message": "1 steps",
                }),
                json!({
                    "timestamp": "2026-06-19T08:00:03Z",
                    "event": "agent_started",
                    "step_key": "step.1.0",
                    "kind": "agent",
                    "message": "old agent"
                })
            ),
        )
        .await
        .expect("journal");
        let log_path = home.join("logs").join("workflow-step-child.log");
        tokio::fs::create_dir_all(log_path.parent().expect("log parent"))
            .await
            .expect("log parent");
        tokio::fs::write(&log_path, "").await.expect("log");
        let mut record = BackgroundTaskRecord::new_workflow(
            run_id.to_string(),
            "session-1".to_string(),
            "Step child check".to_string(),
            cwd.display().to_string(),
            log_path.display().to_string(),
        );
        record.status = BackgroundTaskStatus::Completed;
        write_background_task_record(&home, &record)
            .await
            .expect("record");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home.clone()),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app");
        let detail = app
            .background_job_detail(run_id)
            .await
            .expect("workflow detail");
        let steps = detail.workflow_steps.expect("workflow steps");
        let new_agent = steps
            .iter()
            .find(|step| step.step_key == "step.0")
            .expect("new agent step");
        assert_eq!(
            new_agent.child_session_id.as_deref(),
            Some(child_session_id)
        );
        let parallel = steps
            .iter()
            .find(|step| step.step_key == "step.1")
            .expect("parallel step");
        assert!(parallel.child_session_id.is_none());
        let old_agent = steps
            .iter()
            .find(|step| step.step_key == "step.1.0")
            .expect("old agent step");
        assert!(old_agent.child_session_id.is_none());
    }

    #[tokio::test]
    async fn dynamic_workflow_rejects_invalid_inline_spec_before_task_creation() {
        let home = test_path("dynamic-invalid-home");
        let cwd = test_path("dynamic-invalid-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home.clone()),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app");

        let result = app
            .start_dynamic_workflow(
                "session-1",
                "dynamic:invalid",
                json!({
                    "schema_version": 1,
                    "steps": []
                }),
                "",
            )
            .await;

        assert!(result.is_err());
        let jobs = app.list_background_jobs().await.expect("list jobs");
        assert!(jobs.is_empty());
        assert!(!home.join("workflow-runs").exists());
    }

    #[tokio::test]
    async fn protocol_start_dynamic_workflow_returns_task_id() {
        let home = test_path("dynamic-protocol-home");
        let cwd = test_path("dynamic-protocol-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let mut env = crate::sealed_provider_env_overrides();
        env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            "mock://anthropic?scenario=hello".to_string(),
        );
        env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home.clone()),
                env_overrides: env,
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app");

        let response = app
            .handle_request(ClientRequestEnvelope {
                id: "req-1".to_string(),
                method: method::WORKFLOW_START_DYNAMIC.to_string(),
                params: Some(json!({
                    "session_id": "session-1",
                    "name": "dynamic:protocol",
                    "arguments": "ok",
                    "spec": {
                        "schema_version": 1,
                        "description": "Protocol dynamic check",
                        // A dynamic workflow must include at least one
                        // agent-work step (log-only is rejected).
                        "steps": [
                            { "agent": { "description": "inspect", "prompt": "inspect" } },
                            { "log": { "message": "protocol $1" } }
                        ]
                    }
                })),
            })
            .await;

        let task_id = match response.result {
            ResponseResult::Success { data: Some(data) } => {
                data["task_id"].as_str().expect("task id").to_string()
            }
            other => panic!("unexpected protocol response: {other:?}"),
        };
        let record = wait_for_completed_record(&home, &task_id).await;
        assert_eq!(record.status, BackgroundTaskStatus::Completed);
        assert!(
            record
                .result
                .as_deref()
                .is_some_and(|result| result.contains("protocol ok")),
            "workflow result should include the final log output: {:?}",
            record.result
        );
    }
}
