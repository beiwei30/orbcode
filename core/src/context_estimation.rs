use orbcode_model_provider::ProviderRequest;
use orbcode_protocol::{
    ProviderToolDefinition, TokenUsage, TurnContext, token_count_with_estimation,
};

use crate::overview::ContextCategoryBreakdown;

/// Estimate the per-category token breakdown for a provider request.
///
/// Each category uses the same rough char-based estimator as
/// `token_count_with_estimation` so the relative sizes line up with what
/// `analyzeContextUsage` reports in the TypeScript CLI. Static categories
/// (system prompt, built-in tools, memory, conversation) are populated
/// from the data already in the `ProviderRequest`. The remaining fields
/// (`mcp_tools`, `skills`, `attachments`) stay at zero until the
/// corresponding Rust subsystems land — the field acts as the interface
/// the task callout asked for.
pub(crate) fn estimate_category_breakdown(request: &ProviderRequest) -> ContextCategoryBreakdown {
    let (system_prompt_text, skills_text) = split_preloaded_skills(&request.system_prompt);
    let system_prompt = estimate_text_tokens(system_prompt_text)
        .saturating_add(estimate_context_summary_excluding_memory(&request.context));
    let mut system_tools = 0_u32;
    let mut mcp_tools = 0_u32;
    for tool in &request.tools {
        let tokens = estimate_tool_definition_tokens(tool);
        if is_mcp_tool_name(&tool.name) {
            mcp_tools = mcp_tools.saturating_add(tokens);
        } else {
            system_tools = system_tools.saturating_add(tokens);
        }
    }
    let memory = request
        .context
        .claude_md
        .as_deref()
        .map_or(0, estimate_text_tokens);
    let skills = skills_text.map_or(0, estimate_text_tokens);
    let conversation = token_count_with_estimation(&request.messages);

    ContextCategoryBreakdown {
        system_prompt,
        system_tools,
        mcp_tools,
        memory,
        skills,
        conversation,
        attachments: 0,
        uncategorized: 0,
    }
}

fn estimate_text_tokens(text: &str) -> u32 {
    TokenUsage::from_text("", text).output_tokens
}

fn estimate_tool_definition_tokens(tool: &ProviderToolDefinition) -> u32 {
    let schema =
        serde_json::to_string(&tool.input_schema).unwrap_or_else(|_| tool.input_schema.to_string());
    let serialized = format!("{}\n{}\n{}", tool.name, tool.description, schema);
    estimate_text_tokens(&serialized)
}

fn split_preloaded_skills(system_prompt: &str) -> (&str, Option<&str>) {
    let Some(index) = system_prompt.find("## Preloaded skills") else {
        return (system_prompt, None);
    };
    let (base, skills) = system_prompt.split_at(index);
    (base.trim_end(), Some(skills))
}

fn is_mcp_tool_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("mcp__") else {
        return false;
    };
    let Some((server, tool)) = rest.split_once("__") else {
        return false;
    };
    !server.is_empty() && !tool.is_empty()
}

/// Build a rough text representation of every non-memory entry in the
/// per-turn context message so it can be folded into the `system_prompt`
/// category. Mirrors the entries assembled by
/// `anthropic_user_context_message` minus `claudeMd`, which is tracked
/// separately as `memory`.
fn estimate_context_summary_excluding_memory(context: &TurnContext) -> u32 {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("Today's date is {}.", context.current_date));
    parts.push(format!("The current working directory is {}.", context.cwd));
    if !context.additional_directories.is_empty() {
        parts.push(format!(
            "Additional working directories are: {}.",
            context.additional_directories.join(", ")
        ));
    }
    if let Some(repo_root) = context.repo_root.as_deref() {
        parts.push(format!("The repository root is {repo_root}."));
    }
    if let Some(relative) = context.cwd_relative_to_repo.as_deref() {
        parts.push(format!(
            "The current working directory is the `{relative}` subdirectory inside the repository."
        ));
    }
    if let Some(branch) = context.git_branch.as_deref() {
        parts.push(format!("The current git branch is {branch}."));
    }
    if let Some(default_branch) = context.git_default_branch.as_deref() {
        parts.push(format!("Main branch: {default_branch}"));
    }
    if let Some(state) = context.git_worktree_state {
        parts.push(format!("Worktree state: {}.", state.as_label()));
    }
    if let Some(user) = context.git_user.as_deref() {
        parts.push(format!("Git user: {user}"));
    }
    if let Some(remote) = context.git_remote.as_deref() {
        parts.push(format!("Git remote: {remote}"));
    }
    if let Some(status) = context.git_status.as_deref() {
        parts.push(format!("Status:\n{status}"));
    }
    if let Some(commits) = context.git_recent_commits.as_deref() {
        parts.push(format!("Recent commits:\n{commits}"));
    }
    for detail in &context.additional_directory_details {
        parts.push(format!("Additional dir: {}", detail.path));
        if let Some(branch) = detail.git_branch.as_deref() {
            parts.push(format!("  branch: {branch}"));
        }
    }
    if let Some(trusted) = context.trusted_project {
        parts.push(format!("Project trust: {trusted}"));
    }
    estimate_text_tokens(&parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use orbcode_model_provider::ProviderRequest;
    use orbcode_protocol::{ProviderToolDefinition, TurnContext};
    use serde_json::json;

    use super::estimate_category_breakdown;

    fn test_context(claude_md: Option<&str>) -> TurnContext {
        TurnContext {
            cwd: "/repo".to_string(),
            repo_root: Some("/repo".to_string()),
            current_date: "2026-05-25".to_string(),
            git_branch: Some("main".to_string()),
            claude_md: claude_md.map(str::to_string),
            ..Default::default()
        }
    }

    fn test_request(claude_md: Option<&str>) -> ProviderRequest {
        ProviderRequest {
            session_id: "session-test".to_string(),
            prompt: String::new(),
            system_prompt: "You are Orb Code.".to_string(),
            context: test_context(claude_md),
            messages: Vec::new(),
            tools: vec![ProviderToolDefinition {
                name: "bash".to_string(),
                description: "Run shell commands".to_string(),
                input_schema: json!({"type": "object"}),
            }],
            model: "stub-model".to_string(),
            base_url: String::new(),
            api_key: None,
            auth_token: None,
            disable_thinking: false,
            effort: None,
            options: orbcode_model_provider::ProviderRequestOptions::default(),
        }
    }

    fn mcp_tool() -> ProviderToolDefinition {
        ProviderToolDefinition {
            name: "mcp__docs__search".to_string(),
            description: "Search docs".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }

    #[test]
    fn populates_static_categories() {
        let breakdown = estimate_category_breakdown(&test_request(None));
        assert!(breakdown.system_prompt > 0, "system prompt has tokens");
        assert!(breakdown.system_tools > 0, "tool definitions counted");
        assert_eq!(breakdown.memory, 0);
        assert_eq!(breakdown.conversation, 0);
        assert_eq!(breakdown.mcp_tools, 0);
        assert_eq!(breakdown.skills, 0);
        assert_eq!(breakdown.attachments, 0);
        assert_eq!(breakdown.uncategorized, 0);
    }

    #[test]
    fn separates_memory_from_system_prompt() {
        let without_memory = estimate_category_breakdown(&test_request(None));
        let memory_body = "Always wear gloves.".repeat(40);
        let with_memory = estimate_category_breakdown(&test_request(Some(&memory_body)));

        assert!(with_memory.memory > 0);
        assert_eq!(without_memory.system_prompt, with_memory.system_prompt);
        assert!(with_memory.total() > without_memory.total());
    }

    #[test]
    fn separates_mcp_tools_and_preloaded_skills() {
        let mut request = test_request(None);
        request.tools.push(mcp_tool());
        request
            .system_prompt
            .push_str("\n\n## Preloaded skills\n\n### Skill: docs\nUse docs.");

        let breakdown = estimate_category_breakdown(&request);

        assert!(breakdown.system_tools > 0);
        assert!(breakdown.mcp_tools > 0);
        assert!(breakdown.skills > 0);
    }
}
