use std::collections::{HashMap, HashSet};

use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};
use serde_json::{Value, json};

use super::COMPACT_SUMMARY_PREFIX;
use super::blocks::serialize_block_payload;

pub(crate) fn collect_progress_records(records: &[Value]) -> HashMap<String, Vec<Value>> {
    let mut grouped = HashMap::new();
    for record in records {
        if record.get("type").and_then(Value::as_str) != Some("progress") {
            continue;
        }
        let Some(parent_tool_use_id) = record
            .get("parentToolUseID")
            .or_else(|| record.get("parent_tool_use_id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        grouped
            .entry(parent_tool_use_id.to_string())
            .or_insert_with(Vec::new)
            .push(record.clone());
    }
    grouped
}

pub(crate) fn attach_tool_result_progress_metadata(
    blocks: &mut [TranscriptBlock],
    progress_by_parent_tool_use_id: &HashMap<String, Vec<Value>>,
) {
    for block in blocks.iter_mut() {
        let TranscriptBlock::ToolResult {
            tool_use_id,
            metadata,
            ..
        } = block
        else {
            continue;
        };
        let Some(progress_records) = progress_by_parent_tool_use_id.get(tool_use_id) else {
            continue;
        };

        let merged =
            merge_tool_result_metadata_with_progress(metadata.as_deref(), progress_records);
        *metadata = Some(merged);
    }
}

fn merge_tool_result_metadata_with_progress(
    existing: Option<&str>,
    progress_records: &[Value],
) -> String {
    let existing = existing
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or_else(|| Value::Object(Default::default()));
    let (base, existing_progress) = split_progress_messages_from_metadata(existing);
    let progress = Value::Array(combine_progress_records(
        existing_progress,
        progress_records,
    ));
    let mut base = base.unwrap_or_else(|| Value::Object(Default::default()));

    if let Some(object) = base.as_object_mut() {
        object.insert("progressMessages".to_string(), progress);
        return serialize_block_payload(Some(&base));
    }

    let wrapped = json!({
        "rawResult": base,
        "progressMessages": progress,
    });
    serialize_block_payload(Some(&wrapped))
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

fn combine_progress_records(existing: Vec<Value>, additional: &[Value]) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut combined = Vec::new();

    for record in existing.into_iter().chain(additional.iter().cloned()) {
        let key = progress_record_identity(&record);
        if seen.insert(key) {
            combined.push(record);
        }
    }

    combined
}

fn progress_record_identity(record: &Value) -> String {
    record.get("uuid").and_then(Value::as_str).map_or_else(
        || serde_json::to_string(record).unwrap_or_else(|_| format!("{record:?}")),
        |uuid| format!("uuid:{uuid}"),
    )
}

/// Drop messages before the last compact boundary (a synthetic System message
/// produced by auto-compact or explicit `/compact`). After compaction, those
/// messages are redundant — the boundary message carries the full summary — so
/// keeping them wastes memory and inflates the provider-visible token count on
/// resume.
///
/// The boundary is identified by its content prefix (see
/// [`COMPACT_SUMMARY_PREFIX`]). When multiple boundaries exist (successive
/// compaction rounds without a full rewrite), only the last one wins.
pub(crate) fn gc_pre_compact_messages(messages: &mut Vec<TranscriptMessage>) {
    let boundary_index = messages.iter().rposition(|msg| {
        msg.role == MessageRole::System && msg.content.starts_with(COMPACT_SUMMARY_PREFIX)
    });
    if let Some(idx) = boundary_index
        && idx > 0
    {
        messages.drain(..idx);
    }
}

#[cfg(test)]
mod tests {
    use super::super::decode_session_transcript_with_outcome;
    use orbcode_protocol::TranscriptBlock;
    use serde_json::{Value, json};

    #[test]
    fn hook_progress_record_merges_into_matching_tool_result() {
        let lines = [
            json!({
                "type": "assistant",
                "uuid": "assistant-hook",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_hook",
                        "name": "Bash",
                        "input": { "command": "echo hi" }
                    }],
                    "model": "claude-opus-4-7",
                },
            }),
            json!({
                "type": "progress",
                "uuid": "progress-hook-1",
                "parentToolUseID": "toolu_hook",
                "timestamp": "2026-01-01T00:00:01Z",
                "data": {
                    "type": "hook_progress",
                    "hookEvent": "PreToolUse",
                    "status": "running",
                },
            }),
            json!({
                "type": "user",
                "uuid": "user-hook-result",
                "timestamp": "2026-01-01T00:00:02Z",
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_hook",
                        "content": "hi",
                        "is_error": false,
                    }],
                },
            }),
        ];
        let body = lines
            .iter()
            .map(|line| serde_json::to_string(line).expect("serialize"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        let outcome = decode_session_transcript_with_outcome("s".to_string(), &body);
        let session = outcome.session.expect("session decoded");
        let progress = session
            .messages
            .iter()
            .flat_map(|m| &m.blocks)
            .find_map(|block| match block {
                TranscriptBlock::ToolResult { metadata, .. } => metadata.as_ref().and_then(|raw| {
                    serde_json::from_str::<Value>(raw)
                        .ok()
                        .and_then(|v| v.get("progressMessages")?.as_array().cloned())
                }),
                _ => None,
            })
            .expect("hook progress merged into tool result metadata");
        assert_eq!(progress.len(), 1, "the one hook progress record merged in");
    }

    // ---- GC pre-compact messages ----

    #[test]
    fn gc_drops_messages_before_compact_boundary() {
        let lines = [
            json!({
                "type": "user",
                "uuid": "old-user",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": { "role": "user", "content": "old question" },
                "cwd": "/tmp",
            }),
            json!({
                "type": "assistant",
                "uuid": "old-assistant",
                "timestamp": "2026-01-01T00:00:01Z",
                "message": { "role": "assistant", "content": [{ "type": "text", "text": "old answer" }], "model": "claude-opus-4-7" },
            }),
            json!({
                "type": "system",
                "uuid": "compact-boundary",
                "timestamp": "2026-01-01T00:01:00Z",
                "message": { "role": "system", "content": "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\nSummary:\nUser asked about the repo.\n\nTranscript: /tmp/test.jsonl" },
            }),
            json!({
                "type": "user",
                "uuid": "new-user",
                "timestamp": "2026-01-01T00:02:00Z",
                "message": { "role": "user", "content": "new question" },
                "cwd": "/tmp",
            }),
        ];
        let body = lines
            .iter()
            .map(|l| serde_json::to_string(l).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        let outcome = decode_session_transcript_with_outcome("gc-test".to_string(), &body);
        let session = outcome.session.expect("session decoded");

        assert_eq!(
            session.messages.len(),
            2,
            "only compact boundary + new user message remain"
        );
        assert_eq!(session.messages[0].id, "compact-boundary");
        assert_eq!(session.messages[1].id, "new-user");
    }

    #[test]
    fn gc_keeps_all_messages_when_no_compact_boundary() {
        let lines = [
            json!({
                "type": "user",
                "uuid": "user-1",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": { "role": "user", "content": "hello" },
                "cwd": "/tmp",
            }),
            json!({
                "type": "assistant",
                "uuid": "assistant-1",
                "timestamp": "2026-01-01T00:00:01Z",
                "message": { "role": "assistant", "content": [{ "type": "text", "text": "hi there" }], "model": "claude-opus-4-7" },
            }),
        ];
        let body = lines
            .iter()
            .map(|l| serde_json::to_string(l).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        let outcome = decode_session_transcript_with_outcome("gc-test".to_string(), &body);
        let session = outcome.session.expect("session decoded");

        assert_eq!(session.messages.len(), 2, "all messages preserved");
    }

    #[test]
    fn gc_noop_when_compact_boundary_is_first_message() {
        let lines = [
            json!({
                "type": "system",
                "uuid": "compact-boundary",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": { "role": "system", "content": "This session is being continued from a previous conversation that ran out of context. Summary here." },
            }),
            json!({
                "type": "user",
                "uuid": "user-1",
                "timestamp": "2026-01-01T00:00:01Z",
                "message": { "role": "user", "content": "continue" },
                "cwd": "/tmp",
            }),
        ];
        let body = lines
            .iter()
            .map(|l| serde_json::to_string(l).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        let outcome = decode_session_transcript_with_outcome("gc-test".to_string(), &body);
        let session = outcome.session.expect("session decoded");

        assert_eq!(
            session.messages.len(),
            2,
            "no messages dropped when boundary is first"
        );
        assert_eq!(session.messages[0].id, "compact-boundary");
    }

    #[test]
    fn gc_uses_last_compact_boundary_with_multiple_rounds() {
        let lines = [
            json!({
                "type": "user",
                "uuid": "very-old",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": { "role": "user", "content": "very old" },
                "cwd": "/tmp",
            }),
            json!({
                "type": "system",
                "uuid": "first-boundary",
                "timestamp": "2026-01-01T00:01:00Z",
                "message": { "role": "system", "content": "This session is being continued from a previous conversation that ran out of context. First round summary." },
            }),
            json!({
                "type": "user",
                "uuid": "mid-user",
                "timestamp": "2026-01-01T00:02:00Z",
                "message": { "role": "user", "content": "middle question" },
                "cwd": "/tmp",
            }),
            json!({
                "type": "system",
                "uuid": "second-boundary",
                "timestamp": "2026-01-01T00:03:00Z",
                "message": { "role": "system", "content": "This session is being continued from a previous conversation that ran out of context. Second round summary." },
            }),
            json!({
                "type": "user",
                "uuid": "latest-user",
                "timestamp": "2026-01-01T00:04:00Z",
                "message": { "role": "user", "content": "latest question" },
                "cwd": "/tmp",
            }),
        ];
        let body = lines
            .iter()
            .map(|l| serde_json::to_string(l).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        let outcome = decode_session_transcript_with_outcome("gc-test".to_string(), &body);
        let session = outcome.session.expect("session decoded");

        assert_eq!(
            session.messages.len(),
            2,
            "only last boundary + latest user remain"
        );
        assert_eq!(session.messages[0].id, "second-boundary");
        assert_eq!(session.messages[1].id, "latest-user");
    }

    #[test]
    fn gc_does_not_drop_messages_before_snip_boundary() {
        let lines = [
            json!({
                "type": "user",
                "uuid": "pre-snip-user",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": { "role": "user", "content": "normal message" },
                "cwd": "/tmp",
            }),
            json!({
                "type": "system",
                "subtype": "snip_boundary",
                "uuid": "snip-marker",
                "timestamp": "2026-01-01T00:01:00Z",
            }),
            json!({
                "type": "user",
                "uuid": "post-snip-user",
                "timestamp": "2026-01-01T00:02:00Z",
                "message": { "role": "user", "content": "after snip" },
                "cwd": "/tmp",
            }),
        ];
        let body = lines
            .iter()
            .map(|l| serde_json::to_string(l).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        let outcome = decode_session_transcript_with_outcome("gc-test".to_string(), &body);
        let session = outcome.session.expect("session decoded");

        assert_eq!(
            session.messages.len(),
            3,
            "snip boundary does not trigger GC -- pre-snip messages have value"
        );
    }
}
