//! Explicit, forward-compatible typed schema for on-disk transcript records.
//!
//! Each `.jsonl` line in a session transcript decodes into a [`TranscriptRecord`]
//! envelope plus, where present, a typed [`RecordMessage`] payload and typed
//! [`RawContentBlock`] entries. Core identity/provenance fields (uuid,
//! parentUuid, sessionId, timestamp, cwd, version, requestId, model, toolUseId)
//! have explicit typed homes; opaque, variant-specific payloads stay as
//! `serde_json::Value` so they round-trip byte-for-byte and so unknown/future
//! record and block types decode without panicking or being miscounted as parse
//! failures. Every field is optional with `#[serde(default)]` to stay
//! backward-compatible. Mapping this raw schema onto the protocol-level
//! `TranscriptMessage`/`TranscriptBlock` types lives in `transcript.rs`.

use orbcode_protocol::{SessionGoal, SessionGoalStatus, TranscriptJsonField};
use serde::Deserialize;
use serde_json::Value;

/// Three-state decoded JSON value used when a transcript field must preserve
/// absent, present-null, and present-value separately.
type PresentJsonValue = Option<Option<Value>>;

use crate::transcript::{
    CUSTOM_TITLE_ENTRY_TYPE, GOAL_CLEARED_ENTRY_TYPE, GOAL_ENTRY_TYPE, GOAL_TURN_START_ENTRY_TYPE,
    GOAL_TURN_TERMINAL_ENTRY_TYPE, SESSION_CONTEXT_ENTRY_TYPE,
};

/// The high-value record variants the loader maps explicitly. Anything else —
/// including records with no `type` — classifies as [`TranscriptRecordKind::Unknown`]
/// and is skipped forward-compatibly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRecordKind {
    User,
    Assistant,
    System,
    Progress,
    CustomTitle,
    SessionContext,
    Goal,
    GoalCleared,
    GoalTurnStart,
    GoalTurnTerminal,
    Unknown,
}

/// One decoded transcript record (a single `.jsonl` line).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TranscriptRecord {
    #[serde(rename = "type", default)]
    pub record_type: Option<String>,
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(rename = "parentUuid", default)]
    pub parent_uuid: Option<String>,
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(
        rename = "promptId",
        default,
        deserialize_with = "deserialize_transcript_json_field"
    )]
    pub prompt_id: TranscriptJsonField,
    #[serde(
        rename = "gitBranch",
        default,
        deserialize_with = "deserialize_transcript_json_field"
    )]
    pub git_branch: TranscriptJsonField,
    #[serde(rename = "requestId", default)]
    pub request_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_transcript_json_field")]
    pub provider: TranscriptJsonField,
    #[serde(rename = "billingBasis", default)]
    pub billing_basis: Option<String>,
    #[serde(default)]
    pub message: Option<RecordMessage>,

    /// Top-level `content` string used by `system` records (local command
    /// output, snip boundaries) that carry no nested `message.content`.
    #[serde(default)]
    pub content: Option<Value>,
    /// `system` `api_error` payload inputs.
    #[serde(default)]
    pub error: Option<Value>,
    #[serde(rename = "retryAttempt", default)]
    pub retry_attempt: Option<u64>,
    #[serde(rename = "maxRetries", default)]
    pub max_retries: Option<u64>,

    /// Out-of-line tool-result metadata. The camelCase key is preferred; the
    /// snake_case key is a legacy/decompiled-TS fallback.
    #[serde(rename = "toolUseResult", default)]
    pub tool_use_result: Option<Value>,
    #[serde(rename = "tool_use_result", default)]
    pub tool_use_result_legacy: Option<Value>,

    /// `custom-title` record fields.
    #[serde(rename = "customTitle", default)]
    pub custom_title: Option<String>,
    #[serde(default)]
    pub title: Option<String>,

    /// `session-context` record fields.
    #[serde(rename = "additionalDirectories", default)]
    pub additional_directories: Option<Vec<Value>>,
    #[serde(rename = "sessionPermissions", default)]
    pub session_permissions: Option<SessionPermissionsRecord>,
    /// Double `Option` distinguishes "key absent" (outer `None`) from "key
    /// present but null/clearing" (outer `Some(None)`), mirroring the original
    /// `parsed.get("sessionEffort").is_some()` presence check that clears a
    /// prior effort override.
    #[serde(
        rename = "sessionEffort",
        default,
        deserialize_with = "deserialize_present_value"
    )]
    pub session_effort: PresentJsonValue,

    /// `goal` snapshot fields. Transcript keys are camelCase even though the
    /// canonical app-server DTO uses snake_case.
    #[serde(rename = "goalId", default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub revision: Option<u64>,
    #[serde(default)]
    pub objective: Option<String>,
    #[serde(default)]
    pub status: Option<SessionGoalStatus>,
    #[serde(rename = "tokenBudget", default)]
    pub token_budget: Option<u64>,
    #[serde(rename = "tokensUsed", default)]
    pub tokens_used: Option<u64>,
    #[serde(rename = "elapsedSeconds", default)]
    pub elapsed_seconds: Option<u64>,
    #[serde(rename = "createdAt", default)]
    pub created_at: Option<String>,
    #[serde(rename = "updatedAt", default)]
    pub updated_at: Option<String>,
    #[serde(rename = "stopReason", default)]
    pub stop_reason: Option<String>,
    #[serde(rename = "lastGoalTurnId", default)]
    pub last_goal_turn_id: Option<String>,
    #[serde(rename = "goalRevision", default)]
    pub goal_revision: Option<u64>,
    #[serde(rename = "turnId", default)]
    pub turn_id: Option<String>,
}

impl TranscriptRecord {
    /// Decode one already-parsed JSONL value into the typed envelope. Returns
    /// `None` for JSON that is not a transcript record object (e.g. a bare
    /// scalar or a field with an incompatible type), which the loader skips
    /// without counting it as a parse failure.
    pub fn from_value(value: &Value) -> Option<Self> {
        Self::deserialize(value).ok()
    }

    pub fn kind(&self) -> TranscriptRecordKind {
        match self.record_type.as_deref() {
            Some("user") => TranscriptRecordKind::User,
            Some("assistant") => TranscriptRecordKind::Assistant,
            Some("system") => TranscriptRecordKind::System,
            Some("progress") => TranscriptRecordKind::Progress,
            Some(other) if other == CUSTOM_TITLE_ENTRY_TYPE => TranscriptRecordKind::CustomTitle,
            Some(other) if other == SESSION_CONTEXT_ENTRY_TYPE => {
                TranscriptRecordKind::SessionContext
            }
            Some(other) if other == GOAL_ENTRY_TYPE => TranscriptRecordKind::Goal,
            Some(other) if other == GOAL_CLEARED_ENTRY_TYPE => TranscriptRecordKind::GoalCleared,
            Some(other) if other == GOAL_TURN_START_ENTRY_TYPE => {
                TranscriptRecordKind::GoalTurnStart
            }
            Some(other) if other == GOAL_TURN_TERMINAL_ENTRY_TYPE => {
                TranscriptRecordKind::GoalTurnTerminal
            }
            _ => TranscriptRecordKind::Unknown,
        }
    }

    /// Decode a complete goal snapshot. Malformed historical snapshots remain
    /// preserved as raw transcript records but do not replace valid state.
    pub fn session_goal(&self) -> Option<SessionGoal> {
        Some(SessionGoal {
            goal_id: self.goal_id.clone()?,
            revision: self.revision?,
            session_id: self.session_id.clone()?,
            objective: self.objective.clone()?,
            status: self.status?,
            token_budget: self.token_budget,
            tokens_used: self.tokens_used.unwrap_or_default(),
            elapsed_seconds: self.elapsed_seconds.unwrap_or_default(),
            created_at: crate::transcript::parse_timestamp(self.created_at.as_deref()?)?,
            updated_at: crate::transcript::parse_timestamp(self.updated_at.as_deref()?)?,
            stop_reason: self.stop_reason.clone(),
            last_goal_turn_id: self.last_goal_turn_id.clone(),
        })
    }

    /// Tool-result metadata source with the original precedence: top-level
    /// `toolUseResult`, then top-level `tool_use_result`, then
    /// `message.toolUseResult`.
    pub fn tool_use_result(&self) -> Option<&Value> {
        self.tool_use_result
            .as_ref()
            .or(self.tool_use_result_legacy.as_ref())
            .or_else(|| {
                self.message
                    .as_ref()
                    .and_then(|message| message.tool_use_result.as_ref())
            })
    }
}

/// The nested `message` object on user/assistant/system records.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RecordMessage {
    #[serde(default)]
    pub role: Option<String>,
    /// String (plain text) or array (content blocks); kept as `Value` so the
    /// block-level classifier can decode it lazily.
    #[serde(default)]
    pub content: Option<Value>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// Opaque provider usage payload; decoded leniently downstream.
    #[serde(default)]
    pub usage: Option<Value>,
    #[serde(rename = "toolUseResult", default)]
    pub tool_use_result: Option<Value>,
}

/// `session-context` permission edits.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SessionPermissionsRecord {
    #[serde(default)]
    pub allow: Option<Vec<Value>>,
    #[serde(default)]
    pub deny: Option<Vec<Value>>,
}

/// A single content block as stored on disk, classified by its `type`. Unknown
/// block types retain their raw value so a fallback `content` string can still
/// surface as text.
#[derive(Debug, Clone)]
pub enum RawContentBlock {
    Text(RawTextBlock),
    Thinking(RawThinkingBlock),
    RedactedThinking,
    ToolUse(RawToolUseBlock),
    ToolResult(RawToolResultBlock),
    Other(Value),
}

impl RawContentBlock {
    pub fn from_value(value: &Value) -> Self {
        match value.get("type").and_then(Value::as_str) {
            Some("text") => Self::Text(RawTextBlock::deserialize(value).unwrap_or_default()),
            Some("thinking") => {
                Self::Thinking(RawThinkingBlock::deserialize(value).unwrap_or_default())
            }
            Some("redacted_thinking") => Self::RedactedThinking,
            Some("tool_use") => {
                Self::ToolUse(RawToolUseBlock::deserialize(value).unwrap_or_default())
            }
            Some("tool_result") => {
                Self::ToolResult(RawToolResultBlock::deserialize(value).unwrap_or_default())
            }
            _ => Self::Other(value.clone()),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawTextBlock {
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawThinkingBlock {
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawToolUseBlock {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub input: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawToolResultBlock {
    #[serde(rename = "tool_use_id", alias = "toolUseId", default)]
    pub tool_use_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_transcript_json_field")]
    pub content: TranscriptJsonField,
    #[serde(rename = "is_error", alias = "isError", default)]
    pub is_error: Option<bool>,
}

/// Decode a `message.content` value into raw content blocks. A bare string maps
/// to a single text block (empty strings drop out); an array maps element-wise;
/// any other JSON shape yields no blocks.
pub fn raw_content_blocks(content: &Value) -> Vec<RawContentBlock> {
    match content {
        Value::String(text) => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![RawContentBlock::Text(RawTextBlock {
                    text: Some(text.clone()),
                })]
            }
        }
        Value::Array(items) => items.iter().map(RawContentBlock::from_value).collect(),
        _ => Vec::new(),
    }
}

/// Wrap a present value in `Some(..)` so a `#[serde(default)]` field can tell
/// "absent" (the default `None`) from "present" (including JSON `null`).
fn deserialize_present_value<'de, D>(deserializer: D) -> Result<PresentJsonValue, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<Value>::deserialize(deserializer).map(Some)
}

fn deserialize_transcript_json_field<'de, D>(
    deserializer: D,
) -> Result<TranscriptJsonField, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(TranscriptJsonField::from_present_value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_value_decodes_core_fields_into_typed_homes() {
        let record = TranscriptRecord::from_value(&json!({
            "type": "assistant",
            "uuid": "uuid-1",
            "parentUuid": "uuid-0",
            "sessionId": "session-1",
            "timestamp": "2026-01-01T00:00:00Z",
            "cwd": "/repo",
            "version": "1.2.3",
            "gitBranch": "main",
            "requestId": "req_abc",
            "provider": "anthropic",
            "message": {
                "role": "assistant",
                "model": "claude-opus-4-7",
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "hi" }],
                "usage": { "input_tokens": 1, "output_tokens": 2 },
            },
        }))
        .expect("record decodes");

        assert_eq!(record.kind(), TranscriptRecordKind::Assistant);
        assert_eq!(record.uuid.as_deref(), Some("uuid-1"));
        assert_eq!(record.parent_uuid.as_deref(), Some("uuid-0"));
        assert_eq!(record.session_id.as_deref(), Some("session-1"));
        assert_eq!(record.timestamp.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert_eq!(record.cwd.as_deref(), Some("/repo"));
        assert_eq!(record.version.as_deref(), Some("1.2.3"));
        assert_eq!(record.git_branch.as_str(), Some("main"));
        assert_eq!(record.request_id.as_deref(), Some("req_abc"));
        assert_eq!(record.provider.as_str(), Some("anthropic"));
        let message = record.message.as_ref().expect("message present");
        assert_eq!(message.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(message.stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn target_fields_and_tool_result_content_preserve_absent_null_and_value() {
        let absent = TranscriptRecord::from_value(&json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "tool-1"}]
            }
        }))
        .expect("absent fields decode");
        assert_eq!(absent.prompt_id, TranscriptJsonField::Absent);
        assert_eq!(absent.git_branch, TranscriptJsonField::Absent);
        assert_eq!(absent.provider, TranscriptJsonField::Absent);
        let absent_blocks = raw_content_blocks(
            absent
                .message
                .as_ref()
                .and_then(|message| message.content.as_ref())
                .expect("content"),
        );
        assert!(matches!(
            absent_blocks.as_slice(),
            [RawContentBlock::ToolResult(RawToolResultBlock {
                content: TranscriptJsonField::Absent,
                ..
            })]
        ));

        let null = TranscriptRecord::from_value(&json!({
            "type": "user",
            "promptId": null,
            "gitBranch": null,
            "provider": null,
            "message": {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "tool-1", "content": null}]
            }
        }))
        .expect("null fields decode");
        assert_eq!(null.prompt_id, TranscriptJsonField::Null);
        assert_eq!(null.git_branch, TranscriptJsonField::Null);
        assert_eq!(null.provider, TranscriptJsonField::Null);
        let null_blocks = raw_content_blocks(
            null.message
                .as_ref()
                .and_then(|message| message.content.as_ref())
                .expect("content"),
        );
        assert!(matches!(
            null_blocks.as_slice(),
            [RawContentBlock::ToolResult(RawToolResultBlock {
                content: TranscriptJsonField::Null,
                ..
            })]
        ));

        let value = TranscriptRecord::from_value(&json!({
            "type": "user",
            "promptId": {"future": 1},
            "gitBranch": ["future", "shape"],
            "provider": "future-provider",
            "message": {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "tool-1", "content": [{"type": "text", "text": "ok"}]}]
            }
        }))
        .expect("value fields decode");
        assert_eq!(
            value.prompt_id,
            TranscriptJsonField::Value(json!({"future": 1}))
        );
        assert_eq!(
            value.git_branch,
            TranscriptJsonField::Value(json!(["future", "shape"]))
        );
        assert_eq!(
            value.provider,
            TranscriptJsonField::Value(json!("future-provider"))
        );
        let value_blocks = raw_content_blocks(
            value
                .message
                .as_ref()
                .and_then(|message| message.content.as_ref())
                .expect("content"),
        );
        assert!(matches!(
            value_blocks.as_slice(),
            [RawContentBlock::ToolResult(RawToolResultBlock {
                content: TranscriptJsonField::Value(Value::Array(items)),
                ..
            })] if items.len() == 1
        ));
    }

    /// Transcripts written before the cc-rs -> orbcode rename carry
    /// `entrypoint: "cc-rs"`, `version: "cc-rs-tui-parity"` and synthetic
    /// assistant ids shaped `cc-rs-<uuid>`. We deliberately ship NO
    /// backward-compatibility shim for them, because none is needed:
    ///
    ///   - `entrypoint` is not a field on `TranscriptRecord` at all, so it is
    ///     never deserialized and the unknown key is simply ignored
    ///   - `version` is deserialized but only ever carried, never compared
    ///     against a literal
    ///   - `message.id` is likewise not a field on `RecordMessage`, so the
    ///     synthetic id is ignored on read too
    ///
    /// This test pins that reasoning. If someone later adds an `entrypoint`
    /// field with a match on its value, or starts branching on `version`, this
    /// fails and forces the compatibility question to be answered explicitly
    /// rather than silently dropping pre-rename sessions.
    ///
    /// The same reasoning is what lets the *writer* side move: today it stamps
    /// `TRANSCRIPT_ENTRYPOINT` / `TRANSCRIPT_VERSION` (see
    /// `crate::transcript`), and the value has already changed once since the
    /// rename without needing a read shim.
    #[test]
    fn pre_rename_provenance_values_still_decode() {
        let record = TranscriptRecord::from_value(&json!({
            "type": "assistant",
            "uuid": "uuid-1",
            "sessionId": "session-1",
            "timestamp": "2026-01-01T00:00:00Z",
            "cwd": "/repo",
            "entrypoint": "cc-rs",
            "version": "cc-rs-tui-parity",
            "message": {
                "role": "assistant",
                "id": "cc-rs-11111111-2222-3333-4444-555555555555",
                "content": [{ "type": "text", "text": "hi" }],
            },
        }))
        .expect("a pre-rename transcript line must still decode");

        assert_eq!(record.kind(), TranscriptRecordKind::Assistant);
        assert_eq!(record.version.as_deref(), Some("cc-rs-tui-parity"));
        assert_eq!(record.cwd.as_deref(), Some("/repo"));
        let message = record.message.as_ref().expect("message present");
        assert_eq!(message.role.as_deref(), Some("assistant"));
        assert!(message.content.is_some(), "content survives the decode");
    }

    #[test]
    fn unknown_and_typeless_records_classify_as_unknown() {
        let unknown = TranscriptRecord::from_value(&json!({
            "type": "file-history-snapshot",
            "uuid": "x",
        }))
        .expect("decodes");
        assert_eq!(unknown.kind(), TranscriptRecordKind::Unknown);

        let typeless = TranscriptRecord::from_value(&json!({ "uuid": "y" })).expect("decodes");
        assert_eq!(typeless.kind(), TranscriptRecordKind::Unknown);
    }

    #[test]
    fn non_object_json_is_not_a_record() {
        assert!(TranscriptRecord::from_value(&json!(42)).is_none());
        assert!(TranscriptRecord::from_value(&json!("a string")).is_none());
        assert!(TranscriptRecord::from_value(&json!([1, 2, 3])).is_none());
    }

    #[test]
    fn session_effort_distinguishes_absent_from_present_null() {
        let absent = TranscriptRecord::from_value(&json!({
            "type": SESSION_CONTEXT_ENTRY_TYPE,
            "additionalDirectories": [],
        }))
        .expect("decodes");
        assert!(absent.session_effort.is_none(), "absent key => outer None");

        let cleared = TranscriptRecord::from_value(&json!({
            "type": SESSION_CONTEXT_ENTRY_TYPE,
            "sessionEffort": Value::Null,
        }))
        .expect("decodes");
        assert!(
            matches!(cleared.session_effort, Some(None)),
            "present-null => Some(None) so a prior effort is cleared"
        );

        let set = TranscriptRecord::from_value(&json!({
            "type": SESSION_CONTEXT_ENTRY_TYPE,
            "sessionEffort": "high",
        }))
        .expect("decodes");
        assert!(matches!(
            set.session_effort,
            Some(Some(Value::String(ref value))) if value == "high"
        ));
    }

    #[test]
    fn tool_use_result_precedence_prefers_camel_then_snake_then_message() {
        let camel = TranscriptRecord::from_value(&json!({
            "type": "user",
            "toolUseResult": { "source": "camel" },
            "tool_use_result": { "source": "snake" },
            "message": { "toolUseResult": { "source": "message" } },
        }))
        .expect("decodes");
        assert_eq!(
            camel
                .tool_use_result()
                .and_then(|v| v.get("source"))
                .and_then(Value::as_str),
            Some("camel")
        );

        let snake = TranscriptRecord::from_value(&json!({
            "type": "user",
            "tool_use_result": { "source": "snake" },
            "message": { "toolUseResult": { "source": "message" } },
        }))
        .expect("decodes");
        assert_eq!(
            snake
                .tool_use_result()
                .and_then(|v| v.get("source"))
                .and_then(Value::as_str),
            Some("snake")
        );

        let message = TranscriptRecord::from_value(&json!({
            "type": "user",
            "message": { "toolUseResult": { "source": "message" } },
        }))
        .expect("decodes");
        assert_eq!(
            message
                .tool_use_result()
                .and_then(|v| v.get("source"))
                .and_then(Value::as_str),
            Some("message")
        );
    }

    #[test]
    fn raw_content_blocks_classify_each_variant() {
        let blocks = raw_content_blocks(&json!([
            { "type": "text", "text": "hello" },
            { "type": "thinking", "thinking": "ponder", "signature": "sig" },
            { "type": "redacted_thinking", "data": "AbC==" },
            { "type": "tool_use", "id": "t1", "name": "Read", "input": { "file_path": "x" } },
            { "type": "tool_result", "tool_use_id": "t1", "content": "ok", "is_error": false },
            { "type": "image", "content": "fallback text" },
        ]));

        assert!(matches!(blocks[0], RawContentBlock::Text(_)));
        assert!(matches!(blocks[1], RawContentBlock::Thinking(_)));
        assert!(matches!(blocks[2], RawContentBlock::RedactedThinking));
        assert!(matches!(blocks[3], RawContentBlock::ToolUse(_)));
        assert!(matches!(blocks[4], RawContentBlock::ToolResult(_)));
        assert!(matches!(blocks[5], RawContentBlock::Other(_)));

        if let RawContentBlock::ToolResult(result) = &blocks[4] {
            assert_eq!(result.tool_use_id.as_deref(), Some("t1"));
            assert_eq!(result.is_error, Some(false));
        } else {
            panic!("expected tool_result block");
        }
    }

    #[test]
    fn raw_content_blocks_accepts_bare_string_and_legacy_tool_result_keys() {
        let from_string = raw_content_blocks(&json!("just text"));
        assert!(matches!(from_string.as_slice(), [RawContentBlock::Text(_)]));
        assert!(raw_content_blocks(&json!("")).is_empty());

        let legacy = raw_content_blocks(&json!([
            { "type": "tool_result", "toolUseId": "legacy", "isError": true, "content": "boom" },
        ]));
        if let [RawContentBlock::ToolResult(result)] = legacy.as_slice() {
            assert_eq!(result.tool_use_id.as_deref(), Some("legacy"));
            assert_eq!(result.is_error, Some(true));
        } else {
            panic!("expected one tool_result block, got {legacy:?}");
        }
    }
}
