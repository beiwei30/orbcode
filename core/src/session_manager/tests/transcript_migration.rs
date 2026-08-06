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

async fn write_transcript_lines(
    manager: &SessionManager,
    session_id: &str,
    lines: &[Value],
) -> std::path::PathBuf {
    let path = manager.transcript_store.path(session_id);
    tokio::fs::create_dir_all(path.parent().expect("transcript parent"))
        .await
        .expect("create transcript parent");
    let payload = lines
        .iter()
        .map(|line| serde_json::to_string(line).expect("serialize transcript line"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    tokio::fs::write(&path, payload)
        .await
        .expect("write transcript lines");
    path
}

fn parsed_transcript_lines(contents: &str) -> Vec<Value> {
    contents
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse transcript line"))
        .collect()
}

#[tokio::test]
async fn rewind_preserves_loaded_prompt_and_per_line_provenance() {
    let manager = test_manager().await;
    let session_id = "byte-fidelity-rewind";
    let path = write_transcript_lines(
        &manager,
        session_id,
        &[
            json!({
                "type": "user",
                "uuid": "rewind-user-1",
                "parentUuid": null,
                "timestamp": "2026-08-04T01:00:00.000Z",
                "promptId": "typescript-original-prompt",
                "gitBranch": "historical-main",
                "provider": "future-provider-v9",
                "message": {"role": "user", "content": "keep one"}
            }),
            json!({
                "type": "assistant",
                "uuid": "rewind-assistant-1",
                "parentUuid": "rewind-user-1",
                "timestamp": "2026-08-04T01:00:01.000Z",
                "gitBranch": null,
                "provider": null,
                "message": {
                    "role": "assistant",
                    "model": "claude-old",
                    "content": [{"type": "text", "text": "keep two"}]
                }
            }),
            json!({
                "type": "user",
                "uuid": "rewind-user-2",
                "parentUuid": "rewind-assistant-1",
                "timestamp": "2026-08-04T01:00:02.000Z",
                "promptId": null,
                "gitBranch": "discarded-branch",
                "provider": "anthropic",
                "message": {"role": "user", "content": "discard me"}
            }),
        ],
    )
    .await;
    manager
        .transcript_store
        .record_session_hints(
            session_id,
            orbcode_session_store::SessionWriteHints {
                git_branch: Some("current-branch".to_string()),
                provider: Some(ProviderId::OpenAi),
            },
        )
        .await;

    let rewound = manager
        .rewind_session(session_id, 2)
        .await
        .expect("rewind loaded transcript");
    assert_eq!(rewound.messages.len(), 2);
    let rewritten = parsed_transcript_lines(
        &tokio::fs::read_to_string(path)
            .await
            .expect("read rewound transcript"),
    );
    assert_eq!(rewritten.len(), 2);
    assert_eq!(rewritten[0]["promptId"], "typescript-original-prompt");
    assert_eq!(rewritten[0]["gitBranch"], "historical-main");
    assert_eq!(rewritten[0]["provider"], "future-provider-v9");
    assert!(rewritten[1].get("promptId").is_none());
    assert_eq!(rewritten[1]["gitBranch"], Value::Null);
    assert_eq!(rewritten[1]["provider"], Value::Null);
}

#[tokio::test]
async fn rewind_and_fork_preserve_point_in_time_goal_state() {
    let manager = test_manager().await;
    let session_id = "goal-point-in-time-core";
    let path = write_transcript_lines(
        &manager,
        session_id,
        &[
            json!({
                "type": "user",
                "uuid": "goal-user-1",
                "parentUuid": null,
                "sessionId": session_id,
                "timestamp": "2026-08-05T12:00:00.000Z",
                "message": {"role": "user", "content": "first"}
            }),
            json!({
                "type": "goal",
                "goalId": "goal-original",
                "revision": 1,
                "sessionId": session_id,
                "objective": "Preserve the selected state",
                "status": "active",
                "tokenBudget": 1000,
                "tokensUsed": 10,
                "elapsedSeconds": 1,
                "createdAt": "2026-08-05T12:00:01.000Z",
                "updatedAt": "2026-08-05T12:00:01.000Z",
                "timestamp": "2026-08-05T12:00:01.000Z"
            }),
            json!({
                "type": "assistant",
                "uuid": "goal-assistant-1",
                "parentUuid": "goal-user-1",
                "sessionId": session_id,
                "timestamp": "2026-08-05T12:00:02.000Z",
                "message": {"role": "assistant", "content": "first response"}
            }),
            json!({
                "type": "goal",
                "goalId": "goal-original",
                "revision": 2,
                "sessionId": session_id,
                "objective": "Preserve the selected state",
                "status": "blocked",
                "tokenBudget": 1000,
                "tokensUsed": 25,
                "elapsedSeconds": 3,
                "createdAt": "2026-08-05T12:00:01.000Z",
                "updatedAt": "2026-08-05T12:00:03.000Z",
                "stopReason": "waiting for input",
                "timestamp": "2026-08-05T12:00:03.000Z"
            }),
            json!({
                "type": "user",
                "uuid": "goal-user-2",
                "parentUuid": "goal-assistant-1",
                "sessionId": session_id,
                "timestamp": "2026-08-05T12:00:04.000Z",
                "message": {"role": "user", "content": "discard"}
            }),
            json!({
                "type": "goal-cleared",
                "goalId": "goal-original",
                "revision": 3,
                "sessionId": session_id,
                "timestamp": "2026-08-05T12:00:05.000Z"
            }),
        ],
    )
    .await;

    let rewound = manager
        .rewind_session(session_id, 2)
        .await
        .expect("rewind to blocked snapshot");
    let rewound_goal = rewound.goal.as_ref().expect("goal restored at boundary");
    assert_eq!(rewound_goal.revision, 2);
    assert_eq!(
        rewound_goal.status,
        orbcode_protocol::SessionGoalStatus::Blocked
    );
    let rewritten = tokio::fs::read_to_string(&path).await.expect("read rewind");
    assert!(!rewritten.contains("goal-cleared"));

    let fork = manager
        .fork_session(session_id, Some("goal fork".to_string()), None)
        .await
        .expect("fork goal session");
    let fork_goal = fork.goal.as_ref().expect("forked goal");
    assert_ne!(fork_goal.goal_id, rewound_goal.goal_id);
    assert_eq!(fork_goal.session_id, fork.session_id);
    assert_eq!(fork_goal.revision, 1);
    assert_eq!(fork_goal.status, rewound_goal.status);
    assert_eq!(fork_goal.tokens_used, 25);

    let reloaded_fork = manager
        .load_session(&fork.session_id)
        .await
        .expect("reload fork");
    assert_eq!(reloaded_fork.goal, fork.goal);
}

#[tokio::test]
async fn repair_keeps_loaded_provenance_and_decorates_only_synthetic_result() {
    let manager = test_manager().await;
    let session_id = "byte-fidelity-repair";
    let path = write_transcript_lines(
        &manager,
        session_id,
        &[
            json!({
                "type": "user",
                "uuid": "repair-user",
                "parentUuid": null,
                "timestamp": "2026-08-04T02:00:00.000Z",
                "promptId": "repair-original-prompt",
                "gitBranch": "repair-old-branch",
                "provider": "future-provider-v9",
                "message": {"role": "user", "content": "run the tool"}
            }),
            json!({
                "type": "assistant",
                "uuid": "repair-assistant",
                "parentUuid": "repair-user",
                "timestamp": "2026-08-04T02:00:01.000Z",
                "gitBranch": null,
                "message": {
                    "role": "assistant",
                    "model": "claude-old",
                    "content": [{"type": "tool_use", "id": "repair-tool", "name": "Read", "input": {"path": "README.md"}}]
                }
            }),
        ],
    )
    .await;
    manager
        .transcript_store
        .record_session_hints(
            session_id,
            orbcode_session_store::SessionWriteHints {
                git_branch: Some("repair-current-branch".to_string()),
                provider: Some(ProviderId::OpenAi),
            },
        )
        .await;
    let mut session = manager
        .load_session(session_id)
        .await
        .expect("load repair source");
    session.messages = repair_missing_tool_results(session.messages);
    assert_eq!(session.messages.len(), 3);
    assert!(session.messages[2].transcript_provenance.is_none());
    manager
        .transcript_store
        .persist_full_session(&session)
        .await
        .expect("persist repaired transcript");

    let rewritten = parsed_transcript_lines(
        &tokio::fs::read_to_string(path)
            .await
            .expect("read repaired transcript"),
    );
    assert_eq!(rewritten[0]["promptId"], "repair-original-prompt");
    assert_eq!(rewritten[0]["gitBranch"], "repair-old-branch");
    assert_eq!(rewritten[0]["provider"], "future-provider-v9");
    assert_eq!(rewritten[1]["gitBranch"], Value::Null);
    assert!(rewritten[1].get("provider").is_none());
    let synthetic = rewritten.last().expect("synthetic tool result line");
    assert!(synthetic.get("promptId").and_then(Value::as_str).is_some());
    assert_eq!(synthetic["gitBranch"], "repair-current-branch");
    assert_eq!(synthetic["provider"], "openai");
    assert_eq!(
        synthetic["message"]["content"][0]["content"],
        MISSING_TOOL_RESULT
    );
}

#[tokio::test]
async fn fork_regenerates_line_identity_but_keeps_structured_tool_result_semantics() {
    let manager = test_manager().await;
    let session_id = "byte-fidelity-fork";
    let original_tool_content = json!([
        {"type": "text", "text": "first"},
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AA=="}},
        {"type": "structured", "payload": {"answer": 42}}
    ]);
    write_transcript_lines(
        &manager,
        session_id,
        &[
            json!({
                "type": "user",
                "uuid": "fork-user",
                "parentUuid": null,
                "timestamp": "2026-08-04T03:00:00.000Z",
                "promptId": "fork-source-prompt",
                "gitBranch": "fork-source-branch",
                "provider": "future-provider-v9",
                "message": {"role": "user", "content": "inspect"}
            }),
            json!({
                "type": "assistant",
                "uuid": "fork-assistant",
                "parentUuid": "fork-user",
                "timestamp": "2026-08-04T03:00:01.000Z",
                "provider": "future-provider-v9",
                "message": {
                    "role": "assistant",
                    "model": "claude-old",
                    "content": [{"type": "tool_use", "id": "fork-tool", "name": "Inspect", "input": {}}]
                }
            }),
            json!({
                "type": "user",
                "uuid": "fork-result",
                "parentUuid": "fork-assistant",
                "timestamp": "2026-08-04T03:00:02.000Z",
                "promptId": null,
                "provider": "future-provider-v9",
                "message": {
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": "fork-tool", "content": original_tool_content, "is_error": false}]
                }
            }),
        ],
    )
    .await;

    let fork = manager
        .fork_session(session_id, Some("fidelity fork".to_string()), None)
        .await
        .expect("fork transcript");
    assert!(
        fork.messages
            .iter()
            .all(|message| message.transcript_provenance.is_none())
    );
    assert_ne!(fork.messages[0].id, "fork-user");
    let fork_content = fork.messages[2]
        .blocks
        .iter()
        .find_map(|block| match block {
            TranscriptBlock::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .expect("forked tool result");
    assert!(matches!(
        fork_content.loaded_field(),
        Some(orbcode_protocol::TranscriptJsonField::Value(Value::Array(items)))
            if items.len() == 3
    ));

    let fork_lines = parsed_transcript_lines(
        &tokio::fs::read_to_string(manager.transcript_store.path(&fork.session_id))
            .await
            .expect("read fork transcript"),
    );
    let fork_user = fork_lines
        .iter()
        .find(|line| line["uuid"] == fork.messages[0].id)
        .expect("forked root user line");
    assert_ne!(fork_user["promptId"], "fork-source-prompt");
    assert_eq!(
        fork_user["provider"],
        manager.effective_config().default_provider.as_str()
    );
    let fork_result = fork_lines
        .iter()
        .find(|line| line["uuid"] == fork.messages[2].id)
        .expect("forked tool result line");
    assert_eq!(
        fork_result["message"]["content"][0]["content"],
        original_tool_content
    );

    let request = manager
        .provider_request_for_session(
            &fork.session_id,
            "continue",
            manager.context_preview().await,
            &[],
            false,
            false,
        )
        .await
        .expect("fork provider request");
    assert!(request.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(
                block,
                TranscriptBlock::ToolResult { content, .. }
                    if matches!(
                        content.loaded_field(),
                        Some(orbcode_protocol::TranscriptJsonField::Value(Value::Array(items)))
                            if items.len() == 3
                    )
            )
        })
    }));
}

#[tokio::test]
async fn compact_before_current_prompt_preserves_loaded_suffix_provenance() {
    let manager = test_manager().await;
    let session_id = "byte-fidelity-partial-compact";
    let path = write_transcript_lines(
        &manager,
        session_id,
        &[
            json!({
                "type": "user",
                "uuid": "compact-old-user",
                "parentUuid": null,
                "timestamp": "2026-08-04T04:00:00.000Z",
                "promptId": "compact-old-prompt",
                "gitBranch": "old-branch",
                "provider": "future-provider-v9",
                "message": {"role": "user", "content": "old prompt"}
            }),
            json!({
                "type": "assistant",
                "uuid": "compact-old-assistant",
                "parentUuid": "compact-old-user",
                "timestamp": "2026-08-04T04:00:01.000Z",
                "gitBranch": "old-answer-branch",
                "provider": "future-provider-v9",
                "message": {
                    "role": "assistant",
                    "model": "claude-old",
                    "content": [{"type": "text", "text": "old answer"}]
                }
            }),
            json!({
                "type": "user",
                "uuid": "compact-current-user",
                "parentUuid": "compact-old-assistant",
                "timestamp": "2026-08-04T04:00:02.000Z",
                "promptId": "compact-current-original-prompt",
                "gitBranch": null,
                "provider": "future-current-provider",
                "message": {"role": "user", "content": "current prompt"}
            }),
        ],
    )
    .await;
    manager
        .transcript_store
        .record_session_hints(
            session_id,
            orbcode_session_store::SessionWriteHints {
                git_branch: Some("compact-process-branch".to_string()),
                provider: Some(ProviderId::OpenAi),
            },
        )
        .await;

    let result = manager
        .compact_session_before_current_prompt(session_id, "current prompt")
        .await
        .expect("partial compaction")
        .expect("compaction ran");
    assert_eq!(result.session.messages.len(), 2);
    assert!(result.session.messages[0].transcript_provenance.is_none());
    assert_eq!(
        result.session.messages[1]
            .transcript_provenance
            .as_ref()
            .expect("retained loaded suffix")
            .prompt_id,
        orbcode_protocol::TranscriptJsonField::Value(json!("compact-current-original-prompt"))
    );

    let rewritten = parsed_transcript_lines(
        &tokio::fs::read_to_string(path)
            .await
            .expect("read partially compacted transcript"),
    );
    assert!(
        rewritten
            .iter()
            .all(|line| line["uuid"] != "compact-old-user"
                && line["uuid"] != "compact-old-assistant")
    );
    let summary = &rewritten[0];
    assert_eq!(summary["gitBranch"], "compact-process-branch");
    assert_eq!(summary["provider"], "openai");
    let suffix = rewritten
        .iter()
        .find(|line| line["uuid"] == "compact-current-user")
        .expect("retained current prompt");
    assert_eq!(suffix["promptId"], "compact-current-original-prompt");
    assert_eq!(suffix["gitBranch"], Value::Null);
    assert_eq!(suffix["provider"], "future-current-provider");
}

#[tokio::test]
async fn full_compaction_replaces_history_with_new_hint_decorated_record() {
    let manager = test_manager().await;
    let session_id = "byte-fidelity-full-compact";
    let path = write_transcript_lines(
        &manager,
        session_id,
        &[
            json!({
                "type": "user",
                "uuid": "full-compact-user",
                "parentUuid": null,
                "timestamp": "2026-08-04T05:00:00.000Z",
                "promptId": "deleted-prompt",
                "gitBranch": "deleted-branch",
                "provider": "future-provider-v9",
                "message": {"role": "user", "content": "history to summarize"}
            }),
            json!({
                "type": "assistant",
                "uuid": "full-compact-assistant",
                "parentUuid": "full-compact-user",
                "timestamp": "2026-08-04T05:00:01.000Z",
                "provider": "future-provider-v9",
                "message": {
                    "role": "assistant",
                    "model": "claude-old",
                    "content": [{"type": "text", "text": "answer to summarize"}]
                }
            }),
            json!({
                "type": "goal",
                "goalId": "full-compact-goal",
                "revision": 3,
                "sessionId": session_id,
                "objective": "Survive full compaction",
                "status": "active",
                "tokenBudget": 10000,
                "tokensUsed": 321,
                "elapsedSeconds": 12,
                "createdAt": "2026-08-04T05:00:00.000Z",
                "updatedAt": "2026-08-04T05:00:02.000Z",
                "lastGoalTurnId": "full-compact-turn-2",
                "timestamp": "2026-08-04T05:00:02.000Z"
            }),
        ],
    )
    .await;
    manager
        .transcript_store
        .record_session_hints(
            session_id,
            orbcode_session_store::SessionWriteHints {
                git_branch: Some("full-compact-current".to_string()),
                provider: Some(ProviderId::OpenAi),
            },
        )
        .await;

    let result = manager
        .compact_session(session_id)
        .await
        .expect("full compact");
    assert_eq!(result.session.messages.len(), 1);
    assert_eq!(
        result
            .session
            .goal
            .as_ref()
            .map(|goal| goal.goal_id.as_str()),
        Some("full-compact-goal")
    );
    assert!(result.session.messages[0].transcript_provenance.is_none());
    let rewritten = parsed_transcript_lines(
        &tokio::fs::read_to_string(path)
            .await
            .expect("read full compaction"),
    );
    assert_eq!(rewritten.len(), 2);
    assert_ne!(rewritten[0]["uuid"], "full-compact-user");
    assert_ne!(rewritten[0]["uuid"], "full-compact-assistant");
    assert!(rewritten[0].get("promptId").is_none());
    assert_eq!(rewritten[0]["gitBranch"], "full-compact-current");
    assert_eq!(rewritten[0]["provider"], "openai");
    assert_eq!(rewritten[1]["type"], "goal");
    assert_eq!(rewritten[1]["goalId"], "full-compact-goal");
    let reloaded = manager
        .load_session(session_id)
        .await
        .expect("reload compacted goal");
    assert_eq!(reloaded.goal, result.session.goal);
}

#[tokio::test]
async fn background_snapshot_uses_source_aware_policy_for_mixed_messages() {
    let manager = test_manager().await;
    let loaded = orbcode_session_store::decode_session_transcript_with_outcome(
        "background-loaded".to_string(),
        &serde_json::to_string(&json!({
            "type": "user",
            "uuid": "background-loaded-user",
            "parentUuid": null,
            "timestamp": "2026-08-04T06:00:00.000Z",
            "promptId": "background-original-prompt",
            "gitBranch": null,
            "provider": "future-provider-v9",
            "message": {"role": "user", "content": "loaded child history"}
        }))
        .expect("serialize loaded child line"),
    )
    .session
    .expect("decode loaded child message")
    .messages
    .remove(0);
    let new_message = TranscriptMessage::new(MessageRole::User, "new child turn");
    let new_id = new_message.id.clone();
    let child_id = "byte-fidelity-background-child";
    let agent = orbcode_tools::AgentToolInput {
        description: "fidelity child".to_string(),
        prompt: "continue child".to_string(),
        subagent_type: Some("general-purpose".to_string()),
        run_in_background: true,
    };

    manager
        .persist_child_agent_transcript_snapshot(
            child_id,
            &agent,
            "general-purpose",
            None,
            &[loaded, new_message],
        )
        .await;
    let path = manager.child_session_store.transcript_path_for(child_id);
    let lines = parsed_transcript_lines(
        &tokio::fs::read_to_string(path)
            .await
            .expect("read background snapshot"),
    );
    let historical = lines
        .iter()
        .find(|line| line["uuid"] == "background-loaded-user")
        .expect("loaded background line");
    assert_eq!(historical["promptId"], "background-original-prompt");
    assert_eq!(historical["gitBranch"], Value::Null);
    assert_eq!(historical["provider"], "future-provider-v9");
    let fresh = lines
        .iter()
        .find(|line| line["uuid"] == new_id)
        .expect("fresh background line");
    assert!(fresh.get("promptId").and_then(Value::as_str).is_some());
    assert_eq!(
        fresh["provider"],
        manager.effective_config().default_provider.as_str()
    );
}
