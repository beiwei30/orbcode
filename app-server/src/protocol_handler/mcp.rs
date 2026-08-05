use orbcode_app_server_protocol::{
    McpCapabilitiesResult, McpCapability, McpDiagnoseResult, McpGetPromptParams,
    McpInvokeToolParams, McpListPromptsResult, McpListResourcesResult, McpListServersResult,
    McpListToolsResult, McpLogoutOAuthTokenResult, McpOAuthOverviewParams, McpReadResourceParams,
    McpRemoveServerResult, McpServerIdParams, McpServerInput, McpServerTrustResult,
    McpSessionServerParams, McpSetTrustParams, McpStatusResult, ResponseResult,
};
use serde_json::Value;

use super::{core_error, success, success_empty, try_parse};
use crate::AppServer;
use crate::protocol_conversion::{
    mcp_diagnostic_check_to_wire, mcp_oauth_overview_to_wire, mcp_prompt_result_to_wire,
    mcp_prompt_to_wire, mcp_resource_content_to_wire, mcp_resource_summary_to_wire,
    mcp_server_config_from_input, mcp_server_overview_from_config, mcp_tool_result_to_wire,
    mcp_tool_spec_to_wire, mcp_transport_to_wire, mcp_trust_from_wire, mcp_trust_to_wire,
};

impl AppServer {
    pub(super) async fn handle_mcp_list_servers(&self, _params: Option<Value>) -> ResponseResult {
        let servers = self
            .list_mcp_servers()
            .await
            .into_iter()
            .map(mcp_server_overview_from_config)
            .collect();
        success(McpListServersResult(servers))
    }

    pub(super) async fn handle_mcp_status(&self, _params: Option<Value>) -> ResponseResult {
        success(McpStatusResult(self.mcp_status().await))
    }

    pub(super) async fn handle_mcp_server_trust(&self, params: Option<Value>) -> ResponseResult {
        let p: McpServerIdParams = try_parse!(params);
        let trust = self.mcp_server_trust(&p.server_id).await;
        success(McpServerTrustResult {
            trust: mcp_trust_to_wire(trust),
        })
    }

    pub(super) async fn handle_mcp_set_trust(&self, params: Option<Value>) -> ResponseResult {
        let p: McpSetTrustParams = try_parse!(params);
        match self
            .set_mcp_server_trust(&p.server_id, mcp_trust_from_wire(p.trust))
            .await
        {
            Ok(()) => success_empty(),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_list_tools(&self, params: Option<Value>) -> ResponseResult {
        let p: McpServerIdParams = try_parse!(params);
        match self.list_mcp_tools(&p.server_id).await {
            Ok(tools) => success(McpListToolsResult(
                tools.into_iter().map(mcp_tool_spec_to_wire).collect(),
            )),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_list_resources(&self, params: Option<Value>) -> ResponseResult {
        let p: McpSessionServerParams = try_parse!(params);
        let result = match p.session_id.as_deref() {
            Some(session_id) => {
                self.list_mcp_resources_for_session(session_id, &p.server_id)
                    .await
            }
            None => self.list_mcp_resources(&p.server_id).await,
        };
        match result {
            Ok(resources) => success(McpListResourcesResult(
                resources
                    .into_iter()
                    .map(mcp_resource_summary_to_wire)
                    .collect(),
            )),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_read_resource(&self, params: Option<Value>) -> ResponseResult {
        let p: McpReadResourceParams = try_parse!(params);
        let result = match p.session_id.as_deref() {
            Some(session_id) => {
                self.read_mcp_resource_for_session(session_id, &p.server_id, &p.uri)
                    .await
            }
            None => self.read_mcp_resource(&p.server_id, &p.uri).await,
        };
        match result {
            Ok(content) => success(mcp_resource_content_to_wire(content)),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_list_prompts(&self, params: Option<Value>) -> ResponseResult {
        let p: McpSessionServerParams = try_parse!(params);
        let result = match p.session_id.as_deref() {
            Some(session_id) => {
                self.list_mcp_prompts_for_session(session_id, &p.server_id)
                    .await
            }
            None => self.list_mcp_prompts(&p.server_id).await,
        };
        match result {
            Ok(prompts) => success(McpListPromptsResult(
                prompts.into_iter().map(mcp_prompt_to_wire).collect(),
            )),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_get_prompt(&self, params: Option<Value>) -> ResponseResult {
        let p: McpGetPromptParams = try_parse!(params);
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
            Ok(result) => success(mcp_prompt_result_to_wire(result)),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_invoke_tool(&self, params: Option<Value>) -> ResponseResult {
        let p: McpInvokeToolParams = try_parse!(params);
        match self
            .invoke_mcp_tool(&p.server_id, &p.tool_name, p.input)
            .await
        {
            Ok(result) => success(mcp_tool_result_to_wire(result)),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_diagnose(&self, params: Option<Value>) -> ResponseResult {
        let p: McpServerIdParams = try_parse!(params);
        match self.diagnose_mcp_server(&p.server_id).await {
            Ok(checks) => success(McpDiagnoseResult(
                checks
                    .into_iter()
                    .map(mcp_diagnostic_check_to_wire)
                    .collect(),
            )),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_upsert_server(&self, params: Option<Value>) -> ResponseResult {
        let input: McpServerInput = try_parse!(params);
        match self
            .upsert_mcp_server(mcp_server_config_from_input(input))
            .await
        {
            Ok(()) => success_empty(),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_remove_server(&self, params: Option<Value>) -> ResponseResult {
        let p: McpServerIdParams = try_parse!(params);
        match self.remove_mcp_server(&p.server_id).await {
            Ok(removed) => success(McpRemoveServerResult { removed }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_capabilities(&self, _params: Option<Value>) -> ResponseResult {
        success(McpCapabilitiesResult(
            self.mcp_capabilities()
                .await
                .into_iter()
                .map(|capability| McpCapability {
                    transport: mcp_transport_to_wire(capability.transport),
                    enabled: capability.enabled,
                    note: capability.note,
                })
                .collect(),
        ))
    }

    pub(super) async fn handle_mcp_slash_suggestions(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        let catalog = self.mcp_slash_suggestions().await;
        success(catalog)
    }

    pub(super) async fn handle_mcp_oauth_overview(&self, params: Option<Value>) -> ResponseResult {
        let p: McpOAuthOverviewParams = try_parse!(params);
        match self.mcp_oauth_overview(p.server_id.as_deref()).await {
            Ok(overview) => success(mcp_oauth_overview_to_wire(overview)),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_mcp_logout_oauth_token(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: McpServerIdParams = try_parse!(params);
        match self.logout_mcp_oauth_token(&p.server_id).await {
            Ok(logged_out) => success(McpLogoutOAuthTokenResult { logged_out }),
            Err(e) => core_error(e),
        }
    }
}
