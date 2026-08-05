use orbcode_app_server_protocol::{
    AuthLoginParams, AuthLoginResult, AuthLogoutParams, AuthLogoutResult, ResponseResult,
};
use serde_json::Value;

use super::{core_error, success, try_parse};
use crate::AppServer;
use crate::protocol_conversion::{
    auth_method_from_wire, auth_overview_to_wire, auth_status_entry_to_wire,
};

impl AppServer {
    pub(super) async fn handle_auth_overview(&self, _params: Option<Value>) -> ResponseResult {
        match self.auth_overview().await {
            Ok(overview) => {
                let overview = auth_overview_to_wire(overview);
                success(overview)
            }
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_auth_login(&self, params: Option<Value>) -> ResponseResult {
        let p: AuthLoginParams = try_parse!(params);
        match self
            .auth_login(
                p.provider,
                auth_method_from_wire(p.method),
                p.token,
                p.env_var,
            )
            .await
        {
            Ok(entry) => {
                let entry = auth_status_entry_to_wire(entry);
                success(AuthLoginResult {
                    provider: entry.provider,
                    method: entry.method.to_string(),
                    source_summary: entry.source_summary,
                    persisted: entry.persisted,
                    usable: entry.usable,
                    active: entry.active,
                })
            }
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_auth_logout(&self, params: Option<Value>) -> ResponseResult {
        let p: AuthLogoutParams = try_parse!(params);
        match self.auth_logout(p.provider).await {
            Ok(removed) => success(AuthLogoutResult { removed }),
            Err(e) => core_error(e),
        }
    }
}
