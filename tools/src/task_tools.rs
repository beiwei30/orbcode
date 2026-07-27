use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use chrono::{DateTime, Utc};
use orbcode_config::PermissionMode;
use orbcode_protocol::{
    BackgroundTaskView, BackgroundTaskViewKind, BackgroundTaskViewStatus, ProviderId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    process::Command,
    time::{Duration, sleep},
};
use uuid::Uuid;

use crate::{
    ToolContext, ToolError, ToolOutcome, ToolProgressReporter, ToolRegistry,
    output::{MAX_WEB_OUTPUT_CHARS, truncate_tool_output},
    payload::{bool_field_keys, parse_payload, string_field, string_field_any, usize_field_keys},
    permissions::{ensure_not_cancelled, require_tools},
    process::run_command_output,
};

const TASK_LOCK_RETRIES: usize = 50;
const TASK_LOCK_RETRY_MS: u64 = 20;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct TodoListStore {
    list_name: String,
    items: Vec<TodoItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TodoItem {
    title: String,
    done: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatusKind {
    Pending,
    InProgress,
    Completed,
}

impl From<TaskStatus> for TaskStatusKind {
    fn from(value: TaskStatus) -> Self {
        match value {
            TaskStatus::Pending => Self::Pending,
            TaskStatus::InProgress => Self::InProgress,
            TaskStatus::Completed => Self::Completed,
        }
    }
}

impl TaskStatusKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
}

impl TaskStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pending" => Some(Self::Pending),
            "in_progress" | "in-progress" => Some(Self::InProgress),
            "completed" | "complete" => Some(Self::Completed),
            _ => None,
        }
    }

    fn can_transition_to(self, next: Self) -> bool {
        !matches!(
            (self, next),
            (Self::Completed, Self::Pending | Self::InProgress)
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct TaskRecord {
    id: String,
    subject: String,
    description: String,
    #[serde(rename = "activeForm", skip_serializing_if = "Option::is_none")]
    active_form: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    pub(crate) status: TaskStatus,
    #[serde(default)]
    blocks: Vec<String>,
    #[serde(rename = "blockedBy", default)]
    blocked_by: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Map<String, Value>>,
}

struct TaskListLock {
    path: PathBuf,
}

impl Drop for TaskListLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Orphaned,
}

impl BackgroundTaskStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Orphaned
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Orphaned => "orphaned",
        }
    }
}

/// What kind of background task this record represents. Defaults to the
/// historical shell-job behavior so existing records on disk continue to load.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskKind {
    #[default]
    BackgroundJob,
    LocalAgent,
    Workflow,
}

fn map_background_task_status(status: BackgroundTaskStatus) -> BackgroundTaskViewStatus {
    match status {
        BackgroundTaskStatus::Queued => BackgroundTaskViewStatus::Queued,
        BackgroundTaskStatus::Running => BackgroundTaskViewStatus::Running,
        BackgroundTaskStatus::Completed => BackgroundTaskViewStatus::Completed,
        BackgroundTaskStatus::Failed => BackgroundTaskViewStatus::Failed,
        BackgroundTaskStatus::Cancelled => BackgroundTaskViewStatus::Cancelled,
        BackgroundTaskStatus::Orphaned => BackgroundTaskViewStatus::Orphaned,
    }
}

fn map_background_task_kind(kind: BackgroundTaskKind) -> BackgroundTaskViewKind {
    match kind {
        BackgroundTaskKind::BackgroundJob => BackgroundTaskViewKind::BackgroundJob,
        BackgroundTaskKind::LocalAgent => BackgroundTaskViewKind::LocalAgent,
        BackgroundTaskKind::Workflow => BackgroundTaskViewKind::Workflow,
    }
}

pub fn task_record_to_view(record: &BackgroundTaskRecord) -> BackgroundTaskView {
    BackgroundTaskView {
        task_id: record.job_id.clone(),
        session_id: record.session_id.clone(),
        kind: map_background_task_kind(record.task_kind),
        status: map_background_task_status(record.status),
        description: record.prompt.clone(),
        cwd: record.cwd.clone(),
        created_at: parse_rfc3339_or_now(&record.created_at),
        updated_at: parse_rfc3339_or_now(&record.updated_at),
        started_at: record.started_at.as_deref().map(parse_rfc3339_or_now),
        finished_at: record.finished_at.as_deref().map(parse_rfc3339_or_now),
        pid: record.pid,
        exit_code: record.exit_code,
        signal: record.signal,
        error: record.error.clone(),
        model: record.model.clone(),
        provider: if record.task_kind == BackgroundTaskKind::BackgroundJob {
            Some(ProviderId::Anthropic)
        } else {
            None
        },
        permission_mode: record.permission_mode.map(|mode| mode.as_str().to_string()),
        agent_type: record.agent_type.clone(),
        child_session_id: record.child_session_id.clone(),
        cancellation_reason: None,
        label: None,
        log_tail: None,
        progress_events: None,
        workflow_steps: None,
    }
}

fn parse_rfc3339_or_now(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

impl BackgroundTaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BackgroundJob => "background_job",
            Self::LocalAgent => "local_agent",
            Self::Workflow => "workflow",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BackgroundTaskRecord {
    pub job_id: String,
    pub session_id: String,
    pub prompt: String,
    pub cwd: String,
    pub status: BackgroundTaskStatus,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub pid: Option<u32>,
    pub log_path: String,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "is_default_kind")]
    pub task_kind: BackgroundTaskKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

fn is_default_kind(kind: &BackgroundTaskKind) -> bool {
    matches!(kind, BackgroundTaskKind::BackgroundJob)
}

impl BackgroundTaskRecord {
    pub fn new_local_agent(
        job_id: String,
        parent_session_id: String,
        child_session_id: String,
        tool_use_id: String,
        agent_type: String,
        prompt: String,
        cwd: String,
        model: Option<String>,
        permission_mode: Option<PermissionMode>,
        log_path: String,
    ) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            job_id,
            session_id: parent_session_id,
            prompt,
            cwd,
            status: BackgroundTaskStatus::Running,
            created_at: now.clone(),
            updated_at: now.clone(),
            started_at: Some(now),
            finished_at: None,
            pid: None,
            log_path,
            error: None,
            task_kind: BackgroundTaskKind::LocalAgent,
            tool_use_id: Some(tool_use_id),
            child_session_id: Some(child_session_id),
            agent_type: Some(agent_type),
            model,
            permission_mode,
            result: None,
            exit_code: None,
            signal: None,
            extra: serde_json::Map::new(),
        }
    }

    pub fn new_workflow(
        job_id: String,
        session_id: String,
        description: String,
        cwd: String,
        log_path: String,
    ) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            job_id,
            session_id,
            prompt: description,
            cwd,
            status: BackgroundTaskStatus::Running,
            created_at: now.clone(),
            updated_at: now.clone(),
            started_at: Some(now),
            finished_at: None,
            pid: None,
            log_path,
            error: None,
            task_kind: BackgroundTaskKind::Workflow,
            tool_use_id: None,
            child_session_id: None,
            agent_type: None,
            model: None,
            permission_mode: None,
            result: None,
            exit_code: None,
            signal: None,
            extra: serde_json::Map::new(),
        }
    }
}

impl ToolRegistry {
    pub(crate) async fn todo_write(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let list_name = string_field(&payload, "list").unwrap_or_else(|| "default".to_string());
        let mode = string_field(&payload, "mode").unwrap_or_else(|| "append".to_string());
        let todo_dir = context.home_dir.join("todos");
        tokio::fs::create_dir_all(&todo_dir).await?;
        let path = todo_dir.join(format!("{list_name}.json"));
        let mut store = if tokio::fs::try_exists(&path).await? {
            serde_json::from_str::<TodoListStore>(&tokio::fs::read_to_string(&path).await?)?
        } else {
            TodoListStore {
                list_name: list_name.clone(),
                items: Vec::new(),
            }
        };
        let new_items = parse_todo_items(&payload, input)?;
        if mode == "replace" {
            store.items = new_items;
        } else {
            store.items.extend(new_items);
        }
        tokio::fs::write(&path, serde_json::to_string_pretty(&store)?).await?;
        Ok(ToolOutcome {
            name: "todo-write".to_string(),
            summary: format!("Updated todo list `{list_name}`."),
            output: format!(
                "stored {} todo item(s) in {}",
                store.items.len(),
                path.display()
            ),
            metadata: None,
            changed_paths: vec![path],
        })
    }

    pub(crate) async fn task_create(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let subject = string_field(&payload, "subject")
            .ok_or_else(|| ToolError::InvalidInput("task-create requires `subject`".into()))?;
        let description = string_field(&payload, "description")
            .ok_or_else(|| ToolError::InvalidInput("task-create requires `description`".into()))?;
        let task_dir = ensure_task_list_dir(context).await?;
        let _lock = acquire_task_lock(&task_dir, context).await?;
        let task_id = next_task_id(&task_dir).await?;
        let task_path = task_file_path(&task_dir, &task_id);
        let task = TaskRecord {
            id: task_id.clone(),
            subject: subject.clone(),
            description,
            active_form: string_field_any(&payload, &["activeForm", "active_form"]),
            owner: None,
            status: TaskStatus::Pending,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata: object_field(&payload, "metadata")?,
        };

        write_task_record(&task_path, &task).await?;

        Ok(ToolOutcome {
            name: "task-create".to_string(),
            summary: format!("Created task #{} `{subject}`.", task.id),
            output: format!("Task #{} created successfully: {subject}", task.id),
            metadata: None,
            changed_paths: vec![task_path],
        })
    }

    pub(crate) async fn task_get(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let task_id = task_id_field(&payload)?;
        let task_dir = ensure_task_list_dir(context).await?;
        match read_task_record(&task_dir, &task_id).await? {
            Some(task) => Ok(ToolOutcome {
                name: "task-get".to_string(),
                summary: format!("Loaded task #{}.", task.id),
                output: serde_json::to_string_pretty(&task)?,
                metadata: None,
                changed_paths: Vec::new(),
            }),
            None => Ok(ToolOutcome {
                name: "task-get".to_string(),
                summary: format!("Task #{task_id} was not found."),
                output: format!("Task #{task_id} was not found in the current workspace list."),
                metadata: None,
                changed_paths: Vec::new(),
            }),
        }
    }

    pub(crate) async fn task_list(&self, context: &ToolContext) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let task_dir = ensure_task_list_dir(context).await?;
        let tasks = list_task_records(&task_dir).await?;
        let completed_ids: BTreeSet<String> = tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Completed)
            .map(|task| task.id.clone())
            .collect();
        let task_summaries: Vec<Value> = tasks
            .iter()
            .map(|task| {
                let open_blocked_by: Vec<Value> = task
                    .blocked_by
                    .iter()
                    .filter(|id| !completed_ids.contains(*id))
                    .map(|id| Value::String(id.clone()))
                    .collect();
                json!({
                    "id": task.id,
                    "subject": task.subject,
                    "status": task.status.as_str(),
                    "owner": task.owner.as_deref().unwrap_or(""),
                    "blockedBy": open_blocked_by,
                })
            })
            .collect();
        let output = serde_json::to_string_pretty(&json!({ "tasks": task_summaries }))?;
        Ok(ToolOutcome {
            name: "task-list".to_string(),
            summary: format!("Listed {} task(s).", tasks.len()),
            output,
            metadata: None,
            changed_paths: Vec::new(),
        })
    }

    pub(crate) async fn task_update(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let task_id = task_id_field(&payload)?;
        let task_dir = ensure_task_list_dir(context).await?;
        let _lock = acquire_task_lock(&task_dir, context).await?;
        let Some(mut task) = read_task_record(&task_dir, &task_id).await? else {
            return Ok(ToolOutcome {
                name: "task-update".to_string(),
                summary: format!("Task #{task_id} not found."),
                output: format!("Task #{task_id} not found"),
                metadata: None,
                changed_paths: Vec::new(),
            });
        };

        if string_field_any(&payload, &["status"]).as_deref() == Some("deleted") {
            let task_path = task_file_path(&task_dir, &task_id);
            if tokio::fs::try_exists(&task_path).await? {
                tokio::fs::remove_file(&task_path).await?;
                update_high_water_mark_after_delete(&task_dir, &task_id).await?;
                remove_task_references(&task_dir, &task_id).await?;
            }
            return Ok(ToolOutcome {
                name: "task-update".to_string(),
                summary: format!("Deleted task #{task_id}."),
                output: format!("Updated task #{task_id} deleted"),
                metadata: None,
                changed_paths: vec![task_path],
            });
        }

        let mut updated_fields = Vec::new();

        if let Some(subject) = string_field(&payload, "subject")
            && subject != task.subject
        {
            task.subject = subject;
            updated_fields.push("subject");
        }
        if let Some(description) = string_field(&payload, "description")
            && description != task.description
        {
            task.description = description;
            updated_fields.push("description");
        }
        if let Some(active_form) = payload.get("activeForm") {
            let next_active_form = match active_form {
                Value::Null => None,
                Value::String(value) => Some(value.clone()),
                _ => {
                    return Err(ToolError::InvalidInput(
                        "`activeForm` must be a string or null".into(),
                    ));
                }
            };
            if next_active_form != task.active_form {
                task.active_form = next_active_form;
                updated_fields.push("activeForm");
            }
        }
        if let Some(owner) = payload.get("owner") {
            let next_owner = match owner {
                Value::Null => None,
                Value::String(value) => Some(value.clone()),
                _ => {
                    return Err(ToolError::InvalidInput(
                        "`owner` must be a string or null".into(),
                    ));
                }
            };
            if next_owner != task.owner {
                task.owner = next_owner;
                updated_fields.push("owner");
            }
        }
        if let Some(status) = string_field(&payload, "status") {
            let next_status = TaskStatus::parse(&status).ok_or_else(|| {
                ToolError::InvalidInput(
                    "task-update `status` must be pending, in_progress, completed, or deleted"
                        .into(),
                )
            })?;
            if next_status != task.status {
                if !task.status.can_transition_to(next_status) {
                    return Err(ToolError::InvalidInput(format!(
                        "Cannot transition task from {} to {}",
                        task.status.as_str(),
                        next_status.as_str(),
                    )));
                }
                task.status = next_status;
                updated_fields.push("status");
            }
        }
        if let Some(metadata_patch) = object_field(&payload, "metadata")? {
            let current = task.metadata.get_or_insert_with(serde_json::Map::new);
            apply_metadata_patch(current, metadata_patch);
            if current.is_empty() {
                task.metadata = None;
            }
            updated_fields.push("metadata");
        }

        let replace_blocks = task_id_array_optional_any(&payload, &["blocks"])?;
        let add_blocks = task_id_array_any(&payload, &["addBlocks"])?;
        let replace_blocked_by = task_id_array_optional_any(&payload, &["blockedBy"])?;
        let add_blocked_by = task_id_array_any(&payload, &["addBlockedBy"])?;

        let next_blocks = merge_task_links(task.blocks.clone(), replace_blocks, add_blocks);
        let next_blocked_by =
            merge_task_links(task.blocked_by.clone(), replace_blocked_by, add_blocked_by);
        validate_task_links_exist(&task_dir, &next_blocks).await?;
        validate_task_links_exist(&task_dir, &next_blocked_by).await?;
        if next_blocks != task.blocks {
            reconcile_task_links(
                &task_dir,
                &task.id,
                &task.blocks,
                &next_blocks,
                TaskLinkDirection::Blocks,
            )
            .await?;
            task.blocks = next_blocks;
            updated_fields.push("blocks");
        }
        if next_blocked_by != task.blocked_by {
            reconcile_task_links(
                &task_dir,
                &task.id,
                &task.blocked_by,
                &next_blocked_by,
                TaskLinkDirection::BlockedBy,
            )
            .await?;
            task.blocked_by = next_blocked_by;
            updated_fields.push("blockedBy");
        }

        let task_path = task_file_path(&task_dir, &task.id);
        write_task_record(&task_path, &task).await?;

        let summary = if updated_fields.is_empty() {
            format!("No changes applied to task #{}.", task.id)
        } else {
            format!("Updated task #{} ({}).", task.id, updated_fields.join(", "))
        };
        let output = if updated_fields.is_empty() {
            format!("Updated task #{} (no changes)", task.id)
        } else {
            format!("Updated task #{} {}", task.id, updated_fields.join(", "))
        };
        Ok(ToolOutcome {
            name: "task-update".to_string(),
            summary,
            output,
            metadata: None,
            changed_paths: vec![task_path],
        })
    }

    pub(crate) async fn task_output(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let task_id = string_field_any(&payload, &["task_id", "taskId"])
            .ok_or_else(|| ToolError::InvalidInput("task-output requires `task_id`".into()))?;
        let block = bool_field_keys(&payload, &["block"]).unwrap_or(true);
        let timeout_ms = usize_field_keys(&payload, &["timeout"]).unwrap_or(30_000) as u64;
        let offset = usize_field_keys(&payload, &["offset"]).map(|v| v as u64);
        let follow = bool_field_keys(&payload, &["follow"]).unwrap_or(false);
        let snapshot = read_background_task_record(&context.home_dir, &task_id).await?;
        let Some(snapshot) = snapshot else {
            return Err(ToolError::InvalidInput(format!(
                "no background task found with ID: {task_id}"
            )));
        };

        let result = if block || follow {
            wait_for_background_task(
                &context.home_dir,
                &task_id,
                timeout_ms,
                context.progress.clone(),
                context,
            )
            .await?
        } else {
            snapshot
        };
        let retrieval_status = if result.status.is_active() {
            if block || follow {
                "timeout"
            } else {
                "not_ready"
            }
        } else {
            "success"
        };
        let output = serde_json::to_string_pretty(&json!({
            "retrieval_status": retrieval_status,
            "task": background_task_output_payload(&result, offset).await?,
        }))?;

        Ok(ToolOutcome {
            name: "task-output".to_string(),
            summary: format!("Read background task #{}.", result.job_id),
            output,
            metadata: None,
            changed_paths: Vec::new(),
        })
    }

    pub(crate) async fn task_stop(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let task_id = string_field_any(&payload, &["task_id", "taskId", "shell_id"])
            .ok_or_else(|| ToolError::InvalidInput("task-stop requires `task_id`".into()))?;
        let Some(mut record) = read_background_task_record(&context.home_dir, &task_id).await?
        else {
            return Err(ToolError::InvalidInput(format!(
                "no background task found with ID: {task_id}"
            )));
        };
        if !record.status.is_active() {
            return Err(ToolError::InvalidInput(format!(
                "task {task_id} is not running (status: {:?})",
                record.status
            )));
        }

        let mut stop_detail = None;
        if let Some(pid) = record.pid {
            let mut command = Command::new("kill");
            command.arg("-TERM").arg(pid.to_string());
            match run_command_output(&mut command, context).await {
                Ok(output) if output.status.success() => {}
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    let detail = if stderr.is_empty() {
                        format!("signal exited with {}", output.status)
                    } else {
                        format!("signal exited with {}: {stderr}", output.status)
                    };
                    stop_detail = Some(format!("Cancelled via TaskStop; {detail}"));
                }
                Err(error) => {
                    stop_detail = Some(format!("Cancelled via TaskStop; signal error: {error}"));
                }
            }
        }

        let signalled_in_process = crate::background_cancellation::cancel_background_task(&task_id);
        if record.pid.is_none() && !signalled_in_process {
            stop_detail = Some(
                "Cancelled via TaskStop; no in-process worker was listening — record marked cancelled on disk only"
                    .to_string(),
            );
        }

        let now = Utc::now().to_rfc3339();
        record.status = BackgroundTaskStatus::Cancelled;
        record.updated_at = now.clone();
        record.finished_at = Some(now);
        record.error = Some(stop_detail.unwrap_or_else(|| "Cancelled via TaskStop".to_string()));
        write_background_task_record(&context.home_dir, &record).await?;

        Ok(ToolOutcome {
            name: "task-stop".to_string(),
            summary: format!("Stopped background task #{}.", record.job_id),
            output: serde_json::to_string_pretty(&json!({
                "message": format!("Successfully stopped task: {} ({})", record.job_id, truncate_description(&record.prompt)),
                "task_id": record.job_id.clone(),
                "task_type": record.task_kind.as_str(),
                "command": truncate_description(&record.prompt),
            }))?,
            metadata: None,
            changed_paths: vec![background_job_path(&context.home_dir, &task_id)],
        })
    }
}

async fn ensure_task_list_dir(context: &ToolContext) -> Result<PathBuf, ToolError> {
    let task_list_id = session_task_list_id(context.session_id.as_deref());
    let task_dir = context.home_dir.join("tasks").join(&task_list_id);
    tokio::fs::create_dir_all(&task_dir).await?;
    Ok(task_dir)
}

pub fn session_task_list_id(session_id: Option<&str>) -> String {
    orbcode_config::resolve_process_env("ORBCODE_TASK_LIST_ID").unwrap_or_else(|| {
        session_id
            .filter(|s| !s.is_empty())
            .unwrap_or("default")
            .to_string()
    })
}

pub fn workspace_task_list_id(cwd: &Path) -> String {
    orbcode_config::resolve_process_env("ORBCODE_TASK_LIST_ID")
        .unwrap_or_else(|| sanitize_task_list_id(&cwd.display().to_string()))
}

pub fn workspace_task_list_dir(home_dir: &Path, cwd: &Path) -> PathBuf {
    home_dir.join("tasks").join(workspace_task_list_id(cwd))
}

pub(crate) fn sanitize_task_list_id(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut last_dash = false;
    for ch in value.chars() {
        let normalized = match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => ch,
            _ => '-',
        };
        if normalized == '-' {
            if last_dash {
                continue;
            }
            last_dash = true;
        } else {
            last_dash = false;
        }
        output.push(normalized);
    }
    let trimmed = output.trim_matches('-');
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

async fn acquire_task_lock(
    task_dir: &Path,
    context: &ToolContext,
) -> Result<TaskListLock, ToolError> {
    let lock_path = task_dir.join(".lock");
    for _ in 0..TASK_LOCK_RETRIES {
        ensure_not_cancelled(context)?;
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .await
        {
            Ok(_) => return Ok(TaskListLock { path: lock_path }),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                tokio::select! {
                    _ = sleep(Duration::from_millis(TASK_LOCK_RETRY_MS)) => {}
                    _ = context.cancellation.cancelled() => return Err(ToolError::Interrupted),
                }
            }
            Err(error) => return Err(ToolError::Io(error)),
        }
    }
    Err(ToolError::ExecutionFailed(
        "timed out while acquiring workspace task lock".to_string(),
    ))
}

fn task_file_path(task_dir: &Path, task_id: &str) -> PathBuf {
    task_dir.join(format!("{task_id}.json"))
}

fn task_high_water_mark_path(task_dir: &Path) -> PathBuf {
    task_dir.join(".highwatermark")
}

async fn read_task_record(task_dir: &Path, task_id: &str) -> Result<Option<TaskRecord>, ToolError> {
    let path = task_file_path(task_dir, task_id);
    if !tokio::fs::try_exists(&path).await? {
        return Ok(None);
    }
    let contents = tokio::fs::read_to_string(&path).await?;
    Ok(Some(serde_json::from_str(&contents)?))
}

async fn write_task_record(path: &Path, task: &TaskRecord) -> Result<(), ToolError> {
    tokio::fs::write(path, serde_json::to_string_pretty(task)?).await?;
    Ok(())
}

async fn list_task_records(task_dir: &Path) -> Result<Vec<TaskRecord>, ToolError> {
    if !tokio::fs::try_exists(task_dir).await? {
        return Ok(Vec::new());
    }
    let mut entries = tokio::fs::read_dir(task_dir).await?;
    let mut tasks = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let contents = tokio::fs::read_to_string(&path).await?;
        tasks.push(serde_json::from_str::<TaskRecord>(&contents)?);
    }
    tasks.sort_by_key(task_sort_key);
    Ok(tasks)
}

fn task_sort_key(task: &TaskRecord) -> (u64, String) {
    (task.id.parse::<u64>().unwrap_or(u64::MAX), task.id.clone())
}

async fn next_task_id(task_dir: &Path) -> Result<String, ToolError> {
    let high_water_mark = read_high_water_mark(task_dir).await?;
    let current_max = list_task_records(task_dir)
        .await?
        .into_iter()
        .filter_map(|task| task.id.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    let next = high_water_mark.max(current_max) + 1;
    tokio::fs::write(task_high_water_mark_path(task_dir), next.to_string()).await?;
    Ok(next.to_string())
}

async fn read_high_water_mark(task_dir: &Path) -> Result<u64, ToolError> {
    let path = task_high_water_mark_path(task_dir);
    if !tokio::fs::try_exists(&path).await? {
        return Ok(0);
    }
    let value = tokio::fs::read_to_string(path).await?;
    Ok(value.trim().parse::<u64>().unwrap_or(0))
}

async fn update_high_water_mark_after_delete(
    task_dir: &Path,
    task_id: &str,
) -> Result<(), ToolError> {
    if let Ok(task_numeric_id) = task_id.parse::<u64>() {
        let current = read_high_water_mark(task_dir).await?;
        if task_numeric_id > current {
            tokio::fs::write(
                task_high_water_mark_path(task_dir),
                task_numeric_id.to_string(),
            )
            .await?;
        }
    }
    Ok(())
}

async fn validate_task_links_exist(task_dir: &Path, ids: &[String]) -> Result<(), ToolError> {
    for id in ids {
        if read_task_record(task_dir, id).await?.is_none() {
            return Err(ToolError::InvalidInput(format!(
                "referenced task #{id} does not exist in the current workspace list"
            )));
        }
    }
    Ok(())
}

fn merge_task_links(
    current: Vec<String>,
    replace_with: Option<Vec<String>>,
    add_values: Vec<String>,
) -> Vec<String> {
    let mut merged = replace_with.unwrap_or(current);
    for value in add_values {
        if !merged.contains(&value) {
            merged.push(value);
        }
    }
    merged
}

#[derive(Clone, Copy)]
enum TaskLinkDirection {
    Blocks,
    BlockedBy,
}

async fn reconcile_task_links(
    task_dir: &Path,
    source_id: &str,
    previous: &[String],
    next: &[String],
    direction: TaskLinkDirection,
) -> Result<(), ToolError> {
    let previous_set = previous.iter().cloned().collect::<BTreeSet<_>>();
    let next_set = next.iter().cloned().collect::<BTreeSet<_>>();
    for removed in previous_set.difference(&next_set) {
        unlink_tasks(task_dir, source_id, removed, direction).await?;
    }
    for added in next_set.difference(&previous_set) {
        link_tasks(task_dir, source_id, added, direction).await?;
    }
    Ok(())
}

async fn remove_task_references(task_dir: &Path, target_id: &str) -> Result<(), ToolError> {
    let tasks = list_task_records(task_dir).await?;
    for mut task in tasks {
        let original_blocks_len = task.blocks.len();
        let original_blocked_by_len = task.blocked_by.len();
        task.blocks.retain(|id| id != target_id);
        task.blocked_by.retain(|id| id != target_id);
        if task.blocks.len() != original_blocks_len
            || task.blocked_by.len() != original_blocked_by_len
        {
            write_task_record(&task_file_path(task_dir, &task.id), &task).await?;
        }
    }
    Ok(())
}

async fn link_tasks(
    task_dir: &Path,
    source_id: &str,
    target_id: &str,
    direction: TaskLinkDirection,
) -> Result<(), ToolError> {
    if source_id == target_id {
        return Ok(());
    }
    let Some(mut target) = read_task_record(task_dir, target_id).await? else {
        return Err(ToolError::InvalidInput(format!(
            "referenced task #{target_id} does not exist in the current workspace list"
        )));
    };
    match direction {
        TaskLinkDirection::Blocks => {
            if !target.blocked_by.iter().any(|id| id == source_id) {
                target.blocked_by.push(source_id.to_string());
            }
        }
        TaskLinkDirection::BlockedBy => {
            if !target.blocks.iter().any(|id| id == source_id) {
                target.blocks.push(source_id.to_string());
            }
        }
    }
    write_task_record(&task_file_path(task_dir, target_id), &target).await
}

async fn unlink_tasks(
    task_dir: &Path,
    source_id: &str,
    target_id: &str,
    direction: TaskLinkDirection,
) -> Result<(), ToolError> {
    if source_id == target_id {
        return Ok(());
    }
    let Some(mut target) = read_task_record(task_dir, target_id).await? else {
        return Ok(());
    };
    match direction {
        TaskLinkDirection::Blocks => target.blocked_by.retain(|id| id != source_id),
        TaskLinkDirection::BlockedBy => target.blocks.retain(|id| id != source_id),
    }
    write_task_record(&task_file_path(task_dir, target_id), &target).await
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskView {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub active_form: Option<String>,
    pub owner: Option<String>,
    pub status: TaskStatusKind,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
    pub open_blockers: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskListSummary {
    pub total: usize,
    pub completed: usize,
    pub in_progress: usize,
    pub pending: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskListSnapshot {
    pub task_list_id: String,
    pub directory: PathBuf,
    pub tasks: Vec<TaskView>,
    pub summary: TaskListSummary,
    pub fingerprint: u128,
}

pub async fn load_task_list_snapshot(
    home_dir: &Path,
    task_list_id: &str,
) -> Result<TaskListSnapshot, ToolError> {
    let task_list_id = task_list_id.to_string();
    let directory = home_dir.join("tasks").join(&task_list_id);
    let records = if tokio::fs::try_exists(&directory).await? {
        list_task_records(&directory).await?
    } else {
        Vec::new()
    };

    let completed_ids: BTreeSet<String> = records
        .iter()
        .filter(|task| task.status == TaskStatus::Completed)
        .map(|task| task.id.clone())
        .collect();

    let mut summary = TaskListSummary::default();
    let mut tasks = Vec::with_capacity(records.len());
    for record in &records {
        let status_kind = TaskStatusKind::from(record.status);
        summary.total += 1;
        match status_kind {
            TaskStatusKind::Completed => summary.completed += 1,
            TaskStatusKind::InProgress => summary.in_progress += 1,
            TaskStatusKind::Pending => summary.pending += 1,
        }
        let open_blockers: Vec<String> = record
            .blocked_by
            .iter()
            .filter(|id| !completed_ids.contains(*id))
            .cloned()
            .collect();
        tasks.push(TaskView {
            id: record.id.clone(),
            subject: record.subject.clone(),
            description: record.description.clone(),
            active_form: record.active_form.clone(),
            owner: record.owner.clone(),
            status: status_kind,
            blocks: record.blocks.clone(),
            blocked_by: record.blocked_by.clone(),
            open_blockers,
        });
    }

    let fingerprint = directory_fingerprint(&directory).await;
    Ok(TaskListSnapshot {
        task_list_id,
        directory,
        tasks,
        summary,
        fingerprint,
    })
}

async fn directory_fingerprint(directory: &Path) -> u128 {
    let Ok(metadata_exists) = tokio::fs::try_exists(directory).await else {
        return 0;
    };
    if !metadata_exists {
        return 0;
    }
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return 0;
    };
    let mut fingerprint: u128 = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if extension != "json" {
            continue;
        }
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        fingerprint = fingerprint.wrapping_add(modified_nanos);
        fingerprint = fingerprint.wrapping_add(metadata.len() as u128);
    }
    fingerprint
}

fn task_id_field(payload: &Value) -> Result<String, ToolError> {
    string_field_any(payload, &["taskId", "task_id"])
        .ok_or_else(|| ToolError::InvalidInput("task tool requires `taskId`".into()))
}

fn task_id_array_any(payload: &Value, keys: &[&str]) -> Result<Vec<String>, ToolError> {
    Ok(task_id_array_optional_any(payload, keys)?.unwrap_or_default())
}

fn task_id_array_optional_any(
    payload: &Value,
    keys: &[&str],
) -> Result<Option<Vec<String>>, ToolError> {
    for key in keys {
        if let Some(value) = payload.get(*key) {
            let items = value.as_array().ok_or_else(|| {
                ToolError::InvalidInput(format!("`{key}` must be an array of task IDs"))
            })?;
            let mut deduped = Vec::new();
            for item in items {
                let id = item
                    .as_str()
                    .ok_or_else(|| {
                        ToolError::InvalidInput(format!("`{key}` entries must be strings"))
                    })?
                    .to_string();
                if !deduped.contains(&id) {
                    deduped.push(id);
                }
            }
            return Ok(Some(deduped));
        }
    }
    Ok(None)
}

fn object_field(
    payload: &Value,
    key: &str,
) -> Result<Option<serde_json::Map<String, Value>>, ToolError> {
    match payload.get(key) {
        None => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map.clone())),
        Some(_) => Err(ToolError::InvalidInput(format!(
            "`{key}` must be an object"
        ))),
    }
}

fn apply_metadata_patch(
    current: &mut serde_json::Map<String, Value>,
    patch: serde_json::Map<String, Value>,
) {
    for (key, value) in patch {
        if value.is_null() {
            current.remove(&key);
        } else {
            current.insert(key, value);
        }
    }
}

pub fn background_jobs_dir(home_dir: &Path) -> PathBuf {
    home_dir.join("background").join("jobs")
}

pub fn background_logs_dir(home_dir: &Path) -> PathBuf {
    home_dir.join("background").join("logs")
}

pub fn background_job_path(home_dir: &Path, task_id: &str) -> PathBuf {
    background_jobs_dir(home_dir).join(format!("{task_id}.json"))
}

pub fn background_log_path(home_dir: &Path, task_id: &str) -> PathBuf {
    background_logs_dir(home_dir).join(format!("{task_id}.log"))
}

pub async fn read_background_task_record(
    home_dir: &Path,
    task_id: &str,
) -> Result<Option<BackgroundTaskRecord>, ToolError> {
    let path = background_job_path(home_dir, task_id);
    if !tokio::fs::try_exists(&path).await? {
        return Ok(None);
    }
    let contents = tokio::fs::read_to_string(path).await?;
    Ok(Some(serde_json::from_str(&contents)?))
}

pub async fn write_background_task_record(
    home_dir: &Path,
    record: &BackgroundTaskRecord,
) -> Result<(), ToolError> {
    let jobs_dir = background_jobs_dir(home_dir);
    tokio::fs::create_dir_all(&jobs_dir).await?;
    let path = background_job_path(home_dir, &record.job_id);
    let tmp = jobs_dir.join(format!("{}.{}.tmp", record.job_id, Uuid::new_v4()));
    tokio::fs::write(&tmp, serde_json::to_vec_pretty(record)?).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

/// Scan the background jobs directory for `local_agent` records whose parent
/// is `parent_session_id`. Returns oldest-first by `created_at` (string sort
/// is safe since all records use RFC3339 timestamps). Used by the TUI to
/// surface in-flight background agents next to the input box so the user can
/// see the durable task_id without depending on the model to quote it.
pub async fn list_local_agent_records_for_session(
    home_dir: &Path,
    parent_session_id: &str,
) -> Result<Vec<BackgroundTaskRecord>, ToolError> {
    let jobs_dir = background_jobs_dir(home_dir);
    if !tokio::fs::try_exists(&jobs_dir).await? {
        return Ok(Vec::new());
    }
    let mut entries = tokio::fs::read_dir(&jobs_dir).await?;
    let mut records = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<BackgroundTaskRecord>(&contents) else {
            continue;
        };
        if record.task_kind != BackgroundTaskKind::LocalAgent {
            continue;
        }
        if record.session_id != parent_session_id {
            continue;
        }
        records.push(record);
    }
    records.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    Ok(records)
}

async fn wait_for_background_task(
    home_dir: &Path,
    task_id: &str,
    timeout_ms: u64,
    progress: Option<Arc<dyn ToolProgressReporter>>,
    context: &ToolContext,
) -> Result<BackgroundTaskRecord, ToolError> {
    let started = std::time::Instant::now();
    loop {
        ensure_not_cancelled(context)?;
        let record = read_background_task_record(home_dir, task_id)
            .await?
            .ok_or_else(|| {
                ToolError::InvalidInput(format!("no background task found with ID: {task_id}"))
            })?;
        if !record.status.is_active() {
            return Ok(record);
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            return Ok(record);
        }
        if let Some(reporter) = &progress {
            reporter
                .report(json!({
                    "type": "waiting_for_task",
                    "task_id": task_id,
                    "taskDescription": truncate_description(&record.prompt),
                    "taskType": record.task_kind.as_str(),
                    "status": record.status.as_str(),
                }))
                .await?;
        }
        tokio::select! {
            _ = sleep(Duration::from_millis(100)) => {}
            _ = context.cancellation.cancelled() => return Err(ToolError::Interrupted),
        }
    }
}

const TERMINAL_TAIL_LINES: usize = 200;

async fn read_log_from_offset(
    log_path: &Path,
    start: u64,
    file_len: u64,
) -> Result<(String, u64, usize), ToolError> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
    if start >= file_len || !tokio::fs::try_exists(log_path).await? {
        return Ok((String::new(), start, 0));
    }
    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .open(log_path)
        .await?;
    file.seek(SeekFrom::Start(start)).await?;
    let remaining = (file_len - start) as usize;
    let mut buf = vec![0u8; remaining];
    file.read_exact(&mut buf).await?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let line_count = text.lines().count();
    let truncated = truncate_tool_output(
        text,
        MAX_WEB_OUTPUT_CHARS,
        "Task output truncated for transcript safety.",
    );
    Ok((truncated, file_len, line_count))
}

async fn read_log_tail(log_path: &Path, file_len: u64) -> Result<(String, u64, usize), ToolError> {
    if file_len == 0 || !tokio::fs::try_exists(log_path).await? {
        return Ok((String::new(), 0, 0));
    }
    let full = tokio::fs::read_to_string(log_path).await?;
    let all_lines: Vec<&str> = full.lines().collect();
    let total_lines = all_lines.len();
    if total_lines <= TERMINAL_TAIL_LINES {
        let truncated = truncate_tool_output(
            full,
            MAX_WEB_OUTPUT_CHARS,
            "Task output truncated for transcript safety.",
        );
        return Ok((truncated, file_len, total_lines));
    }
    let tail = &all_lines[total_lines - TERMINAL_TAIL_LINES..];
    let header = format!("[showing last {TERMINAL_TAIL_LINES} of {total_lines} lines]\n");
    let body = tail.join("\n");
    let output = format!("{header}{body}");
    let truncated = truncate_tool_output(
        output,
        MAX_WEB_OUTPUT_CHARS,
        "Task output truncated for transcript safety.",
    );
    Ok((truncated, file_len, total_lines))
}

async fn background_task_output_payload(
    record: &BackgroundTaskRecord,
    offset: Option<u64>,
) -> Result<Value, ToolError> {
    let log_path = PathBuf::from(&record.log_path);
    let log_bytes = tokio::fs::metadata(&log_path)
        .await
        .map_or(0, |metadata| metadata.len());

    let (output, next_offset, total_lines) = if let Some(start) = offset {
        read_log_from_offset(&log_path, start, log_bytes).await?
    } else if record.status.is_terminal() {
        read_log_tail(&log_path, log_bytes).await?
    } else {
        let raw = if tokio::fs::try_exists(&log_path).await? {
            tokio::fs::read_to_string(&log_path).await?
        } else {
            String::new()
        };
        let next = log_bytes;
        let total = raw.lines().count();
        let truncated = truncate_tool_output(
            raw,
            MAX_WEB_OUTPUT_CHARS,
            "Task output truncated for transcript safety.",
        );
        (truncated, next, total)
    };

    let result = if record.status.is_active() {
        Value::Null
    } else if let Some(explicit) = record.result.clone() {
        Value::String(explicit)
    } else {
        Value::String(output.clone())
    };
    let mut payload = json!({
        "task_id": record.job_id.clone(),
        "task_type": record.task_kind.as_str(),
        "status": record.status.as_str(),
        "description": truncate_description(&record.prompt),
        "output": output,
        "output_path": record.log_path.clone(),
        "error": record.error.clone(),
        "result": result,
        "exit_code": record.exit_code,
        "signal": record.signal,
        "next_offset": next_offset,
        "total_lines": total_lines,
        "progress": {
            "active": record.status.is_active(),
            "created_at": record.created_at.clone(),
            "updated_at": record.updated_at.clone(),
            "started_at": record.started_at.clone(),
            "finished_at": record.finished_at.clone(),
            "pid": record.pid,
            "log_bytes": log_bytes,
        },
    });
    if record.task_kind == BackgroundTaskKind::LocalAgent {
        let map = payload.as_object_mut().expect("payload is an object");
        if let Some(child) = record.child_session_id.clone() {
            map.insert("child_session_id".to_string(), Value::String(child));
        }
        if let Some(tool_use_id) = record.tool_use_id.clone() {
            map.insert("tool_use_id".to_string(), Value::String(tool_use_id));
        }
        if let Some(agent_type) = record.agent_type.clone() {
            map.insert("agent_type".to_string(), Value::String(agent_type));
        }
        if let Some(model) = record.model.clone() {
            map.insert("model".to_string(), Value::String(model));
        }
        if let Some(permission_mode) = record.permission_mode {
            map.insert(
                "permission_mode".to_string(),
                Value::String(permission_mode.as_str().to_string()),
            );
        }
    }
    Ok(payload)
}

fn truncate_description(value: &str) -> String {
    const MAX_DESCRIPTION_LEN: usize = 120;
    let mut chars = value.chars();
    let preview = chars.by_ref().take(MAX_DESCRIPTION_LEN).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

fn parse_todo_items(payload: &Value, raw: &str) -> Result<Vec<TodoItem>, ToolError> {
    if let Some(items) = payload.get("items").and_then(Value::as_array) {
        let parsed = items
            .iter()
            .map(|item| match item {
                Value::String(title) => Ok(TodoItem {
                    title: title.clone(),
                    done: false,
                }),
                Value::Object(_) => {
                    let title = item
                        .get("title")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            ToolError::InvalidInput("todo item objects require `title`".into())
                        })?
                        .to_string();
                    let done = item.get("done").and_then(Value::as_bool).unwrap_or(false);
                    Ok(TodoItem { title, done })
                }
                _ => Err(ToolError::InvalidInput(
                    "todo items must be strings or objects".into(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !parsed.is_empty() {
            return Ok(parsed);
        }
    }

    let parsed = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| TodoItem {
            title: line.to_string(),
            done: false,
        })
        .collect::<Vec<_>>();
    if parsed.is_empty() {
        Err(ToolError::InvalidInput(
            "todo-write requires at least one item".into(),
        ))
    } else {
        Ok(parsed)
    }
}
