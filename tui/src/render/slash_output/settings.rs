use std::collections::HashSet;

use orbcode_app_server_client::{
    AgentDefinition, AgentLoadWarning, AuthOverview, HookDiscovery, PermissionOverview,
    PlanOverview, PolicyOverview, SkillDefinition, StatusAuthOverview, StatusOverview,
    WorkspaceDiff,
};
#[cfg(test)]
use orbcode_app_server_client::{MemoryFileOverview, MemoryOverview};

use crate::commands::utils::short_session_id;

pub(crate) fn render_status_overview(overview: &StatusOverview) -> String {
    let permissions = &overview.permissions.permissions;
    let mut lines = vec![
        "Status:".to_string(),
        format!("session: {}", short_session_id(&overview.session_id)),
        format!("cwd: {}", overview.cwd.display()),
        format!("home: {}", overview.home_dir.display()),
        format!("model: {}", overview.model_display_name),
        format!("model id: {}", overview.model_name),
        format!(
            "model capabilities: {}",
            render_capability_list(&overview.model_capabilities)
        ),
        format!(
            "small/fast model: {}",
            overview.small_fast_model_display_name
        ),
        format!(
            "effort: {}",
            overview
                .effort_level
                .map_or_else(|| "auto".to_string(), |level| level.to_string())
        ),
        format!("provider: {}", overview.default_provider),
        format!(
            "fallback provider: {}",
            overview
                .fallback_provider
                .map_or_else(|| "none".to_string(), |provider| provider.to_string())
        ),
        format!("max retries: {}", overview.max_retries),
        format!("sandbox: {}", overview.sandbox_mode),
        format!(
            "additional dirs: {}",
            permissions.additional_directories.len()
        ),
        format!(
            "configured dirs: {}",
            overview.permissions.configured_additional_directories.len()
        ),
        format!(
            "session-only dirs: {}",
            overview.permissions.session_additional_directories.len()
        ),
        format!(
            "sandbox network: {}",
            bool_label(overview.sandbox_allow_network)
        ),
        format!("allow-all: {}", bool_label(overview.permissions.allow_all)),
        format!("tools permission: {}", bool_label(permissions.allow_tools)),
        format!("tool network: {}", bool_label(permissions.allow_network)),
        format!(
            "provider network: {}",
            bool_label(permissions.provider_allow_network)
        ),
        format!("available tools: {}", overview.available_tool_count),
        format!(
            "MCP: {} server(s), {} enabled transport(s)",
            overview.configured_mcp_server_count, overview.enabled_mcp_capability_count
        ),
        format!("persisted sessions: {}", overview.persisted_session_count),
        format!("background jobs: {}", overview.background_job_count),
    ];
    lines.push(String::new());
    lines.push(render_policy_overview(&overview.policy));
    lines.push(String::new());
    lines.push(render_status_auth_overview(&overview.auth));
    lines.join("\n")
}

fn render_policy_overview(overview: &PolicyOverview) -> String {
    let mut lines = vec!["Managed policy:".to_string()];
    match &overview.managed_origin {
        Some(origin) => {
            lines.push(format!("  source: {origin}"));
            if !overview.managed_paths.is_empty() {
                lines.push(format!("  paths: {}", overview.managed_paths.len()));
                for path in &overview.managed_paths {
                    lines.push(format!("    - {}", path.display()));
                }
            }
        }
        None => lines.push("  source: none".to_string()),
    }
    if let Some(models) = &overview.available_models {
        lines.push(format!("  availableModels: [{}]", models.join(", ")));
    }
    if overview.allow_managed_hooks_only {
        lines.push("  allowManagedHooksOnly: true".to_string());
    }
    if overview.allow_managed_permission_rules_only {
        lines.push("  allowManagedPermissionRulesOnly: true".to_string());
    }
    if overview.allow_managed_mcp_servers_only {
        lines.push("  allowManagedMcpServersOnly: true".to_string());
    }
    if overview.disable_bypass_permissions_mode {
        lines.push("  disableBypassPermissionsMode: disable".to_string());
    }
    if let Some(strict) = &overview.strict_plugin_only_customization {
        lines.push(format!("  strictPluginOnlyCustomization: {strict}"));
    }
    if let Some(force) = &overview.force_login_method {
        lines.push(format!("  forceLoginMethod: {force}"));
    }
    if let Some(count) = overview.allowed_mcp_servers {
        lines.push(format!("  allowedMcpServers: {count}"));
    }
    if overview.denied_mcp_servers > 0 {
        lines.push(format!(
            "  deniedMcpServers: {}",
            overview.denied_mcp_servers
        ));
    }
    if let Some(source) = &overview.effective_model_source {
        lines.push(format!("  effective model source: {source}"));
    }

    lines.push(String::new());
    lines.push("Settings sources:".to_string());
    for source in &overview.settings_sources {
        let presence = if source.present { "present" } else { "absent" };
        let access = if source.read_only {
            "read-only"
        } else {
            "writable"
        };
        let errors = if source.error_count > 0 {
            format!(", {} error(s)", source.error_count)
        } else {
            String::new()
        };
        lines.push(format!(
            "  - {} ({presence}, {access}{errors}): {}",
            source.source,
            source.primary_path.display(),
        ));
    }

    lines.push(String::new());
    if overview.conflicts.is_empty() {
        lines.push("Policy conflicts: none".to_string());
    } else {
        lines.push(format!("Policy conflicts: {}", overview.conflicts.len()));
        for conflict in &overview.conflicts {
            lines.push(format!(
                "  - [{}] {} ({})",
                conflict.source,
                conflict.message,
                conflict.source_path.display(),
            ));
        }
    }
    lines.join("\n")
}

pub(crate) fn render_auth_overview(overview: &AuthOverview) -> String {
    render_auth_entries(
        &overview.store_path,
        overview.entries.iter().map(|entry| {
            (
                entry.provider.to_string(),
                entry.method.to_string(),
                entry.source_summary.as_str(),
                entry.persisted,
                entry.usable,
                entry.active,
            )
        }),
    )
}

fn render_status_auth_overview(overview: &StatusAuthOverview) -> String {
    render_auth_entries(
        &overview.store_path,
        overview.entries.iter().map(|entry| {
            (
                entry.provider.to_string(),
                entry.method.to_string(),
                entry.source_summary.as_str(),
                entry.persisted,
                entry.usable,
                entry.active,
            )
        }),
    )
}

fn render_auth_entries<'a>(
    store_path: &std::path::Path,
    entries: impl Iterator<Item = (String, String, &'a str, bool, bool, bool)>,
) -> String {
    let entries = entries.collect::<Vec<_>>();
    let mut lines = vec![format!("auth store: {}", store_path.display())];
    if entries.is_empty() {
        lines.push("auth: no stored credentials".to_string());
    } else {
        lines.push("auth:".to_string());
        for (provider, method, source_summary, is_persisted, usable, is_active) in entries {
            let persisted = if is_persisted { "persisted" } else { "env" };
            if usable {
                let active = if is_active { " active" } else { "" };
                lines.push(format!(
                    "  - {} {} {} ({persisted}){active}",
                    provider, method, source_summary
                ));
            } else {
                lines.push(format!(
                    "  - {} {} {} ({persisted}, blocked)",
                    provider, method, source_summary
                ));
            }
        }
    }
    lines.join("\n")
}

pub(crate) fn render_permission_overview(overview: &PermissionOverview) -> String {
    let permissions = &overview.permissions;
    let allow_layers = [
        (
            &overview.settings_allowed_rules[..],
            "configured · settings",
        ),
        (&overview.startup_allowed_rules[..], "configured · env/CLI"),
        (
            &overview.edited_allowed_rules[..],
            "configured · settings edit",
        ),
        (&overview.runtime_allowed_rules[..], "session"),
    ];
    let deny_layers = [
        (&overview.settings_denied_rules[..], "configured · settings"),
        (&overview.startup_denied_rules[..], "configured · env/CLI"),
        (
            &overview.edited_denied_rules[..],
            "configured · settings edit",
        ),
        (&overview.runtime_denied_rules[..], "session"),
    ];
    let mut lines = vec![
        "Permissions:".to_string(),
        format!("  allow-all: {}", bool_label(overview.allow_all)),
        format!("  tools: {}", bool_label(permissions.allow_tools)),
        format!("  tool network: {}", bool_label(permissions.allow_network)),
        format!(
            "  provider network: {}",
            bool_label(permissions.provider_allow_network)
        ),
        format!("  allow rules: {}", sourced_rule_count(&allow_layers)),
        format!("  deny rules: {}", sourced_rule_count(&deny_layers)),
        format!(
            "  additional directories: {}",
            permissions.additional_directories.len()
        ),
        format!(
            "  configured directories: {}",
            overview.configured_additional_directories.len()
        ),
        format!(
            "  session-only directories: {}",
            overview.session_additional_directories.len()
        ),
    ];
    append_sourced_rule_section(&mut lines, "Allow rules", &allow_layers);
    append_sourced_rule_section(&mut lines, "Deny rules", &deny_layers);
    if !overview.configured_additional_directories.is_empty() {
        lines.push(String::new());
        lines.push("Configured directories:".to_string());
        for directory in &overview.configured_additional_directories {
            lines.push(format!("  - {}", directory.display()));
        }
    }
    if !overview.session_additional_directories.is_empty() {
        lines.push(String::new());
        lines.push("Session-only directories:".to_string());
        for directory in &overview.session_additional_directories {
            lines.push(format!("  - {}", directory.display()));
        }
    }
    lines.push(String::new());
    lines.push("Usage:".to_string());
    lines.push("  /permissions add allow Bash(cargo test:*)".to_string());
    lines.push("  /permissions add settings allow Bash(cargo test:*)".to_string());
    lines.push("  /permissions add deny Bash(rm:*)".to_string());
    lines.push("  /permissions add session allow Read(src/**)".to_string());
    lines.push("  /permissions remove allow Bash(cargo test:*)".to_string());
    lines.push("  /permissions remove session allow Read(src/**)".to_string());
    lines.push(String::new());
    lines.push("Editable sources:".to_string());
    lines.push("  - settings: writes home settings.json".to_string());
    lines.push("  - session: current session only, including approve-always rules".to_string());
    lines.push("Read-only here: env/CLI rules and project-local or managed settings.".to_string());
    lines.join("\n")
}

pub(crate) fn parse_changelog_release_notes(content: &str) -> Vec<(String, Vec<String>)> {
    let mut versions = Vec::new();
    for section in content.split("\n## ").skip(1) {
        let mut lines = section.lines();
        let Some(version_line) = lines.next().map(str::trim).filter(|line| !line.is_empty()) else {
            continue;
        };
        let version = version_line
            .split(" - ")
            .next()
            .unwrap_or(version_line)
            .trim()
            .trim_start_matches('#')
            .trim()
            .to_string();
        if version.is_empty() {
            continue;
        }
        let notes = lines
            .filter_map(|line| line.trim().strip_prefix("- ").map(str::trim))
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if !notes.is_empty() {
            versions.push((version, notes));
        }
    }
    versions
}

pub(crate) fn format_release_notes(notes: &[(String, Vec<String>)]) -> String {
    notes
        .iter()
        .map(|(version, notes)| {
            let bullet_points = notes
                .iter()
                .map(|note| format!("· {note}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("Version {version}:\n{bullet_points}")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
pub(crate) fn render_memory_overview(overview: &MemoryOverview) -> String {
    let mut lines = vec!["Memory:".to_string(), String::new()];
    append_memory_file_overview(&mut lines, &overview.user_memory);
    lines.push(String::new());
    lines.push("Project memory:".to_string());
    if overview.project_memories.is_empty() {
        lines.push("  none".to_string());
    } else {
        for memory in &overview.project_memories {
            lines.push(String::new());
            append_memory_file_overview(&mut lines, memory);
        }
    }
    lines.join("\n")
}

#[cfg(test)]
fn append_memory_file_overview(lines: &mut Vec<String>, memory: &MemoryFileOverview) {
    lines.push(format!("{}:", memory.label));
    lines.push(format!("  path: {}", memory.path.display()));
    lines.push(format!("  status: {}", memory.status.as_label()));
    lines.push(format!(
        "  access: {}",
        if memory.writable {
            "writable"
        } else {
            "read-only"
        }
    ));
    if let Some(boundary) = &memory.trust_boundary {
        lines.push(format!("  trust: {boundary}"));
    }
    if let Some(scope) = &memory.scope {
        lines.push(format!("  scope: {scope}"));
    }
    if let Some(reason) = &memory.skipped_reason {
        lines.push(format!("  skipped: {reason}"));
    }
    if let Some(content) = &memory.content {
        lines.push(String::new());
        for line in content.lines() {
            lines.push(format!("  {line}"));
        }
    }
}

pub(crate) fn render_plan_overview(overview: &PlanOverview) -> String {
    let mut lines = vec![
        "Current Plan:".to_string(),
        format!("file: {}", overview.plan_file.display()),
        format!(
            "mode: {}",
            if overview.in_plan_mode {
                "active"
            } else {
                "inactive"
            }
        ),
        String::new(),
    ];
    if overview.plan.trim().is_empty() {
        lines.push("No plan written yet.".to_string());
    } else {
        lines.push(overview.plan.trim_end().to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
pub(crate) fn render_workspace_diff(diff: &WorkspaceDiff) -> String {
    let mut lines = vec![
        "Workspace diff:".to_string(),
        format!("cwd: {}", diff.cwd.display()),
    ];

    append_diff_status(&mut lines, &diff.status);

    let has_staged = !diff.staged_diff.trim().is_empty();
    let has_unstaged = !diff.unstaged_diff.trim().is_empty();
    let has_untracked = !diff.untracked_files.is_empty();
    if !has_staged && !has_unstaged && !has_untracked {
        lines.push(String::new());
        lines.push("No workspace changes.".to_string());
        return lines.join("\n");
    }

    append_diff_section(&mut lines, "Staged changes", &diff.staged_diff);
    append_diff_section(&mut lines, "Unstaged changes", &diff.unstaged_diff);
    if has_untracked {
        lines.push(String::new());
        lines.push("Untracked files:".to_string());
        for path in &diff.untracked_files {
            lines.push(format!("  - {path}"));
        }
        lines.push("Untracked file contents are not shown until they are staged.".to_string());
    }

    lines.join("\n")
}

#[cfg(test)]
fn append_diff_status(lines: &mut Vec<String>, status: &str) {
    lines.push(String::new());
    if status.trim().is_empty() {
        lines.push("Status: clean".to_string());
        return;
    }
    lines.push("Status:".to_string());
    for line in status.lines() {
        lines.push(format!("  {line}"));
    }
}

#[cfg(test)]
fn append_diff_section(lines: &mut Vec<String>, title: &str, diff: &str) {
    if diff.trim().is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(format!("{title}:"));
    lines.push(diff.to_string());
}

pub(crate) fn workspace_diff_changed_path_count(diff: &WorkspaceDiff) -> usize {
    let mut paths = HashSet::new();
    for line in diff.status.lines() {
        if line.len() < 4 {
            continue;
        }
        let path = line[3..].trim();
        if let Some((old_path, new_path)) = path.split_once(" -> ") {
            paths.insert(old_path.trim().to_string());
            paths.insert(new_path.trim().to_string());
        } else if !path.is_empty() {
            paths.insert(path.to_string());
        }
    }
    paths.len()
}

fn append_sourced_rule_section(lines: &mut Vec<String>, title: &str, layers: &[(&[String], &str)]) {
    lines.push(String::new());
    lines.push(format!("{title}:"));
    let mut rendered = Vec::<(String, Vec<&str>)>::new();
    for (rules, source) in layers {
        for rule in *rules {
            if let Some((_, sources)) = rendered.iter_mut().find(|(existing, _)| existing == rule) {
                if !sources.iter().any(|existing| existing == source) {
                    sources.push(*source);
                }
            } else {
                rendered.push((rule.clone(), vec![*source]));
            }
        }
    }
    if rendered.is_empty() {
        lines.push("  none".to_string());
        return;
    }
    for (rule, sources) in rendered {
        lines.push(format!("  - {rule} ({})", sources.join(" · ")));
    }
}

fn sourced_rule_count(layers: &[(&[String], &str)]) -> usize {
    let mut rendered = Vec::<&str>::new();
    for (rules, _) in layers {
        for rule in *rules {
            if !rendered.iter().any(|existing| existing == rule) {
                rendered.push(rule);
            }
        }
    }
    rendered.len()
}

fn bool_label(value: bool) -> &'static str {
    if value { "enabled" } else { "disabled" }
}

fn render_capability_list(capabilities: &[String]) -> String {
    if capabilities.is_empty() {
        "none".to_string()
    } else {
        capabilities.join(",")
    }
}

pub(crate) fn render_hook_discovery(discovery: &HookDiscovery) -> String {
    let mut lines: Vec<String> = Vec::new();
    if discovery.hooks.is_empty() && discovery.warnings.is_empty() {
        return "No hooks configured.".to_string();
    }
    for warning in &discovery.warnings {
        lines.push(format!("⚠ {}", warning.summary_line()));
    }
    if !discovery.warnings.is_empty() && !discovery.hooks.is_empty() {
        lines.push(String::new());
    }
    for hook in &discovery.hooks {
        lines.push(hook.summary_line());
    }
    lines.join("\n")
}

/// Truncate a preview to at most `max_chars` characters (not bytes), appending
/// `...` when truncated. Operating on chars avoids panicking when a byte-index
/// slice would land inside a multibyte character (e.g. a CJK description).
fn truncate_preview_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() > max_chars {
        let keep = max_chars.saturating_sub(3);
        let truncated: String = text.chars().take(keep).collect();
        format!("{truncated}...")
    } else {
        text.to_string()
    }
}

pub(crate) fn render_skill_definitions(definitions: &[SkillDefinition]) -> String {
    if definitions.is_empty() {
        return "No skills available.".to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    for skill in definitions {
        let source = skill.source.as_str();
        let description = skill
            .when_to_use
            .as_deref()
            .or(skill.description.as_deref())
            .unwrap_or("");
        let preview = truncate_preview_chars(description, 80);
        lines.push(format!("[{source}] {:<30} {preview}", skill.name));
    }
    lines.join("\n")
}

#[cfg(test)]
pub(crate) fn render_agent_definitions(definitions: &[AgentDefinition]) -> String {
    render_agent_definitions_with_warnings(definitions, &[])
}

pub(crate) fn render_agent_definitions_with_warnings(
    definitions: &[AgentDefinition],
    warnings: &[AgentLoadWarning],
) -> String {
    let mut lines: Vec<String> = Vec::new();
    if definitions.is_empty() && warnings.is_empty() {
        return "No agent definitions available.".to_string();
    }
    for agent in definitions {
        let source = agent.source.as_str();
        let model = agent
            .model
            .as_deref()
            .map(|m| format!(" model={m}"))
            .unwrap_or_default();
        let tools_summary = match &agent.tools {
            None => String::new(),
            Some(tools) if tools.is_empty() => " tools=none".to_string(),
            Some(tools) if tools.len() <= 3 => format!(" tools={}", tools.join(",")),
            Some(tools) => format!(" tools={}+{}more", tools[..2].join(","), tools.len() - 2),
        };
        let prompt_preview = truncate_preview_chars(&agent.prompt, 60).replace('\n', " ");
        lines.push(format!(
            "[{source}] {:<30}{model}{tools_summary}",
            agent.agent_type
        ));
        if !prompt_preview.is_empty() {
            lines.push(format!("  {prompt_preview}"));
        }
    }
    if !warnings.is_empty() {
        lines.push(String::new());
        lines.push("Warnings:".to_string());
        for warning in warnings {
            let path_info = warning
                .path
                .as_ref()
                .map(|p| format!(" ({})", p.display()))
                .unwrap_or_default();
            let agent_info = warning
                .agent_type
                .as_ref()
                .map(|t| format!(" agent '{t}'"))
                .unwrap_or_default();
            lines.push(format!(
                "  [{}]{agent_info}{path_info}: {}",
                warning.source.as_str(),
                warning.message
            ));
        }
    }
    lines.join("\n")
}
