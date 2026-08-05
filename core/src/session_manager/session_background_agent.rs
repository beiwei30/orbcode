use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use tokio::sync::Semaphore;

/// Cap on concurrently-running detached background agents. Without it, a model
/// can spawn unbounded background agents that all run — and bill — in parallel.
/// A spawn requested while every slot is taken is refused with an error result
/// rather than silently piling on more concurrent work.
const MAX_CONCURRENT_BACKGROUND_AGENTS: usize = 16;
static BACKGROUND_AGENT_SLOTS: LazyLock<Semaphore> =
    LazyLock::new(|| Semaphore::new(MAX_CONCURRENT_BACKGROUND_AGENTS));

use chrono::Utc;
use orbcode_config::{AgentDefinition, PermissionMode};
use orbcode_mcp::McpRegistry;
use orbcode_model_provider::ProviderCancellationToken;
use orbcode_protocol::MessageRole;
use orbcode_protocol::{
    SessionRecord, StreamEvent, TokenUsage, ToolUseCompletionKind, TranscriptMessage,
};
use orbcode_session_store::{
    SessionWriteHints, agent_tool_result_progress_record, initial_agent_progress_record,
    tool_result_message,
};
use orbcode_tools::read_background_task_record;
use orbcode_tools::{
    AgentToolInput, BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus,
    SkillDefinition, ToolError, ToolOutcome, background_log_path,
    register_background_task_cancel_flag, register_progress_stream, resolve_requested_skills,
    unregister_background_task_cancel_flag, unregister_progress_stream,
    write_background_task_record,
};
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{
    SessionManager,
    session_agent_tool::{
        AgentLoopOutcome, agent_definition_permits_tool, apply_agent_definition_to_request,
        apply_agent_permission_mode, apply_preloaded_skills_to_request,
    },
    session_stream::{AgentProviderStreamSink, NestedAgentToolProgressReporter},
};
use crate::{
    CoreError,
    agent_tool::{
        agent_final_text, agent_nested_tool_error_result, agent_nested_tool_success_result,
        agent_nested_tool_uses,
    },
    hooks::{subagent_start_hook_context, subagent_stop_hook_feedback},
    retry::execute_stream_with_retry_and_fallback,
    tool_flow::ToolUseOutcome,
    tool_runtime::ToolRuntimeHost,
    turn_loop::wait_for_turn_cancellation,
};

/// Upper bound on tool rounds for a single sub-agent invocation. Generous enough
/// for real multi-step tasks, but bounds a tool-looping model that would
/// otherwise run (and bill) indefinitely — the sub-agent loop has no interactive
/// backstop.
const MAX_AGENT_TOOL_ROUNDS: usize = 50;

impl SessionManager {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_agent_session_loop(
        &self,
        session_id: &str,
        tool_use_id: &str,
        agent: &AgentToolInput,
        agent_id: &str,
        agent_type: &str,
        agent_definition: Option<&AgentDefinition>,
        preloaded_skills: &[SkillDefinition],
        child_session_id: &str,
        allow_tools: bool,
        allow_network: bool,
        persist_child_transcript: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<AgentLoopOutcome, CoreError> {
        let context = self.context_preview().await;
        let mut agent_messages = vec![TranscriptMessage::new(
            MessageRole::User,
            agent.prompt.clone(),
        )];
        for additional_context in self
            .run_subagent_start_hooks(session_id, agent_id, agent_type, agent_definition, tx)
            .await
        {
            agent_messages.push(TranscriptMessage::new(
                MessageRole::User,
                subagent_start_hook_context(&additional_context),
            ));
        }
        let mut total_tool_uses = 0_u64;
        let mut usage = TokenUsage::default();
        let mut stop_hook_active = false;

        self.append_background_tool_progress_event(
            session_id,
            tool_use_id,
            "Agent",
            initial_agent_progress_record(agent_id, &agent.prompt),
            tx,
        )
        .await?;
        if persist_child_transcript {
            self.persist_child_agent_transcript_snapshot(
                child_session_id,
                agent,
                agent_type,
                agent_definition,
                &agent_messages,
            )
            .await;
        }

        let mut agent_round = 0_usize;
        let final_text = loop {
            if cancel_flag.load(Ordering::SeqCst) {
                return Ok(AgentLoopOutcome::Cancelled);
            }
            agent_round += 1;
            if agent_round > MAX_AGENT_TOOL_ROUNDS {
                // Bound the sub-agent loop so a tool-looping model cannot run —
                // and bill — indefinitely. The cap is generous enough for real
                // multi-step tasks; hitting it means the agent is stuck.
                break format!(
                    "[agent stopped after reaching the maximum of {MAX_AGENT_TOOL_ROUNDS} tool rounds]"
                );
            }
            let mut request = self
                .provider_request_for_messages(
                    child_session_id,
                    &agent.prompt,
                    context.clone(),
                    agent_messages.clone(),
                    allow_tools,
                    allow_network,
                )
                .await;
            if let Some(definition) = agent_definition.as_ref() {
                apply_agent_definition_to_request(&mut request, definition);
            }
            apply_preloaded_skills_to_request(&mut request, preloaded_skills);
            self.provider_debug_trace
                .record(self.config.default_provider, "agent", &request)
                .await;
            let mut stream_sink = AgentProviderStreamSink::new(
                self,
                session_id,
                tool_use_id,
                agent_id,
                self.config.default_provider,
                tx,
            );
            let stream_result = tokio::select! {
            response = execute_stream_with_retry_and_fallback(
                &self.config,
                &self.auth,
                request,
                &mut stream_sink,
                ProviderCancellationToken::from_flag(cancel_flag.clone()),
                ) => response,
                _ = wait_for_turn_cancellation(cancel_flag.clone()) => {
                    return Ok(AgentLoopOutcome::Cancelled);
                }
            };
            if cancel_flag.load(Ordering::SeqCst) {
                return Ok(AgentLoopOutcome::Cancelled);
            }
            let completion = stream_result?;
            crate::overview::accumulate_token_usage(&mut usage, completion.usage);
            let assistant_message = stream_sink.into_message();
            self.provider_debug_trace
                .append_message_activity(
                    self.config.default_provider,
                    "assistant_response_from_llm",
                    "agent assistant response",
                    &assistant_message,
                )
                .await;
            let tool_uses = agent_nested_tool_uses(&assistant_message);

            if tool_uses.is_empty() {
                let final_text = agent_final_text(&agent.description, &assistant_message.content);
                let stop_hook_outcome = self
                    .run_subagent_stop_hooks(
                        session_id,
                        agent_id,
                        child_session_id,
                        agent_type,
                        &final_text,
                        stop_hook_active,
                        agent_definition,
                        tx,
                    )
                    .await;
                if stop_hook_outcome.prevent_continuation {
                    agent_messages.push(assistant_message);
                    if persist_child_transcript {
                        self.persist_child_agent_transcript_snapshot(
                            child_session_id,
                            agent,
                            agent_type,
                            agent_definition,
                            &agent_messages,
                        )
                        .await;
                    }
                    self.emit_hook_notice(
                        session_id,
                        "SubagentStop",
                        stop_hook_outcome
                            .stop_reason
                            .as_deref()
                            .unwrap_or("SubagentStop hook prevented continuation"),
                        false,
                        tx,
                    )
                    .await;
                    break final_text;
                }
                if !stop_hook_outcome.blocking_errors.is_empty() {
                    agent_messages.push(assistant_message);
                    for blocking_error in stop_hook_outcome.blocking_errors {
                        let message = TranscriptMessage::new(
                            MessageRole::User,
                            subagent_stop_hook_feedback(&blocking_error),
                        );
                        self.provider_debug_trace
                            .append_message_activity(
                                self.config.default_provider,
                                "hook_feedback_to_llm",
                                "SubagentStop hook feedback",
                                &message,
                            )
                            .await;
                        agent_messages.push(message);
                    }
                    if persist_child_transcript {
                        self.persist_child_agent_transcript_snapshot(
                            child_session_id,
                            agent,
                            agent_type,
                            agent_definition,
                            &agent_messages,
                        )
                        .await;
                    }
                    stop_hook_active = true;
                    continue;
                }
                agent_messages.push(assistant_message);
                if persist_child_transcript {
                    self.persist_child_agent_transcript_snapshot(
                        child_session_id,
                        agent,
                        agent_type,
                        agent_definition,
                        &agent_messages,
                    )
                    .await;
                }
                break final_text;
            }

            agent_messages.push(assistant_message.clone());
            for child_tool_use in tool_uses {
                total_tool_uses += 1;
                let result = match self
                    .invoke_nested_agent_tool(
                        session_id,
                        tool_use_id,
                        agent_id,
                        &child_tool_use.tool_name,
                        &child_tool_use.tool_input,
                        agent_definition,
                        allow_tools,
                        allow_network,
                        tx,
                        cancel_flag.clone(),
                    )
                    .await
                {
                    Ok(outcome) => agent_nested_tool_success_result(&outcome),
                    Err(error) if error.is_interrupted() => {
                        return Ok(AgentLoopOutcome::Cancelled);
                    }
                    Err(error) => agent_nested_tool_error_result(&child_tool_use.tool_name, &error),
                };

                let metadata_value =
                    serde_json::from_str::<Value>(&result.metadata).unwrap_or(Value::Null);
                self.append_background_tool_progress_event(
                    session_id,
                    tool_use_id,
                    "Agent",
                    agent_tool_result_progress_record(
                        agent_id,
                        &child_tool_use.tool_use_id,
                        &result.content,
                        result.is_error,
                        &metadata_value,
                    ),
                    tx,
                )
                .await?;
                let message = tool_result_message(
                    &child_tool_use.tool_use_id,
                    result.content,
                    result.is_error,
                    Some(result.metadata),
                );
                self.provider_debug_trace
                    .append_message_activity(
                        self.config.default_provider,
                        "tool_result_to_llm",
                        "agent tool result",
                        &message,
                    )
                    .await;
                agent_messages.push(message);
            }
            if persist_child_transcript {
                self.persist_child_agent_transcript_snapshot(
                    child_session_id,
                    agent,
                    agent_type,
                    agent_definition,
                    &agent_messages,
                )
                .await;
            }
        };

        Ok(AgentLoopOutcome::Completed {
            final_text,
            total_tool_uses,
            usage,
        })
    }

    /// Persists a snapshot of the child agent's transcript for live viewing.
    ///
    /// This is **best-effort**: a transient write failure (disk full,
    /// permissions) must not abort `run_agent_session_loop` after the model
    /// has already produced valid output. On failure we log and continue —
    /// the next snapshot (or the final one) will retry the full write.
    pub(super) async fn persist_child_agent_transcript_snapshot(
        &self,
        child_session_id: &str,
        agent: &AgentToolInput,
        agent_type: &str,
        agent_definition: Option<&AgentDefinition>,
        messages: &[TranscriptMessage],
    ) {
        if messages.is_empty() {
            return;
        }

        let config = self.effective_config();
        let context = self.context_preview().await;
        let mut session = SessionRecord::new();
        session.session_id = child_session_id.to_string();
        let title = agent.description.trim();
        session.title = Some(format!(
            "Agent: {}",
            if title.is_empty() { agent_type } else { title }
        ));
        session.created_at = messages
            .first()
            .map_or_else(Utc::now, |message| message.created_at);
        session.cwd = Some(config.cwd.display().to_string());
        session.git_branch = context.git_branch.clone();
        session.provider = Some(self.config.default_provider);
        session.model = Some(
            agent_definition
                .and_then(|definition| definition.model.as_deref())
                .map(str::trim)
                .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("inherit"))
                .map(str::to_string)
                .unwrap_or_else(|| {
                    self.config
                        .provider_model_resolution(self.config.default_provider)
                        .request_model
                }),
        );
        for message in messages {
            session.push_message(message.clone());
        }
        let transcript_path = self
            .child_session_store
            .transcript_path_for(child_session_id);
        self.transcript_store.record_session_location(
            child_session_id,
            &transcript_path,
            &config.cwd,
        );
        self.transcript_store
            .record_session_hints(
                child_session_id,
                SessionWriteHints {
                    git_branch: context.git_branch,
                    provider: Some(config.default_provider),
                },
            )
            .await;
        if let Err(error) = self.transcript_store.persist_full_session(&session).await {
            eprintln!(
                "background agent {child_session_id}: failed to persist transcript snapshot: {error}"
            );
        }
    }

    /// Reject a background-agent spawn that would exceed the concurrency cap,
    /// emitting a terminal that is consistent with the error tool_result. The
    /// appended tool_result is `is_error=true`, so the completion MUST be
    /// `ExecutionFailed` — emitting `Success` here made stream-json / TUI / ACP
    /// consumers see a contradictory terminal for the same tool use.
    pub(super) async fn reject_background_agent_over_cap(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolUseOutcome, CoreError> {
        let message = format!(
            "Cannot start another background agent: the concurrent limit of \
             {MAX_CONCURRENT_BACKGROUND_AGENTS} is already in use. Wait for a running \
             background agent to finish (or stop one with TaskStop) and try again."
        );
        self.append_tool_result_message(session_id, tool_use_id, message, true, None, tx)
            .await?;
        self.emit_tool_use_completed(
            session_id,
            tool_use_id,
            "Agent",
            ToolUseCompletionKind::ExecutionFailed,
            tx,
        );
        Ok(ToolUseOutcome::Continue)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start_background_agent_task(
        &self,
        session_id: &str,
        tool_use_id: &str,
        agent: AgentToolInput,
        agent_id: String,
        agent_type: String,
        agent_definition: Option<AgentDefinition>,
        child_session_id: String,
        resolved_model: String,
        permission_mode: Option<PermissionMode>,
        allow_tools: bool,
        allow_network: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolUseOutcome, CoreError> {
        // Apply the agent's declared `permissionMode` to the detached loop
        // (see `apply_agent_permission_mode`); the recorded mode still reflects
        // the declaration, but the loop's grant is now enforced.
        let (allow_tools, allow_network) =
            apply_agent_permission_mode(permission_mode, allow_tools, allow_network);

        // Bound concurrent background agents. The permit is held for the whole
        // lifetime of the detached worker and released when it finishes; if none
        // is free, refuse the spawn with an error tool_result instead of piling
        // on unbounded parallel work.
        let slot_permit = match BACKGROUND_AGENT_SLOTS.try_acquire() {
            Ok(permit) => permit,
            Err(_) => {
                return self
                    .reject_background_agent_over_cap(session_id, tool_use_id, tx)
                    .await;
            }
        };

        let config = self.effective_config();
        let job_id = format!("agent-{}", Uuid::new_v4().simple());
        let log_path = background_log_path(&config.home_dir, &job_id);
        if let Some(parent) = log_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&log_path, "").await?;

        let record = BackgroundTaskRecord::new_local_agent(
            job_id.clone(),
            session_id.to_string(),
            child_session_id.clone(),
            tool_use_id.to_string(),
            agent_type.clone(),
            agent.prompt.clone(),
            config.cwd.display().to_string(),
            Some(resolved_model.clone()),
            permission_mode,
            log_path.display().to_string(),
        );
        write_background_task_record(&config.home_dir, &record)
            .await
            .map_err(|error| CoreError::Tool(format!("persist background record: {error}")))?;

        let bg_cancel = Arc::new(AtomicBool::new(false));
        register_background_task_cancel_flag(&job_id, bg_cancel.clone());

        let started_message = format!(
            "Background subagent started.\n\nTask ID: {job_id}\nDescription: {description}\n\nUse the TaskOutput tool with this task_id to collect the final result and TaskStop to cancel.\n\nIMPORTANT: When you reply to the user, you MUST quote this task_id verbatim (`{job_id}`). Do NOT paraphrase, abbreviate, or substitute a placeholder such as `agent-xxxx` or `<id>` — the user needs the exact string to invoke TaskOutput/TaskStop themselves.",
            description = agent.description,
        );
        let metadata = serde_json::json!({
            "status": "background_started",
            "toolName": "Agent",
            "task_id": job_id,
            "task_type": BackgroundTaskKind::LocalAgent.as_str(),
            "child_session_id": child_session_id,
            "agent_type": agent_type,
        });
        self.append_tool_result_message(
            session_id,
            tool_use_id,
            started_message,
            false,
            Some(metadata.to_string()),
            tx,
        )
        .await?;
        self.emit_tool_use_completed(
            session_id,
            tool_use_id,
            "Agent",
            ToolUseCompletionKind::Success,
            tx,
        );

        let preloaded_skills = self
            .preload_agent_skills(agent_definition.as_ref(), session_id, tool_use_id, tx)
            .await;

        let child_mcp = self.maybe_create_child_mcp(agent_definition.as_ref()).await;
        let manager = self.agent_loop_runner(child_mcp.as_ref());
        let session_id_owned = session_id.to_string();
        let synthetic_tool_use_id = format!("bg-{job_id}");
        let job_id_for_task = job_id.clone();
        let child_session_id_for_task = child_session_id.clone();
        let log_path_for_task = log_path.clone();
        let home_dir_for_task = config.home_dir.clone();
        let agent_for_task = agent.clone();
        let agent_id_for_task = agent_id.clone();
        let agent_type_for_task = agent_type.clone();
        let cancel_for_task = bg_cancel.clone();

        let progress_tx = register_progress_stream(&job_id_for_task, 256);
        let spawn_instant = Instant::now();

        // Detached background-agent worker; lifecycle is tracked in the background task record.
        let _background_agent_handle = tokio::spawn(async move {
            // Hold the concurrency permit for the whole worker lifetime; dropping
            // it here (on any exit path) frees the slot for the next spawn.
            let _slot_permit = slot_permit;
            let (bg_tx, mut bg_rx) = mpsc::unbounded_channel::<StreamEvent>();
            let forwarder = tokio::spawn(async move {
                while let Some(event) = bg_rx.recv().await {
                    let _ = progress_tx.send(event);
                }
            });

            let loop_result = manager
                .run_agent_session_loop(
                    &session_id_owned,
                    &synthetic_tool_use_id,
                    &agent_for_task,
                    &agent_id_for_task,
                    &agent_type_for_task,
                    agent_definition.as_ref(),
                    &preloaded_skills,
                    &child_session_id_for_task,
                    allow_tools,
                    allow_network,
                    false,
                    &bg_tx,
                    cancel_for_task,
                )
                .await;
            drop(bg_tx);
            let _ = forwarder.await;

            if let Some(ref child_registry) = child_mcp {
                shutdown_child_mcp_registry(child_registry).await;
            }

            let duration_ms = spawn_instant.elapsed().as_millis() as u64;
            finalize_background_agent_task(
                &manager,
                &home_dir_for_task,
                &job_id_for_task,
                &child_session_id_for_task,
                &log_path_for_task,
                loop_result,
                duration_ms,
            )
            .await;
            unregister_background_task_cancel_flag(&job_id_for_task);
            unregister_progress_stream(&job_id_for_task);
        });

        Ok(ToolUseOutcome::Continue)
    }

    pub(super) async fn invoke_nested_agent_tool(
        &self,
        session_id: &str,
        parent_tool_use_id: &str,
        agent_id: &str,
        tool_name: &str,
        tool_input: &str,
        agent_definition: Option<&AgentDefinition>,
        allow_tools: bool,
        allow_network: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<ToolOutcome, ToolError> {
        if tool_name.eq_ignore_ascii_case("Agent") {
            return Err(ToolError::ExecutionFailed(
                "nested Agent tool use is not supported yet".into(),
            ));
        }

        // Enforce the agent's declared tool allowlist at execution time. The
        // model-visible tool filter is advisory; without this check a sub-agent
        // could run a tool outside its sandbox using the parent's permissions.
        if let Some(definition) = agent_definition
            && !agent_definition_permits_tool(definition, tool_name)
        {
            return Err(ToolError::ExecutionFailed(format!(
                "tool `{tool_name}` is not permitted by this agent's configured tool allowlist"
            )));
        }

        // Enforce the child's tool/network permission grant (which reflects the
        // agent's `permissionMode`, e.g. `plan` → no tool execution). The nested
        // loop cannot prompt interactively, so a tool that requires a permission
        // the agent was not granted is blocked rather than silently executed.
        if let Some(spec) = self.tools.spec(tool_name) {
            let permitted = (!spec.requires_tools_permission || allow_tools)
                && (!spec.requires_network_permission || allow_network);
            if !permitted {
                return Err(ToolError::ExecutionFailed(format!(
                    "tool `{tool_name}` requires a permission this agent was not granted \
                     (see the agent's permissionMode)"
                )));
            }
        }

        let mut context = self.tool_context(
            session_id,
            allow_tools,
            allow_network,
            Arc::new(NestedAgentToolProgressReporter {
                manager: self.clone(),
                session_id: session_id.to_string(),
                parent_tool_use_id: parent_tool_use_id.to_string(),
                agent_id: agent_id.to_string(),
                tx: tx.clone(),
            }),
            cancel_flag,
        );
        if tool_name.eq_ignore_ascii_case("Skill") {
            context.skill_definitions =
                Some(self.skill_definitions_visible_to_session(session_id).await);
        }
        self.tools.invoke(tool_name, tool_input, &context).await
    }

    /// If the agent definition declares `mcp_server_names`, create an
    /// independent child [`McpRegistry`] with only those servers. Returns
    /// `None` when no filtering is needed (inherit parent) or when registry
    /// creation fails (fall back to parent).
    pub(super) async fn maybe_create_child_mcp(
        &self,
        definition: Option<&AgentDefinition>,
    ) -> Option<McpRegistry> {
        let server_names = definition?.mcp_server_names.as_ref()?;
        let config = self.effective_config();
        create_child_mcp_registry(&config.home_dir, &config.cwd, server_names).await
    }

    /// Return a SessionManager clone suitable for running the agent loop. If a
    /// child MCP registry was created, the clone uses it; otherwise the parent
    /// registry is inherited unchanged.
    pub(super) fn agent_loop_runner(&self, child_mcp: Option<&McpRegistry>) -> SessionManager {
        match child_mcp {
            Some(registry) => {
                let mut runner = self.clone();
                runner.mcp = registry.clone();
                runner
            }
            None => self.clone(),
        }
    }

    /// Preload skills declared by the agent definition. Missing skills are
    /// silently skipped so an outdated definition does not break the child
    /// loop. Skill discovery failures fall back to no preload rather than
    /// failing the agent invocation.
    pub(super) async fn preload_agent_skills(
        &self,
        agent_definition: Option<&AgentDefinition>,
        session_id: &str,
        _tool_use_id: &str,
        _tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Vec<SkillDefinition> {
        let Some(definition) = agent_definition else {
            return Vec::new();
        };
        if definition.skills.is_empty() {
            return Vec::new();
        }
        let available = self.skill_definitions_visible_to_session(session_id).await;
        let (matched, _unknown) = resolve_requested_skills(&available, &definition.skills);
        matched.into_iter().cloned().collect()
    }
}

async fn finalize_background_agent_task(
    manager: &SessionManager,
    home_dir: &std::path::Path,
    job_id: &str,
    child_session_id: &str,
    log_path: &std::path::Path,
    loop_result: Result<AgentLoopOutcome, CoreError>,
    duration_ms: u64,
) {
    let (status, error_message, final_text, usage) = match loop_result {
        Ok(AgentLoopOutcome::Completed {
            final_text, usage, ..
        }) => (
            BackgroundTaskStatus::Completed,
            None,
            Some(final_text),
            Some(usage),
        ),
        Ok(AgentLoopOutcome::Cancelled) => (BackgroundTaskStatus::Cancelled, None, None, None),
        Err(error) => (
            BackgroundTaskStatus::Failed,
            Some(error.to_string()),
            None,
            None,
        ),
    };

    match status {
        BackgroundTaskStatus::Completed => {
            let _ = manager.child_session_store.complete(child_session_id).await;
        }
        BackgroundTaskStatus::Cancelled => {
            let _ = manager.child_session_store.cancel(child_session_id).await;
        }
        BackgroundTaskStatus::Failed => {
            let _ = manager
                .child_session_store
                .fail(
                    child_session_id,
                    error_message.as_deref().unwrap_or("agent loop failed"),
                )
                .await;
        }
        _ => {}
    }

    if let Some(text) = final_text.as_deref()
        && let Err(error) = tokio::fs::write(log_path, text).await
    {
        eprintln!("background agent {job_id}: failed to write log: {error}");
    }

    match read_background_task_record(home_dir, job_id).await {
        Ok(Some(mut record)) => {
            let now = Utc::now().to_rfc3339();
            record.status = status;
            record.updated_at = now.clone();
            record.finished_at = Some(now);
            record.error = error_message;
            record.result = final_text;
            record
                .extra
                .insert("duration_ms".to_string(), Value::from(duration_ms));
            if let Some(usage) = usage {
                record.extra.insert(
                    "token_usage".to_string(),
                    serde_json::json!({
                        "input_tokens": usage.input_tokens,
                        "output_tokens": usage.output_tokens,
                        "cache_creation_input_tokens": usage.cache_creation_input_tokens,
                        "cache_read_input_tokens": usage.cache_read_input_tokens,
                    }),
                );
            }
            if let Err(error) = write_background_task_record(home_dir, &record).await {
                eprintln!("background agent {job_id}: failed to update record: {error}");
            }
        }
        Ok(None) => {
            eprintln!("background agent {job_id}: record missing at finalization");
        }
        Err(error) => {
            eprintln!("background agent {job_id}: failed to load record: {error}");
        }
    }
}

/// Create an independent [`McpRegistry`] containing only the servers named
/// in `server_names`. The registry is loaded fresh (its own Arc state) so
/// mutating or shutting it down does not affect the parent's connections.
///
/// Returns `None` when loading fails — callers fall back to the parent
/// registry rather than failing the child agent invocation.
pub(super) async fn create_child_mcp_registry(
    home_dir: &std::path::Path,
    cwd: &std::path::Path,
    server_names: &[String],
) -> Option<McpRegistry> {
    let registry = McpRegistry::load(home_dir, cwd).await.ok()?;
    let server_set: std::collections::HashSet<&str> =
        server_names.iter().map(String::as_str).collect();
    registry
        .retain_policy_allowed(|id| server_set.contains(id))
        .await;
    Some(registry)
}

/// Shut down all stdio/transport clients in `registry`. Called when a child
/// agent loop finishes to release processes and connections that were opened
/// exclusively for the child. Uses `retain_policy_allowed` with a reject-all
/// predicate to trigger shutdown of every server's client.
pub(super) async fn shutdown_child_mcp_registry(registry: &McpRegistry) {
    registry.retain_policy_allowed(|_| false).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_child_mcp_registry_returns_filtered_registry() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let home = tmp.path().join("home");
        tokio::fs::create_dir_all(home.join("mcp"))
            .await
            .expect("create mcp dir");
        let cwd = tmp.path().join("cwd");
        tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

        let registry =
            create_child_mcp_registry(&home, &cwd, &["nonexistent-server".to_string()]).await;
        assert!(
            registry.is_some(),
            "registry loads even with no matching servers"
        );
        let registry = registry.unwrap();
        let servers = registry.list_servers().await;
        assert!(
            servers.is_empty(),
            "no servers match so the registry is empty"
        );
    }

    #[tokio::test]
    async fn shutdown_child_mcp_registry_is_safe_on_empty_registry() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let home = tmp.path().join("home");
        tokio::fs::create_dir_all(home.join("mcp"))
            .await
            .expect("create mcp dir");
        let cwd = tmp.path().join("cwd");
        tokio::fs::create_dir_all(&cwd).await.expect("create cwd");

        let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
        shutdown_child_mcp_registry(&registry).await;
        assert!(
            registry.list_servers().await.is_empty(),
            "all servers removed after shutdown"
        );
    }
}
