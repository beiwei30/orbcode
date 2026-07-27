use orbcode_protocol::{MessageRole, TranscriptMessage};
use serde::Deserialize;

use crate::slash_commands::{
    SlashCommandDeferredFeedback, SlashCommandFeedback, slash_command_invocation,
};
use crate::state::TuiState;

#[derive(Deserialize)]
struct ContextCompactedPayload {
    duration_ms: Option<u64>,
    summary: Option<String>,
}

#[derive(Deserialize)]
struct SlashCommandOutputPayload {
    command: String,
    summary: String,
    detail_markdown: Option<String>,
    deferred_feedback: Option<String>,
}

pub(crate) const LOCAL_TURN_DURATION_PREFIX: &str = "\u{1f}cc_turn_duration:";
pub(crate) const LOCAL_SLASH_COMMAND_OUTPUT_PREFIX: &str = "\u{1f}cc_slash_command_output:";
pub(crate) const LOCAL_CONTEXT_COMPACTED_PREFIX: &str = "\u{1f}cc_context_compacted";
pub(crate) const LOCAL_ERROR_PREFIX: &str = "\u{1f}cc_error:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LocalTranscriptNote {
    TurnDuration {
        duration_ms: u64,
        verb_index: usize,
        total_tokens: u64,
    },
    Error {
        message: String,
    },
    ContextCompacted {
        duration_ms: Option<u64>,
        summary: Option<String>,
    },
    SlashCommandOutput {
        command: String,
        summary: String,
        detail_markdown: Option<String>,
        deferred_feedback: SlashCommandDeferredFeedback,
    },
}

pub(crate) fn parse_local_transcript_note(
    message: &TranscriptMessage,
) -> Option<LocalTranscriptNote> {
    if !matches!(message.role, MessageRole::System) {
        return None;
    }

    if let Some(payload) = message.content.strip_prefix(LOCAL_TURN_DURATION_PREFIX) {
        let mut parts = payload.splitn(3, ':');
        let verb_index: usize = parts.next()?.parse().ok()?;
        let duration_ms: u64 = parts.next()?.parse().ok()?;
        let total_tokens: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        return Some(LocalTranscriptNote::TurnDuration {
            duration_ms,
            verb_index,
            total_tokens,
        });
    }

    if let Some(payload) = message.content.strip_prefix(LOCAL_ERROR_PREFIX) {
        let message = serde_json::from_str::<String>(payload)
            .unwrap_or_else(|_| payload.to_string())
            .trim()
            .to_string();
        if !message.is_empty() {
            return Some(LocalTranscriptNote::Error { message });
        }
    }

    if let Some(payload) = message.content.strip_prefix(LOCAL_CONTEXT_COMPACTED_PREFIX) {
        if payload.is_empty() {
            return Some(LocalTranscriptNote::ContextCompacted {
                duration_ms: None,
                summary: None,
            });
        }
        let parsed: ContextCompactedPayload = serde_json::from_str(payload).ok()?;
        return Some(LocalTranscriptNote::ContextCompacted {
            duration_ms: parsed.duration_ms,
            summary: parsed.summary.filter(|s| !s.trim().is_empty()),
        });
    }

    let payload = message
        .content
        .strip_prefix(LOCAL_SLASH_COMMAND_OUTPUT_PREFIX)?;
    let parsed: SlashCommandOutputPayload = serde_json::from_str(payload).ok()?;
    let deferred_feedback = parsed
        .deferred_feedback
        .as_deref()
        .and_then(SlashCommandDeferredFeedback::parse)
        .unwrap_or(SlashCommandDeferredFeedback::Direct);
    Some(LocalTranscriptNote::SlashCommandOutput {
        command: parsed.command,
        summary: parsed.summary,
        detail_markdown: parsed.detail_markdown.filter(|d| !d.trim().is_empty()),
        deferred_feedback,
    })
}

pub(crate) fn local_slash_command_output_message(
    command: String,
    summary: String,
    detail_markdown: Option<String>,
    deferred_feedback: SlashCommandDeferredFeedback,
) -> TranscriptMessage {
    TranscriptMessage::new(
        MessageRole::System,
        encode_slash_command_output_note_with_deferred_feedback(
            command,
            summary,
            detail_markdown,
            deferred_feedback,
        ),
    )
}

impl TuiState {
    pub(crate) fn push_local_system_message(&mut self, content: String) {
        self.push_message_and_flush_history(TranscriptMessage::new(MessageRole::System, content));
    }

    pub(crate) fn push_local_slash_command_output(
        &mut self,
        command: impl Into<String>,
        summary: impl Into<String>,
        detail_markdown: Option<String>,
    ) {
        let command = command.into();
        let feedback = slash_command_feedback_for_line(&command);
        let detail_markdown = match feedback.deferred {
            SlashCommandDeferredFeedback::Hidden => None,
            SlashCommandDeferredFeedback::Quoted | SlashCommandDeferredFeedback::Direct => {
                detail_markdown.filter(|detail| !detail.trim().is_empty())
            }
        };
        self.push_message_and_flush_history(local_slash_command_output_message(
            command,
            summary.into(),
            detail_markdown,
            feedback.deferred,
        ));
    }
}

pub(crate) fn local_error_message(message: String) -> TranscriptMessage {
    let payload = serde_json::to_string(&message).unwrap_or_else(|_| message.clone());
    TranscriptMessage::new(
        MessageRole::System,
        format!("{LOCAL_ERROR_PREFIX}{payload}"),
    )
}

pub(crate) fn slash_command_feedback_for_line(command: &str) -> SlashCommandFeedback {
    slash_command_invocation(command).map_or(SlashCommandFeedback::DEFAULT, |invocation| {
        invocation.spec.feedback
    })
}

pub(crate) fn local_context_compacted_message(
    duration_ms: Option<u64>,
    summary: Option<String>,
) -> TranscriptMessage {
    let payload = serde_json::json!({
        "duration_ms": duration_ms,
        "summary": summary,
    });
    TranscriptMessage::new(
        MessageRole::System,
        format!("{LOCAL_CONTEXT_COMPACTED_PREFIX}{payload}"),
    )
}

#[cfg(test)]
pub(crate) fn encode_slash_command_output_note(
    command: String,
    summary: String,
    detail_markdown: Option<String>,
) -> String {
    encode_slash_command_output_note_with_deferred_feedback(
        command,
        summary,
        detail_markdown,
        SlashCommandDeferredFeedback::Direct,
    )
}

fn encode_slash_command_output_note_with_deferred_feedback(
    command: String,
    summary: String,
    detail_markdown: Option<String>,
    deferred_feedback: SlashCommandDeferredFeedback,
) -> String {
    let payload = serde_json::json!({
        "command": command,
        "summary": summary,
        "detail_markdown": detail_markdown,
        "deferred_feedback": deferred_feedback.as_str(),
    });
    format!("{LOCAL_SLASH_COMMAND_OUTPUT_PREFIX}{payload}")
}

pub(crate) fn nonempty_detail(detail: String) -> Option<String> {
    (!detail.trim().is_empty()).then_some(detail)
}
