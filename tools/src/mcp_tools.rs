use crate::{
    ToolContext, ToolError, ToolOutcome, ToolRegistry,
    catalog::parse_mcp_provider_tool_name,
    permissions::{map_mcp_error, require_tools},
};

impl ToolRegistry {
    /// Invoke an MCP tool that the model addressed by its stable
    /// `mcp__{server}__{tool}` provider name. The raw tool input is passed
    /// through unchanged so JSON arguments authored by the model line up with
    /// the schema returned during discovery.
    pub(crate) async fn invoke_mcp_provider_tool(
        &self,
        name: &str,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let (server_id, tool_name) = parse_mcp_provider_tool_name(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;
        let result = tokio::select! {
            result = async {
                match context.session_id.as_deref() {
                    Some(session_id) => {
                        context
                            .mcp
                            .invoke_tool_for_session(session_id, server_id, tool_name, input)
                            .await
                    }
                    None => context.mcp.invoke_tool(server_id, tool_name, input).await,
                }
            } => result.map_err(map_mcp_error)?,
            _ = context.cancellation.cancelled() => return Err(ToolError::Interrupted),
        };
        if result.is_error {
            return Err(ToolError::ExecutionFailed(result.output));
        }
        Ok(ToolOutcome {
            name: name.to_string(),
            summary: format!("Invoked MCP tool `{tool_name}` on `{server_id}`."),
            output: result.output,
            metadata: None,
            changed_paths: Vec::new(),
        })
    }
}
