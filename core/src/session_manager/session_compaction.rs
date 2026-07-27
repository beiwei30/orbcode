use orbcode_config::{AppConfig, effective_context_window_size};
use orbcode_model_provider::ProviderCancellationToken;
use orbcode_protocol::{
    MessageRole, SessionRecord, TokenUsage, TranscriptBlock, TranscriptMessage,
};

use super::SessionManager;
use crate::{
    CoreError, ProviderFailure,
    agent_loop::messages::repair_missing_tool_results,
    compaction::{
        CompactProviderStreamSink, CompactSessionResult, MICROCOMPACT_MIN_RESULT_CHARS,
        compact_empty_response_detail, compact_provider_messages, compact_user_summary_message,
        compaction_prompt, estimated_history_tokens, lightweight_compaction_summary,
        microcompact_keep_recent, microcompact_token_threshold, microcompact_tool_results,
        modeled_compaction_summary, snip_message_token_threshold, snip_oversized_messages,
    },
    context::build_turn_context_with_memory_home,
    retry::execute_stream_with_retry_and_fallback,
};

const MANUAL_COMPACT_THRESHOLD_PERCENT_ENV: &str =
    "ORBCODE_MANUAL_COMPACT_THRESHOLD_PERCENT_OVERRIDE";
const AUTOCOMPACT_RECENT_COMPACT_TURNS_ENV: &str =
    "ORBCODE_AUTOCOMPACT_RECENT_COMPACT_TURNS_OVERRIDE";
const DEFAULT_MANUAL_COMPACT_THRESHOLD_PERCENT: u32 = 50;
const DEFAULT_AUTOCOMPACT_RECENT_COMPACT_TURNS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum CompactDecision {
    Proceed,
    NeedsConfirmation {
        context_percent_used: u32,
        threshold_percent: u32,
    },
    SkippedRecentManual {
        turns_since_compact: usize,
    },
}

fn manual_compact_threshold_percent(config: &AppConfig) -> u32 {
    config
        .resolve_env(MANUAL_COMPACT_THRESHOLD_PERCENT_ENV)
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|p| (1..=100).contains(p))
        .unwrap_or(DEFAULT_MANUAL_COMPACT_THRESHOLD_PERCENT)
}

fn autocompact_recent_compact_turns(config: &AppConfig) -> usize {
    config
        .resolve_env(AUTOCOMPACT_RECENT_COMPACT_TURNS_ENV)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_AUTOCOMPACT_RECENT_COMPACT_TURNS)
}

fn context_percent_used(estimated_tokens: u32, effective_window: u32) -> u32 {
    if effective_window == 0 {
        return 100;
    }
    ((estimated_tokens as u64 * 100) / effective_window as u64).min(100) as u32
}

fn is_compact_summary_message(message: &TranscriptMessage) -> bool {
    message.role == MessageRole::System
        && message
            .content
            .starts_with("This session is being continued")
}

/// A user-role message that is purely tool results (parallel/serial tool
/// round-trips carried back to the model), not a genuine user prompt.
fn is_tool_result_message(message: &TranscriptMessage) -> bool {
    !message.blocks.is_empty()
        && message
            .blocks
            .iter()
            .all(|block| matches!(block, TranscriptBlock::ToolResult { .. }))
}

fn turns_since_last_compact(messages: &[TranscriptMessage]) -> Option<usize> {
    let compact_index = messages.iter().rposition(is_compact_summary_message)?;
    // Count genuine user prompts only. Tool-result messages are role User too;
    // counting them would let a burst of tool rounds inflate the count and
    // defeat the recent-compact guard, causing immediate re-compaction.
    let user_turns = messages[compact_index + 1..]
        .iter()
        .filter(|m| m.role == MessageRole::User && !is_tool_result_message(m))
        .count();
    Some(user_turns)
}

/// Outcome of a lightweight (snip + microcompact) compaction pass, carrying the
/// rebuilt session plus a human-readable summary for the context-compacted
/// stream event.
pub(crate) struct LightweightCompaction {
    pub(crate) result: CompactSessionResult,
    pub(crate) summary: String,
}

impl SessionManager {
    pub async fn evaluate_manual_compact_decision(
        &self,
        session_id: &str,
    ) -> Result<CompactDecision, CoreError> {
        let source = self.load_session(session_id).await?;
        if source.messages.is_empty() {
            return Ok(CompactDecision::Proceed);
        }
        let config = self.effective_config();
        let model = config
            .provider_model_resolution(config.default_provider)
            .request_model;
        let estimated = estimated_history_tokens(&source.messages);
        let effective_window = effective_context_window_size(
            &model,
            &config.context_window_options(),
            &config.max_output_token_options(),
        );
        let percent_used = context_percent_used(estimated, effective_window);
        let threshold = manual_compact_threshold_percent(&config);
        if percent_used < threshold {
            Ok(CompactDecision::NeedsConfirmation {
                context_percent_used: percent_used,
                threshold_percent: threshold,
            })
        } else {
            Ok(CompactDecision::Proceed)
        }
    }

    pub fn evaluate_autocompact_recent_guard(
        &self,
        messages: &[TranscriptMessage],
    ) -> CompactDecision {
        let config = self.effective_config();
        let guard_turns = autocompact_recent_compact_turns(&config);
        if let Some(turns) = turns_since_last_compact(messages)
            && turns < guard_turns
        {
            return CompactDecision::SkippedRecentManual {
                turns_since_compact: turns,
            };
        }
        CompactDecision::Proceed
    }

    pub async fn compact_session(
        &self,
        session_id: &str,
    ) -> Result<CompactSessionResult, CoreError> {
        let source = self.load_session(session_id).await?;
        let original_message_count = source.messages.len();
        if original_message_count == 0 {
            return Ok(CompactSessionResult {
                session: source,
                original_message_count,
                compacted_message_count: 0,
                provider_generated: false,
                fallback_reason: None,
                usage: None,
            });
        }

        let (summary, provider_generated, fallback_reason, usage) = self
            .compaction_summary_or_fallback(session_id, &source)
            .await;

        let mut compacted = SessionRecord {
            session_id: source.session_id.clone(),
            title: source.title.clone(),
            custom_title: source.custom_title.clone(),
            created_at: source.created_at,
            updated_at: source.updated_at,
            cwd: source.cwd.clone(),
            git_branch: source.git_branch.clone(),
            model: source.model.clone(),
            provider: source.provider,
            additional_directories: source.additional_directories.clone(),
            session_allowed_tools: source.session_allowed_tools.clone(),
            session_disallowed_tools: source.session_disallowed_tools.clone(),
            session_effort: source.session_effort,
            messages: Vec::new(),
        };
        let compacted_content = summary;
        compacted.push_message(
            TranscriptMessage::new(MessageRole::System, compacted_content).with_synthetic(true),
        );
        let compacted_message_count = compacted.messages.len();
        self.transcript_store
            .persist_full_session(&compacted)
            .await?;
        Ok(CompactSessionResult {
            session: compacted,
            original_message_count,
            compacted_message_count,
            provider_generated,
            fallback_reason,
            usage,
        })
    }

    pub(super) async fn compact_session_before_current_prompt(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<Option<CompactSessionResult>, CoreError> {
        let source = self.load_session(session_id).await?;
        if matches!(
            self.evaluate_autocompact_recent_guard(&source.messages),
            CompactDecision::SkippedRecentManual { .. }
        ) {
            return Ok(None);
        }
        let prompt_trimmed = prompt.trim();
        let prompt_index = source
            .messages
            .iter()
            .rposition(|message| {
                message.role == MessageRole::User && message.content.trim() == prompt_trimmed
            })
            .or_else(|| {
                // The stored message may have been normalized/rebuilt from blocks,
                // so an exact `content ==` compare fails. The current prompt is
                // the most recent genuine (non-tool-result) user message in this
                // call path — fall back to it so compaction still runs instead of
                // silently no-op'ing and surfacing prompt-too-long.
                source.messages.iter().rposition(|message| {
                    message.role == MessageRole::User && !is_tool_result_message(message)
                })
            });
        let Some(prompt_index) = prompt_index else {
            return Ok(None);
        };
        if prompt_index == 0 {
            return Ok(None);
        }

        let prefix = source.messages[..prompt_index].to_vec();
        if prefix.is_empty() {
            return Ok(None);
        }
        let suffix = source.messages[prompt_index..].to_vec();
        let prefix_source = SessionRecord {
            session_id: source.session_id.clone(),
            title: source.title.clone(),
            custom_title: source.custom_title.clone(),
            created_at: source.created_at,
            updated_at: source.updated_at,
            cwd: source.cwd.clone(),
            git_branch: source.git_branch.clone(),
            model: source.model.clone(),
            provider: source.provider,
            additional_directories: source.additional_directories.clone(),
            session_allowed_tools: source.session_allowed_tools.clone(),
            session_disallowed_tools: source.session_disallowed_tools.clone(),
            session_effort: source.session_effort,
            messages: prefix,
        };
        let (summary, provider_generated, fallback_reason, usage) = self
            .compaction_summary_or_fallback(session_id, &prefix_source)
            .await;

        let mut compacted = SessionRecord {
            session_id: source.session_id.clone(),
            title: source.title.clone(),
            custom_title: source.custom_title.clone(),
            created_at: source.created_at,
            updated_at: source.updated_at,
            cwd: source.cwd.clone(),
            git_branch: source.git_branch.clone(),
            model: source.model.clone(),
            provider: source.provider,
            additional_directories: source.additional_directories.clone(),
            session_allowed_tools: source.session_allowed_tools.clone(),
            session_disallowed_tools: source.session_disallowed_tools.clone(),
            session_effort: source.session_effort,
            messages: Vec::new(),
        };
        compacted.push_message(
            TranscriptMessage::new(MessageRole::System, summary).with_synthetic(true),
        );
        compacted.messages.extend(suffix);
        let compacted_message_count = compacted.messages.len();
        self.transcript_store
            .persist_full_session(&compacted)
            .await?;
        Ok(Some(CompactSessionResult {
            session: compacted,
            original_message_count: source.messages.len(),
            compacted_message_count,
            provider_generated,
            fallback_reason,
            usage,
        }))
    }

    /// Run the cheap snip + microcompact passes over the persisted history
    /// before a provider request, persisting any reduction so the rebuilt
    /// request (and a later resume) sees the smaller history. Returns `None`
    /// when nothing qualified, leaving the heavier prompt-too-long autocompact
    /// path to handle the request as before.
    pub(crate) async fn lightweight_compact_before_request(
        &self,
        session_id: &str,
        model: &str,
        config: &AppConfig,
    ) -> Result<Option<LightweightCompaction>, CoreError> {
        let mut source = self.load_session(session_id).await?;
        let original_message_count = source.messages.len();
        if original_message_count < 2 {
            return Ok(None);
        }

        let messages = std::mem::take(&mut source.messages);
        let snip_threshold = snip_message_token_threshold(config, model);
        let (mut messages, snipped_ids) = snip_oversized_messages(messages, snip_threshold);
        let snipped_count = snipped_ids.len();

        let microcompacted =
            if estimated_history_tokens(&messages) >= microcompact_token_threshold(config, model) {
                microcompact_tool_results(
                    &mut messages,
                    microcompact_keep_recent(config),
                    MICROCOMPACT_MIN_RESULT_CHARS,
                )
            } else {
                0
            };

        if snipped_count == 0 && microcompacted == 0 {
            return Ok(None);
        }

        source.messages = messages;
        let compacted_message_count = source.messages.len();
        self.transcript_store.persist_full_session(&source).await?;
        Ok(Some(LightweightCompaction {
            result: CompactSessionResult {
                session: source,
                original_message_count,
                compacted_message_count,
                provider_generated: false,
                fallback_reason: None,
                usage: None,
            },
            summary: lightweight_compaction_summary(snipped_count, microcompacted),
        }))
    }

    async fn compaction_summary_or_fallback(
        &self,
        session_id: &str,
        source: &SessionRecord,
    ) -> (String, bool, Option<String>, Option<TokenUsage>) {
        match self.provider_compaction_summary(session_id, source).await {
            Ok((summary, usage)) => (summary, true, None, Some(usage)),
            Err(error) => (
                // Wrap the modeled fallback in the canonical continuation
                // message too. Persisting the raw `modeled_compaction_summary`
                // (which starts with "Compacted conversation summary:") means the
                // recent-compact guard and session-store boundary detection —
                // both keyed on the "This session is being continued" prefix —
                // miss it, causing repeated re-compaction.
                compact_user_summary_message(
                    &modeled_compaction_summary(&source.messages),
                    &self.transcript_store.path(session_id),
                ),
                false,
                Some(error.to_string()),
                None,
            ),
        }
    }

    async fn provider_compaction_summary(
        &self,
        session_id: &str,
        source: &SessionRecord,
    ) -> Result<(String, TokenUsage), CoreError> {
        let config = self.effective_config();
        self.permission_context()
            .ensure_provider_call_allowed(config.default_provider)?;
        let additional_directories = self.additional_directories();
        let context = build_turn_context_with_memory_home(
            &config.cwd,
            &additional_directories,
            &config.home_dir,
        )
        .await;
        let prompt = compaction_prompt();
        // Repair any dangling tool_use (an assistant tool_use with no matching
        // tool_result) before appending the compaction prompt. Otherwise the
        // user prompt follows an unresolved tool_use and Anthropic rejects the
        // request with a 400, which is swallowed into the degraded modeled
        // summary instead of a real compaction.
        let mut messages = repair_missing_tool_results(compact_provider_messages(&source.messages));
        messages.push(TranscriptMessage::new(MessageRole::User, prompt.clone()));
        let mut request = self
            .provider_request_for_messages(session_id, &prompt, context, messages, false, false)
            .await;
        request.system_prompt =
            "You are a helpful AI assistant tasked with summarizing conversations.".to_string();
        request.disable_thinking = true;
        self.provider_debug_trace
            .record(config.default_provider, "compact", &request)
            .await;
        let mut stream_sink = CompactProviderStreamSink::new(config.default_provider);
        execute_stream_with_retry_and_fallback(
            &config,
            request,
            &mut stream_sink,
            ProviderCancellationToken::default(),
        )
        .await?;
        let response = stream_sink.into_response();
        let usage = response.usage.clone();
        let summary = response.content.trim();
        if summary.is_empty() {
            return Err(CoreError::ProviderFailed(ProviderFailure::from_message(
                format!(
                    "provider returned an empty compaction summary ({})",
                    compact_empty_response_detail(&response)
                ),
            )));
        }
        Ok((
            compact_user_summary_message(summary, &self.transcript_store.path(session_id)),
            usage,
        ))
    }
}

#[cfg(test)]
mod compaction_guard_tests {
    use super::{is_tool_result_message, turns_since_last_compact};
    use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};

    fn compact_boundary() -> TranscriptMessage {
        TranscriptMessage::new(
            MessageRole::System,
            "This session is being continued from a previous conversation...",
        )
    }

    fn tool_result() -> TranscriptMessage {
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                content: "output".to_string(),
                is_error: false,
                metadata: None,
            }],
        )
    }

    #[test]
    fn turns_since_last_compact_ignores_tool_result_messages() {
        let messages = vec![
            compact_boundary(),
            TranscriptMessage::new(MessageRole::User, "first real prompt"),
            TranscriptMessage::new(MessageRole::Assistant, "reply"),
            tool_result(),
            tool_result(),
            TranscriptMessage::new(MessageRole::User, "second real prompt"),
        ];
        // Only the two genuine user prompts count — not the tool_result messages
        // (which would otherwise inflate the count and defeat the guard).
        assert_eq!(turns_since_last_compact(&messages), Some(2));
    }

    #[test]
    fn is_tool_result_message_detects_tool_only_user_messages() {
        assert!(is_tool_result_message(&tool_result()));
        assert!(!is_tool_result_message(&TranscriptMessage::new(
            MessageRole::User,
            "hello"
        )));
    }
}
