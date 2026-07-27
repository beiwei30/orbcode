use orbcode_protocol::{MessageRole, TranscriptMessage};

use crate::history_cell::cells::is_plain_user_text_message;

pub(crate) fn hook_notice_transcript_content(
    hook_event_name: &str,
    message: &str,
    is_error: bool,
) -> String {
    let suffix = if is_error { "feedback" } else { "stopped" };
    format!("{hook_event_name} hook {suffix}:\n{message}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookTranscriptNote {
    pub(crate) event_name: String,
    pub(crate) title: String,
    pub(crate) body: Option<String>,
    pub(crate) is_error: bool,
}

pub(crate) fn parse_hook_transcript_note(
    message: &TranscriptMessage,
) -> Option<HookTranscriptNote> {
    if !matches!(message.role, MessageRole::User) || !is_plain_user_text_message(message) {
        return None;
    }

    parse_hook_transcript_note_content(&message.content)
}

fn parse_hook_transcript_note_content(content: &str) -> Option<HookTranscriptNote> {
    let content = content.trim_end();
    if content
        == "The PermissionDenied hook indicated this command is now approved. You may retry it if you would like."
    {
        return Some(HookTranscriptNote {
            event_name: "PermissionDenied".to_string(),
            title: "PermissionDenied hook".to_string(),
            body: Some(content.to_string()),
            is_error: false,
        });
    }

    let (first_line, body) = content.split_once('\n').unwrap_or((content, ""));
    let first_line = first_line.trim_end();
    let (hook_name, kind) = first_line
        .strip_suffix(" hook context:")
        .map(|hook_name| (hook_name, "context"))
        .or_else(|| {
            first_line
                .strip_suffix(" hook feedback:")
                .map(|hook_name| (hook_name, "feedback"))
        })
        .or_else(|| {
            first_line
                .strip_suffix(" hook stopped:")
                .map(|hook_name| (hook_name, "stopped"))
        })?;
    if !is_hook_note_event_name(hook_name) {
        return None;
    }

    let body = body.trim_end();
    if body.is_empty() {
        return None;
    }

    Some(HookTranscriptNote {
        event_name: hook_name.to_string(),
        title: format!("{hook_name} hook"),
        body: Some(body.to_string()),
        is_error: kind == "feedback",
    })
}

fn is_hook_note_event_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}
