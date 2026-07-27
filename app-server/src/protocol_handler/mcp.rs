use orbcode_app_server_protocol::ResponseResult;
use orbcode_mcp::McpServerTrust;
use serde::Deserialize;
use serde_json::Value;

use super::{core_error, success, success_empty, try_parse};
use crate::AppServer;

impl AppServer {
    pub(super) async fn handle_mcp_list_servers(&self, _params: Option<Value>) -> ResponseResult {
        success(self.list_mcp_servers().await)
    }

    pub(super) async fn handle_mcp_server_trust(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: String,
        }
        let p: Params = try_parse!(params);
        let trust = self.mcp_server_trust(&p.server_id).await;
        success(serde_json::json!({ "trust": trust }))
    }

    pub(super) async fn handle_mcp_set_trust(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: String,
            trust: McpServerTrust,
        }
        let p: Params = try_parse!(params);
        match self.set_mcp_server_trust(&p.server_id, p.trust).await {
            Ok(()) => success_empty(),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_list_tools(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: String,
        }
        let p: Params = try_parse!(params);
        match self.list_mcp_tools(&p.server_id).await {
            Ok(tools) => success(tools),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_list_resources(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: String,
            #[serde(default)]
            session_id: Option<String>,
        }
        let p: Params = try_parse!(params);
        let result = match p.session_id.as_deref() {
            Some(session_id) => {
                self.list_mcp_resources_for_session(session_id, &p.server_id)
                    .await
            }
            None => self.list_mcp_resources(&p.server_id).await,
        };
        match result {
            Ok(resources) => success(resources),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_read_resource(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: String,
            uri: String,
            #[serde(default)]
            session_id: Option<String>,
        }
        let p: Params = try_parse!(params);
        let result = match p.session_id.as_deref() {
            Some(session_id) => {
                self.read_mcp_resource_for_session(session_id, &p.server_id, &p.uri)
                    .await
            }
            None => self.read_mcp_resource(&p.server_id, &p.uri).await,
        };
        match result {
            Ok(content) => success(content),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_list_prompts(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: String,
            #[serde(default)]
            session_id: Option<String>,
        }
        let p: Params = try_parse!(params);
        let result = match p.session_id.as_deref() {
            Some(session_id) => {
                self.list_mcp_prompts_for_session(session_id, &p.server_id)
                    .await
            }
            None => self.list_mcp_prompts(&p.server_id).await,
        };
        match result {
            Ok(prompts) => success(prompts),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_get_prompt(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: String,
            name: String,
            #[serde(default)]
            arguments: serde_json::Value,
            #[serde(default)]
            session_id: Option<String>,
        }
        let p: Params = try_parse!(params);
        let result = match p.session_id.as_deref() {
            Some(session_id) => {
                self.get_mcp_prompt_for_session(session_id, &p.server_id, &p.name, p.arguments)
                    .await
            }
            None => {
                self.get_mcp_prompt(&p.server_id, &p.name, p.arguments)
                    .await
            }
        };
        match result {
            Ok(result) => success(result),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_invoke_tool(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: String,
            tool_name: String,
            #[serde(default = "default_empty_object")]
            input: String,
        }
        let p: Params = try_parse!(params);
        match self
            .invoke_mcp_tool(&p.server_id, &p.tool_name, p.input)
            .await
        {
            Ok(result) => success(result),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_diagnose(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: String,
        }
        let p: Params = try_parse!(params);
        match self.diagnose_mcp_server(&p.server_id).await {
            Ok(checks) => success(checks),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_upsert_server(&self, params: Option<Value>) -> ResponseResult {
        let config: orbcode_mcp::McpServerConfig = try_parse!(params);
        match self.upsert_mcp_server(config).await {
            Ok(()) => success_empty(),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_remove_server(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: String,
        }
        let p: Params = try_parse!(params);
        match self.remove_mcp_server(&p.server_id).await {
            Ok(removed) => success(serde_json::json!({ "removed": removed })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_capabilities(&self, _params: Option<Value>) -> ResponseResult {
        success(self.mcp_capabilities().await)
    }

    pub(super) async fn handle_mcp_slash_suggestions(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        let catalog = self.mcp_slash_suggestions().await;
        success(serde_json::json!({
            "servers": catalog.servers.iter().map(|s| serde_json::json!({
                "id": s.id,
                "summary": s.summary,
            })).collect::<Vec<_>>(),
            "tools": catalog.tools.iter().map(|t| serde_json::json!({
                "server_id": t.server_id,
                "name": t.name,
                "provider_name": t.provider_name,
                "description": t.description,
            })).collect::<Vec<_>>(),
            "resources": catalog.resources.iter().map(|r| serde_json::json!({
                "server_id": r.server_id,
                "uri": r.uri,
                "name": r.name,
                "description": r.description,
            })).collect::<Vec<_>>(),
        }))
    }

    pub(super) async fn handle_mcp_oauth_overview(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: Option<String>,
        }
        let p: Params = try_parse!(params);
        match self.mcp_oauth_overview(p.server_id.as_deref()).await {
            Ok(overview) => {
                let entries: Vec<Value> = overview
                    .entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "server_id": e.server_id,
                            "source_summary": e.source_summary,
                            "usable": e.usable,
                            "expired": e.expired,
                            "has_refresh_token": e.has_refresh_token,
                        })
                    })
                    .collect();
                success(serde_json::json!({
                    "store_path": overview.store_path,
                    "entries": entries,
                }))
            }
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_logout_oauth_token(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            server_id: String,
        }
        let p: Params = try_parse!(params);
        match self.logout_mcp_oauth_token(&p.server_id).await {
            Ok(logged_out) => success(serde_json::json!({ "logged_out": logged_out })),
            Err(e) => core_error(e),
        }
    }
}

fn default_empty_object() -> String {
    "{}".to_string()
}
