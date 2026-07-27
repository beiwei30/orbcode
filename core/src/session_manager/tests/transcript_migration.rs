use super::support::*;
use super::*;
use orbcode_session_store::TranscriptDecodeOutcome;

/// Loads a TypeScript-style transcript that interleaves unknown record
/// types (`file-history-snapshot`, `content-replacement`, `attachment`),
/// a legacy slash-command user echo (`<command-name>...`), and a tool
/// progress record. The loader must skip unknowns without panicking and
/// keep the user/assistant chain intact so resume + provider requests
/// still see a valid alternating message sequence.
#[tokio::test]
async fn loads_typescript_transcript_with_unknown_entries_and_legacy_command() {
    let manager = test_manager().await;
    let session_id = "ts-migration-unknown-entries";
    let transcript_path = manager.transcript_store.path(session_id);
    tokio::fs::create_dir_all(transcript_path.parent().expect("project dir"))
        .await
        .expect("create project dir");

    let lines = [
        // Plain string user content — TypeScript writes this for typed prompts.
        json!({
            "parentUuid": Value::Null,
            "isSidechain": false,
            "type": "user",
            "uuid": "user-1",
            "timestamp": "2026-05-23T01:00:00.000Z",
            "userType": "external",
            "sessionId": session_id,
            "version": "1.0.0",
            "message": {
                "role": "user",
                "content": "open the readme"
            }
        }),
        // Unknown TypeScript metadata record — loader must skip.
        json!({
            "type": "file-history-snapshot",
            "messageId": "user-1",
            "snapshot": { "files": [] },
            "isSnapshotUpdate": false
        }),
        // Assistant emits a tool_use; loader keeps the block.
        json!({
            "parentUuid": "user-1",
            "isSidechain": false,
            "type": "assistant",
            "uuid": "assistant-1",
            "timestamp": "2026-05-23T01:00:01.000Z",
            "sessionId": session_id,
            "message": {
                "id": "msg_legacy_1",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "Read",
                        "input": { "file_path": "README.md" }
                    }
                ],
                "stop_reason": "tool_use"
            }
        }),
        // Standalone progress tied to tool-1; loader merges into metadata.
        json!({
            "type": "progress",
            "uuid": "progress-1",
            "timestamp": "2026-05-23T01:00:01.500Z",
            "parentToolUseID": "tool-1",
            "data": { "type": "read_progress", "status": "Reading file" }
        }),
        // Another unknown metadata record between progress and tool result.
        json!({
            "type": "content-replacement",
            "sessionId": session_id,
            "replacements": []
        }),
        // User tool_result for tool-1.
        json!({
            "parentUuid": "assistant-1",
            "isSidechain": false,
            "type": "user",
            "uuid": "user-2",
            "timestamp": "2026-05-23T01:00:02.000Z",
            "sessionId": session_id,
            "toolUseResult": {
                "status": "completed",
                "fileLines": 12
            },
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": "# Project README\n",
                        "is_error": false
                    }
                ]
            }
        }),
        // Legacy slash-command echo: meta user with <command-name> markers
        // wrapped as plain text. Older TS transcripts wrote these even for
        // local commands that never reached the model.
        json!({
            "parentUuid": "user-2",
            "isSidechain": false,
            "type": "user",
            "uuid": "user-3",
            "timestamp": "2026-05-23T01:00:03.000Z",
            "isMeta": true,
            "sessionId": session_id,
            "message": {
                "role": "user",
                "content": "<command-name>/help</command-name>\n<command-message>help</command-message>\n<command-args></command-args>"
            }
        }),
        // Another unknown TS-only entry.
        json!({
            "type": "attachment",
            "uuid": "attachment-1",
            "timestamp": "2026-05-23T01:00:03.500Z",
            "attachment": { "type": "selected_lines", "filename": "README.md" }
        }),
        // Final assistant text response.
        json!({
            "parentUuid": "user-3",
            "isSidechain": false,
            "type": "assistant",
            "uuid": "assistant-2",
            "timestamp": "2026-05-23T01:00:04.000Z",
            "sessionId": session_id,
            "message": {
                "id": "msg_legacy_2",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4",
                "content": [
                    { "type": "text", "text": "Here are the available commands." }
                ],
                "stop_reason": "end_turn"
            }
        }),
    ];

    let payload = lines
        .iter()
        .map(|value| serde_json::to_string(value).expect("serialize transcript line"))
        .collect::<Vec<_>>()
        .join("\n");
    tokio::fs::write(&transcript_path, format!("{payload}\n"))
        .await
        .expect("write transcript fixture");

    let session = manager
        .load_session(session_id)
        .await
        .expect("load TS-style transcript");

    let roles_and_content = session
        .messages
        .iter()
        .map(|m| (m.role.clone(), m.content.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(roles_and_content.len(), 5, "{roles_and_content:?}");
    assert_eq!(roles_and_content[0].0, MessageRole::User);
    assert_eq!(roles_and_content[0].1, "open the readme");
    assert_eq!(roles_and_content[1].0, MessageRole::Assistant);
    assert!(matches!(
        session.messages[1].blocks.as_slice(),
        [TranscriptBlock::ToolUse { id, name, .. }] if id == "tool-1" && name == "Read"
    ));
    assert_eq!(roles_and_content[2].0, MessageRole::User);
    let tool_result_metadata = match &session.messages[2].blocks[0] {
        TranscriptBlock::ToolResult {
            tool_use_id,
            metadata,
            ..
        } => {
            assert_eq!(tool_use_id, "tool-1");
            metadata.clone().expect("tool result metadata")
        }
        other => panic!("expected tool_result block, got {other:?}"),
    };
    let parsed_metadata: Value =
        serde_json::from_str(&tool_result_metadata).expect("parse tool result metadata");
    assert_eq!(
        parsed_metadata.get("status").and_then(Value::as_str),
        Some("completed"),
    );
    let progress = parsed_metadata
        .get("progressMessages")
        .and_then(Value::as_array)
        .expect("progress merged into metadata");
    assert_eq!(progress.len(), 1);
    assert_eq!(
        progress[0].get("uuid").and_then(Value::as_str),
        Some("progress-1"),
    );
    assert_eq!(roles_and_content[3].0, MessageRole::User);
    assert!(
        roles_and_content[3]
            .1
            .contains("<command-name>/help</command-name>"),
        "legacy slash-command echo content preserved: {:?}",
        roles_and_content[3].1
    );
    assert_eq!(roles_and_content[4].0, MessageRole::Assistant);
    assert_eq!(roles_and_content[4].1, "Here are the available commands.");

    // Resume + provider-visible request must still produce a valid sequence
    // with the existing tool_result preserved (no missing-result repair).
    let request = manager
        .provider_request_for_session(
            session_id,
            "thanks",
            manager.context_preview().await,
            &[],
            false,
            false,
        )
        .await
        .expect("provider request from migrated transcript");

    assert_eq!(request.session_id, session_id);
    assert_eq!(request.prompt, "thanks");
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.blocks.iter().any(
                |block| matches!(block, TranscriptBlock::ToolUse { id, .. } if id == "tool-1")
            )),
        "tool_use should survive migration"
    );
    let preserved_tool_result = request
        .messages
        .iter()
        .find_map(|message| {
            message.blocks.iter().find_map(|block| match block {
                TranscriptBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    ..
                } if tool_use_id == "tool-1" => Some((content.clone(), *is_error)),
                _ => None,
            })
        })
        .expect("tool_result should survive migration");
    assert_eq!(preserved_tool_result.0, "# Project README\n");
    assert!(!preserved_tool_result.1);
    assert!(
        request
            .messages
            .iter()
            .all(|message| !message.content.contains(MISSING_TOOL_RESULT)),
        "no missing tool result repair should be needed"
    );
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.role == MessageRole::Assistant
                && message.content == "Here are the available commands."),
        "final assistant response remains visible"
    );
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.role == MessageRole::User
                && message.content.contains("<command-name>")),
        "legacy slash-command echo remains in the chain"
    );
}

/// Loads a TypeScript-style transcript whose last record was truncated
/// mid-write (no trailing newline) and which contains a `redacted_thinking`
/// block plus a `compact-boundary` record the Rust decoder has no native
/// handling for. The loader must:
///
/// * Surface a `trailing_partial_line` via the decode outcome so the
///   doctor can flag it.
/// * Still decode every well-formed line before the truncation, including
///   the `redacted_thinking` block (preserved as text since Rust does not
///   yet model the redacted variant separately) and skip the unknown
///   `compact-boundary` record without dropping later messages.
/// * Resume cleanly through the SessionManager and produce a valid
///   provider request — the truncated trailing record must not be fused
///   with the next append.
#[tokio::test]
async fn loads_typescript_transcript_with_truncated_tail_and_unknown_compact_boundary() {
    let manager = test_manager().await;
    let session_id = "ts-migration-truncated-tail";
    let transcript_path = manager.transcript_store.path(session_id);
    tokio::fs::create_dir_all(transcript_path.parent().expect("project dir"))
        .await
        .expect("create project dir");

    let lines = [
        json!({
            "parentUuid": Value::Null,
            "isSidechain": false,
            "type": "user",
            "uuid": "user-1",
            "timestamp": "2026-05-24T01:00:00.000Z",
            "userType": "external",
            "sessionId": session_id,
            "version": "1.0.0",
            "message": { "role": "user", "content": "summarize the design" }
        }),
        // Assistant emits a redacted_thinking block (TS preserves these
        // for safety review). Rust currently surfaces them as the
        // underlying signature text — the important contract is that the
        // surrounding text block survives intact.
        json!({
            "parentUuid": "user-1",
            "isSidechain": false,
            "type": "assistant",
            "uuid": "assistant-1",
            "timestamp": "2026-05-24T01:00:01.000Z",
            "sessionId": session_id,
            "message": {
                "id": "msg_legacy_3",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4",
                "content": [
                    {
                        "type": "thinking",
                        "thinking": "considering tradeoffs",
                        "signature": "sig-1"
                    },
                    {
                        "type": "redacted_thinking",
                        "data": "AbCdEf=="
                    },
                    { "type": "text", "text": "Design summary." }
                ],
                "stop_reason": "end_turn"
            }
        }),
        // Unknown TS-only compaction marker — loader must skip.
        json!({
            "type": "compact-boundary",
            "uuid": "compact-1",
            "sessionId": session_id,
            "timestamp": "2026-05-24T01:00:02.000Z",
            "data": { "reason": "manual" }
        }),
        json!({
            "parentUuid": "assistant-1",
            "isSidechain": false,
            "type": "user",
            "uuid": "user-2",
            "timestamp": "2026-05-24T01:00:03.000Z",
            "sessionId": session_id,
            "message": { "role": "user", "content": "thanks" }
        }),
    ];

    let mut payload = lines
        .iter()
        .map(|value| serde_json::to_string(value).expect("serialize transcript line"))
        .collect::<Vec<_>>()
        .join("\n");
    payload.push('\n');
    // Now append a half-written record — what you'd see after a process
    // crash mid-flush. Note: no trailing newline.
    payload.push_str("{\"type\":\"assistant\",\"uuid\":\"assistant-2\",\"timestamp\":\"2026-");
    tokio::fs::write(&transcript_path, &payload)
        .await
        .expect("write truncated transcript fixture");

    // The decoder surfaces the partial-line signal via the outcome API so
    // the doctor can warn the user. The well-formed prefix still loads.
    let raw = tokio::fs::read_to_string(&transcript_path)
        .await
        .expect("read transcript");
    let TranscriptDecodeOutcome {
        skipped_line_count,
        trailing_partial_line,
        session,
    } = orbcode_session_store::decode_session_transcript_with_outcome(session_id.to_string(), &raw);
    assert!(trailing_partial_line, "trailing partial line flagged");
    assert!(skipped_line_count >= 1, "partial line counted as skipped");
    let _ = session.expect("session still decodes from valid prefix");

    let session = manager
        .load_session(session_id)
        .await
        .expect("load truncated TS-style transcript");

    let roles_and_content = session
        .messages
        .iter()
        .map(|m| (m.role.clone(), m.content.as_str()))
        .collect::<Vec<_>>();
    // 3 fully-decoded records — user, assistant (with text), follow-up user.
    assert_eq!(roles_and_content.len(), 3, "{roles_and_content:?}");
    assert_eq!(roles_and_content[0].0, MessageRole::User);
    assert_eq!(roles_and_content[0].1, "summarize the design");
    assert_eq!(roles_and_content[1].0, MessageRole::Assistant);
    assert!(
        roles_and_content[1].1.contains("Design summary."),
        "assistant text block survives compact-boundary skip: {:?}",
        roles_and_content[1].1
    );
    assert_eq!(roles_and_content[2].0, MessageRole::User);
    assert_eq!(roles_and_content[2].1, "thanks");

    // Submitting a new prompt after the truncated tail must heal the file
    // boundary so the next append does not fuse with the partial record.
    manager
        .transcript_store
        .append_message_line(
            session_id,
            &TranscriptMessage::new(MessageRole::User, "follow up"),
            None,
        )
        .await
        .expect("append after truncation");
    let healed = tokio::fs::read_to_string(&transcript_path)
        .await
        .expect("read healed transcript");
    // Last well-formed line is "follow up" — and parses standalone.
    let last_line = healed
        .lines()
        .last()
        .expect("transcript has at least one line");
    let parsed_last: Value =
        serde_json::from_str(last_line).expect("post-heal append parses standalone");
    assert_eq!(
        parsed_last.get("type").and_then(Value::as_str),
        Some("user")
    );

    // Provider request reconstruction must still succeed — no missing
    // tool_result repair (no tool_use was present) and the truncated tail
    // contributes nothing.
    let request = manager
        .provider_request_for_session(
            session_id,
            "ack",
            manager.context_preview().await,
            &[],
            false,
            false,
        )
        .await
        .expect("provider request from truncated transcript");
    assert_eq!(request.session_id, session_id);
    assert!(
        request
            .messages
            .iter()
            .all(|message| !message.content.contains(MISSING_TOOL_RESULT)),
        "no spurious missing tool result repair triggered",
    );
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.role == MessageRole::Assistant
                && message.content.contains("Design summary.")),
        "assistant text remains visible in provider chain",
    );
}
