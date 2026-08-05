use orbcode_protocol::{
    AskUserAnswerValue, AskUserCancellationReason, AskUserOption, AskUserQuestionSpec,
    AskUserResponseOutcome, validate_ask_user_questions,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    ToolContext, ToolError, ToolOutcome, ToolRegistry,
    payload::{field_or_raw, parse_payload},
    types::AskUserRequest,
};

impl ToolRegistry {
    pub(crate) async fn ask_user_question(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let payload = parse_payload(input)?;
        let questions = parse_questions(&payload, input)?;
        validate_ask_user_questions(&questions)
            .map_err(|error| ToolError::InvalidInput(error.to_string()))?;

        let ask_tx = context.ask_user_tx.as_ref().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "AskUserQuestion is not available in this context".to_string(),
            )
        })?;

        let request_id = Uuid::new_v4().to_string();
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        let req = AskUserRequest {
            request_id: request_id.clone(),
            questions: questions.clone(),
            response_tx,
        };

        ask_tx
            .send(req)
            .map_err(|_| ToolError::ExecutionFailed("ask-user channel closed".to_string()))?;

        let outcome = tokio::select! {
            result = response_rx => result.unwrap_or(AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::DeliveryFailed,
            }),
            _ = context.cancellation.cancelled() => AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::Interrupt,
            },
        };

        if let AskUserResponseOutcome::Cancelled { reason } = &outcome {
            let metadata = serde_json::json!({
                "request_id": request_id,
                "ask_user_outcome": "cancelled",
                "reason": reason,
            });
            return Err(if matches!(reason, AskUserCancellationReason::Interrupt) {
                ToolError::InterruptedWithMetadata { metadata }
            } else {
                ToolError::CancelledWithMetadata { metadata }
            });
        }

        let output = provider_visible_summary(&questions, &outcome);

        Ok(ToolOutcome {
            name: "ask-user-question".to_string(),
            summary: format!("Asked user {} question(s)", questions.len()),
            output,
            metadata: Some(serde_json::json!({
                "request_id": request_id,
                "ask_user_outcome": outcome,
            })),
            changed_paths: Vec::new(),
        })
    }
}

#[derive(Deserialize)]
struct CanonicalInput {
    questions: Vec<AskUserQuestionSpec>,
}

fn parse_questions(payload: &Value, raw: &str) -> Result<Vec<AskUserQuestionSpec>, ToolError> {
    if payload.get("questions").is_some() {
        return serde_json::from_value::<CanonicalInput>(payload.clone())
            .map(|input| input.questions)
            .map_err(|error| ToolError::InvalidInput(error.to_string()));
    }

    let question = field_or_raw(payload, "question", raw)?;
    let options = match payload.get("options") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| ToolError::InvalidInput("legacy `options` must be an array".into()))?
            .iter()
            .enumerate()
            .map(|(index, item)| {
                item.as_str()
                    .map(|label| AskUserOption {
                        id: format!("option-{}", index + 1),
                        label: label.to_string(),
                        description: String::new(),
                        preview: None,
                    })
                    .ok_or_else(|| {
                        ToolError::InvalidInput("legacy `options` must contain only strings".into())
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
    };

    let allow_free_text = options.is_empty();
    Ok(vec![AskUserQuestionSpec {
        id: "question-1".into(),
        question,
        header: "Question".into(),
        multi_select: false,
        options,
        allow_free_text,
        allow_annotation: false,
    }])
}

fn provider_visible_summary(
    questions: &[AskUserQuestionSpec],
    outcome: &AskUserResponseOutcome,
) -> String {
    match outcome {
        AskUserResponseOutcome::Answered {
            answers,
            annotations,
        } => {
            let mut lines = vec!["User has answered your questions:".to_string()];
            for question in questions {
                let rendered = answers
                    .get(&question.id)
                    .map(|answer| render_answer(question, answer))
                    .unwrap_or_else(|| "<missing>".to_string());
                lines.push(format!("- \"{}\" = \"{rendered}\"", question.question));
                if let Some(annotation) = annotations.get(&question.id) {
                    lines.push(format!("  Annotation: {annotation}"));
                }
            }
            lines.push("You can now continue with the user's answers in mind.".to_string());
            lines.join("\n")
        }
        AskUserResponseOutcome::Rejected => {
            "The user rejected these questions. Continue without assuming answers, or adapt your approach."
                .to_string()
        }
        AskUserResponseOutcome::Clarify => {
            "The user asked for clarification. Explain or reformulate the questions before continuing."
                .to_string()
        }
        AskUserResponseOutcome::FinishPlanInterview => {
            "The user finished the plan interview and indicated that enough information has been provided."
                .to_string()
        }
        AskUserResponseOutcome::Cancelled { reason } => {
            format!("The question interaction was cancelled ({reason:?}).")
        }
    }
}

fn render_answer(question: &AskUserQuestionSpec, answer: &AskUserAnswerValue) -> String {
    match answer {
        AskUserAnswerValue::Text { text } => text.clone(),
        AskUserAnswerValue::Selected { option_id } => question
            .options
            .iter()
            .find(|option| option.id == *option_id)
            .map(|option| option.label.clone())
            .unwrap_or_else(|| option_id.clone()),
        AskUserAnswerValue::SelectedMany { option_ids } => option_ids
            .iter()
            .map(|option_id| {
                question
                    .options
                    .iter()
                    .find(|option| option.id == *option_id)
                    .map(|option| option.label.clone())
                    .unwrap_or_else(|| option_id.clone())
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn ask_user_canonical_input_parses_multiple_questions() {
        let input = serde_json::json!({
            "questions": [
                {
                    "id": "database",
                    "question": "Which database?",
                    "header": "Database",
                    "options": [{"id": "pg", "label": "PostgreSQL", "description": "SQL"}]
                },
                {
                    "id": "features",
                    "question": "Which features?",
                    "header": "Features",
                    "multi_select": true,
                    "options": [{"id": "search", "label": "Search", "description": "Full text"}]
                }
            ]
        });
        assert_eq!(
            parse_questions(&input, &input.to_string()).unwrap().len(),
            2
        );
    }

    #[test]
    fn ask_user_summary_preserves_question_order_and_selected_labels() {
        let questions = parse_questions(
            &serde_json::json!({
                "question": "Which database?",
                "options": ["PostgreSQL", "SQLite"]
            }),
            "",
        )
        .unwrap();
        let outcome = AskUserResponseOutcome::Answered {
            answers: BTreeMap::from([(
                "question-1".into(),
                AskUserAnswerValue::Selected {
                    option_id: "option-2".into(),
                },
            )]),
            annotations: BTreeMap::new(),
        };
        assert!(provider_visible_summary(&questions, &outcome).contains("SQLite"));
    }

    #[test]
    fn ask_user_summary_preserves_other_multi_select_and_annotations() {
        let questions = vec![
            AskUserQuestionSpec {
                id: "name".into(),
                question: "Project name?".into(),
                header: "Name".into(),
                multi_select: false,
                options: Vec::new(),
                allow_free_text: true,
                allow_annotation: true,
            },
            AskUserQuestionSpec {
                id: "features".into(),
                question: "Features?".into(),
                header: "Features".into(),
                multi_select: true,
                options: vec![
                    AskUserOption {
                        id: "search".into(),
                        label: "Search".into(),
                        description: String::new(),
                        preview: None,
                    },
                    AskUserOption {
                        id: "audit".into(),
                        label: "Audit log".into(),
                        description: String::new(),
                        preview: None,
                    },
                ],
                allow_free_text: false,
                allow_annotation: false,
            },
        ];
        let outcome = AskUserResponseOutcome::Answered {
            answers: BTreeMap::from([
                (
                    "name".into(),
                    AskUserAnswerValue::Text {
                        text: "Northstar".into(),
                    },
                ),
                (
                    "features".into(),
                    AskUserAnswerValue::SelectedMany {
                        option_ids: vec!["audit".into(), "search".into()],
                    },
                ),
            ]),
            annotations: BTreeMap::from([("name".into(), "Internal codename".into())]),
        };
        let summary = provider_visible_summary(&questions, &outcome);
        assert!(summary.find("Project name?").unwrap() < summary.find("Features?").unwrap());
        assert!(summary.contains("Northstar"));
        assert!(summary.contains("Audit log, Search"));
        assert!(summary.contains("Annotation: Internal codename"));
    }

    #[test]
    fn ask_user_special_outcomes_are_explicit_non_placeholder_results() {
        let question = parse_questions(
            &serde_json::json!({"question": "Proceed?", "options": ["yes"]}),
            "",
        )
        .unwrap();
        assert!(
            provider_visible_summary(&question, &AskUserResponseOutcome::Rejected)
                .contains("rejected")
        );
        assert!(
            provider_visible_summary(&question, &AskUserResponseOutcome::Clarify)
                .contains("clarification")
        );
        assert!(
            provider_visible_summary(&question, &AskUserResponseOutcome::FinishPlanInterview)
                .contains("finished the plan interview")
        );
    }
}
