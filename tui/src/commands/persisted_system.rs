use anyhow::Result;
use orbcode_app_server_client::AppClient;
use orbcode_tools::ToolOutcome;

use crate::commands::utils::{render_tool_note, split_first_word};
use crate::state::TuiState;

pub(crate) struct PersistedSystemCommandResult {
    pub(crate) note: String,
    pub(crate) status: String,
}

pub(crate) async fn run_persisted_system_slash_command(
    command_name: &str,
    args: &str,
    app_server: &AppClient,
) -> Result<Option<PersistedSystemCommandResult>> {
    match command_name {
        "tool" => run_tool_slash_command(args, app_server).await.map(Some),
        "mcp" if args.starts_with("read ") => {
            run_mcp_read_slash_command(args, app_server).await.map(Some)
        }
        "mcp" if args.starts_with("call ") => {
            run_mcp_call_slash_command(args, app_server).await.map(Some)
        }
        _ => Ok(None),
    }
}

impl TuiState {
    pub(crate) async fn push_persisted_system_message(
        &mut self,
        app_server: &AppClient,
        content: String,
    ) -> Result<()> {
        app_server
            .record_system_message(&self.session_id, &content)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.push_local_system_message(content);
        Ok(())
    }
}

async fn run_tool_slash_command(
    args: &str,
    app_server: &AppClient,
) -> Result<PersistedSystemCommandResult> {
    let (name, input) =
        split_first_word(args).ok_or_else(|| anyhow::anyhow!("usage: /tool <name> [input]"))?;
    let value = app_server
        .invoke_tool(name, input)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let outcome = ToolOutcome {
        name: value["name"].as_str().unwrap_or("").to_string(),
        summary: value["summary"].as_str().unwrap_or("").to_string(),
        output: value["output"].as_str().unwrap_or("").to_string(),
        metadata: value.get("metadata").cloned(),
        changed_paths: Vec::new(),
    };
    Ok(PersistedSystemCommandResult {
        note: render_tool_note(&outcome),
        status: format!("Tool `{name}` completed."),
    })
}

async fn run_mcp_read_slash_command(
    args: &str,
    app_server: &AppClient,
) -> Result<PersistedSystemCommandResult> {
    let rest = args.trim_start_matches("read ");
    let (server_id, uri) =
        split_first_word(rest).ok_or_else(|| anyhow::anyhow!("usage: /mcp read <server> <uri>"))?;
    let value = app_server
        .read_mcp_resource(server_id, uri)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let contents = value["contents"].as_str().unwrap_or("");
    Ok(PersistedSystemCommandResult {
        note: format!("MCP Resource: {server_id} {uri}\n{contents}"),
        status: format!("Read MCP resource `{uri}`."),
    })
}

async fn run_mcp_call_slash_command(
    args: &str,
    app_server: &AppClient,
) -> Result<PersistedSystemCommandResult> {
    let rest = args.trim_start_matches("call ");
    let (server_id, remaining) = split_first_word(rest)
        .ok_or_else(|| anyhow::anyhow!("usage: /mcp call <server> <tool> [input]"))?;
    let (tool_name, input) = split_first_word(remaining)
        .ok_or_else(|| anyhow::anyhow!("usage: /mcp call <server> <tool> [input]"))?;
    let value = app_server
        .invoke_mcp_tool(server_id, tool_name, input)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let result_server_id = value["server_id"].as_str().unwrap_or("");
    let result_tool_name = value["tool_name"].as_str().unwrap_or("");
    let result_output = value["output"].as_str().unwrap_or("");
    Ok(PersistedSystemCommandResult {
        note: format!("MCP Tool: {result_server_id}::{result_tool_name}\n{result_output}"),
        status: format!("Ran MCP tool `{server_id}::{tool_name}`."),
    })
}
