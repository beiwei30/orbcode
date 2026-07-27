//! Regression and stress tests for transcript decode throughput.
//!
//! These exercise the full `decode_session_transcript_with_outcome` pipeline
//! with generated JSONL fixtures of varying size and complexity.

use orbcode_protocol::{MessageRole, TranscriptBlock};
use orbcode_session_store::decode_session_transcript_with_outcome;
use serde_json::{Value, json};

fn generate_session_jsonl(turn_count: usize, tool_result_chars: usize) -> String {
    let mut lines = Vec::new();
    let mut parent_uuid: Option<String> = None;

    for turn in 0..turn_count {
        let base_seconds = turn * 3;
        let user_uuid = format!("user-{turn}");
        let assistant_uuid = format!("assistant-{turn}");
        let tool_result_uuid = format!("tool-result-{turn}");
        let tool_use_id = format!("tool-use-{turn}");

        lines.push(
            serde_json::to_string(&json!({
                "parentUuid": parent_uuid,
                "type": "user",
                "uuid": user_uuid,
                "timestamp": format!("2026-01-01T00:00:{:02}Z", base_seconds.min(59)),
                "message": {
                    "role": "user",
                    "content": format!("perf fixture user message {turn}")
                },
                "cwd": "/tmp/perf-fixture",
                "sessionId": "perf-session",
            }))
            .expect("serialize user line"),
        );

        lines.push(
            serde_json::to_string(&json!({
                "parentUuid": &user_uuid,
                "type": "assistant",
                "uuid": assistant_uuid,
                "timestamp": format!("2026-01-01T00:00:{:02}Z", (base_seconds + 1).min(59)),
                "message": {
                    "role": "assistant",
                    "content": [{
                        "type": "text",
                        "text": format!("perf fixture assistant response {turn}")
                    }, {
                        "type": "tool_use",
                        "id": tool_use_id,
                        "name": "Read",
                        "input": { "file_path": format!("/tmp/file_{turn}.rs") }
                    }],
                    "model": "claude-sonnet-4-20250514",
                    "stop_reason": "tool_use",
                },
                "cwd": "/tmp/perf-fixture",
                "sessionId": "perf-session",
            }))
            .expect("serialize assistant line"),
        );

        let tool_result_content = "x".repeat(tool_result_chars);
        lines.push(
            serde_json::to_string(&json!({
                "parentUuid": &assistant_uuid,
                "type": "user",
                "uuid": tool_result_uuid,
                "timestamp": format!("2026-01-01T00:00:{:02}Z", (base_seconds + 2).min(59)),
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": tool_result_content,
                        "is_error": false,
                    }],
                },
                "cwd": "/tmp/perf-fixture",
                "sessionId": "perf-session",
            }))
            .expect("serialize tool result line"),
        );

        parent_uuid = Some(tool_result_uuid);
    }

    lines.join("\n") + "\n"
}

fn generate_session_jsonl_with_progress(turn_count: usize, progress_per_tool: usize) -> String {
    let mut lines = Vec::new();
    let mut parent_uuid: Option<String> = None;

    for turn in 0..turn_count {
        let user_uuid = format!("user-{turn}");
        let assistant_uuid = format!("assistant-{turn}");
        let tool_result_uuid = format!("tool-result-{turn}");
        let tool_use_id = format!("tool-use-{turn}");

        lines.push(
            serde_json::to_string(&json!({
                "parentUuid": parent_uuid,
                "type": "user",
                "uuid": user_uuid,
                "timestamp": "2026-01-01T00:00:00Z",
                "message": { "role": "user", "content": format!("message {turn}") },
                "cwd": "/tmp",
                "sessionId": "perf-session",
            }))
            .expect("serialize user"),
        );

        lines.push(
            serde_json::to_string(&json!({
                "parentUuid": &user_uuid,
                "type": "assistant",
                "uuid": assistant_uuid,
                "timestamp": "2026-01-01T00:00:01Z",
                "message": {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": tool_use_id,
                        "name": "Bash",
                        "input": { "command": "echo test" }
                    }],
                    "model": "claude-sonnet-4-20250514",
                    "stop_reason": "tool_use",
                },
                "cwd": "/tmp",
                "sessionId": "perf-session",
            }))
            .expect("serialize assistant"),
        );

        for progress_index in 0..progress_per_tool {
            lines.push(
                serde_json::to_string(&json!({
                    "type": "progress",
                    "parentToolUseID": tool_use_id,
                    "uuid": format!("progress-{turn}-{progress_index}"),
                    "timestamp": "2026-01-01T00:00:01Z",
                    "sessionId": "perf-session",
                    "data": {
                        "type": "tool_progress",
                        "toolName": "Bash",
                        "toolUseId": tool_use_id,
                        "progress": { "status": format!("step {progress_index}") }
                    }
                }))
                .expect("serialize progress"),
            );
        }

        lines.push(
            serde_json::to_string(&json!({
                "parentUuid": &assistant_uuid,
                "type": "user",
                "uuid": tool_result_uuid,
                "timestamp": "2026-01-01T00:00:02Z",
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": format!("result {turn}"),
                        "is_error": false,
                    }],
                },
                "cwd": "/tmp",
                "sessionId": "perf-session",
            }))
            .expect("serialize tool result"),
        );

        parent_uuid = Some(tool_result_uuid);
    }

    lines.join("\n") + "\n"
}

#[test]
fn regression_budget_decode_message_count_matches_generated_turns() {
    let jsonl = generate_session_jsonl(50, 200);
    let outcome = decode_session_transcript_with_outcome("perf-session".to_string(), &jsonl);

    assert_eq!(outcome.skipped_line_count, 0);
    assert!(!outcome.trailing_partial_line);

    let session = outcome.session.expect("session decoded");
    let user_messages = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .count();
    let assistant_messages = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .count();

    assert_eq!(assistant_messages, 50, "one assistant message per turn");
    assert!(
        user_messages >= 50,
        "at least one user message per turn (got {user_messages})"
    );
}

#[test]
fn regression_budget_decode_survives_corrupt_lines_without_dropping_valid() {
    let mut jsonl = generate_session_jsonl(20, 100);
    let lines: Vec<&str> = jsonl.trim().split('\n').collect();
    let line_count = lines.len();
    assert_eq!(line_count, 60, "20 turns x 3 lines");

    let corrupt_lines = [
        "{\"type\":\"user\",\"truncated",
        "not json at all",
        "{incomplete",
        "!!!",
    ];
    let mut all_lines: Vec<String> = lines.iter().map(std::string::ToString::to_string).collect();
    for (index, corrupt) in corrupt_lines.iter().enumerate() {
        all_lines.insert(index * 16 + 1, corrupt.to_string());
    }
    jsonl = all_lines.join("\n") + "\n";

    let outcome = decode_session_transcript_with_outcome("perf-session".to_string(), &jsonl);

    assert_eq!(
        outcome.skipped_line_count, 4,
        "exactly 4 corrupt lines should be skipped"
    );
    let session = outcome.session.expect("session decoded despite corruption");
    let assistant_count = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .count();
    assert_eq!(
        assistant_count, 20,
        "all 20 assistant messages should survive corruption"
    );
}

#[test]
fn regression_budget_decode_progress_records_associate_with_correct_tools() {
    let jsonl = generate_session_jsonl_with_progress(10, 50);
    let outcome = decode_session_transcript_with_outcome("perf-session".to_string(), &jsonl);

    assert_eq!(outcome.skipped_line_count, 0);
    let session = outcome.session.expect("session decoded");

    let tool_result_blocks_with_progress: Vec<_> = session
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .filter_map(|block| match block {
            TranscriptBlock::ToolResult { metadata, .. } => metadata.as_ref().and_then(|m| {
                serde_json::from_str::<Value>(m)
                    .ok()
                    .and_then(|v| v.get("progressMessages")?.as_array().cloned())
            }),
            _ => None,
        })
        .collect();

    assert_eq!(
        tool_result_blocks_with_progress.len(),
        10,
        "each of the 10 tool results should have progress metadata"
    );
    for (index, progress_array) in tool_result_blocks_with_progress.iter().enumerate() {
        assert_eq!(
            progress_array.len(),
            50,
            "turn {index}: each tool result should have 50 progress records"
        );
    }
}

#[test]
fn regression_budget_decode_ignores_trailing_blank_lines() {
    let jsonl = generate_session_jsonl(10, 100);
    let baseline = decode_session_transcript_with_outcome("perf-session".to_string(), &jsonl);
    let baseline_messages = baseline
        .session
        .as_ref()
        .expect("baseline decoded")
        .messages
        .len();

    let padded = format!("{}{}", jsonl, "\n".repeat(10_000));
    let padded_outcome =
        decode_session_transcript_with_outcome("perf-session".to_string(), &padded);

    assert_eq!(
        padded_outcome.skipped_line_count,
        baseline.skipped_line_count
    );
    assert_eq!(
        padded_outcome
            .session
            .as_ref()
            .expect("padded decoded")
            .messages
            .len(),
        baseline_messages,
        "trailing blank lines should not affect message count"
    );
}

#[test]
#[ignore = "manual stress test for long-session transcript decode throughput"]
fn transcript_decode_stress_processes_large_session() {
    use std::time::Instant;

    const TURN_COUNT: usize = 1_000;
    const TOOL_RESULT_CHARS: usize = 5_000;

    let jsonl = generate_session_jsonl(TURN_COUNT, TOOL_RESULT_CHARS);
    let jsonl_bytes = jsonl.len();

    let started = Instant::now();
    let outcome = decode_session_transcript_with_outcome("perf-session".to_string(), &jsonl);
    let duration = started.elapsed();

    let messages = outcome.session.expect("session decoded").messages.len();
    eprintln!(
        "turns={TURN_COUNT} tool_result_chars={TOOL_RESULT_CHARS} \
         jsonl_bytes={jsonl_bytes} messages={messages} decode_us={}",
        duration.as_micros()
    );
}

#[test]
#[ignore = "manual stress test for transcript decode with heavy progress records"]
fn transcript_decode_stress_handles_many_progress_records() {
    use std::time::Instant;

    const TURN_COUNT: usize = 200;
    const PROGRESS_PER_TOOL: usize = 500;

    let jsonl = generate_session_jsonl_with_progress(TURN_COUNT, PROGRESS_PER_TOOL);
    let jsonl_bytes = jsonl.len();

    let started = Instant::now();
    let outcome = decode_session_transcript_with_outcome("perf-session".to_string(), &jsonl);
    let duration = started.elapsed();

    let messages = outcome.session.expect("session decoded").messages.len();
    eprintln!(
        "turns={TURN_COUNT} progress_per_tool={PROGRESS_PER_TOOL} \
         jsonl_bytes={jsonl_bytes} messages={messages} decode_us={}",
        duration.as_micros()
    );
}

/// Returns true when `decoded` is an order-preserving subsequence of
/// `raw` — every decoded id appears in `raw`, in the same relative order.
fn is_ordered_subsequence(decoded: &[&str], raw: &[&str]) -> bool {
    let mut cursor = decoded.iter();
    let mut next = cursor.next();
    for raw_id in raw {
        if next == Some(raw_id) {
            next = cursor.next();
        }
    }
    next.is_none()
}

#[test]
fn mixed_variants_with_truncated_tail_decode_into_ordered_subsequence() {
    let records = [
        json!({
            "type": "system",
            "subtype": "init",
            "uuid": "rec-init",
            "timestamp": "2026-05-29T00:00:00Z",
            "cwd": "/repo",
            "model": "claude-opus-4-7",
        }),
        json!({
            "type": "user",
            "uuid": "rec-user",
            "parentUuid": Value::Null,
            "timestamp": "2026-05-29T00:00:01Z",
            "message": { "role": "user", "content": "build the feature" },
        }),
        json!({
            "type": "system",
            "subtype": "local_command",
            "uuid": "rec-localcmd",
            "timestamp": "2026-05-29T00:00:02Z",
            "content": "<command-name>/status</command-name>",
        }),
        json!({
            "type": "attachment",
            "uuid": "rec-attachment",
            "timestamp": "2026-05-29T00:00:03Z",
            "attachment": { "type": "selected_lines", "filename": "x.rs" },
        }),
        json!({
            "type": "assistant",
            "uuid": "rec-assistant-redacted",
            "parentUuid": "rec-user",
            "timestamp": "2026-05-29T00:00:04Z",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "redacted_thinking", "data": "Zz==" },
                    { "type": "text", "text": "Starting now." }
                ],
                "model": "claude-opus-4-7",
            },
        }),
        json!({
            "type": "system",
            "subtype": "api_error",
            "uuid": "rec-api-error",
            "timestamp": "2026-05-29T00:00:05Z",
            "error": { "message": "overloaded_error" },
            "retryAttempt": 1,
            "maxRetries": 3,
        }),
        json!({
            "type": "assistant",
            "uuid": "rec-assistant-tool",
            "parentUuid": "rec-assistant-redacted",
            "timestamp": "2026-05-29T00:00:06Z",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_mix",
                    "name": "Bash",
                    "input": { "command": "make" }
                }],
                "model": "claude-opus-4-7",
            },
        }),
        json!({
            "type": "progress",
            "uuid": "rec-progress",
            "parentToolUseID": "toolu_mix",
            "timestamp": "2026-05-29T00:00:07Z",
            "data": { "type": "hook_progress", "hookEvent": "PostToolUse" },
        }),
        json!({
            "type": "user",
            "uuid": "rec-tool-result",
            "parentUuid": "rec-assistant-tool",
            "timestamp": "2026-05-29T00:00:08Z",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_mix",
                    "content": "build ok",
                    "is_error": false,
                }],
            },
        }),
        json!({
            "type": "system",
            "subtype": "snip_boundary",
            "uuid": "rec-snip",
            "timestamp": "2026-05-29T00:00:09Z",
            "snipMetadata": { "removedUuids": ["rec-user"] },
        }),
        json!({
            "type": "team-only-future-thing",
            "uuid": "rec-unknown",
            "timestamp": "2026-05-29T00:00:10Z",
            "payload": { "anything": true },
        }),
        json!({
            "type": "system",
            "subtype": "stop_hook_summary",
            "uuid": "rec-stop-hook",
            "timestamp": "2026-05-29T00:00:11Z",
        }),
    ];

    let raw_ids: Vec<&str> = records
        .iter()
        .map(|r| r.get("uuid").and_then(Value::as_str).expect("uuid"))
        .collect();

    let mut body = records
        .iter()
        .map(|line| serde_json::to_string(line).expect("serialize"))
        .collect::<Vec<_>>()
        .join("\n");
    body.push('\n');
    body.push_str("{\"type\":\"assistant\",\"uuid\":\"rec-truncated\",\"timesta");

    let outcome = decode_session_transcript_with_outcome("mixed".to_string(), &body);
    assert!(outcome.trailing_partial_line, "partial tail flagged");
    assert_eq!(
        outcome.skipped_line_count, 1,
        "only the truncated tail counts as skipped"
    );
    let session = outcome.session.expect("mixed session decodes");

    let decoded_ids: Vec<&str> = session.messages.iter().map(|m| m.id.as_str()).collect();
    assert!(
        is_ordered_subsequence(&decoded_ids, &raw_ids),
        "decoded={decoded_ids:?} raw={raw_ids:?}"
    );

    let decoded: std::collections::HashSet<&str> = decoded_ids.iter().copied().collect();
    assert!(decoded.contains("rec-user"));
    assert!(decoded.contains("rec-localcmd"), "local command surfaced");
    assert!(
        decoded.contains("rec-assistant-redacted"),
        "redacted-thinking turn surfaced"
    );
    assert!(decoded.contains("rec-api-error"), "api error surfaced");
    assert!(decoded.contains("rec-assistant-tool"));
    assert!(decoded.contains("rec-tool-result"));
    assert!(decoded.contains("rec-snip"), "snip boundary surfaced");
    assert!(!decoded.contains("rec-init"), "init dropped");
    assert!(!decoded.contains("rec-attachment"), "attachment dropped");
    assert!(!decoded.contains("rec-unknown"), "unknown type dropped");
    assert!(!decoded.contains("rec-stop-hook"), "stop hook dropped");
    assert!(!decoded.contains("rec-truncated"), "truncated tail dropped");

    let by_id = |id: &str| {
        session
            .messages
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.content.clone())
            .unwrap_or_default()
    };
    assert!(by_id("rec-assistant-redacted").contains("[redacted thinking]"));
    assert!(by_id("rec-assistant-redacted").contains("Starting now."));
    assert!(by_id("rec-api-error").contains("overloaded_error"));
    assert_eq!(
        by_id("rec-snip"),
        "[snip] Conversation history before this point has been snipped."
    );

    let tool_result_progress = session
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .find_map(|block| match block {
            TranscriptBlock::ToolResult {
                tool_use_id,
                metadata,
                ..
            } => {
                assert_eq!(tool_use_id, "toolu_mix", "tool result references its use");
                metadata.as_ref().and_then(|raw| {
                    serde_json::from_str::<Value>(raw)
                        .ok()
                        .and_then(|v| v.get("progressMessages")?.as_array().cloned())
                })
            }
            _ => None,
        })
        .expect("tool result with merged hook progress");
    assert_eq!(tool_result_progress.len(), 1);
}

#[test]
fn user_local_command_output_keeps_tool_pairing_and_parent_chain() {
    let lines = [
        json!({
            "type": "assistant",
            "uuid": "assistant-tool",
            "parentUuid": Value::Null,
            "timestamp": "2026-01-01T00:00:00Z",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_abc",
                    "name": "Bash",
                    "input": { "command": "ls" }
                }],
                "model": "claude-opus-4-7",
            },
        }),
        json!({
            "type": "user",
            "uuid": "user-localcmd",
            "parentUuid": "assistant-tool",
            "timestamp": "2026-01-01T00:00:01Z",
            "message": {
                "role": "user",
                "content": "<local-command-stdout>branch is clean</local-command-stdout>",
            },
        }),
        json!({
            "type": "user",
            "uuid": "user-toolresult",
            "parentUuid": "user-localcmd",
            "timestamp": "2026-01-01T00:00:02Z",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_abc",
                    "content": "file_a\nfile_b",
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
    assert_eq!(outcome.skipped_line_count, 0);
    let session = outcome.session.expect("session decoded");
    assert_eq!(session.messages.len(), 3, "all three records surface");

    assert!(
        session
            .messages
            .iter()
            .any(|m| m.content.contains("branch is clean")),
        "local command output preserved"
    );

    let tool_use_ids: Vec<&str> = session
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .filter_map(|block| match block {
            TranscriptBlock::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let tool_result_ids: Vec<&str> = session
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .filter_map(|block| match block {
            TranscriptBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(tool_use_ids, ["toolu_abc"]);
    assert_eq!(tool_result_ids, ["toolu_abc"], "tool result still paired");
}
