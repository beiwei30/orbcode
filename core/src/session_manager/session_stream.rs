use std::sync::{Arc, atomic::AtomicBool};

use async_trait::async_trait;
use orbcode_model_provider::{
    AttemptDiscardDisposition, ProviderContentBlockDelta, ProviderContentBlockStart, ProviderError,
    ProviderStreamEvent, ProviderStreamSink,
};
use orbcode_protocol::{ProviderId, StreamEvent, TranscriptBlock, TranscriptMessage};
use orbcode_session_store::{agent_tool_use_progress_record, attach_agent_id};
use orbcode_tools::{ToolError, ToolProgressReporter};
// Plugin boundary: tool progress records are arbitrary JSON defined by individual tools.
use serde_json::Value;
use tokio::sync::mpsc;

use super::SessionManager;
use crate::{
    agent_loop::tool_round::{ToolRoundStreamCollector, ToolRoundStreamUpdate},
    agent_tool::agent_provider_response_message,
    tool_flow::{SessionProviderStreamResult, StreamedToolUseExecution},
};

pub(super) struct SessionProviderStreamSink<'a> {
    manager: SessionManager,
    session_id: &'a str,
    tx: &'a mpsc::UnboundedSender<StreamEvent>,
    cancel_flag: Arc<AtomicBool>,
    tool_round_stream: ToolRoundStreamCollector,
    streamed_tool_executions: Vec<StreamedToolUseExecution>,
    assistant_started: bool,
}

impl<'a> SessionProviderStreamSink<'a> {
    pub(super) fn new(
        manager: SessionManager,
        session_id: &'a str,
        provider: ProviderId,
        tx: &'a mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            manager,
            session_id,
            tx,
            cancel_flag,
            tool_round_stream: ToolRoundStreamCollector::new(provider, None),
            streamed_tool_executions: Vec::new(),
            assistant_started: false,
        }
    }

    fn ensure_assistant_started(&mut self) {
        if self.assistant_started {
            return;
        }
        let response = self.tool_round_stream.response_snapshot();
        let _ = self.tx.send(StreamEvent::AssistantMessageStarted {
            session_id: self.session_id.to_string(),
            provider: response.provider,
            fallback_from: response.fallback_from,
        });
        self.assistant_started = true;
    }

    pub(super) fn into_session_provider_stream_result(self) -> SessionProviderStreamResult {
        SessionProviderStreamResult {
            tool_round_stream: self.tool_round_stream.into_result(),
            streamed_tool_executions: self.streamed_tool_executions,
        }
    }

    async fn handle_tool_round_stream_update(&mut self, update: ToolRoundStreamUpdate) {
        let Some(ready_item) = update.into_ready_item() else {
            return;
        };
        if let Some(execution) = self
            .manager
            .start_streamed_tool_execution(
                self.session_id,
                ready_item,
                self.tx,
                self.cancel_flag.clone(),
            )
            .await
        {
            self.streamed_tool_executions.push(execution);
        }
    }
}

#[async_trait]
impl ProviderStreamSink for SessionProviderStreamSink<'_> {
    async fn emit(
        &mut self,
        event: ProviderStreamEvent,
    ) -> Result<(), orbcode_model_provider::ProviderError> {
        let update = self.tool_round_stream.apply(&event);
        self.handle_tool_round_stream_update(update).await;
        match event {
            ProviderStreamEvent::MessageStart { .. } => {}
            ProviderStreamEvent::ContentBlockStart { block, .. } => match block {
                ProviderContentBlockStart::Text { text } => {
                    self.ensure_assistant_started();
                    if !text.is_empty() {
                        let _ = self.tx.send(StreamEvent::AssistantDelta {
                            session_id: self.session_id.to_string(),
                            delta: text,
                        });
                    }
                }
                ProviderContentBlockStart::Thinking { text, .. } => {
                    self.ensure_assistant_started();
                    let response = self.tool_round_stream.response_snapshot();
                    let _ = self.tx.send(StreamEvent::ThinkingStarted {
                        session_id: self.session_id.to_string(),
                        provider: response.provider,
                    });
                    if !text.is_empty() {
                        let _ = self.tx.send(StreamEvent::ThinkingDelta {
                            session_id: self.session_id.to_string(),
                            delta: text,
                        });
                    }
                }
                ProviderContentBlockStart::ToolUse { .. } => {
                    self.ensure_assistant_started();
                }
            },
            ProviderStreamEvent::ContentBlockDelta { delta, .. } => match delta {
                ProviderContentBlockDelta::Text(delta) => {
                    self.ensure_assistant_started();
                    let _ = self.tx.send(StreamEvent::AssistantDelta {
                        session_id: self.session_id.to_string(),
                        delta,
                    });
                }
                ProviderContentBlockDelta::Thinking(delta) => {
                    self.ensure_assistant_started();
                    let _ = self.tx.send(StreamEvent::ThinkingDelta {
                        session_id: self.session_id.to_string(),
                        delta,
                    });
                }
                ProviderContentBlockDelta::Signature(_)
                | ProviderContentBlockDelta::InputJson(_) => {}
            },
            ProviderStreamEvent::ContentBlockStop { index } => {
                if matches!(
                    self.tool_round_stream.block(index),
                    Some(TranscriptBlock::Thinking { .. })
                ) {
                    let response = self.tool_round_stream.response_snapshot();
                    let _ = self.tx.send(StreamEvent::ThinkingCompleted {
                        session_id: self.session_id.to_string(),
                        provider: response.provider,
                    });
                }
            }
            ProviderStreamEvent::MessageDelta { .. } | ProviderStreamEvent::MessageStop => {}
        }
        Ok(())
    }

    async fn discard_attempt(
        &mut self,
        provider: ProviderId,
        fallback_provider: ProviderId,
        reason: &str,
    ) -> Result<AttemptDiscardDisposition, orbcode_model_provider::ProviderError> {
        // Preserve partial usage from the discarded attempt so the budget
        // tracks actual provider spend even when the response is thrown away.
        let discarded_response = self.tool_round_stream.response_snapshot();
        if discarded_response.usage.total_tokens > 0 {
            let cost_message = TranscriptMessage::new(orbcode_protocol::MessageRole::Assistant, "")
                .with_usage(discarded_response.usage)
                .with_synthetic(true);
            self.manager
                .accumulate_live_cost(self.session_id, &cost_message)
                .await;
        }

        let streamed_tool_executions = std::mem::take(&mut self.streamed_tool_executions);
        let disposition = if streamed_tool_executions.is_empty() {
            AttemptDiscardDisposition::SafeToFallback
        } else {
            AttemptDiscardDisposition::ToolExecutionStarted
        };
        self.manager
            .interrupt_streamed_tool_executions(self.session_id, streamed_tool_executions, self.tx)
            .await;
        let _ = self.tx.send(StreamEvent::AssistantMessageDiscarded {
            session_id: self.session_id.to_string(),
            provider,
            fallback_provider,
            reason: reason.to_string(),
        });
        self.tool_round_stream = ToolRoundStreamCollector::new(fallback_provider, Some(provider));
        self.assistant_started = false;
        Ok(disposition)
    }
}

#[derive(Clone)]
pub(super) struct LiveToolProgressReporter {
    pub(super) manager: SessionManager,
    pub(super) session_id: String,
    pub(super) tool_use_id: String,
    pub(super) tool_name: String,
    pub(super) tx: mpsc::UnboundedSender<StreamEvent>,
}

#[async_trait]
impl ToolProgressReporter for LiveToolProgressReporter {
    async fn report(&self, progress: Value) -> Result<(), ToolError> {
        self.manager
            .append_tool_progress_event(
                &self.session_id,
                &self.tool_use_id,
                &self.tool_name,
                progress,
                &self.tx,
            )
            .await
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))
    }
}

#[derive(Clone)]
pub(super) struct NestedAgentToolProgressReporter {
    pub(super) manager: SessionManager,
    pub(super) session_id: String,
    pub(super) parent_tool_use_id: String,
    pub(super) agent_id: String,
    pub(super) tx: mpsc::UnboundedSender<StreamEvent>,
}

#[async_trait]
impl ToolProgressReporter for NestedAgentToolProgressReporter {
    async fn report(&self, progress: Value) -> Result<(), ToolError> {
        self.manager
            .append_tool_progress_event(
                &self.session_id,
                &self.parent_tool_use_id,
                "Agent",
                attach_agent_id(progress, &self.agent_id),
                &self.tx,
            )
            .await
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))
    }
}

pub(super) struct AgentProviderStreamSink<'a> {
    manager: &'a SessionManager,
    session_id: &'a str,
    parent_tool_use_id: &'a str,
    agent_id: &'a str,
    tx: &'a mpsc::UnboundedSender<StreamEvent>,
    tool_round_stream: ToolRoundStreamCollector,
}

impl<'a> AgentProviderStreamSink<'a> {
    pub(super) fn new(
        manager: &'a SessionManager,
        session_id: &'a str,
        parent_tool_use_id: &'a str,
        agent_id: &'a str,
        provider: ProviderId,
        tx: &'a mpsc::UnboundedSender<StreamEvent>,
    ) -> Self {
        Self {
            manager,
            session_id,
            parent_tool_use_id,
            agent_id,
            tx,
            tool_round_stream: ToolRoundStreamCollector::new(provider, None),
        }
    }

    pub(super) fn into_message(self) -> TranscriptMessage {
        agent_provider_response_message(self.tool_round_stream.into_response())
    }

    async fn emit_tool_use_progress(
        &self,
        id: &str,
        name: &str,
        input: &str,
    ) -> Result<(), ProviderError> {
        self.manager
            .append_tool_progress_event(
                self.session_id,
                self.parent_tool_use_id,
                "Agent",
                agent_tool_use_progress_record(self.agent_id, id, name, input),
                self.tx,
            )
            .await
            .map_err(|error| ProviderError::fatal(error.to_string()))
    }
}

#[async_trait]
impl ProviderStreamSink for AgentProviderStreamSink<'_> {
    async fn emit(&mut self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        if let Some(tool_use) = self.tool_round_stream.apply(&event).into_ready_item() {
            self.emit_tool_use_progress(
                tool_use.tool_use_id(),
                tool_use.tool_name(),
                tool_use.tool_input(),
            )
            .await?;
        }
        Ok(())
    }

    async fn discard_attempt(
        &mut self,
        provider: ProviderId,
        fallback_provider: ProviderId,
        _reason: &str,
    ) -> Result<AttemptDiscardDisposition, ProviderError> {
        self.tool_round_stream = ToolRoundStreamCollector::new(fallback_provider, Some(provider));
        Ok(AttemptDiscardDisposition::SafeToFallback)
    }
}
