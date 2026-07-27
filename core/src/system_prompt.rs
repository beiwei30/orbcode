use std::fmt::Write as _;

use orbcode_protocol::ProviderToolDefinition;
use orbcode_protocol::{TurnContext, WorktreeState};

pub(crate) fn build_system_prompt(context: &TurnContext) -> String {
    let mut prompt = format!(
        concat!(
            "You are Orb Code, a terminal coding assistant running inside the user's workspace.\n",
            "Current working directory: {cwd}\n",
            "Git branch: {branch}\n"
        ),
        cwd = context.cwd,
        branch = context.git_branch.as_deref().unwrap_or("unknown"),
    );

    if let Some(default_branch) = context.git_default_branch.as_deref() {
        writeln!(prompt, "Main branch: {default_branch}").expect("writing to String cannot fail");
    }
    if let Some(repo_root) = context.repo_root.as_deref() {
        writeln!(prompt, "Repository root: {repo_root}").expect("writing to String cannot fail");
    }
    if let Some(relative) = context.cwd_relative_to_repo.as_deref() {
        writeln!(prompt, "Relative path from repository root: {relative}")
            .expect("writing to String cannot fail");
    }
    if let Some(state) = context.git_worktree_state {
        if matches!(state, WorktreeState::Detached) {
            prompt.push_str("Note: HEAD is detached.\n");
        } else if matches!(state, WorktreeState::Linked) {
            prompt.push_str("Note: working inside a linked git worktree.\n");
        }
    }

    if let Some(git_status) = context.git_status.as_deref() {
        prompt.push_str("\nGit status snapshot:\n");
        prompt.push_str(git_status);
        prompt.push('\n');
    }

    prompt.push_str(
        concat!(
            "\nYou have direct access to tools for reading files, searching the repository, running shell commands, editing files, and reading MCP resources.\n",
            "When the user asks about the current project, files, code, configuration, or repository state, use tools directly instead of asking them to paste file contents or directory listings.\n",
            "Do not claim that you are limited to a text-only environment when tools are available.\n",
            "Prefer `glob`, `grep`, `file-read`, and `bash` for repository inspection.\n",
            "If the current working directory is a nested subdirectory inside the repository, interpret top-level project paths relative to the repository root rather than blindly relative to the current directory.\n",
            "Do not stop after only stating a plan. If you say you will inspect, search, compare, or evaluate the project, immediately do that work in the same turn using the tools, or provide the final answer in that same turn.\n",
            "For repository and code-analysis tasks, your first assistant response should normally include the needed tool_use blocks in the same response after any brief preamble or thinking. Do not end the turn after only thinking or a short planning sentence.\n",
            "If a tool requires approval, request the tool use anyway; the UI will handle the approval step.\n",
            "Be concise and factual."
        ),
    );

    prompt
}

pub(crate) fn append_dynamic_workflow_planning_section(
    prompt: &mut String,
    tools: &[ProviderToolDefinition],
) {
    if !tools.iter().any(|tool| tool.name == "Workflow") {
        return;
    }

    prompt.push_str(
        concat!(
            "\n\nDynamic workflow planning\n",
            "- Use the Workflow tool when the user's goal is better handled as a durable workflow with multi-step work, independent parallel analysis, staged investigation, or pipeline-shaped work where sub-agent contexts should stay isolated.\n",
            "- Avoid Workflow for single-step questions, simple edits, unclear goals that need clarification, or when the user explicitly asks only to discuss a plan.\n",
            "- If Workflow is appropriate, generate the inline JSON spec and call Workflow in this same turn instead of only describing the plan.\n",
            "- Treat the main agent as the owner of intent recognition, workflow planning, task start, progress observation, and final synthesis.\n",
            "- Treat agent steps as the only executable work units. Use phase, parallel, and pipeline only to organize sub-agent work; use log only as a non-work status marker.\n",
            "- Workflow steps are single-key objects: {\"agent\":{\"description\":\"...\",\"prompt\":\"...\"}}, {\"parallel\":{\"steps\":[...]}}, {\"pipeline\":{\"steps\":[...]}}, {\"phase\":{\"name\":\"...\",\"steps\":[...]}}, or {\"log\":{\"message\":\"...\"}}. Do not use kind/name/run_in_background/subagent_type fields for steps.\n",
            "- The Workflow tool input and `spec` must be JSON objects, not quoted JSON strings. For parallel agent steps, close each child object fully: {\"parallel\":{\"steps\":[{\"agent\":{\"description\":\"task2\",\"prompt\":\"...\"}},{\"agent\":{\"description\":\"task3\",\"prompt\":\"...\"}}]}}.\n",
            "- Make each generated agent prompt self-contained with the local goal, relevant constraints, expected output shape, and any dependency on prior pipeline output.\n",
            "- Use short generated workflow names in the form dynamic:<kebab-name>."
        ),
    );
}

#[cfg(test)]
mod tests {
    use orbcode_protocol::TurnContext;

    use super::{append_dynamic_workflow_planning_section, build_system_prompt};

    #[test]
    fn system_prompt_mentions_repo_root_for_nested_workspace() {
        let prompt = build_system_prompt(&TurnContext {
            cwd: "/repo/orbcode".to_string(),
            repo_root: Some("/repo".to_string()),
            cwd_relative_to_repo: Some("orbcode".to_string()),
            current_date: "2026-04-24".to_string(),
            git_branch: Some("main".to_string()),
            ..Default::default()
        });

        assert!(prompt.contains("Repository root: /repo"));
        assert!(prompt.contains("Relative path from repository root: orbcode"));
        assert!(
            prompt.contains("interpret top-level project paths relative to the repository root")
        );
    }

    #[test]
    fn dynamic_workflow_planning_section_depends_on_visible_workflow_tool() {
        let mut prompt = String::from("base");
        append_dynamic_workflow_planning_section(&mut prompt, &[]);
        assert_eq!(prompt, "base");

        append_dynamic_workflow_planning_section(
            &mut prompt,
            &[orbcode_protocol::ProviderToolDefinition {
                name: "Workflow".to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            }],
        );
        assert!(prompt.contains("Dynamic workflow planning"));
        assert!(prompt.contains("call Workflow in this same turn"));
    }
}
