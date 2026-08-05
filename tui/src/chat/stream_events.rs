use std::time::Instant;

use orbcode_app_server_client::AppClient;
use orbcode_config::calculate_token_warning_state;
use orbcode_protocol::{
    BudgetOutcome, MessageRole, StreamEvent, TokenUsage, TranscriptBlock, TranscriptMessage,
    visible_content_from_blocks,
};
use tokio::sync::mpsc;

use orbcode_protocol::ProgressEnvelope;
use serde_json::Value;

use crate::background_agent_panel::background_task_tool_changes_panel;
use crate::embedded_progress::{
    embedded_progress_message_to_transcript, hook_progress_event_name, tool_progress_status_line,
};
use crate::history_cell::hook_note::{hook_notice_transcript_content, parse_hook_transcript_note};
use crate::history_cell::local_note::{
    LOCAL_TURN_DURATION_PREFIX, local_context_compacted_message, local_error_message,
};
use crate::overlays::{OverlayState, PermissionOverlayState, overlay_persists_after_turn};
use crate::prompt_state::ActiveThinkingState;
use crate::render::request_status::{WAITING_COMPLETION_VERBS, WAITING_VERBS};
use crate::render::thinking::{
    message_contains_matching_thinking_block, message_has_non_thinking_block,
};
use crate::render_metrics::RenderEventCounts;
use crate::state::{RequestTokenDirection, TuiState};
use crate::task_panel::task_tool_changes_panel;
use crate::tool_cell::live_state::LiveToolActivity;

pub(crate) const INTERRUPTED_TOOL_RESULT: &str = "Interrupted by user";

pub(crate) fn detach_turn_event_stream(
    turn_events: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
) -> bool {
    turn_events.take().is_some()
}

pub(crate) fn handle_stream_event_batch(
    state: &mut TuiState,
    turn_events: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
    first_event: Option<StreamEvent>,
    event_counts: &mut RenderEventCounts,
    needs_redraw: &mut bool,
    redraw_reasons: &mut Vec<&'static str>,
) {
    let Some(first_event) = first_event else {
        *turn_events = None;
        mark_redraw(needs_redraw, redraw_reasons, "stream_closed");
        return;
    };

    if apply_stream_event_for_redraw(
        state,
        turn_events,
        first_event,
        event_counts,
        needs_redraw,
        redraw_reasons,
    ) {
        return;
    }

    while let Some(receiver) = turn_events.as_mut() {
        match receiver.try_recv() {
            Ok(event) => {
                if apply_stream_event_for_redraw(
                    state,
                    turn_events,
                    event,
                    event_counts,
                    needs_redraw,
                    redraw_reasons,
                ) {
                    break;
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                *turn_events = None;
                mark_redraw(needs_redraw, redraw_reasons, "stream_closed");
                break;
            }
        }
    }
}

fn apply_stream_event_for_redraw(
    state: &mut TuiState,
    turn_events: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
    event: StreamEvent,
    event_counts: &mut RenderEventCounts,
    needs_redraw: &mut bool,
    redraw_reasons: &mut Vec<&'static str>,
) -> bool {
    let stop_batch_after_event = matches!(event, StreamEvent::RequestStarted { .. });
    let needs_stream_redraw = stream_event_needs_redraw(state, &event);
    event_counts.stream_events += 1;
    if state.apply_stream_event(event) {
        *turn_events = None;
        mark_redraw(needs_redraw, redraw_reasons, "stream_finished");
        true
    } else {
        if needs_stream_redraw {
            mark_redraw(needs_redraw, redraw_reasons, "stream_event");
        }
        stop_batch_after_event
    }
}

fn stream_event_needs_redraw(state: &TuiState, event: &StreamEvent) -> bool {
    match event {
        StreamEvent::ToolProgress {
            tool_use_id,
            tool_name,
            progress,
            ..
        } => {
            let Some(activity) = state.find_live_tool_activity_by_tool_use_id(tool_use_id) else {
                return true;
            };
            let status_line = tool_progress_status_line(progress)
                .unwrap_or_else(|| format!("Running `{tool_name}`"));
            activity.status_line != status_line
                || !activity
                    .progress_messages
                    .last()
                    .is_some_and(|existing| existing == progress)
        }
        StreamEvent::HookProgress { progress, .. } => state.hook_progress_would_change(progress),
        _ => true,
    }
}

pub(crate) fn mark_redraw(
    needs_redraw: &mut bool,
    redraw_reasons: &mut Vec<&'static str>,
    reason: &'static str,
) {
    *needs_redraw = true;
    if !redraw_reasons.contains(&reason) {
        redraw_reasons.push(reason);
    }
}

fn usage_output_estimate_chars(usage: &TokenUsage) -> usize {
    (usage.output_tokens as usize).saturating_mul(4)
}

fn usage_total_tokens(usage: &TokenUsage) -> u64 {
    let total_tokens = usage.total_tokens.max(usage.component_total_tokens());
    u64::from(total_tokens)
}

fn embedded_assistant_progress_estimate_chars(progress: &Value) -> usize {
    let Some(data) = ProgressEnvelope::parse(progress) else {
        return 0;
    };
    if data.progress_type.as_deref() != Some("agent_progress") {
        return 0;
    }
    let Some(message) = embedded_progress_message_to_transcript(progress) else {
        return 0;
    };
    if !matches!(message.role, MessageRole::Assistant) {
        return 0;
    }
    transcript_message_estimate_chars(&message)
}

fn transcript_message_estimate_chars(message: &TranscriptMessage) -> usize {
    let content_chars = message.content.chars().count();
    let block_chars = message
        .blocks
        .iter()
        .map(transcript_block_estimate_chars)
        .sum();
    content_chars.max(block_chars)
}

fn transcript_block_estimate_chars(block: &TranscriptBlock) -> usize {
    match block {
        TranscriptBlock::Text { text } | TranscriptBlock::Thinking { text, .. } => {
            text.chars().count()
        }
        TranscriptBlock::ToolUse { name, input, .. } => {
            name.chars().count().saturating_add(input.chars().count())
        }
        TranscriptBlock::ToolResult { content, .. } => content.chars().count(),
        _ => 0,
    }
}

fn assistant_completion_text(message: &TranscriptMessage) -> Option<String> {
    if !matches!(message.role, MessageRole::Assistant) {
        return None;
    }
    let visible = if message.blocks.is_empty() {
        message.content.clone()
    } else {
        visible_content_from_blocks(&message.blocks)
    };
    (!visible.trim().is_empty()).then_some(visible)
}

fn assistant_stream_text_matches(streamed: &str, completed: &str) -> bool {
    streamed.trim_end() == completed.trim_end()
}

fn tool_result_metadata_total_tokens(message: &TranscriptMessage) -> u64 {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::ToolResult {
                metadata: Some(metadata),
                ..
            } => serde_json::from_str::<Value>(metadata).ok(),
            _ => None,
        })
        .filter_map(|metadata| metadata.get("totalTokens").and_then(Value::as_u64))
        .fold(0_u64, u64::saturating_add)
}

impl TuiState {
    pub(crate) fn hook_progress_for_message(&self, message: &TranscriptMessage) -> &[Value] {
        self.hook_progress_by_message_id
            .get(&message.id)
            .map_or(&[], Vec::as_slice)
    }

    fn attach_pending_hook_progress_to_message(&mut self, message: &TranscriptMessage) {
        let Some(note) = parse_hook_transcript_note(message) else {
            if matches!(message.role, MessageRole::User)
                && !message
                    .blocks
                    .iter()
                    .any(|block| matches!(block, TranscriptBlock::ToolResult { .. }))
            {
                self.pending_hook_progress.clear();
            }
            return;
        };

        let mut attached = Vec::new();
        self.pending_hook_progress.retain(|progress| {
            if hook_progress_event_name(progress).as_deref() == Some(note.event_name.as_str()) {
                attached.push(progress.clone());
                false
            } else {
                true
            }
        });
        if !attached.is_empty() {
            self.hook_progress_by_message_id
                .insert(message.id.clone(), attached);
        }
    }

    fn hook_progress_would_change(&self, progress: &Value) -> bool {
        !self
            .hook_progress_target(progress)
            .last()
            .is_some_and(|existing| existing == progress)
    }

    fn hook_progress_target(&self, progress: &Value) -> &[Value] {
        let Some(event_name) = hook_progress_event_name(progress) else {
            return &self.pending_hook_progress;
        };
        let Some(message) = self.messages.last() else {
            return &self.pending_hook_progress;
        };
        let Some(note) = parse_hook_transcript_note(message) else {
            return &self.pending_hook_progress;
        };
        if note.event_name != event_name {
            return &self.pending_hook_progress;
        }

        self.hook_progress_by_message_id
            .get(&message.id)
            .map_or(&[], Vec::as_slice)
    }

    fn push_hook_progress_if_changed(target: &mut Vec<Value>, progress: Value) -> bool {
        if target.last().is_some_and(|existing| existing == &progress) {
            return false;
        }
        target.push(progress);
        true
    }

    fn attach_hook_progress_to_latest_note_or_pending(&mut self, progress: Value) -> bool {
        let Some(event_name) = hook_progress_event_name(&progress) else {
            return Self::push_hook_progress_if_changed(&mut self.pending_hook_progress, progress);
        };
        let Some(message) = self.messages.last() else {
            return Self::push_hook_progress_if_changed(&mut self.pending_hook_progress, progress);
        };
        let Some(note) = parse_hook_transcript_note(message) else {
            return Self::push_hook_progress_if_changed(&mut self.pending_hook_progress, progress);
        };
        if note.event_name != event_name {
            return Self::push_hook_progress_if_changed(&mut self.pending_hook_progress, progress);
        }

        Self::push_hook_progress_if_changed(
            self.hook_progress_by_message_id
                .entry(message.id.clone())
                .or_default(),
            progress,
        )
    }

    fn push_local_turn_duration_note(&mut self, duration_ms: Option<u64>, total_tokens: u64) {
        let Some(duration_ms) = duration_ms else {
            return;
        };
        let note = format!(
            "{LOCAL_TURN_DURATION_PREFIX}{}:{duration_ms}:{total_tokens}",
            self.spinner_verb_index % WAITING_COMPLETION_VERBS.len()
        );
        self.push_local_system_message(note);
    }

    pub(crate) fn begin_waiting_animation(&mut self) {
        self.request_in_flight = true;
        self.reset_pending_assistant_stream();
        self.active_thinking = None;
        self.spinner_frame = 0;
        self.spinner_verb_index = self.request_count % WAITING_VERBS.len();
        self.request_count = self.request_count.saturating_add(1);
        self.request_started_at = Some(Instant::now());
        self.request_token_direction = RequestTokenDirection::Up;
        self.last_usage = None;
    }

    pub(crate) fn stop_waiting_animation(&mut self) {
        self.request_in_flight = false;
        self.spinner_frame = 0;
        self.streamed_response_chars = 0;
        self.request_token_direction = RequestTokenDirection::Up;
        self.finish_stream_reflow_if_needed();
    }

    pub(crate) async fn interrupt_active_turn(
        &mut self,
        app_server: &AppClient,
        turn_events: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
    ) {
        let had_local_stream = detach_turn_event_stream(turn_events);
        let interrupted = app_server
            .interrupt_turn(&self.session_id)
            .await
            .unwrap_or(false);
        self.force_interrupt_active_turn();
        self.set_status_line(if interrupted || had_local_stream {
            "Turn interrupted."
        } else {
            "No active turn to interrupt."
        });
    }

    pub(crate) fn force_interrupt_active_turn(&mut self) {
        self.commit_interrupted_pending_assistant();
        self.commit_interrupted_live_tool_results();
        self.active_thinking = None;
        self.clear_live_tool_activities();
        self.in_progress_tool_use_ids.clear();
        self.overlay = None;
        self.stop_waiting_animation();
        self.request_started_at = None;
    }

    fn commit_interrupted_pending_assistant(&mut self) {
        let partial = std::mem::take(&mut self.pending_assistant);
        self.reset_pending_assistant_stream();
        if !partial.trim().is_empty() {
            self.push_message_and_flush_history(TranscriptMessage::new(
                MessageRole::Assistant,
                partial,
            ));
        }
    }

    fn commit_interrupted_live_tool_results(&mut self) {
        let tool_results = self
            .live_tool_activities()
            .into_iter()
            .filter(|activity| {
                !activity.tool_use_id.is_empty()
                    && self.transcript_has_unresolved_tool_use(&activity.tool_use_id)
            })
            .map(|activity| TranscriptBlock::ToolResult {
                tool_use_id: activity.tool_use_id.clone(),
                content: INTERRUPTED_TOOL_RESULT.into(),
                is_error: true,
                metadata: None,
            })
            .collect::<Vec<_>>();

        if !tool_results.is_empty() {
            self.push_message_and_flush_history(TranscriptMessage::from_blocks(
                MessageRole::User,
                tool_results,
            ));
        }
    }

    fn token_warning_status_line(&self, usage: &TokenUsage) -> Option<String> {
        let token_usage = usage.component_total_tokens();
        if token_usage == 0 {
            return None;
        }
        let warning_state = calculate_token_warning_state(
            token_usage,
            &self.model_display_name,
            &self.context_window_options,
            &self.max_output_token_options,
            &self.token_warning_options,
        );
        let label = if warning_state.is_at_blocking_limit {
            "Context limit reached"
        } else if warning_state.is_above_auto_compact_threshold {
            "Auto-compact recommended"
        } else if warning_state.is_above_error_threshold {
            "Context critical"
        } else if warning_state.is_above_warning_threshold {
            "Context warning"
        } else {
            return None;
        };
        Some(format!(
            "{label}: {token_usage} tokens, {}% left; run /compact.",
            warning_state.percent_left
        ))
    }

    pub(crate) fn finalize_deferred_assistant_message(
        &mut self,
        transcript_width: usize,
        terminal_height: u16,
    ) -> bool {
        let _ = (transcript_width, terminal_height);
        self.commit_deferred_assistant_message()
    }

    pub(crate) fn commit_deferred_assistant_message(&mut self) -> bool {
        if !self.pending_assistant.is_empty() {
            return false;
        }

        let Some(deferred) = self.deferred_assistant_message.take() else {
            return false;
        };

        self.clear_latest_message_focus();
        self.messages.push(deferred.message);
        self.pending_history_flush = true;
        self.focus_latest_message_start = false;
        true
    }

    pub(crate) fn reset_pending_assistant_stream(&mut self) {
        if self.assistant_stream_history_started() {
            self.reset_assistant_stream_history_for_reflow();
        } else {
            self.reset_assistant_stream_history();
        }
        self.pending_assistant.clear();
        self.deferred_assistant_message = None;
    }

    /// Whether a completed (non-streaming, non-empty) thinking block is sitting
    /// in `active_thinking` waiting to be materialized into scrollback. At turn
    /// completion this thinking is pushed to history *before* the final answer
    /// (see `AssistantMessageCompleted`), so the assistant answer must not commit
    /// any of its own lines to scrollback while this is true — otherwise an
    /// incrementally-committed assistant prefix would land ahead of the thinking,
    /// producing the physical order `assistant prefix -> thinking -> answer tail`
    /// (High #3).
    pub(crate) fn has_pending_completed_thinking(&self) -> bool {
        self.active_thinking.as_ref().is_some_and(|thinking| {
            !thinking.is_streaming
                && thinking.completed_at.is_some()
                && !thinking.text.trim().is_empty()
        })
    }

    fn take_completed_thinking_message(&mut self) -> Option<TranscriptMessage> {
        if !self.has_pending_completed_thinking() {
            return None;
        }
        let thinking = self.active_thinking.as_ref()?;

        Some(TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::Thinking {
                text: thinking.text.clone(),
                signature: None,
            }],
        ))
    }

    fn remove_duplicate_completed_thinking(
        thinking_message: &TranscriptMessage,
        mut message: TranscriptMessage,
    ) -> TranscriptMessage {
        if !message_has_non_thinking_block(&message)
            || !message_contains_matching_thinking_block(&message, thinking_message)
        {
            return message;
        }

        message
            .blocks
            .retain(|block| !matches!(block, TranscriptBlock::Thinking { .. }));
        message
    }

    pub(crate) fn apply_stream_event(&mut self, event: StreamEvent) -> bool {
        match event {
            StreamEvent::SessionStarted { .. } | StreamEvent::SessionLoaded { .. } => false,
            StreamEvent::RequestStarted { .. } => {
                let was_request_in_flight = self.request_in_flight;
                if !was_request_in_flight {
                    self.streamed_response_chars = 0;
                    self.current_turn_total_tokens = 0;
                    self.clear_live_tool_activities();
                }
                self.begin_waiting_animation();
                false
            }
            StreamEvent::UserMessage { message } => {
                if message.role == MessageRole::User {
                    self.remove_committed_steered_followup(&message.content);
                }
                let message = self.enrich_tool_result_message_with_live_progress(message);
                let metadata_total_tokens = tool_result_metadata_total_tokens(&message);
                self.current_turn_total_tokens = self
                    .current_turn_total_tokens
                    .saturating_add(metadata_total_tokens);
                self.attach_pending_hook_progress_to_message(&message);
                let completed_tool_use_ids = message
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        TranscriptBlock::ToolResult { tool_use_id, .. } => {
                            Some(tool_use_id.clone())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                for tool_use_id in completed_tool_use_ids {
                    self.in_progress_tool_use_ids.remove(&tool_use_id);
                }
                self.push_message_and_flush_history(message);
                false
            }
            StreamEvent::AssistantMessageStarted { provider, .. } => {
                self.last_provider = Some(provider);
                self.reset_pending_assistant_stream();
                self.request_in_flight = true;
                self.request_token_direction = RequestTokenDirection::Down;
                false
            }
            StreamEvent::ThinkingStarted { provider, .. } => {
                self.active_thinking = Some(ActiveThinkingState {
                    text: String::new(),
                    is_streaming: true,
                    completed_at: None,
                });
                self.last_provider = Some(provider);
                self.request_token_direction = RequestTokenDirection::Down;
                false
            }
            StreamEvent::ThinkingDelta { delta, .. } => {
                let delta_chars = delta.chars().count();
                let thinking = self.active_thinking.get_or_insert(ActiveThinkingState {
                    text: String::new(),
                    is_streaming: true,
                    completed_at: None,
                });
                self.streamed_response_chars =
                    self.streamed_response_chars.saturating_add(delta_chars);
                self.request_token_direction = RequestTokenDirection::Down;
                thinking.text.push_str(&delta);
                false
            }
            StreamEvent::ThinkingCompleted { provider, .. } => {
                if let Some(thinking) = self.active_thinking.as_mut() {
                    thinking.is_streaming = false;
                    thinking.completed_at = Some(Instant::now());
                }
                self.last_provider = Some(provider);
                false
            }
            StreamEvent::AssistantDelta { delta, .. } => {
                let delta_chars = delta.chars().count();
                self.streamed_response_chars =
                    self.streamed_response_chars.saturating_add(delta_chars);
                self.request_token_direction = RequestTokenDirection::Down;
                self.pending_assistant.push_str(&delta);
                false
            }
            StreamEvent::PermissionRequested { request } => {
                self.request_token_direction = RequestTokenDirection::Down;
                self.upsert_live_tool_activity(LiveToolActivity {
                    request_id: Some(request.request_id.clone()),
                    tool_use_id: request.tool_use_id.clone(),
                    tool_name: request.tool_name.clone(),
                    tool_input: request.tool_input.clone(),
                    status_line: "Waiting for permission".to_string(),
                    progress_messages: Vec::new(),
                    is_error: false,
                });
                // If a permission overlay is already open, queue this request
                // behind it rather than replacing it — otherwise the first
                // request loses its overlay and hangs until timeout.
                if let Some(OverlayState::PermissionRequest(active)) = self.overlay.as_mut() {
                    active.enqueue(request);
                } else {
                    self.overlay = Some(OverlayState::PermissionRequest(
                        PermissionOverlayState::new(request),
                    ));
                }
                false
            }
            StreamEvent::PermissionResolved {
                request_id, kind, ..
            } => {
                self.request_token_direction = RequestTokenDirection::Down;
                if let Some(OverlayState::PermissionRequest(state)) = self.overlay.as_mut() {
                    if state.request.request_id == request_id {
                        // The currently-shown request resolved: advance to the
                        // next queued one (if any) rather than dropping the
                        // overlay, so concurrent requests are shown in turn.
                        self.overlay = state
                            .take_next_queued()
                            .map(OverlayState::PermissionRequest);
                    } else {
                        // A QUEUED request resolved out-of-band (timeout, or
                        // another client answered it). Drop it from the queue so
                        // it is not surfaced later as a stale, already-resolved
                        // prompt.
                        state.remove_queued(&request_id);
                    }
                }
                if let Some(activity) = self.find_live_tool_activity_by_request_id_mut(&request_id)
                {
                    activity.status_line = match kind {
                        orbcode_protocol::PermissionResolutionKind::Approved => {
                            "Permission granted".to_string()
                        }
                        orbcode_protocol::PermissionResolutionKind::Denied => {
                            activity.is_error = true;
                            "Permission denied".to_string()
                        }
                        orbcode_protocol::PermissionResolutionKind::Interrupted => {
                            activity.is_error = true;
                            "Permission interrupted".to_string()
                        }
                        _ => {
                            activity.is_error = true;
                            "Permission resolved".to_string()
                        }
                    };
                }
                self.task_panel.mark_dirty();
                false
            }
            StreamEvent::ToolUseStarted {
                tool_use_id,
                tool_name,
                ..
            } => {
                self.request_token_direction = RequestTokenDirection::Down;
                self.in_progress_tool_use_ids.insert(tool_use_id.clone());
                let (resolved_name, resolved_input) = self
                    .lookup_tool_use(&tool_use_id)
                    .unwrap_or_else(|| (tool_name.clone(), String::new()));
                let activity = self.live_tool_activity_mut_or_insert(LiveToolActivity {
                    request_id: None,
                    tool_use_id: tool_use_id.clone(),
                    tool_name: resolved_name,
                    tool_input: resolved_input,
                    status_line: String::new(),
                    progress_messages: Vec::new(),
                    is_error: false,
                });
                activity.status_line = format!("Running `{tool_name}`");
                activity.is_error = false;
                false
            }
            StreamEvent::ToolProgress {
                tool_use_id,
                tool_name,
                progress,
                ..
            } => {
                self.request_token_direction = RequestTokenDirection::Down;
                let status_line = tool_progress_status_line(&progress)
                    .unwrap_or_else(|| format!("Running `{tool_name}`"));
                let progress_estimate_chars = embedded_assistant_progress_estimate_chars(&progress);
                let (resolved_name, resolved_input) = self
                    .lookup_tool_use(&tool_use_id)
                    .unwrap_or_else(|| (tool_name.clone(), String::new()));
                let activity = self.live_tool_activity_mut_or_insert(LiveToolActivity {
                    request_id: None,
                    tool_use_id: tool_use_id.clone(),
                    tool_name: resolved_name,
                    tool_input: resolved_input,
                    status_line: String::new(),
                    progress_messages: Vec::new(),
                    is_error: false,
                });
                let status_changed = activity.status_line != status_line;
                let progress_changed = activity.push_progress_message(progress);
                if status_changed {
                    activity.status_line = status_line.clone();
                }
                if progress_changed && progress_estimate_chars > 0 {
                    self.streamed_response_chars = self
                        .streamed_response_chars
                        .saturating_add(progress_estimate_chars);
                }
                false
            }
            StreamEvent::HookProgress { progress, .. } => {
                self.attach_hook_progress_to_latest_note_or_pending(progress);
                false
            }
            StreamEvent::HookNotice {
                hook_event_name,
                message,
                is_error,
                ..
            } => {
                let message = TranscriptMessage::new(
                    MessageRole::User,
                    hook_notice_transcript_content(&hook_event_name, &message, is_error),
                );
                self.attach_pending_hook_progress_to_message(&message);
                self.push_message_and_flush_history(message);
                false
            }
            StreamEvent::ToolUseCompleted {
                tool_use_id,
                tool_name,
                kind,
                ..
            } => {
                self.request_token_direction = RequestTokenDirection::Down;
                self.in_progress_tool_use_ids.remove(&tool_use_id);
                if task_tool_changes_panel(&tool_name) {
                    self.task_panel.mark_dirty();
                }
                if background_task_tool_changes_panel(&tool_name) {
                    self.background_agent_panel.mark_dirty();
                }
                if let Some(activity) =
                    self.find_live_tool_activity_by_tool_use_id_mut(&tool_use_id)
                {
                    activity.status_line = match kind {
                        orbcode_protocol::ToolUseCompletionKind::Success => {
                            format!("Finished `{tool_name}`")
                        }
                        orbcode_protocol::ToolUseCompletionKind::ExecutionFailed => {
                            activity.is_error = true;
                            "Failed during execution".to_string()
                        }
                        orbcode_protocol::ToolUseCompletionKind::PermissionDenied => {
                            activity.is_error = true;
                            "Permission denied".to_string()
                        }
                        orbcode_protocol::ToolUseCompletionKind::Interrupted => {
                            activity.is_error = true;
                            "Interrupted".to_string()
                        }
                        orbcode_protocol::ToolUseCompletionKind::UnknownTool => {
                            activity.is_error = true;
                            "Unknown tool".to_string()
                        }
                        _ => "Tool finished".to_string(),
                    };
                }
                false
            }
            StreamEvent::AssistantMessageCompleted {
                mut message,
                provider,
                fallback_from: _,
                usage,
            } => {
                self.request_token_direction = RequestTokenDirection::Down;
                let usage_total = usage_total_tokens(&usage);
                let usage_output_chars = usage_output_estimate_chars(&usage);
                self.current_turn_total_tokens =
                    self.current_turn_total_tokens.saturating_add(usage_total);
                self.streamed_response_chars = self.streamed_response_chars.max(usage_output_chars);
                if let Some(thinking_message) = self.take_completed_thinking_message() {
                    let completion_contains_thinking =
                        message_contains_matching_thinking_block(&message, &thinking_message);
                    let completion_has_non_thinking = message_has_non_thinking_block(&message);
                    if !completion_contains_thinking || completion_has_non_thinking {
                        message =
                            Self::remove_duplicate_completed_thinking(&thinking_message, message);
                        self.push_message_and_flush_history(thinking_message);
                    }
                }
                let has_tool_use = message
                    .blocks
                    .iter()
                    .any(|block| matches!(block, TranscriptBlock::ToolUse { .. }));
                let pending_assistant = self.pending_assistant.clone();
                let stream_started = self.assistant_stream_history_started();
                let final_text = assistant_completion_text(&message);
                let can_reuse_streamed_history = stream_started
                    && !has_tool_use
                    && final_text.as_ref().is_some_and(|text| {
                        assistant_stream_text_matches(&pending_assistant, text)
                    });
                if can_reuse_streamed_history {
                    let final_text = final_text.expect("checked above");
                    self.complete_assistant_stream_history_from_source(
                        final_text,
                        message.id.clone(),
                    );
                    self.clear_latest_message_focus();
                    self.pending_assistant.clear();
                    self.deferred_assistant_message = None;
                    self.messages.push(message);
                    self.pending_history_flush = true;
                    self.focus_latest_message_start = false;
                    self.prune_completed_live_tool_activity();
                } else {
                    self.reset_pending_assistant_stream();
                    self.push_message_and_flush_history(message);
                }
                self.active_thinking = None;
                if !has_tool_use {
                    self.stop_waiting_animation();
                }
                self.last_provider = Some(provider);
                self.update_status_context_percent(&usage);
                self.last_usage = Some(usage);
                false
            }
            StreamEvent::AssistantMessageDiscarded {
                provider,
                fallback_provider,
                ..
            } => {
                self.active_thinking = None;
                self.reset_pending_assistant_stream();
                self.clear_live_tool_activities();
                self.in_progress_tool_use_ids.clear();
                self.set_status_line(format!(
                    "Discarded partial response from {provider}; switching to {fallback_provider}."
                ));
                false
            }
            StreamEvent::ContextCompacted {
                duration_ms,
                summary,
                ..
            } => {
                self.push_message_and_flush_history(local_context_compacted_message(
                    Some(duration_ms),
                    summary,
                ));
                self.set_status_line("Conversation compacted automatically.");
                false
            }
            StreamEvent::TurnCancelled {
                kind,
                partial,
                usage,
                ..
            } => {
                let turn_duration_ms = self.current_request_elapsed_ms();
                self.reset_pending_assistant_stream();
                if let Some(message) = partial {
                    self.push_message_and_flush_history(message);
                }
                self.active_thinking = None;
                self.clear_live_tool_activities();
                self.in_progress_tool_use_ids.clear();
                // Mirror TurnFinished: a persistent overlay (e.g. the transcript
                // pager) must survive turn cancellation, not be torn down
                // mid-view.
                if !overlay_persists_after_turn(self.overlay.as_ref()) {
                    self.overlay = None;
                }
                self.stop_waiting_animation();
                if let Some(ref u) = usage {
                    self.update_status_context_percent(u);
                }
                self.last_usage = usage;
                self.set_status_line(match kind {
                    orbcode_protocol::TurnCancellationKind::BeforeResponse => {
                        "Turn cancelled before response.".to_string()
                    }
                    orbcode_protocol::TurnCancellationKind::AssistantStreaming => {
                        "Turn cancelled during assistant streaming.".to_string()
                    }
                    orbcode_protocol::TurnCancellationKind::ToolStage => {
                        "Turn cancelled during tool stage.".to_string()
                    }
                    _ => "Turn cancelled.".to_string(),
                });
                self.push_local_turn_duration_note(turn_duration_ms, 0);
                true
            }
            StreamEvent::TurnFinished {
                provider, usage, ..
            } => {
                let turn_duration_ms = self.current_request_elapsed_ms();
                let final_usage_total = usage_total_tokens(&usage);
                let turn_total_tokens = self.current_turn_total_tokens.max(final_usage_total);
                self.clear_live_tool_activities();
                self.in_progress_tool_use_ids.clear();
                if !overlay_persists_after_turn(self.overlay.as_ref()) {
                    self.overlay = None;
                }
                self.commit_deferred_assistant_message();
                self.stop_waiting_animation();
                self.last_provider = Some(provider);
                self.update_status_context_percent(&usage);
                self.status.has_rate_limit_warning = false;
                self.status.has_auth_warning = false;
                self.last_usage = Some(usage.clone());
                if let Some(warning) = self.token_warning_status_line(&usage) {
                    self.set_status_line(warning);
                }
                self.push_local_turn_duration_note(turn_duration_ms, turn_total_tokens);
                true
            }
            StreamEvent::Budget {
                outcome,
                blocked,
                total_usd,
                max_budget_usd,
                pricing_known,
                ..
            } => {
                if !blocked {
                    // Advisory warning (unknown pricing, non-strict policy): the
                    // turn proceeds, so just surface a status-line note.
                    self.set_status_line(format!(
                        "Budget warning: spend cannot be fully priced; \
                         counted ${total_usd:.4} of ${max_budget_usd:.4} cap."
                    ));
                    // Advisory only: the turn keeps running on the server, so this
                    // is not a terminal event. Returning `true` here would detach
                    // the live turn stream and drop the remaining deltas /
                    // `TurnFinished`, leaving the spinner stuck forever.
                    return false;
                }
                let turn_duration_ms = self.current_request_elapsed_ms();
                self.active_thinking = None;
                self.clear_live_tool_activities();
                self.in_progress_tool_use_ids.clear();
                if !overlay_persists_after_turn(self.overlay.as_ref()) {
                    self.overlay = None;
                }
                self.commit_deferred_assistant_message();
                self.stop_waiting_animation();
                let rendered = match outcome {
                    BudgetOutcome::Exceeded => format!(
                        "Budget limit reached: spent ${total_usd:.4} of \
                         ${max_budget_usd:.4} cap. Turn stopped before the next request."
                    ),
                    BudgetOutcome::UnknownPricing => format!(
                        "Budget enforcement (strict): session cost could not be priced; \
                         counted ${total_usd:.4} of ${max_budget_usd:.4} cap. \
                         Turn stopped before the next request."
                    ),
                    _ => format!(
                        "Budget limit reached: counted ${total_usd:.4} of \
                         ${max_budget_usd:.4} cap. Turn stopped before the next request."
                    ),
                };
                let rendered = if pricing_known {
                    rendered
                } else {
                    format!(
                        "{rendered}\n  note: some usage was unpriced, so the real spend is higher."
                    )
                };
                self.push_message_and_flush_history(local_error_message(rendered.clone()));
                self.push_local_turn_duration_note(turn_duration_ms, 0);
                self.set_status_line(rendered);
                true
            }
            StreamEvent::Error {
                provider,
                category,
                message,
                suggestion,
                ..
            } => {
                let turn_duration_ms = self.current_request_elapsed_ms();
                self.active_thinking = None;
                self.clear_live_tool_activities();
                self.in_progress_tool_use_ids.clear();
                if !overlay_persists_after_turn(self.overlay.as_ref()) {
                    self.overlay = None;
                }
                self.commit_deferred_assistant_message();
                self.stop_waiting_animation();
                if let Some(ref cat) = category {
                    match cat {
                        orbcode_protocol::StreamErrorCategory::RateLimit
                        | orbcode_protocol::StreamErrorCategory::Overload => {
                            self.status.has_rate_limit_warning = true;
                        }
                        orbcode_protocol::StreamErrorCategory::Auth => {
                            self.status.has_auth_warning = true;
                        }
                        _ => {}
                    }
                }
                let rendered = match provider {
                    Some(provider) => format!("{provider}: {message}"),
                    None => message,
                };
                let rendered = match suggestion {
                    Some(hint) if !hint.is_empty() => format!("{rendered}\n  hint: {hint}"),
                    _ => rendered,
                };
                self.push_message_and_flush_history(local_error_message(rendered.clone()));
                self.push_local_turn_duration_note(turn_duration_ms, 0);
                self.set_status_line(rendered);
                true
            }
            StreamEvent::AskUserQuestionRequested { .. }
            | StreamEvent::AskUserQuestionResolved { .. } => {
                // TODO(ask-user-question): render an interactive prompt widget
                // in the TUI. For now, treat as a no-op redraw trigger.
                false
            }
            StreamEvent::McpTrustApprovalRequested { .. }
            | StreamEvent::McpTrustApprovalResolved { .. } => {
                // Mid-turn trust approval is not terminal: the turn continues
                // once the user resolves it. Returning `true` would detach the
                // live turn stream and drop the rest of the response (matching
                // the `AskUserQuestionRequested` no-op above).
                false
            }
            StreamEvent::LocalTaskProgress { .. } => {
                if let Some(OverlayState::BackgroundJobs(state)) = self.overlay.as_mut() {
                    state.needs_refresh = true;
                }
                self.background_agent_panel.mark_dirty();
                false
            }
            StreamEvent::BackgroundTaskUpdated { session_id, task } => {
                if session_id.as_str() == self.session_id.as_str() {
                    self.background_agent_panel.set_session_id(session_id);
                    self.transcript_task_cards
                        .set_session_id(self.session_id.clone());
                    self.transcript_task_cards
                        .apply_pushed_view(task.clone(), Instant::now());
                    self.background_agent_panel
                        .apply_pushed_view(task, Instant::now());
                }
                if let Some(OverlayState::BackgroundJobs(state)) = self.overlay.as_mut() {
                    state.needs_refresh = true;
                }
                false
            }
            _ => false,
        }
    }
}
