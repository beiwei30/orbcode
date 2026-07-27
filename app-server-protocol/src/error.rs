use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Structured error returned inside [`ResponseResult::Error`](crate::ResponseResult).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProtocolError {}

/// Error codes for the app-server protocol.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    InternalError,
    SessionNotFound,
    ActiveTurn,
    NoActiveTurn,
    PermissionDenied,
    ProviderFailed,
    ConfigError,
    ToolError,
    McpError,
    Overloaded,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_code_roundtrip_all_variants() {
        let codes = [
            ErrorCode::ParseError,
            ErrorCode::InvalidRequest,
            ErrorCode::MethodNotFound,
            ErrorCode::InvalidParams,
            ErrorCode::InternalError,
            ErrorCode::SessionNotFound,
            ErrorCode::ActiveTurn,
            ErrorCode::NoActiveTurn,
            ErrorCode::PermissionDenied,
            ErrorCode::ProviderFailed,
            ErrorCode::ConfigError,
            ErrorCode::ToolError,
            ErrorCode::McpError,
            ErrorCode::Overloaded,
        ];
        for code in &codes {
            let json = serde_json::to_string(code).unwrap();
            let back: ErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(*code, back);
        }
    }

    #[test]
    fn error_code_snake_case_serialization() {
        assert_eq!(
            serde_json::to_value(ErrorCode::ParseError).unwrap(),
            json!("parse_error")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::InvalidRequest).unwrap(),
            json!("invalid_request")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::MethodNotFound).unwrap(),
            json!("method_not_found")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::InvalidParams).unwrap(),
            json!("invalid_params")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::InternalError).unwrap(),
            json!("internal_error")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::SessionNotFound).unwrap(),
            json!("session_not_found")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::ActiveTurn).unwrap(),
            json!("active_turn")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::NoActiveTurn).unwrap(),
            json!("no_active_turn")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::PermissionDenied).unwrap(),
            json!("permission_denied")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::ProviderFailed).unwrap(),
            json!("provider_failed")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::ConfigError).unwrap(),
            json!("config_error")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::ToolError).unwrap(),
            json!("tool_error")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::McpError).unwrap(),
            json!("mcp_error")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::Overloaded).unwrap(),
            json!("overloaded")
        );
    }

    #[test]
    fn protocol_error_roundtrip_with_data() {
        let err = ProtocolError {
            code: ErrorCode::InvalidParams,
            message: "missing field 'prompt'".into(),
            data: Some(json!({"field": "prompt"})),
        };
        let json = serde_json::to_string(&err).unwrap();
        let back: ProtocolError = serde_json::from_str(&json).unwrap();
        assert_eq!(err, back);
    }

    #[test]
    fn protocol_error_roundtrip_without_data() {
        let err = ProtocolError {
            code: ErrorCode::InternalError,
            message: "unexpected".into(),
            data: None,
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(!json.contains("\"data\""));
        let back: ProtocolError = serde_json::from_str(&json).unwrap();
        assert_eq!(err, back);
    }

    #[test]
    fn protocol_error_display() {
        let err = ProtocolError {
            code: ErrorCode::SessionNotFound,
            message: "no such session".into(),
            data: None,
        };
        let display = format!("{err}");
        assert!(display.contains("SessionNotFound"));
        assert!(display.contains("no such session"));
    }

    #[test]
    fn error_code_unknown_variant_fails_deserialization() {
        let result = serde_json::from_value::<ErrorCode>(json!("rate_limited"));
        assert!(result.is_err(), "unknown ErrorCode variant should fail");

        let result = serde_json::from_value::<ErrorCode>(json!("unknown_code"));
        assert!(result.is_err(), "unknown ErrorCode variant should fail");
    }
}
