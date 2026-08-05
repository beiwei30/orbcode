//! CLI-private frame classification and validation for the headless control
//! channel (`orbcode -p --input-format stream-json`).
//!
//! This module sits above the transport boundary defined in
//! [`orbcode_protocol::control`]. It reads raw NDJSON lines from stdin, parses
//! them through the transport-facing wire DTOs ([`ControlRequestEnvelope`],
//! [`extract_user_message_text`]), and classifies each line into a
//! [`ControlFrame`] — a CLI-specific validated action that the headless runner
//! can dispatch directly.
//!
//! **CLI-private** types (`ControlFrame`, `parse_control_line`) depend on
//! CLI-layer validation (e.g. [`PermissionMode::parse`]) and are intentionally
//! not exported from `protocol`. Consumers that need only wire-level parsing
//! should depend on `protocol` directly.
//!
//! Parsing is deliberately total — [`parse_control_line`] never panics and
//! never returns an `Err`; malformed or unsupported input is captured as a
//! [`ControlFrame`] variant the caller can turn into a structured response, so
//! a single bad line can never crash the process.

use orbcode_app_server::PermissionMode;
use orbcode_protocol::{
    CONTROL_REQUEST_TYPE, CONTROL_RESPONSE_TYPE, ControlRequest, ControlRequestEnvelope,
    ControlResponse, ControlResponseEnvelope, extract_user_message_text,
};
use serde_json::Value;

/// A single classified control-input line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlFrame {
    /// A `user` message carrying an incremental prompt to run as a turn.
    UserPrompt(String),
    /// Idempotent SDK control-channel initialization.
    Initialize {
        request_id: String,
    },
    /// `control_request` `{"subtype":"interrupt"}` — cancel the active turn.
    Interrupt {
        request_id: String,
    },
    /// `control_request` `{"subtype":"set_permission_mode","mode":"…"}` with a
    /// validated mode.
    SetPermissionMode {
        request_id: String,
        mode: PermissionMode,
    },
    /// `control_request` `{"subtype":"get_session_state"}` — return session summary.
    GetSessionState {
        request_id: String,
    },
    /// `control_request` `{"subtype":"get_context_usage"}` — return context usage.
    GetContextUsage {
        request_id: String,
    },
    McpStatus {
        request_id: String,
    },
    SetModel {
        request_id: String,
        model: Option<String>,
    },
    /// `control_request` `{"subtype":"set_max_thinking_tokens","max_thinking_tokens":N|null}`.
    SetMaxThinkingTokens {
        request_id: String,
        max_thinking_tokens: Option<u32>,
    },
    SeedReadState {
        request_id: String,
        path: String,
        mtime: u64,
    },
    RewindFiles {
        request_id: String,
    },
    CancelAsyncMessage {
        request_id: String,
        message_uuid: String,
    },
    /// Generic host response to an earlier CLI-originated server request. The
    /// dispatcher validates the payload against the pending request kind.
    ServerResponse {
        request_id: String,
        result: Result<Value, String>,
    },
    /// A recognized `control_request` whose subtype the CLI does not implement.
    /// The caller replies with a structured "unsupported" error.
    Unsupported {
        request_id: String,
        subtype: String,
    },
    /// A non-actionable line (blank, `keep_alive`, `control_cancel_request`).
    /// No response is emitted.
    Ignore,
    /// Malformed or schema-invalid input. `request_id` is present only when it
    /// could be recovered from the frame (so an error response can be correlated);
    /// otherwise the caller logs the diagnostic to stderr.
    ParseError {
        request_id: Option<String>,
        message: String,
    },
}

/// Classify one NDJSON control-input line. Total function: every input maps to a
/// [`ControlFrame`], never a panic and never an `Err`.
pub(crate) fn parse_control_line(line: &str) -> ControlFrame {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ControlFrame::Ignore;
    }

    let value: Value = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(error) => {
            return ControlFrame::ParseError {
                request_id: None,
                message: format!("invalid JSON on stdin: {error}"),
            };
        }
    };

    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return ControlFrame::ParseError {
            request_id: recover_request_id(&value),
            message: "stream-json input line missing `type` field".to_string(),
        };
    };

    match kind {
        "user" => match extract_user_message_text(&value) {
            Some(text) => ControlFrame::UserPrompt(text),
            None => ControlFrame::ParseError {
                request_id: None,
                message: "user message has no text content".to_string(),
            },
        },
        CONTROL_REQUEST_TYPE => parse_control_request(value),
        CONTROL_RESPONSE_TYPE => parse_control_response(value),
        "keep_alive" | "control_cancel_request" => ControlFrame::Ignore,
        other => ControlFrame::ParseError {
            request_id: recover_request_id(&value),
            message: format!("unsupported stream-json input type `{other}`"),
        },
    }
}

fn parse_control_request(value: Value) -> ControlFrame {
    let Some(request_id) = value.get("request_id").and_then(Value::as_str) else {
        return ControlFrame::ParseError {
            request_id: None,
            message: "control_request missing `request_id`".to_string(),
        };
    };
    let request_id = request_id.to_string();
    // Captured before consuming `value` so an unsupported subtype can be named
    // in the error response even though `ControlRequest::Other` discards it.
    let subtype = value
        .pointer("/request/subtype")
        .and_then(Value::as_str)
        .map(str::to_string);

    let envelope: ControlRequestEnvelope = match serde_json::from_value(value) {
        Ok(envelope) => envelope,
        Err(error) => {
            return ControlFrame::ParseError {
                request_id: Some(request_id),
                message: format!("invalid control_request: {error}"),
            };
        }
    };

    match envelope.request {
        ControlRequest::Initialize => ControlFrame::Initialize { request_id },
        ControlRequest::Interrupt => ControlFrame::Interrupt { request_id },
        ControlRequest::SetPermissionMode { mode } => match PermissionMode::parse(&mode) {
            Some(mode) => ControlFrame::SetPermissionMode { request_id, mode },
            None => ControlFrame::ParseError {
                request_id: Some(request_id),
                message: format!("unknown permission mode `{mode}`"),
            },
        },
        ControlRequest::GetSessionState => ControlFrame::GetSessionState { request_id },
        ControlRequest::GetContextUsage => ControlFrame::GetContextUsage { request_id },
        ControlRequest::McpStatus => ControlFrame::McpStatus { request_id },
        ControlRequest::SetModel { model } => ControlFrame::SetModel { request_id, model },
        ControlRequest::SetMaxThinkingTokens {
            max_thinking_tokens,
        } => ControlFrame::SetMaxThinkingTokens {
            request_id,
            max_thinking_tokens,
        },
        ControlRequest::SeedReadState { path, mtime } => ControlFrame::SeedReadState {
            request_id,
            path,
            mtime,
        },
        ControlRequest::RewindFiles { .. } => ControlFrame::RewindFiles { request_id },
        ControlRequest::CancelAsyncMessage { message_uuid } => ControlFrame::CancelAsyncMessage {
            request_id,
            message_uuid,
        },
        ControlRequest::Other => ControlFrame::Unsupported {
            request_id,
            subtype: subtype.unwrap_or_else(|| "unknown".to_string()),
        },
        _ => ControlFrame::Unsupported {
            request_id,
            subtype: subtype.unwrap_or_else(|| "unknown".to_string()),
        },
    }
}

fn parse_control_response(value: Value) -> ControlFrame {
    let request_id = value
        .pointer("/response/request_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let envelope: ControlResponseEnvelope = match serde_json::from_value(value) {
        Ok(envelope) => envelope,
        Err(error) => {
            return ControlFrame::ParseError {
                request_id,
                message: format!("invalid control_response: {error}"),
            };
        }
    };
    match envelope.response {
        ControlResponse::Success {
            request_id,
            response: Some(response),
        } => ControlFrame::ServerResponse {
            request_id,
            result: Ok(response),
        },
        ControlResponse::Success {
            request_id,
            response: None,
        } => ControlFrame::ParseError {
            request_id: Some(request_id),
            message: "can_use_tool response is missing its decision payload".to_string(),
        },
        ControlResponse::Error { request_id, error } => ControlFrame::ServerResponse {
            request_id,
            result: Err(error),
        },
        _ => ControlFrame::ParseError {
            request_id,
            message: "unsupported control_response subtype".to_string(),
        },
    }
}

fn recover_request_id(value: &Value) -> Option<String> {
    value
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_line_is_ignored() {
        assert_eq!(parse_control_line("   "), ControlFrame::Ignore);
    }

    #[test]
    fn keep_alive_is_ignored() {
        assert_eq!(
            parse_control_line(r#"{"type":"keep_alive"}"#),
            ControlFrame::Ignore
        );
    }

    #[test]
    fn user_string_content_becomes_prompt() {
        let frame =
            parse_control_line(r#"{"type":"user","message":{"role":"user","content":"hello"}}"#);
        assert_eq!(frame, ControlFrame::UserPrompt("hello".to_string()));
    }

    #[test]
    fn user_block_array_content_is_concatenated() {
        let frame = parse_control_line(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}"#,
        );
        assert_eq!(frame, ControlFrame::UserPrompt("a\nb".to_string()));
    }

    #[test]
    fn user_without_text_is_a_parse_error() {
        let frame = parse_control_line(r#"{"type":"user","message":{"role":"user","content":[]}}"#);
        assert!(matches!(frame, ControlFrame::ParseError { .. }));
    }

    #[test]
    fn interrupt_request_is_parsed() {
        let frame = parse_control_line(
            r#"{"type":"control_request","request_id":"r1","request":{"subtype":"interrupt"}}"#,
        );
        assert_eq!(
            frame,
            ControlFrame::Interrupt {
                request_id: "r1".to_string()
            }
        );
    }

    #[test]
    fn set_permission_mode_request_validates_mode() {
        let frame = parse_control_line(
            r#"{"type":"control_request","request_id":"r2","request":{"subtype":"set_permission_mode","mode":"bypassPermissions"}}"#,
        );
        assert_eq!(
            frame,
            ControlFrame::SetPermissionMode {
                request_id: "r2".to_string(),
                mode: PermissionMode::BypassPermissions,
            }
        );
    }

    #[test]
    fn set_permission_mode_with_bad_mode_is_a_parse_error_with_request_id() {
        let frame = parse_control_line(
            r#"{"type":"control_request","request_id":"r3","request":{"subtype":"set_permission_mode","mode":"nonsense"}}"#,
        );
        match frame {
            ControlFrame::ParseError { request_id, .. } => {
                assert_eq!(request_id.as_deref(), Some("r3"));
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn unknown_subtype_is_unsupported_with_subtype_name() {
        let frame = parse_control_line(
            r#"{"type":"control_request","request_id":"r4","request":{"subtype":"frobnicate"}}"#,
        );
        assert_eq!(
            frame,
            ControlFrame::Unsupported {
                request_id: "r4".to_string(),
                subtype: "frobnicate".to_string(),
            }
        );
    }

    #[test]
    fn truncated_json_is_a_parse_error_not_a_panic() {
        let frame = parse_control_line(r#"{"type":"control_request","request_id":"#);
        assert!(matches!(
            frame,
            ControlFrame::ParseError {
                request_id: None,
                ..
            }
        ));
    }

    #[test]
    fn control_request_without_request_id_is_a_parse_error() {
        let frame =
            parse_control_line(r#"{"type":"control_request","request":{"subtype":"interrupt"}}"#);
        assert!(matches!(
            frame,
            ControlFrame::ParseError {
                request_id: None,
                ..
            }
        ));
    }

    #[test]
    fn get_session_state_parsed() {
        let frame = parse_control_line(
            r#"{"type":"control_request","request_id":"r5","request":{"subtype":"get_session_state"}}"#,
        );
        assert_eq!(
            frame,
            ControlFrame::GetSessionState {
                request_id: "r5".to_string()
            }
        );
    }

    #[test]
    fn get_context_usage_parsed() {
        let frame = parse_control_line(
            r#"{"type":"control_request","request_id":"r6","request":{"subtype":"get_context_usage"}}"#,
        );
        assert_eq!(
            frame,
            ControlFrame::GetContextUsage {
                request_id: "r6".to_string()
            }
        );
    }

    #[test]
    fn set_max_thinking_tokens_with_number() {
        let frame = parse_control_line(
            r#"{"type":"control_request","request_id":"r7","request":{"subtype":"set_max_thinking_tokens","max_thinking_tokens":4096}}"#,
        );
        assert_eq!(
            frame,
            ControlFrame::SetMaxThinkingTokens {
                request_id: "r7".to_string(),
                max_thinking_tokens: Some(4096),
            }
        );
    }

    #[test]
    fn set_max_thinking_tokens_with_null() {
        let frame = parse_control_line(
            r#"{"type":"control_request","request_id":"r8","request":{"subtype":"set_max_thinking_tokens","max_thinking_tokens":null}}"#,
        );
        assert_eq!(
            frame,
            ControlFrame::SetMaxThinkingTokens {
                request_id: "r8".to_string(),
                max_thinking_tokens: None,
            }
        );
    }

    #[test]
    fn set_max_thinking_tokens_with_string_is_parse_error() {
        let frame = parse_control_line(
            r#"{"type":"control_request","request_id":"r9","request":{"subtype":"set_max_thinking_tokens","max_thinking_tokens":"lots"}}"#,
        );
        match frame {
            ControlFrame::ParseError { request_id, .. } => {
                assert_eq!(request_id.as_deref(), Some("r9"));
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn sdk_control_extensions_are_classified_to_typed_frames() {
        let cases = [
            (
                r#"{"type":"control_request","request_id":"init","request":{"subtype":"initialize"}}"#,
                ControlFrame::Initialize {
                    request_id: "init".to_string(),
                },
            ),
            (
                r#"{"type":"control_request","request_id":"mcp","request":{"subtype":"mcp_status"}}"#,
                ControlFrame::McpStatus {
                    request_id: "mcp".to_string(),
                },
            ),
            (
                r#"{"type":"control_request","request_id":"model","request":{"subtype":"set_model","model":null}}"#,
                ControlFrame::SetModel {
                    request_id: "model".to_string(),
                    model: None,
                },
            ),
            (
                r#"{"type":"control_request","request_id":"seed","request":{"subtype":"seed_read_state","path":"src/lib.rs","mtime":42}}"#,
                ControlFrame::SeedReadState {
                    request_id: "seed".to_string(),
                    path: "src/lib.rs".to_string(),
                    mtime: 42,
                },
            ),
            (
                r#"{"type":"control_request","request_id":"rewind","request":{"subtype":"rewind_files","user_message_id":"msg-1","dry_run":false}}"#,
                ControlFrame::RewindFiles {
                    request_id: "rewind".to_string(),
                },
            ),
            (
                r#"{"type":"control_request","request_id":"cancel","request":{"subtype":"cancel_async_message","message_uuid":"task-1"}}"#,
                ControlFrame::CancelAsyncMessage {
                    request_id: "cancel".to_string(),
                    message_uuid: "task-1".to_string(),
                },
            ),
        ];
        for (line, expected) in cases {
            assert_eq!(parse_control_line(line), expected, "{line}");
        }
    }

    #[test]
    fn sdk_permission_control_response_preserves_exact_correlation_and_casing() {
        let frame = parse_control_line(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"permission-1","response":{"behavior":"allow","updatedInput":{"path":"src/lib.rs"},"toolUseID":"tool-1"}}}"#,
        );
        assert_eq!(
            frame,
            ControlFrame::ServerResponse {
                request_id: "permission-1".to_string(),
                result: Ok(serde_json::json!({
                    "behavior": "allow",
                    "updatedInput": {"path": "src/lib.rs"},
                    "toolUseID": "tool-1"
                })),
            }
        );
    }

    #[test]
    fn missing_type_is_a_parse_error() {
        let frame = parse_control_line(r#"{"request_id":"r5"}"#);
        match frame {
            ControlFrame::ParseError { request_id, .. } => {
                assert_eq!(request_id.as_deref(), Some("r5"));
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
    }
}
