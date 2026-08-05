use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use orbcode_protocol::{
    AskUserAnswerValue, AskUserCancellationReason, AskUserOption, AskUserQuestionSpec,
    AskUserResponseOutcome, AskUserValidationError, validate_ask_user_outcome,
    validate_ask_user_questions,
};

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
/// with one or more questions and relay a typed outcome back.
///
/// Sent via [`method::SERVER_REQUEST_ASK_USER`](crate::method::SERVER_REQUEST_ASK_USER).
///
/// `question` and `options` are retained for protocol-1.0 clients. Canonical
/// clients should use `questions`; servers normalize either representation at
/// the boundary and validate the result before registering pending state.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AskUserQuestionRequest {
    /// Session that owns the active turn for this question. Older clients may
    /// omit this field; servers should then fall back to their legacy routing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    /// Turn that owns this interaction, when the transport has a stable id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// Provider tool-use id that caused the interaction.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_use_id: String,
    /// Unique identifier used to correlate the response.
    pub request_id: String,
    /// Optional absolute RFC3339 deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    /// Validation error from a prior response attempt. When present, the same
    /// server-request id remains pending and the client may retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_error: Option<AskUserValidationError>,
    /// Canonical list of one to four questions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<AskUserQuestionSpec>,
    /// Legacy protocol-1.0 question text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub question: String,
    /// Legacy protocol-1.0 option labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

impl AskUserQuestionRequest {
    /// Return the canonical question list, normalizing a legacy one-question
    /// payload when necessary.
    pub fn canonical_questions(&self) -> Result<Vec<AskUserQuestionSpec>, AskUserValidationError> {
        let questions = if self.questions.is_empty() {
            vec![legacy_question_spec(&self.question, &self.options)]
        } else {
            self.questions.clone()
        };
        validate_ask_user_questions(&questions)?;
        Ok(questions)
    }
}

/// Client's response to an [`AskUserQuestionRequest`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AskUserQuestionResponse {
    /// The `request_id` from the original [`AskUserQuestionRequest`].
    pub request_id: String,
    /// Canonical typed response. New clients should always populate this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AskUserResponseOutcome>,
    /// Legacy protocol-1.0 answer. A missing or null value normalizes to a
    /// client-closed cancellation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

impl AskUserQuestionResponse {
    /// Normalize a canonical or legacy response and validate it against the
    /// original request without consuming pending state.
    pub fn canonical_outcome(
        &self,
        request: &AskUserQuestionRequest,
    ) -> Result<AskUserResponseOutcome, AskUserValidationError> {
        let questions = request.canonical_questions()?;
        let outcome = match &self.outcome {
            Some(outcome) => outcome.clone(),
            None => legacy_response_outcome(&questions, self.answer.as_deref()),
        };
        validate_ask_user_outcome(&questions, &outcome)?;
        Ok(outcome)
    }
}

fn legacy_question_spec(question: &str, options: &[String]) -> AskUserQuestionSpec {
    AskUserQuestionSpec {
        id: "question-1".to_string(),
        question: question.to_string(),
        header: "Question".to_string(),
        multi_select: false,
        options: options
            .iter()
            .enumerate()
            .map(|(index, label)| AskUserOption {
                id: format!("option-{}", index + 1),
                label: label.clone(),
                description: String::new(),
                preview: None,
            })
            .collect(),
        allow_free_text: true,
        allow_annotation: false,
    }
}

fn legacy_response_outcome(
    questions: &[AskUserQuestionSpec],
    answer: Option<&str>,
) -> AskUserResponseOutcome {
    let Some(answer) = answer else {
        return AskUserResponseOutcome::Cancelled {
            reason: AskUserCancellationReason::ClientClosed,
        };
    };
    let question = &questions[0];
    let value = question
        .options
        .iter()
        .find(|option| option.id == answer || option.label == answer)
        .map_or_else(
            || AskUserAnswerValue::Text {
                text: answer.to_string(),
            },
            |option| AskUserAnswerValue::Selected {
                option_id: option.id.clone(),
            },
        );
    AskUserResponseOutcome::Answered {
        answers: std::collections::BTreeMap::from([(question.id.clone(), value)]),
        annotations: std::collections::BTreeMap::new(),
    }
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
            turn_id: None,
            tool_use_id: String::new(),
            request_id: "ask-1".into(),
            deadline: None,
            validation_error: None,
            questions: Vec::new(),
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
            turn_id: None,
            tool_use_id: String::new(),
            request_id: "ask-2".into(),
            deadline: None,
            validation_error: None,
            questions: Vec::new(),
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
            turn_id: None,
            tool_use_id: String::new(),
            request_id: "ask-3".into(),
            deadline: None,
            validation_error: None,
            questions: Vec::new(),
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
            outcome: None,
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
            outcome: None,
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

    #[test]
    fn canonical_request_and_answer_roundtrip_and_validate() {
        let question = AskUserQuestionSpec {
            id: "database".into(),
            question: "Which database?".into(),
            header: "Database".into(),
            multi_select: false,
            options: vec![AskUserOption {
                id: "postgres".into(),
                label: "PostgreSQL".into(),
                description: "Relational".into(),
                preview: Some("CREATE TABLE users (...)".into()),
            }],
            allow_free_text: true,
            allow_annotation: true,
        };
        let request = AskUserQuestionRequest {
            session_id: "session-ask".into(),
            turn_id: Some("turn-1".into()),
            tool_use_id: "tool-1".into(),
            request_id: "ask-canonical".into(),
            deadline: Some("2026-08-05T12:00:00Z".into()),
            validation_error: None,
            questions: vec![question],
            question: String::new(),
            options: Vec::new(),
        };
        let response = AskUserQuestionResponse {
            request_id: request.request_id.clone(),
            outcome: Some(AskUserResponseOutcome::Answered {
                answers: std::collections::BTreeMap::from([(
                    "database".into(),
                    AskUserAnswerValue::Selected {
                        option_id: "postgres".into(),
                    },
                )]),
                annotations: std::collections::BTreeMap::from([(
                    "database".into(),
                    "Use the current stable release".into(),
                )]),
            }),
            answer: None,
        };
        let wire = serde_json::to_value(&response).unwrap();
        let decoded: AskUserQuestionResponse = serde_json::from_value(wire).unwrap();
        assert_eq!(
            decoded.canonical_outcome(&request).unwrap(),
            response.outcome.unwrap()
        );
    }

    #[test]
    fn legacy_request_and_answer_normalize_to_option_id() {
        let request: AskUserQuestionRequest = serde_json::from_value(json!({
            "request_id": "ask-legacy",
            "question": "Which database?",
            "options": ["PostgreSQL", "SQLite"]
        }))
        .unwrap();
        let response: AskUserQuestionResponse = serde_json::from_value(json!({
            "request_id": "ask-legacy",
            "answer": "SQLite"
        }))
        .unwrap();
        assert_eq!(
            response.canonical_outcome(&request).unwrap(),
            AskUserResponseOutcome::Answered {
                answers: std::collections::BTreeMap::from([(
                    "question-1".into(),
                    AskUserAnswerValue::Selected {
                        option_id: "option-2".into()
                    }
                )]),
                annotations: std::collections::BTreeMap::new(),
            }
        );
    }
}
