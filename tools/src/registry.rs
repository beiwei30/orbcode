use crate::catalog::{
    canonical_tool_name, parse_mcp_provider_tool_name, parse_plugin_provider_tool_name,
};
use crate::permissions::ensure_not_cancelled;
use crate::types::PluginDispatchError;
use crate::{ToolCapability, ToolContext, ToolError, ToolOutcome, ToolRegistry};
use orbcode_config::{ToolPathBoundary, tool_path_boundary};
use orbcode_protocol::SandboxMode;

impl ToolRegistry {
    pub async fn invoke(
        &self,
        name: &str,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        ensure_not_cancelled(context)?;
        let canonical = canonical_tool_name(name);
        if let Some(spec) = self.spec(canonical)
            && matches!(
                spec.capability,
                ToolCapability::WorkspaceRead | ToolCapability::WorkspaceWrite
            )
            && context.sandbox_mode.is_restrictive()
        {
            if context.sandbox_mode == SandboxMode::ReadOnly
                && spec.capability == ToolCapability::WorkspaceWrite
            {
                return Err(ToolError::PermissionDenied);
            }
            if !matches!(
                tool_path_boundary(
                    &context.cwd,
                    &context.additional_directories,
                    canonical,
                    input,
                ),
                ToolPathBoundary::InsideAllowedRoots
            ) {
                return Err(ToolError::PermissionDenied);
            }
        }
        if self.spec(canonical).is_none() && parse_mcp_provider_tool_name(name).is_some() {
            let result = self.invoke_mcp_provider_tool(name, input, context).await;
            if result.is_ok() {
                ensure_not_cancelled(context)?;
            }
            return result;
        }
        if let Some((plugin, tool)) = parse_plugin_provider_tool_name(name) {
            return Err(self.plugin_dispatch_error(plugin, tool));
        }
        let result = match canonical {
            "Agent" => Err(ToolError::ExecutionFailed(
                "Agent must be executed from an interactive session".into(),
            )),
            "workflow" => Err(ToolError::ExecutionFailed(
                "Workflow must be executed from an interactive session".into(),
            )),
            "bash" => self.run_bash(input, context).await,
            "file-read" => self.file_read(input, context).await,
            "file-edit" => self.file_edit(input, context).await,
            "file-write" => self.file_write(input, context).await,
            "glob" => self.glob(input, context).await,
            "grep" => self.grep(input, context).await,
            "notebook-edit" => self.notebook_edit(input, context).await,
            "web-fetch" => self.web_fetch(input, context).await,
            "web-search" => self.web_search(input, context).await,
            "ask-user-question" => self.ask_user_question(input, context).await,
            "todo-write" => self.todo_write(input, context).await,
            "task-create" => self.task_create(input, context).await,
            "task-get" => self.task_get(input, context).await,
            "task-list" => self.task_list(context).await,
            "task-update" => self.task_update(input, context).await,
            "task-output" => self.task_output(input, context).await,
            "task-stop" => self.task_stop(input, context).await,
            "enter-plan-mode" => self.enter_plan_mode(context).await,
            "exit-plan-mode" => self.exit_plan_mode(input, context).await,
            "verify-plan-execution" => self.verify_plan_execution(context).await,
            "skill" => self.skill(input, context).await,
            "tool-search" => self.tool_search(input, context).await,
            "lsp" => self.lsp(input, context).await,
            _ => Err(ToolError::NotFound(name.to_string())),
        };
        if result.is_ok() {
            ensure_not_cancelled(context)?;
        }
        result
    }

    fn plugin_dispatch_error(&self, plugin: &str, tool: &str) -> ToolError {
        let full_name = crate::catalog::plugin_provider_tool_name(plugin, tool);

        if self.is_feature_disabled(&full_name) {
            return PluginDispatchError::PluginDisabled {
                plugin: plugin.to_string(),
                tool: tool.to_string(),
            }
            .into();
        }

        let dynamic = self.dynamic_definitions();
        let registered = dynamic.iter().any(|d| d.name == full_name);
        if registered {
            return PluginDispatchError::UnsupportedRuntime {
                plugin: plugin.to_string(),
                tool: tool.to_string(),
            }
            .into();
        }

        let plugin_prefix = format!("plugin__{plugin}__");
        let plugin_has_other_tools = dynamic.iter().any(|d| d.name.starts_with(&plugin_prefix));
        if plugin_has_other_tools {
            return PluginDispatchError::ToolNotFound {
                plugin: plugin.to_string(),
                tool: tool.to_string(),
            }
            .into();
        }

        PluginDispatchError::PluginNotInstalled {
            plugin: plugin.to_string(),
            tool: tool.to_string(),
        }
        .into()
    }
}
