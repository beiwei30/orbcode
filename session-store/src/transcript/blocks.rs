use std::collections::HashMap;
use std::fmt::Write as _;

use chrono::Utc;
use orbcode_protocol::{
    MessageRole, TranscriptBlock, TranscriptMessage, blocks_have_renderable_content,
    visible_content_from_blocks,
};
use serde_json::Value;
use uuid::Uuid;

use crate::transcript_schema::{
    RawContentBlock, TranscriptRecord, TranscriptRecordKind, raw_content_blocks,
};

use super::parse_timestamp;
use super::progress::attach_tool_result_progress_metadata;

/// Placeholder surfaced for `redacted_thinking` blocks. The encrypted
/// payload is opaque, but TypeScript keeps these turns visible (rendered as
/// "Thinking..."), so we preserve a visible assistant marker rather than
/// silently dropping the turn.
pub(crate) const REDACTED_THINKING_PLACEHOLDER: &str = "[redacted thinking]";

/// Default text for a `snip_boundary` system record that carries no explicit
/// content, mirroring the TypeScript SnipBoundaryMessage fallback.
pub(crate) const SNIP_BOUNDARY_PLACEHOLDER: &str =
    "[snip] Conversation history before this point has been snipped.";

pub(crate) fn transcript_message_from_record(
    record: &TranscriptRecord,
    progress_by_parent_tool_use_id: &HashMap<String, Vec<Value>>,
) -> Option<TranscriptMessage> {
    let role = match record.kind() {
        TranscriptRecordKind::User => MessageRole::User,
        TranscriptRecordKind::Assistant => MessageRole::Assistant,
        TranscriptRecordKind::System => MessageRole::System,
        _ => return None,
    };

    let (content, blocks) = match role {
        MessageRole::User => {
            let mut blocks = blocks_from_content(record.message.as_ref()?.content.as_ref()?);
            attach_tool_result_metadata(record, &mut blocks);
            attach_tool_result_progress_metadata(&mut blocks, progress_by_parent_tool_use_id);
            let content = visible_content_from_blocks(&blocks);
            if content.trim().is_empty() && !blocks_have_renderable_content(&blocks) {
                return None;
            }
            (content, blocks)
        }
        MessageRole::Assistant => {
            let blocks = blocks_from_content(record.message.as_ref()?.content.as_ref()?);
            let content = visible_content_from_blocks(&blocks);
            if content.trim().is_empty() && !blocks_have_renderable_content(&blocks) {
                return None;
            }
            (content, blocks)
        }
        MessageRole::System => {
            let content = record
                .message
                .as_ref()
                .and_then(|message| message.content.as_ref())
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    record
                        .content
                        .as_ref()
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .or_else(|| system_subtype_synthetic_content(record))?;
            let blocks = if content.is_empty() {
                Vec::new()
            } else {
                vec![TranscriptBlock::Text {
                    text: content.clone(),
                }]
            };
            (content, blocks)
        }
        _ => return None,
    };

    let id = record
        .uuid
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let created_at = record
        .timestamp
        .as_deref()
        .and_then(parse_timestamp)
        .unwrap_or_else(Utc::now);

    Some(TranscriptMessage {
        id,
        role,
        content,
        blocks,
        stop_reason: record
            .message
            .as_ref()
            .and_then(|message| message.stop_reason.clone()),
        usage: record
            .message
            .as_ref()
            .and_then(|message| message.usage.as_ref())
            .and_then(|usage| serde_json::from_value(usage.clone()).ok()),
        created_at,
        is_synthetic: false,
    })
}

/// Test-only bridge keeping the pre-refactor `(record_type, value, progress)`
/// call shape: the unit tests pass a record-type string that always matches the
/// value's `type`, so we decode the value and dispatch through the typed path.
#[cfg(test)]
pub(crate) fn transcript_message_from_value(
    _record_type: &str,
    value: &Value,
    progress_by_parent_tool_use_id: &HashMap<String, Vec<Value>>,
) -> Option<TranscriptMessage> {
    let record = TranscriptRecord::from_value(value)?;
    transcript_message_from_record(&record, progress_by_parent_tool_use_id)
}

fn blocks_from_content(content: &Value) -> Vec<TranscriptBlock> {
    raw_content_blocks(content)
        .into_iter()
        .filter_map(block_from_raw)
        .collect()
}

/// Map one classified [`RawContentBlock`] onto a protocol [`TranscriptBlock`],
/// returning `None` for blocks that carry no renderable content (empty text /
/// thinking, or an unknown block with no fallback `content` string).
fn block_from_raw(block: RawContentBlock) -> Option<TranscriptBlock> {
    match block {
        RawContentBlock::Text(text) => text
            .text
            .filter(|text| !text.is_empty())
            .map(|text| TranscriptBlock::Text { text }),
        RawContentBlock::Thinking(thinking) => {
            let text = thinking.thinking.or(thinking.text).unwrap_or_default();
            if text.is_empty() {
                return None;
            }
            let signature = thinking.signature.filter(|value| !value.is_empty());
            Some(TranscriptBlock::Thinking { text, signature })
        }
        // The encrypted `data` payload is opaque; surface a visible assistant
        // marker so the turn is preserved rather than dropped as content-less.
        RawContentBlock::RedactedThinking => Some(TranscriptBlock::Text {
            text: REDACTED_THINKING_PLACEHOLDER.to_string(),
        }),
        RawContentBlock::ToolUse(tool_use) => Some(TranscriptBlock::ToolUse {
            id: tool_use.id.unwrap_or_else(|| "tool-use".to_string()),
            name: tool_use.name.unwrap_or_else(|| "tool".to_string()),
            input: serialize_block_payload(tool_use.input.as_ref()),
        }),
        RawContentBlock::ToolResult(tool_result) => Some(TranscriptBlock::ToolResult {
            tool_use_id: tool_result
                .tool_use_id
                .unwrap_or_else(|| "tool-result".to_string()),
            content: extract_tool_result_content(tool_result.content.as_ref()),
            is_error: tool_result.is_error.unwrap_or(false),
            metadata: None,
        }),
        RawContentBlock::Other(value) => value
            .get("content")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(|text| TranscriptBlock::Text {
                text: text.to_string(),
            }),
    }
}

pub(crate) fn serialize_block_payload(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if value.is_null() {
        return String::new();
    }
    serde_json::to_string_pretty(value)
        .or_else(|_| serde_json::to_string(value))
        .unwrap_or_default()
}

fn extract_tool_result_content(value: Option<&Value>) -> String {
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
                    .or_else(|| {
                        item.get("content")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .or_else(|| item.as_str().map(str::to_string))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => serde_json::to_string_pretty(value)
            .or_else(|_| serde_json::to_string(value))
            .unwrap_or_default(),
    }
}

fn attach_tool_result_metadata(record: &TranscriptRecord, blocks: &mut [TranscriptBlock]) {
    let metadata: Option<String> = record
        .tool_use_result()
        .map(|value| serialize_block_payload(Some(value)));

    if let Some(metadata) = metadata {
        for block in blocks.iter_mut() {
            if let TranscriptBlock::ToolResult {
                metadata: existing, ..
            } = block
            {
                *existing = Some(metadata.clone());
            }
        }
    }
}

/// Build visible content for `system` records whose variant carries no plain
/// `content` string but should still be surfaced (API errors, snip
/// boundaries). Returns `None` for content-less system variants (e.g. SDK
/// `init`, `stop_hook_summary`), which the loader legitimately drops.
fn system_subtype_synthetic_content(record: &TranscriptRecord) -> Option<String> {
    match record.subtype.as_deref()? {
        "api_error" => Some(api_error_text(record)),
        "snip_boundary" => Some(SNIP_BOUNDARY_PLACEHOLDER.to_string()),
        _ => None,
    }
}

fn api_error_text(record: &TranscriptRecord) -> String {
    let mut text = String::from("API error");
    if let Some(message) = record.error.as_ref().and_then(extract_error_message) {
        text.push_str(": ");
        text.push_str(&message);
    }
    if let (Some(attempt), Some(max_retries)) = (record.retry_attempt, record.max_retries) {
        write!(text, " (attempt {attempt}/{max_retries})").expect("writing to String cannot fail");
    }
    text
}

fn extract_error_message(error: &Value) -> Option<String> {
    match error {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Object(_) => error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| {
                error
                    .get("error")
                    .and_then(|nested| nested.get("message"))
                    .and_then(Value::as_str)
            })
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transcript_message_from_value_keeps_tool_only_blocks_without_marker_content() {
        let message = transcript_message_from_value(
            "assistant",
            &json!({
                "type": "assistant",
                "uuid": "assistant-1",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "tool-1",
                            "name": "Read",
                            "input": { "file_path": "README.md" }
                        }
                    ]
                }
            }),
            &HashMap::new(),
        )
        .expect("tool-only assistant message should decode");

        assert_eq!(message.content, "");
        assert!(matches!(
            message.blocks.as_slice(),
            [TranscriptBlock::ToolUse { id, name, .. }] if id == "tool-1" && name == "Read"
        ));
    }

    #[test]
    fn redacted_thinking_block_is_kept_as_visible_assistant_text() {
        let message = transcript_message_from_value(
            "assistant",
            &json!({
                "type": "assistant",
                "uuid": "assistant-redacted",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "redacted_thinking", "data": "AbCdEf==" },
                        { "type": "text", "text": "Final answer." }
                    ]
                }
            }),
            &HashMap::new(),
        )
        .expect("assistant with redacted thinking decodes");

        assert!(
            message.content.contains(REDACTED_THINKING_PLACEHOLDER),
            "redacted thinking surfaces as visible text: {:?}",
            message.content
        );
        assert!(message.content.contains("Final answer."));
        assert!(
            message
                .blocks
                .iter()
                .any(|block| matches!(block, TranscriptBlock::Text { text } if text == REDACTED_THINKING_PLACEHOLDER)),
            "redacted thinking becomes a visible Text block: {:?}",
            message.blocks
        );
    }

    #[test]
    fn redacted_thinking_only_turn_is_surfaced_not_dropped() {
        let message = transcript_message_from_value(
            "assistant",
            &json!({
                "type": "assistant",
                "uuid": "assistant-redacted-only",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "redacted_thinking", "data": "Zz==" }]
                }
            }),
            &HashMap::new(),
        )
        .expect("redacted-thinking-only assistant turn is surfaced");
        assert_eq!(message.content, REDACTED_THINKING_PLACEHOLDER);
    }

    #[test]
    fn system_api_error_record_surfaces_error_text() {
        let message = transcript_message_from_value(
            "system",
            &json!({
                "type": "system",
                "subtype": "api_error",
                "uuid": "system-api-error",
                "timestamp": "2026-01-01T00:00:00Z",
                "level": "error",
                "error": { "message": "overloaded_error: server is busy" },
                "retryAttempt": 2,
                "maxRetries": 5,
                "retryInMs": 4000,
            }),
            &HashMap::new(),
        )
        .expect("api_error system record surfaces");
        assert_eq!(message.role, MessageRole::System);
        assert!(message.content.contains("overloaded_error: server is busy"));
        assert!(
            message.content.contains("attempt 2/5"),
            "{:?}",
            message.content
        );
    }

    #[test]
    fn system_api_error_with_string_error_surfaces_text() {
        let message = transcript_message_from_value(
            "system",
            &json!({
                "type": "system",
                "subtype": "api_error",
                "uuid": "system-api-error-string",
                "timestamp": "2026-01-01T00:00:00Z",
                "error": "Connection reset by peer",
            }),
            &HashMap::new(),
        )
        .expect("api_error with string error surfaces");
        assert!(message.content.contains("Connection reset by peer"));
    }

    #[test]
    fn system_snip_boundary_record_surfaces_default_text() {
        let message = transcript_message_from_value(
            "system",
            &json!({
                "type": "system",
                "subtype": "snip_boundary",
                "uuid": "system-snip",
                "timestamp": "2026-01-01T00:00:00Z",
                "snipMetadata": { "removedUuids": ["a", "b"] },
            }),
            &HashMap::new(),
        )
        .expect("snip_boundary surfaces default text");
        assert_eq!(message.content, SNIP_BOUNDARY_PLACEHOLDER);
    }

    #[test]
    fn system_snip_boundary_record_prefers_explicit_content() {
        let message = transcript_message_from_value(
            "system",
            &json!({
                "type": "system",
                "subtype": "snip_boundary",
                "uuid": "system-snip-content",
                "timestamp": "2026-01-01T00:00:00Z",
                "content": "snipped 12 messages",
            }),
            &HashMap::new(),
        )
        .expect("snip_boundary with content surfaces it");
        assert_eq!(message.content, "snipped 12 messages");
    }

    #[test]
    fn system_local_command_record_surfaces_content() {
        let message = transcript_message_from_value(
            "system",
            &json!({
                "type": "system",
                "subtype": "local_command",
                "uuid": "system-local-cmd",
                "timestamp": "2026-01-01T00:00:00Z",
                "content": "<command-name>/status</command-name>",
                "level": "info",
            }),
            &HashMap::new(),
        )
        .expect("local_command surfaces content");
        assert_eq!(message.content, "<command-name>/status</command-name>");
    }

    #[test]
    fn system_init_record_is_skipped_as_content_less() {
        let decoded = transcript_message_from_value(
            "system",
            &json!({
                "type": "system",
                "subtype": "init",
                "uuid": "system-init",
                "timestamp": "2026-01-01T00:00:00Z",
                "cwd": "/repo",
                "model": "claude-opus-4-7",
                "tools": ["Read", "Bash"],
            }),
            &HashMap::new(),
        );
        assert!(decoded.is_none(), "content-less system init is skipped");
    }

    #[test]
    fn attachment_record_is_skipped_without_panicking() {
        let decoded = transcript_message_from_value(
            "attachment",
            &json!({
                "type": "attachment",
                "uuid": "attachment-1",
                "timestamp": "2026-01-01T00:00:00Z",
                "attachment": { "type": "selected_lines", "filename": "main.rs" },
            }),
            &HashMap::new(),
        );
        assert!(decoded.is_none(), "attachment record produces no message");
    }

    #[test]
    fn unknown_record_type_is_forward_compatibly_skipped() {
        let decoded = transcript_message_from_value(
            "team-only-future-thing",
            &json!({
                "type": "team-only-future-thing",
                "uuid": "future-1",
                "timestamp": "2026-01-01T00:00:00Z",
                "payload": { "anything": true },
            }),
            &HashMap::new(),
        );
        assert!(
            decoded.is_none(),
            "unknown record type is skipped, not panicked"
        );
    }

    #[test]
    fn user_plan_text_message_is_surfaced() {
        let message = transcript_message_from_value(
            "user",
            &json!({
                "type": "user",
                "uuid": "user-plan",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": {
                    "role": "user",
                    "content": "Plan to implement:\n1. Do the thing\n2. Verify",
                },
            }),
            &HashMap::new(),
        )
        .expect("plan user text surfaces");
        assert!(message.content.contains("Plan to implement"));
    }
}
