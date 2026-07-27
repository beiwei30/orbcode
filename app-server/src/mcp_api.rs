use orbcode_app_server_protocol::{
    McpResourceSlashSuggestion, McpServerSlashSuggestion, McpSlashSuggestionCatalog,
    McpToolSlashSuggestion,
};
use orbcode_core::{CoreError, mcp_permission_target};
use orbcode_tools::mcp_provider_tool_name;
use serde_json::json;

use super::AppServer;

impl AppServer {
    pub async fn mcp_capabilities(&self) -> Vec<orbcode_mcp::McpCapability> {
        self.mcp.capabilities().await
    }

    pub async fn mcp_slash_suggestions(&self) -> McpSlashSuggestionCatalog {
        self.mcp_slash_suggestions_visible_to(None).await
    }

    pub async fn mcp_slash_suggestions_for_session(
        &self,
        session_id: &str,
    ) -> McpSlashSuggestionCatalog {
        self.mcp_slash_suggestions_visible_to(Some(session_id))
            .await
    }

    async fn mcp_slash_suggestions_visible_to(
        &self,
        session_id: Option<&str>,
    ) -> McpSlashSuggestionCatalog {
        let servers = match session_id {
            Some(session_id) => self.mcp.list_servers_for_session(session_id).await,
            None => self.mcp.list_servers().await,
        }
        .into_iter()
        .filter(|server| server.enabled && server.trust.is_trusted())
        .collect::<Vec<_>>();
        let mut catalog = McpSlashSuggestionCatalog {
            servers: servers
                .iter()
                .map(|server| McpServerSlashSuggestion {
                    id: server.id.clone(),
                    summary: server.summary.clone(),
                })
                .collect(),
            ..McpSlashSuggestionCatalog::default()
        };

        for server in servers {
            let tools = match session_id {
                Some(session_id) => {
                    self.mcp
                        .list_tools_for_session(session_id, &server.id)
                        .await
                }
                None => self.mcp.list_tools(&server.id).await,
            };
            if let Ok(tools) = tools {
                catalog
                    .tools
                    .extend(tools.into_iter().map(|tool| McpToolSlashSuggestion {
                        provider_name: mcp_provider_tool_name(&server.id, &tool.name),
                        server_id: server.id.clone(),
                        name: tool.name,
                        description: tool.summary,
                    }));
            }
            let resources = match session_id {
                Some(session_id) => {
                    self.mcp
                        .list_resources_for_session(session_id, &server.id)
                        .await
                }
                None => self.mcp.list_resources(&server.id).await,
            };
            if let Ok(resources) = resources {
                catalog
                    .resources
                    .extend(
                        resources
                            .into_iter()
                            .map(|resource| McpResourceSlashSuggestion {
                                server_id: server.id.clone(),
                                uri: resource.uri,
                                name: resource.name,
                                description: resource.description,
                            }),
                    );
            }
        }

        catalog
    }

    pub async fn list_mcp_servers(&self) -> Vec<orbcode_mcp::McpServerConfig> {
        self.mcp.list_servers().await
    }

    pub async fn diagnose_mcp_server(
        &self,
        server_id: &str,
    ) -> Result<Vec<orbcode_mcp::McpDiagnosticCheck>, CoreError> {
        self.mcp
            .diagnose_server(server_id)
            .await
            .map_err(CoreError::from)
    }

    pub async fn mcp_oauth_overview(
        &self,
        server_id: Option<&str>,
    ) -> Result<orbcode_mcp::McpOAuthOverview, CoreError> {
        self.mcp
            .mcp_oauth_overview(server_id)
            .await
            .map_err(CoreError::from)
    }

    pub async fn store_mcp_oauth_token(
        &self,
        server_id: &str,
        input: orbcode_mcp::McpOAuthTokenInput,
    ) -> Result<orbcode_mcp::McpOAuthStatusEntry, CoreError> {
        self.mcp
            .store_mcp_oauth_token(server_id, input)
            .await
            .map_err(CoreError::from)
    }

    pub async fn start_mcp_oauth_device_login(
        &self,
        server_id: &str,
        input: orbcode_mcp::McpOAuthDeviceLoginInput,
    ) -> Result<orbcode_mcp::McpOAuthDeviceLoginSession, CoreError> {
        self.mcp
            .start_mcp_oauth_device_login(server_id, input)
            .await
            .map_err(CoreError::from)
    }

    pub async fn complete_mcp_oauth_device_login(
        &self,
        session: orbcode_mcp::McpOAuthDeviceLoginSession,
    ) -> Result<orbcode_mcp::McpOAuthStatusEntry, CoreError> {
        self.mcp
            .complete_mcp_oauth_device_login(session)
            .await
            .map_err(CoreError::from)
    }

    pub async fn start_mcp_oauth_browser_login(
        &self,
        server_id: &str,
        input: orbcode_mcp::McpOAuthBrowserLoginInput,
    ) -> Result<orbcode_mcp::McpOAuthBrowserLoginSession, CoreError> {
        self.mcp
            .start_mcp_oauth_browser_login(server_id, input)
            .await
            .map_err(CoreError::from)
    }

    pub async fn complete_mcp_oauth_browser_login(
        &self,
        session: orbcode_mcp::McpOAuthBrowserLoginSession,
    ) -> Result<orbcode_mcp::McpOAuthStatusEntry, CoreError> {
        self.mcp
            .complete_mcp_oauth_browser_login(session)
            .await
            .map_err(CoreError::from)
    }

    pub async fn logout_mcp_oauth_token(&self, server_id: &str) -> Result<bool, CoreError> {
        self.mcp
            .logout_mcp_oauth_token(server_id)
            .await
            .map_err(CoreError::from)
    }

    pub async fn upsert_mcp_server(
        &self,
        config: orbcode_mcp::McpServerConfig,
    ) -> Result<(), CoreError> {
        self.mcp
            .upsert_server(config)
            .await
            .map_err(CoreError::from)
    }

    pub async fn remove_mcp_server(&self, server_id: &str) -> Result<bool, CoreError> {
        self.mcp
            .remove_server(server_id)
            .await
            .map_err(CoreError::from)
    }

    pub async fn mcp_server_trust(&self, server_id: &str) -> orbcode_mcp::McpServerTrust {
        self.mcp.server_trust(server_id).await
    }

    pub async fn set_mcp_server_trust(
        &self,
        server_id: &str,
        trust: orbcode_mcp::McpServerTrust,
    ) -> Result<(), CoreError> {
        self.mcp
            .set_server_trust(server_id, trust)
            .await
            .map_err(CoreError::from)
    }

    pub async fn set_mcp_server_trust_for_session(
        &self,
        session_id: &str,
        server_id: &str,
        trust: orbcode_mcp::McpServerTrust,
    ) -> Result<(), CoreError> {
        self.mcp
            .set_server_trust_for_session(session_id, server_id, trust)
            .await
            .map_err(CoreError::from)
    }

    pub async fn list_mcp_resources(
        &self,
        server_id: &str,
    ) -> Result<Vec<orbcode_mcp::McpResourceSummary>, CoreError> {
        self.list_mcp_resources_visible_to(server_id, None).await
    }

    pub async fn list_mcp_resources_for_session(
        &self,
        session_id: &str,
        server_id: &str,
    ) -> Result<Vec<orbcode_mcp::McpResourceSummary>, CoreError> {
        self.list_mcp_resources_visible_to(server_id, Some(session_id))
            .await
    }

    async fn list_mcp_resources_visible_to(
        &self,
        server_id: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<orbcode_mcp::McpResourceSummary>, CoreError> {
        self.ensure_mcp_command_allowed(
            "list-mcp-resources",
            server_id,
            json!({ "server_id": server_id }).to_string(),
            session_id,
        )
        .await?;
        match session_id {
            Some(session_id) => {
                self.mcp
                    .discover_resources_for_session(session_id, server_id)
                    .await
            }
            None => self.mcp.discover_resources(server_id).await,
        }
        .map_err(CoreError::from)
    }

    pub async fn list_mcp_resource_templates(
        &self,
        server_id: &str,
    ) -> Result<Vec<orbcode_mcp::McpResourceTemplate>, CoreError> {
        self.list_mcp_resource_templates_visible_to(server_id, None)
            .await
    }

    pub async fn list_mcp_resource_templates_for_session(
        &self,
        session_id: &str,
        server_id: &str,
    ) -> Result<Vec<orbcode_mcp::McpResourceTemplate>, CoreError> {
        self.list_mcp_resource_templates_visible_to(server_id, Some(session_id))
            .await
    }

    async fn list_mcp_resource_templates_visible_to(
        &self,
        server_id: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<orbcode_mcp::McpResourceTemplate>, CoreError> {
        self.ensure_mcp_command_allowed(
            "list-mcp-resources",
            server_id,
            json!({ "server_id": server_id }).to_string(),
            session_id,
        )
        .await?;
        match session_id {
            Some(session_id) => {
                self.mcp
                    .discover_resource_templates_for_session(session_id, server_id)
                    .await
            }
            None => self.mcp.discover_resource_templates(server_id).await,
        }
        .map_err(CoreError::from)
    }

    pub async fn read_mcp_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> Result<orbcode_mcp::McpResourceContent, CoreError> {
        self.read_mcp_resource_visible_to(server_id, uri, None)
            .await
    }

    pub async fn read_mcp_resource_for_session(
        &self,
        session_id: &str,
        server_id: &str,
        uri: &str,
    ) -> Result<orbcode_mcp::McpResourceContent, CoreError> {
        self.read_mcp_resource_visible_to(server_id, uri, Some(session_id))
            .await
    }

    async fn read_mcp_resource_visible_to(
        &self,
        server_id: &str,
        uri: &str,
        session_id: Option<&str>,
    ) -> Result<orbcode_mcp::McpResourceContent, CoreError> {
        self.ensure_mcp_command_allowed(
            "read-mcp-resource",
            server_id,
            json!({ "server_id": server_id, "uri": uri }).to_string(),
            session_id,
        )
        .await?;
        match session_id {
            Some(session_id) => {
                self.mcp
                    .read_resource_content_for_session(session_id, server_id, uri)
                    .await
            }
            None => self.mcp.read_resource_content(server_id, uri).await,
        }
        .map_err(CoreError::from)
    }

    pub async fn list_mcp_prompts(
        &self,
        server_id: &str,
    ) -> Result<Vec<orbcode_mcp::McpPrompt>, CoreError> {
        self.list_mcp_prompts_visible_to(server_id, None).await
    }

    pub async fn list_mcp_prompts_for_session(
        &self,
        session_id: &str,
        server_id: &str,
    ) -> Result<Vec<orbcode_mcp::McpPrompt>, CoreError> {
        self.list_mcp_prompts_visible_to(server_id, Some(session_id))
            .await
    }

    async fn list_mcp_prompts_visible_to(
        &self,
        server_id: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<orbcode_mcp::McpPrompt>, CoreError> {
        self.ensure_mcp_command_allowed(
            "list-mcp-prompts",
            server_id,
            json!({ "server_id": server_id }).to_string(),
            session_id,
        )
        .await?;
        match session_id {
            Some(session_id) => {
                self.mcp
                    .list_prompts_for_session(session_id, server_id)
                    .await
            }
            None => self.mcp.list_prompts(server_id).await,
        }
        .map_err(CoreError::from)
    }

    pub async fn get_mcp_prompt(
        &self,
        server_id: &str,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<orbcode_mcp::McpPromptResult, CoreError> {
        self.get_mcp_prompt_visible_to(server_id, name, arguments, None)
            .await
    }

    pub async fn get_mcp_prompt_for_session(
        &self,
        session_id: &str,
        server_id: &str,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<orbcode_mcp::McpPromptResult, CoreError> {
        self.get_mcp_prompt_visible_to(server_id, name, arguments, Some(session_id))
            .await
    }

    async fn get_mcp_prompt_visible_to(
        &self,
        server_id: &str,
        name: &str,
        arguments: serde_json::Value,
        session_id: Option<&str>,
    ) -> Result<orbcode_mcp::McpPromptResult, CoreError> {
        self.ensure_mcp_command_allowed(
            "get-mcp-prompt",
            server_id,
            json!({ "server_id": server_id, "name": name }).to_string(),
            session_id,
        )
        .await?;
        match session_id {
            Some(session_id) => {
                self.mcp
                    .get_prompt_for_session(session_id, server_id, name, arguments)
                    .await
            }
            None => self.mcp.get_prompt(server_id, name, arguments).await,
        }
        .map_err(CoreError::from)
    }

    pub async fn list_mcp_tools(
        &self,
        server_id: &str,
    ) -> Result<Vec<orbcode_mcp::McpToolSpec>, CoreError> {
        self.ensure_mcp_command_allowed(
            "list-mcp-tools",
            server_id,
            json!({ "server_id": server_id }).to_string(),
            None,
        )
        .await?;
        self.mcp
            .list_tools(server_id)
            .await
            .map_err(CoreError::from)
    }

    pub async fn invoke_mcp_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        input: impl Into<String>,
    ) -> Result<orbcode_mcp::McpToolResult, CoreError> {
        let input = input.into();
        self.ensure_mcp_command_allowed(
            "call-mcp-tool",
            server_id,
            json!({ "server_id": server_id, "tool_name": tool_name, "input": input.clone() })
                .to_string(),
            None,
        )
        .await?;
        self.mcp
            .invoke_tool(server_id, tool_name, &input)
            .await
            .map_err(CoreError::from)
    }

    async fn ensure_mcp_command_allowed(
        &self,
        adapter_name: &str,
        server_id: &str,
        adapter_input: String,
        session_id: Option<&str>,
    ) -> Result<(), CoreError> {
        let permissions = self.sessions.permission_context();
        let target = mcp_permission_target(adapter_name, &adapter_input)
            .unwrap_or_else(|| adapter_name.to_string());
        if permissions
            .tool_denied(adapter_name, &adapter_input)
            .is_some()
        {
            return Err(CoreError::PermissionDenied(format!(
                "permission denied for MCP target `{target}` by configured deny rule"
            )));
        }
        let trust = match session_id {
            Some(session_id) => self
                .mcp
                .server_trust_for_session(session_id, server_id)
                .await
                .unwrap_or_default(),
            None => self.mcp.server_trust(server_id).await,
        };
        match trust {
            orbcode_mcp::McpServerTrust::Denied => {
                return Err(CoreError::PermissionDenied(format!(
                    "MCP server `{server_id}` is marked denied; reset it with `orbcode mcp untrust {server_id}`"
                )));
            }
            orbcode_mcp::McpServerTrust::Unknown => {
                return Err(CoreError::PermissionDenied(format!(
                    "MCP server `{server_id}` is not trusted; approve it with `orbcode mcp trust {server_id}` before calling `{target}`"
                )));
            }
            orbcode_mcp::McpServerTrust::Trusted => {}
        }
        if permissions.allow_tools
            || permissions.tool_allowed_without_prompt(adapter_name, &adapter_input)
        {
            return Ok(());
        }
        let server_rule = target
            .split_once("__")
            .and_then(|(_, rest)| rest.split_once("__"))
            .map_or_else(
                || "mcp__<server>__*".to_string(),
                |(server, _)| format!("mcp__{server}__*"),
            );
        Err(CoreError::PermissionDenied(format!(
            "permission denied for MCP target `{target}`: tools permission is required; add an allow rule like `{target}` or `{server_rule}`"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use orbcode_config::AppConfigOverrides;
    use orbcode_mcp::McpServerSource;

    use super::super::AppServer;

    fn test_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "orbcode-app-server-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn app_server_loads_enabled_plugin_mcp_servers() {
        let home = test_path("plugin-mcp-home");
        let cwd = test_path("plugin-mcp-cwd");
        let plugin_root = home.join("plugin-cache").join("demo").join("1.0.0");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::create_dir_all(plugin_root.join(".claude-plugin"))
            .await
            .expect("plugin manifest dir");
        tokio::fs::write(
            plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .await
        .expect("plugin manifest");
        tokio::fs::write(
            plugin_root.join(".mcp.json"),
            r#"{"mcpServers":{"docs":{"type":"http","url":"https://docs.example/mcp"}}}"#,
        )
        .await
        .expect("plugin mcp");
        tokio::fs::create_dir_all(home.join("plugins"))
            .await
            .expect("plugins dir");
        tokio::fs::write(
            home.join("plugins").join("installed_plugins.json"),
            format!(
                r#"{{"version":2,"plugins":{{"demo@market":[{{"scope":"user","installPath":"{}","version":"1.0.0"}}]}}}}"#,
                plugin_root.display()
            ),
        )
        .await
        .expect("installed plugins");
        tokio::fs::write(
            home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await
        .expect("settings");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let server_id = orbcode_mcp::scoped_plugin_server_id("demo@market", "docs");
        let servers = app.list_mcp_servers().await;
        assert!(servers.iter().any(|server| {
            server.id == server_id && matches!(server.source, Some(McpServerSource::Plugin(_)))
        }));
    }
}
