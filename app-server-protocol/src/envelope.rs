use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ProtocolError;

/// Unique identifier for a request/response pair.
pub type RequestId = String;

// ---------------------------------------------------------------------------
// Client -> Server
// ---------------------------------------------------------------------------

/// Messages sent from a client to the server.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClientMessage {
    /// A new request initiated by the client.
    Request(ClientRequestEnvelope),
    /// A response to a server-initiated request.
    Response(ServerRequestResponse),
}

/// A request envelope sent by the client.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ClientRequestEnvelope {
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

// ---------------------------------------------------------------------------
// Server -> Client
// ---------------------------------------------------------------------------

/// Messages sent from the server to a client.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ServerMessage {
    /// A response to a client-initiated request.
    Response(ServerResponseEnvelope),
    /// An unsolicited notification (e.g. stream events).
    Notification(ServerNotificationEnvelope),
    /// A server-initiated request (e.g. permission prompt).
    Request(ServerRequestEnvelope),
}

/// Response envelope returned by the server for a client request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ServerResponseEnvelope {
    pub id: RequestId,
    pub result: ResponseResult,
}

/// Outcome of a request -- either success or an error.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ResponseResult {
    Success {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error(ProtocolError),
}

/// Unsolicited server notification (stream events, progress, etc.).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ServerNotificationEnvelope {
    pub method: String,
    pub params: Value,
}

/// A request initiated by the server that expects a client response.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ServerRequestEnvelope {
    pub id: RequestId,
    pub method: String,
    pub params: Value,
}

/// Client's response to a server-initiated request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ServerRequestResponse {
    pub id: RequestId,
    pub result: ResponseResult,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // ClientMessage
    // -----------------------------------------------------------------------

    #[test]
    fn client_message_request_roundtrip() {
        let msg = ClientMessage::Request(ClientRequestEnvelope {
            id: "req-1".into(),
            method: "turn/submit".into(),
            params: Some(json!({"prompt": "hello"})),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let back: ClientMessage = serde_json::from_str(&json).unwrap();
        match back {
            ClientMessage::Request(env) => {
                assert_eq!(env.id, "req-1");
                assert_eq!(env.method, "turn/submit");
                assert_eq!(env.params, Some(json!({"prompt": "hello"})));
            }
            _ => panic!("expected Request variant"),
        }
    }

    #[test]
    fn client_message_response_roundtrip() {
        let msg = ClientMessage::Response(ServerRequestResponse {
            id: "srv-1".into(),
            result: ResponseResult::Success {
                data: Some(json!(true)),
            },
        });
        let json = serde_json::to_string(&msg).unwrap();
        let back: ClientMessage = serde_json::from_str(&json).unwrap();
        match back {
            ClientMessage::Response(resp) => {
                assert_eq!(resp.id, "srv-1");
            }
            _ => panic!("expected Response variant"),
        }
    }

    #[test]
    fn client_request_envelope_params_absent() {
        let env = ClientRequestEnvelope {
            id: "r1".into(),
            method: "session/list".into(),
            params: None,
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(!json.contains("params"));
        let back: ClientRequestEnvelope = serde_json::from_str(&json).unwrap();
        assert!(back.params.is_none());
    }

    #[test]
    fn client_request_envelope_params_present() {
        let env = ClientRequestEnvelope {
            id: "r2".into(),
            method: "turn/submit".into(),
            params: Some(json!({"text": "hi"})),
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains("params"));
        let back: ClientRequestEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.params, Some(json!({"text": "hi"})));
    }

    // -----------------------------------------------------------------------
    // ServerMessage
    // -----------------------------------------------------------------------

    #[test]
    fn server_message_response_roundtrip() {
        let msg = ServerMessage::Response(ServerResponseEnvelope {
            id: "req-1".into(),
            result: ResponseResult::Success { data: None },
        });
        let json = serde_json::to_string(&msg).unwrap();
        let back: ServerMessage = serde_json::from_str(&json).unwrap();
        match back {
            ServerMessage::Response(env) => {
                assert_eq!(env.id, "req-1");
                match env.result {
                    ResponseResult::Success { data } => assert!(data.is_none()),
                    _ => panic!("expected Success"),
                }
            }
            _ => panic!("expected Response variant"),
        }
    }

    #[test]
    fn server_message_notification_roundtrip() {
        let msg = ServerMessage::Notification(ServerNotificationEnvelope {
            method: "stream/event".into(),
            params: json!({"event": "session_started"}),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let back: ServerMessage = serde_json::from_str(&json).unwrap();
        match back {
            ServerMessage::Notification(env) => {
                assert_eq!(env.method, "stream/event");
            }
            _ => panic!("expected Notification variant"),
        }
    }

    #[test]
    fn server_message_request_roundtrip() {
        let msg = ServerMessage::Request(ServerRequestEnvelope {
            id: "srv-req-1".into(),
            method: "permission/request".into(),
            params: json!({"tool_name": "bash"}),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let back: ServerMessage = serde_json::from_str(&json).unwrap();
        match back {
            ServerMessage::Request(env) => {
                assert_eq!(env.id, "srv-req-1");
                assert_eq!(env.method, "permission/request");
            }
            _ => panic!("expected Request variant"),
        }
    }

    // -----------------------------------------------------------------------
    // ResponseResult
    // -----------------------------------------------------------------------

    #[test]
    fn response_result_success_with_data() {
        let r = ResponseResult::Success {
            data: Some(json!({"sessions": []})),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ResponseResult = serde_json::from_str(&json).unwrap();
        match back {
            ResponseResult::Success { data } => {
                assert_eq!(data, Some(json!({"sessions": []})));
            }
            _ => panic!("expected Success"),
        }
    }

    #[test]
    fn response_result_success_without_data() {
        let r = ResponseResult::Success { data: None };
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("\"data\""));
        let back: ResponseResult = serde_json::from_str(&json).unwrap();
        match back {
            ResponseResult::Success { data } => assert!(data.is_none()),
            _ => panic!("expected Success"),
        }
    }

    #[test]
    fn response_result_error_roundtrip() {
        use crate::error::ErrorCode;
        let r = ResponseResult::Error(crate::error::ProtocolError {
            code: ErrorCode::InternalError,
            message: "boom".into(),
            data: None,
        });
        let json = serde_json::to_string(&r).unwrap();
        let back: ResponseResult = serde_json::from_str(&json).unwrap();
        match back {
            ResponseResult::Error(e) => {
                assert_eq!(e.code, ErrorCode::InternalError);
                assert_eq!(e.message, "boom");
            }
            _ => panic!("expected Error"),
        }
    }

    // -----------------------------------------------------------------------
    // ServerRequestResponse
    // -----------------------------------------------------------------------

    #[test]
    fn server_request_response_roundtrip() {
        let r = ServerRequestResponse {
            id: "srv-1".into(),
            result: ResponseResult::Success {
                data: Some(json!("ok")),
            },
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ServerRequestResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "srv-1");
    }

    // -----------------------------------------------------------------------
    // Unknown type tags
    // -----------------------------------------------------------------------

    #[test]
    fn client_message_unknown_type_tag_fails() {
        let raw = json!({"type": "unknown_type", "id": "1", "method": "foo"});
        let result = serde_json::from_value::<ClientMessage>(raw);
        assert!(
            result.is_err(),
            "unknown ClientMessage type tag should fail"
        );
    }

    #[test]
    fn server_message_unknown_type_tag_fails() {
        let raw = json!({"type": "unknown_type", "id": "1"});
        let result = serde_json::from_value::<ServerMessage>(raw);
        assert!(
            result.is_err(),
            "unknown ServerMessage type tag should fail"
        );
    }

    #[test]
    fn response_result_unknown_status_fails() {
        let raw = json!({"status": "pending"});
        let result = serde_json::from_value::<ResponseResult>(raw);
        assert!(
            result.is_err(),
            "unknown ResponseResult status tag should fail"
        );
    }

    // -----------------------------------------------------------------------
    // Missing required fields
    // -----------------------------------------------------------------------

    #[test]
    fn client_request_envelope_missing_id_fails() {
        let raw = json!({"method": "session/list"});
        let result = serde_json::from_value::<ClientRequestEnvelope>(raw);
        assert!(result.is_err(), "missing id should fail");
    }

    #[test]
    fn client_request_envelope_missing_method_fails() {
        let raw = json!({"id": "req-1"});
        let result = serde_json::from_value::<ClientRequestEnvelope>(raw);
        assert!(result.is_err(), "missing method should fail");
    }

    #[test]
    fn server_response_envelope_missing_id_fails() {
        let raw = json!({"result": {"status": "success"}});
        let result = serde_json::from_value::<ServerResponseEnvelope>(raw);
        assert!(result.is_err(), "missing id should fail");
    }

    #[test]
    fn server_response_envelope_missing_result_fails() {
        let raw = json!({"id": "req-1"});
        let result = serde_json::from_value::<ServerResponseEnvelope>(raw);
        assert!(result.is_err(), "missing result should fail");
    }

    #[test]
    fn server_request_envelope_missing_id_fails() {
        let raw = json!({"method": "permission/request", "params": {}});
        let result = serde_json::from_value::<ServerRequestEnvelope>(raw);
        assert!(result.is_err(), "missing id should fail");
    }

    #[test]
    fn server_request_envelope_missing_method_fails() {
        let raw = json!({"id": "srv-1", "params": {}});
        let result = serde_json::from_value::<ServerRequestEnvelope>(raw);
        assert!(result.is_err(), "missing method should fail");
    }
}
