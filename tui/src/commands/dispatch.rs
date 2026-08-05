use anyhow::Result;
use orbcode_app_server_client::{AppClient, McpPromptResult};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::commands::CommandContext;
use crate::commands::async_local::LocalCommandEnvelope;
use crate::commands::registry::command_registry;
use crate::dynamic_slash_commands::expand_prompt_body;
use crate::slash_commands::{
    SlashCommandExecution, canonicalize_slash_command_line, mcp_prompt_ref,
    slash_command_expansion_body, slash_command_invocation, workflow_slash_command_name,
};
use crate::state::TuiState;

#[derive(Debug)]
pub(crate) enum SlashCommandOutcome {
    Handled,
    PromptToSubmit(String),
    Exit,
}

impl TuiState {
    pub(crate) async fn handle_command(
        &mut self,
        app_server: &AppClient,
        line: &str,
        local_command_tx: &mpsc::UnboundedSender<LocalCommandEnvelope>,
    ) -> Result<SlashCommandOutcome> {
        let canonical_line = canonicalize_slash_command_line(line);
        let line = canonical_line.as_str();
        let Some(invocation) = slash_command_invocation(line) else {
            return Err(anyhow::anyhow!("unknown slash command"));
        };

        if let SlashCommandExecution::PromptExpansion(id) = invocation.spec.execution {
            let Some(body) = slash_command_expansion_body(id) else {
                return Err(anyhow::anyhow!(
                    "prompt expansion body missing for /{}",
                    invocation.spec.name
                ));
            };
            let expanded = expand_prompt_body(&body, invocation.args);
            self.push_local_slash_command_output(
                line,
                format!("Submitting /{}", invocation.spec.name),
                None,
            );
            return Ok(SlashCommandOutcome::PromptToSubmit(expanded));
        }

        if let SlashCommandExecution::McpPromptExpansion(id) = invocation.spec.execution {
            let prompt_ref = mcp_prompt_ref(id).ok_or_else(|| {
                anyhow::anyhow!("MCP prompt ref missing for /{}", invocation.spec.name)
            })?;
            let arguments = if invocation.args.is_empty() {
                serde_json::Value::Object(serde_json::Map::new())
            } else {
                let mut map = serde_json::Map::new();
                map.insert(
                    "arguments".to_string(),
                    serde_json::Value::String(invocation.args.to_string()),
                );
                serde_json::Value::Object(map)
            };
            let result = app_server
                .get_mcp_prompt(prompt_ref.server_id, prompt_ref.prompt_name, arguments)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let expanded = assemble_mcp_prompt_result(&result);
            self.push_local_slash_command_output(
                line,
                format!("Submitting /{}", invocation.spec.name),
                None,
            );
            return Ok(SlashCommandOutcome::PromptToSubmit(expanded));
        }

        if let SlashCommandExecution::Workflow(id) = invocation.spec.execution {
            let workflow_name = workflow_slash_command_name(id).ok_or_else(|| {
                anyhow::anyhow!("workflow name missing for /{}", invocation.spec.name)
            })?;
            let task_id = app_server
                .start_workflow(&self.session_id, &workflow_name, invocation.args)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .task_id;
            let detail = format!(
                "Task ID: {task_id}\n\nUse TaskOutput with this task_id to read the result, or TaskStop to cancel it."
            );
            self.push_local_slash_command_output(
                line,
                format!("Started workflow task {task_id}."),
                Some(detail),
            );
            if let Ok(task) = app_server.background_job_detail(&task_id).await {
                self.transcript_task_cards
                    .apply_pushed_view(task, Instant::now());
            }
            self.set_status_line(format!("Workflow started: {task_id}"));
            return Ok(SlashCommandOutcome::Handled);
        }

        let registry = command_registry();
        let Some(command) = registry.lookup(invocation.spec.name) else {
            return Err(anyhow::anyhow!("unknown slash command"));
        };
        let ctx = CommandContext {
            state: self,
            app_server,
            line,
            args: invocation.args,
            local_command_tx,
        };
        command.execute(ctx).await
    }
}

fn assemble_mcp_prompt_result(result: &McpPromptResult) -> String {
    let mut parts = Vec::new();
    if !result.description.is_empty() {
        parts.push(result.description.clone());
    }
    for message in &result.messages {
        if let Some(text) = &message.content.text {
            parts.push(text.clone());
        }
    }
    parts.join("\n\n")
}
