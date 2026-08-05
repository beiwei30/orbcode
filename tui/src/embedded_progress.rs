use std::fmt::Write as _;

use orbcode_protocol::ProgressEnvelope;
use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};
use serde_json::Value;

use crate::render::text_utils::collapse_inline_whitespace;

pub(crate) fn tool_progress_status_line(progress: &Value) -> Option<String> {
    let data = ProgressEnvelope::parse(progress)?;
    let mut status = data.status?.to_string();
    if let Some(ref error) = data.error {
        let error_text = error.trim();
        if !error_text.is_empty() {
            let status_prefix = status.trim().trim_end_matches('.');
            let error_text = collapse_inline_whitespace(error_text)
                .replace('`', "")
                .trim()
                .trim_end_matches('.')
                .to_string();
            if !error_text.is_empty() {
                status = format!("{status_prefix}: {error_text}");
            }
        }
    }
    Some(normalize_progress_label(&status))
}

pub(crate) fn normalize_progress_label(status: &str) -> String {
    let collapsed = collapse_inline_whitespace(status).replace('`', "");
    let trimmed = collapsed.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "Working...".to_string()
    } else if trimmed.ends_with("...") {
        trimmed.to_string()
    } else {
        format!("{trimmed}...")
    }
}

pub(crate) fn progress_status_text(progress: &Value) -> Option<String> {
    ProgressEnvelope::parse(progress).and_then(|d| d.status)
}

pub(crate) fn progress_error_detail(progress: &Value) -> Option<String> {
    ProgressEnvelope::parse(progress)
        .and_then(|d| d.error)
        .filter(|value| !value.trim().is_empty())
}

pub(crate) fn hook_progress_event_name(progress: &Value) -> Option<String> {
    ProgressEnvelope::parse(progress).and_then(|d| d.hook_event_name)
}

pub(crate) fn hook_progress_result(progress: &Value) -> Option<String> {
    ProgressEnvelope::parse(progress).and_then(|d| d.result)
}

fn hook_progress_duration_ms(progress: &Value) -> Option<u64> {
    ProgressEnvelope::parse(progress).and_then(|d| d.duration_ms)
}

fn hook_progress_exit_code(progress: &Value) -> Option<i64> {
    ProgressEnvelope::parse(progress).and_then(|d| d.exit_code)
}

pub(crate) fn hook_progress_detail_line(progress: &Value) -> Option<String> {
    let result_str = hook_progress_result(progress);
    let result = result_str.as_deref().unwrap_or("completed");
    let verb = match result {
        "blocked" => "blocked",
        "failed" => "failed",
        "timed_out" => "timed out",
        _ => "completed",
    };
    let mut used_duration = false;
    let mut line = if let Some(duration_ms) = hook_progress_duration_ms(progress) {
        used_duration = true;
        format!("{verb} in {duration_ms} ms")
    } else {
        tool_progress_status_line(progress)
            .map(|status| status.trim_end_matches('.').to_string())?
    };
    if let Some(exit_code) = hook_progress_exit_code(progress) {
        write!(line, " (exit {exit_code})").expect("writing to String cannot fail");
    }
    if used_duration && let Some(error) = progress_error_detail(progress) {
        line.push_str(": ");
        line.push_str(
            collapse_inline_whitespace(&error)
                .replace('`', "")
                .trim()
                .trim_end_matches('.'),
        );
    }
    Some(line)
}

pub(crate) fn hook_progress_is_error(progress: &Value) -> bool {
    matches!(
        hook_progress_result(progress).as_deref(),
        Some("blocked" | "failed" | "timed_out")
    )
}

pub(crate) fn should_render_tool_progress_message(progress: &Value) -> bool {
    !is_non_actionable_post_tool_hook_progress(progress)
}

fn is_non_actionable_post_tool_hook_progress(progress: &Value) -> bool {
    if !matches!(
        hook_progress_event_name(progress).as_deref(),
        Some("PostToolUse" | "PostToolUseFailure")
    ) {
        return false;
    }
    if hook_progress_is_error(progress) || progress_error_detail(progress).is_some() {
        return false;
    }

    matches!(hook_progress_result(progress).as_deref(), Some("completed"))
        || (hook_progress_result(progress).is_none()
            && progress_status_text(progress)
                .is_some_and(|status| status.to_ascii_lowercase().contains("hook completed")))
}

fn extract_embedded_tool_result_content(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };

    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| item.as_str().map(str::to_string))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => serde_json::to_string_pretty(value)
            .or_else(|_| serde_json::to_string(value))
            .unwrap_or_default(),
    }
}

fn extract_embedded_visible_content(blocks: &[TranscriptBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::Text { text } | TranscriptBlock::Thinking { text, .. } => {
                (!text.is_empty()).then_some(text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn extract_embedded_message_blocks(value: Option<&Value>) -> Vec<TranscriptBlock> {
    match value {
        Some(Value::String(text)) => vec![TranscriptBlock::Text { text: text.clone() }],
        Some(Value::Array(items)) => {
            let mut blocks = Vec::new();
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = item.get("text").and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            blocks.push(TranscriptBlock::Text {
                                text: text.to_string(),
                            });
                        }
                    }
                    Some("thinking" | "redacted_thinking") => {
                        if let Some(text) = item
                            .get("thinking")
                            .or_else(|| item.get("text"))
                            .and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            blocks.push(TranscriptBlock::Thinking {
                                text: text.to_string(),
                                signature: item
                                    .get("signature")
                                    .and_then(Value::as_str)
                                    .map(ToString::to_string)
                                    .filter(|value| !value.is_empty()),
                            });
                        }
                    }
                    Some("tool_use") => {
                        let id = item
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("tool-use")
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let input = item
                            .get("input")
                            .map(|input| {
                                serde_json::to_string_pretty(input)
                                    .or_else(|_| serde_json::to_string(input))
                                    .unwrap_or_default()
                            })
                            .unwrap_or_default();
                        blocks.push(TranscriptBlock::ToolUse { id, name, input });
                    }
                    Some("tool_result") => {
                        let tool_use_id = item
                            .get("tool_use_id")
                            .or_else(|| item.get("toolUseId"))
                            .and_then(Value::as_str)
                            .unwrap_or("tool-result")
                            .to_string();
                        let is_error = item
                            .get("is_error")
                            .or_else(|| item.get("isError"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        blocks.push(TranscriptBlock::ToolResult {
                            tool_use_id,
                            content: extract_embedded_tool_result_content(item.get("content"))
                                .into(),
                            is_error,
                            metadata: None,
                        });
                    }
                    _ => {}
                }
            }
            blocks
        }
        _ => Vec::new(),
    }
}

fn serialize_embedded_tool_result_metadata(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if value.is_null() {
        return None;
    }

    match value {
        Value::String(raw) => Some(raw.clone()),
        other => serde_json::to_string(other).ok(),
    }
}

fn attach_embedded_tool_result_metadata(blocks: &mut [TranscriptBlock], message: &Value) {
    let Some(metadata) = serialize_embedded_tool_result_metadata(message.get("toolUseResult"))
    else {
        return;
    };

    for block in blocks.iter_mut() {
        if let TranscriptBlock::ToolResult {
            metadata: block_metadata,
            ..
        } = block
        {
            *block_metadata = Some(metadata.clone());
        }
    }
}

pub(crate) fn embedded_progress_message_to_transcript(
    progress: &Value,
) -> Option<TranscriptMessage> {
    let data = ProgressEnvelope::parse(progress)?;
    let message = data.message.as_ref()?;
    let record_type = message.get("type").and_then(Value::as_str)?;

    match record_type {
        "assistant" | "user" => {
            let role = if record_type == "assistant" {
                MessageRole::Assistant
            } else {
                MessageRole::User
            };
            let mut blocks = extract_embedded_message_blocks(
                message.get("message").and_then(|m| m.get("content")),
            );
            if matches!(role, MessageRole::User)
                && blocks
                    .iter()
                    .any(|block| matches!(block, TranscriptBlock::ToolResult { .. }))
            {
                message.get("toolUseResult")?;
                attach_embedded_tool_result_metadata(&mut blocks, message);
            }
            let content = if blocks.is_empty() {
                message
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            } else {
                extract_embedded_visible_content(&blocks)
            };
            if content.trim().is_empty() && blocks.is_empty() {
                None
            } else {
                Some(TranscriptMessage::from_parts(role, content, blocks))
            }
        }
        "system" => {
            let content = message
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    message
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .or_else(|| {
                    message
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })?;
            Some(TranscriptMessage::new(MessageRole::System, content))
        }
        _ => None,
    }
}

pub(crate) fn should_render_embedded_progress_message(
    message: &TranscriptMessage,
    allow_embedded_tool_messages: bool,
) -> bool {
    if allow_embedded_tool_messages || message.blocks.is_empty() {
        return true;
    }

    !message.blocks.iter().all(|block| {
        matches!(
            block,
            TranscriptBlock::ToolUse { .. } | TranscriptBlock::ToolResult { .. }
        )
    })
}
