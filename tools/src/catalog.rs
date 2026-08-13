use std::sync::Arc;

use serde_json::{Value, json};

use orbcode_config::PluginToolDefinition;
use orbcode_mcp::{McpRegistry, McpResourceSummary};
use orbcode_protocol::ProviderToolDefinition;

use crate::payload::{parse_payload, string_field, usize_field_keys};
use crate::{
    InteractionToolVisibility, ToolCapability, ToolContext, ToolError, ToolOutcome, ToolRegistry,
    ToolSpec, ToolStatus,
};

impl ToolRegistry {
    pub fn foundation() -> Self {
        Self {
            planned: vec![
                tool(
                    "Agent",
                    "Launch a local synchronous subagent for delegated work.",
                    true,
                    false,
                ),
                tool(
                    "bash",
                    "Execute a shell command in the current workspace.",
                    true,
                    false,
                ),
                tool(
                    "file-read",
                    "Read file content from the workspace.",
                    true,
                    false,
                ),
                tool(
                    "file-edit",
                    "Apply an exact string replacement inside a file.",
                    true,
                    false,
                ),
                tool("file-write", "Write file content to disk.", true, false),
                tool(
                    "glob",
                    "Enumerate files that match a simple glob pattern.",
                    true,
                    false,
                ),
                tool("grep", "Search file content with ripgrep.", true, false),
                tool(
                    "notebook-edit",
                    "Replace, insert, delete, or append cells in a Jupyter notebook.",
                    true,
                    false,
                ),
                tool("web-fetch", "Fetch a URL with curl.", true, true),
                tool(
                    "web-search",
                    "Run a lightweight DuckDuckGo HTML search.",
                    true,
                    true,
                ),
                hidden_tool(
                    "ask-user-question",
                    "Collect typed answers from an interactive client during a model turn.",
                    false,
                    false,
                ),
                tool(
                    "todo-write",
                    "Persist a todo list snapshot under ORBCODE_HOME.",
                    true,
                    false,
                ),
                tool(
                    "task-create",
                    "Create a persistent task in the current workspace task list.",
                    true,
                    false,
                ),
                tool(
                    "task-get",
                    "Read a task from the current workspace task list.",
                    true,
                    false,
                ),
                tool(
                    "task-list",
                    "List tasks from the current workspace task list.",
                    true,
                    false,
                ),
                tool(
                    "task-update",
                    "Update a task in the current workspace task list.",
                    true,
                    false,
                ),
                tool(
                    "task-output",
                    "Read logs and status from a background task by ID.",
                    true,
                    false,
                ),
                tool(
                    "task-stop",
                    "Stop a running background task by ID.",
                    true,
                    false,
                ),
                tool(
                    "workflow",
                    "Start a generated dynamic workflow as a durable background task.",
                    true,
                    false,
                ),
                tool(
                    "enter-plan-mode",
                    "Enter plan mode and create a workspace plan file for exploration.",
                    false,
                    false,
                ),
                tool(
                    "exit-plan-mode",
                    "Exit plan mode and present the current workspace plan.",
                    false,
                    false,
                ),
                tool(
                    "verify-plan-execution",
                    "Capture a lightweight verification snapshot for the current workspace plan.",
                    false,
                    false,
                ),
                tool(
                    "skill",
                    "Load a project or user skill into the current conversation.",
                    false,
                    false,
                ),
                tool(
                    "tool-search",
                    "Search tool names and return matching schemas.",
                    false,
                    false,
                ),
                tool(
                    "lsp",
                    "Run heuristic code intelligence queries against the workspace.",
                    true,
                    false,
                ),
            ],
            dynamic_definitions: Arc::new(std::sync::RwLock::new(Vec::new())),
            feature_disabled_tools: Arc::new(std::sync::RwLock::new(
                std::collections::HashSet::new(),
            )),
        }
    }

    pub fn planned(&self) -> &[ToolSpec] {
        &self.planned
    }

    pub fn spec(&self, name: &str) -> Option<&ToolSpec> {
        let canonical = canonical_tool_name(name);
        self.planned.iter().find(|spec| spec.name == canonical)
    }

    /// Replace the dynamic (skill/plugin-contributed) tool definitions.
    /// These are merged into the provider tool list on each request, after
    /// foundation tools but before MCP tools.
    pub fn set_dynamic_definitions(&self, definitions: Vec<ProviderToolDefinition>) {
        let mut guard = match self.dynamic_definitions.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = definitions;
    }

    /// Convert plugin tool definitions into provider-facing definitions and
    /// store them as dynamic definitions. Existing dynamic definitions are
    /// replaced.
    pub fn set_plugin_tools(&self, tools: &[PluginToolDefinition]) {
        let defs = tools
            .iter()
            .map(|tool| ProviderToolDefinition {
                name: plugin_provider_tool_name(&tool.plugin_name, &tool.name),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
            .collect();
        self.set_dynamic_definitions(defs);
    }

    /// Read the current dynamic definitions snapshot.
    pub fn dynamic_definitions(&self) -> Vec<ProviderToolDefinition> {
        match self.dynamic_definitions.read() {
            Ok(g) => g.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Set the feature-gate disabled tool names. Tools whose
    /// provider-facing name appears in this set are excluded from provider
    /// requests.
    pub fn set_feature_disabled_tools(&self, disabled: std::collections::HashSet<String>) {
        let mut guard = match self.feature_disabled_tools.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = disabled;
    }

    /// Check whether a tool name is disabled by a feature gate.
    pub fn is_feature_disabled(&self, name: &str) -> bool {
        match self.feature_disabled_tools.read() {
            Ok(g) => g.contains(name),
            Err(poisoned) => poisoned.into_inner().contains(name),
        }
    }

    pub fn visible_specs(&self, allow_tools: bool, allow_network: bool) -> Vec<&ToolSpec> {
        self.visible_specs_for_interactions(
            allow_tools,
            allow_network,
            InteractionToolVisibility::default(),
        )
    }

    pub fn visible_specs_for_interactions(
        &self,
        allow_tools: bool,
        allow_network: bool,
        interactions: InteractionToolVisibility,
    ) -> Vec<&ToolSpec> {
        self.planned
            .iter()
            .filter(|spec| {
                !spec.provider_hidden
                    || (canonical_tool_name(spec.name) == "ask-user-question"
                        && interactions.ask_user_question)
            })
            .filter(|spec| !spec.requires_tools_permission || allow_tools)
            .filter(|spec| !spec.requires_network_permission || allow_network)
            .collect()
    }

    pub fn provider_definitions(
        &self,
        allow_tools: bool,
        allow_network: bool,
    ) -> Vec<ProviderToolDefinition> {
        self.provider_definitions_for_interactions(
            allow_tools,
            allow_network,
            InteractionToolVisibility::default(),
        )
    }

    pub fn provider_definitions_for_interactions(
        &self,
        allow_tools: bool,
        allow_network: bool,
        interactions: InteractionToolVisibility,
    ) -> Vec<ProviderToolDefinition> {
        self.visible_specs_for_interactions(allow_tools, allow_network, interactions)
            .into_iter()
            .map(|spec| ProviderToolDefinition {
                name: provider_facing_tool_name(spec.name).to_string(),
                description: provider_facing_tool_description(spec.name, spec.summary).to_string(),
                input_schema: tool_input_schema(spec.name),
            })
            .collect()
    }

    /// Return all tool definitions including provider-hidden tools and plugin
    /// dynamic definitions. Use this for diagnostic listings (e.g. `orbcode tools`)
    /// where visibility to the model is irrelevant.
    pub async fn diagnostic_definitions_with_mcp(
        &self,
        allow_tools: bool,
        allow_network: bool,
        mcp: &McpRegistry,
    ) -> Vec<ProviderToolDefinition> {
        let mut defs: Vec<ProviderToolDefinition> = self
            .planned
            .iter()
            .filter(|spec| !spec.requires_tools_permission || allow_tools)
            .filter(|spec| !spec.requires_network_permission || allow_network)
            .map(|spec| ProviderToolDefinition {
                name: provider_facing_tool_name(spec.name).to_string(),
                description: provider_facing_tool_description(spec.name, spec.summary).to_string(),
                input_schema: tool_input_schema(spec.name),
            })
            .collect();
        let mut known_names: std::collections::HashSet<String> =
            defs.iter().map(|tool| tool.name.clone()).collect();
        for dynamic in self.dynamic_definitions() {
            if known_names.insert(dynamic.name.clone()) {
                defs.push(dynamic);
            }
        }
        if allow_tools {
            for desc in mcp.list_provider_tools().await {
                let name = mcp_provider_tool_name(&desc.server_id, &desc.tool_name);
                if known_names.contains(&name) {
                    continue;
                }
                let description = if desc.description.trim().is_empty() {
                    format!(
                        "MCP tool `{}` from server `{}`.",
                        desc.tool_name, desc.server_id
                    )
                } else {
                    desc.description
                };
                let input_schema = if desc.input_schema.is_null() {
                    json!({ "type": "object" })
                } else {
                    desc.input_schema
                };
                defs.push(ProviderToolDefinition {
                    name,
                    description,
                    input_schema,
                });
            }
        }
        defs
    }

    /// Build the provider tool list and append any MCP-discovered tools as
    /// stable `mcp__{server}__{tool}` entries so the model can invoke them
    /// directly through the normal tool-use path.
    ///
    /// MCP tools always require the tools permission; if the caller has it
    /// disabled we skip the (potentially expensive) discovery call entirely.
    pub async fn provider_definitions_with_mcp(
        &self,
        allow_tools: bool,
        allow_network: bool,
        mcp: &McpRegistry,
    ) -> Vec<ProviderToolDefinition> {
        self.provider_definitions_with_mcp_visible_to(
            allow_tools,
            allow_network,
            mcp,
            None,
            InteractionToolVisibility::default(),
        )
        .await
    }

    pub async fn provider_definitions_with_mcp_for_session(
        &self,
        allow_tools: bool,
        allow_network: bool,
        mcp: &McpRegistry,
        session_id: &str,
    ) -> Vec<ProviderToolDefinition> {
        self.provider_definitions_with_mcp_visible_to(
            allow_tools,
            allow_network,
            mcp,
            Some(session_id),
            InteractionToolVisibility::default(),
        )
        .await
    }

    pub async fn provider_definitions_with_mcp_for_session_and_interactions(
        &self,
        allow_tools: bool,
        allow_network: bool,
        mcp: &McpRegistry,
        session_id: &str,
        interactions: InteractionToolVisibility,
    ) -> Vec<ProviderToolDefinition> {
        self.provider_definitions_with_mcp_visible_to(
            allow_tools,
            allow_network,
            mcp,
            Some(session_id),
            interactions,
        )
        .await
    }

    async fn provider_definitions_with_mcp_visible_to(
        &self,
        allow_tools: bool,
        allow_network: bool,
        mcp: &McpRegistry,
        session_id: Option<&str>,
        interactions: InteractionToolVisibility,
    ) -> Vec<ProviderToolDefinition> {
        let mut defs =
            self.provider_definitions_for_interactions(allow_tools, allow_network, interactions);
        if !allow_tools {
            return defs;
        }
        let mut known_names: std::collections::HashSet<String> =
            defs.iter().map(|tool| tool.name.clone()).collect();
        for dynamic in self.dynamic_definitions() {
            if dynamic.name.starts_with("plugin__") {
                continue;
            }
            let has_local_dispatch = self.spec(&dynamic.name).is_some();
            let has_mcp_dispatch = parse_mcp_provider_tool_name(&dynamic.name).is_some();
            if !has_local_dispatch && !has_mcp_dispatch {
                continue;
            }
            if known_names.insert(dynamic.name.clone()) {
                defs.push(dynamic);
            }
        }
        let mcp_tools = match session_id {
            Some(session_id) => mcp.list_provider_tools_for_session(session_id).await,
            None => mcp.list_provider_tools().await,
        };
        for desc in mcp_tools {
            let name = mcp_provider_tool_name(&desc.server_id, &desc.tool_name);
            if known_names.contains(&name) {
                continue;
            }
            let description = if desc.description.trim().is_empty() {
                format!(
                    "MCP tool `{}` from server `{}`.",
                    desc.tool_name, desc.server_id
                )
            } else {
                desc.description
            };
            let input_schema = if desc.input_schema.is_null() {
                json!({ "type": "object" })
            } else {
                desc.input_schema
            };
            defs.push(ProviderToolDefinition {
                name,
                description,
                input_schema,
            });
        }
        defs.retain(|tool| !self.is_feature_disabled(&tool.name));
        defs
    }

    pub(crate) async fn tool_search(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let payload = parse_payload(input)?;
        let query = string_field(&payload, "query")
            .or_else(|| payload.as_str().map(str::to_string))
            .ok_or_else(|| ToolError::InvalidInput("tool-search requires `query`".into()))?;
        let max_results = usize_field_keys(&payload, &["max_results"])
            .unwrap_or(5)
            .max(1);
        let matches = search_tool_specs(&self.planned, &query, max_results);
        let mut functions = matches
            .iter()
            .map(|spec| {
                json!({
                    "name": provider_facing_tool_name(spec.name),
                    "description": provider_facing_tool_description(spec.name, spec.summary),
                    "parameters": tool_input_schema(spec.name),
                })
            })
            .collect::<Vec<_>>();
        let foundation_names = functions
            .iter()
            .filter_map(|function| function.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<std::collections::HashSet<_>>();
        let mut mcp_resources = Vec::new();
        if context.allow_tools {
            let remaining = max_results.saturating_sub(functions.len());
            if remaining > 0 {
                for desc in search_mcp_provider_tools(
                    &context.mcp,
                    context.session_id.as_deref(),
                    &query,
                    remaining,
                )
                .await
                {
                    let name = mcp_provider_tool_name(&desc.server_id, &desc.tool_name);
                    if foundation_names.contains(&name) {
                        continue;
                    }
                    functions.push(json!({
                        "name": name,
                        "description": if desc.description.trim().is_empty() {
                            format!("MCP tool `{}` from server `{}`.", desc.tool_name, desc.server_id)
                        } else {
                            desc.description
                        },
                        "parameters": if desc.input_schema.is_null() {
                            json!({ "type": "object" })
                        } else {
                            desc.input_schema
                        },
                        "source": "mcp",
                        "server_id": desc.server_id,
                        "tool_name": desc.tool_name,
                    }));
                }
            }
            mcp_resources = search_mcp_resources(
                &context.mcp,
                context.session_id.as_deref(),
                &query,
                max_results,
            )
            .await;
        }
        Ok(ToolOutcome {
            name: "tool-search".to_string(),
            summary: format!(
                "Found {} matching tool(s) and {} MCP resource(s).",
                functions.len(),
                mcp_resources.len()
            ),
            output: serde_json::to_string_pretty(&json!({
                "matches": functions.iter().map(|item| item.get("name").and_then(Value::as_str).unwrap_or("")).collect::<Vec<_>>(),
                "query": query,
                "total_tools": self.planned.len() + functions.iter().filter(|item| item.get("source").and_then(Value::as_str) == Some("mcp")).count(),
                "functions": functions,
                "mcp_resources": mcp_resources,
            }))?,
            metadata: None,
            changed_paths: Vec::new(),
        })
    }
}

pub fn mcp_provider_tool_name(server_id: &str, tool_name: &str) -> String {
    format!("mcp__{server_id}__{tool_name}")
}

pub fn plugin_provider_tool_name(plugin_name: &str, tool_name: &str) -> String {
    format!("plugin__{plugin_name}__{tool_name}")
}

pub fn parse_plugin_provider_tool_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("plugin__")?;
    let separator = rest.find("__")?;
    let plugin = &rest[..separator];
    let tool = &rest[separator + 2..];
    if plugin.is_empty() || tool.is_empty() {
        return None;
    }
    Some((plugin, tool))
}

/// Split `mcp__{server}__{tool}` into its server id and tool name halves.
/// Returns `None` for names that do not start with the `mcp__` prefix or that
/// lack a separator after the server segment.
pub fn parse_mcp_provider_tool_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    let separator = rest.find("__")?;
    let server = &rest[..separator];
    let tool = &rest[separator + 2..];
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}

pub(crate) fn canonical_tool_name(name: &str) -> &str {
    match name {
        "Agent" | "agent" | "Task" | "task" => "Agent",
        "bash" | "Bash" => "bash",
        "file-read" | "Read" | "read" => "file-read",
        "file-edit" | "Edit" | "edit" => "file-edit",
        "file-write" | "Write" | "write" => "file-write",
        "glob" | "Glob" => "glob",
        "grep" | "Grep" => "grep",
        "notebook-edit" | "NotebookEdit" => "notebook-edit",
        "web-fetch" | "WebFetch" => "web-fetch",
        "web-search" | "WebSearch" => "web-search",
        "ask-user-question" | "AskUserQuestion" => "ask-user-question",
        "todo-write" | "TodoWrite" => "todo-write",
        "task-create" | "TaskCreate" => "task-create",
        "task-get" | "TaskGet" => "task-get",
        "task-list" | "TaskList" => "task-list",
        "task-update" | "TaskUpdate" => "task-update",
        "task-output" | "TaskOutput" | "AgentOutputTool" | "BashOutputTool" => "task-output",
        "task-stop" | "TaskStop" | "KillShell" => "task-stop",
        "workflow" | "Workflow" => "workflow",
        "enter-plan-mode" | "EnterPlanMode" => "enter-plan-mode",
        "exit-plan-mode" | "ExitPlanMode" => "exit-plan-mode",
        "verify-plan-execution" | "VerifyPlanExecution" => "verify-plan-execution",
        "skill" | "Skill" => "skill",
        "tool-search" | "ToolSearch" => "tool-search",
        "lsp" | "LSP" => "lsp",
        _ => name,
    }
}

pub fn provider_facing_tool_name(name: &str) -> &'static str {
    match canonical_tool_name(name) {
        "Agent" => "Agent",
        "bash" => "Bash",
        "file-read" => "Read",
        "file-edit" => "Edit",
        "file-write" => "Write",
        "glob" => "Glob",
        "grep" => "Grep",
        "notebook-edit" => "NotebookEdit",
        "web-fetch" => "WebFetch",
        "web-search" => "WebSearch",
        "ask-user-question" => "AskUserQuestion",
        "todo-write" => "TodoWrite",
        "task-create" => "TaskCreate",
        "task-get" => "TaskGet",
        "task-list" => "TaskList",
        "task-update" => "TaskUpdate",
        "task-output" => "TaskOutput",
        "task-stop" => "TaskStop",
        "workflow" => "Workflow",
        "enter-plan-mode" => "EnterPlanMode",
        "exit-plan-mode" => "ExitPlanMode",
        "verify-plan-execution" => "VerifyPlanExecution",
        "skill" => "Skill",
        "tool-search" => "ToolSearch",
        "lsp" => "LSP",
        _ => unreachable!("unsupported tool name"),
    }
}

pub(crate) fn provider_facing_tool_description(name: &str, fallback: &'static str) -> &'static str {
    match canonical_tool_name(name) {
        "bash" => concat!(
            "Run a shell command in the current workspace.\n",
            "- Use this for directory listings and simple shell inspection.\n",
            "- Prefer Read for opening files, Glob for filename searches, and Grep for content searches.\n",
            "- If commands are independent, you may make multiple Bash tool calls in a single response.\n",
            "- Provide a short `description` when helpful so the action is easier to review."
        ),
        "file-read" => concat!(
            "Read a file from the local filesystem.\n",
            "- The `file_path` should be an absolute path when possible.\n",
            "- Use `offset` and `limit` for targeted reads of large files.\n",
            "- This tool reads files, not directories."
        ),
        "glob" => concat!(
            "Fast file pattern matching tool.\n",
            "- Use this to find files by name patterns like `**/*.rs` or `src/**/*.ts`.\n",
            "- Prefer this over using find or ls recursively from Bash for broad filename searches."
        ),
        "task-create" => concat!(
            "Create a structured task in the current workspace task list.\n",
            "- Prefer this over TodoWrite for interactive multi-step work.\n",
            "- Use it as soon as the user describes 3+ steps so progress is visible in the task panel.\n",
            "- Pair with TaskUpdate to flip a task to `in_progress` before you start work and `completed` when it lands."
        ),
        "task-get" => "Read a single task from the current workspace task list by ID.",
        "task-list" => concat!(
            "List tasks in the current workspace task list.\n",
            "- Run this before adding tasks to avoid duplicates and to see who owns what.\n",
            "- The dedicated task panel already shows live state, so reach for this when you need raw structured output."
        ),
        "task-update" => concat!(
            "Update task status, fields, or dependencies in the current workspace task list.\n",
            "- Move a task to `in_progress` BEFORE you start and to `completed` as soon as the work lands.\n",
            "- Use `status: deleted` to remove a task; ids are never recycled.\n",
            "- Use `addBlocks` / `addBlockedBy` to declare dependency edges between tasks."
        ),
        "todo-write" => concat!(
            "Append or replace a simple todo snapshot under ORBCODE_HOME.\n",
            "- Legacy/compatibility tool: prefer TaskCreate / TaskUpdate for interactive multi-step work.\n",
            "- Useful for non-interactive scripts or quick one-off lists that don't need a structured task panel."
        ),
        "task-output" => "Read logs and status from a background task by ID.",
        "task-stop" => "Stop a running background task by ID.",
        "workflow" => concat!(
            "Start a generated dynamic workflow as a durable background task.\n",
            "- Use this when the user's goal is multi-step, benefits from independent parallel analysis, needs staged investigation, or is pipeline-shaped and sub-agent context should stay isolated.\n",
            "- Do not use this for single-step questions, simple edits, unclear goals that need clarification, or cases where the user only asks to discuss a plan.\n",
            "- If you choose this tool, generate the inline JSON spec and call Workflow in the same turn instead of only describing a plan.\n",
            "- The `spec` must use schema_version 1 and only the supported single-key step objects: {\"agent\":{\"description\":\"...\",\"prompt\":\"...\"}}, {\"parallel\":{\"steps\":[...]}}, {\"pipeline\":{\"steps\":[...]}}, {\"phase\":{\"name\":\"...\",\"steps\":[...]}}, or {\"log\":{\"message\":\"...\"}}.\n",
            "- The tool input itself must be a JSON object; do not encode the entire input or `spec` as a quoted JSON string.\n",
            "- For parallel agent steps, every child must be its own complete object, for example {\"parallel\":{\"steps\":[{\"agent\":{\"description\":\"task2\",\"prompt\":\"...\"}},{\"agent\":{\"description\":\"task3\",\"prompt\":\"...\"}}]}}.\n",
            "- Do not use `kind`, step-level `name`, or `run_in_background` fields; the runtime rejects that shape.\n",
            "- For generated workflows, agent is the only executable work unit; use phase, parallel, and pipeline to organize sub-agent work, and log only as a non-work status marker.\n",
            "- Agent prompts must be self-contained with the local goal, relevant constraints, expected output shape, and any dependency on prior pipeline output.\n",
            "- Use a short generated workflow name like dynamic:<kebab-name>.\n",
            "- The main agent starts the workflow, observes progress, and synthesizes final results for the user after sub-agents complete."
        ),
        "enter-plan-mode" => "Enter plan mode and create a workspace plan file for exploration.",
        "exit-plan-mode" => "Exit plan mode and present the current workspace plan.",
        "verify-plan-execution" => {
            "Capture a lightweight verification snapshot for the current workspace plan."
        }
        "skill" => {
            "Load a project or user skill and expand its instructions into the current conversation."
        }
        "tool-search" => {
            "Search tool names and return matching JSON schemas for follow-up tool calls."
        }
        "lsp" => {
            "Heuristic code intelligence for definitions, references, symbols, hover, and call relationships."
        }
        "grep" => concat!(
            "Powerful repository search built on ripgrep.\n",
            "- Use this for content search tasks instead of calling `grep` or `rg` via Bash.\n",
            "- Supports regex patterns and optional glob filters.\n",
            "- `files_with_matches` is the default output mode for broad code searches."
        ),
        _ => fallback,
    }
}

pub(crate) fn tool(
    name: &'static str,
    summary: &'static str,
    requires_tools_permission: bool,
    requires_network_permission: bool,
) -> ToolSpec {
    ToolSpec {
        name,
        status: ToolStatus::Available,
        summary,
        requires_tools_permission,
        requires_network_permission,
        capability: foundation_tool_capability(name),
        provider_hidden: false,
    }
}

pub(crate) fn hidden_tool(
    name: &'static str,
    summary: &'static str,
    requires_tools_permission: bool,
    requires_network_permission: bool,
) -> ToolSpec {
    ToolSpec {
        name,
        status: ToolStatus::Available,
        summary,
        requires_tools_permission,
        requires_network_permission,
        capability: foundation_tool_capability(name),
        provider_hidden: true,
    }
}

fn foundation_tool_capability(name: &str) -> ToolCapability {
    match canonical_tool_name(name) {
        "file-read" | "glob" | "grep" | "lsp" => ToolCapability::WorkspaceRead,
        "file-write" | "file-edit" | "notebook-edit" => ToolCapability::WorkspaceWrite,
        "bash" => ToolCapability::SandboxedCommand,
        "web-fetch" | "web-search" => ToolCapability::Network,
        "workflow" => ToolCapability::ExternalSideEffect,
        "ask-user-question"
        | "enter-plan-mode"
        | "exit-plan-mode"
        | "verify-plan-execution"
        | "Agent"
        | "skill"
        | "tool-search"
        | "todo-write"
        | "task-create"
        | "task-get"
        | "task-list"
        | "task-update"
        | "task-output"
        | "task-stop" => ToolCapability::Internal,
        _ => ToolCapability::ExternalSideEffect,
    }
}

fn workflow_tool_input_schema() -> Value {
    json!({
        "type": "object",
        "$defs": {
            "workflow_step": {
                "type": "object",
                "description": "A workflow step is a single-key object. Use exactly one of agent, parallel, pipeline, phase, or log. Do not use kind/name/run_in_background fields.",
                "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "agent": {
                                    "type": "object",
                                    "properties": {
                                        "description": { "type": "string" },
                                        "prompt": { "type": "string" }
                                    },
                                    "required": ["description", "prompt"],
                                    "additionalProperties": false
                                }
                        },
                        "required": ["agent"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "parallel": {
                                "type": "object",
                                "properties": {
                                    "steps": {
                                        "type": "array",
                                        "minItems": 1,
                                        "items": { "$ref": "#/$defs/workflow_step" }
                                    }
                                },
                                "required": ["steps"],
                                "additionalProperties": false
                            }
                        },
                        "required": ["parallel"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "pipeline": {
                                "type": "object",
                                "properties": {
                                    "steps": {
                                        "type": "array",
                                        "minItems": 1,
                                        "items": { "$ref": "#/$defs/workflow_step" }
                                    }
                                },
                                "required": ["steps"],
                                "additionalProperties": false
                            }
                        },
                        "required": ["pipeline"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "phase": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string" },
                                    "steps": {
                                        "type": "array",
                                        "minItems": 1,
                                        "items": { "$ref": "#/$defs/workflow_step" }
                                    }
                                },
                                "required": ["name", "steps"],
                                "additionalProperties": false
                            }
                        },
                        "required": ["phase"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "log": {
                                "type": "object",
                                "properties": {
                                    "message": { "type": "string" }
                                },
                                "required": ["message"],
                                "additionalProperties": false
                            }
                        },
                        "required": ["log"],
                        "additionalProperties": false
                    }
                ]
            }
        },
        "properties": {
            "name": {
                "type": "string",
                "description": "Short generated workflow name, for example dynamic:check."
            },
            "arguments": {
                "type": "string",
                "description": "Optional arguments substituted into workflow text fields as $ARGUMENTS and $1..$9."
            },
            "spec": {
                "type": "object",
                "description": "Inline workflow JSON spec object. Do not pass this as a JSON-encoded string. Supports schema_version 1 with single-key agent, parallel, pipeline, phase, and log step objects.",
                "properties": {
                    "schema_version": { "type": "integer", "enum": [1] },
                    "description": { "type": "string" },
                    "max_concurrency": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 16
                    },
                    "steps": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "$ref": "#/$defs/workflow_step" }
                    }
                },
                "required": ["schema_version", "steps"],
                "additionalProperties": false
            }
        },
        "required": ["spec"],
        "additionalProperties": false,
    })
}

pub(crate) fn tool_input_schema(name: &str) -> Value {
    match name {
        "Agent" => json!({
            "type": "object",
            "properties": {
                "description": { "type": "string", "description": "Short description of the delegated task." },
                "prompt": { "type": "string", "description": "Detailed prompt for the local synchronous subagent." },
                "subagent_type": { "type": "string", "description": "Optional agent type label such as Explore." },
                "subagentType": { "type": "string", "description": "CamelCase alias for subagent_type." },
                "run_in_background": {
                    "type": "boolean",
                    "description": "When true, run the subagent as a durable background task; the call returns immediately with a task ID and the model can poll TaskOutput/TaskStop."
                },
                "runInBackground": {
                    "type": "boolean",
                    "description": "CamelCase alias for run_in_background."
                }
            },
            "required": ["description", "prompt"],
            "additionalProperties": false,
        }),
        "workflow" => workflow_tool_input_schema(),
        "bash" => json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to execute in the current workspace." },
                "cmd": { "type": "string", "description": "Alias for command." },
                "script": { "type": "string", "description": "Alias for command." },
                "description": { "type": "string", "description": "Short human-readable summary of what the command is doing." },
                "timeout": { "type": "integer", "description": "Optional timeout in milliseconds." },
                "sandbox_permissions": {
                    "type": "string",
                    "enum": ["use_default", "require_escalated"],
                    "description": "Set to require_escalated to request running this command outside the configured sandbox after user approval."
                },
                "sandboxPermissions": {
                    "type": "string",
                    "enum": ["use_default", "require_escalated"],
                    "description": "CamelCase alias for sandbox_permissions."
                },
                "dangerouslyDisableSandbox": {
                    "type": "boolean",
                    "description": "Legacy compatibility alias for requesting unsandboxed execution."
                }
            },
            "anyOf": [
                { "required": ["command"] },
                { "required": ["cmd"] },
                { "required": ["script"] }
            ],
            "additionalProperties": false,
        }),
        "file-read" => json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Absolute or workspace-relative path to the file to read." },
                "filePath": { "type": "string", "description": "CamelCase alias for file_path." },
                "path": { "type": "string", "description": "Legacy alias for file_path." },
                "offset": { "type": "integer", "description": "Optional 1-based line number to start reading from." },
                "limit": { "type": "integer", "description": "Optional number of lines to read from the offset." },
                "start_line": { "type": "integer", "description": "Legacy alias for offset." },
                "end_line": { "type": "integer", "description": "Legacy alias for an inclusive end line." },
                "pages": { "type": "string", "description": "Optional page range for PDF files." }
            },
            "required": ["file_path"],
            "additionalProperties": false,
        }),
        "file-edit" => json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string" },
                "filePath": { "type": "string", "description": "CamelCase alias for file_path." },
                "path": { "type": "string", "description": "Legacy alias for file_path." },
                "old_string": { "type": "string" },
                "find": { "type": "string", "description": "Legacy alias for old_string." },
                "new_string": { "type": "string" },
                "replace": { "type": "string", "description": "Legacy alias for new_string." },
                "replace_all": { "type": "boolean", "description": "When true, replace every occurrence." },
                "all": { "type": "boolean", "description": "Legacy alias for replace_all." }
            },
            "required": ["file_path", "old_string", "new_string"],
            "additionalProperties": false,
        }),
        "file-write" => json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string" },
                "filePath": { "type": "string", "description": "CamelCase alias for file_path." },
                "path": { "type": "string", "description": "Legacy alias for file_path." },
                "content": { "type": "string" }
            },
            "required": ["file_path", "content"],
            "additionalProperties": false,
        }),
        "glob" => json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern such as **/*.rs or src/*.ts." },
                "glob": { "type": "string", "description": "Alias for pattern." },
                "path": { "type": "string", "description": "Optional directory to search in relative to the workspace." },
                "base": { "type": "string", "description": "Alias for path." }
            },
            "additionalProperties": false,
        }),
        "grep" => json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string", "description": "Optional file or directory to search under." },
                "glob": { "type": "string", "description": "Optional file glob filter such as **/*.rs or src/*.ts." },
                "output_mode": { "type": "string", "enum": ["content", "files_with_matches", "count"] },
                "-n": { "type": "boolean", "description": "Show line numbers when output_mode is content." },
                "-i": { "type": "boolean", "description": "Case insensitive search." },
                "head_limit": { "type": "integer", "description": "Optional limit on returned lines or files." }
            },
            "required": ["pattern"],
            "additionalProperties": false,
        }),
        "notebook-edit" => json!({
            "type": "object",
            "properties": {
                "notebook_path": { "type": "string", "description": "Absolute path to the Jupyter notebook (.ipynb) file to edit." },
                "new_source": { "type": "string", "description": "The new source for the cell." },
                "cell_id": { "type": "string", "description": "Cell id to target. When inserting, the new cell goes after this cell, or at the start if omitted. Accepts the synthetic cell-N index form." },
                "cell_number": { "type": "integer", "description": "Zero-based index of the cell to target, as an alternative to cell_id." },
                "cell_type": { "type": "string", "enum": ["code", "markdown"], "description": "Cell type. Required when edit_mode=insert; defaults to the existing cell type otherwise." },
                "edit_mode": { "type": "string", "enum": ["replace", "insert", "delete", "append"], "description": "The edit to make. Defaults to replace." }
            },
            "required": ["notebook_path", "new_source"],
            "additionalProperties": false,
        }),
        "web-fetch" => json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "Absolute URL to fetch." }
            },
            "required": ["url"],
            "additionalProperties": false,
        }),
        "web-search" => json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"],
            "additionalProperties": false,
        }),
        "ask-user-question" => json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "minLength": 1, "maxLength": 128 },
                            "question": { "type": "string", "minLength": 1, "maxLength": 4096 },
                            "header": { "type": "string", "maxLength": 12 },
                            "multi_select": { "type": "boolean", "default": false },
                            "options": {
                                "type": "array",
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "id": { "type": "string", "minLength": 1, "maxLength": 128 },
                                        "label": { "type": "string", "minLength": 1, "maxLength": 256 },
                                        "description": { "type": "string", "maxLength": 1024 },
                                        "preview": { "type": "string", "maxLength": 16384 }
                                    },
                                    "required": ["id", "label", "description"],
                                    "additionalProperties": false
                                }
                            },
                            "allow_free_text": { "type": "boolean", "default": true },
                            "allow_annotation": { "type": "boolean", "default": false }
                        },
                        "required": ["id", "question", "header", "multi_select", "options"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["questions"],
            "additionalProperties": false,
        }),
        "todo-write" => json!({
            "type": "object",
            "properties": {
                "list": { "type": "string" },
                "mode": { "type": "string", "enum": ["append", "replace"] },
                "items": {
                    "type": "array",
                    "items": {
                        "anyOf": [
                            { "type": "string" },
                            {
                                "type": "object",
                                "properties": {
                                    "title": { "type": "string" },
                                    "done": { "type": "boolean" }
                                },
                                "required": ["title"],
                                "additionalProperties": false
                            }
                        ]
                    }
                }
            },
            "required": ["items"],
            "additionalProperties": false,
        }),
        "task-create" => json!({
            "type": "object",
            "properties": {
                "subject": { "description": "A brief title for the task", "type": "string" },
                "description": { "description": "What needs to be done", "type": "string" },
                "activeForm": { "description": "Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")", "type": "string" },
                "metadata": { "additionalProperties": {}, "description": "Arbitrary metadata to attach to the task", "propertyNames": { "type": "string" }, "type": "object" }
            },
            "required": ["subject", "description"],
            "additionalProperties": false,
        }),
        "task-get" => json!({
            "type": "object",
            "properties": {
                "taskId": { "type": "string" }
            },
            "required": ["taskId"],
            "additionalProperties": false,
        }),
        "task-list" => json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }),
        "task-update" => json!({
            "type": "object",
            "properties": {
                "taskId": { "description": "The ID of the task to update", "type": "string" },
                "status": {
                    "anyOf": [
                        { "enum": ["pending", "in_progress", "completed"], "type": "string" },
                        { "const": "deleted", "type": "string" }
                    ],
                    "description": "New status for the task"
                },
                "subject": { "description": "New subject for the task", "type": "string" },
                "description": { "description": "New description for the task", "type": "string" },
                "activeForm": { "description": "Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")", "type": "string" },
                "owner": { "description": "New owner for the task", "type": "string" },
                "metadata": { "additionalProperties": {}, "description": "Metadata keys to merge into the task. Set a key to null to delete it.", "propertyNames": { "type": "string" }, "type": "object" },
                "addBlocks": { "description": "Task IDs that this task blocks", "items": { "type": "string" }, "type": "array" },
                "addBlockedBy": { "description": "Task IDs that block this task", "items": { "type": "string" }, "type": "array" }
            },
            "required": ["taskId"],
            "additionalProperties": false,
        }),
        "task-output" => json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string" },
                "taskId": { "type": "string" },
                "block": { "type": "boolean" },
                "timeout": { "type": "integer" }
            },
            "anyOf": [
                { "required": ["task_id"] },
                { "required": ["taskId"] }
            ],
            "additionalProperties": false,
        }),
        "task-stop" => json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string" },
                "taskId": { "type": "string" },
                "shell_id": { "type": "string" }
            },
            "anyOf": [
                { "required": ["task_id"] },
                { "required": ["taskId"] },
                { "required": ["shell_id"] }
            ],
            "additionalProperties": false,
        }),
        "enter-plan-mode" => json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }),
        "exit-plan-mode" => json!({
            "type": "object",
            "properties": {
                "plan": { "type": "string" },
                "allowedPrompts": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": { "type": "string" },
                            "prompt": { "type": "string" }
                        },
                        "required": ["tool", "prompt"],
                        "additionalProperties": false
                    }
                }
            },
            "additionalProperties": true,
        }),
        "verify-plan-execution" => json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }),
        "skill" => json!({
            "type": "object",
            "properties": {
                "skill": { "type": "string" },
                "args": { "type": "string" }
            },
            "required": ["skill"],
            "additionalProperties": false,
        }),
        "tool-search" => json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "max_results": { "type": "integer" }
            },
            "required": ["query"],
            "additionalProperties": false,
        }),
        "lsp" => json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": [
                        "goToDefinition",
                        "findReferences",
                        "hover",
                        "documentSymbol",
                        "workspaceSymbol",
                        "goToImplementation",
                        "prepareCallHierarchy",
                        "incomingCalls",
                        "outgoingCalls"
                    ]
                },
                "filePath": { "type": "string" },
                "file_path": { "type": "string" },
                "line": { "type": "integer" },
                "character": { "type": "integer" }
            },
            "required": ["operation", "line", "character"],
            "anyOf": [
                { "required": ["filePath"] },
                { "required": ["file_path"] }
            ],
            "additionalProperties": false,
        }),
        _ => json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true,
        }),
    }
}

pub(crate) fn search_tool_specs<'a>(
    specs: &'a [ToolSpec],
    query: &str,
    max_results: usize,
) -> Vec<&'a ToolSpec> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Some(selected) = trimmed.strip_prefix("select:") {
        let requested = selected
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        return requested
            .into_iter()
            .filter_map(|requested_name| {
                specs.iter().find(|spec| {
                    spec.name.eq_ignore_ascii_case(requested_name)
                        || provider_facing_tool_name(spec.name).eq_ignore_ascii_case(requested_name)
                })
            })
            .collect();
    }
    let query_terms = trimmed
        .to_ascii_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut scored = specs
        .iter()
        .map(|spec| {
            let haystack = format!(
                "{} {} {}",
                spec.name,
                provider_facing_tool_name(spec.name),
                provider_facing_tool_description(spec.name, spec.summary)
            )
            .to_ascii_lowercase();
            let mut score = 0usize;
            for term in &query_terms {
                if haystack.contains(term) {
                    score += if spec.name.eq_ignore_ascii_case(term) {
                        10
                    } else {
                        3
                    };
                }
            }
            (score, spec)
        })
        .filter(|(score, _)| *score > 0)
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.name.cmp(right.1.name))
    });
    scored
        .into_iter()
        .take(max_results)
        .map(|(_, spec)| spec)
        .collect()
}

async fn search_mcp_provider_tools(
    mcp: &McpRegistry,
    session_id: Option<&str>,
    query: &str,
    max_results: usize,
) -> Vec<orbcode_mcp::McpToolDescriptor> {
    let query_terms = search_terms(query);
    if query_terms.is_empty() || max_results == 0 {
        return Vec::new();
    }
    let tools = match session_id {
        Some(session_id) => mcp.list_provider_tools_for_session(session_id).await,
        None => mcp.list_provider_tools().await,
    };
    let mut scored = tools
        .into_iter()
        .filter_map(|desc| {
            let provider_name = mcp_provider_tool_name(&desc.server_id, &desc.tool_name);
            let haystack = format!(
                "{} {} {} {}",
                provider_name, desc.server_id, desc.tool_name, desc.description
            )
            .to_ascii_lowercase();
            let score = score_terms(&haystack, &query_terms);
            (score > 0).then_some((score, provider_name, desc))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    scored
        .into_iter()
        .take(max_results)
        .map(|(_, _, desc)| desc)
        .collect()
}

async fn search_mcp_resources(
    mcp: &McpRegistry,
    session_id: Option<&str>,
    query: &str,
    max_results: usize,
) -> Vec<Value> {
    let query_terms = search_terms(query);
    if query_terms.is_empty() || max_results == 0 {
        return Vec::new();
    }
    let servers = match session_id {
        Some(session_id) => mcp.list_servers_for_session(session_id).await,
        None => mcp.list_servers().await,
    }
    .into_iter()
    .filter(|server| server.enabled && server.trust.is_trusted())
    .collect::<Vec<_>>();
    let mut scored = Vec::new();
    for server in servers {
        let resources = match session_id {
            Some(session_id) => mcp.list_resources_for_session(session_id, &server.id).await,
            None => mcp.list_resources(&server.id).await,
        };
        let Ok(resources) = resources else {
            continue;
        };
        for resource in resources {
            let haystack = mcp_resource_haystack(&server.id, &resource);
            let score = score_terms(&haystack, &query_terms);
            if score > 0 {
                scored.push((score, server.id.clone(), resource));
            }
        }
    }
    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.uri.cmp(&right.2.uri))
    });
    scored
        .into_iter()
        .take(max_results)
        .map(|(_, server_id, resource)| {
            json!({
                "server_id": server_id,
                "uri": resource.uri,
                "name": resource.name,
                "mime_type": resource.mime_type,
                "description": resource.description,
                "use": "ReadMcpResourceTool",
            })
        })
        .collect()
}

fn mcp_resource_haystack(server_id: &str, resource: &McpResourceSummary) -> String {
    format!(
        "{} {} {} {} {}",
        server_id, resource.uri, resource.name, resource.mime_type, resource.description
    )
    .to_ascii_lowercase()
}

fn search_terms(query: &str) -> Vec<String> {
    query
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn score_terms(haystack: &str, query_terms: &[String]) -> usize {
    query_terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .map(|term| if haystack == term.as_str() { 10 } else { 3 })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invariant: every provider-visible foundation tool name must resolve back
    /// to a canonical name that exists in the planned registry AND has explicit
    /// dispatch in `ToolRegistry::invoke`. This ensures the model never receives
    /// a tool name it cannot execute.
    #[test]
    fn registry_invariant_provider_visible_tools_are_dispatchable() {
        let registry = ToolRegistry::foundation();
        let defs = registry.provider_definitions(true, true);

        let canonical_dispatch_names: std::collections::HashSet<&str> = [
            "Agent",
            "bash",
            "file-read",
            "file-edit",
            "file-write",
            "glob",
            "grep",
            "notebook-edit",
            "web-fetch",
            "web-search",
            "ask-user-question",
            "todo-write",
            "task-create",
            "task-get",
            "task-list",
            "task-update",
            "task-output",
            "task-stop",
            "workflow",
            "enter-plan-mode",
            "exit-plan-mode",
            "verify-plan-execution",
            "skill",
            "tool-search",
            "lsp",
        ]
        .into_iter()
        .collect();

        for def in &defs {
            let canonical = canonical_tool_name(&def.name);
            assert!(
                registry.spec(canonical).is_some(),
                "provider-visible tool `{}` (canonical `{}`) has no planned spec",
                def.name,
                canonical
            );
            assert!(
                canonical_dispatch_names.contains(canonical),
                "provider-visible tool `{}` (canonical `{}`) has no dispatch arm in invoke()",
                def.name,
                canonical
            );
        }
    }

    #[test]
    fn workflow_requires_external_side_effect_review() {
        let registry = ToolRegistry::foundation();
        assert_eq!(
            registry.spec("Workflow").expect("workflow spec").capability,
            ToolCapability::ExternalSideEffect
        );
    }

    #[test]
    fn workflow_provider_description_guides_dynamic_planning_behavior() {
        let description = provider_facing_tool_description("Workflow", "");

        assert!(description.contains("call Workflow in the same turn"));
        assert!(description.contains("single-step questions"));
        assert!(description.contains("single-key step objects"));
        assert!(description.contains("Do not use `kind`"));
        assert!(description.contains("agent is the only executable work unit"));
        assert!(description.contains("main agent starts the workflow"));
    }

    #[test]
    fn workflow_provider_schema_describes_recursive_single_key_steps() {
        let schema = tool_input_schema("workflow");
        let step = &schema["$defs"]["workflow_step"];

        assert!(step["oneOf"].is_array());
        assert_eq!(
            schema["properties"]["spec"]["properties"]["steps"]["items"]["$ref"],
            "#/$defs/workflow_step"
        );
        assert_eq!(
            step["oneOf"][0]["properties"]["agent"]["required"],
            json!(["description", "prompt"])
        );
        assert_eq!(
            step["oneOf"][0]["properties"]["agent"]["properties"],
            json!({
                "description": { "type": "string" },
                "prompt": { "type": "string" }
            })
        );
        assert_eq!(
            step["oneOf"][1]["properties"]["parallel"]["properties"]["steps"]["items"]["$ref"],
            "#/$defs/workflow_step"
        );
        assert!(
            step["description"]
                .as_str()
                .is_some_and(|description| description.contains("Do not use kind"))
        );
    }

    /// Invariant: every provider-visible tool must reach a real dispatch arm
    /// in `invoke()`. Unlike the static-list test above, this actually invokes
    /// each tool and asserts it does NOT return `ToolError::NotFound`. A tool
    /// that reaches its handler will return `InvalidInput`, `ExecutionFailed`,
    /// or similar — any error other than `NotFound` proves dispatch is wired.
    #[tokio::test]
    async fn registry_invariant_provider_visible_tools_dispatch_reachable() {
        let registry = ToolRegistry::foundation();
        let defs = registry.provider_definitions(true, true);

        let home = std::env::temp_dir().join("orbcode-registry-invariant-dispatch-reachable");
        let cwd = home.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let mcp = orbcode_mcp::McpRegistry::load(&home, &cwd)
            .await
            .expect("load mcp");
        let context = crate::ToolContext {
            cwd,
            additional_directories: Vec::new(),
            home_dir: home,
            sandbox_mode: orbcode_protocol::SandboxMode::DangerFullAccess,
            sandbox_allow_network: true,
            allow_network: true,
            allow_tools: true,
            mcp,
            progress: None,
            cancellation: crate::ToolCancellationToken::default(),
            read_state: None,
            session_id: Some("dispatch-reachable".to_string()),
            local_shell_tasks: None,
            on_cwd_change: None,
            plans_directory_override: None,
            ask_user_tx: None,
            settings_env: std::collections::BTreeMap::new(),
            skill_definitions: None,
        };

        for def in &defs {
            let result = registry.invoke(&def.name, "{}", &context).await;
            assert!(
                !matches!(result, Err(ToolError::NotFound(_))),
                "provider-visible tool `{}` returned NotFound — \
                 it has no dispatch arm in invoke()",
                def.name
            );
        }
    }

    #[test]
    fn registry_invariant_hidden_tools_excluded_from_provider_definitions() {
        let registry = ToolRegistry::foundation();
        let defs = registry.provider_definitions(true, true);
        let provider_names: std::collections::HashSet<String> =
            defs.iter().map(|d| d.name.clone()).collect();

        assert!(
            !provider_names.contains("AskUserQuestion"),
            "ask-user-question should not appear without an interactive capability context"
        );

        for spec in registry.planned() {
            if spec.provider_hidden {
                let provider_name = provider_facing_tool_name(spec.name).to_string();
                assert!(
                    !provider_names.contains(&provider_name),
                    "hidden tool `{}` should not appear in provider definitions",
                    spec.name
                );
            }
        }
    }

    #[tokio::test]
    async fn registry_invariant_plugin_tools_excluded_from_provider_definitions() {
        let registry = ToolRegistry::foundation();
        let tools = vec![orbcode_config::PluginToolDefinition {
            name: "search".into(),
            description: "Plugin search".into(),
            input_schema: json!({"type": "object"}),
            requires_permission: false,
            plugin_id: "demo@market".into(),
            plugin_name: "demo".into(),
        }];
        registry.set_plugin_tools(&tools);

        let home = std::env::temp_dir().join("orbcode-registry-invariant-plugin");
        let cwd = home.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let mcp = orbcode_mcp::McpRegistry::load(&home, &cwd)
            .await
            .expect("load mcp");

        let defs = registry
            .provider_definitions_with_mcp(true, true, &mcp)
            .await;
        assert!(
            !defs.iter().any(|d| d.name == "plugin__demo__search"),
            "plugin tools must not appear in model-visible provider definitions"
        );

        let diagnostic_defs = registry
            .diagnostic_definitions_with_mcp(true, true, &mcp)
            .await;
        assert!(
            diagnostic_defs
                .iter()
                .any(|d| d.name == "plugin__demo__search"),
            "plugin tools must appear in diagnostic definitions"
        );
    }

    #[test]
    fn plugin_provider_tool_name_format() {
        assert_eq!(
            plugin_provider_tool_name("demo", "search"),
            "plugin__demo__search"
        );
        assert_eq!(
            plugin_provider_tool_name("my-plugin", "do_stuff"),
            "plugin__my-plugin__do_stuff"
        );
    }

    #[test]
    fn parse_plugin_provider_tool_name_roundtrip() {
        let name = plugin_provider_tool_name("demo", "search");
        let (plugin, tool) = parse_plugin_provider_tool_name(&name).unwrap();
        assert_eq!(plugin, "demo");
        assert_eq!(tool, "search");
    }

    #[test]
    fn parse_plugin_provider_tool_name_roundtrip_with_hyphens_and_underscores() {
        let name = plugin_provider_tool_name("my-plugin", "do_stuff");
        let (plugin, tool) = parse_plugin_provider_tool_name(&name).unwrap();
        assert_eq!(plugin, "my-plugin");
        assert_eq!(tool, "do_stuff");
    }

    #[test]
    fn parse_plugin_provider_tool_name_tool_with_double_underscore() {
        let name = plugin_provider_tool_name("abc", "foo__bar");
        let (plugin, tool) = parse_plugin_provider_tool_name(&name).unwrap();
        assert_eq!(plugin, "abc");
        assert_eq!(tool, "foo__bar");
    }

    #[test]
    fn parse_plugin_provider_tool_name_rejects_invalid() {
        assert!(parse_plugin_provider_tool_name("bash").is_none());
        assert!(parse_plugin_provider_tool_name("mcp__server__tool").is_none());
        assert!(parse_plugin_provider_tool_name("plugin__").is_none());
        assert!(parse_plugin_provider_tool_name("plugin____tool").is_none());
        assert!(parse_plugin_provider_tool_name("plugin__name__").is_none());
    }

    #[test]
    fn parse_plugin_provider_tool_name_rejects_prefix_only() {
        assert!(parse_plugin_provider_tool_name("plugin__name").is_none());
        assert!(parse_plugin_provider_tool_name("plugin").is_none());
        assert!(parse_plugin_provider_tool_name("plugin_").is_none());
        assert!(parse_plugin_provider_tool_name("").is_none());
    }

    #[tokio::test]
    async fn invoke_plugin_tool_missing_plugin_returns_not_installed() {
        let registry = ToolRegistry::foundation();

        let home = std::env::temp_dir().join("orbcode-plugin-dispatch-missing");
        let cwd = home.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let mcp = orbcode_mcp::McpRegistry::load(&home, &cwd)
            .await
            .expect("load mcp");
        let context = crate::ToolContext {
            cwd,
            additional_directories: Vec::new(),
            home_dir: home,
            sandbox_mode: orbcode_protocol::SandboxMode::DangerFullAccess,
            sandbox_allow_network: true,
            allow_network: true,
            allow_tools: true,
            mcp,
            progress: None,
            cancellation: crate::ToolCancellationToken::default(),
            read_state: None,
            session_id: Some("plugin-dispatch-test".to_string()),
            local_shell_tasks: None,
            on_cwd_change: None,
            plans_directory_override: None,
            ask_user_tx: None,
            settings_env: std::collections::BTreeMap::new(),
            skill_definitions: None,
        };

        let result = registry.invoke("plugin__ghost__tool", "{}", &context).await;
        let err = result.unwrap_err();
        match err {
            crate::ToolError::PluginDispatch(ref e) => {
                assert!(
                    matches!(
                        e,
                        crate::types::PluginDispatchError::PluginNotInstalled { .. }
                    ),
                    "expected PluginNotInstalled, got: {e:?}"
                );
                assert!(err.to_string().contains("ghost"));
            }
            other => panic!("expected PluginDispatch, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_plugin_tool_disabled_via_feature_gate_returns_disabled_error() {
        let registry = ToolRegistry::foundation();

        let mut disabled = std::collections::HashSet::new();
        disabled.insert("plugin__gated__action".to_string());
        registry.set_feature_disabled_tools(disabled);

        let home = std::env::temp_dir().join("orbcode-plugin-dispatch-disabled");
        let cwd = home.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let mcp = orbcode_mcp::McpRegistry::load(&home, &cwd)
            .await
            .expect("load mcp");
        let context = crate::ToolContext {
            cwd,
            additional_directories: Vec::new(),
            home_dir: home,
            sandbox_mode: orbcode_protocol::SandboxMode::DangerFullAccess,
            sandbox_allow_network: true,
            allow_network: true,
            allow_tools: true,
            mcp,
            progress: None,
            cancellation: crate::ToolCancellationToken::default(),
            read_state: None,
            session_id: Some("plugin-dispatch-test".to_string()),
            local_shell_tasks: None,
            on_cwd_change: None,
            plans_directory_override: None,
            ask_user_tx: None,
            settings_env: std::collections::BTreeMap::new(),
            skill_definitions: None,
        };

        let result = registry
            .invoke("plugin__gated__action", "{}", &context)
            .await;
        let err = result.unwrap_err();
        match err {
            crate::ToolError::PluginDispatch(ref e) => {
                assert!(
                    matches!(e, crate::types::PluginDispatchError::PluginDisabled { .. }),
                    "expected PluginDisabled, got: {e:?}"
                );
                assert!(err.to_string().contains("gated"));
            }
            other => panic!("expected PluginDispatch, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_plugin_tool_not_found_when_plugin_has_other_tools() {
        let registry = ToolRegistry::foundation();
        let tools = vec![PluginToolDefinition {
            name: "search".into(),
            description: "Search something".into(),
            input_schema: json!({"type": "object"}),
            requires_permission: false,
            plugin_id: "demo@market".into(),
            plugin_name: "demo".into(),
        }];
        registry.set_plugin_tools(&tools);

        let home = std::env::temp_dir().join("orbcode-plugin-dispatch-tool-not-found");
        let cwd = home.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let mcp = orbcode_mcp::McpRegistry::load(&home, &cwd)
            .await
            .expect("load mcp");
        let context = crate::ToolContext {
            cwd,
            additional_directories: Vec::new(),
            home_dir: home,
            sandbox_mode: orbcode_protocol::SandboxMode::DangerFullAccess,
            sandbox_allow_network: true,
            allow_network: true,
            allow_tools: true,
            mcp,
            progress: None,
            cancellation: crate::ToolCancellationToken::default(),
            read_state: None,
            session_id: Some("plugin-dispatch-test".to_string()),
            local_shell_tasks: None,
            on_cwd_change: None,
            plans_directory_override: None,
            ask_user_tx: None,
            settings_env: std::collections::BTreeMap::new(),
            skill_definitions: None,
        };

        let result = registry
            .invoke("plugin__demo__nonexistent", "{}", &context)
            .await;
        let err = result.unwrap_err();
        match err {
            crate::ToolError::PluginDispatch(ref e) => {
                assert!(
                    matches!(e, crate::types::PluginDispatchError::ToolNotFound { .. }),
                    "expected ToolNotFound, got: {e:?}"
                );
                assert!(err.to_string().contains("demo"));
                assert!(err.to_string().contains("nonexistent"));
            }
            other => panic!("expected PluginDispatch, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_plugin_tool_registered_but_no_runtime_returns_unsupported() {
        let registry = ToolRegistry::foundation();
        let tools = vec![PluginToolDefinition {
            name: "search".into(),
            description: "Search something".into(),
            input_schema: json!({"type": "object"}),
            requires_permission: false,
            plugin_id: "demo@market".into(),
            plugin_name: "demo".into(),
        }];
        registry.set_plugin_tools(&tools);

        let home = std::env::temp_dir().join("orbcode-plugin-dispatch-unsupported");
        let cwd = home.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let mcp = orbcode_mcp::McpRegistry::load(&home, &cwd)
            .await
            .expect("load mcp");
        let context = crate::ToolContext {
            cwd,
            additional_directories: Vec::new(),
            home_dir: home,
            sandbox_mode: orbcode_protocol::SandboxMode::DangerFullAccess,
            sandbox_allow_network: true,
            allow_network: true,
            allow_tools: true,
            mcp,
            progress: None,
            cancellation: crate::ToolCancellationToken::default(),
            read_state: None,
            session_id: Some("plugin-dispatch-test".to_string()),
            local_shell_tasks: None,
            on_cwd_change: None,
            plans_directory_override: None,
            ask_user_tx: None,
            settings_env: std::collections::BTreeMap::new(),
            skill_definitions: None,
        };

        let result = registry
            .invoke("plugin__demo__search", "{}", &context)
            .await;
        let err = result.unwrap_err();
        match err {
            crate::ToolError::PluginDispatch(ref e) => {
                assert!(
                    matches!(
                        e,
                        crate::types::PluginDispatchError::UnsupportedRuntime { .. }
                    ),
                    "expected UnsupportedRuntime, got: {e:?}"
                );
                assert!(err.to_string().contains("demo"));
                assert!(err.to_string().contains("search"));
            }
            other => panic!("expected PluginDispatch, got: {other:?}"),
        }
    }

    #[test]
    fn set_plugin_tools_produces_provider_definitions() {
        let reg = ToolRegistry::foundation();
        let tools = vec![
            PluginToolDefinition {
                name: "search".into(),
                description: "Search the index".into(),
                input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
                requires_permission: false,
                plugin_id: "demo@market".into(),
                plugin_name: "demo".into(),
            },
            PluginToolDefinition {
                name: "write".into(),
                description: "Write data".into(),
                input_schema: json!({"type": "object"}),
                requires_permission: true,
                plugin_id: "demo@market".into(),
                plugin_name: "demo".into(),
            },
        ];
        reg.set_plugin_tools(&tools);

        let defs = reg.dynamic_definitions();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "plugin__demo__search");
        assert_eq!(defs[0].description, "Search the index");
        assert!(defs[0].input_schema.get("properties").is_some());
        assert_eq!(defs[1].name, "plugin__demo__write");
    }

    #[test]
    fn disabled_plugin_tools_excluded_from_provider_definitions() {
        let reg = ToolRegistry::foundation();
        let tools = vec![PluginToolDefinition {
            name: "hidden".into(),
            description: "Should not appear".into(),
            input_schema: json!({"type": "object"}),
            requires_permission: true,
            plugin_id: "secret@market".into(),
            plugin_name: "secret".into(),
        }];
        reg.set_plugin_tools(&tools);

        let mut disabled = std::collections::HashSet::new();
        disabled.insert("plugin__secret__hidden".to_string());
        reg.set_feature_disabled_tools(disabled);

        let defs = reg.dynamic_definitions();
        assert_eq!(
            defs.len(),
            1,
            "dynamic_definitions still holds the raw list"
        );

        let filtered: Vec<_> = defs
            .iter()
            .filter(|d| !reg.is_feature_disabled(&d.name))
            .collect();
        assert!(
            filtered.is_empty(),
            "disabled plugin tools should be excluded"
        );
    }

    /// Invariant: every tool name returned by `provider_definitions_with_mcp`
    /// must be resolvable through `invoke` — either via a local dispatch arm
    /// (canonical name exists in `planned`) or via MCP dispatch (`mcp__*`
    /// prefix). If this test fails, the model could receive a tool definition
    /// that `invoke` would reject with `ToolError::NotFound`.
    #[tokio::test]
    async fn registry_invariant_full_provider_list_is_dispatchable() {
        let registry = ToolRegistry::foundation();

        registry.set_dynamic_definitions(vec![
            ProviderToolDefinition {
                name: "plugin__demo__search".to_string(),
                description: "Plugin tool (should be filtered).".to_string(),
                input_schema: json!({"type": "object"}),
            },
            ProviderToolDefinition {
                name: "SkillAlpha".to_string(),
                description: "Non-plugin dynamic tool.".to_string(),
                input_schema: json!({"type": "object"}),
            },
        ]);

        let home = std::env::temp_dir().join("orbcode-registry-invariant-full-dispatch");
        let cwd = home.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let mcp = orbcode_mcp::McpRegistry::load(&home, &cwd)
            .await
            .expect("load mcp");

        let defs = registry
            .provider_definitions_with_mcp(true, true, &mcp)
            .await;

        for def in &defs {
            let canonical = canonical_tool_name(&def.name);
            let has_planned_spec = registry.spec(canonical).is_some();
            let is_mcp_route = parse_mcp_provider_tool_name(&def.name).is_some();
            assert!(
                has_planned_spec || is_mcp_route,
                "provider-visible tool `{}` has no dispatch route: \
                 not a known canonical spec and not an mcp__ name",
                def.name
            );
        }

        assert!(
            !defs.iter().any(|d| d.name == "plugin__demo__search"),
            "plugin tools must not appear in provider-visible list"
        );
    }

    /// MCP-discovered tools and dynamic definitions require `allow_tools`.
    /// When tools permission is disabled, only permission-free foundation tools
    /// should appear.
    #[tokio::test]
    async fn registry_invariant_mcp_and_dynamic_tools_require_tools_permission() {
        let registry = ToolRegistry::foundation();

        registry.set_dynamic_definitions(vec![ProviderToolDefinition {
            name: "DynTool".to_string(),
            description: "Dynamic tool.".to_string(),
            input_schema: json!({"type": "object"}),
        }]);

        let home = std::env::temp_dir().join("orbcode-registry-invariant-mcp-permission");
        let cwd = home.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let mcp = orbcode_mcp::McpRegistry::load(&home, &cwd)
            .await
            .expect("load mcp");

        let defs = registry
            .provider_definitions_with_mcp(false, false, &mcp)
            .await;

        for def in &defs {
            let spec = registry.spec(&def.name).unwrap_or_else(|| {
                panic!(
                    "tool `{}` in restricted provider list has no planned spec",
                    def.name
                )
            });
            assert!(
                !spec.requires_tools_permission,
                "tool `{}` requires tools permission but appeared with allow_tools=false",
                def.name
            );
            assert!(
                !spec.requires_network_permission,
                "tool `{}` requires network permission but appeared with allow_network=false",
                def.name
            );
        }

        assert!(
            !defs.iter().any(|d| d.name == "DynTool"),
            "dynamic tools must not appear when tools permission is disabled"
        );
    }

    /// Diagnostic listings must include provider-hidden tools and plugin
    /// contributions that are intentionally excluded from model-visible
    /// definitions.
    #[tokio::test]
    async fn registry_invariant_diagnostic_includes_hidden_and_plugin_tools() {
        let registry = ToolRegistry::foundation();

        let plugin_tools = vec![PluginToolDefinition {
            name: "inspect".into(),
            description: "Plugin inspect".into(),
            input_schema: json!({"type": "object"}),
            requires_permission: false,
            plugin_id: "diag@market".into(),
            plugin_name: "diag".into(),
        }];
        registry.set_plugin_tools(&plugin_tools);

        let home = std::env::temp_dir().join("orbcode-registry-invariant-diagnostic");
        let cwd = home.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let mcp = orbcode_mcp::McpRegistry::load(&home, &cwd)
            .await
            .expect("load mcp");

        let diag = registry
            .diagnostic_definitions_with_mcp(true, true, &mcp)
            .await;
        let diag_names: std::collections::HashSet<String> =
            diag.iter().map(|d| d.name.clone()).collect();

        assert!(
            diag_names.contains("AskUserQuestion"),
            "diagnostic listing must include provider-hidden ask-user-question"
        );
        assert!(
            diag_names.contains("plugin__diag__inspect"),
            "diagnostic listing must include plugin tools"
        );

        let provider = registry
            .provider_definitions_with_mcp(true, true, &mcp)
            .await;
        let provider_names: std::collections::HashSet<String> =
            provider.iter().map(|d| d.name.clone()).collect();

        assert!(
            !provider_names.contains("AskUserQuestion"),
            "model-visible listing must NOT include ask-user-question"
        );
        assert!(
            !provider_names.contains("plugin__diag__inspect"),
            "model-visible listing must NOT include plugin tools"
        );
    }

    /// Deferred TypeScript tools must never appear in the foundation catalog or
    /// provider-visible definitions until they have a complete dispatch arm,
    /// permission model, and output shape. If a new tool is being added and this
    /// test fails, the tool must go through full implementation — no placeholders.
    #[test]
    fn registry_invariant_deferred_tools_not_in_foundation_or_provider_visible() {
        const DEFERRED_CANONICAL: &[&str] = &[
            "powershell",
            "cron",
            "cron-create",
            "cron-delete",
            "cron-list",
            "monitor",
            "sleep",
            "browser",
            "remote-trigger",
            "teams",
            "vault",
            "review-artifact",
            "synthetic-output",
            "marketplace",
            "push-notification",
            "schedule-wakeup",
            "enter-worktree",
            "exit-worktree",
        ];

        const DEFERRED_PROVIDER_FACING: &[&str] = &[
            "PowerShell",
            "CronCreate",
            "CronDelete",
            "CronList",
            "Monitor",
            "Sleep",
            "Browser",
            "RemoteTrigger",
            "Teams",
            "Vault",
            "ReviewArtifact",
            "SyntheticOutput",
            "Marketplace",
            "PushNotification",
            "ScheduleWakeup",
            "EnterWorktree",
            "ExitWorktree",
        ];

        let registry = ToolRegistry::foundation();

        let planned_names: std::collections::HashSet<&str> =
            registry.planned().iter().map(|spec| spec.name).collect();

        for &name in DEFERRED_CANONICAL {
            assert!(
                !planned_names.contains(name),
                "deferred tool `{name}` must not appear in foundation catalog \
                 until fully implemented with dispatch, permissions, and output shape"
            );
        }

        let defs = registry.provider_definitions(true, true);
        let provider_names: std::collections::HashSet<String> =
            defs.iter().map(|d| d.name.clone()).collect();

        for &name in DEFERRED_PROVIDER_FACING {
            assert!(
                !provider_names.contains(name),
                "deferred tool `{name}` must not appear in provider-visible definitions \
                 until fully implemented with dispatch, permissions, and output shape"
            );
        }
    }
}
