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
    let result = app_server
        .invoke_tool(name, input)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let outcome = ToolOutcome {
        name: result.name,
        summary: result.summary,
        output: result.output,
        metadata: result.metadata,
        changed_paths: result.changed_paths,
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
    let result = app_server
        .read_mcp_resource(server_id, uri)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(PersistedSystemCommandResult {
        note: format!("MCP Resource: {server_id} {uri}\n{}", result.contents),
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
    let result = app_server
        .invoke_mcp_tool(server_id, tool_name, input)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(PersistedSystemCommandResult {
        note: format!(
            "MCP Tool: {}::{}\n{}",
            result.server_id, result.tool_name, result.output
        ),
        status: format!("Ran MCP tool `{server_id}::{tool_name}`."),
    })
}
