//! ACP `session/load` replay policy guardrails.
//!
//! These tests intentionally sit next to transcript decoding, not in the ACP
//! CLI adapter. They pin which persisted transcript shapes are safe to replay
//! as stable ACP `session/update` notifications and which shapes require a
//! typed preflight rejection before any partial replay is sent.

use orbcode_compat_fixtures::{FixtureCategory, load_named};
use orbcode_session_store::{
    AcpReplayPolicy, AcpReplayPolicyState, PERSISTED_OUTPUT_TAG, TranscriptRecord,
    acp_load_replay_blockers, classify_record_for_acp_replay,
    decode_session_transcript_with_outcome,
};
use serde_json::{Value, json};

fn transcript_lines(lines: &[Value]) -> String {
    lines
        .iter()
        .map(|line| serde_json::to_string(line).expect("serialize transcript line"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn first_record_policy(line: Value) -> AcpReplayPolicy {
    let record = TranscriptRecord::from_value(&line).expect("valid transcript record");
    classify_record_for_acp_replay(&record)
}

fn assert_record_policy(
    name: &str,
    line: Value,
    expected: AcpReplayPolicyState,
    expected_reason: &str,
) {
    let policy = first_record_policy(line);
    assert_eq!(
        policy.state, expected,
        "{name} ACP replay policy changed; reason was: {}",
        policy.reason
    );
    assert!(
        policy.reason.contains(expected_reason),
        "{name} ACP replay reason should mention {expected_reason:?}, got {:?}",
        policy.reason
    );
}

fn assert_transcript_blocks_load(name: &str, body: &str, expected_reason: &str) {
    let blockers = acp_load_replay_blockers(name, body);

    assert!(
        blockers
            .iter()
            .any(|reason| reason.contains(expected_reason)),
        "{name} should block ACP session/load replay for {expected_reason:?}; blockers: {blockers:?}"
    );
}

#[test]
fn acp_replay_policy_matrix_classifies_replayable_records() {
    assert_record_policy(
        "user text",
        json!({
            "type": "user",
            "uuid": "user-text",
            "timestamp": "2026-01-01T00:00:00Z",
            "cwd": "/tmp/project",
            "message": { "role": "user", "content": "hello" }
        }),
        AcpReplayPolicyState::ReplayAsAcpUpdate,
        "user_message_chunk",
    );
    assert_record_policy(
        "assistant text",
        json!({
            "type": "assistant",
            "uuid": "assistant-text",
            "timestamp": "2026-01-01T00:00:01Z",
            "cwd": "/tmp/project",
            "message": {
                "role": "assistant",
                "content": [{ "type": "text", "text": "hi" }]
            }
        }),
        AcpReplayPolicyState::ReplayAsAcpUpdate,
        "agent_message_chunk",
    );
    assert_record_policy(
        "thinking text/signature",
        json!({
            "type": "assistant",
            "uuid": "assistant-thinking",
            "timestamp": "2026-01-01T00:00:02Z",
            "cwd": "/tmp/project",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "thinking",
                    "thinking": "considering",
                    "signature": "provider-signature"
                }]
            }
        }),
        AcpReplayPolicyState::ReplayAsAcpUpdate,
        "signatures have no stable ACP field",
    );
    assert_record_policy(
        "tool_use",
        json!({
            "type": "assistant",
            "uuid": "assistant-tool",
            "timestamp": "2026-01-01T00:00:03Z",
            "cwd": "/tmp/project",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "Read",
                    "input": { "file_path": "README.md" }
                }]
            }
        }),
        AcpReplayPolicyState::ReplayAsAcpUpdate,
        "tool_call",
    );
    assert_record_policy(
        "tool_result success",
        json!({
            "type": "user",
            "uuid": "user-tool-result",
            "timestamp": "2026-01-01T00:00:04Z",
            "cwd": "/tmp/project",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "content": "file contents",
                    "is_error": false
                }]
            }
        }),
        AcpReplayPolicyState::ReplayAsAcpUpdate,
        "success/error",
    );
    assert_record_policy(
        "tool_result error",
        json!({
            "type": "user",
            "uuid": "user-tool-error",
            "timestamp": "2026-01-01T00:00:05Z",
            "cwd": "/tmp/project",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "content": "permission denied",
                    "is_error": true
                }]
            }
        }),
        AcpReplayPolicyState::ReplayAsAcpUpdate,
        "success/error",
    );
    assert_record_policy(
        "tool progress metadata",
        json!({
            "type": "progress",
            "uuid": "progress-1",
            "parentToolUseID": "toolu_1",
            "timestamp": "2026-01-01T00:00:06Z",
            "data": { "status": "running" }
        }),
        AcpReplayPolicyState::ReplayAsAcpUpdate,
        "tool_call_update",
    );

    let persisted = load_named(
        FixtureCategory::Transcripts,
        "persisted_tool_result_preview",
    )
    .expect("fixture present");
    assert!(
        persisted.contents.contains(PERSISTED_OUTPUT_TAG),
        "fixture must carry the persisted-output preview marker"
    );
    let session =
        decode_session_transcript_with_outcome("persisted".to_string(), &persisted.contents)
            .session
            .expect("fixture decodes");
    assert!(
        session
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .any(|block| matches!(
                block,
                orbcode_protocol::TranscriptBlock::ToolResult { content, .. }
                    if content.contains(PERSISTED_OUTPUT_TAG)
            )),
        "large persisted-output previews must remain replayable as tool output"
    );
}

#[test]
fn acp_replay_policy_matrix_classifies_omittable_records() {
    assert_record_policy(
        "system init",
        json!({
            "type": "system",
            "subtype": "init",
            "uuid": "system-init",
            "timestamp": "2026-01-01T00:00:00Z",
            "cwd": "/tmp/project"
        }),
        AcpReplayPolicyState::OmitWithSafeReason,
        "content-less system metadata",
    );
    assert_record_policy(
        "session context",
        json!({
            "type": "session-context",
            "timestamp": "2026-01-01T00:00:00Z",
            "cwd": "/tmp/project",
            "additionalDirectories": ["/tmp/project/crate-a"]
        }),
        AcpReplayPolicyState::OmitWithSafeReason,
        "session identity",
    );
}

#[test]
fn acp_replay_policy_matrix_blocks_unfaithful_history_records() {
    assert_record_policy(
        "api/system error",
        json!({
            "type": "system",
            "subtype": "api_error",
            "uuid": "system-api-error",
            "timestamp": "2026-01-01T00:00:00Z",
            "cwd": "/tmp/project",
            "error": { "message": "upstream unavailable" }
        }),
        AcpReplayPolicyState::BlocksLoadReplay,
        "system/API-error provenance",
    );
    assert_record_policy(
        "snip boundary",
        json!({
            "type": "system",
            "subtype": "snip_boundary",
            "uuid": "system-snip",
            "timestamp": "2026-01-01T00:00:00Z",
            "cwd": "/tmp/project"
        }),
        AcpReplayPolicyState::BlocksLoadReplay,
        "snip-boundary provenance",
    );
    assert_record_policy(
        "compact summary",
        json!({
            "type": "system",
            "uuid": "system-compact",
            "timestamp": "2026-01-01T00:00:00Z",
            "cwd": "/tmp/project",
            "message": {
                "role": "system",
                "content": "This session is being continued from a previous conversation that ran out of context.\n\nSummary:\nEarlier work happened."
            }
        }),
        AcpReplayPolicyState::BlocksLoadReplay,
        "compact-summary provenance",
    );
}

#[test]
fn acp_load_session_preflight_rejects_blocking_transcripts() {
    let api_error = transcript_lines(&[
        json!({
            "type": "user",
            "uuid": "user-1",
            "timestamp": "2026-01-01T00:00:00Z",
            "cwd": "/tmp/project",
            "message": { "role": "user", "content": "hello" }
        }),
        json!({
            "type": "system",
            "subtype": "api_error",
            "uuid": "system-api-error",
            "timestamp": "2026-01-01T00:00:01Z",
            "cwd": "/tmp/project",
            "error": { "message": "rate limited" }
        }),
    ]);
    assert_transcript_blocks_load("api_error", &api_error, "system/API-error provenance");

    let cwd_less = transcript_lines(&[json!({
        "type": "user",
        "uuid": "cwd-less-user",
        "timestamp": "2026-01-01T00:00:00Z",
        "message": { "role": "user", "content": "hello" }
    })]);
    assert_transcript_blocks_load("cwd_less", &cwd_less, "missing cwd");

    let relative_cwd = transcript_lines(&[json!({
        "type": "user",
        "uuid": "relative-cwd-user",
        "timestamp": "2026-01-01T00:00:00Z",
        "cwd": "relative/project",
        "message": { "role": "user", "content": "hello" }
    })]);
    assert_transcript_blocks_load("relative_cwd", &relative_cwd, "relative cwd");

    let corrupt = format!(
        "{}\n{{\"type\":\"assistant\",\"uuid\":\"truncated\"",
        serde_json::to_string(&json!({
            "type": "user",
            "uuid": "valid-before-corrupt",
            "timestamp": "2026-01-01T00:00:00Z",
            "cwd": "/tmp/project",
            "message": { "role": "user", "content": "hello" }
        }))
        .expect("serialize")
    );
    assert_transcript_blocks_load("corrupt", &corrupt, "corrupt transcript lines");

    let compact_fixture = load_named(FixtureCategory::Transcripts, "compact_boundary_multiturn")
        .expect("fixture present");
    assert_transcript_blocks_load(
        "compact_boundary_multiturn",
        &compact_fixture.contents,
        "compact-summary provenance",
    );
}

#[test]
fn acp_replay_policy_pins_typed_preflight_blockers() {
    let blockers = [
        "system/API-error provenance",
        "snip-boundary provenance",
        "compact-summary provenance",
        "missing cwd",
        "relative cwd",
        "corrupt transcript lines",
    ];
    assert_eq!(
        blockers.len(),
        6,
        "ACP session/load must keep typed preflight rejection for every pinned replay blocker"
    );
}
