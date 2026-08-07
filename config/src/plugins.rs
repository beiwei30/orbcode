//! Plugin discovery and registration.
//!
//! Phase 1 only discovers and registers plugin metadata + contributions.
//! It does not execute MCP servers or hooks; integration with hook runner /
//! MCP client is deferred until the dedicated tracks land.
//!
//! Layout:
//! - `<home>/plugins/installed_plugins.json` (V1 or V2) maps plugin IDs
//!   (`name@marketplace`) to one or more installation entries.
//! - `<home>/settings.json` (and the project-local settings cascade) opt
//!   plugins in or out via `enabledPlugins: { "<id>": true|false }`.
//! - Each plugin lives in a directory containing `.claude-plugin/plugin.json`
//!   (or a top-level `plugin.json`) plus optional `commands/`, `agents/`,
//!   `skills/`, `hooks/hooks.json`, `output-styles/`, and `.mcp.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::ConfigError;
use crate::agents::{AgentDefinition, AgentSource, parse_agent_markdown};
use crate::hooks::{ContributedHookSource, HookMatcher, HookProvenance};

/// Settings layer that controls a plugin's enabled state. Mirrors the
/// TypeScript settings cascade (`managed > local > project > user`); only the
/// last layer to mention a plugin wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginScope {
    User,
    Project,
    Local,
}

impl PluginScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
}

/// Installation record from `installed_plugins.json`. Mirrors the subset of
/// fields the loader needs at runtime; we intentionally ignore marketplace
/// install bookkeeping (lastUpdated, gitCommitSha) for discovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginInstallation {
    pub scope: PluginScope,
    pub project_path: Option<PathBuf>,
    pub install_path: PathBuf,
    pub version: Option<String>,
}

/// Parsed `plugin.json` manifest. Phase 1 only consumes metadata fields; the
/// remaining extension fields (`lspServers`, `userConfig`, `channels`) are
/// intentionally not parsed yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginManifest {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author_name: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    /// Plugin-owned open MCP configuration; the MCP loader validates it after
    /// plugin discovery and config must not close over that extension schema.
    pub mcp_servers: Option<Value>,
}

/// A tool schema contributed by a plugin's `plugin.json` `tools` array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginToolDefinition {
    pub name: String,
    pub description: String,
    /// Plugin-owned JSON Schema, intentionally opaque to settings parsing.
    pub input_schema: Value,
    pub requires_permission: bool,
    pub plugin_id: String,
    pub plugin_name: String,
}

/// File-system entry points contributed by a plugin. Each list contains
/// absolute paths under the plugin root. Empty lists mean the directory was
/// missing or held nothing recognisable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginContributions {
    pub command_files: Vec<PathBuf>,
    pub agent_files: Vec<PathBuf>,
    pub skill_dirs: Vec<PathBuf>,
    pub hook_files: Vec<PathBuf>,
    pub output_style_files: Vec<PathBuf>,
    pub mcp_server_files: Vec<PathBuf>,
    pub tools: Vec<PluginToolDefinition>,
}

/// Why a plugin was skipped or could not be loaded. Surfaces in
/// `PluginRegistry::errors` so diagnostics can show *what* failed without
/// preventing other plugins from loading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginLoadError {
    pub plugin_id: String,
    pub install_path: Option<PathBuf>,
    pub message: String,
}

/// A non-fatal diagnostic about an otherwise-loaded plugin. Unlike
/// `PluginLoadError`, a warning does NOT stop the plugin from contributing —
/// it flags a recoverable problem (e.g. an unparseable or missing
/// `plugin.json`) so startup can surface it without refusing to load.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PluginLoadWarning {
    pub plugin_id: String,
    pub path: Option<PathBuf>,
    pub message: String,
}

/// Snapshot of a discovered plugin. `enabled = false` plugins still appear in
/// the registry (for diagnostics + `/plugins` listing) but contribute nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedPlugin {
    pub id: String,
    pub name: String,
    pub marketplace: Option<String>,
    pub installation: PluginInstallation,
    pub manifest_path: Option<PathBuf>,
    pub manifest: PluginManifest,
    pub contributions: PluginContributions,
    pub enabled: bool,
    pub enabled_by: Option<PluginScope>,
}

impl LoadedPlugin {
    pub fn root(&self) -> &Path {
        &self.installation.install_path
    }

    pub fn effective_version(&self) -> Option<&str> {
        self.manifest
            .version
            .as_deref()
            .or(self.installation.version.as_deref())
    }
}

/// Result of `load_plugin_registry`. Callers downstream can iterate the
/// enabled plugins to fold their contributions into existing loaders.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginRegistry {
    pub plugins: Vec<LoadedPlugin>,
    pub errors: Vec<PluginLoadError>,
    /// Non-fatal diagnostics gathered while loading enabled plugins (e.g. a
    /// malformed `plugin.json`). The plugin still loads; these are surfaced
    /// for visibility, not enforcement.
    pub warnings: Vec<PluginLoadWarning>,
}

impl PluginRegistry {
    pub fn enabled(&self) -> impl Iterator<Item = &LoadedPlugin> {
        self.plugins.iter().filter(|plugin| plugin.enabled)
    }
}

/// Discover plugins from the user/project settings cascade and the installed
/// plugins index. Missing files are treated as "no plugins" — never an error.
pub async fn load_plugin_registry(
    home_dir: &Path,
    cwd: &Path,
) -> Result<PluginRegistry, ConfigError> {
    let mut enabled_map: BTreeMap<String, Option<PluginScope>> = BTreeMap::new();
    for (scope, path) in settings_paths(home_dir, cwd) {
        let opt_in = read_enabled_plugins(&path).await?;
        for (id, enabled) in opt_in {
            enabled_map.insert(id, if enabled { Some(scope) } else { None });
        }
    }

    let installations = read_installed_plugins(home_dir).await?;

    let mut plugins: Vec<LoadedPlugin> = Vec::new();
    let mut errors: Vec<PluginLoadError> = Vec::new();
    let mut warnings: Vec<PluginLoadWarning> = Vec::new();

    for (id, enabled_by) in &enabled_map {
        let Some(installation) = installations.get(id).cloned() else {
            errors.push(PluginLoadError {
                plugin_id: id.clone(),
                install_path: None,
                message: "plugin is enabled in settings but missing from installed_plugins.json"
                    .into(),
            });
            continue;
        };

        match load_plugin_from_installation(id, installation.clone()).await {
            Ok(loaded) => {
                for warning in loaded.warnings {
                    warnings.push(PluginLoadWarning {
                        plugin_id: id.clone(),
                        path: warning.path,
                        message: warning.message,
                    });
                }
                plugins.push(LoadedPlugin {
                    id: id.clone(),
                    name: parse_plugin_name(id),
                    marketplace: parse_marketplace_name(id),
                    installation,
                    manifest_path: loaded.manifest_path,
                    manifest: loaded.manifest,
                    contributions: loaded.contributions,
                    enabled: enabled_by.is_some(),
                    enabled_by: *enabled_by,
                });
            }
            Err(message) => errors.push(PluginLoadError {
                plugin_id: id.clone(),
                install_path: Some(installation.install_path.clone()),
                message,
            }),
        }
    }

    plugins.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(PluginRegistry {
        plugins,
        errors,
        warnings,
    })
}

/// Convert plugin agent files into `AgentDefinition`s with namespaced
/// (`pluginName:agentName`) types. Plugins that aren't enabled contribute
/// nothing. Names that collide within one plugin keep the first occurrence.
pub fn plugin_agent_definitions(registry: &PluginRegistry) -> Vec<AgentDefinition> {
    let mut results = Vec::new();
    for plugin in registry.enabled() {
        for path in &plugin.contributions.agent_files {
            let Ok(contents) = std::fs::read_to_string(path) else {
                continue;
            };
            let Some(mut definition) =
                parse_agent_markdown(path, &contents, AgentSource::ProjectSettings)
            else {
                continue;
            };
            let bare = definition.agent_type.clone();
            definition.agent_type = format!("{}:{}", plugin.name, bare);
            definition.source = AgentSource::Plugin {
                plugin_id: plugin.id.clone(),
            };
            results.push(definition);
        }
    }
    results
}

/// Tool definitions contributed by enabled plugins. Disabled plugins
/// contribute nothing. Returns clones of the validated tool definitions
/// already stamped with plugin identity during loading.
pub fn plugin_tool_definitions(registry: &PluginRegistry) -> Vec<PluginToolDefinition> {
    let mut results = Vec::new();
    for plugin in registry.enabled() {
        results.extend(plugin.contributions.tools.iter().cloned());
    }
    results
}

/// Skill directories contributed by enabled plugins, paired with the
/// namespaced display name (`pluginName:skillName`) that the loader should
/// use so plugin skills cannot shadow user/project skills.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSkillRoot {
    pub plugin_id: String,
    pub plugin_name: String,
    pub namespaced_name: String,
    pub skill_dir: PathBuf,
}

/// Resolve the directory that ships bundled skills with the CLI. Bundled skills
/// are the lowest-priority skill source (overridden by user/project/plugin
/// skills with the same name) and have the same on-disk layout as the user and
/// project skill roots (`<root>/<skill-name>/SKILL.md`).
///
/// The location is overridable via the `ORBCODE_BUNDLED_SKILLS_DIR` environment
/// variable (used for packaging and tests); otherwise it is resolved relative to
/// the running executable (`<exe_dir>/bundled/skills`). Returns `None` when no
/// location can be determined. The returned path may not exist — callers treat a
/// missing directory as "no bundled skills" rather than an error.
pub fn bundled_skills_dir() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("ORBCODE_BUNDLED_SKILLS_DIR") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("bundled").join("skills"))
}

pub fn plugin_skill_roots(registry: &PluginRegistry) -> Vec<PluginSkillRoot> {
    let mut roots = Vec::new();
    for plugin in registry.enabled() {
        for dir in &plugin.contributions.skill_dirs {
            let Some(skill_dir_name) = dir.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            roots.push(PluginSkillRoot {
                plugin_id: plugin.id.clone(),
                plugin_name: plugin.name.clone(),
                namespaced_name: format!("{}:{}", plugin.name, skill_dir_name),
                skill_dir: dir.clone(),
            });
        }
    }
    roots
}

/// MCP configuration sources contributed by enabled plugins. File sources are
/// resolved to absolute paths under the plugin root; inline sources are wrapped
/// in `.mcp.json`-compatible shape so the MCP loader can reuse its parser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginMcpConfigSource {
    pub plugin_id: String,
    pub plugin_name: String,
    pub label: String,
    pub kind: PluginMcpConfigSourceKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginMcpConfigSourceKind {
    File(PathBuf),
    Inline(Value),
}

pub fn plugin_mcp_config_sources(registry: &PluginRegistry) -> Vec<PluginMcpConfigSource> {
    let mut sources = Vec::new();
    for plugin in registry.enabled() {
        for path in &plugin.contributions.mcp_server_files {
            sources.push(PluginMcpConfigSource {
                plugin_id: plugin.id.clone(),
                plugin_name: plugin.name.clone(),
                label: path.display().to_string(),
                kind: PluginMcpConfigSourceKind::File(path.clone()),
            });
        }

        if let Some(mcp_servers) = plugin.manifest.mcp_servers.as_ref() {
            append_manifest_mcp_sources(plugin, mcp_servers, &mut sources);
        }
    }
    sources
}

fn append_manifest_mcp_sources(
    plugin: &LoadedPlugin,
    value: &Value,
    sources: &mut Vec<PluginMcpConfigSource>,
) {
    match value {
        Value::String(path) => {
            sources.push(PluginMcpConfigSource {
                plugin_id: plugin.id.clone(),
                plugin_name: plugin.name.clone(),
                label: manifest_mcp_label(plugin, path),
                kind: PluginMcpConfigSourceKind::File(plugin.root().join(path)),
            });
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                match item {
                    Value::String(path) => sources.push(PluginMcpConfigSource {
                        plugin_id: plugin.id.clone(),
                        plugin_name: plugin.name.clone(),
                        label: manifest_mcp_label(plugin, path),
                        kind: PluginMcpConfigSourceKind::File(plugin.root().join(path)),
                    }),
                    Value::Object(_) => sources.push(PluginMcpConfigSource {
                        plugin_id: plugin.id.clone(),
                        plugin_name: plugin.name.clone(),
                        label: format!("{} mcpServers[{index}]", plugin_manifest_label(plugin)),
                        kind: PluginMcpConfigSourceKind::Inline(wrap_mcp_servers(item.clone())),
                    }),
                    _ => {}
                }
            }
        }
        Value::Object(_) => sources.push(PluginMcpConfigSource {
            plugin_id: plugin.id.clone(),
            plugin_name: plugin.name.clone(),
            label: format!("{} mcpServers", plugin_manifest_label(plugin)),
            kind: PluginMcpConfigSourceKind::Inline(wrap_mcp_servers(value.clone())),
        }),
        _ => {}
    }
}

fn wrap_mcp_servers(value: Value) -> Value {
    serde_json::json!({ "mcpServers": value })
}

fn manifest_mcp_label(plugin: &LoadedPlugin, path: &str) -> String {
    format!("{} mcpServers {}", plugin_manifest_label(plugin), path)
}

fn plugin_manifest_label(plugin: &LoadedPlugin) -> String {
    plugin.manifest_path.as_ref().map_or_else(
        || format!("plugin {}", plugin.id),
        |path| path.display().to_string(),
    )
}

/// Read plugin-contributed hook files and parse them into
/// [`ContributedHookSource`] entries suitable for [`discover_hooks`].
///
/// Each enabled plugin's `hooks/hooks.json` is read and deserialized. Trust
/// is derived from the plugin's enabling scope: user-scoped plugins are always
/// trusted; project/local-scoped plugins inherit `trusted_project`.
///
/// Malformed or unreadable files produce [`PluginLoadWarning`] entries instead
/// of aborting — the remaining plugins keep loading.
pub fn plugin_contributed_hooks(
    registry: &PluginRegistry,
    trusted_project: bool,
) -> (Vec<ContributedHookSource>, Vec<PluginLoadWarning>) {
    let mut sources = Vec::new();
    let mut warnings = Vec::new();
    for plugin in registry.enabled() {
        for path in &plugin.contributions.hook_files {
            let contents = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(err) => {
                    warnings.push(PluginLoadWarning {
                        plugin_id: plugin.id.clone(),
                        path: Some(path.clone()),
                        message: format!("could not read hooks file: {err}"),
                    });
                    continue;
                }
            };
            let hooks: BTreeMap<String, Vec<HookMatcher>> = match serde_json::from_str(&contents) {
                Ok(h) => h,
                Err(err) => {
                    warnings.push(PluginLoadWarning {
                        plugin_id: plugin.id.clone(),
                        path: Some(path.clone()),
                        message: format!("malformed hooks.json: {err}"),
                    });
                    continue;
                }
            };
            let trusted = plugin_hooks_trusted(plugin, trusted_project);
            sources.push(ContributedHookSource {
                provenance: HookProvenance::Plugin {
                    plugin_id: plugin.id.clone(),
                },
                trusted,
                hooks,
            });
        }
    }
    (sources, warnings)
}

fn plugin_hooks_trusted(plugin: &LoadedPlugin, trusted_project: bool) -> bool {
    match plugin.enabled_by {
        Some(PluginScope::User) | None => true,
        Some(PluginScope::Project | PluginScope::Local) => trusted_project,
    }
}

fn settings_paths(home_dir: &Path, cwd: &Path) -> [(PluginScope, PathBuf); 3] {
    [
        (PluginScope::User, home_dir.join("settings.json")),
        (
            PluginScope::Project,
            cwd.join(".claude").join("settings.json"),
        ),
        (
            PluginScope::Local,
            cwd.join(".claude").join("settings.local.json"),
        ),
    ]
}

/// On-disk shape of a settings file for the `enabledPlugins` field.
#[derive(Default, Deserialize)]
#[serde(default)]
struct EnabledPluginsFile {
    #[serde(rename = "enabledPlugins")]
    enabled_plugins: BTreeMap<String, Value>,
}

async fn read_enabled_plugins(path: &Path) -> Result<Vec<(String, bool)>, ConfigError> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(Vec::new());
    }
    let contents = tokio::fs::read_to_string(path).await?;
    let file: EnabledPluginsFile = match serde_json::from_str(&contents) {
        Ok(file) => file,
        // Malformed user-edited settings shouldn't break startup discovery.
        Err(_) => return Ok(Vec::new()),
    };
    let mut entries = Vec::with_capacity(file.enabled_plugins.len());
    for (id, raw) in file.enabled_plugins {
        match raw {
            Value::Bool(enabled) => entries.push((id, enabled)),
            // Truthy / falsy heuristics matching the TypeScript loader.
            Value::Object(_) => entries.push((id, true)),
            Value::Null => entries.push((id, false)),
            _ => {}
        }
    }
    Ok(entries)
}

/// On-disk shape of `installed_plugins.json`.
#[derive(Default, Deserialize)]
#[serde(default)]
struct InstalledPluginsFile {
    version: Option<u64>,
    plugins: BTreeMap<String, Value>,
}

/// Typed V1 installation entry from `installed_plugins.json`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1InstallationEntry {
    install_path: Option<String>,
    version: Option<String>,
}

/// Typed V2 installation entry from `installed_plugins.json`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct V2InstallationEntry {
    install_path: Option<String>,
    scope: Option<String>,
    project_path: Option<String>,
    version: Option<String>,
}

async fn read_installed_plugins(
    home_dir: &Path,
) -> Result<BTreeMap<String, PluginInstallation>, ConfigError> {
    let path = home_dir.join("plugins").join("installed_plugins.json");
    if !tokio::fs::try_exists(&path).await? {
        return Ok(BTreeMap::new());
    }
    let contents = tokio::fs::read_to_string(path).await?;
    let file: InstalledPluginsFile = match serde_json::from_str(&contents) {
        Ok(file) => file,
        Err(_) => return Ok(BTreeMap::new()),
    };
    let mut installations = BTreeMap::new();
    let version = file.version.unwrap_or(1);
    for (id, entry) in file.plugins {
        let installation = match version {
            2 => parse_v2_installation_entry(&entry),
            _ => parse_v1_installation_entry(&entry),
        };
        if let Some(installation) = installation {
            installations.insert(id, installation);
        }
    }
    Ok(installations)
}

fn parse_v1_installation_entry(entry: &Value) -> Option<PluginInstallation> {
    let parsed: V1InstallationEntry = serde_json::from_value(entry.clone()).ok()?;
    let install_path = parsed.install_path.filter(|p| !p.is_empty())?;
    Some(PluginInstallation {
        scope: PluginScope::User,
        project_path: None,
        install_path: PathBuf::from(install_path),
        version: parsed.version,
    })
}

fn parse_v2_installation_entry(entry: &Value) -> Option<PluginInstallation> {
    let entries: Vec<V2InstallationEntry> = serde_json::from_value(entry.clone()).ok()?;
    let parsed = entries.into_iter().next()?;
    let install_path = parsed.install_path.filter(|p| !p.is_empty())?;
    let scope = match parsed.scope.as_deref() {
        Some("project") => PluginScope::Project,
        Some("local") => PluginScope::Local,
        // `managed` collapses to `user` for the discovery story; the
        // distinction only matters once policy enforcement lands.
        _ => PluginScope::User,
    };
    Some(PluginInstallation {
        scope,
        project_path: parsed.project_path.map(PathBuf::from),
        install_path: PathBuf::from(install_path),
        version: parsed.version,
    })
}

/// Internal carrier for a successfully discovered plugin plus any non-fatal
/// warnings raised while reading it. The caller stamps `plugin_id` onto each
/// warning before storing it in the registry.
struct LoadedPluginParts {
    manifest_path: Option<PathBuf>,
    manifest: PluginManifest,
    contributions: PluginContributions,
    warnings: Vec<RawPluginWarning>,
}

/// A warning that has not yet been attributed to a plugin id.
struct RawPluginWarning {
    path: Option<PathBuf>,
    message: String,
}

async fn load_plugin_from_installation(
    id: &str,
    installation: PluginInstallation,
) -> Result<LoadedPluginParts, String> {
    let root = installation.install_path.as_path();
    if !tokio::fs::try_exists(root).await.unwrap_or(false) {
        return Err(format!("install path missing: {}", root.display()));
    }

    let (manifest_path, manifest, validated_tools, mut warnings) = read_plugin_manifest(root)
        .await
        .map_err(|err| format!("failed to read plugin manifest for {id}: {err}"))?;

    let plugin_name = manifest
        .name
        .clone()
        .unwrap_or_else(|| parse_plugin_name(id));
    let tools: Vec<PluginToolDefinition> = validated_tools
        .into_iter()
        .map(|entry| PluginToolDefinition {
            name: entry.name.clone(),
            description: entry.description.clone(),
            input_schema: entry.input_schema.clone(),
            requires_permission: entry.requires_permission,
            plugin_id: id.to_string(),
            plugin_name: plugin_name.to_string(),
        })
        .collect();

    let mut contributions = read_plugin_contributions(root)
        .await
        .map_err(|err| format!("failed to enumerate plugin contributions: {err}"))?;
    contributions.tools = tools;

    if manifest_path.is_none() {
        warnings.push(RawPluginWarning {
            path: Some(root.to_path_buf()),
            message: "no plugin.json manifest found; plugin loaded with empty metadata".into(),
        });
    }

    Ok(LoadedPluginParts {
        manifest_path,
        manifest,
        contributions,
        warnings,
    })
}

async fn read_plugin_manifest(
    root: &Path,
) -> Result<
    (
        Option<PathBuf>,
        PluginManifest,
        Vec<ValidatedToolEntry>,
        Vec<RawPluginWarning>,
    ),
    ConfigError,
> {
    let candidates = [
        root.join(".claude-plugin").join("plugin.json"),
        root.join("plugin.json"),
    ];
    for path in candidates {
        if !tokio::fs::try_exists(&path).await? {
            continue;
        }
        let contents = tokio::fs::read_to_string(&path).await?;
        match serde_json::from_str::<StoredManifest>(&contents) {
            Ok(parsed) => {
                let (manifest, tools, warnings) = parsed.into_manifest(&path);
                return Ok((Some(path), manifest, tools, warnings));
            }
            Err(err) => {
                let warning = RawPluginWarning {
                    path: Some(path.clone()),
                    message: format!("plugin.json is not valid JSON: {err}"),
                };
                return Ok((
                    Some(path),
                    PluginManifest::default(),
                    Vec::new(),
                    vec![warning],
                ));
            }
        }
    }
    Ok((None, PluginManifest::default(), Vec::new(), Vec::new()))
}

async fn read_plugin_contributions(root: &Path) -> std::io::Result<PluginContributions> {
    let command_files = collect_markdown_recursive(&root.join("commands")).await?;
    let agent_files = collect_markdown_recursive(&root.join("agents")).await?;
    let skill_dirs = collect_skill_dirs(&root.join("skills")).await?;
    let output_style_files = collect_markdown_recursive(&root.join("output-styles"))
        .await?
        .into_iter()
        .collect();
    let hook_files = collect_hook_files(&root.join("hooks")).await?;

    let mcp_path = root.join(".mcp.json");
    let mcp_server_files = if tokio::fs::try_exists(&mcp_path).await? {
        vec![mcp_path]
    } else {
        Vec::new()
    };
    Ok(PluginContributions {
        command_files,
        agent_files,
        skill_dirs,
        hook_files,
        output_style_files,
        mcp_server_files,
        tools: Vec::new(),
    })
}

async fn collect_markdown_recursive(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    if !tokio::fs::try_exists(dir).await? {
        return Ok(results);
    }
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&current).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && path.extension().and_then(|value| value.to_str()) == Some("md")
            {
                results.push(path);
            }
        }
    }
    results.sort();
    Ok(results)
}

async fn collect_skill_dirs(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    if !tokio::fs::try_exists(dir).await? {
        return Ok(results);
    }
    // A "skills" root itself can be a single SKILL.md skill (matching the
    // TS plugin loader). Otherwise each immediate subdirectory containing a
    // SKILL.md becomes a skill.
    if tokio::fs::try_exists(&dir.join("SKILL.md")).await? {
        results.push(dir.to_path_buf());
        return Ok(results);
    }
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !entry.file_type().await?.is_dir() {
            continue;
        }
        if tokio::fs::try_exists(&path.join("SKILL.md")).await? {
            results.push(path);
        }
    }
    results.sort();
    Ok(results)
}

async fn collect_hook_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    if !tokio::fs::try_exists(dir).await? {
        return Ok(results);
    }
    let canonical = dir.join("hooks.json");
    if tokio::fs::try_exists(&canonical).await? {
        results.push(canonical);
    }
    Ok(results)
}

fn parse_plugin_name(id: &str) -> String {
    match id.split_once('@') {
        Some((name, _)) => name.to_string(),
        None => id.to_string(),
    }
}

fn parse_marketplace_name(id: &str) -> Option<String> {
    id.split_once('@').map(|(_, mkt)| mkt.to_string())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct StoredManifest {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    author: Option<StoredManifestAuthor>,
    homepage: Option<String>,
    repository: Option<String>,
    license: Option<String>,
    #[serde(rename = "mcpServers")]
    mcp_servers: Option<Value>,
    tools: Option<Vec<Value>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct StoredManifestAuthor {
    name: Option<String>,
}

impl StoredManifest {
    fn into_manifest(
        self,
        manifest_path: &Path,
    ) -> (
        PluginManifest,
        Vec<ValidatedToolEntry>,
        Vec<RawPluginWarning>,
    ) {
        let (tools, warnings) = parse_raw_tool_definitions(self.tools.as_deref(), manifest_path);
        let manifest = PluginManifest {
            name: self.name,
            version: self.version,
            description: self.description,
            author_name: self.author.and_then(|author| author.name),
            homepage: self.homepage,
            repository: self.repository,
            license: self.license,
            mcp_servers: self.mcp_servers,
        };
        (manifest, tools, warnings)
    }
}

/// Typed deserialization target for a single entry in the `tools` array of
/// `plugin.json`. Fields are all optional to allow graceful validation with
/// warnings for missing/empty names.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPluginToolEntry {
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    input_schema: Option<Value>,
    requires_permission: Option<bool>,
}

fn parse_raw_tool_definitions(
    raw: Option<&[Value]>,
    manifest_path: &Path,
) -> (Vec<ValidatedToolEntry>, Vec<RawPluginWarning>) {
    let Some(items) = raw else {
        return (Vec::new(), Vec::new());
    };
    let mut tools = Vec::new();
    let mut warnings = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if !item.is_object() {
            warnings.push(RawPluginWarning {
                path: Some(manifest_path.to_path_buf()),
                message: format!(
                    "tools[{index}]: expected an object, got {}",
                    item_type_name(item)
                ),
            });
            continue;
        }
        let parsed: RawPluginToolEntry = match serde_json::from_value(item.clone()) {
            Ok(entry) => entry,
            Err(_) => {
                warnings.push(RawPluginWarning {
                    path: Some(manifest_path.to_path_buf()),
                    message: format!(
                        "tools[{index}]: expected an object, got {}",
                        item_type_name(item)
                    ),
                });
                continue;
            }
        };
        let name = match parsed.name.filter(|n| !n.is_empty()) {
            Some(n) => n,
            None => {
                warnings.push(RawPluginWarning {
                    path: Some(manifest_path.to_path_buf()),
                    message: format!("tools[{index}]: missing or empty \"name\" field"),
                });
                continue;
            }
        };
        tools.push(ValidatedToolEntry {
            name,
            description: parsed.description.unwrap_or_default(),
            input_schema: parsed
                .input_schema
                .unwrap_or_else(|| serde_json::json!({"type": "object"})),
            requires_permission: parsed.requires_permission.unwrap_or(true),
        });
    }
    (tools, warnings)
}

fn item_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// A validated tool entry parsed from `plugin.json` `tools` array, before
/// being stamped with plugin identity in `plugin_tool_definitions()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ValidatedToolEntry {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) input_schema: Value,
    pub(crate) requires_permission: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn write_json(path: &Path, value: &str) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        tokio::fs::write(path, value).await.unwrap();
    }

    async fn write_text(path: &Path, value: &str) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        tokio::fs::write(path, value).await.unwrap();
    }

    fn make_v2_index(install_path: &Path, version: &str) -> String {
        format!(
            r#"{{
              "version": 2,
              "plugins": {{
                "demo@market": [
                  {{
                    "scope": "user",
                    "installPath": "{install}",
                    "version": "{version}"
                  }}
                ]
              }}
            }}"#,
            install = install_path.display(),
            version = version,
        )
    }

    #[tokio::test]
    async fn empty_setup_returns_empty_registry() {
        let temp = tempdir().unwrap();
        let registry =
            load_plugin_registry(&temp.path().join("home"), &temp.path().join("project"))
                .await
                .unwrap();
        assert!(registry.plugins.is_empty());
        assert!(registry.errors.is_empty());
    }

    #[tokio::test]
    async fn discovers_enabled_plugin_with_contributions() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");

        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo","version":"1.0.0","description":"hi","author":{"name":"me"}}"#,
        )
        .await;
        write_text(
            &plugin_root.join("commands").join("greet.md"),
            "---\nname: greet\n---\nsay hello",
        )
        .await;
        write_text(
            &plugin_root.join("agents").join("worker.md"),
            "---\nname: worker\ndescription: do work\n---\nbody",
        )
        .await;
        write_text(
            &plugin_root.join("skills").join("intro").join("SKILL.md"),
            "---\nname: intro\n---\nbody",
        )
        .await;
        write_text(
            &plugin_root.join("hooks").join("hooks.json"),
            r#"{"hooks":{}}"#,
        )
        .await;
        write_text(
            &plugin_root.join("output-styles").join("Concise.md"),
            "# Concise\nshort",
        )
        .await;
        write_text(&plugin_root.join(".mcp.json"), r"{}").await;

        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();

        assert!(registry.errors.is_empty(), "errors: {:?}", registry.errors);
        assert_eq!(registry.plugins.len(), 1);
        let plugin = &registry.plugins[0];
        assert_eq!(plugin.id, "demo@market");
        assert_eq!(plugin.name, "demo");
        assert_eq!(plugin.marketplace.as_deref(), Some("market"));
        assert!(plugin.enabled);
        assert_eq!(plugin.enabled_by, Some(PluginScope::User));
        assert_eq!(plugin.manifest.version.as_deref(), Some("1.0.0"));
        assert_eq!(plugin.contributions.command_files.len(), 1);
        assert_eq!(plugin.contributions.agent_files.len(), 1);
        assert_eq!(plugin.contributions.skill_dirs.len(), 1);
        assert_eq!(plugin.contributions.hook_files.len(), 1);
        assert_eq!(plugin.contributions.output_style_files.len(), 1);
        assert_eq!(plugin.contributions.mcp_server_files.len(), 1);
    }

    #[tokio::test]
    async fn disabled_plugin_is_listed_but_contributes_nothing() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .await;
        write_text(
            &plugin_root.join("agents").join("worker.md"),
            "---\nname: worker\ndescription: do work\n---\nbody",
        )
        .await;
        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":false}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        assert_eq!(registry.plugins.len(), 1);
        assert!(!registry.plugins[0].enabled);

        let agents = plugin_agent_definitions(&registry);
        assert!(agents.is_empty(), "disabled plugin should not contribute");
        let skills = plugin_skill_roots(&registry);
        assert!(skills.is_empty());
        let mcp = plugin_mcp_config_sources(&registry);
        assert!(mcp.is_empty());
    }

    #[tokio::test]
    async fn project_settings_override_user_enable_state() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo"}"#,
        )
        .await;
        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;
        write_json(
            &cwd.join(".claude").join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":false}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        assert_eq!(registry.plugins.len(), 1);
        assert!(!registry.plugins[0].enabled);
        assert_eq!(registry.plugins[0].enabled_by, None);
    }

    #[tokio::test]
    async fn missing_installation_surfaces_load_error() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"ghost@market":true}}"#,
        )
        .await;
        // installed_plugins.json missing → not registered → load error.
        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        assert!(registry.plugins.is_empty());
        assert_eq!(registry.errors.len(), 1);
        assert_eq!(registry.errors[0].plugin_id, "ghost@market");
    }

    #[tokio::test]
    async fn plugin_agent_definitions_namespace_agent_type() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo"}"#,
        )
        .await;
        write_text(
            &plugin_root.join("agents").join("worker.md"),
            "---\nname: worker\ndescription: do work\n---\nbody",
        )
        .await;
        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        let agents = plugin_agent_definitions(&registry);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_type, "demo:worker");
        assert!(matches!(agents[0].source, AgentSource::Plugin { .. }));
    }

    #[tokio::test]
    async fn plugin_skill_roots_namespace_skill_name() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo"}"#,
        )
        .await;
        write_text(
            &plugin_root.join("skills").join("hello").join("SKILL.md"),
            "---\nname: hello\n---\nbody",
        )
        .await;
        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        let roots = plugin_skill_roots(&registry);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].namespaced_name, "demo:hello");
        assert!(roots[0].skill_dir.ends_with("skills/hello"));
    }

    #[tokio::test]
    async fn plugin_mcp_sources_include_files_and_manifest_definitions() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{
                "name":"demo",
                "mcpServers": {
                    "manifest_docs": {"type":"http","url":"https://manifest.example/mcp"}
                }
            }"#,
        )
        .await;
        write_text(
            &plugin_root.join(".mcp.json"),
            r#"{"mcpServers":{"file_docs":{"type":"http","url":"https://file.example/mcp"}}}"#,
        )
        .await;
        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        let sources = plugin_mcp_config_sources(&registry);

        assert_eq!(sources.len(), 2);
        assert!(matches!(
            sources[0].kind,
            PluginMcpConfigSourceKind::File(_)
        ));
        assert!(matches!(
            sources[1].kind,
            PluginMcpConfigSourceKind::Inline(_)
        ));
        assert_eq!(sources[0].plugin_id, "demo@market");
        assert_eq!(sources[1].plugin_name, "demo");
    }

    #[tokio::test]
    async fn malformed_settings_does_not_abort_discovery() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        write_text(&home.join("settings.json"), "{ not json").await;
        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        assert!(registry.plugins.is_empty());
    }

    #[tokio::test]
    async fn malformed_manifest_loads_plugin_with_warning() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        write_text(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            "{ this is not valid json",
        )
        .await;
        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        // Plugin still loads (never panics), but the bad manifest is flagged.
        assert_eq!(registry.plugins.len(), 1);
        assert!(registry.plugins[0].enabled);
        assert!(registry.errors.is_empty());
        assert_eq!(registry.warnings.len(), 1);
        assert_eq!(registry.warnings[0].plugin_id, "demo@market");
        assert!(
            registry.warnings[0].message.contains("not valid JSON"),
            "warning was: {}",
            registry.warnings[0].message
        );
    }

    #[tokio::test]
    async fn missing_manifest_loads_plugin_with_warning() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        // Install path exists (write an agent file) but no plugin.json anywhere.
        write_text(
            &plugin_root.join("agents").join("worker.md"),
            "---\nname: worker\ndescription: do work\n---\nbody",
        )
        .await;
        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        assert_eq!(registry.plugins.len(), 1);
        assert!(registry.plugins[0].manifest_path.is_none());
        assert_eq!(registry.warnings.len(), 1);
        assert!(
            registry.warnings[0].message.contains("no plugin.json"),
            "warning was: {}",
            registry.warnings[0].message
        );
        // Contributions are still enumerated despite the missing manifest.
        assert_eq!(registry.plugins[0].contributions.agent_files.len(), 1);
    }

    #[tokio::test]
    async fn supports_v1_installed_plugins_layout() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("legacy");
        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"legacy"}"#,
        )
        .await;
        let index = format!(
            r#"{{"version":1,"plugins":{{"legacy@market":{{"version":"0.0.1","installedAt":"now","installPath":"{}"}}}}}}"#,
            plugin_root.display(),
        );
        write_json(&home.join("plugins").join("installed_plugins.json"), &index).await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"legacy@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        assert!(registry.errors.is_empty());
        assert_eq!(registry.plugins.len(), 1);
        assert_eq!(
            registry.plugins[0].installation.install_path, plugin_root,
            "v1 layout install path resolved",
        );
    }

    #[tokio::test]
    async fn plugin_contributed_hooks_loads_valid_hooks_json() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo"}"#,
        )
        .await;
        write_json(
            &plugin_root.join("hooks").join("hooks.json"),
            r#"{"PreToolUse":[{"hooks":[{"type":"command","command":"echo pre"}]}]}"#,
        )
        .await;
        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        let (sources, warnings) = plugin_contributed_hooks(&registry, true);

        assert!(warnings.is_empty(), "warnings: {warnings:?}");
        assert_eq!(sources.len(), 1);
        assert_eq!(
            sources[0].provenance,
            HookProvenance::Plugin {
                plugin_id: "demo@market".to_string()
            }
        );
        assert!(sources[0].trusted);
        assert!(sources[0].hooks.contains_key("PreToolUse"));
    }

    #[tokio::test]
    async fn plugin_contributed_hooks_malformed_json_warns() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo"}"#,
        )
        .await;
        write_text(
            &plugin_root.join("hooks").join("hooks.json"),
            "NOT JSON {{{",
        )
        .await;
        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        let (sources, warnings) = plugin_contributed_hooks(&registry, true);

        assert!(
            sources.is_empty(),
            "malformed file should not produce a source"
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].plugin_id, "demo@market");
        assert!(
            warnings[0].message.contains("malformed hooks.json"),
            "warning should mention malformed: {}",
            warnings[0].message
        );
    }

    #[tokio::test]
    async fn plugin_contributed_hooks_disabled_plugin_contributes_nothing() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo"}"#,
        )
        .await;
        write_json(
            &plugin_root.join("hooks").join("hooks.json"),
            r#"{"PreToolUse":[{"hooks":[{"type":"command","command":"echo hi"}]}]}"#,
        )
        .await;
        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":false}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        let (sources, warnings) = plugin_contributed_hooks(&registry, true);

        assert!(sources.is_empty());
        assert!(warnings.is_empty());
    }

    #[tokio::test]
    async fn plugin_contributed_hooks_project_scope_trust_gating() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");
        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo"}"#,
        )
        .await;
        write_json(
            &plugin_root.join("hooks").join("hooks.json"),
            r#"{"PostToolUse":[{"hooks":[{"type":"command","command":"echo post"}]}]}"#,
        )
        .await;
        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        // Enable at project scope (not user scope).
        write_json(
            &cwd.join(".claude").join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        assert_eq!(
            registry.plugins[0].enabled_by,
            Some(PluginScope::Project),
            "plugin should be enabled at project scope"
        );

        let (sources_trusted, _) = plugin_contributed_hooks(&registry, true);
        assert_eq!(sources_trusted.len(), 1);
        assert!(
            sources_trusted[0].trusted,
            "trusted project -> trusted hooks"
        );

        let (sources_untrusted, _) = plugin_contributed_hooks(&registry, false);
        assert_eq!(sources_untrusted.len(), 1);
        assert!(
            !sources_untrusted[0].trusted,
            "untrusted project -> untrusted plugin hooks"
        );
    }

    #[tokio::test]
    async fn enabled_plugin_tools_parsed_into_contributions() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");

        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{
                "name": "demo",
                "tools": [
                    {
                        "name": "my_tool",
                        "description": "Does something useful",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "arg": { "type": "string" } }
                        }
                    }
                ]
            }"#,
        )
        .await;

        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        assert!(registry.errors.is_empty(), "errors: {:?}", registry.errors);
        assert!(
            registry.warnings.is_empty(),
            "warnings: {:?}",
            registry.warnings
        );

        let tools = plugin_tool_definitions(&registry);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "my_tool");
        assert_eq!(tools[0].description, "Does something useful");
        assert_eq!(tools[0].plugin_id, "demo@market");
        assert_eq!(tools[0].plugin_name, "demo");
        assert!(tools[0].requires_permission);
        assert!(tools[0].input_schema.get("properties").is_some());
    }

    #[tokio::test]
    async fn disabled_plugin_tools_not_contributed() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");

        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{
                "name": "demo",
                "tools": [{"name": "hidden_tool", "description": "secret"}]
            }"#,
        )
        .await;

        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":false}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        let tools = plugin_tool_definitions(&registry);
        assert!(
            tools.is_empty(),
            "disabled plugin should not contribute tools"
        );
    }

    #[tokio::test]
    async fn tool_missing_name_produces_warning() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");

        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{
                "name": "demo",
                "tools": [
                    {"description": "no name field"},
                    {"name": "", "description": "empty name"}
                ]
            }"#,
        )
        .await;

        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        assert!(registry.errors.is_empty());
        assert_eq!(
            registry.warnings.len(),
            2,
            "both missing-name and empty-name should warn: {:?}",
            registry.warnings
        );
        for w in &registry.warnings {
            assert!(
                w.message.contains("name"),
                "warning should mention name: {}",
                w.message
            );
        }
        let tools = plugin_tool_definitions(&registry);
        assert!(
            tools.is_empty(),
            "invalid tools should not appear in contributions"
        );
    }

    #[tokio::test]
    async fn tool_non_object_entry_produces_warning() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");

        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{
                "name": "demo",
                "tools": ["not_an_object", 42]
            }"#,
        )
        .await;

        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        assert_eq!(registry.warnings.len(), 2);
        assert!(registry.warnings[0].message.contains("expected an object"));
        let tools = plugin_tool_definitions(&registry);
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn tool_requires_permission_defaults_to_true() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");

        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{
                "name": "demo",
                "tools": [
                    {"name": "default_perm", "description": "no requiresPermission field"},
                    {"name": "explicit_false", "description": "opt out", "requiresPermission": false},
                    {"name": "explicit_true", "description": "opt in", "requiresPermission": true}
                ]
            }"#,
        )
        .await;

        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        let tools = plugin_tool_definitions(&registry);
        assert_eq!(tools.len(), 3);

        let default_perm = tools.iter().find(|t| t.name == "default_perm").unwrap();
        assert!(default_perm.requires_permission, "should default to true");

        let explicit_false = tools.iter().find(|t| t.name == "explicit_false").unwrap();
        assert!(!explicit_false.requires_permission);

        let explicit_true = tools.iter().find(|t| t.name == "explicit_true").unwrap();
        assert!(explicit_true.requires_permission);
    }

    #[tokio::test]
    async fn tool_missing_input_schema_defaults_to_object() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");

        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{
                "name": "demo",
                "tools": [{"name": "bare", "description": "no schema"}]
            }"#,
        )
        .await;

        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        let tools = plugin_tool_definitions(&registry);
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].input_schema,
            serde_json::json!({"type": "object"}),
            "missing inputSchema should default to {{\"type\":\"object\"}}"
        );
    }

    #[tokio::test]
    async fn no_tools_field_means_empty_tools() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo").join("1.0.0");

        write_json(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name": "demo"}"#,
        )
        .await;

        write_json(
            &home.join("plugins").join("installed_plugins.json"),
            &make_v2_index(&plugin_root, "1.0.0"),
        )
        .await;
        write_json(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let registry = load_plugin_registry(&home, &cwd).await.unwrap();
        let tools = plugin_tool_definitions(&registry);
        assert!(tools.is_empty());
        assert!(registry.warnings.is_empty());
    }

    #[test]
    fn plugin_load_warning_is_serializable() {
        let warning = PluginLoadWarning {
            plugin_id: "demo@market".to_string(),
            path: Some(PathBuf::from("/plugins/demo/plugin.json")),
            message: "plugin.json is not valid JSON".to_string(),
        };
        let json = serde_json::to_value(&warning).expect("serializes");
        assert_eq!(json["plugin_id"], "demo@market");
        assert_eq!(json["path"], "/plugins/demo/plugin.json");
        assert_eq!(json["message"], "plugin.json is not valid JSON");
    }
}
