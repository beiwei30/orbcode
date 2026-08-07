//! TypeScript-vs-Rust golden compatibility tests.
//!
//! These tests load the shared fixtures bundled in the `orbcode-compat-fixtures`
//! crate, decode them through the real `orbcode-session-store` transcript loader,
//! and assert that message shape, parent chains, tool blocks, progress
//! metadata, corrupt-line recovery, and reconstructed provider requests all
//! match the TypeScript reference. Every test name is prefixed `compat_` so the
//! whole suite runs via `cargo test --workspace compat`.

use orbcode_compat_fixtures::{
    FixtureCategory, fixtures_root, load_category, load_dir, load_named, normalize_line,
};
use orbcode_model_provider::{
    ProviderRequest, ProviderRequestOptions, build_anthropic_request_body,
};
use orbcode_protocol::{
    EffortLevel, MessageRole, PermissionRequest, ProviderId, SessionRecord, TranscriptBlock,
    TranscriptJsonField, TurnContext,
};
use orbcode_session_store::{
    PERSISTED_OUTPUT_CLOSING_TAG, PERSISTED_OUTPUT_TAG, SessionStore, SessionWriteHints,
    decode_session_transcript_with_outcome,
};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn decode_transcript_fixture(name: &str) -> SessionRecord {
    let fixture = load_named(FixtureCategory::Transcripts, name)
        .unwrap_or_else(|| panic!("transcript fixture '{name}' not found"));
    decode_session_transcript_with_outcome(name.to_string(), &fixture.contents)
        .session
        .unwrap_or_else(|| panic!("transcript fixture '{name}' decoded to no session"))
}

#[test]
fn compat_goal_lifecycle_snapshot_and_tombstone_are_ordered_metadata() {
    let session = decode_transcript_fixture("goal_lifecycle");
    assert!(session.goal.is_none(), "the later tombstone must win");
    assert_eq!(session.messages.len(), 2);
    assert_eq!(session.goal_transcript_records.len(), 5);
    assert_eq!(
        session
            .goal_transcript_records
            .iter()
            .map(|record| record.after_message_count)
            .collect::<Vec<_>>(),
        vec![1, 1, 2, 2, 2]
    );
    assert_eq!(
        session.goal_transcript_records[0].value["futureAccounting"]["version"],
        2
    );
}

#[test]
fn compat_goal_point_in_time_fixture_pins_rewind_boundaries() {
    let session = decode_transcript_fixture("goal_point_in_time");
    assert!(
        session.goal.is_none(),
        "final tombstone wins at full length"
    );
    assert_eq!(session.messages.len(), 3);
    assert_eq!(
        session
            .goal_transcript_records
            .iter()
            .map(|record| record.after_message_count)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );

    let mut at_first_boundary = session.clone();
    at_first_boundary.messages.truncate(1);
    at_first_boundary.rewind_goal_state(1);
    let first_goal = at_first_boundary.goal.expect("first snapshot visible");
    assert_eq!(first_goal.revision, 1);
    assert_eq!(first_goal.objective, "State at the first boundary");

    let mut at_second_boundary = session;
    at_second_boundary.messages.truncate(2);
    at_second_boundary.rewind_goal_state(2);
    let second_goal = at_second_boundary.goal.expect("second snapshot visible");
    assert_eq!(second_goal.revision, 2);
    assert_eq!(
        second_goal.status,
        orbcode_protocol::SessionGoalStatus::Blocked
    );
}

#[test]
fn compat_goal_interrupted_fixture_pins_unterminated_start() {
    let mut session = decode_transcript_fixture("goal_interrupted");
    let goal = session.goal.as_ref().expect("snapshot decodes");
    assert_eq!(goal.goal_id, "goal-interrupt-1");
    assert_eq!(goal.status, orbcode_protocol::SessionGoalStatus::Paused);
    assert_eq!(goal.revision, 5);
    assert_eq!(
        goal.stop_reason.as_deref(),
        Some("interrupted before terminal checkpoint")
    );
    assert_eq!(session.goal_transcript_records.len(), 2);
    assert_eq!(
        session.goal_transcript_records[1].value["type"],
        "goal-turn-start"
    );
    session.rewind_goal_state(session.messages.len());
    assert_eq!(
        session.goal.as_ref().map(|goal| goal.status),
        Some(orbcode_protocol::SessionGoalStatus::Paused),
        "point-in-time folding must not reactivate a crash-recovered goal"
    );
}

#[test]
fn compat_old_transcript_hydrates_without_goal() {
    let session = decode_transcript_fixture("simple_text_chat");
    assert!(session.goal.is_none());
    assert!(session.goal_transcript_records.is_empty());
    assert_eq!(session.messages.len(), 4);
}

/// Ordered `(uuid, parentUuid)` pairs for every record the loader turns into a
/// transcript message (user / assistant / system), read straight from the raw
/// fixture so we can verify the parent chain independently of the decoder.
fn raw_message_chain(contents: &str) -> Vec<(String, Option<String>)> {
    let mut chain = Vec::new();
    for line in contents.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let record_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(record_type, "user" | "assistant" | "system") {
            continue;
        }
        let uuid = value
            .get("uuid")
            .and_then(Value::as_str)
            .expect("message record has uuid")
            .to_string();
        let parent = value
            .get("parentUuid")
            .and_then(Value::as_str)
            .map(str::to_string);
        chain.push((uuid, parent));
    }
    chain
}

/// Assert the parent chain is referentially sound across *all* records that
/// carry a `uuid` (message records plus chain-participant records such as a
/// content-less `compact_boundary` system record): exactly one root with a null
/// parent, and every other parent points at a uuid seen earlier in the file.
/// Out-of-band records without a `uuid` (attachment, summary, file-history
/// snapshots) are skipped — they do not participate in the chain.
fn assert_parent_chain_referential_integrity(contents: &str) {
    let mut seen = std::collections::HashSet::new();
    let mut roots = 0;
    for line in contents.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(uuid) = value.get("uuid").and_then(Value::as_str) else {
            continue;
        };
        match value.get("parentUuid").and_then(Value::as_str) {
            None => roots += 1,
            Some(parent) => assert!(
                seen.contains(parent),
                "parent {parent} of {uuid} must reference an earlier record"
            ),
        }
        seen.insert(uuid.to_string());
    }
    assert_eq!(roots, 1, "exactly one conversation root");
}

/// Count records of a given `type` in the raw fixture.
fn raw_record_type_count(contents: &str, record_type: &str) -> usize {
    contents
        .split('\n')
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value.get("type").and_then(Value::as_str) == Some(record_type))
        .count()
}

/// Build the Anthropic request body for a (possibly forked) session and return
/// the transcript-derived `messages` array with the synthetic leading context
/// message stripped off.
fn anthropic_messages_for(session: &SessionRecord) -> Value {
    let request = ProviderRequest {
        session_id: session.session_id.clone(),
        prompt: String::new(),
        context: TurnContext::default(),
        messages: session.messages.clone(),
        system_prompt: String::new(),
        tools: Vec::new(),
        model: session
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string()),
        base_url: String::new(),
        api_key: None,
        auth_token: None,
        disable_thinking: true,
        effort: None,
        options: ProviderRequestOptions::default(),
    };

    let body = build_anthropic_request_body(&request);
    let mut messages = body
        .get("messages")
        .and_then(Value::as_array)
        .expect("request body has messages array")
        .clone();

    // build_anthropic_request_body always injects a turn-context user message at
    // index 0. Verify and drop it so the comparison is against transcript
    // content only.
    let context = messages.remove(0);
    let context_text = serde_json::to_string(&context).unwrap_or_default();
    assert!(
        context_text.contains("<system-reminder>"),
        "expected injected context message at index 0, got: {context_text}"
    );

    Value::Array(messages)
}

fn normalized(value: &Value) -> String {
    normalize_line(&serde_json::to_string(value).expect("serialize value"))
}

fn byte_fidelity_projection(contents: &str) -> std::collections::BTreeMap<String, Value> {
    let mut projection = std::collections::BTreeMap::new();
    for value in contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        let Some(uuid) = value.get("uuid").and_then(Value::as_str) else {
            continue;
        };
        let mut fields = serde_json::Map::new();
        for key in ["promptId", "gitBranch", "provider"] {
            if let Some(field) = value.get(key) {
                fields.insert(key.to_string(), field.clone());
            }
        }
        if let Some(tool_result) = value
            .pointer("/message/content")
            .and_then(Value::as_array)
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            })
        {
            fields.insert(
                "toolResultContent".to_string(),
                json!({
                    "present": tool_result.get("content").is_some(),
                    "value": tool_result.get("content").cloned().unwrap_or(Value::Null),
                }),
            );
        }
        projection.insert(uuid.to_string(), Value::Object(fields));
    }
    projection
}

fn first_tool_use(session: &SessionRecord) -> Option<(&str, &str, &str)> {
    session
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .find_map(|b| {
            if let TranscriptBlock::ToolUse { id, name, input } = b {
                Some((id.as_str(), name.as_str(), input.as_str()))
            } else {
                None
            }
        })
}

fn first_tool_result(session: &SessionRecord) -> Option<&TranscriptBlock> {
    session
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .find(|b| matches!(b, TranscriptBlock::ToolResult { .. }))
}

fn assert_tool_use_results_are_paired(session: &SessionRecord) {
    let mut issued = std::collections::HashSet::new();
    let mut result_count = 0;
    for block in session.messages.iter().flat_map(|m| &m.blocks) {
        match block {
            TranscriptBlock::ToolUse { id, .. } => {
                issued.insert(id.as_str());
            }
            TranscriptBlock::ToolResult { tool_use_id, .. } => {
                result_count += 1;
                assert!(
                    issued.contains(tool_use_id.as_str()),
                    "tool_result {tool_use_id} must reference an earlier tool_use"
                );
            }
            _ => {}
        }
    }
    let use_count = issued.len();
    assert_eq!(
        use_count, result_count,
        "tool_use/tool_result count mismatch"
    );
}

#[tokio::test]
async fn compat_byte_fidelity_load_rewrite_is_idempotent_and_source_aware() {
    let fixture = load_named(FixtureCategory::Transcripts, "byte_fidelity_schema")
        .expect("byte-fidelity fixture present");
    let original_projection = byte_fidelity_projection(&fixture.contents);
    let temp = tempfile::tempdir().expect("temp transcript directory");
    let session_id = "byte-fidelity-schema";
    let transcript_path = temp.path().join(format!("{session_id}.jsonl"));
    tokio::fs::write(&transcript_path, &fixture.contents)
        .await
        .expect("copy fixture transcript");
    let store = SessionStore::new(
        temp.path().to_path_buf(),
        std::path::PathBuf::from("/current/project"),
        "claude-current-model".to_string(),
    );
    store
        .record_session_hints(
            session_id,
            SessionWriteHints {
                git_branch: Some("current-process-branch".to_string()),
                provider: Some(ProviderId::OpenAi),
            },
        )
        .await;

    let mut session = store.load_session(session_id).await.expect("load fixture");
    assert_eq!(
        session.messages[0]
            .transcript_provenance
            .as_ref()
            .expect("loaded provenance")
            .prompt_id,
        TranscriptJsonField::Value(Value::String(
            "ts-prompt-not-derived-from-message-uuid".to_string()
        ))
    );
    assert_eq!(
        session.messages[1]
            .transcript_provenance
            .as_ref()
            .expect("loaded provenance")
            .git_branch,
        TranscriptJsonField::Null
    );
    assert_eq!(
        session.messages[1]
            .transcript_provenance
            .as_ref()
            .expect("loaded provenance")
            .provider,
        TranscriptJsonField::Value(Value::String("future-provider-v9".to_string()))
    );
    assert_eq!(
        session.messages[2]
            .transcript_provenance
            .as_ref()
            .expect("loaded provenance")
            .prompt_id,
        TranscriptJsonField::Null
    );
    assert_eq!(
        session.messages[4]
            .transcript_provenance
            .as_ref()
            .expect("loaded provenance")
            .prompt_id,
        TranscriptJsonField::Absent
    );

    let structured_content = session.messages[2]
        .blocks
        .iter()
        .find_map(|block| match block {
            TranscriptBlock::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .expect("structured tool result");
    assert_eq!(structured_content.as_str(), "first text\nlast text");
    assert!(matches!(
        structured_content.loaded_field(),
        Some(TranscriptJsonField::Value(Value::Array(items))) if items.len() == 4
    ));

    let new_message = orbcode_protocol::TranscriptMessage::new(MessageRole::User, "new turn");
    let new_message_id = new_message.id.clone();
    session.push_message(new_message);
    store
        .persist_full_session(&session)
        .await
        .expect("first full rewrite");
    let first_rewrite = tokio::fs::read_to_string(&transcript_path)
        .await
        .expect("read first rewrite");
    let first_projection = byte_fidelity_projection(&first_rewrite);

    for (uuid, expected) in &original_projection {
        assert_eq!(
            first_projection.get(uuid),
            Some(expected),
            "loaded record {uuid} changed targeted field state/value"
        );
    }
    let new_fields = first_projection
        .get(&new_message_id)
        .expect("new message projection");
    assert_eq!(new_fields["gitBranch"], "current-process-branch");
    assert_eq!(new_fields["provider"], "openai");
    assert!(new_fields.get("promptId").and_then(Value::as_str).is_some());

    let reloaded = store
        .load_session(session_id)
        .await
        .expect("reload first rewrite");
    store
        .persist_full_session(&reloaded)
        .await
        .expect("second full rewrite");
    let second_rewrite = tokio::fs::read_to_string(&transcript_path)
        .await
        .expect("read second rewrite");
    assert_eq!(
        byte_fidelity_projection(&first_rewrite),
        byte_fidelity_projection(&second_rewrite),
        "target field projection must be idempotent after load/rewrite"
    );
}

#[test]
fn compat_byte_fidelity_provider_request_keeps_all_tool_result_members() {
    let session = decode_transcript_fixture("byte_fidelity_schema");
    let request_messages = anthropic_messages_for(&session);
    let tool_result_content = request_messages
        .as_array()
        .expect("messages array")
        .iter()
        .flat_map(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .find(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .and_then(|block| block.get("content"))
        .and_then(Value::as_array)
        .expect("native Anthropic tool-result content array");

    assert_eq!(tool_result_content.len(), 4);
    assert_eq!(
        tool_result_content[0],
        json!({"type": "text", "text": "first text"})
    );
    assert_eq!(tool_result_content[1]["type"], "image");
    assert_eq!(
        tool_result_content[1]["source"]["data"],
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB"
    );
    assert_eq!(
        serde_json::from_str::<Value>(
            tool_result_content[2]["text"]
                .as_str()
                .expect("structured JSON text")
        )
        .expect("parse structured member"),
        json!({"type": "structured", "payload": {"answer": 42, "flags": [true, null]}})
    );
    assert_eq!(
        tool_result_content[3],
        json!({"type": "text", "text": "last text"})
    );
}

// ---------------------------------------------------------------------------
// Message shape + parent chain
// ---------------------------------------------------------------------------

#[test]
fn compat_transcripts_preserve_message_shape_and_parent_chain() {
    // Every valid transcript fixture: the raw parent chain is well-formed
    // (root has null parent, each subsequent parent points at its predecessor)
    // and the decoder preserves message ids in the same order.
    let names = [
        "simple_text_chat",
        "tool_use_read",
        "bash_with_progress",
        "thinking_and_tool_error",
        "system_and_multiturn",
        "redacted_thinking",
        "attachment",
        "local_command_output",
        "persisted_tool_result_preview",
        "agent_sub_session",
        "multi_model_fallback",
        "system_subtypes_and_context",
    ];

    for name in names {
        let fixture = load_named(FixtureCategory::Transcripts, name).expect("fixture present");
        let chain = raw_message_chain(&fixture.contents);
        assert!(
            chain.len() >= 2,
            "{name}: expected a multi-record chain, got {}",
            chain.len()
        );
        assert_eq!(chain[0].1, None, "{name}: root parentUuid should be null");
        for window in chain.windows(2) {
            let (prev_uuid, _) = &window[0];
            let (_, this_parent) = &window[1];
            assert_eq!(
                this_parent.as_deref(),
                Some(prev_uuid.as_str()),
                "{name}: parent chain broken between {prev_uuid:?} and {this_parent:?}"
            );
        }

        let session = decode_transcript_fixture(name);
        let decoded_ids: Vec<&str> = session.messages.iter().map(|m| m.id.as_str()).collect();
        let raw_ids: Vec<&str> = chain.iter().map(|(uuid, _)| uuid.as_str()).collect();
        assert_eq!(
            decoded_ids, raw_ids,
            "{name}: decoded message ids should match raw record order"
        );
    }
}

#[test]
fn compat_redacted_thinking_fixture_keeps_visible_turns() {
    let fixture =
        load_named(FixtureCategory::Transcripts, "redacted_thinking").expect("fixture present");
    let outcome =
        decode_session_transcript_with_outcome("redacted_thinking".to_string(), &fixture.contents);
    assert_eq!(outcome.skipped_line_count, 0);
    assert!(!outcome.trailing_partial_line);
    assert_parent_chain_referential_integrity(&fixture.contents);

    let session = outcome.session.expect("session decoded");
    assert_eq!(session.messages.len(), 4);
    assert_eq!(session.messages[0].role, MessageRole::User);
    assert_eq!(session.messages[1].role, MessageRole::Assistant);
    assert!(
        session.messages[1]
            .content
            .contains("I computed it safely; the result is 42."),
        "visible assistant text should survive redacted thinking: {:?}",
        session.messages[1].content
    );
    assert!(
        session.messages[3]
            .content
            .contains("Doubled, the result is 84."),
        "second visible assistant text should survive redacted thinking: {:?}",
        session.messages[3].content
    );
    assert_eq!(session.model.as_deref(), Some("claude-opus-4-20250514"));
    assert_tool_use_results_are_paired(&session);
}

#[test]
fn compat_attachment_fixture_skips_attachment_record_without_losing_message() {
    let fixture = load_named(FixtureCategory::Transcripts, "attachment").expect("fixture present");
    let outcome =
        decode_session_transcript_with_outcome("attachment".to_string(), &fixture.contents);
    assert_eq!(outcome.skipped_line_count, 0);
    assert!(!outcome.trailing_partial_line);
    assert_eq!(raw_record_type_count(&fixture.contents, "attachment"), 1);
    assert_parent_chain_referential_integrity(&fixture.contents);

    let session = outcome.session.expect("session decoded");
    assert_eq!(session.messages.len(), 2);
    assert_eq!(session.messages[0].role, MessageRole::User);
    assert_eq!(
        session.messages[0].content,
        "What does this screenshot show?"
    );
    assert_eq!(session.messages[1].role, MessageRole::Assistant);
    assert_eq!(
        session.messages[1].content,
        "It shows a bar chart with three bars."
    );
    assert_tool_use_results_are_paired(&session);
}

#[test]
fn compat_local_command_output_fixture_keeps_meta_user_records() {
    let fixture =
        load_named(FixtureCategory::Transcripts, "local_command_output").expect("fixture present");
    let outcome = decode_session_transcript_with_outcome(
        "local_command_output".to_string(),
        &fixture.contents,
    );
    assert_eq!(outcome.skipped_line_count, 0);
    assert!(!outcome.trailing_partial_line);
    assert_parent_chain_referential_integrity(&fixture.contents);

    let session = outcome.session.expect("session decoded");
    assert_eq!(session.messages.len(), 4);
    assert_eq!(session.messages[0].role, MessageRole::User);
    assert!(
        session.messages[0]
            .content
            .contains("<command-name>diff</command-name>")
    );
    assert_eq!(session.messages[1].role, MessageRole::User);
    assert!(
        session.messages[1]
            .content
            .contains("<local-command-stdout> src/a.rs | 8 ++++++--")
    );
    assert_eq!(session.messages[2].content, "Summarize that diff.");
    assert_eq!(
        session.messages[3].content,
        "Two files changed: a.rs and b.rs, ten insertions and two deletions."
    );
    assert_tool_use_results_are_paired(&session);
}

#[test]
fn compat_compact_boundary_fixture_preserves_multiturn_tool_cycle() {
    let fixture = load_named(FixtureCategory::Transcripts, "compact_boundary_multiturn")
        .expect("fixture present");
    let outcome = decode_session_transcript_with_outcome(
        "compact_boundary_multiturn".to_string(),
        &fixture.contents,
    );
    assert_eq!(
        outcome.skipped_line_count, 0,
        "valid unknown records are skipped without parse failures"
    );
    assert!(!outcome.trailing_partial_line);
    assert_eq!(raw_record_type_count(&fixture.contents, "summary"), 1);
    assert_eq!(
        raw_record_type_count(&fixture.contents, "compact-boundary"),
        1
    );
    assert_parent_chain_referential_integrity(&fixture.contents);

    let session = outcome.session.expect("session decoded");
    let roles: Vec<MessageRole> = session.messages.iter().map(|m| m.role.clone()).collect();
    assert_eq!(
        roles,
        vec![
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::User,
            MessageRole::Assistant,
        ]
    );
    assert_eq!(
        session.messages[0].content,
        "Start a long task and keep me posted."
    );
    assert_eq!(session.messages[1].content, "Working on it.");
    assert_eq!(
        session.messages[2].content,
        "Continue: read the status file."
    );
    assert_eq!(session.messages[5].content, "Status is green.");

    let tool_use = first_tool_use(&session).expect("tool_use present");
    assert_eq!(tool_use.0, "toolu_cb01");
    assert_eq!(tool_use.1, "Read");
    assert!(tool_use.2.contains("STATUS.md"));
    match first_tool_result(&session).expect("tool_result present") {
        TranscriptBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            metadata,
        } => {
            assert_eq!(tool_use_id, "toolu_cb01");
            assert_eq!(content, "status: green");
            assert!(!is_error);
            assert!(
                metadata
                    .as_deref()
                    .is_some_and(|value| value.contains("STATUS.md")),
                "toolUseResult metadata should preserve file path"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
    assert_tool_use_results_are_paired(&session);
}

#[test]
fn compat_simple_text_chat_roles_and_content() {
    let session = decode_transcript_fixture("simple_text_chat");
    let roles: Vec<MessageRole> = session.messages.iter().map(|m| m.role.clone()).collect();
    assert_eq!(
        roles,
        vec![
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::User,
            MessageRole::Assistant
        ]
    );
    assert_eq!(session.messages[0].content, "What is 2+2?");
    assert_eq!(session.messages[1].content, "2 + 2 = 4.");
    assert_eq!(session.model.as_deref(), Some("claude-sonnet-4-20250514"));
    assert_eq!(session.git_branch.as_deref(), Some("main"));
}

#[test]
fn compat_system_and_multiturn_decodes_system_and_custom_title() {
    let session = decode_transcript_fixture("system_and_multiturn");
    assert_eq!(session.messages.len(), 5, "system + two full turns");
    assert_eq!(session.messages[0].role, MessageRole::System);
    assert_eq!(
        session.custom_title.as_deref(),
        Some("File exploration session")
    );
    // The unknown `file-history-snapshot` record must be skipped without
    // dropping the surrounding messages.
    assert_eq!(session.model.as_deref(), Some("claude-opus-4-20250514"));
}

#[test]
fn compat_system_subtypes_and_context_fixture_surfaces_persistence_records() {
    let fixture = load_named(FixtureCategory::Transcripts, "system_subtypes_and_context")
        .expect("fixture present");
    let outcome = decode_session_transcript_with_outcome(
        "system_subtypes_and_context".to_string(),
        &fixture.contents,
    );
    assert_eq!(outcome.skipped_line_count, 0);
    assert!(!outcome.trailing_partial_line);
    assert_eq!(
        raw_record_type_count(&fixture.contents, "session-context"),
        1
    );
    assert_parent_chain_referential_integrity(&fixture.contents);

    let session = outcome.session.expect("session decoded");
    assert_eq!(
        session.additional_directories,
        vec!["/Users/dev/project/crates", "/Users/dev/shared"]
    );
    assert_eq!(
        session.session_allowed_tools,
        vec!["Bash(cargo test:*)", "Read"]
    );
    assert_eq!(session.session_disallowed_tools, vec!["WebFetch"]);
    assert_eq!(session.session_effort, Some(EffortLevel::High));

    let roles: Vec<MessageRole> = session.messages.iter().map(|m| m.role.clone()).collect();
    assert_eq!(
        roles,
        vec![
            MessageRole::User,
            MessageRole::System,
            MessageRole::System,
            MessageRole::User,
            MessageRole::Assistant,
        ]
    );
    assert!(session.messages[1].content.contains("overloaded_error"));
    assert!(session.messages[1].content.contains("attempt 2/5"));
    assert!(
        session.messages[2]
            .content
            .contains("Conversation history before this point has been snipped")
    );
    assert_eq!(
        session.messages[4].content,
        "The retry warning was preserved and the snip marker stayed visible."
    );
}

// ---------------------------------------------------------------------------
// Tool blocks + progress metadata
// ---------------------------------------------------------------------------

#[test]
fn compat_tool_use_block_carries_name_input_and_result() {
    let session = decode_transcript_fixture("tool_use_read");
    let (id, name, input) = first_tool_use(&session).expect("tool_use block present");
    assert_eq!(name, "Read");
    assert_eq!(id, "toolu_bbbb01");
    assert!(
        input.contains("README.md"),
        "tool_use input should retain file_path, got: {input}"
    );

    match first_tool_result(&session).expect("tool_result block present") {
        TranscriptBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            metadata,
        } => {
            assert_eq!(tool_use_id, "toolu_bbbb01");
            assert!(!is_error);
            assert!(content.contains("demo project"));
            assert!(
                metadata.is_some(),
                "toolUseResult metadata should be attached"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn compat_tool_result_error_flag_is_preserved() {
    let session = decode_transcript_fixture("thinking_and_tool_error");
    let has_thinking = session
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .any(|b| matches!(b, TranscriptBlock::Thinking { .. }));
    assert!(has_thinking, "thinking block should decode");

    match first_tool_result(&session).expect("tool_result present") {
        TranscriptBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(is_error, "errored tool result should set is_error");
            assert!(content.contains("No such file"));
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn compat_standalone_progress_records_merge_into_tool_result_metadata() {
    let session = decode_transcript_fixture("bash_with_progress");
    let metadata = match first_tool_result(&session).expect("tool_result present") {
        TranscriptBlock::ToolResult { metadata, .. } => {
            metadata.clone().expect("progress metadata attached")
        }
        other => panic!("expected ToolResult, got {other:?}"),
    };
    let parsed: Value = serde_json::from_str(&metadata).expect("metadata is json");
    let progress = parsed
        .get("progressMessages")
        .and_then(Value::as_array)
        .expect("progressMessages array present");
    assert_eq!(progress.len(), 3, "all three progress records should merge");
    // Original tool result metadata fields must survive the merge.
    assert!(parsed.get("stdout").is_some(), "stdout metadata retained");
}

#[test]
fn compat_persisted_tool_result_preview_preserves_out_of_line_marker() {
    let fixture = load_named(
        FixtureCategory::Transcripts,
        "persisted_tool_result_preview",
    )
    .expect("fixture present");
    let outcome = decode_session_transcript_with_outcome(
        "persisted_tool_result_preview".to_string(),
        &fixture.contents,
    );
    assert_eq!(outcome.skipped_line_count, 0);
    assert!(!outcome.trailing_partial_line);
    assert_parent_chain_referential_integrity(&fixture.contents);

    let session = outcome.session.expect("session decoded");
    assert_eq!(session.messages.len(), 4);
    let (tool_id, tool_name, tool_input) =
        first_tool_use(&session).expect("tool_use block present");
    assert_eq!(tool_name, "Bash");
    assert!(tool_input.contains("diagnostics --verbose"));

    match first_tool_result(&session).expect("tool_result block present") {
        TranscriptBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            metadata,
        } => {
            assert_eq!(tool_use_id, tool_id);
            assert!(!is_error);
            assert!(
                content.starts_with(PERSISTED_OUTPUT_TAG),
                "persisted preview marker should survive decode: {content:?}"
            );
            assert!(
                content.contains("Full output saved to:"),
                "saved-path text should remain visible: {content:?}"
            );
            assert!(
                content.ends_with(PERSISTED_OUTPUT_CLOSING_TAG),
                "persisted preview closing marker should survive decode: {content:?}"
            );

            let parsed: Value = serde_json::from_str(
                metadata
                    .as_ref()
                    .expect("toolUseResult metadata should be attached"),
            )
            .expect("metadata is json");
            assert_eq!(parsed.get("isImage").and_then(Value::as_bool), Some(false));
            assert!(
                parsed
                    .get("stdout")
                    .and_then(Value::as_str)
                    .is_some_and(|stdout| stdout.starts_with(PERSISTED_OUTPUT_TAG)),
                "stdout metadata should retain the persisted-output preview"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
    assert_tool_use_results_are_paired(&session);
}

// ---------------------------------------------------------------------------
// Provider request round-trip + resume/fork
// ---------------------------------------------------------------------------

#[test]
fn compat_provider_request_round_trips_against_golden() {
    // Rust-built Anthropic message array (encode) must match the TS golden
    // byte-for-byte after normalization.
    let cases = ["simple_text_chat", "tool_use_read", "bash_with_progress"];
    for case in cases {
        let session = decode_transcript_fixture(case);
        let built = anthropic_messages_for(&session);

        let golden = load_named(
            FixtureCategory::ProviderStreams,
            &format!("{case}.anthropic"),
        )
        .unwrap_or_else(|| panic!("provider golden for '{case}' not found"));
        let golden_value: Value =
            serde_json::from_str(&golden.contents).expect("golden parses as json");

        assert_eq!(
            normalized(&built),
            normalized(&golden_value),
            "{case}: built Anthropic messages diverged from TS golden"
        );
    }
}

#[test]
fn compat_resume_fork_preserves_provider_request_content() {
    // Forking a session from a TS transcript (new session id, same history)
    // must produce the same provider request content as the source.
    let source = decode_transcript_fixture("tool_use_read");

    let mut forked = source.clone();
    forked.session_id = "forked-session-1111".to_string();
    forked.messages = source.messages.clone();

    assert_ne!(source.session_id, forked.session_id, "fork has a new id");
    assert_eq!(
        normalized(&anthropic_messages_for(&source)),
        normalized(&anthropic_messages_for(&forked)),
        "forked session provider request should match the source transcript"
    );

    // And the forked request must still match the TS golden.
    let golden = load_named(FixtureCategory::ProviderStreams, "tool_use_read.anthropic")
        .expect("golden present");
    let golden_value: Value = serde_json::from_str(&golden.contents).expect("golden json");
    assert_eq!(
        normalized(&anthropic_messages_for(&forked)),
        normalized(&golden_value)
    );
}

// ---------------------------------------------------------------------------
// Corrupt / partial JSONL recovery
// ---------------------------------------------------------------------------

#[test]
fn compat_corrupt_truncated_tail_recovers_valid_prefix() {
    let fixture = load_dir(&fixtures_root().join("transcripts/corrupt"))
        .into_iter()
        .find(|f| f.name == "truncated_tail")
        .expect("truncated_tail fixture");
    let outcome =
        decode_session_transcript_with_outcome("truncated".to_string(), &fixture.contents);
    assert!(
        outcome.trailing_partial_line,
        "mid-write tail should be flagged"
    );
    assert_eq!(
        outcome.skipped_line_count, 1,
        "only the partial line is skipped"
    );
    let session = outcome.session.expect("valid prefix recovered");
    assert_eq!(session.messages.len(), 2);
    assert_eq!(session.messages[0].content, "first message");
    assert_eq!(session.messages[1].content, "first reply");
}

#[test]
fn compat_corrupt_midline_garbage_is_skipped() {
    let fixture = load_dir(&fixtures_root().join("transcripts/corrupt"))
        .into_iter()
        .find(|f| f.name == "corrupt_line")
        .expect("corrupt_line fixture");
    let outcome = decode_session_transcript_with_outcome("corrupt".to_string(), &fixture.contents);
    assert_eq!(outcome.skipped_line_count, 1, "one unparsable line skipped");
    assert!(!outcome.trailing_partial_line);
    let session = outcome.session.expect("session recovered");
    assert_eq!(session.messages.len(), 2);
    assert_eq!(session.messages[0].content, "before corruption");
    assert_eq!(session.messages[1].content, "after corruption");
}

#[test]
fn compat_corrupt_missing_required_fields_drops_only_bad_records() {
    let fixture = load_dir(&fixtures_root().join("transcripts/corrupt"))
        .into_iter()
        .find(|f| f.name == "missing_fields")
        .expect("missing_fields fixture");
    let outcome = decode_session_transcript_with_outcome("missing".to_string(), &fixture.contents);
    // All lines are valid JSON, so nothing is counted as a parse failure...
    assert_eq!(outcome.skipped_line_count, 0);
    // ...but records missing `message`/`type` produce no message.
    let session = outcome.session.expect("session recovered");
    assert_eq!(
        session.messages.len(),
        2,
        "only the two complete records survive"
    );
    assert_eq!(session.messages[0].content, "valid first");
    assert_eq!(session.messages[1].content, "valid last");
}

// ---------------------------------------------------------------------------
// Tool-call fixtures
// ---------------------------------------------------------------------------

#[test]
fn compat_tool_call_fixtures_decode_use_and_result_pairs() {
    for fixture in load_category(FixtureCategory::ToolCalls) {
        let outcome =
            decode_session_transcript_with_outcome(fixture.name.clone(), &fixture.contents);
        let session = outcome
            .session
            .unwrap_or_else(|| panic!("tool_call fixture '{}' decoded", fixture.name));

        let tool_use = first_tool_use(&session)
            .unwrap_or_else(|| panic!("{}: missing tool_use block", fixture.name));
        let result = first_tool_result(&session)
            .unwrap_or_else(|| panic!("{}: missing tool_result block", fixture.name));

        // The result's tool_use_id must reference the issued tool_use.
        if let TranscriptBlock::ToolResult { tool_use_id, .. } = result {
            assert_eq!(
                tool_use_id, tool_use.0,
                "{}: tool_result must reference its tool_use id",
                fixture.name
            );
        }
    }
}

#[test]
fn compat_tool_call_specific_shapes() {
    let edit = decode_session_transcript_with_outcome(
        "edit".to_string(),
        &load_named(FixtureCategory::ToolCalls, "edit_conflict")
            .unwrap()
            .contents,
    )
    .session
    .unwrap();
    match first_tool_result(&edit).unwrap() {
        TranscriptBlock::ToolResult { is_error, .. } => assert!(*is_error),
        other => panic!("expected ToolResult, got {other:?}"),
    }

    let write = decode_session_transcript_with_outcome(
        "write".to_string(),
        &load_named(FixtureCategory::ToolCalls, "write_file")
            .unwrap()
            .contents,
    )
    .session
    .unwrap();
    match first_tool_result(&write).unwrap() {
        TranscriptBlock::ToolResult { metadata, .. } => {
            assert!(
                metadata.is_some(),
                "write tool result keeps toolUseResult metadata"
            )
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Permission fixtures
// ---------------------------------------------------------------------------

#[test]
fn compat_permission_fixtures_parse_and_summarize() {
    let expected = [
        ("bash_network", "Bash toolu_perm01 (tools, network)"),
        ("write_tools", "Write toolu_perm02 (tools)"),
        ("read_no_extra", "Read toolu_perm03"),
    ];

    for (name, summary) in expected {
        let fixture =
            load_named(FixtureCategory::Permissions, name).expect("permission fixture present");
        let request: PermissionRequest =
            serde_json::from_str(&fixture.contents).expect("permission request parses");
        assert_eq!(request.summary(), summary, "{name}: summary mismatch");

        // Round-trip: re-serialize and confirm normalization is stable (volatile
        // request/session UUIDs collapse to the sentinel on both sides).
        let reserialized = serde_json::to_value(&request).expect("serialize back");
        let fixture_value: Value = serde_json::from_str(&fixture.contents).expect("fixture json");
        assert_eq!(
            normalized(&reserialized),
            normalized(&fixture_value),
            "{name}: permission request round-trip diverged"
        );
    }
}

// ---------------------------------------------------------------------------
// Real TypeScript session end-to-end
// ---------------------------------------------------------------------------

/// Load the bundled real-world TS session (sanitized: home path replaced, all
/// UUIDs/timestamps/structure preserved). Captured from Claude Code v2.6.0.
fn real_ts_session_fixture() -> orbcode_compat_fixtures::Fixture {
    load_dir(&fixtures_root().join("transcripts/real"))
        .into_iter()
        .find(|f| f.name == "ts_session_v2_6")
        .expect("real ts session fixture present")
}

/// True when the raw record with `uuid` carries no text or tool block the
/// provider would replay — a content-less system record or a turn whose only
/// blocks are thinking. Such records are legitimately dropped by the loader.
fn raw_record_has_no_surfaced_content(contents: &str, uuid: &str) -> bool {
    for line in contents.split('\n') {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("uuid").and_then(Value::as_str) != Some(uuid) {
            continue;
        }
        let content = value.get("message").and_then(|m| m.get("content"));
        return match content {
            None | Some(Value::Null) => true,
            Some(Value::String(text)) => text.trim().is_empty(),
            Some(Value::Array(blocks)) => blocks.iter().all(|block| {
                matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("thinking" | "redacted_thinking")
                )
            }),
            _ => false,
        };
    }
    false
}

#[test]
fn compat_real_ts_session_decodes_with_unknown_records_skipped() {
    // A genuine TS transcript carries record types our hand-authored fixtures
    // never exercise: `mode`, `attribution-snapshot`, `file-history-snapshot`,
    // `queue-operation`, and `last-prompt`. The loader must skip every one of
    // them without dropping or reordering the real conversation messages.
    let fixture = real_ts_session_fixture();
    let outcome = decode_session_transcript_with_outcome("real_ts".to_string(), &fixture.contents);

    // Every line is valid JSON, so nothing counts as a parse failure even though
    // most records are unknown types that produce no message.
    assert_eq!(outcome.skipped_line_count, 0, "no JSON parse failures");
    assert!(!outcome.trailing_partial_line);
    let session = outcome.session.expect("real session decodes");

    // The decoder surfaces user/assistant/system records in order, but drops the
    // ones that carry no API-relevant content (here: 3 content-less
    // `stop_hook_summary` system records and 1 thinking-only assistant turn). So
    // the decoded ids must be an order-preserving SUBSEQUENCE of the raw message
    // records, and every dropped record must be one that has no surfaced content.
    let raw = raw_message_chain(&fixture.contents);
    let decoded_ids: Vec<&str> = session.messages.iter().map(|m| m.id.as_str()).collect();
    let raw_ids: Vec<&str> = raw.iter().map(|(uuid, _)| uuid.as_str()).collect();

    let mut cursor = decoded_ids.iter();
    let mut next = cursor.next();
    for raw_id in &raw_ids {
        if next == Some(raw_id) {
            next = cursor.next();
        }
    }
    assert!(
        next.is_none(),
        "decoded ids must be an in-order subsequence of raw record ids:\n decoded={decoded_ids:?}\n raw={raw_ids:?}"
    );

    let decoded_set: std::collections::HashSet<&str> = decoded_ids.iter().copied().collect();
    for dropped in raw_ids.iter().filter(|id| !decoded_set.contains(*id)) {
        assert!(
            raw_record_has_no_surfaced_content(&fixture.contents, dropped),
            "dropped record {dropped} should have no surfaced text/tool content"
        );
    }
    assert!(
        decoded_ids.len() >= 20,
        "real session should surface the bulk of its messages, got {}",
        decoded_ids.len()
    );

    // Referential integrity of the parent chain on real (non-strictly-linear)
    // data: exactly one root, and every other parent points at an earlier uuid.
    let mut seen = std::collections::HashSet::new();
    let mut roots = 0;
    for (uuid, parent) in &raw {
        match parent {
            None => roots += 1,
            Some(parent) => assert!(
                seen.contains(parent.as_str()),
                "parent {parent} of {uuid} must reference an earlier record"
            ),
        }
        seen.insert(uuid.as_str());
    }
    assert_eq!(roots, 1, "exactly one conversation root");

    // The real tool cycle survives: a tool_use and a tool_result that references
    // it, plus a thinking block.
    let tool_use = first_tool_use(&session).expect("real session has a tool_use");
    match first_tool_result(&session).expect("real session has a tool_result") {
        TranscriptBlock::ToolResult { tool_use_id, .. } => {
            assert_eq!(
                tool_use_id, tool_use.0,
                "tool_result references its tool_use"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
    // The whole point of resume: a real on-disk session reconstructs a provider
    // request without panicking and carries its messages through.
    let messages = anthropic_messages_for(&session);
    assert!(
        messages.as_array().is_some_and(|a| !a.is_empty()),
        "provider request rebuilt from real session has messages"
    );
}

#[test]
fn compat_real_on_disk_transcript_decodes_when_env_set() {
    // Opt-in true end-to-end check against an arbitrary real session on this
    // machine. Point ORBCODE_COMPAT_REAL_TRANSCRIPT at any `.jsonl` under
    // ~/.claude/projects to validate the loader on data we did not author. Skips
    // silently when unset so CI stays hermetic.
    let Ok(path) = std::env::var("ORBCODE_COMPAT_REAL_TRANSCRIPT") else {
        return;
    };
    let contents = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let outcome = decode_session_transcript_with_outcome(path.clone(), &contents);
    let session = outcome
        .session
        .unwrap_or_else(|| panic!("{path} decoded to no session"));
    assert!(!session.messages.is_empty(), "{path}: decoded no messages");
    let messages = anthropic_messages_for(&session);
    assert!(
        messages.as_array().is_some_and(|a| !a.is_empty()),
        "{path}: provider request has no messages"
    );
}

// ---------------------------------------------------------------------------
// Agent sub-session transcript
// ---------------------------------------------------------------------------

#[test]
fn compat_agent_sub_session_preserves_agent_tool_cycle() {
    let fixture =
        load_named(FixtureCategory::Transcripts, "agent_sub_session").expect("fixture present");
    assert_parent_chain_referential_integrity(&fixture.contents);

    let session = decode_transcript_fixture("agent_sub_session");
    assert_eq!(session.messages.len(), 4, "expected 4 transcript messages");

    let roles: Vec<_> = session.messages.iter().map(|m| m.role.clone()).collect();
    assert_eq!(
        roles,
        vec![
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::User,
            MessageRole::Assistant,
        ]
    );

    let (tool_id, tool_name, tool_input) =
        first_tool_use(&session).expect("Agent tool_use present");
    assert_eq!(tool_name, "Agent");
    assert!(
        tool_input.contains("prompt"),
        "Agent input should contain 'prompt' field"
    );

    match first_tool_result(&session).expect("tool_result present") {
        TranscriptBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        } => {
            assert_eq!(
                tool_use_id, tool_id,
                "tool_result references the Agent tool_use"
            );
            assert!(!is_error, "Agent tool_result should not be an error");
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
    assert_tool_use_results_are_paired(&session);
}

// ---------------------------------------------------------------------------
// Multi-model fallback transcript
// ---------------------------------------------------------------------------

#[test]
fn compat_multi_model_fallback_preserves_per_turn_model() {
    let fixture =
        load_named(FixtureCategory::Transcripts, "multi_model_fallback").expect("fixture present");
    assert_parent_chain_referential_integrity(&fixture.contents);

    let session = decode_transcript_fixture("multi_model_fallback");
    assert_eq!(session.messages.len(), 4, "expected 4 transcript messages");

    let roles: Vec<_> = session.messages.iter().map(|m| m.role.clone()).collect();
    assert_eq!(
        roles,
        vec![
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::User,
            MessageRole::Assistant,
        ]
    );

    assert_eq!(
        session.model.as_deref(),
        Some("claude-opus-4-20250514"),
        "session model should reflect the last assistant turn's model"
    );
}

// ---------------------------------------------------------------------------
// Disabled provider transcript compat
// ---------------------------------------------------------------------------

#[test]
fn compat_disabled_provider_gemini_transcript_decodes_without_panic() {
    let fixture = load_named(FixtureCategory::Transcripts, "disabled_provider_gemini")
        .expect("fixture present");
    assert_parent_chain_referential_integrity(&fixture.contents);

    let session = decode_transcript_fixture("disabled_provider_gemini");
    assert_eq!(session.messages.len(), 4, "expected 4 transcript messages");

    let roles: Vec<_> = session.messages.iter().map(|m| m.role.clone()).collect();
    assert_eq!(
        roles,
        vec![
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::User,
            MessageRole::Assistant,
        ]
    );

    assert_eq!(
        session.model.as_deref(),
        Some("gemini-2.0-flash"),
        "session model should preserve the disabled provider's model string"
    );
}

#[test]
fn compat_disabled_provider_grok_transcript_decodes_without_panic() {
    let fixture = load_named(FixtureCategory::Transcripts, "disabled_provider_grok")
        .expect("fixture present");
    assert_parent_chain_referential_integrity(&fixture.contents);

    let session = decode_transcript_fixture("disabled_provider_grok");
    assert_eq!(session.messages.len(), 2, "expected 2 transcript messages");

    let roles: Vec<_> = session.messages.iter().map(|m| m.role.clone()).collect();
    assert_eq!(roles, vec![MessageRole::User, MessageRole::Assistant]);

    assert_eq!(
        session.model.as_deref(),
        Some("grok-3"),
        "session model should preserve the disabled provider's model string"
    );
}

// ---------------------------------------------------------------------------
// Aggregate entry point
// ---------------------------------------------------------------------------

#[test]
fn compat_all_transcript_and_tool_fixtures_decode() {
    // Single sweep proving every transcript/tool fixture loads and decodes,
    // so `cargo test compat` exercises the whole corpus in one run.
    let mut decoded = 0;
    for category in [FixtureCategory::Transcripts, FixtureCategory::ToolCalls] {
        for fixture in load_category(category) {
            let outcome =
                decode_session_transcript_with_outcome(fixture.name.clone(), &fixture.contents);
            assert!(
                outcome.session.is_some(),
                "fixture '{}' should decode to a session",
                fixture.name
            );
            decoded += 1;
        }
    }
    assert!(
        decoded >= 8,
        "expected to decode the full corpus, got {decoded}"
    );
}
