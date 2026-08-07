mod blocks;
mod progress;

use chrono::{DateTime, Utc};
use orbcode_protocol::{
    EffortLevel, MessageRole, ProviderId, SessionGoalStatus, SessionGoalTranscriptRecord,
    SessionGoalTranscriptState, SessionRecord,
};
use serde_json::Value;

use crate::transcript_schema::{TranscriptRecord, TranscriptRecordKind};

use blocks::transcript_message_from_record;
use progress::{collect_progress_records, gc_pre_compact_messages};

pub const CUSTOM_TITLE_ENTRY_TYPE: &str = "custom-title";
pub const SESSION_CONTEXT_ENTRY_TYPE: &str = "session-context";
pub const GOAL_ENTRY_TYPE: &str = "goal";
pub const GOAL_CLEARED_ENTRY_TYPE: &str = "goal-cleared";
pub const GOAL_TURN_START_ENTRY_TYPE: &str = "goal-turn-start";
pub const GOAL_TURN_TERMINAL_ENTRY_TYPE: &str = "goal-turn-terminal";

/// `entrypoint` stamped on every transcript record we write. The TypeScript CLI
/// writes `"cli"` here; we deliberately differ, because transcripts share
/// `~/.claude/projects/` with it and a distinguishable provenance marker is
/// worth more than matching the field exactly.
pub const TRANSCRIPT_ENTRYPOINT: &str = "orbcode";

/// `version` stamped on every transcript record we write.
///
/// The TypeScript CLI puts its own semver here (`"2.6.0"`). We keep the
/// `orbcode-` prefix so the value stays unambiguously ours even in isolation —
/// see [`TRANSCRIPT_ENTRYPOINT`] — but the tail is the real crate version
/// rather than a frozen project-phase label. Nothing reads this field back
/// (`TranscriptRecord::version` is decoded and never compared), so it is a
/// provenance stamp only.
pub const TRANSCRIPT_VERSION: &str = concat!("orbcode-", env!("CARGO_PKG_VERSION"));

/// Prefix of the synthetic compaction summary message produced by
/// `compact_user_summary_message()` (in `core/src/compaction.rs`). A System
/// message whose content starts with this prefix marks a compact boundary:
/// all messages prior to it are stale (already summarised) and can be dropped
/// on load to save memory and token budget.
pub(crate) const COMPACT_SUMMARY_PREFIX: &str =
    "This session is being continued from a previous conversation";

/// Result of [`decode_session_transcript_with_outcome`]. Tracks how many
/// raw `.jsonl` lines could not be parsed as JSON so the doctor can
/// surface recovery hints even when the transcript itself remains
/// usable. `skipped_line_count` excludes blank lines and counts a
/// trailing partial line at most once.
#[derive(Debug, Default, Clone)]
pub struct TranscriptDecodeOutcome {
    pub session: Option<SessionRecord>,
    pub skipped_line_count: usize,
    /// True when the final line is non-empty but missing a trailing
    /// newline — the marker for a crash mid-append. Callers may surface
    /// this to the user so they can let the next append heal the file
    /// or run `orbcode doctor` for guidance.
    pub trailing_partial_line: bool,
}

pub fn decode_session_transcript_with_outcome(
    session_id: String,
    contents: &str,
) -> TranscriptDecodeOutcome {
    let mut outcome = TranscriptDecodeOutcome::default();
    let parsed_lines = parsed_lines_with_recovery(contents, &mut outcome);
    outcome.session = build_session_record(session_id, parsed_lines);
    outcome
}

fn parsed_lines_with_recovery(contents: &str, outcome: &mut TranscriptDecodeOutcome) -> Vec<Value> {
    let mut parsed = Vec::new();
    let mut iterator = contents.split('\n').peekable();
    while let Some(line) = iterator.next() {
        let is_last = iterator.peek().is_none();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(value) => parsed.push(value),
            Err(_) => {
                outcome.skipped_line_count += 1;
                if is_last && !contents.ends_with('\n') {
                    outcome.trailing_partial_line = true;
                }
            }
        }
    }
    parsed
}

fn build_session_record(session_id: String, parsed_lines: Vec<Value>) -> Option<SessionRecord> {
    let mut session = SessionRecord {
        session_id,
        title: None,
        custom_title: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        cwd: None,
        git_branch: None,
        model: None,
        provider: None,
        additional_directories: Vec::new(),
        session_allowed_tools: Vec::new(),
        session_disallowed_tools: Vec::new(),
        session_effort: None,
        goal: None,
        goal_transcript_records: Vec::new(),
        messages: Vec::new(),
    };
    let progress_by_parent_tool_use_id = collect_progress_records(&parsed_lines);

    let mut saw_message = false;
    let mut saw_context = false;
    let mut saw_goal_record = false;
    let mut latest_metadata_timestamp: Option<DateTime<Utc>> = None;
    let mut latest_metadata_cwd: Option<String> = None;
    let mut open_goal_turns = Vec::new();
    for value in parsed_lines {
        let Some(record) = TranscriptRecord::from_value(&value) else {
            if is_raw_goal_record(&value) {
                saw_goal_record = true;
                session
                    .goal_transcript_records
                    .push(SessionGoalTranscriptRecord {
                        after_message_count: session.messages.len(),
                        value: value.clone(),
                        state: SessionGoalTranscriptState::Unchanged,
                    });
                merge_latest_timestamp(
                    &mut latest_metadata_timestamp,
                    value.get("timestamp").and_then(Value::as_str),
                );
                merge_latest_cwd(
                    &mut latest_metadata_cwd,
                    value.get("cwd").and_then(Value::as_str),
                );
            }
            continue;
        };

        match record.kind() {
            TranscriptRecordKind::Goal => {
                saw_goal_record = true;
                let decoded_goal = record.session_goal();
                if let Some(goal) = decoded_goal.as_ref() {
                    session.goal = Some(goal.clone());
                }
                session
                    .goal_transcript_records
                    .push(SessionGoalTranscriptRecord {
                        after_message_count: session.messages.len(),
                        value: value.clone(),
                        state: decoded_goal.map_or(
                            SessionGoalTranscriptState::Unchanged,
                            SessionGoalTranscriptState::Set,
                        ),
                    });
                merge_latest_timestamp(&mut latest_metadata_timestamp, record.timestamp.as_deref());
                merge_latest_cwd(&mut latest_metadata_cwd, record.cwd.as_deref());
            }
            TranscriptRecordKind::GoalCleared => {
                saw_goal_record = true;
                session.goal = None;
                session
                    .goal_transcript_records
                    .push(SessionGoalTranscriptRecord {
                        after_message_count: session.messages.len(),
                        value: value.clone(),
                        state: SessionGoalTranscriptState::Cleared,
                    });
                merge_latest_timestamp(&mut latest_metadata_timestamp, record.timestamp.as_deref());
                merge_latest_cwd(&mut latest_metadata_cwd, record.cwd.as_deref());
            }
            TranscriptRecordKind::GoalTurnStart => {
                saw_goal_record = true;
                if let (Some(goal_id), Some(goal_revision), Some(turn_id)) = (
                    record.goal_id.as_deref(),
                    record.goal_revision,
                    record.turn_id.as_deref(),
                ) {
                    open_goal_turns.retain(|(open_goal_id, _, open_turn_id, _)| {
                        open_goal_id != goal_id || open_turn_id != turn_id
                    });
                    open_goal_turns.push((
                        goal_id.to_string(),
                        goal_revision,
                        turn_id.to_string(),
                        record.timestamp.as_deref().and_then(parse_timestamp),
                    ));
                }
                session
                    .goal_transcript_records
                    .push(SessionGoalTranscriptRecord {
                        after_message_count: session.messages.len(),
                        value: value.clone(),
                        state: SessionGoalTranscriptState::Unchanged,
                    });
                merge_latest_timestamp(&mut latest_metadata_timestamp, record.timestamp.as_deref());
                merge_latest_cwd(&mut latest_metadata_cwd, record.cwd.as_deref());
            }
            TranscriptRecordKind::GoalTurnTerminal => {
                saw_goal_record = true;
                if let (Some(goal_id), Some(turn_id)) =
                    (record.goal_id.as_deref(), record.turn_id.as_deref())
                    && let Some(index) =
                        open_goal_turns
                            .iter()
                            .rposition(|(open_goal_id, _, open_turn_id, _)| {
                                open_goal_id == goal_id && open_turn_id == turn_id
                            })
                {
                    open_goal_turns.remove(index);
                }
                session
                    .goal_transcript_records
                    .push(SessionGoalTranscriptRecord {
                        after_message_count: session.messages.len(),
                        value: value.clone(),
                        state: SessionGoalTranscriptState::Unchanged,
                    });
                merge_latest_timestamp(&mut latest_metadata_timestamp, record.timestamp.as_deref());
                merge_latest_cwd(&mut latest_metadata_cwd, record.cwd.as_deref());
            }
            TranscriptRecordKind::CustomTitle => {
                if let Some(title) = record.custom_title.as_deref().or(record.title.as_deref()) {
                    let trimmed = title.trim();
                    if !trimmed.is_empty() {
                        session.custom_title = Some(trimmed.to_string());
                    }
                }
                merge_latest_timestamp(&mut latest_metadata_timestamp, record.timestamp.as_deref());
                merge_latest_cwd(&mut latest_metadata_cwd, record.cwd.as_deref());
            }
            TranscriptRecordKind::SessionContext => {
                saw_context = true;
                if let Some(directories) = record.additional_directories.as_deref() {
                    session.additional_directories = string_array(directories);
                }
                if let Some(permissions) = record.session_permissions.as_ref() {
                    if let Some(allow) = permissions.allow.as_deref() {
                        session.session_allowed_tools = string_array(allow);
                    }
                    if let Some(deny) = permissions.deny.as_deref() {
                        session.session_disallowed_tools = string_array(deny);
                    }
                }
                // `Some(_)` means the `sessionEffort` key was present (even when
                // null), which clears any prior override exactly as the original
                // presence check did.
                if let Some(effort) = record.session_effort.as_ref() {
                    session.session_effort = effort
                        .as_ref()
                        .and_then(Value::as_str)
                        .and_then(EffortLevel::parse);
                }
                merge_latest_timestamp(&mut latest_metadata_timestamp, record.timestamp.as_deref());
                merge_latest_cwd(&mut latest_metadata_cwd, record.cwd.as_deref());
            }
            TranscriptRecordKind::User
            | TranscriptRecordKind::Assistant
            | TranscriptRecordKind::System => {
                let Some(message) =
                    transcript_message_from_record(&record, &progress_by_parent_tool_use_id)
                else {
                    continue;
                };
                if !saw_message {
                    session.created_at = message.created_at;
                    saw_message = true;
                }
                // Message-record cwd is taken verbatim (no empty-string filter),
                // unlike the metadata-record cwd above.
                if session.cwd.is_none()
                    && let Some(cwd) = record.cwd.as_deref()
                {
                    session.cwd = Some(cwd.to_string());
                }
                if let Some(branch) = record.git_branch.as_str()
                    && !branch.is_empty()
                {
                    session.git_branch = Some(branch.to_string());
                }
                if message.role == MessageRole::Assistant
                    && let Some(model) = record
                        .message
                        .as_ref()
                        .and_then(|message| message.model.as_deref())
                {
                    session.model = Some(model.to_string());
                }
                if let Some(provider) = record.provider.as_str().and_then(ProviderId::parse) {
                    session.provider = Some(provider);
                }
                session.push_message(message);
            }
            TranscriptRecordKind::Progress | TranscriptRecordKind::Unknown => {}
        }
    }

    gc_pre_compact_messages(&mut session.messages);

    if let Some(goal) = session.goal.as_mut()
        && goal.status == SessionGoalStatus::Active
        && let Some((_, _, turn_id, started_at)) =
            open_goal_turns
                .iter()
                .rev()
                .find(|(goal_id, goal_revision, _, _)| {
                    goal_id == &goal.goal_id && *goal_revision == goal.revision
                })
    {
        let original = goal.clone();
        goal.status = SessionGoalStatus::Paused;
        goal.revision = goal.revision.saturating_add(1);
        goal.stop_reason = Some("interrupted before terminal checkpoint".to_string());
        goal.last_goal_turn_id = Some(turn_id.clone());
        if let Some(started_at) = started_at {
            goal.updated_at = goal.updated_at.max(*started_at);
        }
        let recovered = goal.clone();
        if let Some(snapshot) = session.goal_transcript_records.iter_mut().rev().find(
            |record| {
                matches!(
                    &record.state,
                    SessionGoalTranscriptState::Set(snapshot) if snapshot.goal_id == recovered.goal_id
                )
            },
        ) {
            snapshot.state = SessionGoalTranscriptState::Recovered {
                original,
                recovered: Box::new(recovered),
                turn_id: turn_id.clone(),
            };
        }
    }

    if session.messages.is_empty()
        && session.custom_title.is_none()
        && !saw_context
        && !saw_goal_record
    {
        return None;
    }

    if !saw_message {
        if let Some(timestamp) = latest_metadata_timestamp {
            session.created_at = timestamp;
            session.updated_at = timestamp;
        }
        if session.cwd.is_none() {
            session.cwd = latest_metadata_cwd;
        }
    } else if let Some(timestamp) = latest_metadata_timestamp {
        session.updated_at = session.updated_at.max(timestamp);
    }

    Some(session)
}

fn is_raw_goal_record(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some(
            GOAL_ENTRY_TYPE
                | GOAL_CLEARED_ENTRY_TYPE
                | GOAL_TURN_START_ENTRY_TYPE
                | GOAL_TURN_TERMINAL_ENTRY_TYPE
        )
    )
}

fn string_array(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

/// Fold a metadata-record timestamp into the running latest, keeping the maximum.
fn merge_latest_timestamp(latest: &mut Option<DateTime<Utc>>, timestamp: Option<&str>) {
    if let Some(timestamp) = timestamp.and_then(parse_timestamp) {
        *latest = Some(latest.map_or(timestamp, |existing| existing.max(timestamp)));
    }
}

/// Record the first non-empty metadata-record cwd seen.
fn merge_latest_cwd(latest: &mut Option<String>, cwd: Option<&str>) {
    if latest.is_none()
        && let Some(cwd) = cwd
        && !cwd.is_empty()
    {
        *latest = Some(cwd.to_string());
    }
}

pub(crate) fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decode_outcome_reports_trailing_partial_line_without_dropping_valid_messages() {
        let valid = serde_json::to_string(&json!({
            "type": "user",
            "uuid": "user-1",
            "timestamp": "2026-05-23T01:00:00.000Z",
            "message": { "role": "user", "content": "first" },
            "sessionId": "s",
            "cwd": "/tmp",
        }))
        .expect("serialize valid line");
        // Final line is mid-write — no trailing newline.
        let body = format!("{valid}\n{{\"type\":\"user\",\"truncated\":true");
        let outcome = decode_session_transcript_with_outcome("s".to_string(), &body);
        assert_eq!(outcome.skipped_line_count, 1);
        assert!(outcome.trailing_partial_line);
        let session = outcome.session.expect("session decoded");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "first");
    }

    #[test]
    fn decode_outcome_treats_blank_trailing_line_as_clean() {
        let valid = serde_json::to_string(&json!({
            "type": "user",
            "uuid": "user-1",
            "timestamp": "2026-05-23T01:00:00.000Z",
            "message": { "role": "user", "content": "hi" },
            "sessionId": "s",
            "cwd": "/tmp",
        }))
        .expect("serialize valid line");
        let body = format!("{valid}\n");
        let outcome = decode_session_transcript_with_outcome("s".to_string(), &body);
        assert_eq!(outcome.skipped_line_count, 0);
        assert!(!outcome.trailing_partial_line);
        assert!(outcome.session.is_some());
    }
}
