use serde_json::{Value, json};

use crate::cancel::McpCancellationToken;
use crate::error::McpError;
use crate::registry::{
    McpRegistry, ensure_server_trusted, is_real_http_server, is_real_stdio_server,
    is_real_websocket_server, server_visible_to_session,
};
use crate::store::StoredMcpServer;
use crate::transport::websocket_client;
use crate::types::{
    McpServerStatus, McpServerTrust, McpToolDescriptor, McpToolResult, McpToolSpec,
    TrustApprovalRequest, TrustApprovalResponse,
};
use crate::wire::{StdioContentBlock, StdioToolSpec};

impl McpRegistry {
    pub async fn list_tools(&self, server_id: &str) -> Result<Vec<McpToolSpec>, McpError> {
        self.list_tools_visible_to(server_id, None).await
    }

    pub async fn list_tools_for_session(
        &self,
        session_id: &str,
        server_id: &str,
    ) -> Result<Vec<McpToolSpec>, McpError> {
        self.list_tools_visible_to(server_id, Some(session_id))
            .await
    }

    async fn list_tools_visible_to(
        &self,
        server_id: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<McpToolSpec>, McpError> {
        let server = match session_id {
            Some(session_id) => {
                self.server_snapshot_for_session(session_id, server_id)
                    .await?
            }
            None => self.server_snapshot(server_id).await?,
        };
        ensure_server_trusted(server_id, &server)?;
        if is_real_stdio_server(&server.config) {
            self.finish_probe(server_id, self.list_stdio_tools(&server.config).await)
                .await
        } else if is_real_http_server(&server.config) {
            self.finish_probe(server_id, self.list_http_tools(&server.config).await)
                .await
        } else if is_real_websocket_server(&server.config) {
            self.finish_probe(server_id, self.list_websocket_tools(&server.config).await)
                .await
        } else if !server.config.enabled {
            self.shutdown_stdio_client(server_id).await;
            self.set_server_status(server_id, McpServerStatus::Disabled, None)
                .await;
            Err(McpError::DisabledServer(server.config.id))
        } else {
            Ok(server.tools)
        }
    }

    pub async fn invoke_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        input: &str,
    ) -> Result<McpToolResult, McpError> {
        self.invoke_tool_cancellable(server_id, tool_name, input, None)
            .await
    }

    pub async fn invoke_tool_for_session(
        &self,
        session_id: &str,
        server_id: &str,
        tool_name: &str,
        input: &str,
    ) -> Result<McpToolResult, McpError> {
        self.invoke_tool_cancellable_visible_to(server_id, tool_name, input, None, Some(session_id))
            .await
    }

    pub async fn invoke_tool_cancellable(
        &self,
        server_id: &str,
        tool_name: &str,
        input: &str,
        cancel: Option<McpCancellationToken>,
    ) -> Result<McpToolResult, McpError> {
        self.invoke_tool_cancellable_visible_to(server_id, tool_name, input, cancel, None)
            .await
    }

    async fn invoke_tool_cancellable_visible_to(
        &self,
        server_id: &str,
        tool_name: &str,
        input: &str,
        cancel: Option<McpCancellationToken>,
        session_id: Option<&str>,
    ) -> Result<McpToolResult, McpError> {
        let server = match session_id {
            Some(session_id) => {
                self.server_snapshot_for_session(session_id, server_id)
                    .await?
            }
            None => self.server_snapshot(server_id).await?,
        };
        if let Err(err) = ensure_server_trusted(server_id, &server) {
            if server.config.trust == McpServerTrust::Unknown
                && let Some(response) = self
                    .request_trust_approval(server_id, tool_name, &server)
                    .await
            {
                match response {
                    TrustApprovalResponse::Trusted => {
                        let _ = match session_id {
                            Some(session_id) => {
                                self.set_server_trust_for_session(
                                    session_id,
                                    server_id,
                                    McpServerTrust::Trusted,
                                )
                                .await
                            }
                            None => {
                                self.set_server_trust(server_id, McpServerTrust::Trusted)
                                    .await
                            }
                        };
                        return self
                            .invoke_tool_inner_visible_to(
                                server_id, tool_name, input, &cancel, session_id,
                            )
                            .await;
                    }
                    TrustApprovalResponse::Denied => {
                        let _ = match session_id {
                            Some(session_id) => {
                                self.set_server_trust_for_session(
                                    session_id,
                                    server_id,
                                    McpServerTrust::Denied,
                                )
                                .await
                            }
                            None => {
                                self.set_server_trust(server_id, McpServerTrust::Denied)
                                    .await
                            }
                        };
                        return Err(McpError::ServerUntrusted {
                            server: server_id.to_string(),
                            status: McpServerTrust::Denied.as_str(),
                        });
                    }
                }
            }
            return Err(err);
        }
        self.invoke_tool_trusted(server_id, tool_name, input, server, &cancel)
            .await
    }

    async fn invoke_tool_inner_visible_to(
        &self,
        server_id: &str,
        tool_name: &str,
        input: &str,
        cancel: &Option<McpCancellationToken>,
        session_id: Option<&str>,
    ) -> Result<McpToolResult, McpError> {
        let server = match session_id {
            Some(session_id) => {
                self.server_snapshot_for_session(session_id, server_id)
                    .await?
            }
            None => self.server_snapshot(server_id).await?,
        };
        self.invoke_tool_trusted(server_id, tool_name, input, server, cancel)
            .await
    }

    async fn invoke_tool_trusted(
        &self,
        server_id: &str,
        tool_name: &str,
        input: &str,
        server: StoredMcpServer,
        cancel: &Option<McpCancellationToken>,
    ) -> Result<McpToolResult, McpError> {
        let (output, is_error) = if is_real_stdio_server(&server.config) {
            self.finish_probe(
                server_id,
                self.invoke_stdio_tool(&server.config, tool_name, input, cancel)
                    .await,
            )
            .await?
        } else if is_real_http_server(&server.config) {
            self.finish_probe(
                server_id,
                self.invoke_http_tool(&server.config, tool_name, input, cancel)
                    .await,
            )
            .await?
        } else if is_real_websocket_server(&server.config) {
            self.finish_probe(
                server_id,
                self.invoke_websocket_tool(&server.config, tool_name, input, cancel)
                    .await,
            )
            .await?
        } else if !server.config.enabled {
            self.shutdown_stdio_client(server_id).await;
            self.set_server_status(server_id, McpServerStatus::Disabled, None)
                .await;
            return Err(McpError::DisabledServer(server.config.id));
        } else {
            (invoke_stored_tool(server, tool_name, input)?, false)
        };

        Ok(McpToolResult {
            server_id: server_id.to_string(),
            tool_name: tool_name.to_string(),
            output,
            is_error,
        })
    }

    async fn request_trust_approval(
        &self,
        server_id: &str,
        tool_name: &str,
        server: &StoredMcpServer,
    ) -> Option<TrustApprovalResponse> {
        let handler = {
            let guard = self.trust_approval_handler.lock().await;
            guard.clone()?
        };
        let request = TrustApprovalRequest {
            request_id: generate_request_id(),
            server_id: server_id.to_string(),
            tool_name: tool_name.to_string(),
            server_source: server.config.source.clone(),
        };
        handler.request_trust_approval(request).await
    }

    /// Enumerate MCP tools that should be exposed to the model as first-class
    /// provider tools.
    pub async fn list_provider_tools(&self) -> Vec<McpToolDescriptor> {
        self.list_provider_tools_visible_to(None).await
    }

    pub async fn list_provider_tools_for_session(
        &self,
        session_id: &str,
    ) -> Vec<McpToolDescriptor> {
        self.list_provider_tools_visible_to(Some(session_id)).await
    }

    async fn list_provider_tools_visible_to(
        &self,
        session_id: Option<&str>,
    ) -> Vec<McpToolDescriptor> {
        let servers = {
            let state = self.state.lock().await;
            state
                .store
                .servers
                .iter()
                .filter(|server| server_visible_to_session(&state, &server.config.id, session_id))
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut descriptors = Vec::new();
        for server in servers {
            if !server.config.enabled {
                continue;
            }
            if !matches!(server.config.trust, McpServerTrust::Trusted) {
                continue;
            }
            if !is_real_stdio_server(&server.config)
                && !is_real_http_server(&server.config)
                && !is_real_websocket_server(&server.config)
            {
                for tool in server.tools {
                    descriptors.push(McpToolDescriptor {
                        server_id: server.config.id.clone(),
                        tool_name: tool.name,
                        description: tool.summary,
                        input_schema: json!({ "type": "object" }),
                        source: server.config.source.clone(),
                    });
                }
                continue;
            }
            let result = if is_real_stdio_server(&server.config) {
                self.list_stdio_tools_full(&server.config).await
            } else if is_real_http_server(&server.config) {
                self.list_http_tools_full(&server.config).await
            } else {
                self.list_websocket_tools_full(&server.config).await
            };
            match result {
                Ok(tools) => {
                    self.set_server_status(&server.config.id, McpServerStatus::Ready, None)
                        .await;
                    for tool in tools {
                        descriptors.push(McpToolDescriptor {
                            server_id: server.config.id.clone(),
                            tool_name: tool.name,
                            description: tool.description,
                            input_schema: tool.input_schema,
                            source: server.config.source.clone(),
                        });
                    }
                }
                Err(error) => {
                    let status = if matches!(error, McpError::AuthRequired { .. }) {
                        McpServerStatus::Unauthorized
                    } else {
                        McpServerStatus::Failed
                    };
                    self.set_server_status(&server.config.id, status, Some(error.to_string()))
                        .await;
                }
            }
        }
        descriptors
    }

    async fn list_stdio_tools(
        &self,
        config: &crate::types::McpServerConfig,
    ) -> Result<Vec<McpToolSpec>, McpError> {
        Ok(self
            .list_stdio_tools_full(config)
            .await?
            .into_iter()
            .map(|tool| McpToolSpec {
                name: tool.name,
                summary: tool.description,
            })
            .collect())
    }

    async fn list_stdio_tools_full(
        &self,
        config: &crate::types::McpServerConfig,
    ) -> Result<Vec<StdioToolSpec>, McpError> {
        let result = self.stdio_list_tools_once(config).await;
        match result {
            Ok(tools) => Ok(tools),
            Err(error) if self.should_restart(&config.id, &error).await => {
                self.restart_stdio_client(config, error).await?;
                self.stdio_list_tools_once(config).await
            }
            Err(error) => Err(error),
        }
    }

    async fn stdio_list_tools_once(
        &self,
        config: &crate::types::McpServerConfig,
    ) -> Result<Vec<StdioToolSpec>, McpError> {
        let slot = self.stdio_client(config).await?;
        let (_permit, mut client) = slot.acquire().await?;
        let result = client.list_tools().await;
        slot.return_client(client);
        Ok(result?.tools)
    }

    async fn stdio_call_tool_once(
        &self,
        config: &crate::types::McpServerConfig,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<crate::wire::StdioToolCallResult, McpError> {
        let slot = self.stdio_client(config).await?;
        let (_permit, mut client) = slot.acquire().await?;
        let result = client.call_tool(tool_name, arguments.clone()).await;
        slot.return_client(client);
        result
    }

    async fn list_http_tools(
        &self,
        config: &crate::types::McpServerConfig,
    ) -> Result<Vec<McpToolSpec>, McpError> {
        Ok(self
            .list_http_tools_full(config)
            .await?
            .into_iter()
            .map(|tool| McpToolSpec {
                name: tool.name,
                summary: tool.description,
            })
            .collect())
    }

    async fn list_http_tools_full(
        &self,
        config: &crate::types::McpServerConfig,
    ) -> Result<Vec<StdioToolSpec>, McpError> {
        Ok(self
            .http_rpc::<crate::wire::StdioListToolsResult>(config, "tools/list", json!({}))
            .await?
            .tools)
    }

    async fn list_websocket_tools(
        &self,
        config: &crate::types::McpServerConfig,
    ) -> Result<Vec<McpToolSpec>, McpError> {
        Ok(self
            .list_websocket_tools_full(config)
            .await?
            .into_iter()
            .map(|tool| McpToolSpec {
                name: tool.name,
                summary: tool.description,
            })
            .collect())
    }

    async fn list_websocket_tools_full(
        &self,
        config: &crate::types::McpServerConfig,
    ) -> Result<Vec<StdioToolSpec>, McpError> {
        let access_token = self.mcp_oauth_access_token(&config.id).await?;
        let mut client = websocket_client(config, access_token.as_deref()).await?;
        client.initialize().await?;
        Ok(client.list_tools().await?.tools)
    }

    async fn invoke_stdio_tool(
        &self,
        config: &crate::types::McpServerConfig,
        tool_name: &str,
        input: &str,
        cancel: &Option<McpCancellationToken>,
    ) -> Result<(String, bool), McpError> {
        let arguments = parse_stdio_tool_arguments(input)?;
        let rpc_fut = async {
            let result = self
                .stdio_call_tool_once(config, tool_name, &arguments)
                .await;
            match result {
                Ok(result) => Ok(result),
                Err(error) if self.should_restart(&config.id, &error).await => {
                    self.restart_stdio_client(config, error).await?;
                    self.stdio_call_tool_once(config, tool_name, &arguments)
                        .await
                }
                Err(error) => Err(error),
            }
        };
        let result = if let Some(token) = cancel {
            tokio::select! {
                biased;
                _ = token.cancelled() => return Err(McpError::Cancelled),
                result = rpc_fut => result?,
            }
        } else {
            rpc_fut.await?
        };
        let output = result
            .content
            .into_iter()
            .map(render_stdio_content_block)
            .collect::<Vec<_>>()
            .join("\n");
        Ok((output, result.is_error))
    }

    async fn invoke_http_tool(
        &self,
        config: &crate::types::McpServerConfig,
        tool_name: &str,
        input: &str,
        cancel: &Option<McpCancellationToken>,
    ) -> Result<(String, bool), McpError> {
        let arguments = parse_stdio_tool_arguments(input)?;
        let rpc_fut = async {
            self.http_rpc::<crate::wire::StdioToolCallResult>(
                config,
                "tools/call",
                json!({
                    "name": tool_name,
                    "arguments": arguments,
                }),
            )
            .await
        };
        let result = if let Some(token) = cancel {
            tokio::select! {
                biased;
                _ = token.cancelled() => return Err(McpError::Cancelled),
                result = rpc_fut => result?,
            }
        } else {
            rpc_fut.await?
        };
        let output = result
            .content
            .into_iter()
            .map(render_stdio_content_block)
            .collect::<Vec<_>>()
            .join("\n");
        Ok((output, result.is_error))
    }

    async fn invoke_websocket_tool(
        &self,
        config: &crate::types::McpServerConfig,
        tool_name: &str,
        input: &str,
        cancel: &Option<McpCancellationToken>,
    ) -> Result<(String, bool), McpError> {
        let arguments = parse_stdio_tool_arguments(input)?;
        let rpc_fut = async {
            let access_token = self.mcp_oauth_access_token(&config.id).await?;
            let mut client = websocket_client(config, access_token.as_deref()).await?;
            client.initialize().await?;
            client.call_tool(tool_name, arguments).await
        };
        let result = if let Some(token) = cancel {
            tokio::select! {
                biased;
                _ = token.cancelled() => return Err(McpError::Cancelled),
                result = rpc_fut => result?,
            }
        } else {
            rpc_fut.await?
        };
        let output = result
            .content
            .into_iter()
            .map(render_stdio_content_block)
            .collect::<Vec<_>>()
            .join("\n");
        Ok((output, result.is_error))
    }
}

fn parse_stdio_tool_arguments(input: &str) -> Result<Value, McpError> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_str(input) {
        Ok(value) => Ok(value),
        Err(_) => Ok(json!({
            "input": input,
            "text": input,
        })),
    }
}

fn render_stdio_content_block(block: StdioContentBlock) -> String {
    if block.kind == "text" {
        return block.text.unwrap_or_default();
    }
    let mut value = serde_json::Map::new();
    value.insert("type".to_string(), Value::String(block.kind));
    if let Some(text) = block.text {
        value.insert("text".to_string(), Value::String(text));
    }
    value.extend(block.extra);
    Value::Object(value).to_string()
}

fn invoke_stored_tool(
    server: StoredMcpServer,
    tool_name: &str,
    input: &str,
) -> Result<String, McpError> {
    match tool_name {
        "echo" => Ok(format!(
            "server={} transport={} endpoint={} input={}",
            server.config.id,
            server.config.transport,
            server.config.endpoint,
            input.trim()
        )),
        "inspect" => Ok(format!(
            "server={} summary={} auth={} resources={} tools={}",
            server.config.id,
            server.config.summary,
            server.config.auth.summary(),
            server.resources.len(),
            server.tools.len()
        )),
        _ => Err(McpError::UnknownTool(tool_name.to_string())),
    }
}

fn generate_request_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let a = RandomState::new().build_hasher().finish();
    let b = RandomState::new().build_hasher().finish();
    format!("{a:016x}{b:016x}")
}
