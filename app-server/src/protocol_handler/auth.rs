use orbcode_app_server_protocol::ResponseResult;
use orbcode_config::AuthMethod;
use orbcode_protocol::ProviderId;
use serde::Deserialize;
use serde_json::Value;

use super::{core_error, success, try_parse};
use crate::AppServer;

impl AppServer {
    pub(super) async fn handle_auth_overview(&self, _params: Option<Value>) -> ResponseResult {
        match self.auth_overview().await {
            Ok(overview) => success(serde_json::json!({
                "store_path": overview.store_path,
                "entries": overview.entries.iter().map(|e| serde_json::json!({
                    "provider": e.provider,
                    "method": e.method,
                    "source_summary": e.source_summary,
                    "persisted": e.persisted,
                    "usable": e.usable,
                    "active": e.active,
                })).collect::<Vec<_>>(),
            })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_auth_login(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            provider: ProviderId,
            method: AuthMethod,
            token: Option<String>,
            env_var: Option<String>,
        }
        let p: Params = try_parse!(params);
        match self
            .auth_login(p.provider, p.method, p.token, p.env_var)
            .await
        {
            Ok(entry) => success(serde_json::json!({
                "provider": entry.provider,
                // Use Display (.to_string()) for the method so the wire
                // representation matches the TS CLI ("oauth_device") rather
                // than serde's snake_case ("o_auth_device").
                "method": entry.method.to_string(),
                "source_summary": entry.source_summary,
                "persisted": entry.persisted,
                "usable": entry.usable,
                "active": entry.active,
            })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_auth_logout(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            provider: Option<ProviderId>,
        }
        let p: Params = try_parse!(params);
        match self.auth_logout(p.provider).await {
            Ok(count) => success(serde_json::json!({ "removed": count })),
            Err(e) => core_error(e),
        }
    }
}
