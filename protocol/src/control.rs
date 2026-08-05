//! Transport-facing wire types for the headless SDK control protocol.
//!
//! This module defines the serde DTOs that represent the bidirectional control
//! channel between an SDK host and the headless CLI (`--input-format
//! stream-json`). The types here form the **transport boundary**: any crate that
//! needs to parse or emit control frames can depend on `protocol` alone —
//! no CLI internals required.
//!
//! **Inbound** (SDK → CLI): [`ControlRequestEnvelope`] wrapping a
//! [`ControlRequest`], discriminated by `subtype`. Unknown subtypes deserialize
//! to [`ControlRequest::Other`] for graceful "unsupported" handling.
//!
//! **Outbound** (CLI → SDK): [`ControlResponseEnvelope`] wrapping a
//! [`ControlResponse`] (`success` | `error`), correlated by `request_id`.
//!
//! **User messages**: [`extract_user_message_text`] parses the SDK `user` frame
//! content (string or block-array form) without any CLI dependency.
//!
//! CLI-private concerns (frame classification, permission-mode validation,
//! stdin line parsing) live in `cli/src/control.rs` and are **not** part of this
//! transport surface.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Wire `type` discriminator for an inbound SDK control request frame
/// (`{"type":"control_request",...}`).
pub const CONTROL_REQUEST_TYPE: &str = "control_request";

/// Wire `type` discriminator for an outbound SDK control response frame
/// (`{"type":"control_response",...}`).
pub const CONTROL_RESPONSE_TYPE: &str = "control_response";

/// SDK controls with intentionally supported behavior in duplex stream-json.
///
/// This is the canonical support inventory consumed by initialization responses
/// and documentation tests. `can_use_tool` is included even though it travels
/// CLI -> host; all other entries are host -> CLI requests.
pub const SUPPORTED_CONTROL_SUBTYPES: &[&str] = &[
    "initialize",
    "interrupt",
    "can_use_tool",
    "set_permission_mode",
    "get_session_state",
    "get_context_usage",
    "mcp_status",
    "set_model",
    "set_max_thinking_tokens",
    "seed_read_state",
    "cancel_async_message",
];

/// SDK controls that are recognized but deliberately unavailable.
pub const UNSUPPORTED_CONTROL_SUBTYPES: &[&str] = &["rewind_files"];

/// Inner payload of an SDK `control_request`, discriminated by `subtype`.
///
/// Mirrors the request union in the TypeScript SDK `controlSchemas.ts`. Only the
/// subtypes the headless CLI implements are modeled explicitly; every other
/// recognized control request deserializes to [`ControlRequest::Other`] so an
/// unknown frame yields a structured "unsupported" response instead of a hard
/// parse failure. The correlation id lives on the surrounding
/// [`ControlRequestEnvelope`], matching the wire layout.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "subtype", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ControlRequest {
    /// `{"subtype":"initialize"}` — return correlated SDK capabilities and
    /// authoritative session bootstrap state. Extra SDK initialization fields
    /// are intentionally ignored by this compatibility adapter.
    Initialize,
    /// `{"subtype":"interrupt"}` — cancel the active turn.
    Interrupt,
    /// `{"subtype":"can_use_tool",...}` — server-originated permission
    /// callback delivered to the SDK host. Tool input is intentionally opaque
    /// JSON while all routing/correlation fields remain typed.
    CanUseTool {
        tool_name: String,
        input: Value,
        tool_use_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocked_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision_reason: Option<String>,
    },
    /// `{"subtype":"set_permission_mode","mode":"<PermissionMode>"}` — change the
    /// permission mode for subsequent tool execution. `mode` carries the raw SDK
    /// string (e.g. `bypassPermissions`); the consumer validates it.
    SetPermissionMode { mode: String },
    /// `{"subtype":"get_session_state"}` — return session summary.
    GetSessionState,
    /// `{"subtype":"get_context_usage"}` — return context usage breakdown.
    GetContextUsage,
    /// `{"subtype":"mcp_status"}` — return a secret-free MCP status view.
    McpStatus,
    /// `{"subtype":"set_model","model":"..."}` — change the model used by
    /// the next provider request. Missing/null/default clears the override.
    SetModel {
        #[serde(default)]
        model: Option<String>,
    },
    /// `{"subtype":"set_max_thinking_tokens","max_thinking_tokens":N|null}` —
    /// parse a requested thinking-budget override. `null` requests clearing the
    /// override (maps to `None`); an integer requests setting it (maps to
    /// `Some(n)`). Non-integer values are rejected at deserialization time. The
    /// CLI may still return an unsupported response if no runtime effect is
    /// wired for the override.
    SetMaxThinkingTokens { max_thinking_tokens: Option<u32> },
    /// `{"subtype":"seed_read_state","path":"...","mtime":N}` — seed the
    /// shared stale-write guard after validating the current file identity.
    SeedReadState { path: String, mtime: u64 },
    /// `{"subtype":"rewind_files",...}` — recognized but unsupported until a
    /// real file checkpoint/restore contract exists.
    RewindFiles {
        user_message_id: String,
        #[serde(default)]
        dry_run: bool,
    },
    /// `{"subtype":"cancel_async_message","message_uuid":"..."}` — cancel
    /// the single owned background task identified by the UUID.
    CancelAsyncMessage { message_uuid: String },
    /// Any other recognized control-request subtype. The CLI replies with a
    /// structured "unsupported" error rather than crashing.
    #[serde(other)]
    Other,
}

/// Envelope wrapping a [`ControlRequest`] with its correlation id, matching the
/// SDK wire shape `{"type":"control_request","request_id":"...","request":{...}}`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlRequestEnvelope {
    #[serde(rename = "type", default = "control_request_type")]
    pub request_type: String,
    pub request_id: String,
    pub request: ControlRequest,
}

impl ControlRequestEnvelope {
    pub fn new(request_id: impl Into<String>, request: ControlRequest) -> Self {
        Self {
            request_type: CONTROL_REQUEST_TYPE.to_string(),
            request_id: request_id.into(),
            request,
        }
    }
}

fn control_request_type() -> String {
    CONTROL_REQUEST_TYPE.to_string()
}

/// Outbound envelope wrapping a [`ControlResponse`] for the SDK wire shape
/// `{"type":"control_response","response":{...}}`. The `type` field is always
/// [`CONTROL_RESPONSE_TYPE`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlResponseEnvelope {
    #[serde(rename = "type")]
    pub response_type: String,
    pub response: ControlResponse,
}

impl ControlResponseEnvelope {
    pub fn success(request_id: impl Into<String>) -> Self {
        Self {
            response_type: CONTROL_RESPONSE_TYPE.to_string(),
            response: ControlResponse::Success {
                request_id: request_id.into(),
                response: None,
            },
        }
    }

    pub fn success_with_data(request_id: impl Into<String>, data: Value) -> Self {
        Self {
            response_type: CONTROL_RESPONSE_TYPE.to_string(),
            response: ControlResponse::Success {
                request_id: request_id.into(),
                response: Some(data),
            },
        }
    }

    /// Build a success response from a typed SDK projection.
    pub fn success_with<T: Serialize>(
        request_id: impl Into<String>,
        data: &T,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self::success_with_data(
            request_id,
            serde_json::to_value(data)?,
        ))
    }

    pub fn error(request_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            response_type: CONTROL_RESPONSE_TYPE.to_string(),
            response: ControlResponse::Error {
                request_id: request_id.into(),
                error: error.into(),
            },
        }
    }
}

/// Inner body of a [`ControlResponseEnvelope`], discriminated by `subtype`.
/// Mirrors the TypeScript SDK `SDKControlResponseSchema` union.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "subtype", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ControlResponse {
    Success {
        request_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        response: Option<Value>,
    },
    Error {
        request_id: String,
        error: String,
    },
}

/// Host decision returned for a server-originated `can_use_tool` request.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum ToolPermissionResult {
    Allow {
        #[serde(
            default,
            rename = "updatedInput",
            skip_serializing_if = "Option::is_none"
        )]
        updated_input: Option<Value>,
        #[serde(default, rename = "toolUseID", skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
    },
    Deny {
        #[serde(default)]
        message: String,
        #[serde(default)]
        interrupt: bool,
        #[serde(default, rename = "toolUseID", skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
    },
}

/// Typed projection returned by `initialize` on the SDK control channel.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdkInitializeResponse {
    pub protocol_version: String,
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub permission_mode: String,
    pub tools: Vec<String>,
    #[serde(rename = "mcpServers")]
    pub mcp_servers: Vec<SdkMcpServerStatus>,
    pub supported_controls: Vec<String>,
}

/// Typed `get_session_state` success payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdkSessionStateResponse {
    pub session_id: String,
    pub cwd: String,
    pub model_display_name: String,
    pub model_name: String,
    pub model_capabilities: Vec<String>,
    pub effort_level: Option<String>,
    pub max_thinking_tokens: Option<u32>,
    pub default_provider: String,
    pub fallback_provider: Option<String>,
    pub sandbox_mode: String,
    pub persisted_session_count: usize,
    pub background_job_count: usize,
    pub available_tool_count: usize,
    pub configured_mcp_server_count: usize,
}

/// Token categories in the typed `get_context_usage` success payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdkContextCategoriesResponse {
    pub system_prompt: u32,
    pub system_tools: u32,
    pub mcp_tools: u32,
    pub memory: u32,
    pub skills: u32,
    pub conversation: u32,
    pub attachments: u32,
    pub uncategorized: u32,
}

/// Typed `get_context_usage` success payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdkContextUsageResponse {
    pub model: String,
    pub max_thinking_tokens: Option<u32>,
    pub estimated_tokens: u32,
    pub categories: SdkContextCategoriesResponse,
    pub context_window: u32,
    pub effective_context_window: u32,
    pub free_space_tokens: u32,
    pub percent_left: u32,
    pub is_above_auto_compact_threshold: bool,
    pub is_above_warning_threshold: bool,
    pub is_above_error_threshold: bool,
    pub is_at_blocking_limit: bool,
}

/// Secret-free MCP status entry used by initialize and `mcp_status`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdkMcpServerStatus {
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Typed `mcp_status` success payload. The camelCase field matches the SDK.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdkMcpStatusResponse {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: Vec<SdkMcpServerStatus>,
}

/// Typed `set_model` success payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdkModelChangeResponse {
    pub provider: String,
    pub model: String,
    pub display_name: String,
}

/// Typed `set_max_thinking_tokens` success payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdkThinkingBudgetResponse {
    pub max_thinking_tokens: Option<u32>,
}

/// Typed `seed_read_state` success payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdkSeedReadStateResponse {
    pub path: String,
    pub mtime: u64,
    pub seeded: bool,
}

/// Typed cancellation outcome for `cancel_async_message`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AsyncCancellationOutcome {
    Signalled,
    AlreadyTerminal,
    NotFound,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdkAsyncCancellationResponse {
    pub task_id: String,
    pub outcome: AsyncCancellationOutcome,
    pub cancelled: bool,
}

/// Extract the prompt text from a stream-json `user` message. Accepts both the
/// string content form (`"content":"hello"`) and the Anthropic block-array form
/// (`"content":[{"type":"text","text":"…"}]`), concatenating text blocks. Returns
/// `None` when no text is present so the caller can report a schema error.
///
/// The `value` should be the top-level JSON object of the `user` frame; the
/// function tolerates both `{"message":{"content":…}}` and `{"content":…}`.
pub fn extract_user_message_text(value: &Value) -> Option<String> {
    let message = value.get("message").unwrap_or(value);
    let content = message.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    if let Some(parts) = content.as_array() {
        let mut buffer = String::new();
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if !buffer.is_empty() {
                    buffer.push('\n');
                }
                buffer.push_str(text);
            }
        }
        if !buffer.is_empty() {
            return Some(buffer);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_request_round_trips() {
        let line =
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"interrupt"}}"#;
        let envelope: ControlRequestEnvelope = serde_json::from_str(line).expect("parse interrupt");
        assert_eq!(envelope.request_id, "req-1");
        assert_eq!(envelope.request, ControlRequest::Interrupt);
    }

    #[test]
    fn set_permission_mode_request_carries_raw_mode() {
        let line = r#"{"type":"control_request","request_id":"req-2","request":{"subtype":"set_permission_mode","mode":"bypassPermissions"}}"#;
        let envelope: ControlRequestEnvelope =
            serde_json::from_str(line).expect("parse set_permission_mode");
        assert_eq!(envelope.request_id, "req-2");
        assert_eq!(
            envelope.request,
            ControlRequest::SetPermissionMode {
                mode: "bypassPermissions".to_string(),
            }
        );
    }

    #[test]
    fn unknown_subtype_falls_back_to_other() {
        let line =
            r#"{"type":"control_request","request_id":"req-3","request":{"subtype":"frobnicate"}}"#;
        let envelope: ControlRequestEnvelope =
            serde_json::from_str(line).expect("unknown subtype still parses");
        assert_eq!(envelope.request, ControlRequest::Other);
    }

    #[test]
    fn set_permission_mode_missing_mode_is_a_schema_error() {
        let line = r#"{"type":"control_request","request_id":"req-4","request":{"subtype":"set_permission_mode"}}"#;
        let parsed: Result<ControlRequestEnvelope, _> = serde_json::from_str(line);
        assert!(
            parsed.is_err(),
            "a known subtype with a missing required field must not silently fall through to Other"
        );
    }

    #[test]
    fn get_session_state_request_round_trips() {
        let line = r#"{"type":"control_request","request_id":"req-5","request":{"subtype":"get_session_state"}}"#;
        let envelope: ControlRequestEnvelope =
            serde_json::from_str(line).expect("parse get_session_state");
        assert_eq!(envelope.request_id, "req-5");
        assert_eq!(envelope.request, ControlRequest::GetSessionState);
    }

    #[test]
    fn get_context_usage_request_round_trips() {
        let line = r#"{"type":"control_request","request_id":"req-6","request":{"subtype":"get_context_usage"}}"#;
        let envelope: ControlRequestEnvelope =
            serde_json::from_str(line).expect("parse get_context_usage");
        assert_eq!(envelope.request_id, "req-6");
        assert_eq!(envelope.request, ControlRequest::GetContextUsage);
    }

    #[test]
    fn set_max_thinking_tokens_carries_number() {
        let line = r#"{"type":"control_request","request_id":"req-7","request":{"subtype":"set_max_thinking_tokens","max_thinking_tokens":4096}}"#;
        let envelope: ControlRequestEnvelope =
            serde_json::from_str(line).expect("parse set_max_thinking_tokens");
        assert_eq!(envelope.request_id, "req-7");
        assert_eq!(
            envelope.request,
            ControlRequest::SetMaxThinkingTokens {
                max_thinking_tokens: Some(4096),
            }
        );
    }

    #[test]
    fn set_max_thinking_tokens_accepts_null() {
        let line = r#"{"type":"control_request","request_id":"req-8","request":{"subtype":"set_max_thinking_tokens","max_thinking_tokens":null}}"#;
        let envelope: ControlRequestEnvelope =
            serde_json::from_str(line).expect("parse set_max_thinking_tokens null");
        assert_eq!(
            envelope.request,
            ControlRequest::SetMaxThinkingTokens {
                max_thinking_tokens: None,
            }
        );
    }

    #[test]
    fn set_max_thinking_tokens_rejects_string_at_deser() {
        let line = r#"{"type":"control_request","request_id":"req-9","request":{"subtype":"set_max_thinking_tokens","max_thinking_tokens":"lots"}}"#;
        let parsed: Result<ControlRequestEnvelope, _> = serde_json::from_str(line);
        assert!(
            parsed.is_err(),
            "a non-integer max_thinking_tokens must fail deserialization"
        );
    }

    #[test]
    fn response_success_matches_wire_shape() {
        let envelope = ControlResponseEnvelope::success("req-1");
        let value = serde_json::to_value(&envelope).expect("serialize");
        assert_eq!(value["type"], "control_response");
        assert_eq!(value["response"]["subtype"], "success");
        assert_eq!(value["response"]["request_id"], "req-1");
        assert!(
            value["response"].get("response").is_none(),
            "success without data must not carry a response field"
        );
        assert!(
            value["response"].get("error").is_none(),
            "success must not carry an error field"
        );
    }

    #[test]
    fn response_success_with_data_matches_wire_shape() {
        let data = serde_json::json!({"model": "claude-test", "tokens": 42});
        let envelope = ControlResponseEnvelope::success_with_data("req-2", data);
        let value = serde_json::to_value(&envelope).expect("serialize");
        assert_eq!(value["type"], "control_response");
        assert_eq!(value["response"]["subtype"], "success");
        assert_eq!(value["response"]["request_id"], "req-2");
        assert_eq!(value["response"]["response"]["model"], "claude-test");
        assert_eq!(value["response"]["response"]["tokens"], 42);
    }

    #[test]
    fn response_error_matches_wire_shape() {
        let envelope = ControlResponseEnvelope::error("req-3", "unsupported control request");
        let value = serde_json::to_value(&envelope).expect("serialize");
        assert_eq!(value["type"], "control_response");
        assert_eq!(value["response"]["subtype"], "error");
        assert_eq!(value["response"]["request_id"], "req-3");
        assert_eq!(value["response"]["error"], "unsupported control request");
    }

    #[test]
    fn response_success_round_trips_through_json() {
        let original = ControlResponseEnvelope::success("req-rt");
        let json_str = serde_json::to_string(&original).expect("serialize");
        let parsed: ControlResponseEnvelope = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(original, parsed);
    }

    #[test]
    fn response_error_round_trips_through_json() {
        let original = ControlResponseEnvelope::error("req-rt", "bad request");
        let json_str = serde_json::to_string(&original).expect("serialize");
        let parsed: ControlResponseEnvelope = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(original, parsed);
    }

    #[test]
    fn current_control_frames_are_byte_locked() {
        let request = ControlRequestEnvelope::new(
            "req-wire",
            ControlRequest::SetPermissionMode {
                mode: "bypassPermissions".to_string(),
            },
        );
        assert_eq!(
            serde_json::to_string(&request).expect("request JSON"),
            r#"{"type":"control_request","request_id":"req-wire","request":{"subtype":"set_permission_mode","mode":"bypassPermissions"}}"#
        );
        assert_eq!(
            serde_json::to_string(&ControlResponseEnvelope::success("req-wire"))
                .expect("response JSON"),
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-wire"}}"#
        );
        assert_eq!(
            serde_json::to_string(&ControlResponseEnvelope::error("req-wire", "unsupported"))
                .expect("error JSON"),
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"req-wire","error":"unsupported"}}"#
        );
    }

    #[test]
    fn all_new_sdk_request_variants_have_typed_serde_coverage() {
        let cases = [
            (r#"{"subtype":"initialize"}"#, ControlRequest::Initialize),
            (r#"{"subtype":"mcp_status"}"#, ControlRequest::McpStatus),
            (
                r#"{"subtype":"set_model","model":"claude-test"}"#,
                ControlRequest::SetModel {
                    model: Some("claude-test".to_string()),
                },
            ),
            (
                r#"{"subtype":"seed_read_state","path":"src/lib.rs","mtime":42}"#,
                ControlRequest::SeedReadState {
                    path: "src/lib.rs".to_string(),
                    mtime: 42,
                },
            ),
            (
                r#"{"subtype":"rewind_files","user_message_id":"msg-1","dry_run":true}"#,
                ControlRequest::RewindFiles {
                    user_message_id: "msg-1".to_string(),
                    dry_run: true,
                },
            ),
            (
                r#"{"subtype":"cancel_async_message","message_uuid":"task-1"}"#,
                ControlRequest::CancelAsyncMessage {
                    message_uuid: "task-1".to_string(),
                },
            ),
        ];
        for (wire, expected) in cases {
            let request: ControlRequest = serde_json::from_str(wire).expect(wire);
            assert_eq!(request, expected);
            assert_eq!(serde_json::to_string(&request).expect("serialize"), wire);
        }
    }

    #[test]
    fn can_use_tool_and_permission_result_lock_sdk_field_casing() {
        let request = ControlRequestEnvelope::new(
            "permission-1",
            ControlRequest::CanUseTool {
                tool_name: "Edit".to_string(),
                input: serde_json::json!({"file_path": "src/lib.rs"}),
                tool_use_id: "tool-1".to_string(),
                blocked_path: Some("src/lib.rs".to_string()),
                decision_reason: Some("ask rule".to_string()),
            },
        );
        let request_json = serde_json::to_string(&request).expect("request JSON");
        assert_eq!(
            request_json,
            r#"{"type":"control_request","request_id":"permission-1","request":{"subtype":"can_use_tool","tool_name":"Edit","input":{"file_path":"src/lib.rs"},"tool_use_id":"tool-1","blocked_path":"src/lib.rs","decision_reason":"ask rule"}}"#
        );
        let allow = ToolPermissionResult::Allow {
            updated_input: Some(serde_json::json!({"file_path": "src/lib.rs"})),
            tool_use_id: Some("tool-1".to_string()),
        };
        let allow_json = serde_json::to_string(&allow).expect("allow JSON");
        assert_eq!(
            allow_json,
            r#"{"behavior":"allow","updatedInput":{"file_path":"src/lib.rs"},"toolUseID":"tool-1"}"#
        );
        assert_eq!(
            serde_json::from_str::<ToolPermissionResult>(&allow_json).expect("allow parse"),
            allow
        );
    }

    #[test]
    fn typed_sdk_success_projections_lock_camel_case() {
        let response = SdkInitializeResponse {
            protocol_version: "1.0".to_string(),
            session_id: "session-1".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-test".to_string(),
            permission_mode: "default".to_string(),
            tools: vec!["Read".to_string()],
            mcp_servers: vec![SdkMcpServerStatus {
                name: "docs".to_string(),
                status: "connected".to_string(),
                error: None,
            }],
            supported_controls: vec!["initialize".to_string()],
        };
        let value = serde_json::to_value(response).expect("initialize value");
        assert!(value.get("mcpServers").is_some());
        assert!(value.get("mcp_servers").is_none());

        let session_value = serde_json::json!({
            "session_id": "session-1",
            "cwd": "/tmp/project",
            "model_display_name": "Claude Test",
            "model_name": "claude-test",
            "model_capabilities": ["thinking"],
            "effort_level": "high",
            "max_thinking_tokens": 4096,
            "default_provider": "anthropic",
            "fallback_provider": null,
            "sandbox_mode": "workspace_write",
            "persisted_session_count": 1,
            "background_job_count": 0,
            "available_tool_count": 12,
            "configured_mcp_server_count": 2
        });
        let session: SdkSessionStateResponse =
            serde_json::from_value(session_value.clone()).expect("session projection");
        assert_eq!(
            serde_json::to_value(session).expect("session value"),
            session_value
        );

        let context_value = serde_json::json!({
            "model": "claude-test",
            "max_thinking_tokens": 4096,
            "estimated_tokens": 100,
            "categories": {
                "system_prompt": 10,
                "system_tools": 20,
                "mcp_tools": 0,
                "memory": 5,
                "skills": 0,
                "conversation": 65,
                "attachments": 0,
                "uncategorized": 0
            },
            "context_window": 200000,
            "effective_context_window": 180000,
            "free_space_tokens": 179900,
            "percent_left": 99,
            "is_above_auto_compact_threshold": false,
            "is_above_warning_threshold": false,
            "is_above_error_threshold": false,
            "is_at_blocking_limit": false
        });
        let context: SdkContextUsageResponse =
            serde_json::from_value(context_value.clone()).expect("context projection");
        assert_eq!(
            serde_json::to_value(context).expect("context value"),
            context_value
        );
    }

    #[test]
    fn extract_string_content() {
        let value: Value =
            serde_json::from_str(r#"{"message":{"role":"user","content":"hello"}}"#).unwrap();
        assert_eq!(extract_user_message_text(&value), Some("hello".to_string()));
    }

    #[test]
    fn extract_block_array_content() {
        let value: Value = serde_json::from_str(
            r#"{"message":{"role":"user","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}"#,
        )
        .unwrap();
        assert_eq!(extract_user_message_text(&value), Some("a\nb".to_string()));
    }

    #[test]
    fn extract_empty_array_returns_none() {
        let value: Value =
            serde_json::from_str(r#"{"message":{"role":"user","content":[]}}"#).unwrap();
        assert_eq!(extract_user_message_text(&value), None);
    }

    #[test]
    fn extract_missing_content_returns_none() {
        let value: Value = serde_json::from_str(r#"{"message":{"role":"user"}}"#).unwrap();
        assert_eq!(extract_user_message_text(&value), None);
    }
}
