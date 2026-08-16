use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use orbcode_config::mcp_permission_target;
use orbcode_protocol::{
    AskUserCancellationReason, AskUserResponseOutcome, StreamEvent, ToolUseCompletionKind,
};
use orbcode_tools::{
    SkillDefinition, ToolContext, ToolOutcome, ToolProgressReporter, ToolRegistry, ToolSpec,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
};

use crate::{
    CoreError,
    agent_loop::tool_round::ToolRoundReadyItem,
    hooks::{HookPermissionDecision, PreToolPhaseOutcome},
    interaction_runtime::{InteractionRuntime, PendingInteraction},
    permissions::{PermissionBoundaryReason, PermissionContext, PermissionEvaluation},
    tool_flow::{
        BufferedToolResult, BufferedToolUseCompletion, McpTrustResolutionOutcome,
        ToolDenyPrecedenceStage, ToolInvocationPermissions, ToolLookupOutcome,
        ToolPermissionResolutionOutcome, ToolUseOutcome, tool_error_result_details,
    },
};

pub(crate) struct ToolSuccessResultDetails {
    pub(crate) content: String,
    pub(crate) metadata: String,
}

#[async_trait]
pub(crate) trait ToolRuntimeHost {
    async fn lookup_tool_spec_or_append_unknown(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolLookupOutcome, CoreError>;

    fn permission_context(&self, session_id: &str) -> PermissionContext;

    async fn tool_deny_precedence_reason(
        &self,
        session_id: &str,
        permissions: &PermissionContext,
        tool_name: &str,
        tool_input: &str,
        stage: ToolDenyPrecedenceStage,
    ) -> Option<String>;

    async fn run_pre_tool_phase(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<PreToolPhaseOutcome, CoreError>;

    async fn deny_tool_use(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        reason: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolUseOutcome, CoreError>;

    async fn evaluate_tool_permission(
        &self,
        session_id: &str,
        permissions: &PermissionContext,
        tool_name: &str,
        tool_input: &str,
        spec: &ToolSpec,
        hook_allowed: bool,
    ) -> PermissionEvaluation;

    async fn resolve_tool_permission_request(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        spec: &ToolSpec,
        evaluation: &PermissionEvaluation,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> ToolPermissionResolutionOutcome;

    async fn resolve_mcp_trust_if_needed(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> McpTrustResolutionOutcome;

    async fn append_initial_tool_progress_event(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError>;

    fn live_tool_progress_reporter(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Arc<dyn ToolProgressReporter>;

    fn tool_context(
        &self,
        session_id: &str,
        allow_tools: bool,
        allow_network: bool,
        boundary_override: bool,
        progress: Arc<dyn ToolProgressReporter>,
        cancel_flag: Arc<AtomicBool>,
    ) -> ToolContext;

    async fn skill_definitions(&self, session_id: &str) -> Vec<SkillDefinition>;

    fn ask_user_pending(&self) -> InteractionRuntime;

    async fn active_interaction_context(
        &self,
        session_id: &str,
    ) -> Option<(uuid::Uuid, crate::TurnInteractionContext)>;

    async fn tool_success_result_details(
        &self,
        session_id: &str,
        tool_use_id: &str,
        outcome: &ToolOutcome,
    ) -> Result<ToolSuccessResultDetails, CoreError>;

    async fn run_agent_tool(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
        allow_tools: bool,
        allow_network: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<ToolUseOutcome, CoreError>;

    async fn run_workflow_tool(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolUseOutcome, CoreError>;

    async fn run_persistent_goal_tool(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolUseOutcome, CoreError>;

    async fn run_agent_tool_buffered(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
        allow_tools: bool,
        allow_network: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<BufferedToolUseCompletion, CoreError>;

    async fn run_workflow_tool_buffered(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<BufferedToolUseCompletion, CoreError>;

    async fn run_persistent_goal_tool_buffered(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
    ) -> Result<BufferedToolUseCompletion, CoreError>;

    async fn append_tool_result(
        &self,
        session_id: &str,
        tool_use_id: &str,
        content: impl Into<String> + Send,
        is_error: bool,
        metadata: Option<String>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError>;

    async fn append_post_tool_contexts(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        outcome: &ToolOutcome,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError>;

    async fn append_post_tool_failure_contexts(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        error_message: &str,
        is_interrupt: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError>;

    fn emit_tool_use_started(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    );

    fn emit_tool_use_completed(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        kind: ToolUseCompletionKind,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    );
}

pub(crate) struct ToolRuntime<'a, H> {
    tools: &'a ToolRegistry,
    host: &'a H,
}

impl<'a, H> ToolRuntime<'a, H>
where
    H: ToolRuntimeHost + Sync,
{
    pub(crate) fn new(tools: &'a ToolRegistry, host: &'a H) -> Self {
        Self { tools, host }
    }

    pub(crate) async fn execute_tool_use(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<ToolUseOutcome, CoreError> {
        if cancel_flag.load(Ordering::SeqCst) {
            return Ok(ToolUseOutcome::Cancelled);
        }

        let spec = match self
            .host
            .lookup_tool_spec_or_append_unknown(session_id, tool_use_id, tool_name, tx)
            .await?
        {
            ToolLookupOutcome::Found(spec) => spec,
            ToolLookupOutcome::UnknownHandled => return Ok(ToolUseOutcome::Continue),
        };

        let permissions = self.host.permission_context(session_id);
        if let Some(reason) = self
            .host
            .tool_deny_precedence_reason(
                session_id,
                &permissions,
                tool_name,
                tool_input,
                ToolDenyPrecedenceStage::OriginalInput,
            )
            .await
        {
            return self
                .host
                .deny_tool_use(session_id, tool_use_id, tool_name, tool_input, &reason, tx)
                .await;
        }

        let pre_tool_outcome = self
            .host
            .run_pre_tool_phase(session_id, tool_use_id, tool_name, tool_input, tx)
            .await?;
        let effective_tool_input = pre_tool_outcome.tool_input;

        if effective_tool_input != tool_input
            && let Some(reason) = self
                .host
                .tool_deny_precedence_reason(
                    session_id,
                    &permissions,
                    tool_name,
                    &effective_tool_input,
                    ToolDenyPrecedenceStage::PreToolInputUpdate,
                )
                .await
        {
            return self
                .host
                .deny_tool_use(
                    session_id,
                    tool_use_id,
                    tool_name,
                    &effective_tool_input,
                    &reason,
                    tx,
                )
                .await;
        }

        if matches!(
            pre_tool_outcome.decision,
            Some(HookPermissionDecision::Deny)
        ) {
            return self
                .host
                .deny_tool_use(
                    session_id,
                    tool_use_id,
                    tool_name,
                    &effective_tool_input,
                    &format!(
                        "permission denied for tool `{tool_name}` by PreToolUse hook{}",
                        pre_tool_outcome
                            .reason
                            .as_deref()
                            .map(|reason| format!(": {reason}"))
                            .unwrap_or_default()
                    ),
                    tx,
                )
                .await;
        }

        let evaluation = if matches!(pre_tool_outcome.decision, Some(HookPermissionDecision::Ask)) {
            PermissionEvaluation::AskUser {
                reason: PermissionBoundaryReason::ExplicitHookAsk,
            }
        } else {
            self.host
                .evaluate_tool_permission(
                    session_id,
                    &permissions,
                    tool_name,
                    &effective_tool_input,
                    &spec,
                    matches!(
                        pre_tool_outcome.decision,
                        Some(HookPermissionDecision::Allow)
                    ),
                )
                .await
        };
        let invocation_permissions = match &evaluation {
            PermissionEvaluation::Allow { .. } => {
                if evaluation.is_explicit_allow() {
                    ToolInvocationPermissions::after_explicit_allow(
                        &permissions,
                        &spec,
                        permissions.requires_sandbox_boundary_override(
                            &spec,
                            tool_name,
                            &effective_tool_input,
                        ),
                    )
                } else {
                    ToolInvocationPermissions::from_permission_context(&permissions, &spec)
                }
            }
            PermissionEvaluation::Deny { reason } => {
                return self
                    .host
                    .deny_tool_use(
                        session_id,
                        tool_use_id,
                        tool_name,
                        &effective_tool_input,
                        reason,
                        tx,
                    )
                    .await;
            }
            PermissionEvaluation::AskUser { .. } | PermissionEvaluation::AutoReview { .. } => {
                match self
                    .host
                    .resolve_tool_permission_request(
                        session_id,
                        tool_use_id,
                        tool_name,
                        &effective_tool_input,
                        &spec,
                        &evaluation,
                        tx,
                        cancel_flag.clone(),
                    )
                    .await
                {
                    ToolPermissionResolutionOutcome::Approved => {
                        ToolInvocationPermissions::after_explicit_allow(
                            &permissions,
                            &spec,
                            permissions.requires_sandbox_boundary_override(
                                &spec,
                                tool_name,
                                &effective_tool_input,
                            ),
                        )
                    }
                    ToolPermissionResolutionOutcome::Denied => {
                        let denied_tool = mcp_permission_target(tool_name, &effective_tool_input)
                            .unwrap_or_else(|| tool_name.to_string());
                        return self
                            .host
                            .deny_tool_use(
                                session_id,
                                tool_use_id,
                                tool_name,
                                &effective_tool_input,
                                &format!("permission denied for tool `{denied_tool}`"),
                                tx,
                            )
                            .await;
                    }
                    ToolPermissionResolutionOutcome::Interrupted => {
                        return Ok(ToolUseOutcome::Cancelled);
                    }
                }
            }
        };

        self.invoke_tool_and_append_result(
            session_id,
            tool_use_id,
            tool_name,
            &effective_tool_input,
            invocation_permissions,
            tx,
            cancel_flag,
        )
        .await
    }

    pub(crate) async fn invoke_tool_and_append_result(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        permissions: ToolInvocationPermissions,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<ToolUseOutcome, CoreError> {
        self.host
            .emit_tool_use_started(session_id, tool_use_id, tool_name, tool_input, tx);
        self.host
            .append_initial_tool_progress_event(session_id, tool_use_id, tool_name, tool_input, tx)
            .await?;

        if tool_name.eq_ignore_ascii_case("Agent") {
            return self
                .host
                .run_agent_tool(
                    session_id,
                    tool_use_id,
                    tool_input,
                    permissions.allow_tools,
                    permissions.inherited_allow_network,
                    tx,
                    cancel_flag,
                )
                .await;
        }
        if tool_name.eq_ignore_ascii_case("Workflow") {
            return self
                .host
                .run_workflow_tool(session_id, tool_use_id, tool_input, tx)
                .await;
        }
        if crate::session_manager::session_goal_tools::is_persistent_goal_tool(tool_name) {
            return self
                .host
                .run_persistent_goal_tool(session_id, tool_use_id, tool_name, tool_input, tx)
                .await;
        }

        match self
            .host
            .resolve_mcp_trust_if_needed(
                session_id,
                tool_use_id,
                tool_name,
                tx,
                cancel_flag.clone(),
            )
            .await
        {
            McpTrustResolutionOutcome::Proceed | McpTrustResolutionOutcome::Trusted => {}
            McpTrustResolutionOutcome::Denied => {
                let denied_tool = mcp_permission_target(tool_name, tool_input)
                    .unwrap_or_else(|| tool_name.to_string());
                return self
                    .host
                    .deny_tool_use(
                        session_id,
                        tool_use_id,
                        tool_name,
                        tool_input,
                        &format!("MCP server not trusted for tool `{denied_tool}`"),
                        tx,
                    )
                    .await;
            }
            McpTrustResolutionOutcome::Interrupted => {
                return Ok(ToolUseOutcome::Cancelled);
            }
        }

        let mut context = self.host.tool_context(
            session_id,
            permissions.allow_tools,
            permissions.allow_network,
            permissions.boundary_override,
            self.host
                .live_tool_progress_reporter(session_id, tool_use_id, tool_name, tx),
            cancel_flag,
        );
        if tool_name.eq_ignore_ascii_case("Skill") {
            context.skill_definitions = Some(self.host.skill_definitions(session_id).await);
        }

        let ask_forward_handle = self
            .attach_ask_user_channel(session_id, tool_use_id, &mut context, tx)
            .await;

        let invoke_result = self.tools.invoke(tool_name, tool_input, &context).await;
        drop(context);
        finish_ask_user_forwarder(ask_forward_handle).await?;

        match invoke_result {
            Ok(outcome) => {
                let details = self
                    .host
                    .tool_success_result_details(session_id, tool_use_id, &outcome)
                    .await?;
                self.host
                    .append_tool_result(
                        session_id,
                        tool_use_id,
                        details.content,
                        false,
                        Some(details.metadata),
                        tx,
                    )
                    .await?;
                self.host
                    .append_post_tool_contexts(
                        session_id,
                        tool_use_id,
                        tool_name,
                        tool_input,
                        &outcome,
                        tx,
                    )
                    .await?;
                self.host.emit_tool_use_completed(
                    session_id,
                    tool_use_id,
                    tool_name,
                    ToolUseCompletionKind::Success,
                    tx,
                );
            }
            Err(error) => {
                let details = tool_error_result_details(tool_name, &error);
                self.host
                    .append_tool_result(
                        session_id,
                        tool_use_id,
                        details.content.clone(),
                        true,
                        details.metadata,
                        tx,
                    )
                    .await?;
                if matches!(
                    details.completion_kind,
                    ToolUseCompletionKind::Interrupted | ToolUseCompletionKind::Cancelled
                ) {
                    self.host.emit_tool_use_completed(
                        session_id,
                        tool_use_id,
                        tool_name,
                        details.completion_kind,
                        tx,
                    );
                    self.host
                        .append_post_tool_failure_contexts(
                            session_id,
                            tool_use_id,
                            tool_name,
                            tool_input,
                            &details.content,
                            true,
                            tx,
                        )
                        .await?;
                    return Ok(ToolUseOutcome::Cancelled);
                }
                self.host
                    .append_post_tool_failure_contexts(
                        session_id,
                        tool_use_id,
                        tool_name,
                        tool_input,
                        &details.content,
                        false,
                        tx,
                    )
                    .await?;
                self.host.emit_tool_use_completed(
                    session_id,
                    tool_use_id,
                    tool_name,
                    details.completion_kind,
                    tx,
                );
            }
        }

        Ok(ToolUseOutcome::Continue)
    }

    pub(crate) async fn execute_streamed_tool_use(
        &self,
        session_id: &str,
        ready_item: ToolRoundReadyItem,
        permissions: ToolInvocationPermissions,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<BufferedToolUseCompletion, CoreError> {
        let tool_use_id = ready_item.tool_use_id().to_string();
        let tool_name = ready_item.tool_name().to_string();
        let tool_input = ready_item.tool_input().to_string();
        self.host
            .append_initial_tool_progress_event(
                session_id,
                &tool_use_id,
                &tool_name,
                &tool_input,
                tx,
            )
            .await?;

        if tool_name.eq_ignore_ascii_case("Agent") {
            return self
                .host
                .run_agent_tool_buffered(
                    session_id,
                    &tool_use_id,
                    &tool_input,
                    permissions.allow_tools,
                    permissions.inherited_allow_network,
                    tx,
                    cancel_flag,
                )
                .await;
        }
        if tool_name.eq_ignore_ascii_case("Workflow") {
            return self
                .host
                .run_workflow_tool_buffered(session_id, &tool_use_id, &tool_input, tx)
                .await;
        }
        if crate::session_manager::session_goal_tools::is_persistent_goal_tool(&tool_name) {
            return self
                .host
                .run_persistent_goal_tool_buffered(
                    session_id,
                    &tool_use_id,
                    &tool_name,
                    &tool_input,
                )
                .await;
        }

        let mut context = self.host.tool_context(
            session_id,
            permissions.allow_tools,
            permissions.allow_network,
            permissions.boundary_override,
            self.host
                .live_tool_progress_reporter(session_id, &tool_use_id, &tool_name, tx),
            cancel_flag,
        );
        if tool_name.eq_ignore_ascii_case("Skill") {
            context.skill_definitions = Some(self.host.skill_definitions(session_id).await);
        }
        let ask_forward_handle = self
            .attach_ask_user_channel(session_id, &tool_use_id, &mut context, tx)
            .await;

        let invoke_result = self.tools.invoke(&tool_name, &tool_input, &context).await;
        drop(context);
        finish_ask_user_forwarder(ask_forward_handle).await?;

        let result = match invoke_result {
            Ok(outcome) => {
                let details = self
                    .host
                    .tool_success_result_details(session_id, &tool_use_id, &outcome)
                    .await?;
                BufferedToolResult {
                    tool_use_id,
                    tool_name,
                    content: details.content,
                    is_error: false,
                    metadata: Some(details.metadata),
                    completion_kind: ToolUseCompletionKind::Success,
                }
            }
            Err(error) => {
                let details = tool_error_result_details(&tool_name, &error);
                BufferedToolResult {
                    tool_use_id,
                    tool_name,
                    content: details.content,
                    is_error: true,
                    metadata: details.metadata,
                    completion_kind: details.completion_kind,
                }
            }
        };
        let outcome = match result.completion_kind {
            ToolUseCompletionKind::Interrupted | ToolUseCompletionKind::Cancelled => {
                ToolUseOutcome::Cancelled
            }
            _ => ToolUseOutcome::Continue,
        };
        Ok(BufferedToolUseCompletion { outcome, result })
    }

    async fn attach_ask_user_channel(
        &self,
        session_id: &str,
        tool_use_id: &str,
        context: &mut ToolContext,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Option<JoinHandle<Result<(), CoreError>>> {
        let (turn_id, interaction) = self.host.active_interaction_context(session_id).await?;
        if !interaction.capabilities.any_supported() {
            return None;
        }
        let (ask_tx, mut ask_rx) = mpsc::unbounded_channel::<orbcode_tools::AskUserRequest>();
        context.ask_user_tx = Some(ask_tx);

        let tx_clone = tx.clone();
        let session_id = session_id.to_string();
        let tool_use_id = tool_use_id.to_string();
        let turn_id = turn_id.to_string();
        let owner_id = interaction.owner_id;
        let capability_snapshot = interaction.capabilities;
        let interaction_runtime = self.host.ask_user_pending();
        Some(tokio::spawn(async move {
            let mut request_tasks = JoinSet::new();
            loop {
                tokio::select! {
                    request = ask_rx.recv() => {
                        let Some(req) = request else {
                            break;
                        };
                        let request_id = req.request_id.clone();
                        let questions = req.questions.clone();
                        if !capability_snapshot.can_complete(&questions) {
                            let _ = req.response_tx.send(AskUserResponseOutcome::Cancelled {
                                reason: AskUserCancellationReason::ClientClosed,
                            });
                            continue;
                        }
                        let legacy = legacy_stream_fields(&questions);
                        let deadline = chrono::Utc::now() + chrono::Duration::seconds(300);
                        let (pending_response_tx, pending_response_rx) = oneshot::channel();
                        if interaction_runtime
                            .register(
                                request_id.clone(),
                                PendingInteraction {
                                    session_id: session_id.clone(),
                                    turn_id: turn_id.clone(),
                                    tool_use_id: tool_use_id.clone(),
                                    owner_id: owner_id.clone(),
                                    capability_snapshot: capability_snapshot.clone(),
                                    deadline: Some(deadline.to_rfc3339()),
                                    questions: questions.clone(),
                                    response_tx: pending_response_tx,
                                },
                            )
                            .is_err()
                        {
                            continue;
                        }
                        if tx_clone
                            .send(StreamEvent::AskUserQuestionRequested {
                                session_id: session_id.clone(),
                                turn_id: Some(turn_id.clone()),
                                tool_use_id: tool_use_id.clone(),
                                request_id: request_id.clone(),
                                deadline: Some(deadline.to_rfc3339()),
                                questions,
                                question: legacy.0,
                                options: legacy.1,
                            })
                            .is_err()
                        {
                            interaction_runtime.cancel_request(
                                &request_id,
                                AskUserCancellationReason::DeliveryFailed,
                            );
                        }
                        request_tasks.spawn(run_pending_ask_user_request(
                            interaction_runtime.clone(),
                            request_id,
                            pending_response_rx,
                            req.response_tx,
                            std::time::Duration::from_secs(300),
                        ));
                    }
                    joined = request_tasks.join_next(), if !request_tasks.is_empty() => {
                        let Some(joined) = joined else {
                            continue;
                        };
                        observe_ask_user_request_task(joined)?;
                    }
                }
            }
            while let Some(joined) = request_tasks.join_next().await {
                observe_ask_user_request_task(joined)?;
            }
            Ok(())
        }))
    }
}

struct PendingAskUserRequestGuard {
    interaction_runtime: InteractionRuntime,
    request_id: String,
    armed: bool,
}

impl PendingAskUserRequestGuard {
    fn new(interaction_runtime: InteractionRuntime, request_id: String) -> Self {
        Self {
            interaction_runtime,
            request_id,
            armed: true,
        }
    }

    fn cancel(&mut self, reason: AskUserCancellationReason) {
        self.armed = false;
        self.interaction_runtime
            .cancel_request(&self.request_id, reason);
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingAskUserRequestGuard {
    fn drop(&mut self) {
        if self.armed {
            self.interaction_runtime
                .cancel_request(&self.request_id, AskUserCancellationReason::ClientClosed);
        }
    }
}

async fn run_pending_ask_user_request(
    interaction_runtime: InteractionRuntime,
    request_id: String,
    mut pending_response_rx: oneshot::Receiver<AskUserResponseOutcome>,
    mut response_tx: oneshot::Sender<AskUserResponseOutcome>,
    timeout: std::time::Duration,
) {
    let mut guard = PendingAskUserRequestGuard::new(interaction_runtime, request_id);
    tokio::select! {
        outcome = &mut pending_response_rx => {
            guard.disarm();
            if let Ok(outcome) = outcome {
                let _ = response_tx.send(outcome);
            }
        }
        () = response_tx.closed() => {
            guard.cancel(AskUserCancellationReason::Interrupt);
        }
        () = tokio::time::sleep(timeout) => {
            guard.cancel(AskUserCancellationReason::Timeout);
            let outcome = pending_response_rx.await.unwrap_or(AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::Timeout,
            });
            let _ = response_tx.send(outcome);
        }
    }
}

fn observe_ask_user_request_task(
    result: Result<(), tokio::task::JoinError>,
) -> Result<(), CoreError> {
    result.map_err(|error| CoreError::Tool(format!("AskUser request task failed: {error}")))
}

async fn finish_ask_user_forwarder(
    handle: Option<JoinHandle<Result<(), CoreError>>>,
) -> Result<(), CoreError> {
    let Some(handle) = handle else {
        return Ok(());
    };
    handle
        .await
        .map_err(|error| CoreError::Tool(format!("AskUser forwarder task failed: {error}")))?
}

fn legacy_stream_fields(
    questions: &[orbcode_protocol::AskUserQuestionSpec],
) -> (String, Vec<String>) {
    if questions.len() == 1 && !questions[0].multi_select {
        (
            questions[0].question.clone(),
            questions[0]
                .options
                .iter()
                .map(|option| option.label.clone())
                .collect(),
        )
    } else {
        (String::new(), Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use orbcode_protocol::AskUserQuestionSpec;

    use super::*;

    fn spawn_pending_request(
        runtime: &InteractionRuntime,
        timeout: Duration,
    ) -> (oneshot::Receiver<AskUserResponseOutcome>, JoinHandle<()>) {
        let request_id = "request-1".to_string();
        let (pending_response_tx, pending_response_rx) = oneshot::channel();
        let (response_tx, response_rx) = oneshot::channel();
        runtime
            .register(
                request_id.clone(),
                PendingInteraction {
                    session_id: "session-1".into(),
                    turn_id: "turn-1".into(),
                    tool_use_id: "tool-1".into(),
                    owner_id: "owner-1".into(),
                    capability_snapshot: crate::InteractiveQuestionCapabilities::full(),
                    deadline: None,
                    questions: vec![AskUserQuestionSpec {
                        id: "question-1".into(),
                        question: "Continue?".into(),
                        header: "Continue".into(),
                        multi_select: false,
                        options: Vec::new(),
                        allow_free_text: true,
                        allow_annotation: false,
                    }],
                    response_tx: pending_response_tx,
                },
            )
            .expect("unique test request id");
        let task = tokio::spawn(run_pending_ask_user_request(
            runtime.clone(),
            request_id,
            pending_response_rx,
            response_tx,
            timeout,
        ));
        (response_rx, task)
    }

    #[tokio::test]
    async fn ask_user_response_stops_timeout_task() {
        let runtime = InteractionRuntime::default();
        let (response_rx, task) = spawn_pending_request(&runtime, Duration::from_secs(60));
        runtime
            .resolve("session-1", "request-1", AskUserResponseOutcome::Rejected)
            .expect("resolve request");

        let outcome = tokio::time::timeout(Duration::from_millis(100), response_rx)
            .await
            .expect("response is forwarded without waiting for the timer")
            .expect("response sender remains open");
        assert_eq!(outcome, AskUserResponseOutcome::Rejected);
        tokio::time::timeout(Duration::from_millis(100), task)
            .await
            .expect("request task stops after forwarding the response")
            .expect("request task does not panic");
        assert_eq!(runtime.len(), 0);
    }

    #[tokio::test]
    async fn ask_user_timeout_cancels_pending_request_once() {
        let runtime = InteractionRuntime::default();
        let (response_rx, task) = spawn_pending_request(&runtime, Duration::from_millis(1));

        assert_eq!(
            response_rx.await.expect("timeout response"),
            AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::Timeout,
            }
        );
        task.await.expect("request task does not panic");
        assert_eq!(runtime.len(), 0);
        assert!(matches!(
            runtime.resolve("session-1", "request-1", AskUserResponseOutcome::Rejected,),
            Err(crate::interaction_runtime::InteractionResolveError::UnknownRequest { .. })
        ));
    }

    #[tokio::test]
    async fn ask_user_receiver_drop_cancels_pending_request_and_timer() {
        let runtime = InteractionRuntime::default();
        let (response_rx, task) = spawn_pending_request(&runtime, Duration::from_secs(60));
        drop(response_rx);

        tokio::time::timeout(Duration::from_millis(100), task)
            .await
            .expect("request task observes the closed tool receiver")
            .expect("request task does not panic");
        assert_eq!(runtime.len(), 0);
    }

    async fn panic_request_task() {
        panic!("request task panic canary");
    }

    #[tokio::test]
    async fn ask_user_request_task_panic_is_observable() {
        let joined = tokio::spawn(panic_request_task()).await;
        let error = observe_ask_user_request_task(joined).expect_err("panic must be an error");
        assert!(
            matches!(error, CoreError::Tool(message) if message.contains("request task failed"))
        );
    }

    async fn panic_forwarder_task() -> Result<(), CoreError> {
        panic!("forwarder task panic canary");
    }

    #[tokio::test]
    async fn ask_user_forwarder_panic_is_observable() {
        let error = finish_ask_user_forwarder(Some(tokio::spawn(panic_forwarder_task())))
            .await
            .expect_err("panic must be an error");
        assert!(
            matches!(error, CoreError::Tool(message) if message.contains("forwarder task failed"))
        );
    }
}
