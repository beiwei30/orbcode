use std::sync::{Arc, atomic::AtomicBool};
use std::time::Instant;

use orbcode_config::{AgentDefinition, PermissionMode, canonical_tool_name};
use orbcode_model_provider::ProviderRequest;
use orbcode_protocol::{StreamEvent, TokenUsage, ToolUseCompletionKind};
use orbcode_session_store::agent_tool_result_metadata;
use orbcode_tools::{AgentToolInput, SkillDefinition};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{SessionManager, session_background_agent::shutdown_child_mcp_registry};
use crate::{
    CoreError,
    tool_flow::{
        BufferedToolResult, BufferedToolUseCompletion, INTERRUPTED_TOOL_RESULT, ToolUseOutcome,
    },
};

pub(super) enum AgentLoopOutcome {
    Completed {
        final_text: String,
        total_tool_uses: u64,
        usage: TokenUsage,
    },
    Cancelled,
}

struct PreparedAgentToolInvocation {
    agent: AgentToolInput,
    started: Instant,
    agent_id: String,
    child_session_id: String,
    agent_type: String,
    agent_definition: Option<AgentDefinition>,
    resolved_model: String,
    permission_mode: Option<PermissionMode>,
}

impl SessionManager {
    pub(super) async fn invoke_agent_tool_and_append_result(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
        allow_tools: bool,
        allow_network: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<ToolUseOutcome, CoreError> {
        let invocation = match self
            .prepare_agent_tool_invocation(session_id, tool_use_id, tool_input)
            .await
        {
            Ok(invocation) => invocation,
            Err(error) => {
                // A malformed Agent input must yield an error tool_result (like
                // normal tools and Workflow), not a fatal CoreError that aborts
                // the whole turn and leaves the tool_use unanswered.
                return self
                    .append_buffered_agent_tool_completion(
                        session_id,
                        agent_input_error_completion(tool_use_id, &error),
                        tx,
                    )
                    .await;
            }
        };

        if invocation.agent.run_in_background {
            return self
                .start_background_agent_task(
                    session_id,
                    tool_use_id,
                    invocation.agent,
                    invocation.agent_id,
                    invocation.agent_type,
                    invocation.agent_definition,
                    invocation.child_session_id,
                    invocation.resolved_model,
                    invocation.permission_mode,
                    allow_tools,
                    allow_network,
                    tx,
                )
                .await;
        }

        let completion = self
            .invoke_foreground_agent_tool(
                session_id,
                tool_use_id,
                invocation,
                allow_tools,
                allow_network,
                tx,
                cancel_flag,
            )
            .await?;
        self.append_buffered_agent_tool_completion(session_id, completion, tx)
            .await
    }

    pub(super) async fn invoke_agent_tool_and_buffer_result(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
        allow_tools: bool,
        allow_network: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<BufferedToolUseCompletion, CoreError> {
        let invocation = match self
            .prepare_agent_tool_invocation(session_id, tool_use_id, tool_input)
            .await
        {
            Ok(invocation) => invocation,
            // Return an error result to buffer rather than a fatal CoreError, so
            // a malformed Agent input in a parallel round doesn't abort the turn.
            Err(error) => return Ok(agent_input_error_completion(tool_use_id, &error)),
        };
        if invocation.agent.run_in_background {
            return Err(CoreError::Tool(
                "background Agent tool use cannot be buffered".to_string(),
            ));
        }
        self.invoke_foreground_agent_tool(
            session_id,
            tool_use_id,
            invocation,
            allow_tools,
            allow_network,
            tx,
            cancel_flag,
        )
        .await
    }

    async fn prepare_agent_tool_invocation(
        &self,
        session_id: &str,
        tool_use_id: &str,
        tool_input: &str,
    ) -> Result<PreparedAgentToolInvocation, CoreError> {
        let agent = serde_json::from_str::<AgentToolInput>(tool_input)
            .map_err(|error| CoreError::Tool(format!("invalid Agent input: {error}")))?;
        let started = Instant::now();
        let agent_id = format!("agent-{}", Uuid::new_v4().simple());
        let child_session_id = format!("{session_id}:{agent_id}");
        let agent_type = agent
            .subagent_type
            .as_deref()
            .unwrap_or("general-purpose")
            .to_string();
        let agent_definition = self.lookup_agent_definition(&agent_type);
        let resolved_model = agent_definition
            .as_ref()
            .and_then(|definition| definition.model.clone())
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("inherit"))
            .unwrap_or_else(|| {
                self.config
                    .provider_model_resolution(self.config.default_provider)
                    .request_model
            });
        let permission_mode = agent_definition
            .as_ref()
            .and_then(|definition| definition.permission_mode);

        let config = self.effective_config();
        let _ = self
            .child_session_store
            .start(orbcode_session_store::StartChildSessionInput {
                child_session_id: child_session_id.clone(),
                parent_session_id: session_id.to_string(),
                agent_id: agent_id.clone(),
                agent_type: agent_type.clone(),
                source_tool_use_id: tool_use_id.to_string(),
                cwd: config.cwd.display().to_string(),
                model: Some(resolved_model.clone()),
                permission_mode: permission_mode.map(|mode| mode.as_str().to_string()),
                prompt: agent.prompt.clone(),
            })
            .await;

        Ok(PreparedAgentToolInvocation {
            agent,
            started,
            agent_id,
            child_session_id,
            agent_type,
            agent_definition,
            resolved_model,
            permission_mode,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn invoke_foreground_agent_tool(
        &self,
        session_id: &str,
        tool_use_id: &str,
        invocation: PreparedAgentToolInvocation,
        allow_tools: bool,
        allow_network: bool,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<BufferedToolUseCompletion, CoreError> {
        // Apply the agent's declared `permissionMode` to the child loop. `plan`
        // disables tool execution; the always-on modes enable it; `default`
        // inherits the ambient grant. Without this, permissionMode was recorded
        // but never enforced.
        let (allow_tools, allow_network) =
            apply_agent_permission_mode(invocation.permission_mode, allow_tools, allow_network);

        let preloaded_skills = self
            .preload_agent_skills(
                invocation.agent_definition.as_ref(),
                session_id,
                tool_use_id,
                tx,
            )
            .await;

        let child_mcp = self
            .maybe_create_child_mcp(invocation.agent_definition.as_ref())
            .await;
        let runner = self.agent_loop_runner(child_mcp.as_ref());
        let loop_result = runner
            .run_agent_session_loop(
                session_id,
                tool_use_id,
                &invocation.agent,
                &invocation.agent_id,
                &invocation.agent_type,
                invocation.agent_definition.as_ref(),
                &preloaded_skills,
                &invocation.child_session_id,
                allow_tools,
                allow_network,
                false,
                tx,
                cancel_flag,
            )
            .await;
        if let Some(ref child_registry) = child_mcp {
            shutdown_child_mcp_registry(child_registry).await;
        }

        let outcome = match loop_result {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = self
                    .child_session_store
                    .fail(&invocation.child_session_id, &error.to_string())
                    .await;
                return Err(error);
            }
        };

        match outcome {
            AgentLoopOutcome::Cancelled => {
                let _ = self
                    .child_session_store
                    .cancel(&invocation.child_session_id)
                    .await;
                Ok(BufferedToolUseCompletion {
                    outcome: ToolUseOutcome::Cancelled,
                    result: BufferedToolResult {
                        tool_use_id: tool_use_id.to_string(),
                        tool_name: "Agent".to_string(),
                        content: INTERRUPTED_TOOL_RESULT.to_string(),
                        is_error: true,
                        metadata: None,
                        completion_kind: ToolUseCompletionKind::Interrupted,
                    },
                })
            }
            AgentLoopOutcome::Completed {
                final_text,
                total_tool_uses,
                usage,
            } => {
                let metadata = agent_tool_result_metadata(
                    &invocation.agent.prompt,
                    invocation.agent.subagent_type.as_deref(),
                    &final_text,
                    total_tool_uses,
                    invocation.started.elapsed().as_millis() as u64,
                    &usage,
                );
                let _ = self
                    .child_session_store
                    .complete(&invocation.child_session_id)
                    .await;
                Ok(BufferedToolUseCompletion {
                    outcome: ToolUseOutcome::Continue,
                    result: BufferedToolResult {
                        tool_use_id: tool_use_id.to_string(),
                        tool_name: "Agent".to_string(),
                        content: final_text,
                        is_error: false,
                        metadata: Some(metadata),
                        completion_kind: ToolUseCompletionKind::Success,
                    },
                })
            }
        }
    }

    async fn append_buffered_agent_tool_completion(
        &self,
        session_id: &str,
        completion: BufferedToolUseCompletion,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<ToolUseOutcome, CoreError> {
        let result = completion.result;
        self.append_tool_result_message(
            session_id,
            &result.tool_use_id,
            result.content,
            result.is_error,
            result.metadata,
            tx,
        )
        .await?;
        self.emit_tool_use_completed(
            session_id,
            &result.tool_use_id,
            &result.tool_name,
            result.completion_kind,
            tx,
        );
        Ok(completion.outcome)
    }
}

/// Build an error tool_result completion for a malformed Agent tool input, so
/// the model receives an error result (and can retry) instead of the turn
/// aborting with an unanswered tool_use.
fn agent_input_error_completion(tool_use_id: &str, error: &CoreError) -> BufferedToolUseCompletion {
    BufferedToolUseCompletion {
        outcome: ToolUseOutcome::Continue,
        result: BufferedToolResult {
            tool_use_id: tool_use_id.to_string(),
            tool_name: "Agent".to_string(),
            content: error.to_string(),
            is_error: true,
            metadata: None,
            completion_kind: ToolUseCompletionKind::ExecutionFailed,
        },
    }
}

/// Resolve the effective `(allow_tools, allow_network)` for a sub-agent loop
/// from its declared `permissionMode`, falling back to the ambient grant when
/// the mode does not dictate an override (`default`).
pub(super) fn apply_agent_permission_mode(
    permission_mode: Option<PermissionMode>,
    allow_tools: bool,
    allow_network: bool,
) -> (bool, bool) {
    let allow_tools = permission_mode
        .and_then(PermissionMode::default_allow_tools)
        .unwrap_or(allow_tools);
    let allow_network = permission_mode
        .and_then(PermissionMode::default_allow_network)
        .unwrap_or(allow_network);
    (allow_tools, allow_network)
}

/// Apply the agent definition's prompt, model, and tool constraints to the
/// outbound provider request built for the child agent loop.
pub(super) fn apply_agent_definition_to_request(
    request: &mut ProviderRequest,
    definition: &AgentDefinition,
) {
    if !definition.prompt.trim().is_empty() {
        request.system_prompt = definition.prompt.clone();
    }
    if let Some(model) = definition.model.as_deref() {
        let trimmed = model.trim();
        if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("inherit") {
            request.model = trimmed.to_string();
        }
    }
    if let Some(allowed) = definition.tools.as_ref() {
        let allowed: Vec<String> = allowed
            .iter()
            .map(|name| canonical_tool_name(name))
            .collect();
        request.tools.retain(|tool| {
            allowed
                .iter()
                .any(|name| name == &canonical_tool_name(&tool.name))
        });
    }
    if let Some(disallowed) = definition.disallowed_tools.as_ref() {
        let disallowed: Vec<String> = disallowed
            .iter()
            .map(|name| canonical_tool_name(name))
            .collect();
        request.tools.retain(|tool| {
            !disallowed
                .iter()
                .any(|name| name == &canonical_tool_name(&tool.name))
        });
    }
    if let Some(allowed_servers) = definition.mcp_server_names.as_ref() {
        request.tools.retain(|tool| {
            let Some(rest) = tool.name.strip_prefix("mcp__") else {
                return true;
            };
            let server = rest.split("__").next().unwrap_or("");
            allowed_servers.iter().any(|name| name == server)
        });
    }
}

/// Whether the agent definition permits *executing* `tool_name`. This mirrors
/// the model-visible filtering in [`apply_agent_definition_to_request`] but is
/// enforced at invocation time: the tool-visibility filter only controls what
/// the model is *told* about, so a model that emits a tool outside its
/// allowlist anyway (jailbreak, hallucination, replayed history) must still be
/// blocked here — otherwise the sandbox is advisory only.
pub(super) fn agent_definition_permits_tool(definition: &AgentDefinition, tool_name: &str) -> bool {
    let canonical = canonical_tool_name(tool_name);
    if let Some(allowed) = definition.tools.as_ref()
        && !allowed
            .iter()
            .any(|name| canonical_tool_name(name) == canonical)
    {
        return false;
    }
    if let Some(disallowed) = definition.disallowed_tools.as_ref()
        && disallowed
            .iter()
            .any(|name| canonical_tool_name(name) == canonical)
    {
        return false;
    }
    if let Some(allowed_servers) = definition.mcp_server_names.as_ref()
        && let Some(rest) = tool_name.strip_prefix("mcp__")
    {
        let server = rest.split("__").next().unwrap_or("");
        if !allowed_servers.iter().any(|name| name == server) {
            return false;
        }
    }
    true
}

/// Append preloaded skill instructions to the child agent's system prompt.
/// Skills are injected as system context — never as user messages — so they
/// stay scoped to the child loop and never enter the model-visible message
/// history of either the parent or child session.
pub(super) fn apply_preloaded_skills_to_request(
    request: &mut ProviderRequest,
    skills: &[SkillDefinition],
) {
    if skills.is_empty() {
        return;
    }
    let mut section = String::from("\n\n## Preloaded skills\n");
    for skill in skills {
        section.push_str("\n### Skill: ");
        section.push_str(&skill.name);
        section.push('\n');
        if let Some(description) = skill.description.as_deref()
            && !description.trim().is_empty()
        {
            section.push_str(description.trim());
            section.push('\n');
        }
        if !skill.body.trim().is_empty() {
            section.push('\n');
            section.push_str(skill.body.trim());
            section.push('\n');
        }
    }
    request.system_prompt.push_str(&section);
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_config::AgentDefinition;
    use orbcode_model_provider::{ProviderRequest, ProviderRequestOptions};
    use orbcode_protocol::{ProviderToolDefinition, TurnContext};
    use serde_json::json;

    fn provider_request_with_tools(tool_names: &[&'static str]) -> ProviderRequest {
        ProviderRequest {
            session_id: "session".to_string(),
            prompt: String::new(),
            system_prompt: "default system prompt".to_string(),
            context: TurnContext {
                cwd: "/repo".to_string(),
                current_date: "2026-05-25".to_string(),
                ..Default::default()
            },
            messages: Vec::new(),
            tools: tool_names
                .iter()
                .map(|name| ProviderToolDefinition {
                    name: (*name).to_string(),
                    description: String::new(),
                    input_schema: json!({}),
                })
                .collect(),
            model: "default-model".to_string(),
            base_url: String::new(),
            api_key: None,
            auth_token: None,
            disable_thinking: false,
            effort: None,
            options: ProviderRequestOptions::default(),
        }
    }

    fn definition_with(
        tools: Option<Vec<&str>>,
        disallowed: Option<Vec<&str>>,
        model: Option<&str>,
        prompt: &str,
    ) -> AgentDefinition {
        AgentDefinition {
            agent_type: "Explore".to_string(),
            description: "Read-only exploration agent.".to_string(),
            prompt: prompt.to_string(),
            tools: tools.map(|values| values.into_iter().map(String::from).collect()),
            disallowed_tools: disallowed
                .map(|values| values.into_iter().map(String::from).collect()),
            model: model.map(String::from),
            permission_mode: None,
            skills: Vec::new(),
            mcp_server_names: None,
            hooks: std::collections::BTreeMap::new(),
            source: orbcode_config::AgentSource::ProjectSettings,
            path: None,
        }
    }

    #[test]
    fn agent_definition_permits_tool_enforces_allowlist_at_execution() {
        // Read-only agent: only Read/Grep are permitted; a model-emitted Bash
        // (outside the allowlist) must be blocked even though the request
        // filter never sent Bash to the model.
        let definition = definition_with(Some(vec!["Read", "Grep"]), None, None, "explore");
        assert!(agent_definition_permits_tool(&definition, "Read"));
        assert!(agent_definition_permits_tool(&definition, "Grep"));
        assert!(!agent_definition_permits_tool(&definition, "Bash"));
        assert!(!agent_definition_permits_tool(&definition, "Write"));

        // Disallowed list blocks a tool even when no allowlist is set.
        let definition = definition_with(None, Some(vec!["Bash"]), None, "agent");
        assert!(agent_definition_permits_tool(&definition, "Read"));
        assert!(!agent_definition_permits_tool(&definition, "Bash"));

        // MCP server scoping: only tools from allowed servers pass.
        let mut definition = definition_with(None, None, None, "agent");
        definition.mcp_server_names = Some(vec!["docs".to_string()]);
        assert!(agent_definition_permits_tool(
            &definition,
            "mcp__docs__search"
        ));
        assert!(!agent_definition_permits_tool(
            &definition,
            "mcp__secrets__read"
        ));
        assert!(agent_definition_permits_tool(&definition, "Read"));
    }

    #[test]
    fn apply_agent_permission_mode_maps_mode_to_grant() {
        // Plan mode disables tool execution regardless of the ambient grant.
        assert_eq!(
            apply_agent_permission_mode(Some(PermissionMode::Plan), true, true),
            (false, true)
        );
        // Bypass enables tools and network.
        assert_eq!(
            apply_agent_permission_mode(Some(PermissionMode::BypassPermissions), false, false),
            (true, true)
        );
        // Default / unspecified inherit the ambient grant.
        assert_eq!(
            apply_agent_permission_mode(Some(PermissionMode::Default), true, false),
            (true, false)
        );
        assert_eq!(
            apply_agent_permission_mode(None, true, false),
            (true, false)
        );
    }

    #[test]
    fn applies_prompt_model_and_tool_whitelist() {
        let mut request = provider_request_with_tools(&["Bash", "Read", "Grep", "Write"]);
        let definition = definition_with(
            Some(vec!["Read", "Grep"]),
            None,
            Some("claude-haiku-4-5"),
            "You are Explore. Read only.",
        );
        apply_agent_definition_to_request(&mut request, &definition);

        assert_eq!(request.system_prompt, "You are Explore. Read only.");
        assert_eq!(request.model, "claude-haiku-4-5");
        let names: Vec<&str> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(names, vec!["Read", "Grep"]);
    }

    #[test]
    fn star_tools_keeps_all_tools_and_disallowed_filters_them() {
        let mut request = provider_request_with_tools(&["Bash", "Read", "Grep"]);
        let definition = definition_with(
            None, // None == all tools
            Some(vec!["Bash"]),
            None,
            "prompt",
        );
        apply_agent_definition_to_request(&mut request, &definition);

        let names: Vec<&str> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(names, vec!["Read", "Grep"]);
    }

    #[test]
    fn inherit_model_does_not_override_request_model() {
        let mut request = provider_request_with_tools(&["Read"]);
        let definition = definition_with(None, None, Some("INHERIT"), "prompt");
        apply_agent_definition_to_request(&mut request, &definition);

        assert_eq!(request.model, "default-model");
    }

    #[test]
    fn mcp_server_names_filter_retains_only_listed_servers() {
        let mut request = provider_request_with_tools(&[
            "Read",
            "mcp__context7__resolve-library-id",
            "mcp__context7__query-docs",
            "mcp__github__create_issue",
        ]);
        let mut definition = definition_with(None, None, None, "prompt");
        definition.mcp_server_names = Some(vec!["context7".to_string()]);
        apply_agent_definition_to_request(&mut request, &definition);

        let names: Vec<&str> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "Read",
                "mcp__context7__resolve-library-id",
                "mcp__context7__query-docs",
            ]
        );
    }

    #[test]
    fn mcp_server_names_empty_filter_drops_all_mcp_tools() {
        let mut request = provider_request_with_tools(&[
            "Read",
            "mcp__context7__resolve-library-id",
            "mcp__github__create_issue",
        ]);
        let mut definition = definition_with(None, None, None, "prompt");
        definition.mcp_server_names = Some(Vec::new());
        apply_agent_definition_to_request(&mut request, &definition);

        let names: Vec<&str> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(names, vec!["Read"]);
    }

    #[test]
    fn mcp_server_names_none_preserves_all_mcp_tools() {
        let mut request = provider_request_with_tools(&[
            "Read",
            "mcp__context7__resolve-library-id",
            "mcp__github__create_issue",
        ]);
        let definition = definition_with(None, None, None, "prompt");
        assert!(definition.mcp_server_names.is_none());
        apply_agent_definition_to_request(&mut request, &definition);

        let names: Vec<&str> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "Read",
                "mcp__context7__resolve-library-id",
                "mcp__github__create_issue",
            ]
        );
    }

    #[test]
    fn empty_prompt_does_not_override_default_system_prompt() {
        let mut request = provider_request_with_tools(&["Read"]);
        let definition = definition_with(None, None, None, "   ");
        apply_agent_definition_to_request(&mut request, &definition);

        assert_eq!(request.system_prompt, "default system prompt");
    }

    fn skill(name: &str, description: Option<&str>, body: &str) -> SkillDefinition {
        SkillDefinition {
            name: name.to_string(),
            description: description.map(std::string::ToString::to_string),
            path: std::path::PathBuf::from(format!("/skills/{name}/SKILL.md")),
            body: body.to_string(),
            source: orbcode_tools::SkillSource::User,
            ..SkillDefinition::default()
        }
    }

    #[test]
    fn empty_skill_list_leaves_system_prompt_untouched() {
        let mut request = provider_request_with_tools(&["Read"]);
        let original = request.system_prompt.clone();
        apply_preloaded_skills_to_request(&mut request, &[]);
        assert_eq!(request.system_prompt, original);
    }

    #[test]
    fn skill_body_is_appended_to_system_prompt_with_header() {
        let mut request = provider_request_with_tools(&["Read"]);
        let skills = vec![
            skill(
                "rust-patterns",
                Some("Idiomatic Rust"),
                "Use ? for error propagation.\nPrefer iterators.",
            ),
            skill("review-helper", None, "Be pedantic about ownership."),
        ];
        apply_preloaded_skills_to_request(&mut request, &skills);

        assert!(request.system_prompt.starts_with("default system prompt"));
        assert!(request.system_prompt.contains("## Preloaded skills"));
        assert!(request.system_prompt.contains("### Skill: rust-patterns"));
        assert!(request.system_prompt.contains("Idiomatic Rust"));
        assert!(
            request
                .system_prompt
                .contains("Use ? for error propagation.")
        );
        assert!(request.system_prompt.contains("### Skill: review-helper"));
        assert!(
            request
                .system_prompt
                .contains("Be pedantic about ownership.")
        );
    }

    #[test]
    fn skill_preload_preserves_caller_order() {
        let mut request = provider_request_with_tools(&["Read"]);
        let skills = vec![
            skill("beta", None, "beta body"),
            skill("alpha", None, "alpha body"),
        ];
        apply_preloaded_skills_to_request(&mut request, &skills);
        let beta_idx = request
            .system_prompt
            .find("### Skill: beta")
            .expect("beta header");
        let alpha_idx = request
            .system_prompt
            .find("### Skill: alpha")
            .expect("alpha header");
        assert!(
            beta_idx < alpha_idx,
            "skills must keep caller-supplied order"
        );
    }

    #[test]
    fn mcp_server_names_none_means_no_child_registry_needed() {
        let definition = definition_with(None, None, None, "prompt");
        assert!(
            definition.mcp_server_names.is_none(),
            "None means inherit parent MCP tools"
        );
    }

    #[test]
    fn mcp_server_names_some_empty_drops_all_mcp_tools_from_request() {
        let mut request = provider_request_with_tools(&[
            "Read",
            "mcp__context7__resolve-library-id",
            "mcp__github__create_issue",
        ]);
        let mut definition = definition_with(None, None, None, "prompt");
        definition.mcp_server_names = Some(Vec::new());
        apply_agent_definition_to_request(&mut request, &definition);

        let names: Vec<&str> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["Read"],
            "empty mcp_server_names drops all MCP tools"
        );
    }

    #[test]
    fn mcp_server_names_filter_only_keeps_listed_server_tools() {
        let mut request = provider_request_with_tools(&[
            "Read",
            "Bash",
            "mcp__context7__resolve-library-id",
            "mcp__context7__query-docs",
            "mcp__github__create_issue",
            "mcp__slack__post_message",
        ]);
        let mut definition = definition_with(None, None, None, "prompt");
        definition.mcp_server_names = Some(vec!["context7".to_string(), "slack".to_string()]);
        apply_agent_definition_to_request(&mut request, &definition);

        let names: Vec<&str> = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "Read",
                "Bash",
                "mcp__context7__resolve-library-id",
                "mcp__context7__query-docs",
                "mcp__slack__post_message",
            ],
            "only context7 and slack MCP tools survive, github is removed"
        );
    }
}
