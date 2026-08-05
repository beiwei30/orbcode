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
use tokio::sync::mpsc;

use crate::{
    CoreError,
    agent_loop::tool_round::ToolRoundReadyItem,
    hooks::{HookPermissionDecision, PreToolPhaseOutcome},
    interaction_runtime::{InteractionRuntime, PendingInteraction},
    permissions::PermissionContext,
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

    fn permission_context(&self) -> PermissionContext;

    async fn tool_deny_precedence_reason(
        &self,
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

    async fn matches_permission_rule(&self, tool_name: &str, tool_input: &str) -> bool;

    async fn resolve_tool_permission_request(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        spec: &ToolSpec,
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

        let permissions = self.host.permission_context();
        if let Some(reason) = self
            .host
            .tool_deny_precedence_reason(
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

        if spec.requires_tools_permission || spec.requires_network_permission {
            // An `ask` rule forces an interactive prompt (deny > ask > allow),
            // suppressing the config-allow and blanket auto-approve fast paths —
            // exactly as `streamed_tool_invocation_permissions` does on the
            // no-hooks path. Without this gate, an overlapping `ask` + `allow`
            // rule would auto-execute here on the hook/fallback path. An explicit
            // PreToolUse hook `Allow` and an in-session "always allow" grant
            // (`matches_permission_rule` below) still win — those are deliberate
            // authorizations, not the ambient config-allow.
            let should_ask = permissions.tool_should_ask(tool_name, &effective_tool_input);
            if matches!(
                pre_tool_outcome.decision,
                Some(HookPermissionDecision::Allow)
            ) {
                return self
                    .invoke_tool_and_append_result(
                        session_id,
                        tool_use_id,
                        tool_name,
                        &effective_tool_input,
                        ToolInvocationPermissions::after_explicit_allow(&permissions, &spec),
                        tx,
                        cancel_flag.clone(),
                    )
                    .await;
            }

            if !should_ask
                && permissions.tool_allowed_without_prompt(tool_name, &effective_tool_input)
            {
                return self
                    .invoke_tool_and_append_result(
                        session_id,
                        tool_use_id,
                        tool_name,
                        &effective_tool_input,
                        ToolInvocationPermissions::after_explicit_allow(&permissions, &spec),
                        tx,
                        cancel_flag.clone(),
                    )
                    .await;
            }

            if !should_ask
                && permissions.allows_tool_request(
                    spec.requires_tools_permission,
                    spec.requires_network_permission,
                )
            {
                return self
                    .invoke_tool_and_append_result(
                        session_id,
                        tool_use_id,
                        tool_name,
                        &effective_tool_input,
                        ToolInvocationPermissions::from_permission_context(&permissions, &spec),
                        tx,
                        cancel_flag.clone(),
                    )
                    .await;
            }

            if self
                .host
                .matches_permission_rule(tool_name, &effective_tool_input)
                .await
            {
                return self
                    .invoke_tool_and_append_result(
                        session_id,
                        tool_use_id,
                        tool_name,
                        &effective_tool_input,
                        ToolInvocationPermissions::after_explicit_allow(&permissions, &spec),
                        tx,
                        cancel_flag.clone(),
                    )
                    .await;
            }

            match self
                .host
                .resolve_tool_permission_request(
                    session_id,
                    tool_use_id,
                    tool_name,
                    &effective_tool_input,
                    &spec,
                    tx,
                    cancel_flag.clone(),
                )
                .await
            {
                ToolPermissionResolutionOutcome::Approved => {}
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

        self.invoke_tool_and_append_result(
            session_id,
            tool_use_id,
            tool_name,
            &effective_tool_input,
            ToolInvocationPermissions::after_explicit_allow(&permissions, &spec),
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
                    permissions.allow_network,
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
            self.host
                .live_tool_progress_reporter(session_id, tool_use_id, tool_name, tx),
            cancel_flag,
        );
        if tool_name.eq_ignore_ascii_case("Skill") {
            context.skill_definitions = Some(self.host.skill_definitions(session_id).await);
        }

        let _ask_forward_handle = self
            .attach_ask_user_channel(session_id, tool_use_id, &mut context, tx)
            .await;

        match self.tools.invoke(tool_name, tool_input, &context).await {
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
                    permissions.allow_network,
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

        let mut context = self.host.tool_context(
            session_id,
            permissions.allow_tools,
            permissions.allow_network,
            self.host
                .live_tool_progress_reporter(session_id, &tool_use_id, &tool_name, tx),
            cancel_flag,
        );
        if tool_name.eq_ignore_ascii_case("Skill") {
            context.skill_definitions = Some(self.host.skill_definitions(session_id).await);
        }
        let _ask_forward_handle = self
            .attach_ask_user_channel(session_id, &tool_use_id, &mut context, tx)
            .await;

        let result = match self.tools.invoke(&tool_name, &tool_input, &context).await {
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
    ) -> Option<tokio::task::JoinHandle<()>> {
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
            while let Some(req) = ask_rx.recv().await {
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
                            response_tx: req.response_tx,
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
                    interaction_runtime
                        .cancel_request(&request_id, AskUserCancellationReason::DeliveryFailed);
                    continue;
                }
                let timeout_runtime = interaction_runtime.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                    timeout_runtime.cancel_request(&request_id, AskUserCancellationReason::Timeout);
                });
            }
        }))
    }
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
