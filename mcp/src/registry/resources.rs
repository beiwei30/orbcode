use serde_json::{Value, json};

use crate::error::McpError;
use crate::registry::{McpRegistry, ensure_server_trusted};
use crate::types::{
    McpPrompt, McpPromptResult, McpResourceContent, McpResourceSummary, McpResourceTemplate,
    McpServerStatus,
};
use crate::wire::{
    GetPromptResult, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    ReadResourceResult,
};

impl McpRegistry {
    pub async fn list_resources(
        &self,
        server_id: &str,
    ) -> Result<Vec<McpResourceSummary>, McpError> {
        self.list_resources_visible_to(server_id, None).await
    }

    pub async fn list_resources_for_session(
        &self,
        session_id: &str,
        server_id: &str,
    ) -> Result<Vec<McpResourceSummary>, McpError> {
        self.list_resources_visible_to(server_id, Some(session_id))
            .await
    }

    async fn list_resources_visible_to(
        &self,
        server_id: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<McpResourceSummary>, McpError> {
        let server = match session_id {
            Some(session_id) => {
                self.server_snapshot_for_session(session_id, server_id)
                    .await?
            }
            None => self.server_snapshot(server_id).await?,
        };
        ensure_server_trusted(server_id, &server)?;
        Ok(server
            .resources
            .into_iter()
            .map(|resource| McpResourceSummary {
                uri: resource.uri,
                name: resource.name,
                mime_type: resource.mime_type,
                description: resource.description,
                annotations: None,
            })
            .collect())
    }

    pub async fn read_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> Result<McpResourceContent, McpError> {
        self.read_resource_visible_to(server_id, uri, None).await
    }

    pub async fn read_resource_for_session(
        &self,
        session_id: &str,
        server_id: &str,
        uri: &str,
    ) -> Result<McpResourceContent, McpError> {
        self.read_resource_visible_to(server_id, uri, Some(session_id))
            .await
    }

    async fn read_resource_visible_to(
        &self,
        server_id: &str,
        uri: &str,
        session_id: Option<&str>,
    ) -> Result<McpResourceContent, McpError> {
        let server = match session_id {
            Some(session_id) => {
                self.server_snapshot_for_session(session_id, server_id)
                    .await?
            }
            None => self.server_snapshot(server_id).await?,
        };
        ensure_server_trusted(server_id, &server)?;
        server
            .resources
            .into_iter()
            .find(|resource| resource.uri == uri)
            .map(|resource| McpResourceContent {
                uri: resource.uri,
                mime_type: resource.mime_type,
                contents: resource.contents,
                blob: None,
                is_binary: false,
                annotations: None,
            })
            .ok_or_else(|| McpError::UnknownResource(uri.to_string()))
    }

    /// Run `resources/list` against a real transport, preserving annotations.
    /// Modeled servers fall back to their seeded resources.
    pub async fn discover_resources(
        &self,
        server_id: &str,
    ) -> Result<Vec<McpResourceSummary>, McpError> {
        self.discover_resources_visible_to(server_id, None).await
    }

    pub async fn discover_resources_for_session(
        &self,
        session_id: &str,
        server_id: &str,
    ) -> Result<Vec<McpResourceSummary>, McpError> {
        self.discover_resources_visible_to(server_id, Some(session_id))
            .await
    }

    async fn discover_resources_visible_to(
        &self,
        server_id: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<McpResourceSummary>, McpError> {
        let server = match session_id {
            Some(session_id) => {
                self.server_snapshot_for_session(session_id, server_id)
                    .await?
            }
            None => self.server_snapshot(server_id).await?,
        };
        ensure_server_trusted(server_id, &server)?;
        if let Some(result) = self
            .transport_rpc::<ListResourcesResult>(&server.config, "resources/list", json!({}))
            .await
        {
            let parsed = self.finish_probe(server_id, result).await?;
            return Ok(parsed
                .resources
                .into_iter()
                .map(McpResourceSummary::from)
                .collect());
        }
        if !server.config.enabled {
            self.set_server_status(server_id, McpServerStatus::Disabled, None)
                .await;
            return Err(McpError::DisabledServer(server.config.id));
        }
        Ok(server
            .resources
            .into_iter()
            .map(|resource| McpResourceSummary {
                uri: resource.uri,
                name: resource.name,
                mime_type: resource.mime_type,
                description: resource.description,
                annotations: None,
            })
            .collect())
    }

    /// Run `resources/templates/list` against a real transport. Modeled servers
    /// advertise no templates and return an empty list.
    pub async fn discover_resource_templates(
        &self,
        server_id: &str,
    ) -> Result<Vec<McpResourceTemplate>, McpError> {
        self.discover_resource_templates_visible_to(server_id, None)
            .await
    }

    pub async fn discover_resource_templates_for_session(
        &self,
        session_id: &str,
        server_id: &str,
    ) -> Result<Vec<McpResourceTemplate>, McpError> {
        self.discover_resource_templates_visible_to(server_id, Some(session_id))
            .await
    }

    async fn discover_resource_templates_visible_to(
        &self,
        server_id: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<McpResourceTemplate>, McpError> {
        let server = match session_id {
            Some(session_id) => {
                self.server_snapshot_for_session(session_id, server_id)
                    .await?
            }
            None => self.server_snapshot(server_id).await?,
        };
        ensure_server_trusted(server_id, &server)?;
        if let Some(result) = self
            .transport_rpc::<ListResourceTemplatesResult>(
                &server.config,
                "resources/templates/list",
                json!({}),
            )
            .await
        {
            let parsed = self.finish_probe(server_id, result).await?;
            return Ok(parsed
                .resource_templates
                .into_iter()
                .map(McpResourceTemplate::from)
                .collect());
        }
        if !server.config.enabled {
            self.set_server_status(server_id, McpServerStatus::Disabled, None)
                .await;
            return Err(McpError::DisabledServer(server.config.id));
        }
        Ok(Vec::new())
    }

    /// Read a resource's contents through a real transport, marking binary
    /// (base64 `blob`) payloads so they never enter the text path.
    pub async fn read_resource_content(
        &self,
        server_id: &str,
        uri: &str,
    ) -> Result<McpResourceContent, McpError> {
        self.read_resource_content_visible_to(server_id, uri, None)
            .await
    }

    pub async fn read_resource_content_for_session(
        &self,
        session_id: &str,
        server_id: &str,
        uri: &str,
    ) -> Result<McpResourceContent, McpError> {
        self.read_resource_content_visible_to(server_id, uri, Some(session_id))
            .await
    }

    async fn read_resource_content_visible_to(
        &self,
        server_id: &str,
        uri: &str,
        session_id: Option<&str>,
    ) -> Result<McpResourceContent, McpError> {
        let server = match session_id {
            Some(session_id) => {
                self.server_snapshot_for_session(session_id, server_id)
                    .await?
            }
            None => self.server_snapshot(server_id).await?,
        };
        ensure_server_trusted(server_id, &server)?;
        if let Some(result) = self
            .transport_rpc::<ReadResourceResult>(
                &server.config,
                "resources/read",
                json!({ "uri": uri }),
            )
            .await
        {
            let parsed = self.finish_probe(server_id, result).await?;
            return parsed
                .contents
                .into_iter()
                .next()
                .map(McpResourceContent::from)
                .ok_or_else(|| McpError::UnknownResource(uri.to_string()));
        }
        if !server.config.enabled {
            self.set_server_status(server_id, McpServerStatus::Disabled, None)
                .await;
            return Err(McpError::DisabledServer(server.config.id));
        }
        self.read_resource_visible_to(server_id, uri, session_id)
            .await
    }

    /// Run `prompts/list` against a real transport. Modeled servers expose no
    /// prompts and return an empty list.
    pub async fn list_prompts(&self, server_id: &str) -> Result<Vec<McpPrompt>, McpError> {
        self.list_prompts_visible_to(server_id, None).await
    }

    pub async fn list_prompts_for_session(
        &self,
        session_id: &str,
        server_id: &str,
    ) -> Result<Vec<McpPrompt>, McpError> {
        self.list_prompts_visible_to(server_id, Some(session_id))
            .await
    }

    async fn list_prompts_visible_to(
        &self,
        server_id: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<McpPrompt>, McpError> {
        let server = match session_id {
            Some(session_id) => {
                self.server_snapshot_for_session(session_id, server_id)
                    .await?
            }
            None => self.server_snapshot(server_id).await?,
        };
        ensure_server_trusted(server_id, &server)?;
        if let Some(result) = self
            .transport_rpc::<ListPromptsResult>(&server.config, "prompts/list", json!({}))
            .await
        {
            let parsed = self.finish_probe(server_id, result).await?;
            return Ok(parsed.prompts.into_iter().map(McpPrompt::from).collect());
        }
        if !server.config.enabled {
            self.set_server_status(server_id, McpServerStatus::Disabled, None)
                .await;
            return Err(McpError::DisabledServer(server.config.id));
        }
        Ok(Vec::new())
    }

    /// Run `prompts/get` against a real transport, preserving message content
    /// (including binary image/audio data). Prompts require a real MCP server.
    pub async fn get_prompt(
        &self,
        server_id: &str,
        name: &str,
        arguments: Value,
    ) -> Result<McpPromptResult, McpError> {
        self.get_prompt_visible_to(server_id, name, arguments, None)
            .await
    }

    pub async fn get_prompt_for_session(
        &self,
        session_id: &str,
        server_id: &str,
        name: &str,
        arguments: Value,
    ) -> Result<McpPromptResult, McpError> {
        self.get_prompt_visible_to(server_id, name, arguments, Some(session_id))
            .await
    }

    async fn get_prompt_visible_to(
        &self,
        server_id: &str,
        name: &str,
        arguments: Value,
        session_id: Option<&str>,
    ) -> Result<McpPromptResult, McpError> {
        let server = match session_id {
            Some(session_id) => {
                self.server_snapshot_for_session(session_id, server_id)
                    .await?
            }
            None => self.server_snapshot(server_id).await?,
        };
        ensure_server_trusted(server_id, &server)?;
        let params = json!({ "name": name, "arguments": arguments });
        if let Some(result) = self
            .transport_rpc::<GetPromptResult>(&server.config, "prompts/get", params)
            .await
        {
            let parsed = self.finish_probe(server_id, result).await?;
            return Ok(parsed.into());
        }
        if !server.config.enabled {
            self.set_server_status(server_id, McpServerStatus::Disabled, None)
                .await;
            return Err(McpError::DisabledServer(server.config.id));
        }
        Err(McpError::Protocol(format!(
            "server `{server_id}` does not support prompts over its modeled transport"
        )))
    }
}
