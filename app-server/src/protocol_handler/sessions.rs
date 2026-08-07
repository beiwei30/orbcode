use orbcode_app_server_protocol::{
    AcpDeleteSessionParams, BootstrapParams, ResponseResult, SessionFindByTitleParams,
    SessionFindByTitleResult, SessionForkParams, SessionForkResult, SessionGoalClearResult,
    SessionGoalContinueParams, SessionGoalContinueResult, SessionGoalGetResult,
    SessionGoalNotStartedReason, SessionGoalSetParams, SessionGoalSetResult, SessionIdParams,
    SessionListResult, SessionRecordMessageParams, SessionRenameParams, SessionRewindParams,
    SetSessionEffortParams, SetSessionModelParams, SetSessionPermissionModeParams,
};
use orbcode_core::{
    GoalContinuationOutcome, GoalNotStartedReason, GoalSetRequest, GoalUpdateAuthority,
};
use serde_json::Value;

use super::{core_error, invalid_params, success, try_parse};
use crate::AppServer;

impl AppServer {
    pub(super) async fn handle_session_goal_get(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        match self.get_goal(&p.session_id).await {
            Ok(goal) => success(SessionGoalGetResult { goal }),
            Err(error) => super::goal_error(error),
        }
    }

    pub(super) async fn handle_session_goal_set(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionGoalSetParams = try_parse!(params);
        match self
            .set_goal(GoalSetRequest {
                session_id: p.session_id,
                expected_revision: p.expected_revision,
                replace: p.replace,
                objective: p.objective,
                status: p.status,
                token_budget: p.token_budget,
                stop_reason: None,
                authority: GoalUpdateAuthority::User,
            })
            .await
        {
            Ok(goal) => success(SessionGoalSetResult { goal }),
            Err(error) => super::goal_error(error),
        }
    }

    pub(super) async fn handle_session_goal_clear(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        match self.clear_goal(&p.session_id).await {
            Ok(cleared) => success(SessionGoalClearResult { cleared }),
            Err(error) => super::goal_error(error),
        }
    }

    pub(super) async fn handle_session_goal_continue_without_client(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: SessionGoalContinueParams = try_parse!(params);
        match self
            .continue_goal_if_eligible(
                &p.session_id,
                &p.goal_id,
                p.expected_revision,
                false,
                orbcode_core::TurnInteractionContext::disabled(),
            )
            .await
        {
            Ok(GoalContinuationOutcome::NotStarted { reason, goal }) => {
                success(SessionGoalContinueResult::NotStarted {
                    reason: goal_not_started_reason_to_wire(reason),
                    goal,
                })
            }
            Ok(GoalContinuationOutcome::Started(_)) => {
                unreachable!("incapable direct call cannot start")
            }
            Err(error) => super::goal_error(error),
        }
    }

    pub(super) async fn handle_session_bootstrap(&self, params: Option<Value>) -> ResponseResult {
        let p: BootstrapParams = try_parse!(params);
        match self.bootstrap_with_params(p).await {
            Ok(state) => success(state),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_list(&self, _params: Option<Value>) -> ResponseResult {
        match self.list_sessions().await {
            Ok(sessions) => success(SessionListResult(sessions)),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_rename(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionRenameParams = try_parse!(params);
        match self.rename_session(&p.session_id, &p.new_title).await {
            Ok(()) => super::success_empty(),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_fork(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionForkParams = try_parse!(params);
        match self.fork_session(&p.session_id, p.title, p.note).await {
            Ok(session) => success(SessionForkResult(session)),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_clear(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        match self.clear_session(&p.session_id).await {
            Ok(state) => success(state),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_rewind(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionRewindParams = try_parse!(params);
        match self.rewind_session(&p.session_id, p.keep_messages).await {
            Ok(state) => success(state),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_record_message(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: SessionRecordMessageParams = try_parse!(params);
        match self.record_system_message(&p.session_id, p.message).await {
            Ok(_) => super::success_empty(),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_compact(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        match self.compact_session(&p.session_id).await {
            Ok(result) => success(result),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_compact_decision(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        match self.evaluate_manual_compact_decision(&p.session_id).await {
            Ok(decision) => success(decision),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_find_by_title(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: SessionFindByTitleParams = try_parse!(params);
        match self.session_id_for_exact_custom_title(&p.title).await {
            Ok(session_id) => success(SessionFindByTitleResult { session_id }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_acp_load_preflight(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        match self.acp_load_replay_preflight(&p.session_id).await {
            Ok(result) => success(result),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_acp_load_setup(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: BootstrapParams = try_parse!(params);
        match self.acp_load_setup(p).await {
            Ok(state) => success(state),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_acp_resume_setup(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: BootstrapParams = try_parse!(params);
        match self.acp_resume_setup(p).await {
            Ok(state) => success(state),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_acp_delete(&self, params: Option<Value>) -> ResponseResult {
        let p: AcpDeleteSessionParams = try_parse!(params);
        match self.acp_delete_session(p).await {
            Ok(()) => super::success_empty(),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_acp_close(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        self.cleanup_session(&p.session_id).await;
        super::success_empty()
    }

    pub(super) fn handle_session_control_state(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        match self.session_control_state(&p.session_id) {
            Ok(state) => success(state),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_set_permission_mode(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: SetSessionPermissionModeParams = try_parse!(params);
        match self
            .set_session_permission_mode(&p.session_id, p.mode)
            .await
        {
            Ok(state) => success(state),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_set_model(&self, params: Option<Value>) -> ResponseResult {
        let p: SetSessionModelParams = try_parse!(params);
        let selection = match p.selection() {
            Ok(selection) => selection,
            Err(message) => return invalid_params(message),
        };
        match self
            .set_session_model_override(&p.session_id, selection)
            .await
        {
            Ok(state) => success(state),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_set_effort(&self, params: Option<Value>) -> ResponseResult {
        let p: SetSessionEffortParams = try_parse!(params);
        match self.set_session_effort(&p.session_id, p.effort).await {
            Ok(state) => success(state),
            Err(e) => core_error(e),
        }
    }
}

pub(crate) fn goal_not_started_reason_to_wire(
    reason: GoalNotStartedReason,
) -> SessionGoalNotStartedReason {
    match reason {
        GoalNotStartedReason::Missing => SessionGoalNotStartedReason::Missing,
        GoalNotStartedReason::StaleRevision => SessionGoalNotStartedReason::StaleRevision,
        GoalNotStartedReason::Inactive => SessionGoalNotStartedReason::Inactive,
        GoalNotStartedReason::UsageLimited => SessionGoalNotStartedReason::UsageLimited,
        GoalNotStartedReason::BudgetLimited => SessionGoalNotStartedReason::BudgetLimited,
        GoalNotStartedReason::PendingUserInput => SessionGoalNotStartedReason::PendingUserInput,
        GoalNotStartedReason::ActiveTurn => SessionGoalNotStartedReason::ActiveTurn,
        GoalNotStartedReason::ClientNotCapable => SessionGoalNotStartedReason::ClientNotCapable,
    }
}
