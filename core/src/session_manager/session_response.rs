use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, atomic::AtomicBool};

use orbcode_protocol::{
    MessageRole, StreamEvent, ToolUseCompletionKind, TranscriptBlock, TranscriptMessage,
    TurnCancellationKind,
};
use orbcode_session_store::assistant_message_has_visible_content;
use orbcode_tools::AgentToolInput;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::SessionManager;
use crate::{
    CoreError, ProviderFailure,
    agent_loop::{
        no_tool::{NoToolTurnDecision, NoToolTurnReason, decide_no_tool_turn_action},
        tool_round::{
            SequentialToolRoundOutcome, ToolRoundReadyItem, ToolRoundResponse, ToolRoundScheduler,
        },
    },
    hooks::matching_tool_hook_has_command,
    tool_flow::{
        StreamedToolUseExecution, ToolDenyPrecedenceStage, ToolInvocationPermissions,
        ToolUseOutcome,
    },
    tool_runtime::ToolRuntime,
    turn_loop::TurnLoopOutcome,
};

impl SessionManager {
    #[cfg(test)]
    pub(super) async fn finish_provider_response(
        &self,
        session_id: &str,
        turn_id: Uuid,
        prompt: &str,
        tool_round_response: ToolRoundResponse,
        assembled: String,
        auto_continue_attempts: usize,
        stop_hook_active: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<TurnLoopOutcome, CoreError> {
        self.finish_provider_response_with_streamed_tools(
            session_id,
            turn_id,
            prompt,
            tool_round_response,
            assembled,
            auto_continue_attempts,
            stop_hook_active,
            tx,
            cancel_flag,
            Vec::new(),
        )
        .await
    }

    pub(super) async fn finish_provider_response_with_streamed_tools(
        &self,
        session_id: &str,
        turn_id: Uuid,
        prompt: &str,
        tool_round_response: ToolRoundResponse,
        assembled: String,
        auto_continue_attempts: usize,
        stop_hook_active: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
        streamed_tool_executions: Vec<StreamedToolUseExecution>,
    ) -> Result<TurnLoopOutcome, CoreError> {
        let has_tool_use = tool_round_response
            .response
            .blocks
            .iter()
            .any(|block| matches!(block, TranscriptBlock::ToolUse { .. }));
        let assistant_message = if tool_round_response.response.blocks.is_empty() {
            TranscriptMessage::new(MessageRole::Assistant, assembled)
        } else {
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                tool_round_response.response.blocks.clone(),
            )
        }
        .with_stop_reason(
            tool_round_response
                .response
                .stop_reason
                .clone()
                .unwrap_or_else(|| "end_turn".to_string()),
        )
        .with_usage(tool_round_response.response.usage.clone());
        let assistant_message = self.with_message_cost_attribution(
            assistant_message,
            tool_round_response.response.provider,
        );
        if !has_tool_use && !assistant_message_has_visible_content(&assistant_message) {
            if self
                .last_message_is_successful_workflow_tool_result(session_id)
                .await?
            {
                let _ = tx.send(StreamEvent::TurnFinished {
                    session_id: session_id.to_string(),
                    provider: tool_round_response.response.provider,
                    fallback_from: tool_round_response.response.fallback_from,
                    usage: tool_round_response.response.usage.clone(),
                });
                return Ok(TurnLoopOutcome::Finished);
            }
            return Err(CoreError::ProviderFailed(ProviderFailure::from_message(
                "provider returned an empty assistant response",
            )));
        }
        let resolved_tool_round = tool_round_response.resolve_for_blocks(&assistant_message.blocks);
        let response = resolved_tool_round.response;
        let scheduler = resolved_tool_round.scheduler;

        if scheduler.is_empty() {
            let decision = decide_no_tool_turn_action(
                prompt,
                &assistant_message,
                response.stop_reason.as_deref(),
                auto_continue_attempts,
            );
            self.maybe_append_no_tool_turn_diagnostic(
                session_id,
                &assistant_message,
                response.stop_reason.as_deref(),
                auto_continue_attempts,
                decision,
            )
            .await?;
            if let NoToolTurnDecision::AutoContinue(reason) = decision {
                if matches!(reason, NoToolTurnReason::MaxOutput) {
                    self.append_message(session_id, assistant_message.clone())
                        .await?;
                    let _ = tx.send(StreamEvent::AssistantMessageCompleted {
                        message: assistant_message.clone(),
                        provider: response.provider,
                        fallback_from: response.fallback_from,
                        usage: response.usage.clone(),
                    });
                    self.provider_debug_trace
                        .append_message_activity(
                            self.config.default_provider,
                            "assistant_response_from_llm",
                            "assistant response",
                            &assistant_message,
                        )
                        .await;
                }
                return Ok(TurnLoopOutcome::AutoContinue(reason));
            }
            self.append_message(session_id, assistant_message.clone())
                .await?;
            let _ = tx.send(StreamEvent::AssistantMessageCompleted {
                message: assistant_message.clone(),
                provider: response.provider,
                fallback_from: response.fallback_from,
                usage: response.usage.clone(),
            });
            self.provider_debug_trace
                .append_message_activity(
                    self.config.default_provider,
                    "assistant_response_from_llm",
                    "assistant response",
                    &assistant_message,
                )
                .await;
            let stop_hook_outcome = self
                .run_stop_hooks(session_id, &assistant_message.content, stop_hook_active, tx)
                .await;
            if stop_hook_outcome.prevent_continuation {
                self.emit_hook_notice(
                    session_id,
                    "Stop",
                    stop_hook_outcome
                        .stop_reason
                        .as_deref()
                        .unwrap_or("Stop hook prevented continuation"),
                    false,
                    tx,
                )
                .await;
                let _ = tx.send(StreamEvent::TurnFinished {
                    session_id: session_id.to_string(),
                    provider: response.provider,
                    fallback_from: response.fallback_from,
                    usage: response.usage.clone(),
                });
                return Ok(TurnLoopOutcome::Finished);
            }
            if !stop_hook_outcome.blocking_errors.is_empty() {
                self.append_stop_hook_feedback(session_id, stop_hook_outcome.blocking_errors, tx)
                    .await?;
                return Ok(TurnLoopOutcome::StopHookContinue);
            }
            if self.has_queued_user_commands(session_id).await {
                return Ok(TurnLoopOutcome::Continue);
            }
            let _ = tx.send(StreamEvent::TurnFinished {
                session_id: session_id.to_string(),
                provider: response.provider,
                fallback_from: response.fallback_from,
                usage: response.usage.clone(),
            });
            return Ok(TurnLoopOutcome::Finished);
        }

        self.append_message(session_id, assistant_message.clone())
            .await?;
        let _ = tx.send(StreamEvent::AssistantMessageCompleted {
            message: assistant_message.clone(),
            provider: response.provider,
            fallback_from: response.fallback_from,
            usage: response.usage.clone(),
        });
        self.provider_debug_trace
            .append_message_activity(
                self.config.default_provider,
                "assistant_response_from_llm",
                "assistant response",
                &assistant_message,
            )
            .await;

        match self
            .execute_tool_round(
                session_id,
                scheduler,
                streamed_tool_executions,
                tx,
                cancel_flag,
            )
            .await?
        {
            SequentialToolRoundOutcome::Continue => {}
            SequentialToolRoundOutcome::Denied {
                remaining_tool_uses,
            } => {
                // Answer the remaining (un-run) parallel tool_uses with
                // interrupted results so every tool_use in the round has a
                // matching result in-turn (the denied tool already has its
                // denial result). Otherwise the next request would 400 until the
                // chain is repaired on the next load — the `Cancelled` branch
                // already does this.
                self.append_interrupted_tool_results(session_id, &remaining_tool_uses, tx)
                    .await?;
                self.flush_tool_result_context_queue(session_id, tx).await?;
                let _ = tx.send(StreamEvent::TurnFinished {
                    session_id: session_id.to_string(),
                    provider: response.provider,
                    fallback_from: response.fallback_from,
                    usage: response.usage.clone(),
                });
                return Ok(TurnLoopOutcome::Finished);
            }
            SequentialToolRoundOutcome::Cancelled {
                remaining_tool_uses,
            } => {
                self.append_interrupted_tool_results(session_id, &remaining_tool_uses, tx)
                    .await?;
                self.flush_tool_result_context_queue(session_id, tx).await?;
                if self.active_turns.is_active(session_id, turn_id).await {
                    self.append_interruption_message(session_id, true, tx)
                        .await?;
                }
                let _ = tx.send(StreamEvent::TurnCancelled {
                    session_id: session_id.to_string(),
                    kind: TurnCancellationKind::ToolStage,
                    partial: None,
                    usage: Some(response.usage),
                });
                return Ok(TurnLoopOutcome::Cancelled);
            }
        }

        Ok(TurnLoopOutcome::Continue)
    }

    #[cfg(test)]
    pub(super) async fn execute_sequential_tool_round(
        &self,
        session_id: &str,
        scheduler: ToolRoundScheduler,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<SequentialToolRoundOutcome, CoreError> {
        self.execute_tool_round(session_id, scheduler, Vec::new(), tx, cancel_flag)
            .await
    }

    async fn execute_tool_round(
        &self,
        session_id: &str,
        mut scheduler: ToolRoundScheduler,
        streamed_tool_executions: Vec<StreamedToolUseExecution>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<SequentialToolRoundOutcome, CoreError> {
        self.begin_tool_result_context_queue(session_id).await;
        let mut streamed_tool_executions = streamed_tool_executions
            .into_iter()
            .map(|execution| (execution.tool_use_id.clone(), execution))
            .collect::<HashMap<_, _>>();

        loop {
            let mut ready_items = VecDeque::new();
            while let Some(item) = scheduler.next_ready() {
                let tool_use_id = item.tool_use_id().to_string();
                if streamed_tool_executions.contains_key(&tool_use_id) {
                    ready_items.push_back(item);
                    continue;
                }
                if let Some(execution) = self
                    .start_streamed_tool_execution(
                        session_id,
                        item.clone(),
                        tx,
                        cancel_flag.clone(),
                    )
                    .await
                {
                    streamed_tool_executions.insert(tool_use_id, execution);
                    ready_items.push_back(item);
                    continue;
                }
                ready_items.push_back(item);
                break;
            }
            if ready_items.is_empty() {
                break;
            }

            while let Some(item) = ready_items.pop_front() {
                let outcome =
                    if let Some(execution) = streamed_tool_executions.remove(item.tool_use_id()) {
                        match self
                            .finish_streamed_tool_execution(session_id, execution, tx)
                            .await
                        {
                            Ok(outcome) => outcome,
                            Err(error) => {
                                self.discard_tool_result_context_queue(session_id).await;
                                self.interrupt_streamed_tool_executions(
                                    session_id,
                                    streamed_tool_executions.into_values(),
                                    tx,
                                );
                                return Err(error);
                            }
                        }
                    } else {
                        match self
                            .execute_tool_use_in_active_context_queue(
                                session_id,
                                item.tool_use_id(),
                                item.tool_name(),
                                item.tool_input(),
                                tx,
                                cancel_flag.clone(),
                            )
                            .await
                        {
                            Ok(outcome) => outcome,
                            Err(error) => {
                                self.discard_tool_result_context_queue(session_id).await;
                                self.interrupt_streamed_tool_executions(
                                    session_id,
                                    streamed_tool_executions.into_values(),
                                    tx,
                                );
                                return Err(error);
                            }
                        }
                    };
                let terminal_outcome =
                    scheduler.record_execution_outcome(item, outcome.into_tool_round_outcome());
                if let Some(outcome) = terminal_outcome {
                    self.interrupt_streamed_tool_executions(
                        session_id,
                        streamed_tool_executions.into_values(),
                        tx,
                    );
                    if matches!(outcome, SequentialToolRoundOutcome::Cancelled { .. }) {
                        return Ok(outcome);
                    }
                    self.flush_tool_result_context_queue(session_id, tx).await?;
                    return Ok(outcome);
                }
            }
        }

        self.interrupt_streamed_tool_executions(
            session_id,
            streamed_tool_executions.into_values(),
            tx,
        );
        self.flush_tool_result_context_queue(session_id, tx).await?;
        self.flush_queued_user_commands(session_id, tx).await?;
        Ok(SequentialToolRoundOutcome::Continue)
    }

    pub(super) fn interrupt_streamed_tool_executions<I>(
        &self,
        session_id: &str,
        streamed_tool_executions: I,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) where
        I: IntoIterator<Item = StreamedToolUseExecution>,
    {
        for execution in streamed_tool_executions {
            self.emit_tool_use_completed(
                session_id,
                &execution.tool_use_id,
                &execution.tool_name,
                ToolUseCompletionKind::Interrupted,
                tx,
            );
        }
    }

    async fn finish_streamed_tool_execution(
        &self,
        session_id: &str,
        execution: StreamedToolUseExecution,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolUseOutcome, CoreError> {
        let completion = execution.finish().await?;
        let result = completion.result;
        self.append_tool_result_message(
            session_id,
            &result.tool_use_id,
            result.content,
            result.is_error,
            result.metadata,
            tx,
        )
        .await?;
        self.emit_tool_use_completed(
            session_id,
            &result.tool_use_id,
            &result.tool_name,
            result.completion_kind,
            tx,
        );
        Ok(completion.outcome)
    }

    pub(super) async fn start_streamed_tool_execution(
        &self,
        session_id: &str,
        ready_item: ToolRoundReadyItem,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Option<StreamedToolUseExecution> {
        let permissions = self
            .streamed_tool_invocation_permissions(&ready_item)
            .await?;
        let tool_use_id = ready_item.tool_use_id().to_string();
        let tool_name = ready_item.tool_name().to_string();
        let tool_input = ready_item.tool_input().to_string();
        self.emit_tool_use_started(session_id, &tool_use_id, &tool_name, &tool_input, tx);
        let manager = self.clone();
        let session_id = session_id.to_string();
        let tx = tx.clone();
        let handle = tokio::spawn(async move {
            ToolRuntime::new(&manager.tools, &manager)
                .execute_streamed_tool_use(&session_id, ready_item, permissions, &tx, cancel_flag)
                .await
        });
        Some(StreamedToolUseExecution::new(
            tool_use_id,
            tool_name,
            handle,
        ))
    }

    async fn streamed_tool_invocation_permissions(
        &self,
        ready_item: &ToolRoundReadyItem,
    ) -> Option<ToolInvocationPermissions> {
        let tool_name = ready_item.tool_name();
        let tool_input = ready_item.tool_input();
        if self.has_matching_tool_lifecycle_hooks(tool_name, tool_input) {
            return None;
        }
        if tool_name.eq_ignore_ascii_case("Agent") && agent_tool_runs_in_background(tool_input) {
            return None;
        }

        let spec = self.tools.spec(tool_name).cloned()?;
        let permissions = self.permission_context();
        if self
            .tool_deny_precedence_reason(
                &permissions,
                tool_name,
                tool_input,
                ToolDenyPrecedenceStage::OriginalInput,
            )
            .await
            .is_some()
        {
            return None;
        }

        if spec.requires_tools_permission || spec.requires_network_permission {
            // An `ask` rule forces an interactive prompt (deny > ask > allow),
            // suppressing the config-allow and blanket auto-approve fast paths.
            // An explicit in-session grant (a remembered runtime rule) still
            // wins, so the user is not re-prompted for something they already
            // chose to always allow this session.
            let should_ask = permissions.tool_should_ask(tool_name, tool_input);
            if !should_ask && permissions.tool_allowed_without_prompt(tool_name, tool_input) {
                return Some(ToolInvocationPermissions::after_explicit_allow(
                    &permissions,
                    &spec,
                ));
            }
            if !should_ask
                && permissions.allows_tool_request(
                    spec.requires_tools_permission,
                    spec.requires_network_permission,
                )
            {
                return Some(ToolInvocationPermissions::from_permission_context(
                    &permissions,
                    &spec,
                ));
            }
            if self
                .permission_runtime
                .matches_permission_rule(tool_name, tool_input)
                .await
            {
                return Some(ToolInvocationPermissions::after_explicit_allow(
                    &permissions,
                    &spec,
                ));
            }
            return None;
        }

        Some(ToolInvocationPermissions::after_explicit_allow(
            &permissions,
            &spec,
        ))
    }

    fn has_matching_tool_lifecycle_hooks(&self, tool_name: &str, tool_input: &str) -> bool {
        ["PreToolUse", "PostToolUse", "PostToolUseFailure"]
            .into_iter()
            .any(|hook_event| self.has_matching_tool_hooks(hook_event, tool_name, tool_input))
    }

    fn has_matching_tool_hooks(&self, hook_event: &str, tool_name: &str, tool_input: &str) -> bool {
        let matchers = self.config.hooks_for_event(hook_event);
        matchers
            .iter()
            .any(|matcher| matching_tool_hook_has_command(matcher, tool_name, tool_input))
    }

    async fn last_message_is_successful_workflow_tool_result(
        &self,
        session_id: &str,
    ) -> Result<bool, CoreError> {
        let Some(session) = self
            .transcript_store
            .load_session_if_present(session_id)
            .await?
        else {
            return Ok(false);
        };
        let Some(message) = session.messages.last() else {
            return Ok(false);
        };
        if message.role != MessageRole::User {
            return Ok(false);
        }

        Ok(message.blocks.iter().any(|block| {
            let TranscriptBlock::ToolResult {
                content, is_error, ..
            } = block
            else {
                return false;
            };
            if *is_error {
                return false;
            }
            serde_json::from_str::<serde_json::Value>(content)
                .ok()
                .and_then(|value| {
                    value
                        .get("task_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .is_some_and(|task_id| task_id.starts_with("workflow-"))
        }))
    }
}

fn agent_tool_runs_in_background(tool_input: &str) -> bool {
    serde_json::from_str::<AgentToolInput>(tool_input).map_or(true, |agent| agent.run_in_background)
}
