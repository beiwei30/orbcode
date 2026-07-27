#![allow(clippy::if_same_then_else)]

mod bash_allow;
mod bash_ast;
mod bash_deny;
mod bash_stdin;
mod command_helpers;
mod crontab;
mod parser;
mod path_rules;
mod pattern_matching;
mod shell_bodies;
#[cfg(test)]
mod tests;
mod wrapper_spec;
mod wrapper_strip;
mod wrappers;

use bash_allow::bash_atomic_command_matches_allow_rule;
pub use bash_allow::{bash_command_allowed_by_rules, suggested_bash_permission_rules};
use bash_deny::bash_command_matches_deny_rule;
use wrappers::container_cli::{container_cli_argv_command_body, kubectl_exec_argv_command_body};
use wrappers::editor_env::env_assignment_command_string_bodies;
use wrappers::package_runner::{
    bun_exec_argv_command_body, conda_run_argv_command_body, direnv_exec_argv_command_body,
    entr_argv_command_body, guix_shell_argv_command_body, nix_cli_command_argv_body,
    nix_shell_run_command_body, npm_exec_argv_command_body, npm_exec_command_string_body,
    pnpm_exec_argv_command_body, python_project_runner_argv_command_body,
    ruby_project_runner_argv_command_body, screen_argv_command_body, watchexec_argv_command_body,
    yarn_exec_argv_command_body,
};
use wrappers::pager::{git_pager_command_config_body, man_pager_command_option_body};
use wrappers::remote_shell::{
    openssh_transfer_command_string_bodies, rsync_remote_shell_command_body,
    ssh_option_command_string_bodies, ssh_remote_command_body, sshpass_argv_command_bodies,
    tar_command_option_bodies,
};
use wrappers::vcs_cmd::{
    git_askpass_command_config_body, git_bisect_run_command_body,
    git_credential_helper_command_bodies, git_difftool_extcmd_command_bodies,
    git_editor_command_config_body, git_external_diff_command_config_body,
    git_filter_branch_command_bodies, git_rebase_exec_command_bodies, git_shell_alias_command_body,
    git_ssh_command_config_body, git_submodule_foreach_command_body, git_tool_command_config_body,
    hg_editor_command_config_body, hg_ssh_command_config_body,
};

pub use path_rules::tool_path_allowed_by_additional_directory;

use serde::Deserialize;

use command_helpers::{
    find_exec_command_bodies, parallel_command_bodies, socat_exec_command_bodies,
    socat_shell_command_bodies,
};
use path_rules::extract_path_targets_from_tool_input;
use pattern_matching::{bash_command_matches_pattern, path_matches_pattern};
use pattern_matching::{
    has_unescaped_wildcard, is_bare_shell_prefix, is_bash_prefix_token_like,
    is_bash_subcommand_like, unescape_rule_literal,
};
use shell_bodies::{
    eval_command_string_body, flock_command_string_body, is_shell_combined_command_option,
    is_shell_command_token, is_shell_flag_option, runuser_command_string_body,
    script_command_string_body, sg_command_string_body, shell_command_string_body,
    su_command_string_body, tmux_command_string_body, trap_command_string_body,
};
use wrapper_strip::{strip_bash_wrappers, strip_bash_wrappers_with_shell_command_strings};

#[derive(Deserialize)]
struct BashToolInput {
    command: Option<String>,
    cmd: Option<String>,
    script: Option<String>,
}

#[derive(Deserialize)]
struct McpAdapterInput {
    server_id: Option<String>,
    server: Option<String>,
    tool_name: Option<String>,
    tool: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PermissionRule {
    pub raw: String,
    pub tool_name: String,
    pub rule_content: Option<String>,
}

impl PermissionRule {
    pub fn parse(raw: &str) -> Self {
        parser::parse(raw)
    }

    pub fn for_tool(tool_name: &str, raw: &str) -> Self {
        let parsed = Self::parse(raw);
        if parsed.rule_content.is_some()
            || tool_name_matches_rule(tool_name, &parsed.tool_name)
            || is_mcp_permission_rule_name(&parsed.tool_name)
        {
            parsed
        } else {
            Self {
                raw: raw.trim().to_string(),
                tool_name: canonical_tool_name(tool_name),
                rule_content: Some(raw.trim().to_string()),
            }
        }
    }

    pub fn matches_tool_wide(&self, tool_name: &str) -> bool {
        self.rule_content.is_none() && tool_name_matches_rule(tool_name, &self.tool_name)
    }

    pub fn matches_tool_call(&self, tool_name: &str, tool_input: &str) -> bool {
        self.matches_tool_call_with_mode(tool_name, tool_input, PermissionRuleMatchMode::Allow)
    }

    pub fn matches_tool_call_with_mode(
        &self,
        tool_name: &str,
        tool_input: &str,
        mode: PermissionRuleMatchMode,
    ) -> bool {
        let matches_direct = tool_name_matches_rule(tool_name, &self.tool_name);
        let mcp_target = if matches_direct {
            None
        } else {
            mcp_permission_target(tool_name, tool_input)
        };

        if !matches_direct
            && !mcp_target
                .as_deref()
                .is_some_and(|target| tool_name_matches_rule(target, &self.tool_name))
        {
            return false;
        }

        let Some(content) = &self.rule_content else {
            return true;
        };

        if canonical_tool_name(tool_name) == "bash" {
            return extract_command_from_tool_input(tool_input)
                .is_some_and(|command| bash_command_matches_rule(&command, content, mode));
        }

        extract_path_targets_from_tool_input(tool_name, tool_input)
            .iter()
            .any(|target| path_matches_pattern(target, content))
    }
}

pub fn normalize_permission_rule_for_edit(raw: &str) -> Result<String, String> {
    parser::normalize_for_edit(raw)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionRuleMatchMode {
    Allow,
    Deny,
}

pub fn canonical_tool_name(name: &str) -> String {
    match name.trim() {
        "Agent" | "agent" | "Task" | "task" => "Agent".to_string(),
        "bash" | "Bash" => "bash".to_string(),
        "file-read" | "Read" | "read" => "file-read".to_string(),
        "file-edit" | "Edit" | "edit" => "file-edit".to_string(),
        "file-write" | "Write" | "write" => "file-write".to_string(),
        "file" | "File" => "file".to_string(),
        "glob" | "Glob" => "glob".to_string(),
        "grep" | "Grep" => "grep".to_string(),
        "notebook-edit" | "NotebookEdit" => "notebook-edit".to_string(),
        "web-fetch" | "WebFetch" => "web-fetch".to_string(),
        "web-search" | "WebSearch" => "web-search".to_string(),
        "ask-user-question" | "AskUserQuestion" => "ask-user-question".to_string(),
        "todo-write" | "TodoWrite" => "todo-write".to_string(),
        "task-create" | "TaskCreate" => "task-create".to_string(),
        "task-get" | "TaskGet" => "task-get".to_string(),
        "task-list" | "TaskList" => "task-list".to_string(),
        "task-update" | "TaskUpdate" => "task-update".to_string(),
        "task-output" | "TaskOutput" | "AgentOutputTool" | "BashOutputTool" => {
            "task-output".to_string()
        }
        "task-stop" | "TaskStop" | "KillShell" => "task-stop".to_string(),
        "enter-plan-mode" | "EnterPlanMode" => "enter-plan-mode".to_string(),
        "exit-plan-mode" | "ExitPlanMode" => "exit-plan-mode".to_string(),
        "verify-plan-execution" | "VerifyPlanExecution" => "verify-plan-execution".to_string(),
        "skill" | "Skill" => "skill".to_string(),
        "tool-search" | "ToolSearch" => "tool-search".to_string(),
        "lsp" | "LSP" => "lsp".to_string(),
        other => other.to_string(),
    }
}

/// Build the synthetic `mcp__{server}__{tool}` target string from a legacy
/// adapter call. Used by slash commands and UI labels that still construct
/// adapter-style names (`CallMcpTool`, `ReadMcpResourceTool`, ...) so a single
/// canonical `mcp__server__tool` rule covers both paths.
pub fn mcp_permission_target(tool_name: &str, tool_input: &str) -> Option<String> {
    let parsed: McpAdapterInput = serde_json::from_str(tool_input).ok()?;
    let server_id = parsed.server_id.or(parsed.server)?;
    match tool_name {
        "CallMcpTool" | "call-mcp-tool" | "callMcpTool" => {
            let mcp_tool = parsed.tool_name.or(parsed.tool)?;
            Some(format!("mcp__{server_id}__{mcp_tool}"))
        }
        "ReadMcpResourceTool" | "read-mcp-resource" | "readMcpResource" => {
            Some(format!("mcp__{server_id}__resources"))
        }
        "ListMcpResourcesTool" | "list-mcp-resources" | "listMcpResources" => {
            Some(format!("mcp__{server_id}__list_resources"))
        }
        "ListMcpToolsTool" | "list-mcp-tools" | "listMcpTools" => {
            Some(format!("mcp__{server_id}__list_tools"))
        }
        "ListMcpPromptsTool" | "list-mcp-prompts" | "listMcpPrompts" => {
            Some(format!("mcp__{server_id}__list_prompts"))
        }
        "GetMcpPromptTool" | "get-mcp-prompt" | "getMcpPrompt" => {
            Some(format!("mcp__{server_id}__prompts"))
        }
        _ => None,
    }
}

pub(super) fn tool_name_matches_rule(tool_name: &str, rule_tool_name: &str) -> bool {
    let tool_name = canonical_tool_name(tool_name);
    let rule_tool_name = canonical_tool_name(rule_tool_name);

    if tool_name.eq_ignore_ascii_case(&rule_tool_name) {
        return true;
    }

    if rule_tool_name == "file" {
        return matches!(
            tool_name.as_str(),
            "file-read" | "file-write" | "file-edit" | "notebook-edit"
        );
    }

    if let Some(server) = rule_tool_name.strip_suffix("__*") {
        return tool_name.starts_with(&format!("{server}__"));
    }

    tool_name.starts_with(&format!("{rule_tool_name}__"))
}

fn is_mcp_permission_rule_name(tool_name: &str) -> bool {
    canonical_tool_name(tool_name).starts_with("mcp__")
}

pub(super) fn extract_command_from_tool_input(tool_input: &str) -> Option<String> {
    let parsed: BashToolInput = serde_json::from_str(tool_input).ok()?;
    parsed.command.or(parsed.cmd).or(parsed.script)
}

fn bash_command_matches_rule(command: &str, pattern: &str, mode: PermissionRuleMatchMode) -> bool {
    match mode {
        PermissionRuleMatchMode::Allow => bash_atomic_command_matches_allow_rule(command, pattern),
        PermissionRuleMatchMode::Deny => bash_command_matches_deny_rule(command, pattern),
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BashRuleDecision {
    Deny,
    Allow,
    Ask,
}

#[cfg(test)]
pub(crate) fn resolve_bash_command_permission(
    deny_rules: &[PermissionRule],
    allow_rules: &[PermissionRule],
    tool_name: &str,
    tool_input: &str,
) -> BashRuleDecision {
    let denied = deny_rules.iter().any(|rule| {
        rule.matches_tool_call_with_mode(tool_name, tool_input, PermissionRuleMatchMode::Deny)
    });
    if denied {
        return BashRuleDecision::Deny;
    }

    if bash_command_allowed_by_rules(allow_rules, tool_name, tool_input).is_some() {
        return BashRuleDecision::Allow;
    }

    BashRuleDecision::Ask
}

pub(super) fn bash_word_width(command: &str) -> Option<usize> {
    bash_ast::first_word(command).map(|word| word.end_byte)
}

pub(super) fn bash_shell_command_string_bodies(command: &str) -> Vec<String> {
    let mut bodies = Vec::new();
    for candidate in
        std::iter::once(command.to_string()).chain(analyze_bash_command(command).subcommands)
    {
        let Some(mut tokens) = tokenize_bash_words(&candidate) else {
            continue;
        };
        for body in env_assignment_command_string_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
        strip_leading_env_assignments(&mut tokens, true);
        strip_bash_wrappers_with_shell_command_strings(&mut tokens, true, false);
        if let Some(body) = shell_command_string_body(&tokens)
            .or_else(|| eval_command_string_body(&tokens))
            .or_else(|| trap_command_string_body(&tokens))
            .or_else(|| su_command_string_body(&tokens))
            .or_else(|| runuser_command_string_body(&tokens))
            .or_else(|| sg_command_string_body(&tokens))
            .or_else(|| flock_command_string_body(&tokens))
            .or_else(|| script_command_string_body(&tokens))
            .or_else(|| tmux_command_string_body(&tokens))
            .or_else(|| rsync_remote_shell_command_body(&tokens))
            .or_else(|| npm_exec_command_string_body(&tokens))
            .or_else(|| nix_shell_run_command_body(&tokens))
            .or_else(|| git_shell_alias_command_body(&tokens))
            .or_else(|| git_submodule_foreach_command_body(&tokens))
            .or_else(|| git_ssh_command_config_body(&tokens))
            .or_else(|| git_askpass_command_config_body(&tokens))
            .or_else(|| git_external_diff_command_config_body(&tokens))
            .or_else(|| git_editor_command_config_body(&tokens))
            .or_else(|| hg_editor_command_config_body(&tokens))
            .or_else(|| hg_ssh_command_config_body(&tokens))
            .or_else(|| git_pager_command_config_body(&tokens))
            .or_else(|| git_tool_command_config_body(&tokens))
            .or_else(|| man_pager_command_option_body(&tokens))
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        for body in git_filter_branch_command_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
        for body in git_rebase_exec_command_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
        for body in git_difftool_extcmd_command_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
        for body in git_credential_helper_command_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
        for body in tar_command_option_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
        for body in ssh_option_command_string_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
        for body in openssh_transfer_command_string_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
        for body in socat_shell_command_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
    }
    bodies
}

pub(super) fn bash_argv_command_bodies(command: &str) -> Vec<String> {
    let mut bodies = Vec::new();
    for candidate in
        std::iter::once(command.to_string()).chain(analyze_bash_command(command).subcommands)
    {
        let Some(mut tokens) = tokenize_bash_words(&candidate) else {
            continue;
        };
        strip_leading_env_assignments(&mut tokens, true);
        strip_bash_wrappers_with_shell_command_strings(&mut tokens, true, false);
        for body in find_exec_command_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
        for body in parallel_command_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
        if let Some(body) = ssh_remote_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        for body in sshpass_argv_command_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
        if let Some(body) = git_bisect_run_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = npm_exec_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = pnpm_exec_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = yarn_exec_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = bun_exec_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = python_project_runner_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = conda_run_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = ruby_project_runner_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = direnv_exec_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = nix_cli_command_argv_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = guix_shell_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = watchexec_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = entr_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = screen_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = container_cli_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        if let Some(body) = kubectl_exec_argv_command_body(&tokens)
            && !bodies.iter().any(|existing| existing == &body)
        {
            bodies.push(body);
        }
        for body in socat_exec_command_bodies(&tokens) {
            if !bodies.iter().any(|existing| existing == &body) {
                bodies.push(body);
            }
        }
    }
    bodies
}

pub(super) fn nested_bash_command_candidates(command: &str) -> Vec<String> {
    bash_ast::nested_command_candidates(command)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct BashCommandAnalysis {
    pub(super) subcommands: Vec<String>,
    pub(super) too_complex: bool,
}

pub(super) fn analyze_bash_command(command: &str) -> BashCommandAnalysis {
    let analysis = bash_ast::analyze(command);
    BashCommandAnalysis {
        subcommands: analysis.subcommands,
        too_complex: analysis.too_complex,
    }
}

pub(super) fn normalize_bash_command_for_rule(
    command: &str,
    strip_all_env_vars: bool,
) -> Option<String> {
    let mut tokens = tokenize_bash_words(command)?;
    strip_leading_env_assignments(&mut tokens, strip_all_env_vars);
    strip_bash_wrappers(&mut tokens, strip_all_env_vars);
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" "))
    }
}

pub(super) fn tokenize_bash_words(command: &str) -> Option<Vec<String>> {
    bash_ast::tokenize_words(command)
}

pub(super) fn strip_leading_env_assignments(tokens: &mut Vec<String>, strip_all: bool) {
    while tokens
        .first()
        .is_some_and(|token| is_bash_env_assignment(token, strip_all))
    {
        tokens.remove(0);
    }
}

pub(super) fn is_bash_env_assignment(token: &str, strip_all: bool) -> bool {
    let Some((name, value)) = token.split_once('=') else {
        return false;
    };
    if name.is_empty()
        || !name
            .chars()
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || !name
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return false;
    }
    if strip_all {
        return true;
    }
    is_safe_bash_env_var(name) && value.chars().all(is_safe_bash_env_value_char)
}

fn is_safe_bash_env_var(name: &str) -> bool {
    matches!(
        name,
        "GOEXPERIMENT"
            | "GOOS"
            | "GOARCH"
            | "CGO_ENABLED"
            | "GO111MODULE"
            | "RUST_BACKTRACE"
            | "RUST_LOG"
            | "NODE_ENV"
            | "PYTHONUNBUFFERED"
            | "PYTHONDONTWRITEBYTECODE"
            | "PYTEST_DISABLE_PLUGIN_AUTOLOAD"
            | "PYTEST_DEBUG"
            | "ANTHROPIC_API_KEY"
            | "LANG"
            | "LANGUAGE"
            | "LC_ALL"
            | "LC_CTYPE"
            | "LC_TIME"
            | "CHARSET"
            | "TERM"
            | "COLORTERM"
            | "NO_COLOR"
            | "FORCE_COLOR"
            | "TZ"
            | "LS_COLORS"
            | "LSCOLORS"
            | "GREP_COLOR"
            | "GREP_COLORS"
            | "GCC_COLORS"
            | "TIME_STYLE"
            | "BLOCK_SIZE"
            | "BLOCKSIZE"
    )
}

fn is_safe_bash_env_value_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '/' | ':' | '-')
}
