use orbcode_protocol::{StreamEvent, ToolUseCompletionKind, TranscriptMessage};
use orbcode_session_store::{
    PERSISTED_OUTPUT_TAG, persisted_tool_result_preview_message, session_has_tool_result,
    tool_result_message, tool_result_persistence_threshold,
};
use tokio::sync::mpsc;

use super::{PERMISSION_DENIED_RETRY_MESSAGE, SessionManager};
use crate::{
    CoreError,
    agent_loop::{
        messages::{add_tool_round_summaries, repair_missing_tool_results},
        tool_round::ToolRoundToolUse,
    },
    tool_flow::{INTERRUPTED_TOOL_RESULT, ToolUseOutcome},
};

impl SessionManager {
    pub(super) async fn append_interrupted_tool_results(
        &self,
        session_id: &str,
        tool_uses: &[ToolRoundToolUse],
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        let existing = self.load_session(session_id).await?;
        for tool_use in tool_uses {
            if session_has_tool_result(&existing.messages, &tool_use.tool_use_id) {
                continue;
            }
            self.append_tool_result_message(
                session_id,
                &tool_use.tool_use_id,
                INTERRUPTED_TOOL_RESULT,
                true,
                None,
                tx,
            )
            .await?;
            self.emit_tool_use_completed(
                session_id,
                &tool_use.tool_use_id,
                &tool_use.tool_name,
                ToolUseCompletionKind::Interrupted,
                tx,
            );
        }

        Ok(())
    }

    pub(super) async fn append_tool_result_message(
        &self,
        session_id: &str,
        tool_use_id: &str,
        content: impl Into<String>,
        is_error: bool,
        metadata: Option<String>,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        let message = tool_result_message(tool_use_id, content, is_error, metadata);
        self.append_message(session_id, message.clone()).await?;
        self.provider_debug_trace
            .append_message_activity(
                self.config.default_provider,
                "tool_result_to_llm",
                "tool result",
                &message,
            )
            .await;
        let _ = tx.send(StreamEvent::UserMessage { message });
        Ok(())
    }

    pub(super) async fn deny_tool_use_with_result(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        message: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolUseOutcome, CoreError> {
        let retry = self
            .run_permission_denied_hooks(
                session_id,
                tool_use_id,
                tool_name,
                tool_input,
                message,
                tx,
            )
            .await;
        self.append_tool_result_message(session_id, tool_use_id, message, true, None, tx)
            .await?;
        if retry {
            self.append_model_visible_context_message(
                session_id,
                "hook_retry_context_to_llm",
                "PermissionDenied retry guidance",
                PERMISSION_DENIED_RETRY_MESSAGE.to_string(),
                tx,
            )
            .await?;
        }
        self.emit_tool_use_completed(
            session_id,
            tool_use_id,
            tool_name,
            ToolUseCompletionKind::PermissionDenied,
            tx,
        );
        Ok(ToolUseOutcome::Denied)
    }

    pub(super) async fn maybe_persist_large_tool_result(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        content: String,
    ) -> Result<String, CoreError> {
        let Some(threshold) = tool_result_persistence_threshold(tool_name) else {
            return Ok(content);
        };
        if content.starts_with(PERSISTED_OUTPUT_TAG) || content.chars().count() <= threshold {
            return Ok(content);
        }

        self.persist_tool_result_preview(session_id, tool_use_id, content)
            .await
    }

    async fn persist_tool_result_preview(
        &self,
        session_id: &str,
        tool_use_id: &str,
        content: String,
    ) -> Result<String, CoreError> {
        let path_display = self
            .transcript_store
            .persist_tool_result(session_id, tool_use_id, &content)
            .await?;

        Ok(persisted_tool_result_preview_message(
            &content,
            &path_display,
        ))
    }

    pub(super) async fn model_visible_messages_with_tool_result_budget(
        &self,
        session_id: &str,
        mut messages: Vec<TranscriptMessage>,
    ) -> Result<Vec<TranscriptMessage>, CoreError> {
        messages.retain(|message| {
            !(message.is_synthetic
                && message.usage.is_some()
                && message.content.is_empty()
                && message.blocks.is_empty())
        });
        let mut messages = add_tool_round_summaries(repair_missing_tool_results(messages));
        self.transcript_store
            .apply_tool_result_budget(session_id, &mut messages)
            .await?;
        Ok(messages)
    }
}
