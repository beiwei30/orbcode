use std::collections::{BTreeMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const ASK_USER_MAX_QUESTIONS: usize = 4;
pub const ASK_USER_MAX_OPTIONS: usize = 4;
pub const ASK_USER_MAX_ID_BYTES: usize = 128;
pub const ASK_USER_MAX_QUESTION_BYTES: usize = 4 * 1024;
pub const ASK_USER_MAX_HEADER_CHARS: usize = 12;
pub const ASK_USER_MAX_LABEL_BYTES: usize = 256;
pub const ASK_USER_MAX_DESCRIPTION_BYTES: usize = 1024;
pub const ASK_USER_MAX_PREVIEW_BYTES: usize = 16 * 1024;
pub const ASK_USER_MAX_ANNOTATION_BYTES: usize = 4 * 1024;
pub const ASK_USER_MAX_REQUEST_BYTES: usize = 64 * 1024;

fn default_true() -> bool {
    true
}

/// One selectable answer presented for an interactive question.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct AskUserOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// Canonical model-visible specification for one interactive question.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct AskUserQuestionSpec {
    pub id: String,
    pub question: String,
    pub header: String,
    #[serde(default)]
    pub multi_select: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<AskUserOption>,
    /// Whether the client should offer an automatic Other/free-text path.
    #[serde(default = "default_true")]
    pub allow_free_text: bool,
    /// Whether the client should offer an annotation/note field.
    #[serde(default)]
    pub allow_annotation: bool,
}

/// A typed answer to one canonical question.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskUserAnswerValue {
    Text { text: String },
    Selected { option_id: String },
    SelectedMany { option_ids: Vec<String> },
}

/// Why a pending interactive question was cancelled.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AskUserCancellationReason {
    Interrupt,
    Disconnect,
    Timeout,
    ClientClosed,
    DeliveryFailed,
    SessionClosed,
    Shutdown,
}

/// Canonical response lifecycle for an interactive question request.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AskUserResponseOutcome {
    Answered {
        answers: BTreeMap<String, AskUserAnswerValue>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        annotations: BTreeMap<String, String>,
    },
    Rejected,
    Clarify,
    FinishPlanInterview,
    Cancelled {
        reason: AskUserCancellationReason,
    },
}

/// Stable machine-readable category for request or response validation errors.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AskUserValidationCode {
    QuestionCount,
    EmptyId,
    DuplicateId,
    EmptyQuestion,
    HeaderTooLong,
    OptionCount,
    EmptyLabel,
    DuplicateLabel,
    FieldTooLarge,
    RequestTooLarge,
    MissingAnswer,
    UnknownQuestion,
    UnknownOption,
    AnswerKind,
    FreeTextDisabled,
    AnnotationDisabled,
    MalformedResponse,
    RequestIdMismatch,
}

/// Validation failure safe to return to a client or model as structured data.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct AskUserValidationError {
    pub code: AskUserValidationCode,
    pub message: String,
}

impl AskUserValidationError {
    fn new(code: AskUserValidationCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AskUserValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AskUserValidationError {}

pub fn validate_ask_user_questions(
    questions: &[AskUserQuestionSpec],
) -> Result<(), AskUserValidationError> {
    if questions.is_empty() || questions.len() > ASK_USER_MAX_QUESTIONS {
        return Err(AskUserValidationError::new(
            AskUserValidationCode::QuestionCount,
            format!("AskUserQuestion requires 1 to {ASK_USER_MAX_QUESTIONS} questions"),
        ));
    }

    let mut question_ids = HashSet::new();
    for question in questions {
        validate_non_empty_id("question", &question.id)?;
        if !question_ids.insert(question.id.as_str()) {
            return Err(AskUserValidationError::new(
                AskUserValidationCode::DuplicateId,
                format!("duplicate question id `{}`", question.id),
            ));
        }
        if question.question.trim().is_empty() {
            return Err(AskUserValidationError::new(
                AskUserValidationCode::EmptyQuestion,
                format!("question `{}` has empty text", question.id),
            ));
        }
        validate_bytes(
            "question text",
            &question.question,
            ASK_USER_MAX_QUESTION_BYTES,
        )?;
        if question.header.chars().count() > ASK_USER_MAX_HEADER_CHARS {
            return Err(AskUserValidationError::new(
                AskUserValidationCode::HeaderTooLong,
                format!(
                    "question `{}` header exceeds {ASK_USER_MAX_HEADER_CHARS} characters",
                    question.id
                ),
            ));
        }
        if question.options.len() > ASK_USER_MAX_OPTIONS {
            return Err(AskUserValidationError::new(
                AskUserValidationCode::OptionCount,
                format!(
                    "question `{}` has more than {ASK_USER_MAX_OPTIONS} options",
                    question.id
                ),
            ));
        }

        let mut option_ids = HashSet::new();
        let mut option_labels = HashSet::new();
        for option in &question.options {
            validate_non_empty_id("option", &option.id)?;
            if !option_ids.insert(option.id.as_str()) {
                return Err(AskUserValidationError::new(
                    AskUserValidationCode::DuplicateId,
                    format!(
                        "question `{}` contains duplicate option id `{}`",
                        question.id, option.id
                    ),
                ));
            }
            if option.label.trim().is_empty() {
                return Err(AskUserValidationError::new(
                    AskUserValidationCode::EmptyLabel,
                    format!("question `{}` contains an empty option label", question.id),
                ));
            }
            if !option_labels.insert(option.label.as_str()) {
                return Err(AskUserValidationError::new(
                    AskUserValidationCode::DuplicateLabel,
                    format!(
                        "question `{}` contains duplicate option label `{}`",
                        question.id, option.label
                    ),
                ));
            }
            validate_bytes("option label", &option.label, ASK_USER_MAX_LABEL_BYTES)?;
            validate_bytes(
                "option description",
                &option.description,
                ASK_USER_MAX_DESCRIPTION_BYTES,
            )?;
            if let Some(preview) = option.preview.as_deref() {
                validate_bytes("option preview", preview, ASK_USER_MAX_PREVIEW_BYTES)?;
            }
        }
    }

    let encoded_len = serde_json::to_vec(questions).map_or(usize::MAX, |value| value.len());
    if encoded_len > ASK_USER_MAX_REQUEST_BYTES {
        return Err(AskUserValidationError::new(
            AskUserValidationCode::RequestTooLarge,
            format!("AskUserQuestion request exceeds {ASK_USER_MAX_REQUEST_BYTES} bytes"),
        ));
    }
    Ok(())
}

pub fn validate_ask_user_outcome(
    questions: &[AskUserQuestionSpec],
    outcome: &AskUserResponseOutcome,
) -> Result<(), AskUserValidationError> {
    validate_ask_user_questions(questions)?;
    let AskUserResponseOutcome::Answered {
        answers,
        annotations,
    } = outcome
    else {
        return Ok(());
    };

    if answers.len() != questions.len() {
        return Err(AskUserValidationError::new(
            AskUserValidationCode::MissingAnswer,
            "answered outcome must contain exactly one answer for every question",
        ));
    }
    for question_id in answers.keys().chain(annotations.keys()) {
        if !questions.iter().any(|question| question.id == *question_id) {
            return Err(AskUserValidationError::new(
                AskUserValidationCode::UnknownQuestion,
                format!("response references unknown question `{question_id}`"),
            ));
        }
    }

    for question in questions {
        let answer = answers.get(&question.id).ok_or_else(|| {
            AskUserValidationError::new(
                AskUserValidationCode::MissingAnswer,
                format!(
                    "response is missing an answer for question `{}`",
                    question.id
                ),
            )
        })?;
        match answer {
            AskUserAnswerValue::Text { text } => {
                if !question.allow_free_text {
                    return Err(AskUserValidationError::new(
                        AskUserValidationCode::FreeTextDisabled,
                        format!("question `{}` does not allow free text", question.id),
                    ));
                }
                if text.trim().is_empty() {
                    return Err(AskUserValidationError::new(
                        AskUserValidationCode::MissingAnswer,
                        format!("question `{}` has an empty text answer", question.id),
                    ));
                }
                validate_bytes("answer text", text, ASK_USER_MAX_QUESTION_BYTES)?;
            }
            AskUserAnswerValue::Selected { option_id } => {
                if question.multi_select {
                    return Err(answer_kind_error(question));
                }
                validate_selected_option(question, option_id)?;
            }
            AskUserAnswerValue::SelectedMany { option_ids } => {
                if !question.multi_select || option_ids.is_empty() {
                    return Err(answer_kind_error(question));
                }
                let mut unique = HashSet::new();
                for option_id in option_ids {
                    if !unique.insert(option_id.as_str()) {
                        return Err(AskUserValidationError::new(
                            AskUserValidationCode::DuplicateId,
                            format!(
                                "question `{}` selected option `{option_id}` more than once",
                                question.id
                            ),
                        ));
                    }
                    validate_selected_option(question, option_id)?;
                }
            }
        }
        if let Some(annotation) = annotations.get(&question.id) {
            if !question.allow_annotation {
                return Err(AskUserValidationError::new(
                    AskUserValidationCode::AnnotationDisabled,
                    format!("question `{}` does not allow an annotation", question.id),
                ));
            }
            validate_bytes(
                "answer annotation",
                annotation,
                ASK_USER_MAX_ANNOTATION_BYTES,
            )?;
        }
    }
    Ok(())
}

fn validate_non_empty_id(kind: &str, id: &str) -> Result<(), AskUserValidationError> {
    if id.trim().is_empty() {
        return Err(AskUserValidationError::new(
            AskUserValidationCode::EmptyId,
            format!("{kind} id must not be empty"),
        ));
    }
    validate_bytes(&format!("{kind} id"), id, ASK_USER_MAX_ID_BYTES)
}

fn validate_bytes(field: &str, value: &str, max: usize) -> Result<(), AskUserValidationError> {
    if value.len() > max {
        return Err(AskUserValidationError::new(
            AskUserValidationCode::FieldTooLarge,
            format!("{field} exceeds {max} bytes"),
        ));
    }
    Ok(())
}

fn answer_kind_error(question: &AskUserQuestionSpec) -> AskUserValidationError {
    AskUserValidationError::new(
        AskUserValidationCode::AnswerKind,
        format!(
            "answer kind does not match multi_select={} for question `{}`",
            question.multi_select, question.id
        ),
    )
}

fn validate_selected_option(
    question: &AskUserQuestionSpec,
    option_id: &str,
) -> Result<(), AskUserValidationError> {
    if question.options.iter().any(|option| option.id == option_id) {
        Ok(())
    } else {
        Err(AskUserValidationError::new(
            AskUserValidationCode::UnknownOption,
            format!(
                "question `{}` response references unknown option `{option_id}`",
                question.id
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(multi_select: bool) -> AskUserQuestionSpec {
        AskUserQuestionSpec {
            id: "database".into(),
            question: "Which database?".into(),
            header: "Database".into(),
            multi_select,
            options: vec![
                AskUserOption {
                    id: "postgres".into(),
                    label: "PostgreSQL".into(),
                    description: "Relational database".into(),
                    preview: Some("CREATE TABLE ...".into()),
                },
                AskUserOption {
                    id: "sqlite".into(),
                    label: "SQLite".into(),
                    description: "Embedded database".into(),
                    preview: None,
                },
            ],
            allow_free_text: true,
            allow_annotation: true,
        }
    }

    #[test]
    fn ask_user_canonical_questions_validate() {
        validate_ask_user_questions(&[question(false)]).unwrap();
    }

    #[test]
    fn ask_user_question_count_and_duplicate_ids_are_rejected() {
        assert_eq!(
            validate_ask_user_questions(&[]).unwrap_err().code,
            AskUserValidationCode::QuestionCount
        );
        let q = question(false);
        assert_eq!(
            validate_ask_user_questions(&[q.clone(), q])
                .unwrap_err()
                .code,
            AskUserValidationCode::DuplicateId
        );
    }

    #[test]
    fn ask_user_selected_answer_must_match_question_shape_and_option_ids() {
        let questions = [question(false)];
        let bad_kind = AskUserResponseOutcome::Answered {
            answers: BTreeMap::from([(
                "database".into(),
                AskUserAnswerValue::SelectedMany {
                    option_ids: vec!["postgres".into()],
                },
            )]),
            annotations: BTreeMap::new(),
        };
        assert_eq!(
            validate_ask_user_outcome(&questions, &bad_kind)
                .unwrap_err()
                .code,
            AskUserValidationCode::AnswerKind
        );

        let unknown = AskUserResponseOutcome::Answered {
            answers: BTreeMap::from([(
                "database".into(),
                AskUserAnswerValue::Selected {
                    option_id: "mysql".into(),
                },
            )]),
            annotations: BTreeMap::new(),
        };
        assert_eq!(
            validate_ask_user_outcome(&questions, &unknown)
                .unwrap_err()
                .code,
            AskUserValidationCode::UnknownOption
        );
    }

    #[test]
    fn ask_user_free_text_and_annotations_honor_affordance_flags() {
        let mut q = question(false);
        q.allow_free_text = false;
        q.allow_annotation = false;
        let text = AskUserResponseOutcome::Answered {
            answers: BTreeMap::from([(
                "database".into(),
                AskUserAnswerValue::Text {
                    text: "Other".into(),
                },
            )]),
            annotations: BTreeMap::new(),
        };
        assert_eq!(
            validate_ask_user_outcome(&[q.clone()], &text)
                .unwrap_err()
                .code,
            AskUserValidationCode::FreeTextDisabled
        );

        let annotated = AskUserResponseOutcome::Answered {
            answers: BTreeMap::from([(
                "database".into(),
                AskUserAnswerValue::Selected {
                    option_id: "postgres".into(),
                },
            )]),
            annotations: BTreeMap::from([("database".into(), "Use version 18".into())]),
        };
        assert_eq!(
            validate_ask_user_outcome(&[q], &annotated)
                .unwrap_err()
                .code,
            AskUserValidationCode::AnnotationDisabled
        );
    }
}
