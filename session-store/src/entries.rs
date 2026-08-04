use std::path::Path;

use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::codec::{deserialize_block_payload, effective_blocks};
use crate::transcript::{TRANSCRIPT_ENTRYPOINT, TRANSCRIPT_VERSION};

pub fn transcript_entries(
    cwd: &Path,
    anthropic_model: &str,
    session_id: &str,
    message: &TranscriptMessage,
    parent_uuid: Option<&str>,
) -> Vec<Value> {
    let timestamp = message
        .created_at
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    match message.role {
        MessageRole::User => {
            let (tool_result_metadata, progress_records) =
                extract_tool_result_metadata_and_progress(message);
            let mut entries = progress_records
                .into_iter()
                .map(|progress| {
                    progress_transcript_entry(cwd, session_id, parent_uuid, &timestamp, progress)
                })
                .collect::<Vec<_>>();
            let mut entry = json!({
                "parentUuid": parent_uuid,
                "isSidechain": false,
                // Derive the promptId deterministically from the message id so a
                // full rewrite does not re-mint it every time — a fresh random
                // id per serialization broke prompt-to-turn correlation and
                // churned bytes on every rewrite.
                "promptId": Uuid::new_v5(&Uuid::NAMESPACE_OID, message.id.as_bytes()).to_string(),
                "type": "user",
                "message": {
                    "role": "user",
                    "content": serialize_user_content(message),
                },
                "uuid": message.id,
                "timestamp": timestamp,
                "permissionMode": "default",
                "userType": "external",
                "entrypoint": TRANSCRIPT_ENTRYPOINT,
                "cwd": cwd.display().to_string(),
                "sessionId": session_id,
                "version": TRANSCRIPT_VERSION,
            });
            if let Some(metadata) = tool_result_metadata {
                entry["toolUseResult"] = metadata;
            }
            entries.push(entry);
            entries
        }
        MessageRole::Assistant => {
            let model = message
                .cost_attribution
                .as_ref()
                .map_or(anthropic_model, |attribution| attribution.model.as_str());
            let mut entry = json!({
                "parentUuid": parent_uuid,
                "isSidechain": false,
                "message": {
                    "id": format!("orbcode-{}", message.id),
                    "type": "message",
                    "role": "assistant",
                    "content": serialize_assistant_content(message),
                    "model": model,
                    "stop_reason": message
                        .stop_reason
                        .clone()
                        .unwrap_or_else(|| "end_turn".to_string()),
                    "stop_sequence": Value::Null,
                    "usage": message.usage,
                },
                "type": "assistant",
                "uuid": message.id,
                "timestamp": timestamp,
                "userType": "external",
                "entrypoint": TRANSCRIPT_ENTRYPOINT,
                "cwd": cwd.display().to_string(),
                "sessionId": session_id,
                "version": TRANSCRIPT_VERSION,
            });
            if let Some(attribution) = message.cost_attribution.as_ref() {
                entry["provider"] = Value::String(attribution.provider.as_str().to_string());
                entry["billingBasis"] = Value::String(
                    if attribution.subscription {
                        "subscription"
                    } else {
                        "api"
                    }
                    .to_string(),
                );
            }
            vec![entry]
        }
        MessageRole::System => vec![json!({
            "parentUuid": parent_uuid,
            "isSidechain": false,
            "type": "system",
            "message": {
                "role": "system",
                "content": message.content,
            },
            "uuid": message.id,
            "timestamp": timestamp,
            "userType": "external",
            "entrypoint": TRANSCRIPT_ENTRYPOINT,
            "cwd": cwd.display().to_string(),
            "sessionId": session_id,
            "version": TRANSCRIPT_VERSION,
        })],
        _ => Vec::new(),
    }
}

pub fn progress_transcript_entry(
    cwd: &Path,
    session_id: &str,
    parent_uuid: Option<&str>,
    default_timestamp: &str,
    progress: Value,
) -> Value {
    let mut entry = match progress {
        Value::Object(map) => Value::Object(map),
        other => json!({ "data": other }),
    };

    if let Some(object) = entry.as_object_mut() {
        object.insert("type".to_string(), Value::String("progress".to_string()));
        object.insert(
            "uuid".to_string(),
            object
                .get("uuid")
                .cloned()
                .unwrap_or_else(|| Value::String(Uuid::new_v4().to_string())),
        );
        object.insert(
            "timestamp".to_string(),
            object
                .get("timestamp")
                .cloned()
                .unwrap_or_else(|| Value::String(default_timestamp.to_string())),
        );
        object.insert(
            "parentUuid".to_string(),
            parent_uuid.map_or(Value::Null, |value| Value::String(value.to_string())),
        );
        object.insert("isSidechain".to_string(), Value::Bool(false));
        object.insert(
            "userType".to_string(),
            Value::String("external".to_string()),
        );
        object.insert(
            "entrypoint".to_string(),
            Value::String(TRANSCRIPT_ENTRYPOINT.to_string()),
        );
        object.insert("cwd".to_string(), Value::String(cwd.display().to_string()));
        object.insert(
            "sessionId".to_string(),
            Value::String(session_id.to_string()),
        );
        object.insert(
            "version".to_string(),
            Value::String(TRANSCRIPT_VERSION.to_string()),
        );
    }

    entry
}

pub fn serialize_assistant_content(message: &TranscriptMessage) -> Value {
    Value::Array(serialize_blocks(&effective_blocks(message)))
}

fn extract_tool_result_metadata_and_progress(
    message: &TranscriptMessage,
) -> (Option<Value>, Vec<Value>) {
    for block in effective_blocks(message) {
        let TranscriptBlock::ToolResult { metadata, .. } = block else {
            continue;
        };
        // Skip tool_result blocks without metadata rather than returning: a
        // message like `[ToolResult{meta:None}, ToolResult{meta:Some}]` (parallel
        // tools) must not drop the second block's `toolUseResult`/progress.
        let Some(metadata) = metadata else {
            continue;
        };
        let parsed = serde_json::from_str::<Value>(&metadata).unwrap_or(Value::String(metadata));
        return split_progress_messages_from_metadata(parsed);
    }

    (None, Vec::new())
}

fn split_progress_messages_from_metadata(metadata: Value) -> (Option<Value>, Vec<Value>) {
    let mut metadata = metadata;
    let Some(object) = metadata.as_object_mut() else {
        return (Some(metadata), Vec::new());
    };

    let progress = object
        .remove("progressMessages")
        .or_else(|| object.remove("progress_messages"))
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();

    let metadata = if object.is_empty() {
        None
    } else {
        Some(metadata)
    };

    (metadata, progress)
}

fn serialize_user_content(message: &TranscriptMessage) -> Value {
    let blocks = effective_blocks(message);
    if blocks
        .iter()
        .all(|block| matches!(block, TranscriptBlock::Text { .. }))
    {
        return Value::String(message.content.clone());
    }
    Value::Array(serialize_blocks(&blocks))
}

fn serialize_blocks(blocks: &[TranscriptBlock]) -> Vec<Value> {
    blocks
        .iter()
        .map(|block| match block {
            TranscriptBlock::Text { text } => json!({
                "type": "text",
                "text": text,
            }),
            TranscriptBlock::Thinking { text, signature } => json!({
                "type": "thinking",
                "thinking": text,
                "signature": signature.clone().unwrap_or_default(),
            }),
            TranscriptBlock::ToolUse { id, name, input } => json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": deserialize_block_payload(input),
            }),
            TranscriptBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
            }),
            _ => json!({
                "type": "unknown",
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn message(
        role: MessageRole,
        id: &str,
        content: &str,
        blocks: Vec<TranscriptBlock>,
    ) -> TranscriptMessage {
        TranscriptMessage {
            id: id.to_string(),
            role,
            content: content.to_string(),
            blocks,
            stop_reason: None,
            usage: None,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            is_synthetic: false,
            cost_attribution: None,
        }
    }

    #[test]
    fn transcript_entries_builds_user_entry_with_split_progress() {
        let message = message(
            MessageRole::User,
            "user-1",
            "",
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "done".to_string(),
                is_error: false,
                metadata: Some(
                    json!({
                        "status": "completed",
                        "progressMessages": [
                            {
                                "uuid": "progress-1",
                                "parentToolUseID": "tool-1",
                                "data": { "message": "working" }
                            }
                        ]
                    })
                    .to_string(),
                ),
            }],
        );

        let entries = transcript_entries(
            Path::new("/tmp/project"),
            "claude-sonnet-4",
            "session-1",
            &message,
            Some("assistant-1"),
        );

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].get("type").and_then(Value::as_str),
            Some("progress")
        );
        assert_eq!(
            entries[0].get("parentToolUseID").and_then(Value::as_str),
            Some("tool-1")
        );
        assert_eq!(entries[1].get("type").and_then(Value::as_str), Some("user"));
        assert_eq!(
            entries[1]
                .get("toolUseResult")
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str),
            Some("completed")
        );
        assert!(
            entries[1]
                .get("toolUseResult")
                .and_then(|value| value.get("progressMessages"))
                .is_none()
        );
    }

    #[test]
    fn prompt_id_is_stable_across_serializations() {
        let msg = message(MessageRole::User, "user-42", "hello", Vec::new());
        let first = transcript_entries(Path::new("/tmp/p"), "m", "s", &msg, None);
        let second = transcript_entries(Path::new("/tmp/p"), "m", "s", &msg, None);
        let id_of = |entries: &[Value]| {
            entries
                .last()
                .and_then(|e| e.get("promptId"))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        assert_eq!(
            id_of(&first),
            id_of(&second),
            "promptId must be stable across rewrites (not re-minted)"
        );
        assert!(id_of(&first).is_some_and(|id| !id.is_empty()));
    }

    #[test]
    fn second_tool_result_metadata_is_not_dropped_when_first_is_none() {
        // `[ToolResult{meta:None}, ToolResult{meta:Some}]` (parallel tools) must
        // keep the second block's toolUseResult rather than early-returning None.
        let msg = message(
            MessageRole::User,
            "user-2",
            "",
            vec![
                TranscriptBlock::ToolResult {
                    tool_use_id: "tool-a".to_string(),
                    content: "a".to_string(),
                    is_error: false,
                    metadata: None,
                },
                TranscriptBlock::ToolResult {
                    tool_use_id: "tool-b".to_string(),
                    content: "b".to_string(),
                    is_error: false,
                    metadata: Some(json!({ "status": "completed" }).to_string()),
                },
            ],
        );
        let entries = transcript_entries(Path::new("/tmp/p"), "m", "s", &msg, None);
        let user = entries.last().expect("user entry");
        assert_eq!(
            user.get("toolUseResult")
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str),
            Some("completed"),
            "second tool_result's metadata must survive"
        );
    }

    #[test]
    fn transcript_entries_builds_assistant_entry_with_model_and_usage() {
        let message = message(
            MessageRole::Assistant,
            "assistant-1",
            "",
            vec![TranscriptBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "Read".to_string(),
                input: r#"{"file_path":"README.md"}"#.to_string(),
            }],
        )
        .with_stop_reason("tool_use");

        let entries = transcript_entries(
            Path::new("/tmp/project"),
            "claude-sonnet-4",
            "session-1",
            &message,
            Some("user-1"),
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].get("type").and_then(Value::as_str),
            Some("assistant")
        );
        assert_eq!(
            entries[0]
                .get("message")
                .and_then(|message| message.get("model"))
                .and_then(Value::as_str),
            Some("claude-sonnet-4")
        );
        assert_eq!(
            entries[0]
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|block| block.get("input"))
                .and_then(|input| input.get("file_path"))
                .and_then(Value::as_str),
            Some("README.md")
        );
    }

    #[test]
    fn transcript_entries_builds_system_entry() {
        let message = message(MessageRole::System, "system-1", "be concise", Vec::new());

        let entries = transcript_entries(
            Path::new("/tmp/project"),
            "claude-sonnet-4",
            "session-1",
            &message,
            None,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].get("type").and_then(Value::as_str),
            Some("system")
        );
        assert_eq!(
            entries[0]
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str),
            Some("be concise")
        );
    }

    /// Every record we write is stamped with our provenance, and the `version`
    /// tail tracks the real crate version. It used to be the frozen literal
    /// `"orbcode-tui-parity"`, repeated at each of these call sites — a label
    /// for a project phase that had long since ended. Nothing reads the field
    /// back (see `pre_rename_provenance_values_still_decode`), so this is the
    /// only thing keeping the shape honest.
    #[test]
    fn every_role_is_stamped_with_versioned_orbcode_provenance() {
        for role in [
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::System,
        ] {
            let label = format!("{role:?}");
            let message = message(role, "id-1", "hello", Vec::new());
            let entries = transcript_entries(
                Path::new("/tmp/project"),
                "claude-sonnet-4",
                "session-1",
                &message,
                None,
            );
            assert!(!entries.is_empty(), "{label} produced no entry");

            for entry in &entries {
                assert_eq!(
                    entry.get("entrypoint").and_then(Value::as_str),
                    Some("orbcode"),
                    "{label} entry must carry our entrypoint"
                );
                let version = entry
                    .get("version")
                    .and_then(Value::as_str)
                    .expect("version present");
                assert_eq!(version, format!("orbcode-{}", env!("CARGO_PKG_VERSION")));
                assert!(
                    !version.contains("parity"),
                    "the retired project-phase label must not come back: {version}"
                );
            }
        }
    }

    #[test]
    fn progress_transcript_entry_preserves_supplied_identity_fields() {
        let entry = progress_transcript_entry(
            Path::new("/tmp/project"),
            "session-1",
            Some("parent-1"),
            "2026-01-01T00:00:00.000Z",
            json!({
                "uuid": "progress-1",
                "timestamp": "2026-01-01T00:00:01.000Z",
                "parentToolUseID": "tool-1",
            }),
        );

        assert_eq!(entry.get("type").and_then(Value::as_str), Some("progress"));
        assert_eq!(
            entry.get("uuid").and_then(Value::as_str),
            Some("progress-1")
        );
        assert_eq!(
            entry.get("timestamp").and_then(Value::as_str),
            Some("2026-01-01T00:00:01.000Z")
        );
        assert_eq!(
            entry.get("parentUuid").and_then(Value::as_str),
            Some("parent-1")
        );
        assert_eq!(
            entry.get("parentToolUseID").and_then(Value::as_str),
            Some("tool-1")
        );
        assert_eq!(
            entry.get("cwd").and_then(Value::as_str),
            Some("/tmp/project")
        );
        assert_eq!(
            entry.get("sessionId").and_then(Value::as_str),
            Some("session-1")
        );
    }

    #[test]
    fn progress_transcript_entry_wraps_non_object_progress() {
        let entry = progress_transcript_entry(
            Path::new("/tmp/project"),
            "session-1",
            None,
            "2026-01-01T00:00:00.000Z",
            json!("still working"),
        );

        assert_eq!(entry.get("type").and_then(Value::as_str), Some("progress"));
        assert_eq!(
            entry.get("timestamp").and_then(Value::as_str),
            Some("2026-01-01T00:00:00.000Z")
        );
        assert!(entry.get("parentUuid").is_some_and(Value::is_null));
        assert_eq!(
            entry.get("data").and_then(Value::as_str),
            Some("still working")
        );
    }
}
