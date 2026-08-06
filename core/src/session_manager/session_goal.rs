use std::{sync::Arc, time::Instant};

use orbcode_protocol::{
    SessionGoal, SessionGoalStatus, SessionGoalTranscriptState, SessionGoalTurnTerminalKind,
    SessionRecord, StreamErrorCategory, StreamEvent, TokenUsage, TranscriptMessage,
    get_token_count_from_usage,
};
use thiserror::Error;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::SessionManager;
use crate::{CoreError, TurnInteractionContext};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalUpdateAuthority {
    User,
    Model,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalSetRequest {
    pub session_id: String,
    pub expected_revision: Option<u64>,
    pub replace: bool,
    pub objective: Option<String>,
    pub status: Option<SessionGoalStatus>,
    pub token_budget: Option<Option<u64>>,
    pub stop_reason: Option<Option<String>>,
    pub authority: GoalUpdateAuthority,
}

#[derive(Debug, Error)]
pub enum GoalError {
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("session has no current goal")]
    Missing,
    #[error("expected goal revision {expected}, but current revision is {actual}")]
    StaleRevision { expected: u64, actual: u64 },
    #[error("objective must not be empty")]
    EmptyObjective,
    #[error("token budget must be a positive integer")]
    InvalidTokenBudget,
    #[error("an unfinished goal already exists")]
    UnfinishedGoal,
    #[error("goal replacement requires explicit user authority")]
    ReplacementNotAllowed,
    #[error("invalid goal transition from {from:?} to {to:?} for {authority:?} authority")]
    InvalidTransition {
        from: SessionGoalStatus,
        to: SessionGoalStatus,
        authority: GoalUpdateAuthority,
    },
    #[error("budget-limited goal requires an explicit budget increase above tokens used")]
    BudgetIncreaseRequired,
    #[error("stop reason is only valid for a blocked goal and must not be empty")]
    InvalidStopReason,
    #[error(transparent)]
    Core(#[from] CoreError),
}

impl From<orbcode_session_store::SessionStoreError> for GoalError {
    fn from(error: orbcode_session_store::SessionStoreError) -> Self {
        Self::Core(error.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalNotStartedReason {
    Missing,
    StaleRevision,
    Inactive,
    UsageLimited,
    BudgetLimited,
    PendingUserInput,
    ActiveTurn,
    ClientNotCapable,
}

pub struct StartedGoalTurn {
    pub turn_id: String,
    pub goal: SessionGoal,
    pub events: mpsc::UnboundedReceiver<StreamEvent>,
}

pub enum GoalContinuationOutcome {
    Started(StartedGoalTurn),
    NotStarted {
        reason: GoalNotStartedReason,
        goal: Option<SessionGoal>,
    },
}

impl SessionManager {
    pub async fn get_goal(&self, session_id: &str) -> Result<Option<SessionGoal>, GoalError> {
        self.load_current_goal(session_id).await
    }

    pub async fn set_goal(&self, request: GoalSetRequest) -> Result<SessionGoal, GoalError> {
        let append_lock = self.transcript_append_lock(&request.session_id).await;
        let _guard = append_lock.lock().await;
        let current = self.load_current_goal(&request.session_id).await?;

        let next = match current {
            None => create_goal_from_request(&request)?,
            Some(current) if request.replace => replace_goal_from_request(&request, &current)?,
            Some(current) => mutate_goal_from_request(&request, current)?,
        };

        self.transcript_store.append_goal_snapshot(&next).await?;
        Ok(next)
    }

    pub async fn clear_goal(&self, session_id: &str) -> Result<bool, GoalError> {
        let append_lock = self.transcript_append_lock(session_id).await;
        let _guard = append_lock.lock().await;
        let Some(current) = self.load_current_goal(session_id).await? else {
            return Ok(false);
        };
        self.transcript_store
            .append_goal_cleared(
                session_id,
                &current.goal_id,
                current.revision.saturating_add(1),
            )
            .await?;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn continue_goal_if_eligible(
        &self,
        session_id: &str,
        goal_id: &str,
        expected_revision: u64,
        client_capable: bool,
        mut interaction: TurnInteractionContext,
    ) -> Result<GoalContinuationOutcome, GoalError> {
        let append_lock = self.transcript_append_lock(session_id).await;
        let guard = append_lock.lock().await;
        let current = self.load_current_goal(session_id).await?;

        if !client_capable {
            return Ok(GoalContinuationOutcome::NotStarted {
                reason: GoalNotStartedReason::ClientNotCapable,
                goal: current,
            });
        }
        interaction.persistent_goals = true;
        let Some(mut goal) = current else {
            return Ok(GoalContinuationOutcome::NotStarted {
                reason: GoalNotStartedReason::Missing,
                goal: None,
            });
        };
        if goal.goal_id != goal_id || goal.revision != expected_revision {
            return Ok(GoalContinuationOutcome::NotStarted {
                reason: GoalNotStartedReason::StaleRevision,
                goal: Some(goal),
            });
        }
        if self.has_queued_user_commands(session_id).await
            || self.interaction_runtime.has_pending_session(session_id)
        {
            return Ok(GoalContinuationOutcome::NotStarted {
                reason: GoalNotStartedReason::PendingUserInput,
                goal: Some(goal),
            });
        }
        if self.active_turns.has_active_session(session_id).await {
            return Ok(GoalContinuationOutcome::NotStarted {
                reason: GoalNotStartedReason::ActiveTurn,
                goal: Some(goal),
            });
        }
        let inactive_reason = match goal.status {
            SessionGoalStatus::Active => None,
            SessionGoalStatus::UsageLimited => Some(GoalNotStartedReason::UsageLimited),
            SessionGoalStatus::BudgetLimited => Some(GoalNotStartedReason::BudgetLimited),
            SessionGoalStatus::Paused
            | SessionGoalStatus::Blocked
            | SessionGoalStatus::Complete => Some(GoalNotStartedReason::Inactive),
            _ => Some(GoalNotStartedReason::Inactive),
        };
        if let Some(reason) = inactive_reason {
            return Ok(GoalContinuationOutcome::NotStarted {
                reason,
                goal: Some(goal),
            });
        }
        if goal
            .token_budget
            .is_some_and(|budget| goal.tokens_used >= budget)
        {
            goal.status = SessionGoalStatus::BudgetLimited;
            goal.revision = goal.revision.saturating_add(1);
            goal.updated_at = chrono::Utc::now();
            goal.stop_reason = Some("persistent goal token budget reached".to_string());
            self.transcript_store.append_goal_snapshot(&goal).await?;
            return Ok(GoalContinuationOutcome::NotStarted {
                reason: GoalNotStartedReason::BudgetLimited,
                goal: Some(goal),
            });
        }

        let turn_id = Uuid::new_v4();
        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.active_turns
            .insert(session_id, turn_id, cancel_flag.clone(), interaction)
            .await
            .map_err(GoalError::Core)?;

        goal.revision = goal.revision.saturating_add(1);
        goal.updated_at = chrono::Utc::now();
        goal.stop_reason = None;
        goal.last_goal_turn_id = Some(turn_id.to_string());
        if let Err(error) = self
            .transcript_store
            .append_goal_snapshot_and_turn_start(&goal, &turn_id.to_string())
            .await
        {
            self.active_turns
                .clear_if_matching(session_id, turn_id)
                .await;
            return Err(GoalError::Core(error.into()));
        }
        drop(guard);

        let prompt = continuation_prompt(&goal);
        let config = self.effective_config_for_session(session_id);
        let user_message = TranscriptMessage::new(orbcode_protocol::MessageRole::User, &prompt);
        let setup_result = async {
            self.live_session_registry
                .register_with_cwd(session_id, "interactive", config.cwd.clone())
                .await?;
            self.append_message(session_id, user_message.clone())
                .await?;
            self.provider_debug_trace.clear().await;
            Ok::<(), CoreError>(())
        }
        .await;
        if let Err(error) = setup_result {
            self.active_turns
                .clear_if_matching(session_id, turn_id)
                .await;
            return Err(GoalError::Core(error));
        }

        let (external_tx, external_rx) = mpsc::unbounded_channel();
        let _ = external_tx.send(StreamEvent::UserMessage {
            message: user_message,
        });
        self.spawn_supervised_goal_turn(
            session_id.to_string(),
            turn_id,
            prompt,
            config,
            cancel_flag,
            goal.clone(),
            external_tx,
        );

        Ok(GoalContinuationOutcome::Started(StartedGoalTurn {
            turn_id: turn_id.to_string(),
            goal,
            events: external_rx,
        }))
    }

    fn spawn_supervised_goal_turn(
        &self,
        session_id: String,
        turn_id: Uuid,
        prompt: String,
        config: orbcode_config::AppConfig,
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
        started_goal: SessionGoal,
        external_tx: mpsc::UnboundedSender<StreamEvent>,
    ) {
        let manager = self.clone();
        tokio::spawn(async move {
            let started_at = Instant::now();
            let (driver_tx, mut driver_rx) = mpsc::unbounded_channel();
            let driver_manager = manager.clone();
            let driver_session_id = session_id.clone();
            let driver_cancel_flag = cancel_flag.clone();
            let driver = tokio::spawn(async move {
                driver_manager
                    .run_turn_loop(
                        &driver_session_id,
                        turn_id,
                        &prompt,
                        &config,
                        driver_cancel_flag,
                        &driver_tx,
                    )
                    .await;
            });

            let mut aggregate_usage = TokenUsage::default();
            let mut saw_terminal = false;
            while let Some(event) = driver_rx.recv().await {
                accumulate_goal_usage(&mut aggregate_usage, &event);
                if event.is_terminal() {
                    saw_terminal = true;
                    let kind = goal_terminal_kind(&event);
                    let elapsed_seconds = started_at.elapsed().as_secs();
                    let terminal = manager
                        .finish_goal_turn(
                            &session_id,
                            &started_goal.goal_id,
                            started_goal.revision,
                            &turn_id.to_string(),
                            kind,
                            &aggregate_usage,
                            elapsed_seconds,
                        )
                        .await;
                    match terminal {
                        Ok(()) => {
                            let _ = external_tx.send(event);
                        }
                        Err(error) => {
                            let _ = external_tx.send(StreamEvent::Error {
                                session_id: Some(session_id.clone()),
                                provider: None,
                                category: None,
                                message: format!("failed to checkpoint persistent goal: {error}"),
                                suggestion: Some(
                                    "resume the goal explicitly after checking transcript storage"
                                        .to_string(),
                                ),
                            });
                        }
                    }
                    break;
                }
                if external_tx.send(event).is_err() {
                    let _ = manager.active_turns.cancel(&session_id).await;
                }
            }
            let _ = driver.await;

            if !saw_terminal {
                let elapsed_seconds = started_at.elapsed().as_secs();
                let _ = manager
                    .finish_goal_turn(
                        &session_id,
                        &started_goal.goal_id,
                        started_goal.revision,
                        &turn_id.to_string(),
                        SessionGoalTurnTerminalKind::Interrupted,
                        &aggregate_usage,
                        elapsed_seconds,
                    )
                    .await;
                let _ = external_tx.send(StreamEvent::Error {
                    session_id: Some(session_id.clone()),
                    provider: None,
                    category: Some(StreamErrorCategory::Interrupted),
                    message: "persistent goal turn ended without a terminal event".to_string(),
                    suggestion: Some("resume the paused goal explicitly".to_string()),
                });
            }
            manager.interaction_runtime.cancel_turn(
                &turn_id.to_string(),
                orbcode_protocol::AskUserCancellationReason::SessionClosed,
            );
            manager
                .active_turns
                .clear_if_matching(&session_id, turn_id)
                .await;
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn finish_goal_turn(
        &self,
        session_id: &str,
        goal_id: &str,
        started_revision: u64,
        turn_id: &str,
        terminal_kind: SessionGoalTurnTerminalKind,
        usage: &TokenUsage,
        elapsed_seconds: u64,
    ) -> Result<(), GoalError> {
        let append_lock = self.transcript_append_lock(session_id).await;
        let _guard = append_lock.lock().await;
        let Some(session) = self
            .transcript_store
            .load_session_if_present(session_id)
            .await?
        else {
            return Err(GoalError::SessionNotFound(session_id.to_string()));
        };
        let terminal_already_persisted = session.goal_transcript_records.iter().any(|record| {
            record.value["type"] == "goal-turn-terminal"
                && record.value["goalId"] == goal_id
                && record.value["turnId"] == turn_id
        });
        if terminal_already_persisted {
            return Ok(());
        }
        let Some(mut goal) = goal_for_live_turn(&session, turn_id) else {
            self.transcript_store
                .append_goal_turn_terminal(
                    session_id,
                    goal_id,
                    started_revision,
                    turn_id,
                    terminal_kind,
                    usage,
                    elapsed_seconds,
                )
                .await?;
            return Ok(());
        };
        if goal.goal_id != goal_id {
            self.transcript_store
                .append_goal_turn_terminal(
                    session_id,
                    goal_id,
                    started_revision,
                    turn_id,
                    terminal_kind,
                    usage,
                    elapsed_seconds,
                )
                .await?;
            return Ok(());
        }

        goal.tokens_used = goal
            .tokens_used
            .saturating_add(u64::from(get_token_count_from_usage(usage)));
        goal.elapsed_seconds = goal.elapsed_seconds.saturating_add(elapsed_seconds);
        goal.revision = goal.revision.saturating_add(1);
        goal.updated_at = chrono::Utc::now();
        goal.last_goal_turn_id = Some(turn_id.to_string());
        apply_terminal_status(&mut goal, terminal_kind);
        if goal
            .token_budget
            .is_some_and(|budget| goal.tokens_used >= budget)
            && goal.status == SessionGoalStatus::Active
        {
            goal.status = SessionGoalStatus::BudgetLimited;
            goal.stop_reason = Some("persistent goal token budget reached".to_string());
        }
        self.transcript_store
            .append_goal_turn_terminal_and_snapshot(
                &goal,
                started_revision,
                turn_id,
                terminal_kind,
                usage,
                elapsed_seconds,
            )
            .await?;
        Ok(())
    }

    async fn load_current_goal(&self, session_id: &str) -> Result<Option<SessionGoal>, GoalError> {
        match self
            .transcript_store
            .load_session_if_present(session_id)
            .await?
        {
            Some(session) => {
                let active_turn_id = self
                    .active_turns
                    .active_turn_id(session_id)
                    .await
                    .map(|turn_id| turn_id.to_string());
                Ok(active_turn_id
                    .as_deref()
                    .and_then(|turn_id| goal_before_recovery(&session, turn_id))
                    .or(session.goal))
            }
            None if self
                .session_controls
                .read()
                .expect("session controls lock poisoned")
                .contains_key(session_id) =>
            {
                Ok(None)
            }
            None => Err(GoalError::SessionNotFound(session_id.to_string())),
        }
    }
}

fn goal_before_recovery(session: &SessionRecord, turn_id: &str) -> Option<SessionGoal> {
    session
        .goal_transcript_records
        .iter()
        .rev()
        .find_map(|record| match &record.state {
            SessionGoalTranscriptState::Recovered {
                original,
                turn_id: recovered_turn_id,
                ..
            } if recovered_turn_id == turn_id => Some(original.clone()),
            _ => None,
        })
}

fn goal_for_live_turn(session: &SessionRecord, turn_id: &str) -> Option<SessionGoal> {
    goal_before_recovery(session, turn_id).or_else(|| session.goal.clone())
}

fn create_goal_from_request(request: &GoalSetRequest) -> Result<SessionGoal, GoalError> {
    if request.expected_revision.is_some() {
        return Err(GoalError::Missing);
    }
    if request.replace {
        return Err(GoalError::Missing);
    }
    let objective = validated_objective(request.objective.as_deref())?;
    let status = request.status.unwrap_or(SessionGoalStatus::Active);
    if status != SessionGoalStatus::Active {
        return Err(GoalError::InvalidTransition {
            from: SessionGoalStatus::Active,
            to: status,
            authority: request.authority,
        });
    }
    let token_budget = validated_budget(request.token_budget)?;
    let now = chrono::Utc::now();
    Ok(SessionGoal {
        goal_id: Uuid::new_v4().to_string(),
        revision: 1,
        session_id: request.session_id.clone(),
        objective,
        status: SessionGoalStatus::Active,
        token_budget,
        tokens_used: 0,
        elapsed_seconds: 0,
        created_at: now,
        updated_at: now,
        stop_reason: None,
        last_goal_turn_id: None,
    })
}

fn replace_goal_from_request(
    request: &GoalSetRequest,
    current: &SessionGoal,
) -> Result<SessionGoal, GoalError> {
    validate_expected_revision(request.expected_revision, current.revision)?;
    if request.authority == GoalUpdateAuthority::System {
        return Err(GoalError::ReplacementNotAllowed);
    }
    if request.authority == GoalUpdateAuthority::Model
        && current.status != SessionGoalStatus::Complete
    {
        return Err(GoalError::UnfinishedGoal);
    }
    create_goal_from_request(&GoalSetRequest {
        expected_revision: None,
        replace: false,
        ..request.clone()
    })
}

fn mutate_goal_from_request(
    request: &GoalSetRequest,
    mut goal: SessionGoal,
) -> Result<SessionGoal, GoalError> {
    validate_expected_revision(request.expected_revision, goal.revision)?;
    if let Some(objective) = request.objective.as_deref() {
        goal.objective = validated_objective(Some(objective))?;
    }
    let old_budget = goal.token_budget;
    if request.token_budget.is_some() {
        goal.token_budget = validated_budget(request.token_budget)?;
    }
    if let Some(status) = request.status
        && status != goal.status
    {
        validate_transition(
            goal.status,
            status,
            request.authority,
            old_budget,
            goal.token_budget,
            goal.tokens_used,
        )?;
        goal.status = status;
        goal.stop_reason = None;
    }
    if let Some(stop_reason) = request.stop_reason.as_ref() {
        if goal.status != SessionGoalStatus::Blocked {
            return Err(GoalError::InvalidStopReason);
        }
        let stop_reason = stop_reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .ok_or(GoalError::InvalidStopReason)?;
        goal.stop_reason = Some(stop_reason.to_string());
    } else if request.status == Some(SessionGoalStatus::Blocked) {
        goal.stop_reason = Some("model reported the persistent goal blocked".to_string());
    }
    if goal.status == SessionGoalStatus::Active
        && goal
            .token_budget
            .is_some_and(|budget| goal.tokens_used >= budget)
    {
        goal.status = SessionGoalStatus::BudgetLimited;
        goal.stop_reason = Some("persistent goal token budget reached".to_string());
    }
    goal.revision = goal.revision.saturating_add(1);
    goal.updated_at = chrono::Utc::now();
    Ok(goal)
}

fn validate_expected_revision(expected: Option<u64>, actual: u64) -> Result<(), GoalError> {
    match expected {
        Some(expected) if expected == actual => Ok(()),
        Some(expected) => Err(GoalError::StaleRevision { expected, actual }),
        None => Err(GoalError::StaleRevision {
            expected: 0,
            actual,
        }),
    }
}

fn validated_objective(objective: Option<&str>) -> Result<String, GoalError> {
    let objective = objective.ok_or(GoalError::EmptyObjective)?.trim();
    if objective.is_empty() {
        Err(GoalError::EmptyObjective)
    } else {
        Ok(objective.to_string())
    }
}

fn validated_budget(value: Option<Option<u64>>) -> Result<Option<u64>, GoalError> {
    match value {
        None | Some(None) => Ok(None),
        Some(Some(0)) => Err(GoalError::InvalidTokenBudget),
        Some(Some(value)) => Ok(Some(value)),
    }
}

fn validate_transition(
    from: SessionGoalStatus,
    to: SessionGoalStatus,
    authority: GoalUpdateAuthority,
    old_budget: Option<u64>,
    new_budget: Option<u64>,
    tokens_used: u64,
) -> Result<(), GoalError> {
    let allowed = match authority {
        GoalUpdateAuthority::Model => {
            from == SessionGoalStatus::Active
                && matches!(to, SessionGoalStatus::Blocked | SessionGoalStatus::Complete)
        }
        GoalUpdateAuthority::System => {
            from == SessionGoalStatus::Active
                && matches!(
                    to,
                    SessionGoalStatus::Paused
                        | SessionGoalStatus::UsageLimited
                        | SessionGoalStatus::BudgetLimited
                )
        }
        GoalUpdateAuthority::User => match from {
            SessionGoalStatus::Active => {
                matches!(
                    to,
                    SessionGoalStatus::Paused
                        | SessionGoalStatus::Blocked
                        | SessionGoalStatus::Complete
                )
            }
            SessionGoalStatus::Paused | SessionGoalStatus::Blocked => {
                matches!(to, SessionGoalStatus::Active | SessionGoalStatus::Complete)
            }
            SessionGoalStatus::UsageLimited => to == SessionGoalStatus::Active,
            SessionGoalStatus::BudgetLimited => {
                if to != SessionGoalStatus::Active {
                    false
                } else {
                    let increased = match (old_budget, new_budget) {
                        (Some(old), Some(new)) => new > old,
                        (None, Some(_)) => true,
                        (Some(_), None) => true,
                        (None, None) => false,
                    };
                    if !increased || new_budget.is_some_and(|budget| budget <= tokens_used) {
                        return Err(GoalError::BudgetIncreaseRequired);
                    }
                    true
                }
            }
            SessionGoalStatus::Complete => false,
            _ => false,
        },
    };
    if allowed {
        Ok(())
    } else {
        Err(GoalError::InvalidTransition {
            from,
            to,
            authority,
        })
    }
}

fn continuation_prompt(goal: &SessionGoal) -> String {
    let budget = goal.token_budget.map_or_else(
        || "none".to_string(),
        |budget| format!("{}/{} tokens", goal.tokens_used, budget),
    );
    format!(
        "Continue working toward the active persistent goal.\n\nObjective:\n{}\n\nCurrent revision: {}\nToken budget: {}\n\nUse get_goal to inspect current state. Only mark the goal complete when every requirement is verified; mark it blocked only under the documented repeated-blocker rule.",
        goal.objective, goal.revision, budget
    )
}

fn accumulate_goal_usage(total: &mut TokenUsage, event: &StreamEvent) {
    let usage = match event {
        StreamEvent::AssistantMessageCompleted { usage, .. } => Some(usage),
        StreamEvent::TurnCancelled {
            usage: Some(usage), ..
        } => Some(usage),
        _ => None,
    };
    let Some(usage) = usage else {
        return;
    };
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.cache_creation_input_tokens = total
        .cache_creation_input_tokens
        .saturating_add(usage.cache_creation_input_tokens);
    total.cache_read_input_tokens = total
        .cache_read_input_tokens
        .saturating_add(usage.cache_read_input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.server_tool_use.web_search_requests = total
        .server_tool_use
        .web_search_requests
        .saturating_add(usage.server_tool_use.web_search_requests);
    total.server_tool_use.web_fetch_requests = total
        .server_tool_use
        .web_fetch_requests
        .saturating_add(usage.server_tool_use.web_fetch_requests);
    total.cache_creation.ephemeral_1h_input_tokens = total
        .cache_creation
        .ephemeral_1h_input_tokens
        .saturating_add(usage.cache_creation.ephemeral_1h_input_tokens);
    total.cache_creation.ephemeral_5m_input_tokens = total
        .cache_creation
        .ephemeral_5m_input_tokens
        .saturating_add(usage.cache_creation.ephemeral_5m_input_tokens);
    total.iterations.extend(usage.iterations.clone());
    total.service_tier.clone_from(&usage.service_tier);
    total.speed.clone_from(&usage.speed);
    total.refresh_total_from_components();
}

fn goal_terminal_kind(event: &StreamEvent) -> SessionGoalTurnTerminalKind {
    match event {
        StreamEvent::TurnFinished { .. } => SessionGoalTurnTerminalKind::Finished,
        StreamEvent::TurnCancelled { .. } => SessionGoalTurnTerminalKind::Cancelled,
        StreamEvent::Budget { blocked: true, .. } => SessionGoalTurnTerminalKind::BudgetLimited,
        StreamEvent::Error {
            category: Some(StreamErrorCategory::RateLimit | StreamErrorCategory::AccountSuspended),
            ..
        } => SessionGoalTurnTerminalKind::UsageLimited,
        StreamEvent::Error { .. } => SessionGoalTurnTerminalKind::Error,
        _ => SessionGoalTurnTerminalKind::Interrupted,
    }
}

fn apply_terminal_status(goal: &mut SessionGoal, kind: SessionGoalTurnTerminalKind) {
    if goal.status != SessionGoalStatus::Active {
        return;
    }
    match kind {
        SessionGoalTurnTerminalKind::Finished => {
            goal.stop_reason = None;
        }
        SessionGoalTurnTerminalKind::Cancelled => {
            goal.status = SessionGoalStatus::Paused;
            goal.stop_reason = Some("persistent goal turn cancelled".to_string());
        }
        SessionGoalTurnTerminalKind::UsageLimited => {
            goal.status = SessionGoalStatus::UsageLimited;
            goal.stop_reason = Some("provider usage limit reached".to_string());
        }
        SessionGoalTurnTerminalKind::BudgetLimited => {
            goal.status = SessionGoalStatus::Paused;
            goal.stop_reason = Some("session cost budget blocked the goal turn".to_string());
        }
        SessionGoalTurnTerminalKind::Error => {
            goal.status = SessionGoalStatus::Paused;
            goal.stop_reason = Some("persistent goal turn failed".to_string());
        }
        SessionGoalTurnTerminalKind::Interrupted => {
            goal.status = SessionGoalStatus::Paused;
            goal.stop_reason = Some("persistent goal turn interrupted".to_string());
        }
    }
}
