use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::hooks::{HookCommand, HookMatcher};
use crate::plugins::{load_plugin_registry, plugin_agent_definitions};
use crate::{ConfigError, PermissionMode};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSource {
    BuiltIn,
    UserSettings,
    ProjectSettings,
    /// Sourced from an enabled plugin in `~/.claude/plugins/...`. The
    /// `plugin_id` carries the canonical `name@marketplace` identifier so
    /// `/agents` can surface where the definition came from.
    Plugin {
        plugin_id: String,
    },
}

impl AgentSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::UserSettings => "user",
            Self::ProjectSettings => "project",
            Self::Plugin { .. } => "plugin",
        }
    }
}

/// Parsed agent definition. `tools` follows the TypeScript convention:
/// `None` means "all tools" (or `*`), `Some(empty)` means "no tools".
///
/// `mcp_server_names` and `hooks` come from optional frontmatter blocks.
/// Both expect **flow-style** (JSON-compatible) YAML so we can parse them
/// without a full YAML dependency, e.g.:
///
/// ```yaml
/// mcpServers: ["context7", "rust-docs"]
/// hooks: {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo blocked"}]}]}
/// ```
///
/// Both fields are scoped to the child agent loop only; they MUST NOT
/// affect the parent session's hook matchers or MCP visibility.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentDefinition {
    pub agent_type: String,
    pub description: String,
    pub prompt: String,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub skills: Vec<String>,
    /// Names of MCP servers the agent is allowed to see. `None` means
    /// "inherit parent's visible MCP tools"; `Some(empty)` means "no MCP
    /// tools". When `Some(names)`, only MCP tools whose server name
    /// appears in `names` are kept in the child's tool pool.
    pub mcp_server_names: Option<Vec<String>>,
    /// Agent-scoped hook matchers, keyed by hook event name (e.g.
    /// `"PreToolUse"`, `"PostToolUse"`, `"SubagentStop"`). These run in
    /// addition to the settings-cascade hooks during the child agent loop
    /// and are discarded when the child loop returns.
    pub hooks: BTreeMap<String, Vec<HookMatcher>>,
    pub source: AgentSource,
    pub path: Option<PathBuf>,
}

impl AgentDefinition {
    pub fn built_in(
        agent_type: impl Into<String>,
        description: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            agent_type: agent_type.into(),
            description: description.into(),
            prompt: prompt.into(),
            tools: None,
            disallowed_tools: None,
            model: None,
            permission_mode: None,
            skills: Vec::new(),
            mcp_server_names: None,
            hooks: BTreeMap::new(),
            source: AgentSource::BuiltIn,
            path: None,
        }
    }
}

/// Why an agent file produced a non-fatal diagnostic during loading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentWarningKind {
    /// Frontmatter was present but a required field (`name` or `description`)
    /// was missing or empty, so the file was skipped.
    MissingField,
    /// Two files in the same source layer declared the same agent name; the
    /// first (by path order) was kept and the later one was dropped.
    DuplicateName,
}

/// A non-fatal diagnostic raised while loading agent definitions. The loader
/// keeps going; these surface so users can fix typos without the agent list
/// silently dropping entries.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentLoadWarning {
    pub kind: AgentWarningKind,
    pub source: AgentSource,
    pub path: Option<PathBuf>,
    pub agent_type: Option<String>,
    pub message: String,
}

/// Result of loading agent definitions with diagnostics. `definitions` is the
/// merged, precedence-resolved list; `warnings` collects every non-fatal
/// problem found along the way.
#[derive(Clone, Debug, Default)]
pub struct AgentLoadOutcome {
    pub definitions: Vec<AgentDefinition>,
    pub warnings: Vec<AgentLoadWarning>,
}

/// Outcome of parsing a single agent markdown file.
enum AgentParseOutcome {
    /// Successfully parsed into a definition.
    Parsed(Box<AgentDefinition>),
    /// No frontmatter block at all — treated as a co-located reference doc and
    /// skipped silently (no warning).
    NotAnAgent,
    /// Frontmatter was present but a required field was missing/empty. Carries
    /// a human-readable reason for the warning.
    MissingRequiredField(String),
}

/// Returns the always-available built-in agents.
pub fn built_in_agent_definitions() -> Vec<AgentDefinition> {
    vec![AgentDefinition::built_in(
        "general-purpose",
        "General-purpose agent for researching complex questions, searching for code, and executing multi-step tasks. When you are searching for a keyword or file and are not confident that you will find the right match in the first few tries use this agent to perform the search for you.",
        concat!(
            "You are an agent for Orb Code, a terminal coding assistant. Given the user's message, ",
            "use the tools available to complete the task. Complete the task fully — don't gold-plate, ",
            "but don't leave it half-done.\n\n",
            "When you complete the task, respond with a concise report covering what was done and any ",
            "key findings — the caller will relay this to the user, so it only needs the essentials.\n\n",
            "Guidelines:\n",
            "- For file searches: search broadly when you don't know where something lives.\n",
            "- For analysis: start broad and narrow down. Use multiple strategies.\n",
            "- Be thorough: check multiple locations, consider different naming conventions.\n",
            "- NEVER create files unless absolutely necessary; prefer editing existing files.\n",
            "- NEVER create documentation (*.md) unless explicitly requested.",
        ),
    )]
}

/// Loads all agent definitions (built-in + user + project) and merges them by
/// agent_type with project > user > built-in precedence. This is the
/// backwards-compatible surface; use [`load_agent_definitions_with_warnings`]
/// when you also need the non-fatal diagnostics.
pub async fn load_agent_definitions(
    home_dir: &Path,
    cwd: &Path,
) -> Result<Vec<AgentDefinition>, ConfigError> {
    Ok(load_agent_definitions_with_warnings(home_dir, cwd)
        .await?
        .definitions)
}

/// Like [`load_agent_definitions`] but also returns non-fatal diagnostics:
/// agent files whose frontmatter is missing a required field, and duplicate
/// names declared within the same source layer.
pub async fn load_agent_definitions_with_warnings(
    home_dir: &Path,
    cwd: &Path,
) -> Result<AgentLoadOutcome, ConfigError> {
    let mut by_type: Vec<AgentDefinition> = built_in_agent_definitions();
    let mut warnings: Vec<AgentLoadWarning> = Vec::new();
    for (source, dir) in [
        (AgentSource::UserSettings, home_dir.join("agents")),
        (
            AgentSource::ProjectSettings,
            cwd.join(".claude").join("agents"),
        ),
    ] {
        let dir_outcome = load_agent_definitions_from_dir(&dir, source).await?;
        warnings.extend(dir_outcome.warnings);
        for definition in dir_outcome.definitions {
            // Same-name across layers is intentional precedence (project beats
            // user beats built-in), not a duplicate — so we override quietly.
            if let Some(existing) = by_type
                .iter()
                .position(|entry| entry.agent_type == definition.agent_type)
            {
                by_type[existing] = definition;
            } else {
                by_type.push(definition);
            }
        }
    }

    // Plugin agents come last and never override user/project agents because
    // their type is namespaced (`pluginName:agentName`). They appear above
    // built-in only because the namespace prevents collisions.
    if let Ok(registry) = load_plugin_registry(home_dir, cwd).await {
        for definition in plugin_agent_definitions(&registry) {
            if !by_type
                .iter()
                .any(|entry| entry.agent_type == definition.agent_type)
            {
                by_type.push(definition);
            }
        }
    }
    Ok(AgentLoadOutcome {
        definitions: by_type,
        warnings,
    })
}

async fn load_agent_definitions_from_dir(
    dir: &Path,
    source: AgentSource,
) -> Result<AgentLoadOutcome, ConfigError> {
    let mut outcome = AgentLoadOutcome::default();
    if !tokio::fs::try_exists(dir).await? {
        return Ok(outcome);
    }
    // Collect (path, contents) first, then process in a deterministic path
    // order so duplicate-name detection picks a stable "first" winner
    // regardless of read_dir ordering.
    let mut files: Vec<PathBuf> = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("md") {
            files.push(path);
        }
    }
    files.sort();

    let mut seen: Vec<String> = Vec::new();
    for path in files {
        let contents = tokio::fs::read_to_string(&path).await?;
        match parse_agent_markdown_detailed(&path, &contents, source.clone()) {
            AgentParseOutcome::Parsed(definition) => {
                if seen.contains(&definition.agent_type) {
                    outcome.warnings.push(AgentLoadWarning {
                        kind: AgentWarningKind::DuplicateName,
                        source: source.clone(),
                        path: Some(path.clone()),
                        agent_type: Some(definition.agent_type.clone()),
                        message: format!(
                            "duplicate agent name '{}' in {}; keeping the first definition",
                            definition.agent_type,
                            dir.display(),
                        ),
                    });
                    continue;
                }
                seen.push(definition.agent_type.clone());
                outcome.definitions.push(*definition);
            }
            AgentParseOutcome::MissingRequiredField(reason) => {
                outcome.warnings.push(AgentLoadWarning {
                    kind: AgentWarningKind::MissingField,
                    source: source.clone(),
                    path: Some(path.clone()),
                    agent_type: None,
                    message: format!("skipping {}: {reason}", path.display()),
                });
            }
            AgentParseOutcome::NotAnAgent => {}
        }
    }
    outcome
        .definitions
        .sort_by(|left, right| left.agent_type.cmp(&right.agent_type));
    Ok(outcome)
}

/// Parses an agent definition from a markdown file with YAML-ish frontmatter.
/// Returns `None` when required fields (`name`, `description`) are missing or
/// when the file has no frontmatter at all (likely co-located reference docs).
///
/// This is the lossy compatibility surface; [`parse_agent_markdown_detailed`]
/// distinguishes "not an agent" from "missing required field" so loaders can
/// emit warnings.
pub fn parse_agent_markdown(
    path: &Path,
    contents: &str,
    source: AgentSource,
) -> Option<AgentDefinition> {
    match parse_agent_markdown_detailed(path, contents, source) {
        AgentParseOutcome::Parsed(definition) => Some(*definition),
        _ => None,
    }
}

/// Parses an agent markdown file, distinguishing a missing frontmatter block
/// (`NotAnAgent`, skipped silently) from a present-but-incomplete one
/// (`MissingRequiredField`, which callers surface as a warning).
fn parse_agent_markdown_detailed(
    path: &Path,
    contents: &str,
    source: AgentSource,
) -> AgentParseOutcome {
    let trimmed = contents.trim_start_matches('\u{feff}');
    let Some((frontmatter, body)) = split_frontmatter(trimmed) else {
        return AgentParseOutcome::NotAnAgent;
    };
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut tools_raw: Option<String> = None;
    let mut disallowed_tools_raw: Option<String> = None;
    let mut model: Option<String> = None;
    let mut permission_mode: Option<String> = None;
    let mut skills_raw: Option<String> = None;
    let mut mcp_servers_raw: Option<String> = None;
    let mut hooks_raw: Option<String> = None;
    for (key, value) in parse_frontmatter_fields(frontmatter) {
        match key.as_str() {
            "name" => name = Some(value),
            "description" => description = Some(value),
            "tools" => tools_raw = Some(value),
            "disallowedTools" | "disallowed_tools" => disallowed_tools_raw = Some(value),
            "model" => model = Some(value),
            "permissionMode" | "permission_mode" => permission_mode = Some(value),
            "skills" => skills_raw = Some(value),
            "mcpServers" | "mcp_servers" => mcp_servers_raw = Some(value),
            "hooks" => hooks_raw = Some(value),
            _ => {}
        }
    }

    let agent_type = name.unwrap_or_default().trim().to_string();
    if agent_type.is_empty() {
        return AgentParseOutcome::MissingRequiredField(
            "agent frontmatter is missing a non-empty `name`".to_string(),
        );
    }
    let description = description.unwrap_or_default().trim().to_string();
    if description.is_empty() {
        return AgentParseOutcome::MissingRequiredField(format!(
            "agent '{agent_type}' frontmatter is missing a non-empty `description`"
        ));
    }
    let prompt = body.trim().to_string();

    let tools = parse_tool_list_for_agent(tools_raw.as_deref());
    let disallowed_tools = disallowed_tools_raw
        .as_deref()
        .map(parse_string_list)
        .filter(|values| !values.is_empty());
    let skills = skills_raw
        .as_deref()
        .map(parse_string_list)
        .unwrap_or_default();
    let mcp_server_names = mcp_servers_raw
        .as_deref()
        .and_then(parse_mcp_server_names_for_agent);
    let hooks = hooks_raw
        .as_deref()
        .and_then(parse_agent_hooks_block)
        .unwrap_or_default();
    let permission_mode = match permission_mode
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        Some(value) => match PermissionMode::parse(&value) {
            Some(mode) => Some(mode),
            None => {
                return AgentParseOutcome::MissingRequiredField(format!(
                    "agent '{agent_type}' frontmatter has invalid `permissionMode`: {value}"
                ));
            }
        },
        None => None,
    };

    AgentParseOutcome::Parsed(Box::new(AgentDefinition {
        agent_type,
        description,
        prompt,
        tools,
        disallowed_tools,
        model: model
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        permission_mode,
        skills,
        mcp_server_names,
        hooks,
        source,
        path: Some(path.to_path_buf()),
    }))
}

fn split_frontmatter(contents: &str) -> Option<(&str, &str)> {
    let rest = contents
        .strip_prefix("---\n")
        .or_else(|| contents.strip_prefix("---\r\n"))?;
    // The closing delimiter is a line that is *exactly* `---`, not merely any
    // line starting with `---`. A substring `\n---` match would truncate a
    // frontmatter value like `---foo` and drop later keys (`model`, `tools`,
    // `permissionMode`).
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let line_content = line.trim_end_matches('\n').trim_end_matches('\r');
        if line_content == "---" {
            // `offset` is the start of the delimiter line; the frontmatter is
            // everything before the newline that precedes it.
            let frontmatter = rest[..offset].trim_end_matches('\n').trim_end_matches('\r');
            let body = &rest[offset + line.len()..];
            return Some((frontmatter, body));
        }
        offset += line.len();
    }
    None
}

fn parse_frontmatter_fields(frontmatter: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    for raw_line in frontmatter.lines() {
        let line = raw_line.trim_end();
        if line.is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let key = line[..colon].trim().to_string();
        if key.is_empty() {
            continue;
        }
        let value = strip_quotes(line[colon + 1..].trim());
        entries.push((key, value.to_string()));
    }
    entries
}

fn strip_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if first == last && (first == b'"' || first == b'\'') {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

fn parse_string_list(value: &str) -> Vec<String> {
    let mut trimmed = value.trim();
    if let Some(stripped) = trimmed.strip_prefix('[')
        && let Some(stripped) = stripped.strip_suffix(']')
    {
        trimmed = stripped;
    }
    trimmed
        .split(',')
        .map(|item| strip_quotes(item.trim()).trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Agent-specific tool list parsing:
/// - missing field => `None` (all tools)
/// - field present containing `*` => `None` (all tools)
/// - field present otherwise => `Some(list)` (possibly empty == no tools)
fn parse_tool_list_for_agent(value: Option<&str>) -> Option<Vec<String>> {
    let raw = value?;
    let items = parse_string_list(raw);
    if items.iter().any(|item| item == "*") {
        return None;
    }
    Some(items)
}

/// Parse the `mcpServers` frontmatter value into an optional list of
/// MCP server names. Accepts comma-separated or JSON-array flow syntax;
/// inline-object server definitions are intentionally rejected here —
/// frontmatter authors should reference servers that are already
/// configured in the parent settings cascade. Returns `None` when the
/// value is empty or unparseable so unknown shapes fall back to
/// "inherit parent's visible MCP tools" instead of "block everything".
fn parse_mcp_server_names_for_agent(value: &str) -> Option<Vec<String>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let items = parse_string_list(trimmed);
    if items.is_empty() {
        return None;
    }
    if items.iter().any(|item| item == "*") {
        return None;
    }
    Some(items)
}

/// Parse the `hooks` frontmatter value as JSON-encoded
/// `BTreeMap<String, Vec<HookMatcher>>`. Returns `None` when the value
/// is empty or unparseable; bad input falls back to "no agent-specific
/// hooks" rather than failing the agent definition outright.
///
/// Agent-authored `Stop` hooks are remapped to `SubagentStop` because
/// the child loop only fires `SubagentStop` on completion — this matches
/// `registerFrontmatterHooks(..., isAgent=true)` in the TypeScript
/// reference and means agent authors can use the familiar `Stop` event
/// name without dropping notifications.
fn parse_agent_hooks_block(value: &str) -> Option<BTreeMap<String, Vec<HookMatcher>>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed: BTreeMap<String, Vec<HookMatcher>> = serde_json::from_str(trimmed).ok()?;
    if parsed.is_empty() {
        return None;
    }
    let mut cleaned: BTreeMap<String, Vec<HookMatcher>> = BTreeMap::new();
    for (event, matchers) in parsed {
        let live: Vec<HookMatcher> = matchers
            .into_iter()
            .filter(|matcher| {
                matcher
                    .hooks
                    .iter()
                    .any(|hook| matches!(hook, HookCommand::Command { .. }))
            })
            .collect();
        if live.is_empty() {
            continue;
        }
        let target = if event == "Stop" {
            "SubagentStop".to_string()
        } else {
            event
        };
        cleaned.entry(target).or_default().extend(live);
    }
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    #[test]
    fn parses_full_agent_frontmatter() {
        let contents = "---\nname: Explore\ndescription: 'Read-only exploration agent.'\ntools: Read, Grep, Glob\nmodel: claude-haiku-4-5\npermissionMode: plan\nskills: skill-a, skill-b\n---\nYou are the Explore agent.\n";
        let agent = parse_agent_markdown(
            &fixture_path("Explore.md"),
            contents,
            AgentSource::UserSettings,
        )
        .expect("parses agent");

        assert_eq!(agent.agent_type, "Explore");
        assert_eq!(agent.description, "Read-only exploration agent.");
        assert_eq!(agent.prompt, "You are the Explore agent.");
        assert_eq!(
            agent.tools.as_deref(),
            Some(["Read", "Grep", "Glob"].as_slice())
                .map(|values| values
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>())
                .as_deref()
        );
        assert_eq!(agent.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(agent.permission_mode, Some(PermissionMode::Plan));
        assert_eq!(
            agent.skills,
            vec!["skill-a".to_string(), "skill-b".to_string()]
        );
        assert_eq!(agent.source, AgentSource::UserSettings);
    }

    #[test]
    fn frontmatter_value_starting_with_dashes_does_not_truncate_later_keys() {
        // A value line beginning with `---` must not be mistaken for the closing
        // delimiter (which would drop `model`/`permissionMode`).
        let contents = "---\nname: Weird\ndescription: \"---divider style\"\nmodel: claude-haiku-4-5\npermissionMode: plan\n---\nBody text.\n";
        let agent = parse_agent_markdown(
            &fixture_path("Weird.md"),
            contents,
            AgentSource::UserSettings,
        )
        .expect("parses agent");
        assert_eq!(agent.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(agent.permission_mode, Some(PermissionMode::Plan));
        assert_eq!(agent.prompt, "Body text.");
    }

    #[test]
    fn missing_required_fields_returns_none() {
        let no_name = "---\ndescription: x\n---\nbody";
        assert!(
            parse_agent_markdown(&fixture_path("a.md"), no_name, AgentSource::UserSettings)
                .is_none()
        );

        let no_description = "---\nname: a\n---\nbody";
        assert!(
            parse_agent_markdown(
                &fixture_path("a.md"),
                no_description,
                AgentSource::UserSettings
            )
            .is_none()
        );

        let no_frontmatter = "just markdown body";
        assert!(
            parse_agent_markdown(
                &fixture_path("a.md"),
                no_frontmatter,
                AgentSource::UserSettings
            )
            .is_none()
        );
    }

    #[test]
    fn star_in_tools_means_all_tools() {
        let contents = "---\nname: a\ndescription: d\ntools: '*'\n---\nbody";
        let agent =
            parse_agent_markdown(&fixture_path("a.md"), contents, AgentSource::UserSettings)
                .expect("parses");
        assert!(agent.tools.is_none());
    }

    #[test]
    fn parses_mcp_server_names_from_flow_array() {
        let contents =
            "---\nname: a\ndescription: d\nmcpServers: [\"context7\", \"rust-docs\"]\n---\nbody";
        let agent =
            parse_agent_markdown(&fixture_path("a.md"), contents, AgentSource::UserSettings)
                .expect("parses");
        assert_eq!(
            agent.mcp_server_names.as_deref(),
            Some(vec!["context7".to_string(), "rust-docs".to_string()].as_slice())
        );
    }

    #[test]
    fn star_in_mcp_servers_means_inherit_all() {
        let contents = "---\nname: a\ndescription: d\nmcpServers: [\"*\"]\n---\nbody";
        let agent =
            parse_agent_markdown(&fixture_path("a.md"), contents, AgentSource::UserSettings)
                .expect("parses");
        assert!(agent.mcp_server_names.is_none());
    }

    #[test]
    fn parses_hooks_block_from_flow_json() {
        let contents = concat!(
            "---\n",
            "name: a\n",
            "description: d\n",
            "hooks: {\"PreToolUse\": [{\"matcher\": \"Bash\", \"hooks\": [{\"type\": \"command\", \"command\": \"echo blocked\"}]}]}\n",
            "---\nbody",
        );
        let agent =
            parse_agent_markdown(&fixture_path("a.md"), contents, AgentSource::UserSettings)
                .expect("parses");
        let matchers = agent
            .hooks
            .get("PreToolUse")
            .expect("PreToolUse hook present");
        assert_eq!(matchers.len(), 1);
        let matcher = &matchers[0];
        assert_eq!(matcher.matcher.as_deref(), Some("Bash"));
        match &matcher.hooks[0] {
            HookCommand::Command { command, .. } => assert_eq!(command, "echo blocked"),
            other => panic!("expected command hook, got {other:?}"),
        }
    }

    #[test]
    fn stop_hook_in_agent_frontmatter_is_remapped_to_subagent_stop() {
        let contents = concat!(
            "---\n",
            "name: a\n",
            "description: d\n",
            "hooks: {\"Stop\": [{\"matcher\": \"\", \"hooks\": [{\"type\": \"command\", \"command\": \"echo done\"}]}]}\n",
            "---\nbody",
        );
        let agent =
            parse_agent_markdown(&fixture_path("a.md"), contents, AgentSource::UserSettings)
                .expect("parses");
        assert!(agent.hooks.contains_key("SubagentStop"));
        assert!(!agent.hooks.contains_key("Stop"));
    }

    #[test]
    fn missing_or_bad_hooks_block_returns_empty_map() {
        let no_hooks = "---\nname: a\ndescription: d\n---\nbody";
        let agent =
            parse_agent_markdown(&fixture_path("a.md"), no_hooks, AgentSource::UserSettings)
                .expect("parses");
        assert!(agent.hooks.is_empty());

        let bad_hooks = "---\nname: a\ndescription: d\nhooks: not-json\n---\nbody";
        let agent =
            parse_agent_markdown(&fixture_path("a.md"), bad_hooks, AgentSource::UserSettings)
                .expect("parses even with bad hooks");
        assert!(agent.hooks.is_empty());
    }

    #[test]
    fn empty_tools_list_means_no_tools() {
        let contents = "---\nname: a\ndescription: d\ntools: ''\n---\nbody";
        let agent =
            parse_agent_markdown(&fixture_path("a.md"), contents, AgentSource::UserSettings)
                .expect("parses");
        assert_eq!(
            agent.tools.as_deref(),
            Some(Vec::<String>::new().as_slice())
        );
    }

    #[tokio::test]
    async fn project_definitions_override_user_definitions() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let user_agents = home.join("agents");
        let project_agents = cwd.join(".claude").join("agents");
        tokio::fs::create_dir_all(&user_agents).await.unwrap();
        tokio::fs::create_dir_all(&project_agents).await.unwrap();
        tokio::fs::write(
            user_agents.join("Explore.md"),
            "---\nname: Explore\ndescription: user-version\nmodel: user-model\n---\nuser body\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            project_agents.join("Explore.md"),
            "---\nname: Explore\ndescription: project-version\nmodel: project-model\n---\nproject body\n",
        )
        .await
        .unwrap();

        let agents = load_agent_definitions(&home, &cwd).await.expect("load");

        let explore = agents
            .iter()
            .find(|agent| agent.agent_type == "Explore")
            .expect("Explore found");
        assert_eq!(explore.description, "project-version");
        assert_eq!(explore.model.as_deref(), Some("project-model"));
        assert_eq!(explore.source, AgentSource::ProjectSettings);

        // Built-in general-purpose remains present.
        assert!(
            agents
                .iter()
                .any(|agent| agent.agent_type == "general-purpose")
        );
    }

    #[tokio::test]
    async fn enabled_plugin_agents_are_merged_with_namespaced_name() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo");
        tokio::fs::create_dir_all(plugin_root.join(".claude-plugin"))
            .await
            .unwrap();
        tokio::fs::write(
            plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .await
        .unwrap();
        tokio::fs::create_dir_all(plugin_root.join("agents"))
            .await
            .unwrap();
        tokio::fs::write(
            plugin_root.join("agents").join("worker.md"),
            "---\nname: worker\ndescription: do work\n---\nbody\n",
        )
        .await
        .unwrap();
        tokio::fs::create_dir_all(home.join("plugins"))
            .await
            .unwrap();
        let index = format!(
            r#"{{"version":2,"plugins":{{"demo@market":[{{"scope":"user","installPath":"{}","version":"1.0.0"}}]}}}}"#,
            plugin_root.display(),
        );
        tokio::fs::write(home.join("plugins").join("installed_plugins.json"), index)
            .await
            .unwrap();
        tokio::fs::create_dir_all(&home).await.unwrap();
        tokio::fs::write(
            home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await
        .unwrap();

        let agents = load_agent_definitions(&home, &cwd).await.expect("load");
        let plugin_agent = agents
            .iter()
            .find(|agent| agent.agent_type == "demo:worker")
            .expect("plugin agent surfaced");
        assert!(matches!(plugin_agent.source, AgentSource::Plugin { .. }));
    }

    #[test]
    fn detailed_parse_distinguishes_not_an_agent_from_missing_field() {
        let not_agent = parse_agent_markdown_detailed(
            &fixture_path("doc.md"),
            "just a co-located reference doc",
            AgentSource::UserSettings,
        );
        assert!(matches!(not_agent, AgentParseOutcome::NotAnAgent));

        let missing_name = parse_agent_markdown_detailed(
            &fixture_path("a.md"),
            "---\ndescription: d\n---\nbody",
            AgentSource::UserSettings,
        );
        assert!(matches!(
            missing_name,
            AgentParseOutcome::MissingRequiredField(_)
        ));
    }

    #[test]
    fn invalid_permission_mode_is_rejected_at_agent_boundary() {
        let outcome = parse_agent_markdown_detailed(
            &fixture_path("bad-mode.md"),
            "---\nname: Explore\ndescription: d\npermissionMode: dangerous\n---\nbody",
            AgentSource::UserSettings,
        );
        let AgentParseOutcome::MissingRequiredField(reason) = outcome else {
            panic!("expected invalid permissionMode to reject the agent");
        };
        assert!(reason.contains("invalid `permissionMode`: dangerous"));
    }

    #[tokio::test]
    async fn missing_required_field_emits_warning_and_skips_file() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let user_agents = home.join("agents");
        tokio::fs::create_dir_all(&user_agents).await.unwrap();
        // Missing description → warning, file skipped.
        tokio::fs::write(
            user_agents.join("broken.md"),
            "---\nname: broken\n---\nbody\n",
        )
        .await
        .unwrap();
        // A plain doc with no frontmatter → silently skipped, no warning.
        tokio::fs::write(user_agents.join("README.md"), "just notes\n")
            .await
            .unwrap();

        let outcome = load_agent_definitions_with_warnings(&home, &cwd)
            .await
            .expect("load");
        assert!(
            !outcome
                .definitions
                .iter()
                .any(|agent| agent.agent_type == "broken")
        );
        assert_eq!(outcome.warnings.len(), 1);
        assert_eq!(outcome.warnings[0].kind, AgentWarningKind::MissingField);
        assert!(outcome.warnings[0].message.contains("description"));
    }

    #[tokio::test]
    async fn duplicate_name_within_one_layer_emits_warning() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let user_agents = home.join("agents");
        tokio::fs::create_dir_all(&user_agents).await.unwrap();
        tokio::fs::write(
            user_agents.join("a-first.md"),
            "---\nname: dup\ndescription: first\n---\nfirst body\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            user_agents.join("b-second.md"),
            "---\nname: dup\ndescription: second\n---\nsecond body\n",
        )
        .await
        .unwrap();

        let outcome = load_agent_definitions_with_warnings(&home, &cwd)
            .await
            .expect("load");
        let dup_count = outcome
            .definitions
            .iter()
            .filter(|agent| agent.agent_type == "dup")
            .count();
        assert_eq!(dup_count, 1, "duplicate kept only once");
        let dup = outcome
            .definitions
            .iter()
            .find(|agent| agent.agent_type == "dup")
            .unwrap();
        // Path-order "first" wins (a-first.md sorts before b-second.md).
        assert_eq!(dup.description, "first");
        assert_eq!(outcome.warnings.len(), 1);
        assert_eq!(outcome.warnings[0].kind, AgentWarningKind::DuplicateName);
    }

    #[tokio::test]
    async fn empty_agents_dir_produces_no_warnings() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        tokio::fs::create_dir_all(home.join("agents"))
            .await
            .unwrap();

        let outcome = load_agent_definitions_with_warnings(&home, &cwd)
            .await
            .expect("load");
        assert!(outcome.warnings.is_empty());
        // Built-ins still present.
        assert!(
            outcome
                .definitions
                .iter()
                .any(|agent| agent.agent_type == "general-purpose")
        );
    }

    #[tokio::test]
    async fn project_override_of_user_is_not_a_duplicate_warning() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        tokio::fs::create_dir_all(home.join("agents"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(cwd.join(".claude").join("agents"))
            .await
            .unwrap();
        tokio::fs::write(
            home.join("agents").join("shared.md"),
            "---\nname: shared\ndescription: user\n---\nuser body\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            cwd.join(".claude").join("agents").join("shared.md"),
            "---\nname: shared\ndescription: project\n---\nproject body\n",
        )
        .await
        .unwrap();

        let outcome = load_agent_definitions_with_warnings(&home, &cwd)
            .await
            .expect("load");
        assert!(
            outcome.warnings.is_empty(),
            "cross-layer override should not warn: {:?}",
            outcome.warnings
        );
    }

    #[tokio::test]
    async fn user_definitions_are_loaded_when_no_project_override() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let user_agents = home.join("agents");
        tokio::fs::create_dir_all(&user_agents).await.unwrap();
        tokio::fs::write(
            user_agents.join("rust-reviewer.md"),
            "---\nname: rust-reviewer\ndescription: review rust code\ntools: Read, Grep\n---\nbe pedantic about ownership.\n",
        )
        .await
        .unwrap();

        let agents = load_agent_definitions(&home, &cwd).await.expect("load");
        let reviewer = agents
            .iter()
            .find(|agent| agent.agent_type == "rust-reviewer")
            .expect("user agent present");
        assert_eq!(reviewer.source, AgentSource::UserSettings);
        assert_eq!(
            reviewer
                .tools
                .as_deref()
                .map(<[std::string::String]>::to_vec),
            Some(vec!["Read".to_string(), "Grep".to_string()])
        );
        assert!(reviewer.prompt.contains("ownership"));
    }

    #[test]
    fn agent_load_warning_is_serializable() {
        let warning = AgentLoadWarning {
            kind: AgentWarningKind::MissingField,
            source: AgentSource::UserSettings,
            path: Some(PathBuf::from("/home/.claude/agents/broken.md")),
            agent_type: None,
            message: "missing description".to_string(),
        };
        let json = serde_json::to_value(&warning).expect("serializes");
        assert_eq!(json["kind"], "missing_field");
        assert_eq!(json["source"], "user_settings");
        assert_eq!(json["message"], "missing description");

        let dup_warning = AgentLoadWarning {
            kind: AgentWarningKind::DuplicateName,
            source: AgentSource::Plugin {
                plugin_id: "demo@market".to_string(),
            },
            path: None,
            agent_type: Some("worker".to_string()),
            message: "duplicate".to_string(),
        };
        let json = serde_json::to_value(&dup_warning).expect("serializes");
        assert_eq!(json["kind"], "duplicate_name");
        assert_eq!(json["source"]["plugin"]["plugin_id"], "demo@market");
    }

    #[tokio::test]
    async fn malformed_agent_files_produce_warnings_not_panics() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let user_agents = home.join("agents");
        tokio::fs::create_dir_all(&user_agents).await.unwrap();
        // Frontmatter with empty required fields.
        tokio::fs::write(
            user_agents.join("garbage.md"),
            "---\nname: \ndescription: \n---\nbody\n",
        )
        .await
        .unwrap();
        // Frontmatter opened but never closed.
        tokio::fs::write(
            user_agents.join("unclosed.md"),
            "---\nname: unclosed\ndescription: oops\nstill going\n",
        )
        .await
        .unwrap();
        // Empty file.
        tokio::fs::write(user_agents.join("empty.md"), "")
            .await
            .unwrap();

        let outcome = load_agent_definitions_with_warnings(&home, &cwd)
            .await
            .expect("must not panic");
        // None of the broken files should appear as definitions.
        assert!(
            !outcome
                .definitions
                .iter()
                .any(|agent| agent.agent_type == "garbage"
                    || agent.agent_type == "unclosed"
                    || agent.agent_type.is_empty())
        );
        // The garbage file (empty name) should produce a MissingField warning.
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.kind == AgentWarningKind::MissingField)
        );
    }
}
