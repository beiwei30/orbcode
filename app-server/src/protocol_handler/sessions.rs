use orbcode_app_server_protocol::{
    AcpDeleteSessionParams, BootstrapParams, ResponseResult, SessionIdParams,
    SetSessionEffortParams, SetSessionModelParams, SetSessionPermissionModeParams,
};
use serde::Deserialize;
use serde_json::Value;

use super::{core_error, success, try_parse};
use crate::AppServer;

impl AppServer {
    pub(super) async fn handle_session_bootstrap(&self, params: Option<Value>) -> ResponseResult {
        let p: BootstrapParams = try_parse!(params);
        match self.bootstrap_with_params(p).await {
            Ok(state) => success(serde_json::json!({
                "session": state.session,
                "bootstrap_event": state.bootstrap_event,
                "prompt_history": state.prompt_history,
                "available_tool_count": state.available_tool_count,
                "configured_mcp_server_count": state.configured_mcp_server_count,
                "enabled_mcp_capability_count": state.enabled_mcp_capability_count,
                "home_dir": state.home_dir,
                "cwd": state.cwd,
                "model_display_name": state.model_display_name,
                "default_provider": state.default_provider,
                "fallback_provider": state.fallback_provider,
                "max_retries": state.max_retries,
                "context_window_options": state.context_window_options,
                "max_output_token_options": state.max_output_token_options,
                "token_warning_options": state.token_warning_options,
                "theme": state.theme,
                "editor_mode": state.editor_mode,
                "permissions": state.permissions,
                "mcp_slash_suggestions": state.mcp_slash_suggestions,
                "statusline_command": state.statusline_command,
                "statusline_refresh_interval_secs": state.statusline_refresh_interval_secs,
            })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_list(&self, _params: Option<Value>) -> ResponseResult {
        match self.list_sessions().await {
            Ok(sessions) => success(sessions),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_rename(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
            new_title: String,
        }
        let p: Params = try_parse!(params);
        match self.rename_session(&p.session_id, &p.new_title).await {
            Ok(()) => super::success_empty(),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_fork(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
            title: Option<String>,
            note: Option<String>,
        }
        let p: Params = try_parse!(params);
        match self.fork_session(&p.session_id, p.title, p.note).await {
            Ok(session) => success(session),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_clear(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
        }
        let p: Params = try_parse!(params);
        match self.clear_session(&p.session_id).await {
            Ok(state) => success(state),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_rewind(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
            keep_messages: usize,
        }
        let p: Params = try_parse!(params);
        match self.rewind_session(&p.session_id, p.keep_messages).await {
            Ok(state) => success(state),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_record_message(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
            message: String,
        }
        let p: Params = try_parse!(params);
        match self.record_system_message(&p.session_id, p.message).await {
            Ok(_) => super::success_empty(),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_compact(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
        }
        let p: Params = try_parse!(params);
        match self.compact_session(&p.session_id).await {
            Ok(result) => success(result),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_compact_decision(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
        }
        let p: Params = try_parse!(params);
        match self.evaluate_manual_compact_decision(&p.session_id).await {
            Ok(decision) => success(decision),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_find_by_title(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            title: String,
        }
        let p: Params = try_parse!(params);
        match self.session_id_for_exact_custom_title(&p.title).await {
            Ok(session_id) => success(serde_json::json!({ "session_id": session_id })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_session_acp_load_preflight(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
        }
        let p: Params = try_parse!(params);
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
        match self.set_session_model(&p.session_id, p.model).await {
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

    pub(super) async fn handle_session_cleanup(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        success(self.cleanup_session(&p.session_id).await)
    }
}
