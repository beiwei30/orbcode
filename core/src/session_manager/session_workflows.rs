use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::{DateTime, Utc};
use orbcode_config::AgentDefinition;
use orbcode_config::PermissionRuleSettingKind;
use orbcode_protocol::{BackgroundTaskProgressEvent, StreamEvent};
use orbcode_session_store::{ChildSessionStatus, StartChildSessionInput};
use orbcode_tools::{
    AgentToolInput, BackgroundTaskRecord, BackgroundTaskStatus, SkillDefinition,
    background_log_path, read_background_task_record, register_background_task_cancel_flag,
    register_progress_stream, task_record_to_view, unregister_background_task_cancel_flag,
    unregister_progress_stream, write_background_task_record,
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncWriteExt,
    sync::{Semaphore, broadcast, mpsc},
};
use uuid::Uuid;

use super::{
    SessionManager,
    session_agent_tool::{AgentLoopOutcome, apply_agent_permission_mode},
    session_background_agent::shutdown_child_mcp_registry,
};
use crate::CoreError;

const DEFAULT_MAX_CONCURRENCY: usize = 4;
const MAX_CONCURRENCY_LIMIT: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowSource {
    Project,
    User,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowCommand {
    pub name: String,
    pub description: String,
    pub source: WorkflowSource,
}

#[derive(Clone, Debug)]
struct DiscoveredWorkflow {
    command: WorkflowCommand,
    spec: WorkflowSpec,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorkflowSpec {
    schema_version: u32,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    max_concurrency: Option<usize>,
    #[serde(default)]
    steps: Vec<WorkflowStepRaw>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct WorkflowStepRaw {
    #[serde(default)]
    agent: Option<AgentStep>,
    #[serde(default)]
    parallel: Option<WorkflowStepList>,
    #[serde(default)]
    pipeline: Option<WorkflowStepList>,
    #[serde(default)]
    phase: Option<PhaseStep>,
    #[serde(default)]
    log: Option<LogStep>,
    #[serde(default, flatten, skip_serializing_if = "BTreeMap::is_empty")]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorkflowStepList {
    steps: Vec<WorkflowStepRaw>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AgentStep {
    description: String,
    prompt: String,
    #[serde(default)]
    subagent_type: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PhaseStep {
    name: String,
    steps: Vec<WorkflowStepRaw>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LogStep {
    message: String,
}

#[derive(Clone, Debug)]
struct WorkflowPlan {
    name: String,
    description: String,
    max_concurrency: usize,
    steps: Vec<WorkflowStep>,
    spec: WorkflowSpec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkflowValidationMode {
    Saved,
    DynamicGenerated,
}

#[derive(Clone, Debug)]
enum WorkflowStep {
    Agent(AgentStep),
    Parallel(Vec<WorkflowStep>),
    Pipeline(Vec<WorkflowStep>),
    Phase {
        name: String,
        steps: Vec<WorkflowStep>,
    },
    Log(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkflowRunStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl WorkflowRunStatus {
    fn as_background_status(self) -> BackgroundTaskStatus {
        match self {
            Self::Running => BackgroundTaskStatus::Running,
            Self::Completed => BackgroundTaskStatus::Completed,
            Self::Failed => BackgroundTaskStatus::Failed,
            Self::Cancelled => BackgroundTaskStatus::Cancelled,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorkflowRunRecord {
    run_id: String,
    session_id: String,
    workflow_name: String,
    arguments: String,
    status: WorkflowRunStatus,
    created_at: String,
    updated_at: String,
    finished_at: Option<String>,
    result: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorkflowJournalEvent {
    timestamp: String,
    event: String,
    step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    message: Option<String>,
    output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    child_session_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct WorkflowJournal {
    completed_outputs: HashMap<String, String>,
}

#[cfg(test)]
#[derive(Clone, Default)]
struct FakeWorkflowAgentExecutor {
    calls: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    active: Arc<std::sync::atomic::AtomicUsize>,
    max_active: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl FakeWorkflowAgentExecutor {
    async fn invoke(
        &self,
        key: &str,
        agent: &AgentToolInput,
        cancel_flag: &Arc<AtomicBool>,
    ) -> Result<String, WorkflowError> {
        self.calls
            .lock()
            .expect("fake calls poisoned")
            .push((key.to_string(), agent.prompt.clone()));
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        let _guard = FakeActiveAgentGuard { fake: self.clone() };

        if let Some(message) = agent.prompt.strip_prefix("fail:") {
            return Err(CoreError::Tool(message.to_string()).into());
        }
        if agent.prompt.starts_with("wait") {
            while !cancel_flag.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            return Err(WorkflowError::Cancelled);
        }
        if let Some(rest) = agent.prompt.strip_prefix("delay:") {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            return Ok(rest.to_string());
        }
        Ok(agent.prompt.clone())
    }

    fn call_prompts(&self) -> Vec<String> {
        self.calls
            .lock()
            .expect("fake calls poisoned")
            .iter()
            .map(|(_, prompt)| prompt.clone())
            .collect()
    }

    fn max_active(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
struct FakeActiveAgentGuard {
    fake: FakeWorkflowAgentExecutor,
}

#[cfg(test)]
impl Drop for FakeActiveAgentGuard {
    fn drop(&mut self) {
        self.fake.active.fetch_sub(1, Ordering::SeqCst);
    }
}

impl SessionManager {
    pub async fn list_workflows(&self) -> Result<Vec<WorkflowCommand>, CoreError> {
        let config = self.effective_config();
        Ok(discover_workflows(&config.home_dir, &config.cwd)
            .await?
            .into_iter()
            .map(|workflow| workflow.command)
            .collect())
    }

    pub async fn start_workflow(
        &self,
        session_id: &str,
        name: &str,
        arguments: &str,
    ) -> Result<String, CoreError> {
        let config = self.effective_config();
        let discovered = discover_workflows(&config.home_dir, &config.cwd)
            .await?
            .into_iter()
            .find(|workflow| workflow.command.name == name)
            .ok_or_else(|| CoreError::Config(format!("workflow not found: {name}")))?;
        let plan = validate_workflow_spec(
            discovered.command.name,
            discovered.command.description,
            discovered.spec,
        )?;
        self.spawn_workflow_run(session_id, plan, arguments.to_string(), None, None)
            .await
    }

    pub async fn start_dynamic_workflow(
        &self,
        session_id: &str,
        name: &str,
        spec: serde_json::Value,
        arguments: &str,
    ) -> Result<String, CoreError> {
        self.start_dynamic_workflow_with_progress_tx(session_id, name, spec, arguments, None)
            .await
    }

    pub(super) async fn start_dynamic_workflow_with_progress_tx(
        &self,
        session_id: &str,
        name: &str,
        spec: serde_json::Value,
        arguments: &str,
        progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
    ) -> Result<String, CoreError> {
        let workflow_name = if name.trim().is_empty() {
            "dynamic".to_string()
        } else {
            name.trim().to_string()
        };
        let spec: WorkflowSpec = serde_json::from_value(spec)?;
        let description = fallback_description(&workflow_name, &spec);
        let plan = validate_workflow_spec_with_mode(
            workflow_name,
            description,
            spec,
            WorkflowValidationMode::DynamicGenerated,
        )?;
        self.spawn_workflow_run(session_id, plan, arguments.to_string(), None, progress_tx)
            .await
    }

    pub async fn resume_workflow(&self, run_id: &str) -> Result<String, CoreError> {
        let config = self.effective_config();
        let run_dir = workflow_run_dir(&config.home_dir, run_id);
        let run: WorkflowRunRecord = read_json(&run_dir.join("run.json")).await?;
        let spec: WorkflowSpec = read_json(&run_dir.join("workflow.json")).await?;
        let description = fallback_description(&run.workflow_name, &spec);
        let plan = validate_workflow_spec(run.workflow_name.clone(), description, spec)?;
        self.spawn_workflow_run(
            &run.session_id,
            plan,
            run.arguments,
            Some(run_id.to_string()),
            None,
        )
        .await
    }

    async fn spawn_workflow_run(
        &self,
        session_id: &str,
        plan: WorkflowPlan,
        arguments: String,
        resume_run_id: Option<String>,
        progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
    ) -> Result<String, CoreError> {
        let config = self.effective_config();
        self.persist_workflow_parent_session_context(session_id)
            .await?;
        let run_id =
            resume_run_id.unwrap_or_else(|| format!("workflow-{}", Uuid::new_v4().simple()));
        let run_dir = workflow_run_dir(&config.home_dir, &run_id);
        tokio::fs::create_dir_all(&run_dir).await?;
        write_json(&run_dir.join("workflow.json"), &plan.spec).await?;

        let now = Utc::now().to_rfc3339();
        let run = WorkflowRunRecord {
            run_id: run_id.clone(),
            session_id: session_id.to_string(),
            workflow_name: plan.name.clone(),
            arguments: arguments.clone(),
            status: WorkflowRunStatus::Running,
            created_at: now.clone(),
            updated_at: now,
            finished_at: None,
            result: None,
            error: None,
        };
        write_json(&run_dir.join("run.json"), &run).await?;

        let log_path = background_log_path(&config.home_dir, &run_id);
        if let Some(parent) = log_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if !tokio::fs::try_exists(&log_path).await? {
            tokio::fs::write(&log_path, "").await?;
        }
        let mut record = BackgroundTaskRecord::new_workflow(
            run_id.clone(),
            session_id.to_string(),
            plan.description.clone(),
            config.cwd.display().to_string(),
            log_path.display().to_string(),
        );
        record.extra.insert(
            "workflow_name".to_string(),
            serde_json::Value::String(plan.name.clone()),
        );
        record.extra.insert(
            "run_dir".to_string(),
            serde_json::Value::String(run_dir.display().to_string()),
        );
        write_background_task_record(&config.home_dir, &record).await?;

        let cancel_flag = Arc::new(AtomicBool::new(false));
        register_background_task_cancel_flag(&run_id, cancel_flag.clone());
        let progress_broadcast_tx = register_progress_stream(&run_id, 256);
        let manager = self.clone();
        let session_id = session_id.to_string();
        let home_dir = config.home_dir.clone();
        let run_id_for_task = run_id.clone();
        tokio::spawn(async move {
            let result = manager
                .execute_workflow_run(
                    &home_dir,
                    &run_id_for_task,
                    &session_id,
                    plan,
                    arguments,
                    cancel_flag,
                    progress_tx,
                    Some(progress_broadcast_tx),
                )
                .await;
            if let Err(error) = result {
                eprintln!("workflow {run_id_for_task}: failed to finalize: {error}");
            }
            unregister_background_task_cancel_flag(&run_id_for_task);
            unregister_progress_stream(&run_id_for_task);
        });

        Ok(run_id)
    }

    async fn persist_workflow_parent_session_context(
        &self,
        session_id: &str,
    ) -> Result<(), CoreError> {
        self.transcript_store
            .append_session_context_line(
                session_id,
                &self.runtime_additional_directories(),
                &self.session_permission_rules(PermissionRuleSettingKind::Allow),
                &self.session_permission_rules(PermissionRuleSettingKind::Deny),
                self.runtime_effort_override(),
            )
            .await?;
        Ok(())
    }

    async fn execute_workflow_run(
        &self,
        home_dir: &Path,
        run_id: &str,
        session_id: &str,
        plan: WorkflowPlan,
        arguments: String,
        cancel_flag: Arc<AtomicBool>,
        progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
        progress_broadcast_tx: Option<broadcast::Sender<StreamEvent>>,
    ) -> Result<(), CoreError> {
        let run_dir = workflow_run_dir(home_dir, run_id);
        let journal_path = run_dir.join("journal.jsonl");
        let journal = load_journal(&journal_path).await?;
        append_journal(
            &journal_path,
            WorkflowJournalEvent {
                timestamp: Utc::now().to_rfc3339(),
                event: "run_started".to_string(),
                step_key: None,
                kind: None,
                message: Some(plan.name.clone()),
                output: None,
                child_session_id: None,
            },
        )
        .await?;

        let semaphore = Arc::new(Semaphore::new(plan.max_concurrency));
        let mut ctx = WorkflowExecutionContext {
            manager: self.clone(),
            home_dir: home_dir.to_path_buf(),
            run_id: run_id.to_string(),
            run_dir,
            session_id: session_id.to_string(),
            arguments,
            cancel_flag,
            semaphore,
            journal_path,
            completed_outputs: journal.completed_outputs,
            progress_tx,
            progress_broadcast_tx,
            #[cfg(test)]
            fake_agents: None,
        };

        let outcome = ctx
            .run_steps(&plan.steps, "step".to_string(), None, false)
            .await;
        let (status, result, error) = match outcome {
            Ok(output) => (WorkflowRunStatus::Completed, Some(output), None),
            Err(WorkflowError::Cancelled) => (WorkflowRunStatus::Cancelled, None, None),
            Err(WorkflowError::Failed(error)) => {
                (WorkflowRunStatus::Failed, None, Some(error.to_string()))
            }
        };
        ctx.finalize(status, result, error).await
    }
}

struct WorkflowExecutionContext {
    manager: SessionManager,
    home_dir: PathBuf,
    run_id: String,
    run_dir: PathBuf,
    session_id: String,
    arguments: String,
    cancel_flag: Arc<AtomicBool>,
    semaphore: Arc<Semaphore>,
    journal_path: PathBuf,
    completed_outputs: HashMap<String, String>,
    progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
    progress_broadcast_tx: Option<broadcast::Sender<StreamEvent>>,
    #[cfg(test)]
    fake_agents: Option<FakeWorkflowAgentExecutor>,
}

#[derive(Clone, Debug)]
struct WorkflowAgentInvocationIds {
    agent_id: String,
    child_session_id: String,
    tool_use_id: String,
}

#[derive(Debug)]
enum WorkflowError {
    Cancelled,
    Failed(CoreError),
}

impl From<CoreError> for WorkflowError {
    fn from(value: CoreError) -> Self {
        Self::Failed(value)
    }
}

impl From<std::io::Error> for WorkflowError {
    fn from(value: std::io::Error) -> Self {
        Self::Failed(CoreError::Io(value))
    }
}

impl WorkflowExecutionContext {
    fn run_steps<'a>(
        &'a mut self,
        steps: &'a [WorkflowStep],
        prefix: String,
        mut pipeline_input: Option<String>,
        pass_pipeline_output: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, WorkflowError>> + Send + 'a>> {
        Box::pin(async move {
            let mut outputs = Vec::new();
            for (index, step) in steps.iter().enumerate() {
                self.ensure_not_cancelled()?;
                let key = format!("{prefix}.{index}");
                let output = self.run_step(step, key, pipeline_input.take()).await?;
                if pass_pipeline_output {
                    pipeline_input = Some(output.clone());
                }
                if !output.is_empty() {
                    outputs.push(output);
                }
            }
            Ok(outputs.join("\n"))
        })
    }

    fn run_step<'a>(
        &'a mut self,
        step: &'a WorkflowStep,
        key: String,
        pipeline_input: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<String, WorkflowError>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(output) = self.completed_outputs.get(&key).cloned() {
                return Ok(output);
            }

            self.start_step(&key, step).await?;
            let result = match step {
                WorkflowStep::Agent(agent) => {
                    self.run_agent_step(&key, agent, pipeline_input).await
                }
                WorkflowStep::Log(message) => {
                    let rendered = render_arguments(message, &self.arguments);
                    self.append_log_line(&rendered).await?;
                    self.complete_step(&key, rendered.clone()).await?;
                    Ok(rendered)
                }
                WorkflowStep::Phase { name, steps } => {
                    let rendered = render_arguments(name, &self.arguments);
                    self.append_log_line(&format!("== {rendered} ==")).await?;
                    let output = self
                        .run_steps(steps, key.clone(), pipeline_input, false)
                        .await?;
                    self.complete_step(&key, output.clone()).await?;
                    Ok(output)
                }
                WorkflowStep::Pipeline(steps) => {
                    let output = self
                        .run_steps(steps, key.clone(), pipeline_input, true)
                        .await?;
                    self.complete_step(&key, output.clone()).await?;
                    Ok(output)
                }
                WorkflowStep::Parallel(steps) => self.run_parallel_steps(&key, steps).await,
            };
            if let Err(error) = &result {
                match error {
                    WorkflowError::Cancelled => self.cancel_step(&key).await?,
                    WorkflowError::Failed(error) => {
                        self.fail_step(&key, error.to_string()).await?;
                    }
                }
            }
            result
        })
    }

    async fn run_parallel_steps(
        &mut self,
        key: &str,
        steps: &[WorkflowStep],
    ) -> Result<String, WorkflowError> {
        // Spawn every branch into a `JoinSet` so we observe whichever finishes
        // FIRST — including a fast failure — regardless of spawn order. Awaiting
        // handles in creation order (the old approach) blocked on a slow first
        // branch, so a second branch that failed immediately would not trigger
        // cancellation until the slow one finished.
        //
        // Do NOT hold a semaphore permit around the (possibly structural) child
        // node. The permit is acquired per *leaf* agent execution (see
        // `run_agent_step`). Holding one here across a nested `Parallel`/
        // `Pipeline` child — which then tries to acquire more permits from the
        // same semaphore — deadlocks when `max_concurrency == 1`.
        let mut join_set = tokio::task::JoinSet::new();
        for (index, step) in steps.iter().cloned().enumerate() {
            self.ensure_not_cancelled()?;
            let mut child = self.child_context();
            let child_key = format!("{key}.{index}");
            join_set.spawn(async move { (index, child.run_step(&step, child_key, None).await) });
        }

        // Collect outputs by spawn index so the joined order (which is
        // completion order) does not change the concatenation order.
        let mut outputs: Vec<Option<String>> = vec![None; steps.len()];
        let mut first_error: Option<WorkflowError> = None;
        while let Some(joined) = join_set.join_next().await {
            match joined {
                Ok((index, Ok(output))) => {
                    outputs[index] = Some(output);
                }
                Ok((_, Err(error))) => {
                    first_error = Some(error);
                    break;
                }
                Err(error) => {
                    first_error = Some(
                        CoreError::Tool(format!("workflow parallel step task failed: {error}"))
                            .into(),
                    );
                    break;
                }
            }
        }
        if let Some(error) = first_error {
            // A sibling failed (observed as soon as it completes, whatever the
            // spawn order): signal cancellation and abort the rest. `abort_all`
            // cancels every still-running task; draining ensures they are fully
            // stopped before returning so no child agent keeps running/billing.
            self.cancel_flag.store(true, Ordering::SeqCst);
            join_set.abort_all();
            while join_set.join_next().await.is_some() {}
            // The aborted siblings' futures were dropped before their explicit
            // child-session terminal transition. Sweep them here, AWAITED, so no
            // child lingers as `Running` and every `agent_started` gets a terminal
            // even if the process shuts down immediately after this returns.
            self.cancel_orphaned_run_children().await;
            return Err(error);
        }
        let output = outputs
            .into_iter()
            .flatten()
            .filter(|output| !output.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        self.complete_step(key, output.clone()).await?;
        Ok(output)
    }

    fn child_context(&self) -> Self {
        Self {
            manager: self.manager.clone(),
            home_dir: self.home_dir.clone(),
            run_id: self.run_id.clone(),
            run_dir: self.run_dir.clone(),
            session_id: self.session_id.clone(),
            arguments: self.arguments.clone(),
            cancel_flag: self.cancel_flag.clone(),
            semaphore: self.semaphore.clone(),
            journal_path: self.journal_path.clone(),
            completed_outputs: self.completed_outputs.clone(),
            progress_tx: self.progress_tx.clone(),
            progress_broadcast_tx: self.progress_broadcast_tx.clone(),
            #[cfg(test)]
            fake_agents: self.fake_agents.clone(),
        }
    }

    async fn run_agent_step(
        &mut self,
        key: &str,
        step: &AgentStep,
        pipeline_input: Option<String>,
    ) -> Result<String, WorkflowError> {
        self.ensure_not_cancelled()?;
        let description = render_arguments(&step.description, &self.arguments);
        let mut prompt = render_arguments(&step.prompt, &self.arguments);
        if let Some(input) = pipeline_input.filter(|value| !value.trim().is_empty()) {
            prompt.push_str("\n\nPrevious step output:\n");
            prompt.push_str(&input);
        }
        let invocation_ids = self.agent_invocation_ids(key);
        self.append_journal_event(
            "agent_started",
            Some(key),
            Some("agent".to_string()),
            Some(description.clone()),
            None,
            Some(invocation_ids.child_session_id.clone()),
        )
        .await?;
        self.append_log_line(&format!("agent {key}: {description}"))
            .await?;

        let agent = AgentToolInput {
            description,
            prompt,
            subagent_type: step.subagent_type.clone(),
            run_in_background: false,
        };
        // Hold a concurrency permit only around the actual agent execution (the
        // real concurrent work), not around structural Parallel/Pipeline nodes.
        // This is what bounds concurrency to `max_concurrency` without letting a
        // parent parallel node's permit deadlock a nested parallel child.
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| {
                WorkflowError::Failed(CoreError::Tool(format!(
                    "workflow semaphore closed: {error}"
                )))
            })?;
        let output = self.invoke_agent(key, &agent, invocation_ids).await;
        drop(permit);
        let output = output?;
        self.append_log_line(&output).await?;
        self.complete_step(key, output.clone()).await?;
        Ok(output)
    }

    fn agent_invocation_ids(&self, key: &str) -> WorkflowAgentInvocationIds {
        let agent_id = format!("agent-{}", Uuid::new_v4().simple());
        WorkflowAgentInvocationIds {
            child_session_id: format!("{}:{}:{agent_id}", self.session_id, self.run_id),
            tool_use_id: format!("workflow:{}:{key}", self.run_id),
            agent_id,
        }
    }

    async fn invoke_agent(
        &self,
        key: &str,
        agent: &AgentToolInput,
        invocation_ids: WorkflowAgentInvocationIds,
    ) -> Result<String, WorkflowError> {
        #[cfg(test)]
        if let Some(fake) = &self.fake_agents {
            return fake.invoke(key, agent, &self.cancel_flag).await;
        }

        #[cfg(not(test))]
        let _ = key;
        let config = self.manager.effective_config();
        let WorkflowAgentInvocationIds {
            agent_id,
            child_session_id,
            tool_use_id,
        } = invocation_ids;
        let agent_type = agent
            .subagent_type
            .as_deref()
            .unwrap_or("general-purpose")
            .to_string();
        let agent_definition = self.manager.lookup_agent_definition(&agent_type);
        let resolved_model = resolved_agent_model(&self.manager, agent_definition.as_ref());
        let permission_mode = agent_definition
            .as_ref()
            .and_then(|definition| definition.permission_mode);
        // Apply the agent's declared permissionMode to the child loop's grant,
        // like the normal Agent path — otherwise a `permissionMode: plan`
        // workflow agent would still run tools and reach the network.
        let (allow_tools, allow_network) = apply_agent_permission_mode(permission_mode, true, true);
        let _ = self
            .manager
            .child_session_store
            .start(StartChildSessionInput {
                child_session_id: child_session_id.clone(),
                parent_session_id: self.session_id.clone(),
                agent_id: agent_id.clone(),
                agent_type: agent_type.clone(),
                source_tool_use_id: tool_use_id.clone(),
                cwd: config.cwd.display().to_string(),
                model: Some(resolved_model),
                permission_mode: permission_mode.map(|mode| mode.as_str().to_string()),
                prompt: agent.prompt.clone(),
            })
            .await;

        let preloaded_skills = self
            .preload_skills(agent_definition.as_ref(), &tool_use_id)
            .await;
        let child_mcp = self
            .manager
            .maybe_create_child_mcp(agent_definition.as_ref())
            .await;
        let runner = self.manager.agent_loop_runner(child_mcp.as_ref());
        let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let result = runner
            .run_agent_session_loop(
                &self.session_id,
                &tool_use_id,
                agent,
                &agent_id,
                &agent_type,
                agent_definition.as_ref(),
                &preloaded_skills,
                &child_session_id,
                allow_tools,
                allow_network,
                true,
                &tx,
                self.cancel_flag.clone(),
            )
            .await;
        drop(tx);
        let _ = drain.await;
        if let Some(ref registry) = child_mcp {
            shutdown_child_mcp_registry(registry).await;
        }

        match result {
            Ok(AgentLoopOutcome::Completed { final_text, .. }) => {
                let _ = self
                    .manager
                    .child_session_store
                    .complete(&child_session_id)
                    .await;
                Ok(final_text)
            }
            Ok(AgentLoopOutcome::Cancelled) => {
                let _ = self
                    .manager
                    .child_session_store
                    .cancel(&child_session_id)
                    .await;
                Err(WorkflowError::Cancelled)
            }
            Err(error) => {
                let _ = self
                    .manager
                    .child_session_store
                    .fail(&child_session_id, &error.to_string())
                    .await;
                Err(WorkflowError::Failed(error))
            }
        }
    }

    /// Cancel and journal every child session belonging to THIS run that is still
    /// `Running`. Called on the parallel-abort path *after* the aborted `JoinSet`
    /// has fully drained (so no aborted sibling's agent loop is still writing).
    ///
    /// Unlike a `Drop`-based backstop, this is AWAITED by the parent, so the
    /// cleanup runs before the workflow returns — and thus before a headless
    /// process or the Tokio runtime can shut down. A child session is marked
    /// `Running` before its agent loop; an aborted sibling's future is dropped
    /// before its explicit terminal transition, so without this sweep it would
    /// linger as `Running` with a journal that has no terminal. Children are
    /// matched by the `{session_id}:{run_id}:` id prefix; since the whole run is
    /// failing, cancelling all of its still-running children is correct (this also
    /// reaches children of nested aborted parallels whose own abort path never ran).
    ///
    /// This is AWAITED BEST-EFFORT, not a hard guarantee: a store/journal I/O
    /// error on an individual child is counted and logged, not propagated (the
    /// caller is already returning the original workflow error), so an I/O failure
    /// can still leave a child `Running`.
    async fn cancel_orphaned_run_children(&self) {
        let prefix = format!("{}:{}:", self.session_id, self.run_id);
        // The child's `source_tool_use_id` is `workflow:{run_id}:{step_key}`;
        // recovering the step key lets us emit a canonical `step_cancelled` event
        // that the workflow-detail projection actually applies (it skips events
        // with no `step_key` and does not recognize `agent_cancelled`), so the
        // step transitions out of `Running` in the UI, matching the child session.
        let tool_use_prefix = format!("workflow:{}:", self.run_id);
        let children = match self
            .manager
            .child_session_store
            .list_for_parent(&self.session_id)
            .await
        {
            Ok(children) => children,
            Err(error) => {
                eprintln!(
                    "warning: workflow {} abort cleanup could not list child sessions: {error}",
                    self.run_id
                );
                return;
            }
        };
        let mut failures = 0_usize;
        for child in children {
            if !child.child_session_id.starts_with(&prefix)
                || child.status != ChildSessionStatus::Running
            {
                continue;
            }
            let step_key = child
                .source_tool_use_id
                .strip_prefix(&tool_use_prefix)
                .map(str::to_string);
            if let Err(error) = self
                .manager
                .child_session_store
                .cancel(&child.child_session_id)
                .await
            {
                failures += 1;
                eprintln!(
                    "warning: workflow {} abort cleanup failed to cancel child {}: {error}",
                    self.run_id, child.child_session_id
                );
            }
            if let Err(error) = self
                .append_journal_event(
                    "step_cancelled",
                    step_key.as_deref(),
                    Some("agent".to_string()),
                    Some("aborted by workflow cancellation".to_string()),
                    None,
                    Some(child.child_session_id.clone()),
                )
                .await
            {
                failures += 1;
                eprintln!(
                    "warning: workflow {} abort cleanup failed to journal step_cancelled for {}: {error:?}",
                    self.run_id, child.child_session_id
                );
            }
        }
        if failures > 0 {
            eprintln!(
                "warning: workflow {} abort cleanup had {failures} I/O failure(s); \
                 some child sessions may remain Running",
                self.run_id
            );
        }
    }

    async fn preload_skills(
        &self,
        agent_definition: Option<&AgentDefinition>,
        tool_use_id: &str,
    ) -> Vec<SkillDefinition> {
        let (tx, _rx) = mpsc::unbounded_channel();
        self.manager
            .preload_agent_skills(agent_definition, &self.session_id, tool_use_id, &tx)
            .await
    }

    async fn start_step(&self, key: &str, step: &WorkflowStep) -> Result<(), WorkflowError> {
        let (kind, message) = match step {
            WorkflowStep::Agent(agent) => (
                "agent",
                Some(render_arguments(&agent.description, &self.arguments)),
            ),
            WorkflowStep::Log(message) => ("log", Some(render_arguments(message, &self.arguments))),
            WorkflowStep::Phase { name, .. } => {
                let rendered = render_arguments(name, &self.arguments);
                self.append_journal_event(
                    "phase_started",
                    Some(key),
                    Some("phase".to_string()),
                    Some(rendered.clone()),
                    None,
                    None,
                )
                .await?;
                ("phase", Some(rendered))
            }
            WorkflowStep::Pipeline(_) => ("pipeline", None),
            WorkflowStep::Parallel(steps) => {
                let count = format!("{} steps", steps.len());
                self.append_journal_event(
                    "parallel_started",
                    Some(key),
                    Some("parallel".to_string()),
                    Some(count.clone()),
                    None,
                    None,
                )
                .await?;
                ("parallel", Some(count))
            }
        };
        self.append_journal_event(
            "step_started",
            Some(key),
            Some(kind.to_string()),
            message,
            None,
            None,
        )
        .await
    }

    async fn fail_step(&self, key: &str, message: String) -> Result<(), WorkflowError> {
        self.append_journal_event("step_failed", Some(key), None, Some(message), None, None)
            .await
    }

    async fn cancel_step(&self, key: &str) -> Result<(), WorkflowError> {
        self.append_journal_event("step_cancelled", Some(key), None, None, None, None)
            .await
    }

    async fn complete_step(&mut self, key: &str, output: String) -> Result<(), WorkflowError> {
        self.completed_outputs
            .insert(key.to_string(), output.clone());
        self.append_journal_event("step_completed", Some(key), None, None, Some(output), None)
            .await
    }

    async fn append_journal_event(
        &self,
        event: &str,
        step_key: Option<&str>,
        kind: Option<String>,
        message: Option<String>,
        output: Option<String>,
        child_session_id: Option<String>,
    ) -> Result<(), WorkflowError> {
        let journal_event = WorkflowJournalEvent {
            timestamp: Utc::now().to_rfc3339(),
            event: event.to_string(),
            step_key: step_key.map(str::to_string),
            kind,
            message,
            output,
            child_session_id,
        };
        append_journal(&self.journal_path, journal_event.clone()).await?;
        self.emit_background_task_snapshot(Some(&journal_event))
            .await;
        Ok(())
    }

    async fn emit_background_task_snapshot(&self, event: Option<&WorkflowJournalEvent>) {
        if self.progress_tx.is_none() && self.progress_broadcast_tx.is_none() {
            return;
        }
        let Ok(Some(record)) = read_background_task_record(&self.home_dir, &self.run_id).await
        else {
            return;
        };
        let mut task = task_record_to_view(&record);
        task.progress_events = event.map(|event| vec![workflow_progress_event(event)]);
        let stream_event = StreamEvent::BackgroundTaskUpdated {
            session_id: self.session_id.clone(),
            task,
        };
        if let Some(tx) = &self.progress_tx {
            let _ = tx.send(stream_event.clone());
        }
        if let Some(tx) = &self.progress_broadcast_tx {
            let _ = tx.send(stream_event);
        }
    }

    async fn append_log_line(&self, line: &str) -> Result<(), WorkflowError> {
        let log_path = background_log_path(&self.home_dir, &self.run_id);
        if let Some(parent) = log_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut value = line.to_string();
        if !value.ends_with('\n') {
            value.push('\n');
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .await?;
        file.write_all(value.as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }

    fn ensure_not_cancelled(&self) -> Result<(), WorkflowError> {
        if self.cancel_flag.load(Ordering::SeqCst) {
            Err(WorkflowError::Cancelled)
        } else {
            Ok(())
        }
    }

    async fn finalize(
        &self,
        status: WorkflowRunStatus,
        result: Option<String>,
        error: Option<String>,
    ) -> Result<(), CoreError> {
        let now = Utc::now().to_rfc3339();
        let run_path = self.run_dir.join("run.json");
        let mut run: WorkflowRunRecord = read_json(&run_path).await?;
        run.status = status;
        run.updated_at = now.clone();
        run.finished_at = Some(now.clone());
        run.result = result.clone();
        run.error = error.clone();
        write_json(&run_path, &run).await?;
        let finished_event = WorkflowJournalEvent {
            timestamp: now.clone(),
            event: "run_finished".to_string(),
            step_key: None,
            kind: None,
            message: error.clone(),
            output: result.clone(),
            child_session_id: None,
        };
        append_journal(&self.journal_path, finished_event.clone()).await?;

        if let Some(mut record) = read_background_task_record(&self.home_dir, &self.run_id).await? {
            record.status = status.as_background_status();
            record.updated_at = now.clone();
            record.finished_at = Some(now);
            record.error = error;
            record.result = result;
            write_background_task_record(&self.home_dir, &record).await?;
        }
        self.emit_background_task_snapshot(Some(&finished_event))
            .await;
        Ok(())
    }
}

fn workflow_progress_event(event: &WorkflowJournalEvent) -> BackgroundTaskProgressEvent {
    let timestamp = DateTime::parse_from_rfc3339(&event.timestamp)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    BackgroundTaskProgressEvent {
        timestamp,
        event: event.event.clone(),
        step_key: event.step_key.clone(),
        kind: event.kind.clone(),
        message: event.message.clone(),
        output: event.output.clone(),
        child_session_id: event.child_session_id.clone(),
    }
}

fn resolved_agent_model(manager: &SessionManager, definition: Option<&AgentDefinition>) -> String {
    definition
        .and_then(|definition| definition.model.clone())
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("inherit"))
        .unwrap_or_else(|| {
            let config = manager.effective_config();
            config
                .provider_model_resolution(config.default_provider)
                .request_model
        })
}

async fn discover_workflows(
    home_dir: &Path,
    cwd: &Path,
) -> Result<Vec<DiscoveredWorkflow>, CoreError> {
    let mut by_name: BTreeMap<String, DiscoveredWorkflow> = BTreeMap::new();
    collect_workflows_from_root(
        &home_dir.join("workflows"),
        WorkflowSource::User,
        &mut by_name,
        false,
    )
    .await?;
    collect_workflows_from_root(
        &cwd.join(".claude").join("workflows"),
        WorkflowSource::Project,
        &mut by_name,
        true,
    )
    .await?;
    Ok(by_name.into_values().collect())
}

async fn collect_workflows_from_root(
    root: &Path,
    source: WorkflowSource,
    by_name: &mut BTreeMap<String, DiscoveredWorkflow>,
    override_existing: bool,
) -> Result<(), CoreError> {
    if !tokio::fs::try_exists(root).await? {
        return Ok(());
    }
    let paths = workflow_json_paths(root).await?;
    let mut seen_this_scope = HashSet::new();
    for path in paths {
        let Some(name) = workflow_name(root, &path) else {
            continue;
        };
        if !seen_this_scope.insert(name.clone()) {
            continue;
        }
        let Ok(spec) = read_json::<WorkflowSpec>(&path).await else {
            continue;
        };
        let Ok(plan) = validate_workflow_spec(
            name.clone(),
            fallback_description(&name, &spec),
            spec.clone(),
        ) else {
            continue;
        };
        if override_existing || !by_name.contains_key(&name) {
            by_name.insert(
                name.clone(),
                DiscoveredWorkflow {
                    command: WorkflowCommand {
                        name,
                        description: plan.description,
                        source,
                    },
                    spec,
                },
            );
        }
    }
    Ok(())
}

async fn workflow_json_paths(root: &Path) -> Result<Vec<PathBuf>, CoreError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && path.extension().and_then(|ext| ext.to_str()) == Some("json")
            {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn workflow_name(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let segments: Vec<String> = relative
        .with_extension("")
        .components()
        .filter_map(|component| component.as_os_str().to_str().map(str::to_string))
        .collect();
    if segments.is_empty() {
        None
    } else {
        Some(segments.join(":"))
    }
}

fn fallback_description(name: &str, spec: &WorkflowSpec) -> String {
    spec.description
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("Run workflow {name}"))
}

fn validate_workflow_spec(
    name: String,
    description: String,
    spec: WorkflowSpec,
) -> Result<WorkflowPlan, CoreError> {
    validate_workflow_spec_with_mode(name, description, spec, WorkflowValidationMode::Saved)
}

fn validate_workflow_spec_with_mode(
    name: String,
    description: String,
    spec: WorkflowSpec,
    mode: WorkflowValidationMode,
) -> Result<WorkflowPlan, CoreError> {
    if spec.schema_version != 1 {
        return Err(CoreError::Config(
            "workflow schema_version must be 1".to_string(),
        ));
    }
    if spec.steps.is_empty() {
        return Err(CoreError::Config(
            "workflow steps must be non-empty".to_string(),
        ));
    }
    let max_concurrency = spec.max_concurrency.unwrap_or(DEFAULT_MAX_CONCURRENCY);
    if !(1..=MAX_CONCURRENCY_LIMIT).contains(&max_concurrency) {
        return Err(CoreError::Config(
            "workflow max_concurrency must be within 1..=16".to_string(),
        ));
    }
    // Validate per-step shape first so malformed steps (e.g. kind/name-style
    // objects) surface their precise shape guidance instead of being masked by
    // the dynamic "at least one agent work step" aggregate check.
    let steps = validate_steps(&spec.steps)?;
    if mode == WorkflowValidationMode::DynamicGenerated {
        validate_dynamic_generated_steps(&spec.steps)?;
    }
    Ok(WorkflowPlan {
        name,
        description,
        max_concurrency,
        steps,
        spec,
    })
}

fn validate_dynamic_generated_steps(raw_steps: &[WorkflowStepRaw]) -> Result<(), CoreError> {
    let mut has_agent_work = false;
    for raw in raw_steps {
        has_agent_work |= validate_dynamic_generated_step(raw)?;
    }
    if !has_agent_work {
        return Err(CoreError::Config(
            "dynamic workflow steps must include at least one agent work step".to_string(),
        ));
    }
    Ok(())
}

fn validate_dynamic_generated_step(raw: &WorkflowStepRaw) -> Result<bool, CoreError> {
    if raw.agent.is_some() {
        return Ok(true);
    }
    if raw.log.is_some() {
        return Ok(false);
    }
    if let Some(parallel) = &raw.parallel {
        return validate_dynamic_generated_container("parallel", &parallel.steps);
    }
    if let Some(pipeline) = &raw.pipeline {
        return validate_dynamic_generated_container("pipeline", &pipeline.steps);
    }
    if let Some(phase) = &raw.phase {
        return validate_dynamic_generated_container("phase", &phase.steps);
    }
    Ok(false)
}

fn validate_dynamic_generated_container(
    kind: &str,
    steps: &[WorkflowStepRaw],
) -> Result<bool, CoreError> {
    let mut has_agent_work = false;
    for step in steps {
        has_agent_work |= validate_dynamic_generated_step(step)?;
    }
    if !has_agent_work {
        return Err(CoreError::Config(format!(
            "dynamic workflow {kind}.steps must include at least one agent work step"
        )));
    }
    Ok(true)
}

fn validate_steps(raw_steps: &[WorkflowStepRaw]) -> Result<Vec<WorkflowStep>, CoreError> {
    raw_steps.iter().map(validate_step).collect()
}

fn unsupported_step_fields(raw: &WorkflowStepRaw) -> Vec<String> {
    raw.extra
        .keys()
        .filter(|key| !(raw.agent.is_some() && (*key == "subagent_type" || *key == "subagentType")))
        .cloned()
        .collect()
}

fn take_agent_with_misplaced_fields(raw: &WorkflowStepRaw) -> Result<Option<AgentStep>, CoreError> {
    let Some(mut agent) = raw.agent.clone() else {
        return Ok(None);
    };
    for key in ["subagent_type", "subagentType"] {
        let Some(value) = raw.extra.get(key) else {
            continue;
        };
        match value {
            serde_json::Value::Null => {}
            serde_json::Value::String(value) => {
                if agent.subagent_type.is_none() {
                    agent.subagent_type = Some(value.clone());
                }
            }
            _ => {
                return Err(CoreError::Config(format!(
                    "workflow agent step field `{key}` must be a string when present"
                )));
            }
        }
    }
    Ok(Some(agent))
}

fn validate_step(raw: &WorkflowStepRaw) -> Result<WorkflowStep, CoreError> {
    let unexpected_fields = unsupported_step_fields(raw);
    if !unexpected_fields.is_empty() {
        let unexpected = unexpected_fields.join(", ");
        return Err(CoreError::Config(format!(
            "workflow step uses unsupported field(s): {unexpected}. Steps must be single-key objects like {{\"agent\":{{\"description\":\"...\",\"prompt\":\"...\"}}}}, {{\"parallel\":{{\"steps\":[...]}}}}, {{\"pipeline\":{{\"steps\":[...]}}}}, {{\"phase\":{{\"name\":\"...\",\"steps\":[...]}}}}, or {{\"log\":{{\"message\":\"...\"}}}}; do not use kind/name/run_in_background fields"
        )));
    }
    let kind_count = [
        raw.agent.is_some(),
        raw.parallel.is_some(),
        raw.pipeline.is_some(),
        raw.phase.is_some(),
        raw.log.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if kind_count != 1 {
        return Err(CoreError::Config(
            "workflow step must declare exactly one kind".to_string(),
        ));
    }
    if let Some(agent) = take_agent_with_misplaced_fields(raw)? {
        if agent.description.trim().is_empty() || agent.prompt.trim().is_empty() {
            return Err(CoreError::Config(
                "workflow agent.description and agent.prompt must be non-empty".to_string(),
            ));
        }
        return Ok(WorkflowStep::Agent(agent));
    }
    if let Some(parallel) = &raw.parallel {
        if parallel.steps.is_empty() {
            return Err(CoreError::Config(
                "workflow parallel.steps must be non-empty".to_string(),
            ));
        }
        return Ok(WorkflowStep::Parallel(validate_steps(&parallel.steps)?));
    }
    if let Some(pipeline) = &raw.pipeline {
        if pipeline.steps.is_empty() {
            return Err(CoreError::Config(
                "workflow pipeline.steps must be non-empty".to_string(),
            ));
        }
        return Ok(WorkflowStep::Pipeline(validate_steps(&pipeline.steps)?));
    }
    if let Some(phase) = &raw.phase {
        if phase.steps.is_empty() {
            return Err(CoreError::Config(
                "workflow phase.steps must be non-empty".to_string(),
            ));
        }
        return Ok(WorkflowStep::Phase {
            name: phase.name.clone(),
            steps: validate_steps(&phase.steps)?,
        });
    }
    let log = raw.log.as_ref().expect("kind count already checked");
    Ok(WorkflowStep::Log(log.message.clone()))
}

fn render_arguments(body: &str, arguments: &str) -> String {
    let positional: Vec<&str> = arguments.split_whitespace().collect();
    let mut out = String::with_capacity(body.len() + arguments.len());
    let mut chars = body.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        if character != '$' {
            out.push(character);
            continue;
        }
        let Some(&(_, next_char)) = chars.peek() else {
            out.push(character);
            continue;
        };
        if next_char.is_ascii_digit() {
            chars.next();
            let digit = next_char.to_digit(10).unwrap_or(0) as usize;
            if digit >= 1
                && let Some(arg) = positional.get(digit - 1)
            {
                out.push_str(arg);
            }
            continue;
        }
        if body[index + 1..].starts_with("ARGUMENTS") {
            for _ in 0.."ARGUMENTS".len() {
                chars.next();
            }
            out.push_str(arguments);
            continue;
        }
        out.push(character);
    }
    out
}

async fn load_journal(path: &Path) -> Result<WorkflowJournal, CoreError> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(WorkflowJournal::default());
    }
    let contents = tokio::fs::read_to_string(path).await?;
    let mut journal = WorkflowJournal::default();
    for line in contents.lines() {
        let Ok(event) = serde_json::from_str::<WorkflowJournalEvent>(line) else {
            continue;
        };
        if event.event == "step_completed"
            && let (Some(key), Some(output)) = (event.step_key, event.output)
        {
            journal.completed_outputs.insert(key, output);
        }
    }
    Ok(journal)
}

async fn append_journal(path: &Path, event: WorkflowJournalEvent) -> Result<(), CoreError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut line = serde_json::to_string(&event)?;
    line.push('\n');
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(line.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

async fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, CoreError> {
    Ok(serde_json::from_slice(&tokio::fs::read(path).await?)?)
}

async fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), CoreError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, serde_json::to_vec_pretty(value)?).await?;
    Ok(())
}

fn workflow_run_dir(home_dir: &Path, run_id: &str) -> PathBuf {
    home_dir.join("workflow-runs").join(run_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_config::{AppConfig, AppConfigOverrides};
    use orbcode_mcp::McpRegistry;
    use orbcode_protocol::BackgroundTaskViewKind;
    use orbcode_tools::ToolRegistry;
    use tempfile::tempdir;

    async fn test_manager(home: &Path, cwd: &Path) -> SessionManager {
        let config = AppConfig::load(
            cwd.to_path_buf(),
            AppConfigOverrides {
                home_dir: Some(home.to_path_buf()),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("load config");
        let mcp = McpRegistry::load(home.to_path_buf(), cwd.to_path_buf())
            .await
            .expect("load mcp");
        SessionManager::new(config, ToolRegistry::foundation(), mcp)
            .await
            .expect("create session manager")
    }

    fn parse_plan(name: &str, json: &str) -> WorkflowPlan {
        let spec: WorkflowSpec = serde_json::from_str(json).expect("workflow json");
        validate_workflow_spec(name.to_string(), format!("Run {name}"), spec).expect("valid plan")
    }

    async fn test_context(
        plan: &WorkflowPlan,
        completed_outputs: HashMap<String, String>,
    ) -> (
        tempfile::TempDir,
        WorkflowExecutionContext,
        FakeWorkflowAgentExecutor,
    ) {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        let manager = test_manager(&home, &cwd).await;
        let run_id = format!("workflow-{}", Uuid::new_v4().simple());
        let run_dir = workflow_run_dir(&home, &run_id);
        tokio::fs::create_dir_all(&run_dir).await.expect("run dir");
        let fake = FakeWorkflowAgentExecutor::default();
        let ctx = WorkflowExecutionContext {
            manager,
            home_dir: home,
            run_id,
            run_dir: run_dir.clone(),
            session_id: "session-1".to_string(),
            arguments: "arg-one arg-two".to_string(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            semaphore: Arc::new(Semaphore::new(plan.max_concurrency)),
            journal_path: run_dir.join("journal.jsonl"),
            completed_outputs,
            progress_tx: None,
            progress_broadcast_tx: None,
            fake_agents: Some(fake.clone()),
        };
        (temp, ctx, fake)
    }

    #[tokio::test]
    async fn workflow_snapshot_broadcast_does_not_require_originating_turn_stream() {
        let plan = parse_plan(
            "broadcast",
            r#"{
                "schema_version": 1,
                "description": "broadcast workflow",
                "steps": [{ "log": { "message": "hello" } }]
            }"#,
        );
        let (_temp, mut ctx, _fake) = test_context(&plan, HashMap::new()).await;
        let log_path = background_log_path(&ctx.home_dir, &ctx.run_id);
        let record = BackgroundTaskRecord::new_workflow(
            ctx.run_id.clone(),
            ctx.session_id.clone(),
            "broadcast workflow".to_string(),
            "/tmp".to_string(),
            log_path.display().to_string(),
        );
        write_background_task_record(&ctx.home_dir, &record)
            .await
            .expect("write record");
        let broadcast_tx = register_progress_stream(&ctx.run_id, 16);
        let mut rx = orbcode_tools::subscribe_progress_stream(&ctx.run_id).expect("subscribe");
        ctx.progress_tx = None;
        ctx.progress_broadcast_tx = Some(broadcast_tx);

        ctx.emit_background_task_snapshot(None).await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("broadcast event")
            .expect("receive event");
        match event {
            StreamEvent::BackgroundTaskUpdated { session_id, task } => {
                assert_eq!(session_id, ctx.session_id);
                assert_eq!(task.task_id, ctx.run_id);
                assert_eq!(task.kind, BackgroundTaskViewKind::Workflow);
            }
            other => panic!("unexpected event: {other:?}"),
        }
        unregister_progress_stream(&ctx.run_id);
    }

    async fn read_journal_events(path: &Path) -> Vec<WorkflowJournalEvent> {
        let contents = tokio::fs::read_to_string(path).await.expect("journal");
        contents
            .lines()
            .map(|line| serde_json::from_str(line).expect("journal event"))
            .collect()
    }

    #[test]
    fn validates_schema_and_required_agent_fields() {
        let spec = WorkflowSpec {
            schema_version: 1,
            description: None,
            max_concurrency: Some(4),
            steps: vec![WorkflowStepRaw {
                agent: Some(AgentStep {
                    description: "d".to_string(),
                    prompt: "p".to_string(),
                    subagent_type: None,
                }),
                ..WorkflowStepRaw::default()
            }],
        };
        let plan = validate_workflow_spec("check".to_string(), "Check".to_string(), spec)
            .expect("valid workflow");
        assert_eq!(plan.max_concurrency, 4);
    }

    #[test]
    fn rejects_multi_kind_steps() {
        let spec = WorkflowSpec {
            schema_version: 1,
            description: None,
            max_concurrency: None,
            steps: vec![WorkflowStepRaw {
                agent: Some(AgentStep {
                    description: "d".to_string(),
                    prompt: "p".to_string(),
                    subagent_type: None,
                }),
                log: Some(LogStep {
                    message: "also".to_string(),
                }),
                ..WorkflowStepRaw::default()
            }],
        };
        assert!(validate_workflow_spec("x".to_string(), "x".to_string(), spec).is_err());
    }

    #[test]
    fn dynamic_generated_validation_requires_agent_work_in_organizers() {
        let spec: WorkflowSpec = serde_json::from_str(
            r#"{"schema_version":1,"steps":[{"phase":{"name":"only logs","steps":[
                {"parallel":{"steps":[
                    {"log":{"message":"inspect manually"}}
                ]}}
            ]}}]}"#,
        )
        .expect("workflow json");

        let error = validate_workflow_spec_with_mode(
            "dynamic:logs".to_string(),
            "logs".to_string(),
            spec,
            WorkflowValidationMode::DynamicGenerated,
        )
        .expect_err("dynamic generated organizers must include agent work");

        assert!(
            error
                .to_string()
                .contains("must include at least one agent work step"),
            "{error}"
        );
    }

    #[test]
    fn dynamic_generated_validation_requires_agent_work_at_top_level() {
        let spec: WorkflowSpec = serde_json::from_str(
            r#"{"schema_version":1,"steps":[
                {"log":{"message":"nothing to do"}},
                {"log":{"message":"still nothing"}}
            ]}"#,
        )
        .expect("workflow json");

        let error = validate_workflow_spec_with_mode(
            "dynamic:top-level-logs".to_string(),
            "logs".to_string(),
            spec,
            WorkflowValidationMode::DynamicGenerated,
        )
        .expect_err("all-log top-level dynamic workflow must be rejected");

        assert!(
            error
                .to_string()
                .contains("must include at least one agent work step"),
            "{error}"
        );
    }

    #[test]
    fn validation_rejects_kind_style_steps_with_shape_guidance() {
        let spec: WorkflowSpec = serde_json::from_str(
            r#"{"schema_version":1,"steps":[
                {"kind":"agent","name":"audit-core","prompt":"inspect","run_in_background":true}
            ]}"#,
        )
        .expect("workflow json");

        let error = validate_workflow_spec_with_mode(
            "dynamic:bad-shape".to_string(),
            "bad shape".to_string(),
            spec,
            WorkflowValidationMode::DynamicGenerated,
        )
        .expect_err("kind-style steps must be rejected");

        assert!(error.to_string().contains("single-key objects"), "{error}");
        assert!(
            error
                .to_string()
                .contains("do not use kind/name/run_in_background fields"),
            "{error}"
        );
    }

    #[test]
    fn validation_tolerates_misplaced_subagent_type_on_agent_steps() {
        let spec: WorkflowSpec = serde_json::from_str(
            r#"{"schema_version":1,"steps":[
                {
                    "parallel": {
                        "steps": [
                            {
                                "agent": {
                                    "description": "audit",
                                    "prompt": "inspect"
                                },
                                "subagent_type": null
                            }
                        ]
                    }
                }
            ]}"#,
        )
        .expect("workflow json");

        let plan = validate_workflow_spec_with_mode(
            "dynamic:bad-subagent-type".to_string(),
            "bad subagent type".to_string(),
            spec,
            WorkflowValidationMode::DynamicGenerated,
        )
        .expect("misplaced null subagent_type should be ignored for model compatibility");

        assert!(
            matches!(&plan.steps[0], WorkflowStep::Parallel(steps) if matches!(&steps[0], WorkflowStep::Agent(agent) if agent.subagent_type.is_none()))
        );
    }

    #[test]
    fn validation_migrates_misplaced_string_subagent_type_on_agent_steps() {
        let spec: WorkflowSpec = serde_json::from_str(
            r#"{"schema_version":1,"steps":[
                {
                    "agent": {
                        "description": "audit",
                        "prompt": "inspect"
                    },
                    "subagent_type": "general-purpose"
                }
            ]}"#,
        )
        .expect("workflow json");

        let plan = validate_workflow_spec_with_mode(
            "dynamic:misplaced-subagent-type".to_string(),
            "misplaced subagent type".to_string(),
            spec,
            WorkflowValidationMode::DynamicGenerated,
        )
        .expect("misplaced string subagent_type should be migrated for model compatibility");

        assert!(
            matches!(&plan.steps[0], WorkflowStep::Agent(agent) if agent.subagent_type.as_deref() == Some("general-purpose"))
        );
    }

    #[test]
    fn saved_workflow_validation_keeps_broader_v1_log_behavior() {
        let spec: WorkflowSpec = serde_json::from_str(
            r#"{"schema_version":1,"steps":[{"phase":{"name":"status","steps":[
                {"parallel":{"steps":[
                    {"log":{"message":"checkpoint"}}
                ]}}
            ]}}]}"#,
        )
        .expect("workflow json");

        validate_workflow_spec("saved:logs".to_string(), "logs".to_string(), spec)
            .expect("saved workflow files keep v1 log behavior");
    }

    #[test]
    fn renders_arguments_like_slash_commands() {
        assert_eq!(
            render_arguments("$1/$2/$9/$ARGUMENTS", "alpha beta"),
            "alpha/beta//alpha beta"
        );
    }

    #[tokio::test]
    async fn discovery_uses_project_over_user_and_nested_names() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("cwd");
        tokio::fs::create_dir_all(home.join("workflows"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(cwd.join(".claude/workflows/acp"))
            .await
            .unwrap();
        let json = r#"{"schema_version":1,"description":"user","steps":[{"log":{"message":"u"}}]}"#;
        tokio::fs::write(home.join("workflows/check.json"), json)
            .await
            .unwrap();
        let project =
            r#"{"schema_version":1,"description":"project","steps":[{"log":{"message":"p"}}]}"#;
        tokio::fs::write(cwd.join(".claude/workflows/check.json"), project)
            .await
            .unwrap();
        tokio::fs::write(cwd.join(".claude/workflows/acp/review.json"), project)
            .await
            .unwrap();

        let workflows = discover_workflows(&home, &cwd).await.unwrap();
        let check = workflows
            .iter()
            .find(|workflow| workflow.command.name == "check")
            .unwrap();
        assert_eq!(check.command.description, "project");
        assert_eq!(check.command.source, WorkflowSource::Project);
        assert!(
            workflows
                .iter()
                .any(|workflow| workflow.command.name == "acp:review")
        );
    }

    #[tokio::test]
    async fn runtime_fake_agents_run_serial_steps_in_order() {
        let plan = parse_plan(
            "serial",
            r#"{"schema_version":1,"steps":[
                {"agent":{"description":"first","prompt":"first $1"}},
                {"agent":{"description":"second","prompt":"second $2"}}
            ]}"#,
        );
        let (_temp, mut ctx, fake) = test_context(&plan, HashMap::new()).await;

        let output = ctx
            .run_steps(&plan.steps, "step".to_string(), None, false)
            .await
            .expect("run workflow");

        assert_eq!(output, "first arg-one\nsecond arg-two");
        assert_eq!(
            fake.call_prompts(),
            vec!["first arg-one".to_string(), "second arg-two".to_string()]
        );
    }

    #[tokio::test]
    async fn runtime_persists_progress_events_to_journal() {
        let plan = parse_plan(
            "progress",
            r#"{"schema_version":1,"steps":[{"phase":{"name":"phase $1","steps":[
                {"parallel":{"steps":[
                    {"agent":{"description":"agent $2","prompt":"done"}}
                ]}}
            ]}}]}"#,
        );
        let (_temp, mut ctx, _fake) = test_context(&plan, HashMap::new()).await;
        let journal_path = ctx.journal_path.clone();

        let output = ctx
            .run_steps(&plan.steps, "step".to_string(), None, false)
            .await
            .expect("run workflow");

        assert_eq!(output, "done");
        let events = read_journal_events(&journal_path).await;
        assert!(events.iter().any(|event| {
            event.event == "step_started"
                && event.step_key.as_deref() == Some("step.0")
                && event.kind.as_deref() == Some("phase")
                && event.message.as_deref() == Some("phase arg-one")
        }));
        assert!(events.iter().any(|event| {
            event.event == "phase_started"
                && event.step_key.as_deref() == Some("step.0")
                && event.message.as_deref() == Some("phase arg-one")
        }));
        assert!(events.iter().any(|event| {
            event.event == "parallel_started"
                && event.step_key.as_deref() == Some("step.0.0")
                && event.message.as_deref() == Some("1 steps")
        }));
        assert!(events.iter().any(|event| {
            event.event == "agent_started"
                && event.step_key.as_deref() == Some("step.0.0.0")
                && event.message.as_deref() == Some("agent arg-two")
                && event
                    .child_session_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with("session-1:workflow-"))
        }));
        assert!(events.iter().any(|event| {
            event.event == "step_completed"
                && event.step_key.as_deref() == Some("step.0.0.0")
                && event.output.as_deref() == Some("done")
        }));
    }

    #[tokio::test]
    async fn runtime_pipeline_passes_previous_output() {
        let plan = parse_plan(
            "pipeline",
            r#"{"schema_version":1,"steps":[{"pipeline":{"steps":[
                {"agent":{"description":"first","prompt":"alpha"}},
                {"agent":{"description":"second","prompt":"beta"}}
            ]}}]}"#,
        );
        let (_temp, mut ctx, fake) = test_context(&plan, HashMap::new()).await;

        let output = ctx
            .run_steps(&plan.steps, "step".to_string(), None, false)
            .await
            .expect("run workflow");

        assert!(output.contains("alpha"));
        let prompts = fake.call_prompts();
        assert_eq!(prompts[0], "alpha");
        assert!(
            prompts[1].contains("Previous step output:\nalpha"),
            "pipeline input missing from second prompt: {prompts:?}"
        );
    }

    #[tokio::test]
    async fn runtime_parallel_obeys_max_concurrency() {
        let plan = parse_plan(
            "parallel",
            r#"{"schema_version":1,"max_concurrency":1,"steps":[{"parallel":{"steps":[
                {"agent":{"description":"a","prompt":"delay:a"}},
                {"agent":{"description":"b","prompt":"delay:b"}},
                {"agent":{"description":"c","prompt":"delay:c"}}
            ]}}]}"#,
        );
        let (_temp, mut ctx, fake) = test_context(&plan, HashMap::new()).await;

        let output = ctx
            .run_steps(&plan.steps, "step".to_string(), None, false)
            .await
            .expect("run workflow");

        assert_eq!(output, "a\nb\nc");
        assert_eq!(fake.max_active(), 1);
    }

    #[tokio::test]
    async fn runtime_nested_parallel_does_not_deadlock_at_max_concurrency_one() {
        // A parallel branch that is itself a `Parallel` must not deadlock: the
        // permit is held per leaf agent, not around the structural node, so the
        // nested parallel's leaves can still acquire permits.
        let plan = parse_plan(
            "nested",
            r#"{"schema_version":1,"max_concurrency":1,"steps":[{"parallel":{"steps":[
                {"parallel":{"steps":[
                    {"agent":{"description":"a","prompt":"a"}},
                    {"agent":{"description":"b","prompt":"b"}}
                ]}},
                {"agent":{"description":"c","prompt":"c"}}
            ]}}]}"#,
        );
        let (_temp, mut ctx, fake) = test_context(&plan, HashMap::new()).await;

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            ctx.run_steps(&plan.steps, "step".to_string(), None, false),
        )
        .await
        .expect("nested parallel workflow must not deadlock")
        .expect("run workflow");

        // All three leaf agents ran; concurrency stayed capped at 1.
        assert!(output.contains('a') && output.contains('b') && output.contains('c'));
        assert_eq!(fake.max_active(), 1);
    }

    #[tokio::test]
    async fn runtime_fake_agent_failure_fails_workflow() {
        let plan = parse_plan(
            "failure",
            r#"{"schema_version":1,"steps":[
                {"agent":{"description":"fail","prompt":"fail:boom"}}
            ]}"#,
        );
        let (_temp, mut ctx, _fake) = test_context(&plan, HashMap::new()).await;

        let result = ctx
            .run_steps(&plan.steps, "step".to_string(), None, false)
            .await;

        assert!(
            matches!(result, Err(WorkflowError::Failed(CoreError::Tool(message))) if message == "boom")
        );
    }

    #[tokio::test]
    async fn runtime_parallel_fast_failure_cancels_slow_sibling_regardless_of_order() {
        // The slow-forever `wait` branch is spawned FIRST; the fast `fail:boom`
        // branch is spawned second. `wait` only returns once `cancel_flag` is
        // set, and the flag is set only after a sibling failure is observed.
        // Awaiting handles in spawn order (the old bug) would block on `wait`
        // forever, never observing the second branch's failure → deadlock. With
        // JoinSet the failure is observed as soon as it happens, cancelling the
        // slow sibling. The timeout turns a regression into a fast failure
        // instead of a hung suite.
        let plan = parse_plan(
            "parallel-fast-fail",
            r#"{"schema_version":1,"steps":[{"parallel":{"steps":[
                {"agent":{"description":"slow","prompt":"wait"}},
                {"agent":{"description":"boom","prompt":"fail:boom"}}
            ]}}]}"#,
        );
        let (_temp, mut ctx, _fake) = test_context(&plan, HashMap::new()).await;
        let cancel_flag = ctx.cancel_flag.clone();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            ctx.run_steps(&plan.steps, "step".to_string(), None, false),
        )
        .await
        .expect("a fast-failing sibling must cancel the slow one, not deadlock");

        assert!(
            matches!(&result, Err(WorkflowError::Failed(CoreError::Tool(message))) if message == "boom"),
            "the parallel step must fail with the sibling error: {result:?}"
        );
        assert!(
            cancel_flag.load(Ordering::SeqCst),
            "a sibling failure must set the cancel flag to abort the rest"
        );
    }

    #[tokio::test]
    async fn runtime_cancellation_reaches_active_fake_agent() {
        let plan = parse_plan(
            "cancel",
            r#"{"schema_version":1,"steps":[
                {"agent":{"description":"wait","prompt":"wait"}}
            ]}"#,
        );
        let (_temp, mut ctx, _fake) = test_context(&plan, HashMap::new()).await;
        let cancel_flag = ctx.cancel_flag.clone();

        let handle = tokio::spawn(async move {
            ctx.run_steps(&plan.steps, "step".to_string(), None, false)
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        cancel_flag.store(true, Ordering::SeqCst);
        let result = handle.await.expect("join workflow");

        assert!(matches!(result, Err(WorkflowError::Cancelled)));
    }

    #[tokio::test]
    async fn runtime_resume_skips_completed_agent_steps() {
        let plan = parse_plan(
            "resume",
            r#"{"schema_version":1,"steps":[
                {"agent":{"description":"already","prompt":"already"}},
                {"agent":{"description":"next","prompt":"next"}}
            ]}"#,
        );
        let mut completed = HashMap::new();
        completed.insert("step.0".to_string(), "cached".to_string());
        let (_temp, mut ctx, fake) = test_context(&plan, completed).await;

        let output = ctx
            .run_steps(&plan.steps, "step".to_string(), None, false)
            .await
            .expect("resume workflow");

        assert_eq!(output, "cached\nnext");
        assert_eq!(fake.call_prompts(), vec!["next".to_string()]);
    }

    #[tokio::test]
    async fn cancel_orphaned_run_children_cancels_this_runs_running_children() {
        let plan = parse_plan(
            "sweep",
            r#"{"schema_version":1,"steps":[{"log":{"message":"x"}}]}"#,
        );
        let (_temp, ctx, _fake) = test_context(&plan, HashMap::new()).await;
        let store = ctx.manager.child_session_store.clone();

        let child_input =
            |child_session_id: &str, source_tool_use_id: &str| StartChildSessionInput {
                child_session_id: child_session_id.to_string(),
                parent_session_id: ctx.session_id.clone(),
                agent_id: "agent".to_string(),
                agent_type: "general-purpose".to_string(),
                source_tool_use_id: source_tool_use_id.to_string(),
                cwd: "/tmp".to_string(),
                model: None,
                permission_mode: None,
                prompt: "prompt".to_string(),
            };
        // A Running child of THIS run (aborted sibling) and one of a DIFFERENT run.
        // The `source_tool_use_id` matches the real `workflow:{run_id}:{step_key}`
        // shape so the sweep can recover the step key.
        let mine = format!("{}:{}:agent-mine", ctx.session_id, ctx.run_id);
        let other = format!("{}:other-run:agent-other", ctx.session_id);
        let step_key = "step.0.1";
        store
            .start(child_input(
                &mine,
                &format!("workflow:{}:{step_key}", ctx.run_id),
            ))
            .await
            .expect("start mine");
        store
            .start(child_input(&other, "workflow:other-run:step.0"))
            .await
            .expect("start other");

        // Awaited sweep (the parallel-abort cleanup path).
        ctx.cancel_orphaned_run_children().await;

        assert_eq!(
            store.load(&mine).await.unwrap().unwrap().status,
            ChildSessionStatus::Cancelled,
            "an orphaned Running child of this run must be cancelled (not left Running)"
        );
        assert_eq!(
            store.load(&other).await.unwrap().unwrap().status,
            ChildSessionStatus::Running,
            "a child of a different run must be left untouched"
        );
        // The journal event must be the canonical `step_cancelled` carrying the
        // recovered step key, so the workflow-detail projection transitions the
        // step out of Running (an `agent_cancelled` with no step_key is skipped).
        let journal = tokio::fs::read_to_string(&ctx.journal_path)
            .await
            .unwrap_or_default();
        let cancelled_event = journal
            .lines()
            .filter_map(|line| serde_json::from_str::<WorkflowJournalEvent>(line).ok())
            .find(|event| event.event == "step_cancelled")
            .expect("sweep must write a step_cancelled journal event");
        assert_eq!(
            cancelled_event.step_key.as_deref(),
            Some(step_key),
            "step_cancelled must carry the recovered step key for the projection"
        );
    }
}
