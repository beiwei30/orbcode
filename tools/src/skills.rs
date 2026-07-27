use std::path::{Path, PathBuf};
use std::time::Duration;

use orbcode_config::{bundled_skills_dir, load_plugin_registry, plugin_skill_roots};
use orbcode_mcp::{McpPromptResult, McpRegistry};
use serde_json::{Value, json};

use crate::payload::{parse_payload, string_field};
use crate::{ToolContext, ToolError, ToolOutcome, ToolRegistry};

/// Where a skill was discovered. The `Plugin` variant carries the canonical
/// `name@marketplace` id so `/skills` and diagnostics can attribute the skill
/// back to the plugin that contributed it. `Mcp` skills come from trusted MCP
/// server prompts and have the lowest priority.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SkillSource {
    User,
    Project,
    Plugin { plugin_id: String },
    Bundled,
    Mcp { server_id: String },
}

impl SkillSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Plugin { .. } => "plugin",
            Self::Bundled => "bundled",
            Self::Mcp { .. } => "mcp",
        }
    }

    /// Merge priority for same-name dedup. Higher wins. Matches the TypeScript
    /// CLI ordering: project > plugin > user > bundled > mcp.
    fn priority(&self) -> u8 {
        match self {
            Self::Project => 4,
            Self::Plugin { .. } => 3,
            Self::User => 2,
            Self::Bundled => 1,
            Self::Mcp { .. } => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillDefinition {
    pub name: String,
    pub description: Option<String>,
    /// `when_to_use` frontmatter: a hint for when the model should invoke the
    /// skill. Surfaced to skill discovery/listing without loading the body.
    pub when_to_use: Option<String>,
    /// `allowed-tools` frontmatter parsed into a list of tool identifiers.
    pub allowed_tools: Vec<String>,
    /// `model` frontmatter hint (the `inherit` sentinel is dropped to `None`).
    pub model: Option<String>,
    /// Absolute paths of reference files / scripts shipped alongside `SKILL.md`
    /// in the skill directory (sorted). Empty for the loose top-level
    /// `SKILL.md` form.
    pub assets: Vec<PathBuf>,
    pub path: PathBuf,
    pub body: String,
    pub source: SkillSource,
}

/// Pre-fetched MCP prompt data suitable for conversion to a [`SkillDefinition`].
/// The caller (e.g. `AppServer`) is responsible for listing prompts from MCP
/// servers and filtering to only those marked as skills (e.g. via `skill: true`
/// metadata). Only prompts with `trusted: true` are loaded; untrusted entries
/// are silently skipped.
#[derive(Clone, Debug)]
pub struct McpSkillPrompt {
    pub server_id: String,
    pub prompt_name: String,
    pub description: String,
    /// Rendered prompt body (from `prompts/get`). When `None`, the
    /// `description` is used as the skill body.
    pub body: Option<String>,
    pub trusted: bool,
}

impl Default for SkillDefinition {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: None,
            when_to_use: None,
            allowed_tools: Vec::new(),
            model: None,
            assets: Vec::new(),
            path: PathBuf::new(),
            body: String::new(),
            source: SkillSource::User,
        }
    }
}

/// Discover skills from every source: bundled (shipped with the CLI), user
/// (`<home>/skills`), project (`<cwd>/.claude/skills`), and every enabled
/// plugin's `skills/` directory. The bundled root is resolved via
/// [`orbcode_config::bundled_skills_dir`]; use [`load_skill_definitions_with_bundled`]
/// to inject an explicit bundled root.
///
/// Same-name entries are deduped by source priority (project > plugin > user >
/// bundled > mcp). Plugin skills use a namespaced name (`pluginName:skillName`)
/// so they occupy their own namespace and cannot shadow user/project/bundled
/// skills. The returned list is sorted by name. Missing roots are skipped
/// silently and a missing/malformed plugin never breaks the user's skill set.
pub async fn load_skill_definitions(
    home_dir: &Path,
    cwd: &Path,
) -> std::io::Result<Vec<SkillDefinition>> {
    let bundled = bundled_skills_dir();
    load_skill_definitions_with_bundled_and_mcp(home_dir, cwd, bundled.as_deref(), &[]).await
}

/// Variant of [`load_skill_definitions`] that takes an explicit bundled-skills
/// root (or `None` to skip bundled discovery). Primarily useful for tests and
/// callers that resolve the bundled location themselves.
pub async fn load_skill_definitions_with_bundled(
    home_dir: &Path,
    cwd: &Path,
    bundled_root: Option<&Path>,
) -> std::io::Result<Vec<SkillDefinition>> {
    load_skill_definitions_with_bundled_and_mcp(home_dir, cwd, bundled_root, &[]).await
}

/// Variant of [`load_skill_definitions`] that includes MCP-provided skills from
/// trusted servers. The bundled root is auto-resolved.
pub async fn load_skill_definitions_with_mcp(
    home_dir: &Path,
    cwd: &Path,
    mcp_skills: &[McpSkillPrompt],
) -> std::io::Result<Vec<SkillDefinition>> {
    let bundled = bundled_skills_dir();
    load_skill_definitions_with_bundled_and_mcp(home_dir, cwd, bundled.as_deref(), mcp_skills).await
}

/// Load local skills and merge trusted MCP prompt skills when discovery finishes
/// within `discovery_timeout`. Slow or failing MCP discovery never blocks local
/// skills; a timed-out discovery task is detached so in-flight stdio requests
/// can still return their client to the registry slot.
pub async fn load_skill_definitions_with_bounded_mcp(
    home_dir: &Path,
    cwd: &Path,
    mcp: &McpRegistry,
    discovery_timeout: Duration,
) -> std::io::Result<Vec<SkillDefinition>> {
    load_skill_definitions_with_bounded_mcp_visible_to(home_dir, cwd, mcp, None, discovery_timeout)
        .await
}

/// Session-scoped variant of [`load_skill_definitions_with_bounded_mcp`].
pub async fn load_skill_definitions_with_bounded_mcp_for_session(
    home_dir: &Path,
    cwd: &Path,
    mcp: &McpRegistry,
    session_id: &str,
    discovery_timeout: Duration,
) -> std::io::Result<Vec<SkillDefinition>> {
    load_skill_definitions_with_bounded_mcp_visible_to(
        home_dir,
        cwd,
        mcp,
        Some(session_id.to_string()),
        discovery_timeout,
    )
    .await
}

async fn load_skill_definitions_with_bounded_mcp_visible_to(
    home_dir: &Path,
    cwd: &Path,
    mcp: &McpRegistry,
    session_id: Option<String>,
    discovery_timeout: Duration,
) -> std::io::Result<Vec<SkillDefinition>> {
    let local_skills = load_skill_definitions(home_dir, cwd).await?;
    let Some(mcp_skills) =
        discover_mcp_skill_prompts_with_timeout(mcp.clone(), session_id, discovery_timeout).await
    else {
        return Ok(local_skills);
    };
    if mcp_skills.is_empty() {
        return Ok(local_skills);
    }

    match load_skill_definitions_with_mcp(home_dir, cwd, &mcp_skills).await {
        Ok(skills) => Ok(skills),
        Err(error) => {
            eprintln!("warning: failed to load skill definitions: {error}");
            Ok(local_skills)
        }
    }
}

async fn discover_mcp_skill_prompts_with_timeout(
    mcp: McpRegistry,
    session_id: Option<String>,
    discovery_timeout: Duration,
) -> Option<Vec<McpSkillPrompt>> {
    let mut handle =
        tokio::spawn(async move { collect_mcp_skill_prompts(&mcp, session_id.as_deref()).await });
    tokio::select! {
        result = &mut handle => match result {
            Ok(skills) => Some(skills),
            Err(error) => {
                eprintln!("warning: MCP skill prompt discovery task failed: {error}");
                Some(Vec::new())
            }
        },
        _ = tokio::time::sleep(discovery_timeout) => {
            // Detached late monitor; discovery timed out, but the task should
            // still be observed so registry-held MCP clients can be returned.
            let _late_discovery_monitor_handle = tokio::spawn(async move {
                if let Err(error) = handle.await {
                    eprintln!("warning: MCP skill prompt discovery task failed after timeout: {error}");
                }
            });
            eprintln!(
                "warning: timed out discovering MCP skill prompts after {}ms",
                discovery_timeout.as_millis()
            );
            None
        }
    }
}

/// Full variant that accepts an explicit bundled root and MCP skill data.
/// Combines all skill sources with proper priority ordering.
pub async fn load_skill_definitions_with_bundled_and_mcp(
    home_dir: &Path,
    cwd: &Path,
    bundled_root: Option<&Path>,
    mcp_skills: &[McpSkillPrompt],
) -> std::io::Result<Vec<SkillDefinition>> {
    // Collect candidates in ascending priority order so the dedup pass keeps the
    // highest-priority entry for each name (and, on ties, the last read).
    let mut candidates = Vec::<SkillDefinition>::new();

    // MCP skills have the lowest priority — loaded first.
    for prompt in mcp_skills {
        if !prompt.trusted {
            continue;
        }
        candidates.push(mcp_prompt_to_skill(prompt));
    }

    if let Some(root) = bundled_root {
        candidates.extend(load_dir_skills(root, SkillSource::Bundled).await?);
    }
    candidates.extend(load_dir_skills(&home_dir.join("skills"), SkillSource::User).await?);

    // Plugin skills: load the discovery registry but never let load failures
    // (missing/malformed plugins) break the user's skill set.
    if let Ok(registry) = load_plugin_registry(home_dir, cwd).await {
        for root in plugin_skill_roots(&registry) {
            let skill_file = root.skill_dir.join("SKILL.md");
            if !tokio::fs::try_exists(&skill_file).await? {
                continue;
            }
            let contents = tokio::fs::read_to_string(&skill_file).await?;
            let source = SkillSource::Plugin {
                plugin_id: root.plugin_id.clone(),
            };
            if let Some(mut skill) = parse_skill_definition(&skill_file, &contents, source) {
                skill.name = root.namespaced_name;
                skill.assets = collect_skill_assets(&root.skill_dir).await?;
                candidates.push(skill);
            }
        }
    }

    candidates
        .extend(load_dir_skills(&cwd.join(".claude").join("skills"), SkillSource::Project).await?);

    Ok(dedupe_by_priority(candidates))
}

/// Load every skill in a `<root>/<skill-name>/SKILL.md` directory tree. Bundled,
/// user, and project roots all share this layout. Missing roots yield an empty
/// list. Reference files alongside each `SKILL.md` are recorded as `assets`.
async fn load_dir_skills(
    root: &Path,
    source: SkillSource,
) -> std::io::Result<Vec<SkillDefinition>> {
    let mut skills = Vec::new();
    if !tokio::fs::try_exists(root).await? {
        return Ok(skills);
    }
    let mut entries = tokio::fs::read_dir(root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        // Directory form (`skill-name/SKILL.md`) is canonical and the only form
        // that carries reference assets; a loose top-level `SKILL.md` is still
        // accepted but without sibling-asset enumeration.
        let (skill_file, skill_dir) = if path.is_dir() {
            (path.join("SKILL.md"), Some(path.clone()))
        } else if path.file_name().and_then(|value| value.to_str()) == Some("SKILL.md") {
            (path.clone(), None)
        } else {
            continue;
        };
        if !tokio::fs::try_exists(&skill_file).await? {
            continue;
        }
        let contents = tokio::fs::read_to_string(&skill_file).await?;
        if let Some(mut skill) = parse_skill_definition(&skill_file, &contents, source.clone()) {
            if let Some(dir) = &skill_dir {
                skill.assets = collect_skill_assets(dir).await?;
            }
            skills.push(skill);
        }
    }
    Ok(skills)
}

/// Enumerate reference files / scripts shipped in a skill directory (everything
/// except the top-level `SKILL.md`), recursively, returned sorted.
async fn collect_skill_assets(skill_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut assets = Vec::new();
    let mut stack = vec![skill_dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&current).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let is_top_level_skill_file = current == skill_dir
                    && path.file_name().and_then(|value| value.to_str()) == Some("SKILL.md");
                if !is_top_level_skill_file {
                    assets.push(path);
                }
            }
        }
    }
    assets.sort();
    Ok(assets)
}

/// Resolve same-name collisions across all sources keeping the highest-priority
/// entry (project > plugin > user > bundled). Candidates must be supplied in
/// ascending priority order so equal-priority ties keep the last read. The
/// result is sorted by name.
fn dedupe_by_priority(candidates: Vec<SkillDefinition>) -> Vec<SkillDefinition> {
    let mut result = Vec::<SkillDefinition>::new();
    for skill in candidates {
        match result
            .iter()
            .position(|existing| existing.name == skill.name)
        {
            Some(pos) => {
                if skill.source.priority() >= result[pos].source.priority() {
                    result[pos] = skill;
                }
            }
            None => result.push(skill),
        }
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    result
}

/// Resolve a list of requested skill names (case-insensitive) against the
/// available set, preserving requested order, deduplicating, and dropping
/// unknown names. Returns `(matched, unknown)` so callers can decide how to
/// surface missing skills.
pub fn resolve_requested_skills<'a>(
    available: &'a [SkillDefinition],
    requested: &[String],
) -> (Vec<&'a SkillDefinition>, Vec<String>) {
    let mut matched = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut unknown = Vec::new();
    for raw in requested {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_ascii_lowercase();
        if !seen.insert(key.clone()) {
            continue;
        }
        match available
            .iter()
            .find(|skill| skill.name.eq_ignore_ascii_case(trimmed))
        {
            Some(skill) => matched.push(skill),
            None => unknown.push(trimmed.to_string()),
        }
    }
    (matched, unknown)
}

pub(crate) async fn load_available_skills(
    context: &ToolContext,
) -> Result<Vec<SkillDefinition>, ToolError> {
    if let Some(skills) = &context.skill_definitions {
        return Ok(skills.clone());
    }
    load_skill_definitions(&context.home_dir, &context.cwd)
        .await
        .map_err(ToolError::from)
}

async fn collect_mcp_skill_prompts(
    mcp: &McpRegistry,
    session_id: Option<&str>,
) -> Vec<McpSkillPrompt> {
    let servers = match session_id {
        Some(session_id) => mcp.list_servers_for_session(session_id).await,
        None => mcp.list_servers().await,
    };
    let mut prompts = Vec::new();

    for server in servers {
        if !server.enabled || !server.trust.is_trusted() {
            continue;
        }
        let listed = match session_id {
            Some(session_id) => mcp.list_prompts_for_session(session_id, &server.id).await,
            None => mcp.list_prompts(&server.id).await,
        };
        let listed = match listed {
            Ok(prompts) => prompts,
            Err(error) => {
                eprintln!(
                    "warning: failed to list MCP skill prompts from `{}`: {error}",
                    server.id
                );
                continue;
            }
        };
        for prompt in listed {
            if !mcp_prompt_is_skill(&prompt) {
                continue;
            }
            if prompt.arguments.iter().any(|argument| argument.required) {
                eprintln!(
                    "warning: skipping MCP skill prompt `{}:{}` because required prompt arguments are not supported in skill definitions",
                    server.id, prompt.name
                );
                continue;
            }
            let result = match session_id {
                Some(session_id) => {
                    mcp.get_prompt_for_session(session_id, &server.id, &prompt.name, json!({}))
                        .await
                }
                None => mcp.get_prompt(&server.id, &prompt.name, json!({})).await,
            };
            let body = match result {
                Ok(result) => prompt_result_body(&result),
                Err(error) => {
                    eprintln!(
                        "warning: failed to render MCP skill prompt `{}:{}`: {error}",
                        server.id, prompt.name
                    );
                    None
                }
            };
            prompts.push(McpSkillPrompt {
                server_id: server.id.clone(),
                prompt_name: prompt.name,
                description: prompt.description,
                body,
                trusted: true,
            });
        }
    }

    prompts
}

fn prompt_result_body(result: &McpPromptResult) -> Option<String> {
    let body = result
        .messages
        .iter()
        .filter_map(|message| message.content.text.as_deref())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!body.is_empty()).then_some(body)
}

impl ToolRegistry {
    pub(crate) async fn skill(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let payload = parse_payload(input)?;
        let requested = string_field(&payload, "skill")
            .or_else(|| payload.as_str().map(str::to_string))
            .ok_or_else(|| ToolError::InvalidInput("skill requires `skill`".into()))?;
        let args = string_field(&payload, "args").unwrap_or_default();
        let normalized = requested.trim().trim_start_matches('/').to_string();
        if normalized.is_empty() {
            return Err(ToolError::InvalidInput("skill name cannot be empty".into()));
        }

        let skills = load_available_skills(context).await?;
        let Some(skill) = skills
            .iter()
            .find(|skill| skill.name.eq_ignore_ascii_case(&normalized))
        else {
            let available = skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ToolError::InvalidInput(format!(
                "unknown skill `{normalized}`. Available skills: {}",
                if available.is_empty() {
                    "(none discovered)".to_string()
                } else {
                    available
                }
            )));
        };

        let expanded = skill
            .body
            .replace("$ARGUMENTS", &args)
            .replace("{{args}}", &args);
        Ok(ToolOutcome {
            name: "skill".to_string(),
            summary: format!("Loaded skill `{}`.", skill.name),
            output: format!(
                "Skill: {}\nDescription: {}\nSource: {}\n\nInstructions:\n{}",
                skill.name,
                skill.description.as_deref().unwrap_or(""),
                skill.path.display(),
                expanded.trim()
            ),
            metadata: None,
            changed_paths: Vec::new(),
        })
    }
}

fn mcp_prompt_to_skill(prompt: &McpSkillPrompt) -> SkillDefinition {
    let namespaced_name = format!("{}:{}", prompt.server_id, prompt.prompt_name);
    let sentinel_path = PathBuf::from(format!("mcp://{}/{}", prompt.server_id, prompt.prompt_name));
    let source = SkillSource::Mcp {
        server_id: prompt.server_id.clone(),
    };
    let body_text = prompt.body.as_deref().unwrap_or(&prompt.description);

    if let Some(mut skill) = parse_skill_definition(&sentinel_path, body_text, source.clone()) {
        skill.name = namespaced_name;
        if skill.description.is_none() {
            skill.description = non_empty(&prompt.description);
        }
        return skill;
    }

    SkillDefinition {
        name: namespaced_name,
        description: non_empty(&prompt.description),
        body: body_text.to_string(),
        path: sentinel_path,
        source,
        ..SkillDefinition::default()
    }
}

fn parse_skill_definition(
    path: &Path,
    contents: &str,
    source: SkillSource,
) -> Option<SkillDefinition> {
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (frontmatter, body) = if let Some(rest) = trimmed.strip_prefix("---\n") {
        if let Some((frontmatter, body)) = rest.split_once("\n---\n") {
            (Some(frontmatter), body)
        } else {
            (None, trimmed)
        }
    } else {
        (None, trimmed)
    };
    let mut name = path
        .parent()
        .and_then(|value| value.file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("skill")
        .to_string();
    let mut description = None;
    let mut when_to_use = None;
    let mut allowed_tools = Vec::new();
    let mut model = None;
    if let Some(frontmatter) = frontmatter {
        for line in frontmatter.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let value = unquote(value.trim());
            match key {
                // `name` mirrors the directory name unless overridden.
                "name" if !value.is_empty() => name = value.to_string(),
                "description" => description = non_empty(value),
                // Accept both the snake_case (TS frontmatter) and camelCase spellings.
                "when_to_use" | "whenToUse" => when_to_use = non_empty(value),
                "allowed-tools" | "allowed_tools" | "allowedTools" => {
                    allowed_tools = parse_tool_list(value)
                }
                // The `inherit` sentinel means "use the active model" — drop it.
                "model" if !value.is_empty() && value != "inherit" => {
                    model = Some(value.to_string())
                }
                _ => {}
            }
        }
    }
    Some(SkillDefinition {
        name,
        description,
        when_to_use,
        allowed_tools,
        model,
        assets: Vec::new(),
        path: path.to_path_buf(),
        body: body.trim().to_string(),
        source,
    })
}

/// Strip a single layer of matching quotes from a frontmatter scalar.
fn unquote(value: &str) -> &str {
    value.trim_matches('"').trim_matches('\'').trim()
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Parse an `allowed-tools` frontmatter scalar. Accepts a comma-separated list
/// (`Read, Write, Bash`) and an inline array (`[Read, Write]`).
fn parse_tool_list(value: &str) -> Vec<String> {
    let inner = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(value);
    inner
        .split(',')
        .map(|item| unquote(item.trim()).to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn mcp_prompt_is_skill(prompt: &orbcode_mcp::McpPrompt) -> bool {
    prompt.skill || metadata_marks_skill(&prompt.extra)
}

fn metadata_marks_skill(metadata: &serde_json::Map<String, Value>) -> bool {
    metadata.get("skill").is_some_and(value_marks_skill)
        || metadata.get("isSkill").is_some_and(value_marks_skill)
        || metadata
            .get("_meta")
            .and_then(Value::as_object)
            .is_some_and(metadata_marks_skill)
        || metadata
            .get("metadata")
            .and_then(Value::as_object)
            .is_some_and(metadata_marks_skill)
}

fn value_marks_skill(value: &Value) -> bool {
    value.as_bool() == Some(true)
        || value
            .as_str()
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

#[cfg(test)]
mod loader_tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn load_skill_definitions_returns_empty_when_no_roots_exist() {
        let temp = tempdir().unwrap();
        let skills = load_skill_definitions(&temp.path().join("home"), &temp.path().join("cwd"))
            .await
            .expect("load");
        assert!(skills.is_empty());
    }

    #[test]
    fn mcp_prompt_skill_marker_accepts_wrapped_metadata() {
        let prompt = orbcode_mcp::McpPrompt {
            name: "wrapped".to_string(),
            description: String::new(),
            arguments: Vec::new(),
            skill: false,
            extra: serde_json::Map::from_iter([(
                "metadata".to_string(),
                serde_json::json!({"skill": "true"}),
            )]),
        };

        assert!(mcp_prompt_is_skill(&prompt));
    }

    #[tokio::test]
    async fn bounded_mcp_skill_discovery_timeout_does_not_cancel_stdio_request() {
        use orbcode_mcp::{
            McpAuth, McpRegistry, McpServerConfig, McpServerStatus, McpServerTrust, McpTransport,
        };
        use std::collections::BTreeMap;
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let script = temp.path().join("slow-stdio-mcp.sh");
        let marker = temp.path().join("starts.txt");
        tokio::fs::create_dir_all(&home).await.unwrap();
        tokio::fs::create_dir_all(&cwd).await.unwrap();
        tokio::fs::write(
            &script,
            r#"#!/bin/sh
printf 'started\n' >> "$MARKER_PATH"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  if [ -z "$id" ]; then
    continue
  fi
  case "$line" in
    *\"method\":\"initialize\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{},"prompts":{}},"serverInfo":{"name":"slow-stdio","version":"0.1.0"}}}\n' "$id"
      ;;
    *\"method\":\"prompts/list\"*)
      sleep 0.7
      printf '{"jsonrpc":"2.0","id":%s,"result":{"prompts":[]}}\n' "$id"
      ;;
    *\"method\":\"tools/list\"*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}\n' "$id"
      ;;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
  esac
done
"#,
        )
        .await
        .unwrap();
        let mut permissions = tokio::fs::metadata(&script).await.unwrap().permissions();
        permissions.set_mode(0o755);
        tokio::fs::set_permissions(&script, permissions)
            .await
            .unwrap();

        let registry = McpRegistry::load(&home, &cwd).await.expect("registry");
        registry
            .upsert_server(McpServerConfig {
                id: "slow".to_string(),
                transport: McpTransport::Stdio,
                endpoint: script.display().to_string(),
                args: Vec::new(),
                env: BTreeMap::from([("MARKER_PATH".to_string(), marker.display().to_string())]),
                cwd: None,
                headers: BTreeMap::new(),
                enabled: true,
                status: McpServerStatus::Ready,
                error: None,
                summary: "Slow stdio".to_string(),
                auth: McpAuth::None,
                trust: McpServerTrust::Trusted,
                transport_type_hint: None,
                source: None,
            })
            .await
            .expect("upsert stdio server");

        let skills = load_skill_definitions_with_bounded_mcp(
            &home,
            &cwd,
            &registry,
            Duration::from_millis(100),
        )
        .await
        .expect("load skills");

        assert!(skills.is_empty());
        tokio::time::sleep(Duration::from_millis(1000)).await;
        registry
            .list_tools("slow")
            .await
            .expect("stdio client should be returned after timed-out discovery finishes");

        let starts = tokio::fs::read_to_string(&marker).await.expect("starts");
        assert_eq!(
            starts.lines().count(),
            1,
            "timed-out skill discovery must not kill and restart the stdio MCP server"
        );
    }

    #[tokio::test]
    async fn load_skill_definitions_prefers_project_over_user_with_same_name() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let user_skill = home.join("skills").join("rust-patterns");
        let project_skill = cwd.join(".claude").join("skills").join("rust-patterns");
        tokio::fs::create_dir_all(&user_skill).await.unwrap();
        tokio::fs::create_dir_all(&project_skill).await.unwrap();
        tokio::fs::write(
            user_skill.join("SKILL.md"),
            "---\nname: rust-patterns\ndescription: user version\n---\nuser body\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            project_skill.join("SKILL.md"),
            "---\nname: rust-patterns\ndescription: project version\n---\nproject body\n",
        )
        .await
        .unwrap();
        // Also add a project-only skill.
        let only_project = cwd.join(".claude").join("skills").join("review-helper");
        tokio::fs::create_dir_all(&only_project).await.unwrap();
        tokio::fs::write(
            only_project.join("SKILL.md"),
            "---\nname: review-helper\ndescription: project-only\n---\nreview body\n",
        )
        .await
        .unwrap();

        let skills = load_skill_definitions(&home, &cwd).await.expect("load");
        let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
        assert_eq!(names, vec!["review-helper", "rust-patterns"]);

        let rust_patterns = skills
            .iter()
            .find(|skill| skill.name == "rust-patterns")
            .unwrap();
        assert_eq!(
            rust_patterns.description.as_deref(),
            Some("project version")
        );
        assert_eq!(rust_patterns.body, "project body");
    }

    #[tokio::test]
    async fn load_skill_definitions_includes_enabled_plugin_skills_namespaced() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo");
        let plugin_skill = plugin_root.join("skills").join("hello");
        tokio::fs::create_dir_all(&plugin_skill).await.unwrap();
        tokio::fs::write(
            plugin_skill.join("SKILL.md"),
            "---\nname: hello\ndescription: from plugin\n---\nplugin body",
        )
        .await
        .unwrap();
        tokio::fs::create_dir_all(plugin_root.join(".claude-plugin"))
            .await
            .unwrap();
        tokio::fs::write(
            plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo"}"#,
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

        let skills = load_skill_definitions(&home, &cwd).await.expect("load");
        let plugin_skill = skills
            .iter()
            .find(|skill| skill.name == "demo:hello")
            .expect("plugin skill discovered with namespace");
        assert_eq!(plugin_skill.description.as_deref(), Some("from plugin"));
        assert!(matches!(plugin_skill.source, SkillSource::Plugin { .. }));
    }

    #[tokio::test]
    async fn load_skill_definitions_skips_disabled_plugins() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo");
        let plugin_skill = plugin_root.join("skills").join("hello");
        tokio::fs::create_dir_all(&plugin_skill).await.unwrap();
        tokio::fs::write(plugin_skill.join("SKILL.md"), "---\nname: hello\n---\nbody")
            .await
            .unwrap();
        tokio::fs::create_dir_all(plugin_root.join(".claude-plugin"))
            .await
            .unwrap();
        tokio::fs::write(
            plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo"}"#,
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
        tokio::fs::write(
            home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":false}}"#,
        )
        .await
        .unwrap();

        let skills = load_skill_definitions(&home, &cwd).await.expect("load");
        assert!(!skills.iter().any(|skill| skill.name.starts_with("demo:")));
    }

    #[tokio::test]
    async fn load_skill_definitions_ignores_unrelated_entries() {
        let temp = tempdir().unwrap();
        let cwd = temp.path().join("project");
        let skills_root = cwd.join(".claude").join("skills");
        tokio::fs::create_dir_all(&skills_root).await.unwrap();
        // Loose .md without SKILL.md naming should be skipped.
        tokio::fs::write(skills_root.join("notes.md"), "not a skill")
            .await
            .unwrap();
        // SKILL.md inside a dir is the canonical form.
        let dir = skills_root.join("good-skill");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(
            dir.join("SKILL.md"),
            "---\nname: good-skill\n---\ngood body\n",
        )
        .await
        .unwrap();

        let skills = load_skill_definitions(&temp.path().join("home"), &cwd)
            .await
            .expect("load");
        let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
        assert_eq!(names, vec!["good-skill"]);
    }

    fn make_skill(name: &str, body: &str) -> SkillDefinition {
        SkillDefinition {
            name: name.to_string(),
            path: PathBuf::from(format!("/skills/{name}/SKILL.md")),
            body: body.to_string(),
            source: SkillSource::User,
            ..SkillDefinition::default()
        }
    }

    #[test]
    fn resolve_requested_skills_preserves_order_and_dedupes() {
        let available = vec![
            make_skill("alpha", "A"),
            make_skill("beta", "B"),
            make_skill("gamma", "C"),
        ];
        let requested = vec![
            "gamma".to_string(),
            "ALPHA".to_string(),
            "alpha".to_string(),   // duplicate (case-insensitive)
            "missing".to_string(), // unknown
            "  beta ".to_string(), // whitespace trimmed
            String::new(),         // empty skipped
        ];
        let (matched, unknown) = resolve_requested_skills(&available, &requested);
        let names: Vec<&str> = matched.iter().map(|skill| skill.name.as_str()).collect();
        assert_eq!(names, vec!["gamma", "alpha", "beta"]);
        assert_eq!(unknown, vec!["missing".to_string()]);
    }

    async fn write_skill(dir: &Path, contents: &str) {
        tokio::fs::create_dir_all(dir).await.unwrap();
        tokio::fs::write(dir.join("SKILL.md"), contents)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn discovers_bundled_skills_with_bundled_source() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let bundled = temp.path().join("bundled");
        write_skill(
            &bundled.join("greeter"),
            "---\nname: greeter\ndescription: bundled greeter\n---\nhello body",
        )
        .await;

        let skills = load_skill_definitions_with_bundled(&home, &cwd, Some(bundled.as_path()))
            .await
            .expect("load");
        let greeter = skills
            .iter()
            .find(|skill| skill.name == "greeter")
            .expect("bundled skill discovered");
        assert_eq!(greeter.source, SkillSource::Bundled);
        assert_eq!(greeter.description.as_deref(), Some("bundled greeter"));
        assert_eq!(greeter.body, "hello body");
    }

    #[tokio::test]
    async fn project_and_user_override_bundled_with_same_name() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let bundled = temp.path().join("bundled");

        // Same name across all three: project must win, then user, then bundled.
        write_skill(
            &bundled.join("shared"),
            "---\nname: shared\ndescription: bundled\n---\nbundled body",
        )
        .await;
        write_skill(
            &home.join("skills").join("shared"),
            "---\nname: shared\ndescription: user\n---\nuser body",
        )
        .await;
        write_skill(
            &cwd.join(".claude").join("skills").join("shared"),
            "---\nname: shared\ndescription: project\n---\nproject body",
        )
        .await;
        // A bundled-only skill survives because nothing overrides it.
        write_skill(
            &bundled.join("bundled-only"),
            "---\nname: bundled-only\ndescription: only bundled\n---\nbody",
        )
        .await;

        let skills = load_skill_definitions_with_bundled(&home, &cwd, Some(bundled.as_path()))
            .await
            .expect("load");

        let shared = skills.iter().find(|skill| skill.name == "shared").unwrap();
        assert_eq!(shared.source, SkillSource::Project);
        assert_eq!(shared.description.as_deref(), Some("project"));

        let bundled_only = skills
            .iter()
            .find(|skill| skill.name == "bundled-only")
            .unwrap();
        assert_eq!(bundled_only.source, SkillSource::Bundled);
        // Only one `shared` entry remains after dedup.
        assert_eq!(skills.iter().filter(|s| s.name == "shared").count(), 1);
    }

    #[tokio::test]
    async fn parses_full_skill_metadata_and_assets() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let skill_dir = cwd.join(".claude").join("skills").join("rich");
        write_skill(
            &skill_dir,
            concat!(
                "---\n",
                "name: rich\n",
                "description: a rich skill\n",
                "when_to_use: use when refactoring rust\n",
                "allowed-tools: Read, Write, Bash\n",
                "model: claude-opus\n",
                "---\n",
                "Run ${CLAUDE_SKILL_DIR}/scripts/run.sh\n",
            ),
        )
        .await;
        // Reference script + nested reference doc become assets.
        let scripts = skill_dir.join("scripts");
        tokio::fs::create_dir_all(&scripts).await.unwrap();
        tokio::fs::write(scripts.join("run.sh"), "echo hi")
            .await
            .unwrap();
        tokio::fs::write(skill_dir.join("reference.md"), "ref")
            .await
            .unwrap();

        let skills = load_skill_definitions_with_bundled(&home, &cwd, None)
            .await
            .expect("load");
        let rich = skills.iter().find(|skill| skill.name == "rich").unwrap();
        assert_eq!(rich.description.as_deref(), Some("a rich skill"));
        assert_eq!(
            rich.when_to_use.as_deref(),
            Some("use when refactoring rust")
        );
        assert_eq!(rich.allowed_tools, vec!["Read", "Write", "Bash"]);
        assert_eq!(rich.model.as_deref(), Some("claude-opus"));
        let asset_names: Vec<String> = rich
            .assets
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(asset_names.contains(&"run.sh".to_string()));
        assert!(asset_names.contains(&"reference.md".to_string()));
        // SKILL.md itself is never listed as an asset.
        assert!(!asset_names.contains(&"SKILL.md".to_string()));
    }

    #[tokio::test]
    async fn model_inherit_sentinel_is_dropped() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        write_skill(
            &cwd.join(".claude").join("skills").join("inheritor"),
            "---\nname: inheritor\nmodel: inherit\n---\nbody",
        )
        .await;
        let skills = load_skill_definitions_with_bundled(&home, &cwd, None)
            .await
            .expect("load");
        let inheritor = skills.iter().find(|s| s.name == "inheritor").unwrap();
        assert_eq!(inheritor.model, None);
    }

    #[tokio::test]
    async fn plugin_skill_with_missing_manifest_does_not_panic() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo");
        // Plugin install path exists with a skill, but NO plugin.json manifest.
        write_skill(
            &plugin_root.join("skills").join("hello"),
            "---\nname: hello\ndescription: from plugin\n---\nplugin body",
        )
        .await;
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

        // Must not panic even though the manifest is missing.
        let skills = load_skill_definitions_with_bundled(&home, &cwd, None)
            .await
            .expect("load");
        let plugin_skill = skills
            .iter()
            .find(|skill| skill.name == "demo:hello")
            .expect("plugin skill still discovered without manifest");
        assert!(matches!(plugin_skill.source, SkillSource::Plugin { .. }));
    }

    #[tokio::test]
    async fn disabled_plugin_contributes_no_skills_with_bundled_loader() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo");
        write_skill(
            &plugin_root.join("skills").join("hello"),
            "---\nname: hello\n---\nbody",
        )
        .await;
        tokio::fs::create_dir_all(plugin_root.join(".claude-plugin"))
            .await
            .unwrap();
        tokio::fs::write(
            plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo"}"#,
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
        tokio::fs::write(
            home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":false}}"#,
        )
        .await
        .unwrap();

        let skills = load_skill_definitions_with_bundled(&home, &cwd, None)
            .await
            .expect("load");
        assert!(!skills.iter().any(|skill| skill.name.starts_with("demo:")));
    }

    fn make_mcp_prompt(server_id: &str, name: &str, description: &str) -> McpSkillPrompt {
        McpSkillPrompt {
            server_id: server_id.to_string(),
            prompt_name: name.to_string(),
            description: description.to_string(),
            body: None,
            trusted: true,
        }
    }

    #[tokio::test]
    async fn mcp_skill_prompt_appears_in_load_results() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let mcp_skills = vec![make_mcp_prompt(
            "context7",
            "query-docs",
            "Query documentation",
        )];

        let skills = load_skill_definitions_with_bundled_and_mcp(&home, &cwd, None, &mcp_skills)
            .await
            .expect("load");
        let skill = skills
            .iter()
            .find(|s| s.name == "context7:query-docs")
            .expect("MCP skill discovered");
        assert_eq!(skill.description.as_deref(), Some("Query documentation"));
        assert_eq!(skill.body, "Query documentation");
        assert!(matches!(&skill.source, SkillSource::Mcp { server_id } if server_id == "context7"));
        assert_eq!(skill.source.as_str(), "mcp");
    }

    #[tokio::test]
    async fn mcp_skill_with_frontmatter_body_is_parsed() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let mcp_skills = vec![McpSkillPrompt {
            server_id: "my-server".to_string(),
            prompt_name: "deploy".to_string(),
            description: "Deploy the app".to_string(),
            body: Some(
                "---\nname: deploy-skill\ndescription: Deploy to production\nwhen_to_use: when deploying\nallowed-tools: Bash\n---\nRun deploy.sh".to_string(),
            ),
            trusted: true,
        }];

        let skills = load_skill_definitions_with_bundled_and_mcp(&home, &cwd, None, &mcp_skills)
            .await
            .expect("load");
        let skill = skills
            .iter()
            .find(|s| s.name == "my-server:deploy")
            .expect("MCP skill with frontmatter");
        assert_eq!(skill.description.as_deref(), Some("Deploy to production"));
        assert_eq!(skill.when_to_use.as_deref(), Some("when deploying"));
        assert_eq!(skill.allowed_tools, vec!["Bash"]);
        assert_eq!(skill.body, "Run deploy.sh");
    }

    #[tokio::test]
    async fn mcp_skill_priority_lower_than_all_other_sources() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let bundled = temp.path().join("bundled");

        // Same name across MCP and bundled: bundled must win.
        let mcp_skills = vec![McpSkillPrompt {
            server_id: "srv".to_string(),
            prompt_name: "shared".to_string(),
            description: "mcp version".to_string(),
            body: Some("mcp body".to_string()),
            trusted: true,
        }];
        write_skill(
            &bundled.join("shared"),
            "---\nname: shared\ndescription: bundled version\n---\nbundled body",
        )
        .await;

        let skills = load_skill_definitions_with_bundled_and_mcp(
            &home,
            &cwd,
            Some(bundled.as_path()),
            &mcp_skills,
        )
        .await
        .expect("load");
        let shared = skills.iter().find(|s| s.name == "shared").unwrap();
        assert_eq!(shared.source, SkillSource::Bundled);
        assert_eq!(shared.description.as_deref(), Some("bundled version"));
        // MCP namespaced version also exists separately since names differ.
        let mcp_namespaced = skills.iter().find(|s| s.name == "srv:shared");
        assert!(
            mcp_namespaced.is_some(),
            "MCP skill survives under its namespaced name"
        );
    }

    #[tokio::test]
    async fn mcp_skill_overridden_by_user_project_and_plugin() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");

        let mcp_skills = vec![make_mcp_prompt("srv", "greeter", "mcp greeter")];
        write_skill(
            &home.join("skills").join("srv:greeter"),
            "---\nname: srv:greeter\ndescription: user greeter\n---\nuser body",
        )
        .await;

        let skills = load_skill_definitions_with_bundled_and_mcp(&home, &cwd, None, &mcp_skills)
            .await
            .expect("load");
        let greeter = skills.iter().find(|s| s.name == "srv:greeter").unwrap();
        assert_eq!(greeter.source, SkillSource::User);
        assert_eq!(greeter.description.as_deref(), Some("user greeter"));
        assert_eq!(skills.iter().filter(|s| s.name == "srv:greeter").count(), 1);
    }

    #[tokio::test]
    async fn untrusted_mcp_skills_not_loaded() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let mcp_skills = vec![
            McpSkillPrompt {
                server_id: "trusted-srv".to_string(),
                prompt_name: "good".to_string(),
                description: "trusted skill".to_string(),
                body: None,
                trusted: true,
            },
            McpSkillPrompt {
                server_id: "evil-srv".to_string(),
                prompt_name: "bad".to_string(),
                description: "untrusted skill".to_string(),
                body: None,
                trusted: false,
            },
        ];

        let skills = load_skill_definitions_with_bundled_and_mcp(&home, &cwd, None, &mcp_skills)
            .await
            .expect("load");
        assert!(skills.iter().any(|s| s.name == "trusted-srv:good"));
        assert!(
            !skills.iter().any(|s| s.name == "evil-srv:bad"),
            "untrusted MCP skill must not be loaded"
        );
    }

    #[tokio::test]
    async fn mcp_skills_visible_alongside_filesystem_skills() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let bundled = temp.path().join("bundled");

        write_skill(
            &bundled.join("init"),
            "---\nname: init\ndescription: bundled init\n---\ninit body",
        )
        .await;
        write_skill(
            &cwd.join(".claude").join("skills").join("review"),
            "---\nname: review\ndescription: project review\n---\nreview body",
        )
        .await;
        let mcp_skills = vec![make_mcp_prompt("c7", "docs", "query docs from c7")];

        let skills = load_skill_definitions_with_bundled_and_mcp(
            &home,
            &cwd,
            Some(bundled.as_path()),
            &mcp_skills,
        )
        .await
        .expect("load");
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"init"));
        assert!(names.contains(&"review"));
        assert!(names.contains(&"c7:docs"));
    }

    #[tokio::test]
    async fn load_skill_definitions_with_mcp_auto_resolves_bundled() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let mcp_skills = vec![make_mcp_prompt("srv", "helper", "A helper skill")];

        let skills = load_skill_definitions_with_mcp(&home, &cwd, &mcp_skills)
            .await
            .expect("load");
        assert!(skills.iter().any(|s| s.name == "srv:helper"));
    }

    #[test]
    fn mcp_skill_source_priority_is_lowest() {
        let mcp = SkillSource::Mcp {
            server_id: "s".to_string(),
        };
        let bundled = SkillSource::Bundled;
        let user = SkillSource::User;
        let plugin = SkillSource::Plugin {
            plugin_id: "p".to_string(),
        };
        let project = SkillSource::Project;
        assert!(mcp.priority() < bundled.priority());
        assert!(bundled.priority() < user.priority());
        assert!(user.priority() < plugin.priority());
        assert!(plugin.priority() < project.priority());
    }

    /// Full-pipeline integration test: simulates what AppServer.skill_definitions()
    /// would do when all 5 skill sources are present, including name collisions,
    /// MCP frontmatter parsing, trust filtering, and resolve_requested_skills.
    /// Verifies the output contract consumed by `/skills` and `/` completion.
    #[tokio::test]
    async fn integration_full_pipeline_all_sources_with_collisions() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let bundled = temp.path().join("bundled");

        // --- Set up filesystem skills ---
        // Bundled: "init", "shared" (will be overridden by user)
        write_skill(
            &bundled.join("init"),
            "---\nname: init\ndescription: bundled init\n---\ninit body",
        )
        .await;
        write_skill(
            &bundled.join("shared"),
            "---\nname: shared\ndescription: bundled shared\n---\nbundled shared body",
        )
        .await;

        // User: "shared" (overrides bundled), "my-helper"
        write_skill(
            &home.join("skills").join("shared"),
            "---\nname: shared\ndescription: user shared\n---\nuser shared body",
        )
        .await;
        write_skill(
            &home.join("skills").join("my-helper"),
            "---\nname: my-helper\ndescription: user helper\n---\nhelper body",
        )
        .await;

        // Project: "review", "shared" (overrides user)
        write_skill(
            &cwd.join(".claude").join("skills").join("review"),
            "---\nname: review\ndescription: project review\nwhen_to_use: code review\nallowed-tools: Read, Bash\n---\nreview body",
        )
        .await;
        write_skill(
            &cwd.join(".claude").join("skills").join("shared"),
            "---\nname: shared\ndescription: project shared\n---\nproject shared body",
        )
        .await;

        // --- Set up MCP skill prompts ---
        let mcp_skills = vec![
            // Trusted: plain description (no frontmatter body)
            make_mcp_prompt("context7", "query-docs", "Query library documentation"),
            // Trusted: with SKILL.md frontmatter body
            McpSkillPrompt {
                server_id: "deploy-srv".to_string(),
                prompt_name: "deploy".to_string(),
                description: "Deploy the app".to_string(),
                body: Some(
                    "---\ndescription: Deploy to staging\nwhen_to_use: deploying code\nallowed-tools: Bash\nmodel: claude-sonnet\n---\nRun ./deploy.sh $ARGUMENTS"
                        .to_string(),
                ),
                trusted: true,
            },
            // Untrusted: must NOT appear
            McpSkillPrompt {
                server_id: "evil".to_string(),
                prompt_name: "steal-creds".to_string(),
                description: "steals credentials".to_string(),
                body: None,
                trusted: false,
            },
            // Trusted but same name as bundled "init" — MCP version uses
            // namespaced name so no collision
            make_mcp_prompt("my-mcp", "init", "MCP init prompt"),
        ];

        // --- Load all skills ---
        let skills = load_skill_definitions_with_bundled_and_mcp(
            &home,
            &cwd,
            Some(bundled.as_path()),
            &mcp_skills,
        )
        .await
        .expect("load");

        // --- Verify: sorted name list (what /skills displays) ---
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "context7:query-docs",
                "deploy-srv:deploy",
                "init",
                "my-helper",
                "my-mcp:init",
                "review",
                "shared",
            ]
        );

        // --- Verify: untrusted MCP skill excluded ---
        assert!(!names.contains(&"evil:steal-creds"));

        // --- Verify: priority dedup (project > user > bundled for "shared") ---
        let shared = skills.iter().find(|s| s.name == "shared").unwrap();
        assert_eq!(shared.source, SkillSource::Project);
        assert_eq!(shared.description.as_deref(), Some("project shared"));
        assert_eq!(skills.iter().filter(|s| s.name == "shared").count(), 1);

        // --- Verify: bundled "init" survives (no MCP collision due to namespacing) ---
        let init = skills.iter().find(|s| s.name == "init").unwrap();
        assert_eq!(init.source, SkillSource::Bundled);
        let mcp_init = skills.iter().find(|s| s.name == "my-mcp:init").unwrap();
        assert!(
            matches!(&mcp_init.source, SkillSource::Mcp { server_id } if server_id == "my-mcp")
        );

        // --- Verify: MCP skill with frontmatter body is fully parsed ---
        let deploy = skills
            .iter()
            .find(|s| s.name == "deploy-srv:deploy")
            .unwrap();
        assert_eq!(deploy.description.as_deref(), Some("Deploy to staging"));
        assert_eq!(deploy.when_to_use.as_deref(), Some("deploying code"));
        assert_eq!(deploy.allowed_tools, vec!["Bash"]);
        assert_eq!(deploy.model.as_deref(), Some("claude-sonnet"));
        assert_eq!(deploy.body, "Run ./deploy.sh $ARGUMENTS");
        assert!(deploy.path.to_str().unwrap().starts_with("mcp://"));

        // --- Verify: MCP skill without body uses description ---
        let query = skills
            .iter()
            .find(|s| s.name == "context7:query-docs")
            .unwrap();
        assert_eq!(
            query.description.as_deref(),
            Some("Query library documentation")
        );
        assert_eq!(query.body, "Query library documentation");

        // --- Verify: project skill metadata intact ---
        let review = skills.iter().find(|s| s.name == "review").unwrap();
        assert_eq!(review.when_to_use.as_deref(), Some("code review"));
        assert_eq!(review.allowed_tools, vec!["Read", "Bash"]);

        // --- Verify: resolve_requested_skills works with MCP skills ---
        let requested = vec![
            "deploy-srv:deploy".to_string(),
            "review".to_string(),
            "context7:query-docs".to_string(),
            "nonexistent".to_string(),
        ];
        let (matched, unknown) = resolve_requested_skills(&skills, &requested);
        let matched_names: Vec<&str> = matched.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            matched_names,
            vec!["deploy-srv:deploy", "review", "context7:query-docs"]
        );
        assert_eq!(unknown, vec!["nonexistent"]);

        // --- Verify: source labels (what /skills output and context.rs consume) ---
        for skill in &skills {
            let label = skill.source.as_str();
            match &skill.source {
                SkillSource::Mcp { .. } => assert_eq!(label, "mcp"),
                SkillSource::Bundled => assert_eq!(label, "bundled"),
                SkillSource::User => assert_eq!(label, "user"),
                SkillSource::Project => assert_eq!(label, "project"),
                SkillSource::Plugin { .. } => assert_eq!(label, "plugin"),
            }
        }
    }
}
