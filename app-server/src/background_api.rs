use chrono::{DateTime, Utc};
use orbcode_core::CoreError;
use orbcode_protocol::{
    BackgroundTaskProgressEvent, BackgroundTaskView, BackgroundTaskViewKind, StreamEvent,
    WorkflowStepView, WorkflowStepViewStatus,
};
use orbcode_tools::{read_background_task_record, subscribe_progress_stream, task_record_to_view};
use std::collections::HashMap;
use std::path::Path;
use tokio::sync::mpsc;

use super::{AppServer, BackgroundJobRecord};
use crate::background_task_view::{job_detail_to_view, job_summary_to_view};

#[derive(Debug, PartialEq, Eq)]
enum BacklogDrain {
    /// The receiver was advanced to the current tail (buffer empty).
    Drained,
    /// The broadcast sender was dropped while draining.
    Closed,
}

/// Consume every event still buffered behind a lagged broadcast receiver,
/// advancing its cursor to the current tail. Used after a `Lagged` error so a
/// freshly-read snapshot is the last state forwarded to the client rather than
/// being clobbered by replayed stale backlog.
fn drain_broadcast_backlog(rx: &mut tokio::sync::broadcast::Receiver<StreamEvent>) -> BacklogDrain {
    use tokio::sync::broadcast::error::TryRecvError;
    loop {
        match rx.try_recv() {
            Ok(_) | Err(TryRecvError::Lagged(_)) => continue,
            Err(TryRecvError::Empty) => return BacklogDrain::Drained,
            Err(TryRecvError::Closed) => return BacklogDrain::Closed,
        }
    }
}

impl AppServer {
    pub async fn create_background_job(
        &self,
        session_id: &str,
        prompt: impl Into<String>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        let config = self.sessions.effective_config();
        let model = config.provider_model_name(config.default_provider);
        let permission_mode = config.permission_mode;
        self.background
            .create_job(super::CreateBackgroundJob {
                session_id: session_id.to_string(),
                prompt: prompt.into(),
                cwd: config.cwd,
                provider: config.default_provider,
                fallback_provider: config.fallback_provider,
                model,
                permission_mode,
            })
            .await
    }

    pub async fn list_background_jobs(&self) -> Result<Vec<BackgroundTaskView>, CoreError> {
        let summaries = self.background.list_jobs().await?;
        let mut views: Vec<BackgroundTaskView> =
            summaries.iter().map(job_summary_to_view).collect();
        let config = self.sessions.effective_config();
        let agents = super::background_agent::list_local_task_views(&config.home_dir).await?;
        views.extend(agents);
        attach_workflow_progress_events(&config.home_dir, &mut views).await;
        views.sort_by_key(|v| std::cmp::Reverse(v.updated_at));
        Ok(views)
    }

    pub async fn load_background_job(
        &self,
        job_id: &str,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.background.load_job(job_id).await
    }

    pub async fn mark_background_running(
        &self,
        job_id: &str,
        pid: Option<u32>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.background.mark_running(job_id, pid).await
    }

    pub async fn complete_background_job(
        &self,
        job_id: &str,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.background.mark_completed(job_id).await
    }

    pub async fn complete_background_job_with_exit(
        &self,
        job_id: &str,
        exit_code: Option<i32>,
        signal: Option<i32>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.background
            .mark_completed_with_exit(job_id, exit_code, signal)
            .await
    }

    pub async fn fail_background_job(
        &self,
        job_id: &str,
        error: impl Into<String>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.background.mark_failed(job_id, error).await
    }

    pub async fn fail_background_job_with_exit(
        &self,
        job_id: &str,
        error: impl Into<String>,
        exit_code: Option<i32>,
        signal: Option<i32>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.background
            .mark_failed_with_exit(job_id, error, exit_code, signal)
            .await
    }

    pub async fn mark_background_cancelled(
        &self,
        job_id: &str,
        reason: Option<String>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.background.mark_cancelled(job_id, reason).await
    }

    pub async fn mark_background_cancelled_with_signal(
        &self,
        job_id: &str,
        reason: Option<String>,
        signal: Option<i32>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.background
            .mark_cancelled_with_signal(job_id, reason, signal)
            .await
    }

    pub async fn append_background_log(&self, job_id: &str, chunk: &str) -> Result<(), CoreError> {
        self.background.append_log(job_id, chunk).await
    }

    pub async fn read_background_log(&self, job_id: &str) -> Result<String, CoreError> {
        self.background.read_log(job_id).await
    }

    pub async fn read_background_events(&self, job_id: &str) -> Result<String, CoreError> {
        self.background.read_events(job_id).await
    }

    pub async fn append_background_event_line(
        &self,
        job_id: &str,
        value: &serde_json::Value,
    ) -> Result<(), CoreError> {
        self.background.append_event_line(job_id, value).await
    }

    pub async fn background_task_output(
        &self,
        job_id: &str,
        block: bool,
        timeout_ms: u64,
    ) -> Result<super::BackgroundTaskOutputResponse, CoreError> {
        self.background.task_output(job_id, block, timeout_ms).await
    }

    pub async fn list_background_jobs_summary(&self) -> Result<Vec<BackgroundTaskView>, CoreError> {
        self.list_background_jobs().await
    }

    pub async fn background_job_detail(
        &self,
        job_id: &str,
    ) -> Result<BackgroundTaskView, CoreError> {
        match self.background.job_detail(job_id).await {
            Ok(detail) => Ok(job_detail_to_view(&detail)),
            Err(error) => {
                let config = self.sessions.effective_config();
                let Some(record) = read_background_task_record(&config.home_dir, job_id).await?
                else {
                    return Err(error);
                };
                let mut view = task_record_to_view(&record);
                view.log_tail = read_log_tail(&record.log_path).await;
                view.progress_events =
                    read_workflow_progress_events(&config.home_dir, job_id).await;
                view.workflow_steps = read_workflow_step_views(
                    &config.home_dir,
                    job_id,
                    view.progress_events.clone(),
                )
                .await;
                Ok(view)
            }
        }
    }

    pub async fn background_task_progress_stream(
        &self,
        task_id: &str,
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, CoreError> {
        let mut broadcast_rx = subscribe_progress_stream(task_id).ok_or_else(|| {
            CoreError::Config(format!(
                "background task progress stream not found: {task_id}"
            ))
        })?;
        let initial = self.background_job_detail(task_id).await.ok();
        let (tx, rx) = mpsc::unbounded_channel();
        if let Some(task) = initial {
            let _ = tx.send(StreamEvent::BackgroundTaskUpdated {
                session_id: task.session_id.clone(),
                task,
            });
        }
        let resync = self.clone();
        let resync_task_id = task_id.to_string();
        tokio::spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match broadcast_rx.recv().await {
                    Ok(event) => {
                        if tx.send(event).is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        // A slow consumer overran the 256-slot broadcast buffer,
                        // so per-step progress events (including *terminal* ones)
                        // may have been dropped. The forwarded events carry only
                        // a single progress entry, not cumulative workflow_steps,
                        // and the client replaces its whole card state per event,
                        // so silently dropping them would leave completed steps
                        // stuck "Running".
                        //
                        // Resynchronize by sending a fresh full snapshot (which
                        // carries cumulative workflow_steps and child_session_ids).
                        // But first drain whatever stale events are still buffered
                        // behind the lagged cursor: otherwise the next recv() would
                        // replay that old backlog *after* the snapshot and clobber
                        // the resynced state right back to a single-progress view.
                        // Draining makes the snapshot the last state the client
                        // applies.
                        if matches!(
                            drain_broadcast_backlog(&mut broadcast_rx),
                            BacklogDrain::Closed
                        ) {
                            break;
                        }
                        if let Ok(task) = resync.background_job_detail(&resync_task_id).await
                            && tx
                                .send(StreamEvent::BackgroundTaskUpdated {
                                    session_id: task.session_id.clone(),
                                    task,
                                })
                                .is_err()
                        {
                            break;
                        }
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        });
        Ok(rx)
    }

    pub fn local_task_progress_event(
        session_id: &str,
        task_id: &str,
        status: &str,
        command: Option<&str>,
        exit_code: Option<i32>,
        signal: Option<i32>,
    ) -> StreamEvent {
        StreamEvent::LocalTaskProgress {
            session_id: session_id.to_string(),
            task_id: task_id.to_string(),
            status: status.to_string(),
            command: command.map(String::from),
            exit_code,
            signal,
        }
    }

    /// Submit a turn for a background prompt job, bridging the
    /// `BackgroundManager` cancel token to the turn's cancellation. When the
    /// job's cancel token is set (e.g. by `cancel_background_job`), the bridge
    /// calls `cancel_turn` so the in-progress turn — and any tools it is
    /// executing — observe cancellation through `ToolContext.cancellation`.
    pub async fn submit_background_turn(
        &self,
        session_id: &str,
        prompt: impl Into<String>,
        job_id: &str,
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, CoreError> {
        let rx = self.sessions.submit_turn(session_id, prompt).await?;

        if let Some(cancel_flag) = self.background.cancel_token(job_id) {
            let sessions = self.sessions.clone();
            let sid = session_id.to_string();
            // Detached cancellation watcher; it exits after cancel or when the flag is dropped.
            let _cancel_watcher_handle = tokio::spawn(async move {
                loop {
                    if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
                        sessions.cancel_turn(&sid).await;
                        break;
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }
            });
        }

        Ok(rx)
    }

    pub async fn cancel_background_job(
        &self,
        job_id: &str,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.cancel_background_job_for_session(job_id, None).await
    }

    /// Cancel a background job with session-ownership enforcement. When
    /// `current_session_id` is `Some`, the call is rejected with
    /// `PermissionDenied` if the job belongs to a different session — unless
    /// the permission mode grants `allow_all` (i.e. `bypassPermissions`).
    pub async fn cancel_background_job_for_session(
        &self,
        job_id: &str,
        current_session_id: Option<&str>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        let record = self.background.load_job(job_id).await?;

        if let Some(caller_session) = current_session_id
            && record.session_id != caller_session
            && !self.allow_all()
        {
            return Err(CoreError::PermissionDenied(format!(
                "cannot stop background job {job_id}: it belongs to session {} \
                 (current session is {caller_session})",
                record.session_id,
            )));
        }

        if !record.status.is_active() {
            return Ok(record);
        }

        self.background.signal_cancel(job_id);

        if let Some(pid) = record.pid.filter(|_| record.status.is_active()) {
            let output = tokio::process::Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .output()
                .await;

            if let Err(error) = output {
                self.background
                    .mark_cancelled(
                        job_id,
                        Some(format!("termination requested; signal error: {error}")),
                    )
                    .await?;
                return Err(CoreError::Tool(format!(
                    "failed to signal background job {job_id}: {error}"
                )));
            }
        }

        self.background
            .mark_cancelled(job_id, Some("termination requested".to_string()))
            .await
    }

    /// Returns the in-memory cancellation token for a background job, if one
    /// exists. Callers (e.g. a background worker) can clone the `Arc<AtomicBool>`
    /// and poll it to detect cancellation requests.
    pub fn background_cancel_token(
        &self,
        job_id: &str,
    ) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        self.background.cancel_token(job_id)
    }
}

async fn read_log_tail(path: &str) -> Option<Vec<String>> {
    let contents = tokio::fs::read_to_string(path).await.ok()?;
    Some(
        contents
            .lines()
            .rev()
            .take(10)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(str::to_string)
            .collect(),
    )
}

async fn read_workflow_progress_events(
    home_dir: &Path,
    job_id: &str,
) -> Option<Vec<BackgroundTaskProgressEvent>> {
    let path = home_dir
        .join("workflow-runs")
        .join(job_id)
        .join("journal.jsonl");
    let contents = tokio::fs::read_to_string(path).await.ok()?;
    Some(
        contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect(),
    )
}

async fn read_workflow_step_views(
    home_dir: &Path,
    job_id: &str,
    progress_events: Option<Vec<BackgroundTaskProgressEvent>>,
) -> Option<Vec<WorkflowStepView>> {
    let path = home_dir
        .join("workflow-runs")
        .join(job_id)
        .join("workflow.json");
    let workflow: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(path).await.ok()?).ok()?;
    let steps = workflow.get("steps")?.as_array()?;
    let mut views = Vec::new();
    append_workflow_step_views(steps, "step", None, 0, &mut views);
    merge_workflow_step_events(&mut views, progress_events.unwrap_or_default());
    Some(views)
}

fn append_workflow_step_views(
    steps: &[serde_json::Value],
    prefix: &str,
    parent_key: Option<&str>,
    depth: u32,
    views: &mut Vec<WorkflowStepView>,
) {
    for (index, step) in steps.iter().enumerate() {
        let step_key = format!("{prefix}.{index}");
        let (kind, label, children) = workflow_step_shape(step);
        views.push(WorkflowStepView {
            step_key: step_key.clone(),
            parent_key: parent_key.map(str::to_string),
            depth,
            kind,
            label,
            status: WorkflowStepViewStatus::Pending,
            started_at: None,
            finished_at: None,
            output: None,
            error: None,
            child_session_id: None,
        });
        if let Some(children) = children {
            append_workflow_step_views(children, &step_key, Some(&step_key), depth + 1, views);
        }
    }
}

fn workflow_step_shape(step: &serde_json::Value) -> (String, String, Option<&[serde_json::Value]>) {
    if let Some(agent) = step.get("agent").filter(|value| !value.is_null()) {
        let label = agent
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Agent step");
        return ("agent".to_string(), label.to_string(), None);
    }
    if let Some(log) = step.get("log").filter(|value| !value.is_null()) {
        let label = log
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Log step");
        return ("log".to_string(), label.to_string(), None);
    }
    if let Some(phase) = step.get("phase").filter(|value| !value.is_null()) {
        let label = phase
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Phase");
        return (
            "phase".to_string(),
            label.to_string(),
            phase
                .get("steps")
                .and_then(serde_json::Value::as_array)
                .map(Vec::as_slice),
        );
    }
    if let Some(parallel) = step.get("parallel").filter(|value| !value.is_null()) {
        let children = parallel.get("steps").and_then(serde_json::Value::as_array);
        let count = children.map_or(0, Vec::len);
        return (
            "parallel".to_string(),
            format!("{count} parallel steps"),
            children.map(Vec::as_slice),
        );
    }
    if let Some(pipeline) = step.get("pipeline").filter(|value| !value.is_null()) {
        let children = pipeline.get("steps").and_then(serde_json::Value::as_array);
        let count = children.map_or(0, Vec::len);
        return (
            "pipeline".to_string(),
            format!("{count} pipeline steps"),
            children.map(Vec::as_slice),
        );
    }
    ("unknown".to_string(), "Unknown step".to_string(), None)
}

fn merge_workflow_step_events(
    views: &mut [WorkflowStepView],
    progress_events: Vec<BackgroundTaskProgressEvent>,
) {
    let by_key: HashMap<String, usize> = views
        .iter()
        .enumerate()
        .map(|(index, view)| (view.step_key.clone(), index))
        .collect();
    for event in progress_events {
        let Some(step_key) = event.step_key.as_ref() else {
            continue;
        };
        let Some(index) = by_key.get(step_key).copied() else {
            continue;
        };
        let view = &mut views[index];
        match event.event.as_str() {
            "step_started" | "agent_started" | "phase_started" | "parallel_started" => {
                if !matches!(
                    view.status,
                    WorkflowStepViewStatus::Completed
                        | WorkflowStepViewStatus::Failed
                        | WorkflowStepViewStatus::Cancelled
                ) {
                    view.status = WorkflowStepViewStatus::Running;
                }
                set_started_at(view, event.timestamp);
                if let Some(kind) = event.kind.filter(|value| !value.trim().is_empty()) {
                    view.kind = kind;
                }
                if let Some(message) = event.message.filter(|value| !value.trim().is_empty()) {
                    view.label = message;
                }
                if event.event == "agent_started"
                    && let Some(child_session_id) = event
                        .child_session_id
                        .filter(|value| !value.trim().is_empty())
                {
                    view.child_session_id = Some(child_session_id);
                }
            }
            "step_completed" => {
                view.status = WorkflowStepViewStatus::Completed;
                view.finished_at = Some(event.timestamp);
                view.output = event.output;
            }
            "step_failed" => {
                view.status = WorkflowStepViewStatus::Failed;
                view.finished_at = Some(event.timestamp);
                view.error = event.message.or(event.output);
            }
            "step_cancelled" => {
                view.status = WorkflowStepViewStatus::Cancelled;
                view.finished_at = Some(event.timestamp);
                view.error = event.message.or(event.output);
            }
            _ => {}
        }
    }
}

fn set_started_at(view: &mut WorkflowStepView, timestamp: DateTime<Utc>) {
    if view.started_at.is_none() {
        view.started_at = Some(timestamp);
    }
}

async fn attach_workflow_progress_events(home_dir: &Path, views: &mut [BackgroundTaskView]) {
    for view in views {
        if view.kind != BackgroundTaskViewKind::Workflow {
            continue;
        }
        view.progress_events = read_workflow_progress_events(home_dir, &view.task_id).await;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use orbcode_config::AppConfigOverrides;
    use orbcode_core::CoreError;
    use orbcode_protocol::StreamEvent;

    use orbcode_protocol::BackgroundTaskViewStatus;

    use super::super::AppServer;
    use super::{BacklogDrain, drain_broadcast_backlog, merge_workflow_step_events};
    use crate::background::BackgroundJobStatus;

    #[test]
    fn step_cancelled_event_transitions_the_workflow_step_view() {
        use orbcode_protocol::{
            BackgroundTaskProgressEvent, WorkflowStepView, WorkflowStepViewStatus,
        };

        let running_agent_step = || WorkflowStepView {
            step_key: "step.0.1".to_string(),
            parent_key: Some("step.0".to_string()),
            depth: 2,
            kind: "agent".to_string(),
            label: "agent".to_string(),
            status: WorkflowStepViewStatus::Running,
            started_at: None,
            finished_at: None,
            output: None,
            error: None,
            child_session_id: Some("s:run:agent-1".to_string()),
        };
        let event = |name: &str, step_key: Option<&str>| BackgroundTaskProgressEvent {
            timestamp: chrono::Utc::now(),
            event: name.to_string(),
            step_key: step_key.map(str::to_string),
            kind: Some("agent".to_string()),
            message: Some("aborted by workflow cancellation".to_string()),
            output: None,
            child_session_id: None,
        };

        // The canonical `step_cancelled` (with the step key) transitions the step
        // out of Running — this is the projection path the abort sweep must hit.
        let mut views = vec![running_agent_step()];
        merge_workflow_step_events(&mut views, vec![event("step_cancelled", Some("step.0.1"))]);
        assert_eq!(views[0].status, WorkflowStepViewStatus::Cancelled);
        assert!(views[0].finished_at.is_some());

        // The old shape (`agent_cancelled`, no step_key) is NOT applied — it would
        // leave the step permanently Running (the regression this guards).
        let mut views = vec![running_agent_step()];
        merge_workflow_step_events(&mut views, vec![event("agent_cancelled", None)]);
        assert_eq!(views[0].status, WorkflowStepViewStatus::Running);
    }

    fn delta(text: &str) -> StreamEvent {
        StreamEvent::AssistantDelta {
            session_id: "s".to_string(),
            delta: text.to_string(),
        }
    }

    #[tokio::test]
    async fn drain_backlog_advances_lagged_receiver_to_tail() {
        // Overflow a 4-slot buffer so the receiver lags by 2, then drain: the
        // receiver must land at the current tail so the next event sent (a
        // stand-in for the resync snapshot) is what recv() returns — not the
        // stale backlog.
        let (tx, mut rx) = tokio::sync::broadcast::channel(4);
        for index in 0..6 {
            tx.send(delta(&format!("stale-{index}"))).expect("send");
        }

        assert_eq!(drain_broadcast_backlog(&mut rx), BacklogDrain::Drained);

        tx.send(delta("snapshot")).expect("send snapshot");
        assert_eq!(rx.try_recv().expect("recv snapshot"), delta("snapshot"));
    }

    #[tokio::test]
    async fn drain_backlog_reports_closed_when_sender_dropped() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(4);
        for index in 0..6 {
            tx.send(delta(&format!("stale-{index}"))).expect("send");
        }
        drop(tx);
        assert_eq!(drain_broadcast_backlog(&mut rx), BacklogDrain::Closed);
    }

    fn test_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "orbcode-app-server-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn background_job_e2e_metadata_and_cancel_token() {
        let home = test_path("bg-e2e-home");
        let cwd = test_path("bg-e2e-cwd");
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
        .expect("app server");

        let record = app
            .create_background_job("e2e-session", "background e2e test")
            .await
            .expect("create background job");

        assert!(
            !record.model.is_empty(),
            "model should be populated from config"
        );
        assert_eq!(record.provider, app.default_provider());

        let on_disk = tokio::fs::read_to_string(
            home.join("background")
                .join("jobs")
                .join(format!("{}.json", record.job_id)),
        )
        .await
        .expect("read persisted record");
        assert!(
            on_disk.contains("\"model\""),
            "JSON should contain model field"
        );

        let jobs = app.list_background_jobs().await.expect("list jobs");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].provider, Some(record.provider));
        assert_eq!(jobs[0].model.as_deref(), Some(record.model.as_str()));
        let display = jobs[0].to_string();
        assert!(
            display.contains(&record.provider.to_string()),
            "display should include provider: {display}"
        );

        let token = app
            .background_cancel_token(&record.job_id)
            .expect("cancel token should exist");
        assert!(
            !token.load(std::sync::atomic::Ordering::SeqCst),
            "token should not be cancelled yet"
        );

        let cancelled = app
            .cancel_background_job(&record.job_id)
            .await
            .expect("cancel background job");
        assert_eq!(cancelled.status, BackgroundJobStatus::Cancelled);
        assert!(
            token.load(std::sync::atomic::Ordering::SeqCst),
            "token should be signalled after cancel"
        );

        let second_cancel = app
            .cancel_background_job(&record.job_id)
            .await
            .expect("second cancel should be no-op");
        assert_eq!(second_cancel.status, BackgroundJobStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancel_background_job_denied_for_different_session() {
        let home = test_path("cancel-deny-home");
        let cwd = test_path("cancel-deny-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let record = app
            .create_background_job("session-owner", "owned job")
            .await
            .expect("create job");
        app.mark_background_running(&record.job_id, None)
            .await
            .expect("mark running");

        let err = app
            .cancel_background_job_for_session(&record.job_id, Some("session-other"))
            .await
            .expect_err("should be denied");
        assert!(
            matches!(err, CoreError::PermissionDenied(_)),
            "expected PermissionDenied, got: {err:?}"
        );

        let loaded = app
            .load_background_job(&record.job_id)
            .await
            .expect("load job");
        assert!(
            loaded.status.is_active(),
            "job should still be active after denied cancel"
        );
    }

    #[tokio::test]
    async fn cancel_background_job_allowed_for_same_session() {
        let home = test_path("cancel-allow-home");
        let cwd = test_path("cancel-allow-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let record = app
            .create_background_job("session-owner", "same session job")
            .await
            .expect("create job");
        app.mark_background_running(&record.job_id, None)
            .await
            .expect("mark running");

        let cancelled = app
            .cancel_background_job_for_session(&record.job_id, Some("session-owner"))
            .await
            .expect("same-session cancel should succeed");
        assert_eq!(cancelled.status, BackgroundJobStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancel_background_job_bypass_permissions_skips_ownership() {
        let home = test_path("cancel-bypass-home");
        let cwd = test_path("cancel-bypass-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let record = app
            .create_background_job("session-owner", "bypass test job")
            .await
            .expect("create job");
        app.mark_background_running(&record.job_id, None)
            .await
            .expect("mark running");

        app.set_allow_all(true);

        let cancelled = app
            .cancel_background_job_for_session(&record.job_id, Some("session-different"))
            .await
            .expect("bypassPermissions should skip ownership check");
        assert_eq!(cancelled.status, BackgroundJobStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancel_background_job_no_session_skips_ownership() {
        let home = test_path("cancel-nosession-home");
        let cwd = test_path("cancel-nosession-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let record = app
            .create_background_job("session-owner", "no-session cancel")
            .await
            .expect("create job");
        app.mark_background_running(&record.job_id, None)
            .await
            .expect("mark running");

        let cancelled = app
            .cancel_background_job(&record.job_id)
            .await
            .expect("cancel without session should succeed");
        assert_eq!(cancelled.status, BackgroundJobStatus::Cancelled);
    }

    #[tokio::test]
    async fn list_background_jobs_summary_returns_all_jobs() {
        let home = test_path("list-summary-home");
        let cwd = test_path("list-summary-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let r1 = app
            .create_background_job("s1", "first job")
            .await
            .expect("create job 1");
        app.create_background_job("s1", "second job")
            .await
            .expect("create job 2");
        app.mark_background_running(&r1.job_id, Some(42))
            .await
            .expect("mark running");

        let summaries = app
            .list_background_jobs_summary()
            .await
            .expect("list summaries");
        assert_eq!(summaries.len(), 2);
        for s in &summaries {
            assert!(!s.task_id.is_empty());
            assert_eq!(s.session_id, "s1");
            assert!(s.model.as_ref().is_some_and(|m| !m.is_empty()));
            assert_eq!(s.provider, Some(app.default_provider()));
            assert!(s.elapsed_ms() >= 0);
        }
    }

    #[tokio::test]
    async fn background_job_detail_returns_log_tail_and_metadata() {
        let home = test_path("detail-e2e-home");
        let cwd = test_path("detail-e2e-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let record = app
            .create_background_job("s1", "detail test prompt")
            .await
            .expect("create job");
        app.mark_background_running(&record.job_id, Some(77))
            .await
            .expect("mark running");
        app.append_background_log(&record.job_id, "log line 1\nlog line 2\nlog line 3\n")
            .await
            .expect("append log");

        let detail = app
            .background_job_detail(&record.job_id)
            .await
            .expect("job detail");
        assert_eq!(detail.task_id, record.job_id);
        assert_eq!(detail.status, BackgroundTaskViewStatus::Running);
        assert_eq!(detail.provider, Some(app.default_provider()));
        assert!(detail.model.as_ref().is_some_and(|m| !m.is_empty()));
        assert_eq!(
            detail.log_tail,
            Some(vec![
                "log line 1".to_string(),
                "log line 2".to_string(),
                "log line 3".to_string()
            ])
        );
        assert!(detail.elapsed_ms() >= 0);
    }

    #[test]
    fn local_task_progress_event_constructs_correctly() {
        let event = AppServer::local_task_progress_event(
            "session-1",
            "task-42",
            "succeeded",
            Some("ls -la"),
            Some(0),
            None,
        );
        match event {
            StreamEvent::LocalTaskProgress {
                session_id,
                task_id,
                status,
                command,
                exit_code,
                signal,
            } => {
                assert_eq!(session_id, "session-1");
                assert_eq!(task_id, "task-42");
                assert_eq!(status, "succeeded");
                assert_eq!(command.as_deref(), Some("ls -la"));
                assert_eq!(exit_code, Some(0));
                assert_eq!(signal, None);
            }
            _ => panic!("expected LocalTaskProgress"),
        }
    }

    #[test]
    fn local_task_progress_event_serde_round_trip() {
        let event =
            AppServer::local_task_progress_event("s1", "t1", "failed", None, Some(1), Some(9));
        let json = serde_json::to_string(&event).expect("serialize");
        let back: StreamEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
        assert!(json.contains("\"local_task_progress\""));
    }
}
