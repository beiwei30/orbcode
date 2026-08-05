use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use orbcode_protocol::{
    AskUserCancellationReason, AskUserQuestionSpec, AskUserResponseOutcome, AskUserValidationError,
    validate_ask_user_outcome,
};
use tokio::sync::oneshot;

/// Behaviors an active client can faithfully complete for AskUserQuestion.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InteractiveQuestionCapabilities {
    pub single_select: bool,
    pub multi_select: bool,
    pub free_text: bool,
    pub previews: bool,
    pub annotations: bool,
    pub special_outcomes: bool,
}

impl InteractiveQuestionCapabilities {
    pub fn full() -> Self {
        Self {
            single_select: true,
            multi_select: true,
            free_text: true,
            previews: true,
            annotations: true,
            special_outcomes: true,
        }
    }

    pub fn fully_supported(&self) -> bool {
        self.single_select
            && self.multi_select
            && self.free_text
            && self.previews
            && self.annotations
            && self.special_outcomes
    }

    pub(crate) fn can_complete(&self, questions: &[AskUserQuestionSpec]) -> bool {
        !questions.is_empty()
            && questions.iter().all(|question| {
                let selection_supported = if question.multi_select {
                    self.multi_select
                } else {
                    self.single_select
                };
                selection_supported
                    && (!question.options.is_empty() || self.free_text)
                    && (!question.allow_free_text || self.free_text)
                    && (!question.allow_annotation || self.annotations)
                    && (self.previews
                        || question
                            .options
                            .iter()
                            .all(|option| option.preview.is_none()))
            })
    }

    pub(crate) fn any_supported(&self) -> bool {
        self.single_select || self.multi_select || self.free_text
    }
}

/// Per-turn interaction ownership passed by the client-facing transport.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnInteractionContext {
    pub owner_id: String,
    pub capabilities: InteractiveQuestionCapabilities,
}

impl TurnInteractionContext {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn capable(owner_id: impl Into<String>) -> Self {
        Self {
            owner_id: owner_id.into(),
            capabilities: InteractiveQuestionCapabilities::full(),
        }
    }
}

pub(crate) struct PendingInteraction {
    pub(crate) session_id: String,
    pub(crate) turn_id: String,
    pub(crate) tool_use_id: String,
    pub(crate) owner_id: String,
    pub(crate) capability_snapshot: InteractiveQuestionCapabilities,
    pub(crate) deadline: Option<String>,
    pub(crate) questions: Vec<AskUserQuestionSpec>,
    pub(crate) response_tx: oneshot::Sender<AskUserResponseOutcome>,
}

impl std::fmt::Debug for PendingInteraction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingInteraction")
            .field("session_id", &self.session_id)
            .field("turn_id", &self.turn_id)
            .field("tool_use_id", &self.tool_use_id)
            .field("owner_id", &self.owner_id)
            .field("capability_snapshot", &self.capability_snapshot)
            .field("deadline", &self.deadline)
            .field("questions", &self.questions)
            .field("response_tx", &"<oneshot sender>")
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InteractionResolveError {
    #[error("unknown or already-resolved AskUserQuestion request `{request_id}`")]
    UnknownRequest { request_id: String },
    #[error("AskUserQuestion request `{request_id}` belongs to session `{expected_session}`")]
    WrongSession {
        request_id: String,
        expected_session: String,
    },
    #[error("invalid AskUserQuestion response: {0}")]
    InvalidResponse(#[from] AskUserValidationError),
    #[error("AskUserQuestion request `{request_id}` receiver is closed")]
    ReceiverClosed { request_id: String },
    #[error("duplicate AskUserQuestion request id `{request_id}`")]
    DuplicateRequest { request_id: String },
}

#[derive(Clone, Default)]
pub(crate) struct InteractionRuntime {
    pending: Arc<Mutex<HashMap<String, PendingInteraction>>>,
}

impl InteractionRuntime {
    pub(crate) fn register(
        &self,
        request_id: String,
        interaction: PendingInteraction,
    ) -> Result<(), InteractionResolveError> {
        let mut pending = self.pending.lock().unwrap();
        if pending.contains_key(&request_id) {
            return Err(InteractionResolveError::DuplicateRequest { request_id });
        }
        pending.insert(request_id, interaction);
        Ok(())
    }

    /// Validate while the record is still registered, then atomically remove
    /// and resolve it. Invalid responses leave the request pending.
    pub(crate) fn resolve(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: AskUserResponseOutcome,
    ) -> Result<(), InteractionResolveError> {
        let mut pending = self.pending.lock().unwrap();
        let interaction =
            pending
                .get(request_id)
                .ok_or_else(|| InteractionResolveError::UnknownRequest {
                    request_id: request_id.to_string(),
                })?;
        if interaction.session_id != session_id {
            return Err(InteractionResolveError::WrongSession {
                request_id: request_id.to_string(),
                expected_session: interaction.session_id.clone(),
            });
        }
        validate_ask_user_outcome(&interaction.questions, &outcome)?;
        let interaction = pending
            .remove(request_id)
            .expect("pending interaction exists after validation");
        interaction
            .response_tx
            .send(outcome)
            .map_err(|_| InteractionResolveError::ReceiverClosed {
                request_id: request_id.to_string(),
            })
    }

    pub(crate) fn cancel_request(&self, request_id: &str, reason: AskUserCancellationReason) {
        if let Some(interaction) = self.pending.lock().unwrap().remove(request_id) {
            let _ = interaction
                .response_tx
                .send(AskUserResponseOutcome::Cancelled { reason });
        }
    }

    pub(crate) fn cancel_requests(
        &self,
        request_ids: &[String],
        reason: AskUserCancellationReason,
    ) {
        let mut pending = self.pending.lock().unwrap();
        let interactions = request_ids
            .iter()
            .filter_map(|request_id| pending.remove(request_id))
            .collect::<Vec<_>>();
        drop(pending);
        for interaction in interactions {
            let _ = interaction
                .response_tx
                .send(AskUserResponseOutcome::Cancelled { reason });
        }
    }

    pub(crate) fn cancel_session(&self, session_id: &str, reason: AskUserCancellationReason) {
        self.cancel_matching(reason, |interaction| interaction.session_id == session_id);
    }

    pub(crate) fn cancel_turn(&self, turn_id: &str, reason: AskUserCancellationReason) {
        self.cancel_matching(reason, |interaction| interaction.turn_id == turn_id);
    }

    pub(crate) fn cancel_owner(&self, owner_id: &str, reason: AskUserCancellationReason) {
        self.cancel_matching(reason, |interaction| interaction.owner_id == owner_id);
    }

    pub(crate) fn cancel_all(&self, reason: AskUserCancellationReason) {
        self.cancel_matching(reason, |_| true);
    }

    fn cancel_matching(
        &self,
        reason: AskUserCancellationReason,
        predicate: impl Fn(&PendingInteraction) -> bool,
    ) {
        let mut pending = self.pending.lock().unwrap();
        let request_ids = pending
            .iter()
            .filter_map(|(request_id, interaction)| {
                predicate(interaction).then_some(request_id.clone())
            })
            .collect::<Vec<_>>();
        let interactions = request_ids
            .iter()
            .filter_map(|request_id| pending.remove(request_id))
            .collect::<Vec<_>>();
        drop(pending);
        for interaction in interactions {
            let _ = interaction
                .response_tx
                .send(AskUserResponseOutcome::Cancelled { reason });
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

impl Drop for InteractionRuntime {
    fn drop(&mut self) {
        if Arc::strong_count(&self.pending) == 1 {
            self.cancel_all(AskUserCancellationReason::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use orbcode_protocol::{AskUserAnswerValue, AskUserOption};

    use super::*;

    fn question() -> AskUserQuestionSpec {
        AskUserQuestionSpec {
            id: "database".into(),
            question: "Which database?".into(),
            header: "Database".into(),
            multi_select: false,
            options: vec![AskUserOption {
                id: "postgres".into(),
                label: "PostgreSQL".into(),
                description: String::new(),
                preview: None,
            }],
            allow_free_text: true,
            allow_annotation: false,
        }
    }

    fn register(
        runtime: &InteractionRuntime,
        request_id: &str,
    ) -> oneshot::Receiver<AskUserResponseOutcome> {
        register_for(runtime, request_id, "session-1", "turn-1", "owner-1")
    }

    fn register_for(
        runtime: &InteractionRuntime,
        request_id: &str,
        session_id: &str,
        turn_id: &str,
        owner_id: &str,
    ) -> oneshot::Receiver<AskUserResponseOutcome> {
        let (response_tx, response_rx) = oneshot::channel();
        runtime
            .register(
                request_id.into(),
                PendingInteraction {
                    session_id: session_id.into(),
                    turn_id: turn_id.into(),
                    tool_use_id: "tool-1".into(),
                    owner_id: owner_id.into(),
                    capability_snapshot: InteractiveQuestionCapabilities::full(),
                    deadline: None,
                    questions: vec![question()],
                    response_tx,
                },
            )
            .unwrap();
        response_rx
    }

    #[tokio::test]
    async fn ask_user_invalid_response_does_not_consume_pending_request() {
        let runtime = InteractionRuntime::default();
        let response_rx = register(&runtime, "request-1");
        let invalid = AskUserResponseOutcome::Answered {
            answers: BTreeMap::from([(
                "database".into(),
                AskUserAnswerValue::Selected {
                    option_id: "unknown".into(),
                },
            )]),
            annotations: BTreeMap::new(),
        };
        assert!(matches!(
            runtime.resolve("session-1", "request-1", invalid),
            Err(InteractionResolveError::InvalidResponse(_))
        ));
        assert_eq!(runtime.len(), 1);

        runtime
            .resolve("session-1", "request-1", AskUserResponseOutcome::Rejected)
            .unwrap();
        assert_eq!(response_rx.await.unwrap(), AskUserResponseOutcome::Rejected);
        assert_eq!(runtime.len(), 0);
    }

    #[tokio::test]
    async fn ask_user_cross_session_and_duplicate_resolution_are_rejected() {
        let runtime = InteractionRuntime::default();
        let response_rx = register(&runtime, "request-1");
        assert!(matches!(
            runtime.resolve("session-2", "request-1", AskUserResponseOutcome::Clarify),
            Err(InteractionResolveError::WrongSession { .. })
        ));
        runtime
            .resolve("session-1", "request-1", AskUserResponseOutcome::Clarify)
            .unwrap();
        assert_eq!(response_rx.await.unwrap(), AskUserResponseOutcome::Clarify);
        assert!(matches!(
            runtime.resolve("session-1", "request-1", AskUserResponseOutcome::Clarify),
            Err(InteractionResolveError::UnknownRequest { .. })
        ));
    }

    #[tokio::test]
    async fn ask_user_owner_cancellation_resolves_only_matching_requests() {
        let runtime = InteractionRuntime::default();
        let response_rx = register(&runtime, "request-1");
        runtime.cancel_owner("owner-1", AskUserCancellationReason::Disconnect);
        assert_eq!(
            response_rx.await.unwrap(),
            AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::Disconnect
            }
        );
        assert_eq!(runtime.len(), 0);
    }

    #[tokio::test]
    async fn ask_user_all_cancellation_scopes_resolve_only_their_targets() {
        let runtime = InteractionRuntime::default();
        let request_rx = register_for(&runtime, "by-request", "s1", "t1", "o1");
        let session_rx = register_for(&runtime, "by-session", "s2", "t2", "o2");
        let turn_rx = register_for(&runtime, "by-turn", "s3", "t3", "o3");
        let global_rx = register_for(&runtime, "by-global", "s4", "t4", "o4");

        runtime.cancel_request("by-request", AskUserCancellationReason::Timeout);
        runtime.cancel_session("s2", AskUserCancellationReason::SessionClosed);
        runtime.cancel_turn("t3", AskUserCancellationReason::Interrupt);
        assert_eq!(runtime.len(), 1);
        runtime.cancel_all(AskUserCancellationReason::Shutdown);

        assert_eq!(
            request_rx.await.unwrap(),
            AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::Timeout
            }
        );
        assert_eq!(
            session_rx.await.unwrap(),
            AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::SessionClosed
            }
        );
        assert_eq!(
            turn_rx.await.unwrap(),
            AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::Interrupt
            }
        );
        assert_eq!(
            global_rx.await.unwrap(),
            AskUserResponseOutcome::Cancelled {
                reason: AskUserCancellationReason::Shutdown
            }
        );
    }

    #[test]
    fn ask_user_duplicate_registration_is_rejected_without_replacement() {
        let runtime = InteractionRuntime::default();
        let _rx = register(&runtime, "duplicate");
        let (response_tx, _response_rx) = oneshot::channel();
        let error = runtime
            .register(
                "duplicate".into(),
                PendingInteraction {
                    session_id: "other-session".into(),
                    turn_id: "other-turn".into(),
                    tool_use_id: "other-tool".into(),
                    owner_id: "other-owner".into(),
                    capability_snapshot: InteractiveQuestionCapabilities::full(),
                    deadline: None,
                    questions: vec![question()],
                    response_tx,
                },
            )
            .expect_err("duplicate id");
        assert!(matches!(
            error,
            InteractionResolveError::DuplicateRequest { .. }
        ));
        assert_eq!(runtime.len(), 1);
    }
}
