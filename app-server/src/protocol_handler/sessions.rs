use orbcode_app_server_protocol::{
    AcpDeleteSessionParams, BootstrapParams, ResponseResult, SessionFindByTitleParams,
    SessionFindByTitleResult, SessionForkParams, SessionForkResult, SessionIdParams,
    SessionListResult, SessionRecordMessageParams, SessionRenameParams, SessionRewindParams,
};
use serde_json::Value;

use super::{core_error, success, try_parse};
use crate::AppServer;

impl AppServer {
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
        self.remove_session_mcp_servers(&p.session_id).await;
        super::success_empty()
    }
}
