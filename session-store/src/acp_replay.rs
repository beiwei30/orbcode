//! ACP `session/load` replay/reject policy.
//!
//! The ACP adapter must not decide replay safety by looking at raw JSONL
//! itself. This module keeps transcript-shape decisions next to transcript
//! decoding so app-server boundary code can reject unsafe sessions before the
//! ACP adapter emits any partial history.

use std::path::Path;

use serde_json::Value;

use crate::{
    RawContentBlock, TranscriptRecord, TranscriptRecordKind,
    transcript::{COMPACT_SUMMARY_PREFIX, decode_session_transcript_with_outcome},
    transcript_schema::raw_content_blocks,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpReplayPolicyState {
    ReplayAsAcpUpdate,
    OmitWithSafeReason,
    BlocksLoadReplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcpReplayPolicy {
    pub state: AcpReplayPolicyState,
    pub reason: &'static str,
}

impl AcpReplayPolicy {
    const fn replay(reason: &'static str) -> Self {
        Self {
            state: AcpReplayPolicyState::ReplayAsAcpUpdate,
            reason,
        }
    }

    const fn omit(reason: &'static str) -> Self {
        Self {
            state: AcpReplayPolicyState::OmitWithSafeReason,
            reason,
        }
    }

    const fn blocks(reason: &'static str) -> Self {
        Self {
            state: AcpReplayPolicyState::BlocksLoadReplay,
            reason,
        }
    }
}

pub fn classify_record_for_acp_replay(record: &TranscriptRecord) -> AcpReplayPolicy {
    match record.kind() {
        TranscriptRecordKind::User => classify_user_record(record),
        TranscriptRecordKind::Assistant => classify_assistant_record(record),
        TranscriptRecordKind::System => classify_system_record(record),
        TranscriptRecordKind::Progress => {
            AcpReplayPolicy::replay("persisted progress can replay as tool_call_update raw output")
        }
        TranscriptRecordKind::CustomTitle
        | TranscriptRecordKind::SessionContext
        | TranscriptRecordKind::Goal
        | TranscriptRecordKind::GoalCleared
        | TranscriptRecordKind::GoalTurnStart
        | TranscriptRecordKind::GoalTurnTerminal => {
            AcpReplayPolicy::omit("metadata is represented on session identity, not history replay")
        }
        TranscriptRecordKind::Unknown => match record.record_type.as_deref() {
            Some("summary" | "compact-boundary") => AcpReplayPolicy::blocks(
                "stable ACP history has no compact-summary provenance update",
            ),
            _ => AcpReplayPolicy::omit("unknown record type is skipped"),
        },
    }
}

pub fn acp_load_replay_blockers(session_id: &str, contents: &str) -> Vec<String> {
    let outcome = decode_session_transcript_with_outcome(session_id.to_string(), contents);
    let mut blockers = Vec::new();
    if outcome.skipped_line_count > 0 || outcome.trailing_partial_line {
        blockers.push("corrupt transcript lines cannot be replayed honestly".to_string());
    }

    let Some(session) = outcome.session else {
        blockers.push("transcript decoded to no session".to_string());
        return blockers;
    };

    match session.cwd.as_deref() {
        None => blockers.push("missing cwd prevents ACP load cwd validation".to_string()),
        Some(cwd) if !Path::new(cwd).is_absolute() => {
            blockers.push("relative cwd prevents ACP load cwd validation".to_string());
        }
        _ => {}
    }

    for line in contents.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(record) = TranscriptRecord::from_value(&value) else {
            continue;
        };
        let policy = classify_record_for_acp_replay(&record);
        if policy.state == AcpReplayPolicyState::BlocksLoadReplay {
            blockers.push(policy.reason.to_string());
        }
    }

    blockers
}

fn classify_user_record(record: &TranscriptRecord) -> AcpReplayPolicy {
    let blocks = content_blocks(record);
    if blocks
        .iter()
        .any(|block| matches!(block, RawContentBlock::ToolResult(_)))
    {
        return AcpReplayPolicy::replay(
            "tool_result success/error can replay as tool_call_update status and output",
        );
    }
    if blocks
        .iter()
        .any(|block| matches!(block, RawContentBlock::Text(_)))
    {
        return AcpReplayPolicy::replay("user text can replay as user_message_chunk");
    }
    AcpReplayPolicy::omit("content-less user record has no ACP history update")
}

fn classify_assistant_record(record: &TranscriptRecord) -> AcpReplayPolicy {
    let blocks = content_blocks(record);
    if blocks
        .iter()
        .any(|block| matches!(block, RawContentBlock::ToolUse(_)))
    {
        return AcpReplayPolicy::replay("tool_use can replay as tool_call");
    }
    if blocks
        .iter()
        .any(|block| matches!(block, RawContentBlock::Thinking(_)))
    {
        return AcpReplayPolicy::replay(
            "thinking text can replay as agent_thought_chunk; signatures have no stable ACP field",
        );
    }
    if blocks
        .iter()
        .any(|block| matches!(block, RawContentBlock::Text(_)))
    {
        return AcpReplayPolicy::replay("assistant text can replay as agent_message_chunk");
    }
    AcpReplayPolicy::omit("content-less assistant record has no ACP history update")
}

fn classify_system_record(record: &TranscriptRecord) -> AcpReplayPolicy {
    if record.subtype.as_deref() == Some("api_error") {
        return AcpReplayPolicy::blocks(
            "stable ACP history has no system/API-error provenance update",
        );
    }
    if record.subtype.as_deref() == Some("snip_boundary") {
        return AcpReplayPolicy::blocks(
            "stable ACP history has no snip-boundary provenance update",
        );
    }
    if system_content(record).is_some_and(|content| content.starts_with(COMPACT_SUMMARY_PREFIX)) {
        return AcpReplayPolicy::blocks(
            "stable ACP history has no compact-summary provenance update",
        );
    }
    if system_content(record).is_some() {
        return AcpReplayPolicy::blocks("stable ACP history has no generic system-note update");
    }
    AcpReplayPolicy::omit("content-less system metadata can be omitted from history replay")
}

fn content_blocks(record: &TranscriptRecord) -> Vec<RawContentBlock> {
    record
        .message
        .as_ref()
        .and_then(|message| message.content.as_ref())
        .map(raw_content_blocks)
        .unwrap_or_default()
}

fn system_content(record: &TranscriptRecord) -> Option<&str> {
    record
        .message
        .as_ref()
        .and_then(|message| message.content.as_ref())
        .and_then(Value::as_str)
        .or_else(|| record.content.as_ref().and_then(Value::as_str))
}
