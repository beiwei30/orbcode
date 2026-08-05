use orbcode_protocol::{
    MessageRole, SessionRecord, StreamEvent, ToolUseCompletionKind, TranscriptMessage,
};
use orbcode_session_store::{
    SessionWriteHints, acp_load_replay_blockers, normalize_tool_progress_record,
};
// Plugin boundary: tool progress records are arbitrary JSON defined by individual tools.
use serde_json::Value;
use tokio::sync::mpsc;

use super::{INTERRUPTED_TURN_MESSAGE, INTERRUPTED_TURN_MESSAGE_FOR_TOOL_USE, SessionManager};
use crate::{
    CoreError,
    agent_loop::no_tool::{NoToolTurnDecision, assistant_message_shape},
};

impl SessionManager {
    pub(super) async fn append_interruption_message(
        &self,
        session_id: &str,
        tool_use: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        let message = TranscriptMessage::new(
            MessageRole::User,
            if tool_use {
                INTERRUPTED_TURN_MESSAGE_FOR_TOOL_USE
            } else {
                INTERRUPTED_TURN_MESSAGE
            },
        );
        self.append_message(session_id, message.clone()).await?;
        self.provider_debug_trace
            .append_message_activity(
                self.config.default_provider,
                "interruption_to_llm",
                "interruption",
                &message,
            )
            .await;
        let _ = tx.send(StreamEvent::UserMessage { message });
        Ok(())
    }

    pub(super) fn emit_tool_use_started(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        tool_input: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) {
        let _ = tx.send(StreamEvent::ToolUseStarted {
            session_id: session_id.to_string(),
            tool_use_id: tool_use_id.to_string(),
            tool_name: tool_name.to_string(),
            tool_input: tool_input.to_string(),
        });
    }

    pub(super) fn emit_tool_use_completed(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        kind: ToolUseCompletionKind,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) {
        let _ = tx.send(StreamEvent::ToolUseCompleted {
            session_id: session_id.to_string(),
            tool_use_id: tool_use_id.to_string(),
            tool_name: tool_name.to_string(),
            kind,
        });
    }

    pub(super) async fn append_tool_progress_event(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        progress: Value,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        let progress = normalize_tool_progress_record(tool_use_id, progress);
        self.transcript_store
            .append_progress_for_latest_parent(session_id, progress.clone())
            .await?;
        let _ = tx.send(StreamEvent::ToolProgress {
            session_id: session_id.to_string(),
            tool_use_id: tool_use_id.to_string(),
            tool_name: tool_name.to_string(),
            progress,
        });
        Ok(())
    }

    /// Like [`Self::append_tool_progress_event`], but anchors the persisted
    /// progress line to the parent message that emitted `tool_use_id` rather
    /// than to whatever message is newest. Used by the detached background
    /// agent: its parent session keeps advancing while the agent runs, so
    /// "latest parent" would misattribute the progress to an unrelated later
    /// entry.
    pub(super) async fn append_background_tool_progress_event(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_name: &str,
        progress: Value,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), CoreError> {
        let progress = normalize_tool_progress_record(tool_use_id, progress);
        self.transcript_store
            .append_progress_for_tool_use_parent(session_id, tool_use_id, progress.clone())
            .await?;
        let _ = tx.send(StreamEvent::ToolProgress {
            session_id: session_id.to_string(),
            tool_use_id: tool_use_id.to_string(),
            tool_name: tool_name.to_string(),
            progress,
        });
        Ok(())
    }

    pub async fn load_session(&self, session_id: &str) -> Result<SessionRecord, CoreError> {
        self.transcript_store
            .load_session(session_id)
            .await
            .map_err(Into::into)
    }

    pub async fn acp_load_replay_preflight(
        &self,
        session_id: &str,
    ) -> Result<(SessionRecord, Vec<String>), CoreError> {
        let session = self
            .transcript_store
            .load_session_any_project(session_id)
            .await?;
        let contents = tokio::fs::read_to_string(self.transcript_store.path(session_id)).await?;
        let blockers = acp_load_replay_blockers(session_id, &contents);
        Ok((session, blockers))
    }

    pub async fn append_system_message(
        &self,
        session_id: &str,
        content: impl Into<String>,
    ) -> Result<TranscriptMessage, CoreError> {
        let message = TranscriptMessage::new(MessageRole::System, content);
        self.append_message(session_id, message.clone()).await?;
        Ok(message)
    }

    pub async fn fork_session(
        &self,
        session_id: &str,
        title: Option<String>,
        note: Option<String>,
    ) -> Result<SessionRecord, CoreError> {
        let source = self.load_session(session_id).await?;
        let config = self.effective_config();
        let context = self.context_preview().await;
        let mut fork = SessionRecord::new();
        fork.cwd = Some(config.cwd.display().to_string());
        fork.git_branch = context.git_branch.clone();
        fork.provider = Some(config.default_provider);
        fork.title = Some(title.unwrap_or_else(|| {
            source.title.clone().map_or_else(
                || format!("Fork of {}", session_id.chars().take(8).collect::<String>()),
                |title| format!("{title} (fork)"),
            )
        }));

        for message in source.messages {
            fork.push_message(TranscriptMessage::from_parts(
                message.role,
                message.content,
                message.blocks,
            ));
        }
        fork.push_message(TranscriptMessage::new(
            MessageRole::System,
            format!("Forked from session {session_id}."),
        ));
        if let Some(note) = note {
            fork.push_message(TranscriptMessage::new(MessageRole::System, note));
        }

        self.transcript_store
            .record_session_hints(
                &fork.session_id,
                SessionWriteHints {
                    git_branch: context.git_branch,
                    provider: Some(config.default_provider),
                },
            )
            .await;
        self.transcript_store.persist_full_session(&fork).await?;
        Ok(fork)
    }

    /// Rewind a session by discarding every transcript message at or after
    /// `keep_messages`, then rewriting the persisted transcript so the
    /// truncation survives resume. `keep_messages` is clamped to the current
    /// message count, so an out-of-range index is a no-op rather than an
    /// error. Returns the truncated record.
    pub async fn rewind_session(
        &self,
        session_id: &str,
        keep_messages: usize,
    ) -> Result<SessionRecord, CoreError> {
        let mut source = self.load_session(session_id).await?;
        let keep = keep_messages.min(source.messages.len());
        source.messages.truncate(keep);
        if let Some(last) = source.messages.last() {
            source.updated_at = last.created_at;
        }
        self.transcript_store.persist_full_session(&source).await?;
        // The discarded messages must stop counting toward `/cost`: drop the
        // cached live-cost tracker so it re-seeds from the truncated transcript.
        self.reset_live_cost(session_id).await;
        Ok(source)
    }

    /// Return (creating if needed) the per-session lock that serializes
    /// transcript appends. See [`SessionManager::transcript_append_locks`].
    async fn transcript_append_lock(
        &self,
        session_id: &str,
    ) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.transcript_append_locks.lock().await;
        locks.entry(session_id.to_string()).or_default().clone()
    }

    pub async fn append_message(
        &self,
        session_id: &str,
        message: TranscriptMessage,
    ) -> Result<(), CoreError> {
        let message =
            self.with_message_cost_attribution(message, self.effective_config().default_provider);
        // Serialize the read-parent + write per session so overlapping turn
        // drivers cannot both observe the same last message and fork the
        // `parent_uuid` chain (e.g. an interrupted turn still draining while the
        // next turn begins appending).
        let append_lock = self.transcript_append_lock(session_id).await;
        let _guard = append_lock.lock().await;
        let existing = self
            .transcript_store
            .load_session_if_present(session_id)
            .await?;
        let parent_uuid = existing
            .as_ref()
            .and_then(|session| session.messages.last().map(|message| message.id.clone()));
        self.transcript_store
            .append_message_line(session_id, &message, parent_uuid.as_deref())
            .await?;
        drop(_guard);
        self.accumulate_live_cost(session_id, &message).await;
        Ok(())
    }

    pub(super) async fn maybe_append_no_tool_turn_diagnostic(
        &self,
        session_id: &str,
        message: &TranscriptMessage,
        stop_reason: Option<&str>,
        auto_continue_attempts: usize,
        decision: NoToolTurnDecision,
    ) -> Result<(), CoreError> {
        let enabled = std::env::var("ORBCODE_DEBUG_AUTO_CONTINUE")
            .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"));
        if !enabled {
            return Ok(());
        }

        let shape = assistant_message_shape(message);
        let action = match decision {
            NoToolTurnDecision::AutoContinue(_) => "auto_continue",
            NoToolTurnDecision::Finish(_) => "finish",
        };
        let reason = match decision {
            NoToolTurnDecision::AutoContinue(reason) | NoToolTurnDecision::Finish(reason) => {
                format!("{reason:?}")
            }
        };
        let content = format!(
            "[debug:auto-continue] action={action} reason={reason} stop_reason={} attempts={} text_chars={} thinking_chars={} text_lines={} thinking_lines={} structured={} has_tool_blocks={}",
            stop_reason.unwrap_or("null"),
            auto_continue_attempts,
            shape.visible_text_chars,
            shape.thinking_chars,
            shape.visible_text_lines,
            shape.thinking_lines,
            shape.has_structured_formatting,
            shape.has_tool_blocks,
        );
        let existing = self
            .transcript_store
            .load_session_if_present(session_id)
            .await?;
        let parent_uuid = existing
            .as_ref()
            .and_then(|session| session.messages.last().map(|message| message.id.clone()));
        self.transcript_store
            .append_message_line(
                session_id,
                &TranscriptMessage::new(MessageRole::System, content),
                parent_uuid.as_deref(),
            )
            .await
            .map_err(Into::into)
    }

    pub(super) async fn maybe_append_provider_round_diagnostic(
        &self,
        session_id: &str,
        content: String,
    ) -> Result<(), CoreError> {
        let enabled = std::env::var("ORBCODE_DEBUG_PROVIDER_ROUNDS")
            .or_else(|_| std::env::var("ORBCODE_DEBUG_AUTO_CONTINUE"))
            .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"));
        if !enabled {
            return Ok(());
        }

        let existing = self
            .transcript_store
            .load_session_if_present(session_id)
            .await?;
        let parent_uuid = existing
            .as_ref()
            .and_then(|session| session.messages.last().map(|message| message.id.clone()));
        self.transcript_store
            .append_message_line(
                session_id,
                &TranscriptMessage::new(MessageRole::System, content),
                parent_uuid.as_deref(),
            )
            .await
            .map_err(Into::into)
    }
}
