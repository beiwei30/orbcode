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

/// Wire discriminator for an inbound response to a server-initiated request.
pub const SERVER_RESPONSE_TYPE: &str = "server_response";

/// Generic SDK response ingress. The request-specific payload stays opaque at
/// this boundary and is decoded by the owner of the server request.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ServerResponseInputEnvelope {
    pub request_id: String,
    pub response: Value,
}

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
    /// `{"subtype":"interrupt"}` — cancel the active turn.
    Interrupt,
    /// `{"subtype":"set_permission_mode","mode":"<PermissionMode>"}` — change the
    /// permission mode for subsequent tool execution. `mode` carries the raw SDK
    /// string (e.g. `bypassPermissions`); the consumer validates it.
    SetPermissionMode { mode: String },
    /// `{"subtype":"get_session_state"}` — return session summary.
    GetSessionState,
    /// `{"subtype":"get_context_usage"}` — return context usage breakdown.
    GetContextUsage,
    /// `{"subtype":"set_max_thinking_tokens","max_thinking_tokens":N|null}` —
    /// parse a requested thinking-budget override. `null` requests clearing the
    /// override (maps to `None`); an integer requests setting it (maps to
    /// `Some(n)`). Non-integer values are rejected at deserialization time. The
    /// CLI may still return an unsupported response if no runtime effect is
    /// wired for the override.
    SetMaxThinkingTokens { max_thinking_tokens: Option<u32> },
    /// Any other recognized control-request subtype. The CLI replies with a
    /// structured "unsupported" error rather than crashing.
    #[serde(other)]
    Other,
}

/// Envelope wrapping a [`ControlRequest`] with its correlation id, matching the
/// SDK wire shape `{"type":"control_request","request_id":"...","request":{...}}`.
/// The `type` field is validated by the consumer and intentionally omitted here
/// so the struct can be deserialized straight from the raw line (unknown fields
/// are ignored).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlRequestEnvelope {
    pub request_id: String,
    pub request: ControlRequest,
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
    fn server_response_input_keeps_typed_payload_opaque() {
        let envelope: ServerResponseInputEnvelope = serde_json::from_str(
            r#"{"type":"server_response","request_id":"ask-1","response":{"outcome":"clarify"}}"#,
        )
        .unwrap();
        assert_eq!(envelope.request_id, "ask-1");
        assert_eq!(envelope.response["outcome"], "clarify");
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
