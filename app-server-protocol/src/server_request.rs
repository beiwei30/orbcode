use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Permission decision (serializable wire type)
// ---------------------------------------------------------------------------

/// Client's decision in response to a [`PermissionRequest`].
///
/// This is a wire-serializable enum mirroring `core`'s `PermissionDecision`,
/// kept here so the protocol crate stays free of core dependencies.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "decision", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PermissionDecisionWire {
    Approve,
    Deny,
    ApproveAlways {
        #[serde(default)]
        rules: Vec<String>,
    },
}

/// Parameters for the [`method::PERMISSION_RESPOND`](crate::method::PERMISSION_RESPOND)
/// method sent by the client in response to a server-initiated permission
/// request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PermissionResponseParams {
    pub request_id: String,
    pub decision: PermissionDecisionWire,
}

// ---------------------------------------------------------------------------
// MCP trust decision (serializable wire type)
// ---------------------------------------------------------------------------

/// Client's decision in response to a [`McpTrustApprovalRequest`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpTrustDecisionWire {
    Trust,
    Deny,
}

/// Parameters for the [`method::SERVER_REQUEST_MCP_TRUST`](crate::method::SERVER_REQUEST_MCP_TRUST)
/// response sent by the client.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpTrustResponseParams {
    pub request_id: String,
    pub decision: McpTrustDecisionWire,
}

// ---------------------------------------------------------------------------
// AskUserQuestion (serializable wire types)
// ---------------------------------------------------------------------------

/// Server-initiated request asking the connected client to prompt the user
/// with a question and relay the answer back.
///
/// Sent via [`method::SERVER_REQUEST_ASK_USER`](crate::method::SERVER_REQUEST_ASK_USER).
///
/// The full tool-level integration is wired: the `AskUserQuestion` tool
/// pauses execution via `ToolContext::ask_user_tx`, the event pump in
/// `MessageProcessor::pump_events` sends this as a server-request, and the
/// client's response is routed back to the tool via
/// `AppServer::resolve_ask_user_question`. Cancellation (disconnect,
/// timeout, turn cancel) resolves with `None`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AskUserQuestionRequest {
    /// Session that owns the active turn for this question. Older clients may
    /// omit this field; servers should then fall back to their legacy routing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    /// Unique identifier for this question, used to correlate the response.
    pub request_id: String,
    /// The question text to present to the user.
    pub question: String,
    /// Optional list of choices. When provided, the client should present
    /// these as selectable options rather than a free-text input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// Client's response to an [`AskUserQuestionRequest`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AskUserQuestionResponse {
    /// The `request_id` from the original [`AskUserQuestionRequest`].
    pub request_id: String,
    /// The user's answer. `None` if the user dismissed or cancelled the
    /// prompt without providing an answer.
    pub answer: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // PermissionDecisionWire
    // -----------------------------------------------------------------------

    #[test]
    fn permission_decision_approve_roundtrip() {
        let d = PermissionDecisionWire::Approve;
        let json = serde_json::to_string(&d).unwrap();
        let back: PermissionDecisionWire = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn permission_decision_deny_roundtrip() {
        let d = PermissionDecisionWire::Deny;
        let json = serde_json::to_string(&d).unwrap();
        let back: PermissionDecisionWire = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn permission_decision_approve_always_roundtrip() {
        let d = PermissionDecisionWire::ApproveAlways {
            rules: vec!["Bash(npm test)".into(), "Read(*)".into()],
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: PermissionDecisionWire = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn permission_decision_approve_always_empty_rules() {
        let d = PermissionDecisionWire::ApproveAlways { rules: vec![] };
        let json = serde_json::to_string(&d).unwrap();
        let back: PermissionDecisionWire = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn permission_decision_approve_always_default_rules() {
        // When "rules" is absent in JSON, it should default to empty vec.
        let value = json!({"decision": "approve_always"});
        let d: PermissionDecisionWire = serde_json::from_value(value).unwrap();
        assert_eq!(d, PermissionDecisionWire::ApproveAlways { rules: vec![] });
    }

    #[test]
    fn permission_decision_tagged_serialization() {
        let v = serde_json::to_value(PermissionDecisionWire::Approve).unwrap();
        assert_eq!(v["decision"], json!("approve"));

        let v = serde_json::to_value(PermissionDecisionWire::Deny).unwrap();
        assert_eq!(v["decision"], json!("deny"));

        let v = serde_json::to_value(PermissionDecisionWire::ApproveAlways {
            rules: vec!["r1".into()],
        })
        .unwrap();
        assert_eq!(v["decision"], json!("approve_always"));
        assert_eq!(v["rules"], json!(["r1"]));
    }

    // -----------------------------------------------------------------------
    // PermissionResponseParams
    // -----------------------------------------------------------------------

    #[test]
    fn permission_response_params_roundtrip() {
        let p = PermissionResponseParams {
            request_id: "perm-123".into(),
            decision: PermissionDecisionWire::Approve,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: PermissionResponseParams = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    // -----------------------------------------------------------------------
    // McpTrustDecisionWire
    // -----------------------------------------------------------------------

    #[test]
    fn mcp_trust_decision_trust_roundtrip() {
        let d = McpTrustDecisionWire::Trust;
        let json = serde_json::to_string(&d).unwrap();
        let back: McpTrustDecisionWire = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn mcp_trust_decision_deny_roundtrip() {
        let d = McpTrustDecisionWire::Deny;
        let json = serde_json::to_string(&d).unwrap();
        let back: McpTrustDecisionWire = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn mcp_trust_decision_snake_case_serialization() {
        assert_eq!(
            serde_json::to_value(McpTrustDecisionWire::Trust).unwrap(),
            json!("trust")
        );
        assert_eq!(
            serde_json::to_value(McpTrustDecisionWire::Deny).unwrap(),
            json!("deny")
        );
    }

    // -----------------------------------------------------------------------
    // McpTrustResponseParams
    // -----------------------------------------------------------------------

    #[test]
    fn mcp_trust_response_params_roundtrip() {
        let p = McpTrustResponseParams {
            request_id: "mcp-trust-456".into(),
            decision: McpTrustDecisionWire::Trust,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: McpTrustResponseParams = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn mcp_trust_response_params_deny_roundtrip() {
        let p = McpTrustResponseParams {
            request_id: "mcp-trust-789".into(),
            decision: McpTrustDecisionWire::Deny,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: McpTrustResponseParams = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    // -----------------------------------------------------------------------
    // AskUserQuestionRequest
    // -----------------------------------------------------------------------

    #[test]
    fn ask_user_question_request_roundtrip() {
        let req = AskUserQuestionRequest {
            session_id: "session-ask".into(),
            request_id: "ask-1".into(),
            question: "What is the target branch?".into(),
            options: vec!["main".into(), "develop".into()],
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: AskUserQuestionRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn ask_user_question_request_no_options_roundtrip() {
        let req = AskUserQuestionRequest {
            session_id: "session-ask".into(),
            request_id: "ask-2".into(),
            question: "Enter your name".into(),
            options: vec![],
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: AskUserQuestionRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn ask_user_question_request_options_skipped_when_empty() {
        let req = AskUserQuestionRequest {
            session_id: "session-ask".into(),
            request_id: "ask-3".into(),
            question: "Confirm?".into(),
            options: vec![],
        };
        let value = serde_json::to_value(&req).unwrap();
        assert!(
            value.get("options").is_none(),
            "empty options should be skipped in serialization"
        );
    }

    #[test]
    fn ask_user_question_request_default_options() {
        // When "options" is absent in JSON, it should default to empty vec.
        let value = json!({
            "request_id": "ask-4",
            "question": "Which env?"
        });
        let req: AskUserQuestionRequest = serde_json::from_value(value).unwrap();
        assert_eq!(req.session_id, "");
        assert_eq!(req.options, Vec::<String>::new());
    }

    // -----------------------------------------------------------------------
    // AskUserQuestionResponse
    // -----------------------------------------------------------------------

    #[test]
    fn ask_user_question_response_with_answer_roundtrip() {
        let resp = AskUserQuestionResponse {
            request_id: "ask-1".into(),
            answer: Some("main".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: AskUserQuestionResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, back);
    }

    #[test]
    fn ask_user_question_response_cancelled_roundtrip() {
        let resp = AskUserQuestionResponse {
            request_id: "ask-1".into(),
            answer: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: AskUserQuestionResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, back);
    }

    #[test]
    fn ask_user_question_response_null_answer_roundtrip() {
        let value = json!({
            "request_id": "ask-5",
            "answer": null,
        });
        let resp: AskUserQuestionResponse = serde_json::from_value(value).unwrap();
        assert_eq!(resp.answer, None);
    }
}
