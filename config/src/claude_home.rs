use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::ConfigError;
use crate::hooks::HookMatcher;
use crate::output_styles::load_output_style_definitions;

const MAX_SANITIZED_LENGTH: usize = 200;
const DEFAULT_OUTPUT_STYLE_NAME: &str = "default";

#[derive(Clone, Debug, Default)]
pub struct ClaudeSettings {
    pub env: BTreeMap<String, String>,
    pub model: Option<String>,
    pub theme: ThemeSetting,
    pub editor_mode: EditorModeSetting,
    pub always_thinking_enabled: Option<bool>,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    /// Rules that force an interactive prompt even when an allow rule or the
    /// blanket tools-permission would otherwise auto-approve (`permissions.ask`).
    pub ask_tools: Vec<String>,
    pub additional_directories: Vec<String>,
    pub hooks: BTreeMap<String, Vec<HookMatcher>>,
    pub hook_sources: BTreeMap<String, Vec<HookSource>>,
    /// Optional spend cap in USD (`maxBudgetUsd`). `None` disables enforcement.
    pub max_budget_usd: Option<f64>,
    /// When the running cost cannot be priced, whether to block the turn (`true`)
    /// rather than warn and proceed (`false`/`None`).
    pub max_budget_strict_unknown_pricing: Option<bool>,
    pub statusline_command: Option<String>,
    pub statusline_refresh_interval_secs: Option<u64>,
}

impl ClaudeSettings {
    pub fn hook_source_at(&self, event: &str, index: usize) -> HookSource {
        self.hook_sources
            .get(event)
            .and_then(|sources| sources.get(index))
            .copied()
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HookSource {
    #[default]
    Settings,
    /// Shared project settings (`<cwd>/.claude/settings.json`). Sourced from the
    /// working directory, so — like local settings — it requires project trust.
    ProjectSettings,
    LocalSettings,
}

impl HookSource {
    pub fn label(self) -> &'static str {
        match self {
            HookSource::Settings => "settings.json",
            HookSource::ProjectSettings => ".claude/settings.json",
            HookSource::LocalSettings => "settings.local.json",
        }
    }

    /// Whether this source originates from the working directory and therefore
    /// requires explicit project trust before its hooks may execute. Both the
    /// shared project settings and the local settings qualify.
    pub fn is_local(self) -> bool {
        matches!(
            self,
            HookSource::ProjectSettings | HookSource::LocalSettings
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SandboxLocalSettings {
    pub enabled: bool,
    pub auto_allow_bash_if_sandboxed: bool,
    pub allow_unsandboxed_commands: bool,
    pub excluded_commands: Vec<String>,
    pub filesystem: SandboxFilesystemLocalSettings,
    pub network: SandboxNetworkLocalSettings,
}

impl Default for SandboxLocalSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_allow_bash_if_sandboxed: true,
            allow_unsandboxed_commands: true,
            excluded_commands: Vec::new(),
            filesystem: SandboxFilesystemLocalSettings::default(),
            network: SandboxNetworkLocalSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SandboxFilesystemLocalSettings {
    pub allow_write: Vec<String>,
    pub deny_write: Vec<String>,
    pub deny_read: Vec<String>,
    pub allow_read: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SandboxNetworkLocalSettings {
    pub allowed_domains: Vec<String>,
    pub allow_unix_sockets: Vec<String>,
    pub allow_all_unix_sockets: Option<bool>,
    pub allow_local_binding: Option<bool>,
    pub http_proxy_port: Option<u64>,
    pub socks_proxy_port: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SandboxSettingsUpdate {
    pub enabled: Option<bool>,
    pub auto_allow_bash_if_sandboxed: Option<bool>,
    pub allow_unsandboxed_commands: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionRuleSettingKind {
    Allow,
    Deny,
}

impl PermissionRuleSettingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PermissionRuleSettingsUpdate {
    pub path: PathBuf,
    pub rule: String,
    pub changed: bool,
}

/// Persisted trust decision for an MCP server, recorded in the same settings
/// layers as other permissions. Mirrors the TypeScript CLI's
/// `enabledMcpjsonServers` / `disabledMcpjsonServers` keys so the on-disk
/// representation stays byte-compatible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpServerTrustSetting {
    /// Approved: recorded under `enabledMcpjsonServers`.
    Trusted,
    /// Rejected: recorded under `disabledMcpjsonServers`.
    Denied,
    /// No persisted decision: removed from both lists.
    Cleared,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputStyleOption {
    pub value: String,
    pub label: String,
    pub description: String,
    pub current: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThemeSetting {
    #[default]
    Auto,
    Dark,
    Light,
    DarkDaltonized,
    LightDaltonized,
    DarkAnsi,
    LightAnsi,
}

impl ThemeSetting {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "dark-daltonized" => Some(Self::DarkDaltonized),
            "light-daltonized" => Some(Self::LightDaltonized),
            "dark-ansi" => Some(Self::DarkAnsi),
            "light-ansi" => Some(Self::LightAnsi),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::DarkDaltonized => "dark-daltonized",
            Self::LightDaltonized => "light-daltonized",
            Self::DarkAnsi => "dark-ansi",
            Self::LightAnsi => "light-ansi",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EditorModeSetting {
    #[default]
    Normal,
    Vim,
}

impl EditorModeSetting {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "normal" | "emacs" => Some(Self::Normal),
            "vim" => Some(Self::Vim),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Vim => "vim",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct StoredClaudeSettings {
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    theme: Option<String>,
    #[serde(rename = "editorMode")]
    editor_mode: Option<String>,
    #[serde(rename = "alwaysThinkingEnabled")]
    always_thinking_enabled: Option<bool>,
    #[serde(default)]
    permissions: StoredPermissionSettings,
    #[serde(default)]
    hooks: BTreeMap<String, Vec<HookMatcher>>,
    #[serde(default, rename = "maxBudgetUsd")]
    max_budget_usd: Option<f64>,
    #[serde(default, rename = "maxBudgetUsdStrictUnknownPricing")]
    max_budget_strict_unknown_pricing: Option<bool>,
    #[serde(default)]
    statusline: StoredStatuslineSettings,
}

#[derive(Debug, Default, Deserialize)]
struct StoredStatuslineSettings {
    command: Option<String>,
    #[serde(default, rename = "refreshInterval")]
    refresh_interval: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct StoredPermissionSettings {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    #[serde(default)]
    ask: Vec<String>,
    #[serde(default, rename = "additionalDirectories")]
    additional_directories: Vec<String>,
}

/// Directory name of the opt-in, Orb Code-only home.
const ORBCODE_HOME_DIR: &str = ".orbcode";
/// Directory name of the home shared with the TypeScript CLI.
const CLAUDE_HOME_DIR: &str = ".claude";

/// Files whose presence means a home directory holds real user state.
const HOME_STATE_FILES: &[&str] = &[
    "settings.json",
    "settings.local.json",
    ".credentials.json",
    "history.jsonl",
];

/// Whether this home has actually been used, as opposed to merely initialised.
///
/// Deliberately does NOT treat `projects/` or `sessions/` as markers by existence,
/// nor even by being non-empty: bootstrap creates `projects/<slug>/` on every run,
/// including in a home that has never held a conversation. Only real transcripts
/// count.
///
/// Kept next to the resolution logic so the "is it empty?" question is answered
/// the same way everywhere.
fn home_has_state(dir: &Path) -> bool {
    if HOME_STATE_FILES
        .iter()
        .any(|entry| dir.join(entry).is_file())
    {
        return true;
    }
    has_transcript(&dir.join("projects"))
}

/// True when any `projects/<slug>/` holds at least one `.jsonl` transcript.
fn has_transcript(projects_dir: &Path) -> bool {
    let Ok(project_dirs) = std::fs::read_dir(projects_dir) else {
        return false;
    };
    for project in project_dirs.flatten() {
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        if entries
            .flatten()
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        {
            return true;
        }
    }
    false
}

/// An opted-into `~/.orbcode` that is still empty while `~/.claude` next to it
/// holds real state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowedHome {
    /// The home currently in use — the empty `~/.orbcode`.
    pub active: PathBuf,
    /// The populated `~/.claude` being shadowed.
    pub shadowed: PathBuf,
}

/// Detect the one genuinely confusing outcome of the `~/.orbcode` opt-in: the
/// user created the directory, so it wins resolution, but nothing was copied into
/// it — so sessions, credentials and settings all appear to have vanished while
/// they actually sit in `~/.claude`.
///
/// Returns `None` for every other shape, including an explicit `ORBCODE_HOME`
/// pointing somewhere unrelated (the directory name will not match) and a
/// populated `~/.orbcode` (the opt-in is working as intended).
pub fn shadowed_home(active_home: &Path) -> Option<ShadowedHome> {
    if active_home.file_name()? != OsStr::new(ORBCODE_HOME_DIR) {
        return None;
    }
    if home_has_state(active_home) {
        return None;
    }
    let shadowed = active_home.parent()?.join(CLAUDE_HOME_DIR);
    if !home_has_state(&shadowed) {
        return None;
    }
    Some(ShadowedHome {
        active: active_home.to_path_buf(),
        shadowed,
    })
}

/// Pick the home directory when neither env var names one.
///
/// Prefers an existing `~/.orbcode`, otherwise `~/.claude`.
///
/// `~/.orbcode` is strictly opt-in: this never creates it. An installation that
/// has not deliberately made that directory keeps using `~/.claude`, so settings,
/// credentials, prompt history and transcripts stay shared with the TypeScript
/// CLI and nothing has to be migrated. Creating `~/.orbcode` is how a user asks
/// for a separate, orbcode-only state directory.
///
/// The probe is `is_dir`, not `exists`: a stray *file* at `~/.orbcode` is not a
/// usable home, and treating it as one would fail later with a confusing error
/// instead of falling through here.
fn default_home_dir(user_home: &Path) -> PathBuf {
    let orbcode_home = user_home.join(".orbcode");
    if orbcode_home.is_dir() {
        return orbcode_home;
    }
    user_home.join(".claude")
}

pub fn resolve_home_dir() -> Result<PathBuf, ConfigError> {
    // An env var set to the empty string must fall back to the default home,
    // not resolve to `PathBuf::from("")` (the current directory), which would
    // silently redirect config/auth/transcripts to a bogus relative path.
    if let Ok(explicit) = env::var("ORBCODE_HOME")
        && !explicit.trim().is_empty()
    {
        return Ok(PathBuf::from(explicit));
    }

    if let Ok(explicit) = env::var("CLAUDE_CONFIG_DIR")
        && !explicit.trim().is_empty()
    {
        return Ok(PathBuf::from(explicit));
    }

    let base_dirs = BaseDirs::new()
        .ok_or_else(|| ConfigError::Config("failed to determine home directory".into()))?;
    Ok(default_home_dir(base_dirs.home_dir()))
}

pub async fn load_settings(home_dir: &Path) -> Result<ClaudeSettings, ConfigError> {
    let path = home_dir.join("settings.json");
    if !tokio::fs::try_exists(&path).await? {
        return Ok(ClaudeSettings::default());
    }

    let contents = tokio::fs::read_to_string(path).await?;
    let parsed = serde_json::from_str::<StoredClaudeSettings>(&contents)?;
    let hook_sources = parsed
        .hooks
        .iter()
        .map(|(event, matchers)| (event.clone(), vec![HookSource::Settings; matchers.len()]))
        .collect();
    Ok(ClaudeSettings {
        env: parsed.env,
        model: parsed.model,
        theme: parsed
            .theme
            .as_deref()
            .and_then(ThemeSetting::parse)
            .unwrap_or_default(),
        editor_mode: parsed
            .editor_mode
            .as_deref()
            .and_then(EditorModeSetting::parse)
            .unwrap_or_default(),
        always_thinking_enabled: parsed.always_thinking_enabled,
        allowed_tools: parsed.permissions.allow,
        disallowed_tools: parsed.permissions.deny,
        ask_tools: parsed.permissions.ask,
        additional_directories: parsed.permissions.additional_directories,
        hooks: parsed.hooks,
        hook_sources,
        max_budget_usd: parsed.max_budget_usd,
        max_budget_strict_unknown_pricing: parsed.max_budget_strict_unknown_pricing,
        statusline_command: parsed.statusline.command,
        statusline_refresh_interval_secs: parsed.statusline.refresh_interval,
    })
}

/// Merge the shared project settings layer (`<cwd>/.claude/settings.json`) on
/// top of the user layer. Must run before [`merge_project_local_settings`] so
/// the documented `User → Project → Local` precedence holds (later layers win).
pub async fn merge_project_settings(
    settings: &mut ClaudeSettings,
    cwd: &Path,
) -> Result<(), ConfigError> {
    let path = project_settings_path(cwd);
    merge_settings_file(settings, &path, HookSource::ProjectSettings).await
}

pub async fn merge_project_local_settings(
    settings: &mut ClaudeSettings,
    cwd: &Path,
) -> Result<(), ConfigError> {
    let path = local_settings_path(cwd);
    merge_settings_file(settings, &path, HookSource::LocalSettings).await
}

/// Merge a single settings file at `path` into `settings`, attributing any
/// hooks it contributes to `hook_source`. Scalar keys override; collections
/// (env, permission rules, additional directories, hooks) extend — so a later
/// layer both overrides scalars and appends rules.
async fn merge_settings_file(
    settings: &mut ClaudeSettings,
    path: &Path,
    hook_source: HookSource,
) -> Result<(), ConfigError> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(());
    }

    let contents = tokio::fs::read_to_string(path).await?;
    let parsed = serde_json::from_str::<StoredClaudeSettings>(&contents)?;
    settings.env.extend(parsed.env);
    if let Some(model) = parsed.model {
        settings.model = Some(model);
    }
    if let Some(theme) = parsed.theme.as_deref().and_then(ThemeSetting::parse) {
        settings.theme = theme;
    }
    if let Some(editor_mode) = parsed
        .editor_mode
        .as_deref()
        .and_then(EditorModeSetting::parse)
    {
        settings.editor_mode = editor_mode;
    }
    if parsed.always_thinking_enabled.is_some() {
        settings.always_thinking_enabled = parsed.always_thinking_enabled;
    }
    if parsed.max_budget_usd.is_some() {
        settings.max_budget_usd = parsed.max_budget_usd;
    }
    if parsed.max_budget_strict_unknown_pricing.is_some() {
        settings.max_budget_strict_unknown_pricing = parsed.max_budget_strict_unknown_pricing;
    }
    if let Some(cmd) = parsed.statusline.command {
        settings.statusline_command = Some(cmd);
    }
    if let Some(interval) = parsed.statusline.refresh_interval {
        settings.statusline_refresh_interval_secs = Some(interval);
    }
    settings.allowed_tools.extend(parsed.permissions.allow);
    settings.disallowed_tools.extend(parsed.permissions.deny);
    settings.ask_tools.extend(parsed.permissions.ask);
    settings
        .additional_directories
        .extend(parsed.permissions.additional_directories);
    for (event, mut matchers) in parsed.hooks {
        let added = matchers.len();
        settings
            .hooks
            .entry(event.clone())
            .or_default()
            .append(&mut matchers);
        settings
            .hook_sources
            .entry(event)
            .or_default()
            .extend(std::iter::repeat_n(hook_source, added));
    }
    Ok(())
}

pub async fn add_permission_rule_setting(
    home_dir: &Path,
    kind: PermissionRuleSettingKind,
    rule: &str,
) -> Result<PermissionRuleSettingsUpdate, ConfigError> {
    update_permission_rule_setting(home_dir, kind, rule, true).await
}

pub async fn remove_permission_rule_setting(
    home_dir: &Path,
    kind: PermissionRuleSettingKind,
    rule: &str,
) -> Result<PermissionRuleSettingsUpdate, ConfigError> {
    update_permission_rule_setting(home_dir, kind, rule, false).await
}

async fn update_permission_rule_setting(
    home_dir: &Path,
    kind: PermissionRuleSettingKind,
    rule: &str,
    add: bool,
) -> Result<PermissionRuleSettingsUpdate, ConfigError> {
    let rule = rule.trim();
    if rule.is_empty() {
        return Err(ConfigError::Config(
            "permission rule cannot be empty".into(),
        ));
    }

    let path = home_dir.join("settings.json");
    tokio::fs::create_dir_all(home_dir).await?;

    let mut settings = if tokio::fs::try_exists(&path).await? {
        let contents = tokio::fs::read_to_string(&path).await?;
        match serde_json::from_str::<Value>(&contents)? {
            Value::Object(settings) => settings,
            _ => Map::new(),
        }
    } else {
        Map::new()
    };

    let rules = permission_rule_array(&mut settings, kind);
    let existing_index = rules
        .iter()
        .position(|value| value.as_str().is_some_and(|existing| existing == rule));
    let changed = if add {
        if existing_index.is_none() {
            rules.push(Value::String(rule.to_string()));
            true
        } else {
            false
        }
    } else if let Some(index) = existing_index {
        rules.remove(index);
        true
    } else {
        false
    };

    if changed {
        let payload = format!(
            "{}\n",
            serde_json::to_string_pretty(&Value::Object(settings))?
        );
        let tmp_path = path.with_extension("json.tmp");
        tokio::fs::write(&tmp_path, payload).await?;
        tokio::fs::rename(tmp_path, &path).await?;
    }

    Ok(PermissionRuleSettingsUpdate {
        path,
        rule: rule.to_string(),
        changed,
    })
}

fn permission_rule_array(
    settings: &mut Map<String, Value>,
    kind: PermissionRuleSettingKind,
) -> &mut Vec<Value> {
    let permissions = settings
        .entry("permissions".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !permissions.is_object() {
        *permissions = Value::Object(Map::new());
    }
    let permissions = permissions.as_object_mut().expect("permissions object");
    let rules = permissions
        .entry(kind.as_str().to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !rules.is_array() {
        *rules = Value::Array(Vec::new());
    }
    rules.as_array_mut().expect("permission rule array")
}

const ENABLED_MCPJSON_KEY: &str = "enabledMcpjsonServers";
const DISABLED_MCPJSON_KEY: &str = "disabledMcpjsonServers";

/// Persist an MCP server trust decision into the User settings layer
/// (`home_dir/settings.json`), the same file other permission decisions use.
/// Returns the settings path that was written.
pub async fn set_mcp_server_trust_setting(
    home_dir: &Path,
    server_id: &str,
    setting: McpServerTrustSetting,
) -> Result<PathBuf, ConfigError> {
    let server_id = server_id.trim();
    if server_id.is_empty() {
        return Err(ConfigError::Config("mcp server id cannot be empty".into()));
    }

    let path = home_dir.join("settings.json");
    tokio::fs::create_dir_all(home_dir).await?;

    let mut settings = if tokio::fs::try_exists(&path).await? {
        let contents = tokio::fs::read_to_string(&path).await?;
        match serde_json::from_str::<Value>(&contents)? {
            Value::Object(settings) => settings,
            _ => Map::new(),
        }
    } else {
        Map::new()
    };

    let (enabled, disabled) = match setting {
        McpServerTrustSetting::Trusted => (true, false),
        McpServerTrustSetting::Denied => (false, true),
        McpServerTrustSetting::Cleared => (false, false),
    };
    let mut changed = false;
    changed |= set_string_in_array(&mut settings, ENABLED_MCPJSON_KEY, server_id, enabled);
    changed |= set_string_in_array(&mut settings, DISABLED_MCPJSON_KEY, server_id, disabled);

    if changed {
        let payload = format!(
            "{}\n",
            serde_json::to_string_pretty(&Value::Object(settings))?
        );
        let tmp_path = path.with_extension("json.tmp");
        tokio::fs::write(&tmp_path, payload).await?;
        tokio::fs::rename(tmp_path, &path).await?;
    }

    Ok(path)
}

/// Read persisted MCP trust decisions across the User → Project → Local
/// settings layers (later layers win per server), matching the file set that
/// MCP config loading already consults. Only servers with an explicit
/// `Trusted`/`Denied` decision appear in the result.
pub async fn load_mcp_trust_overrides(
    home_dir: &Path,
    cwd: &Path,
) -> BTreeMap<String, McpServerTrustSetting> {
    let mut overrides = BTreeMap::new();
    let layer_files = [
        home_dir.join("settings.json"),
        cwd.join(".claude").join("settings.json"),
        cwd.join(".claude").join("settings.local.json"),
    ];
    for path in layer_files {
        apply_mcp_trust_layer(&path, &mut overrides).await;
    }
    overrides
}

async fn apply_mcp_trust_layer(
    path: &Path,
    overrides: &mut BTreeMap<String, McpServerTrustSetting>,
) {
    let Ok(contents) = tokio::fs::read_to_string(path).await else {
        return;
    };
    let Ok(Value::Object(settings)) = serde_json::from_str::<Value>(&contents) else {
        return;
    };
    for id in settings_string_array(&settings, ENABLED_MCPJSON_KEY) {
        overrides.insert(id, McpServerTrustSetting::Trusted);
    }
    // A deny within the same layer wins over an enable: trust must fail closed.
    for id in settings_string_array(&settings, DISABLED_MCPJSON_KEY) {
        overrides.insert(id, McpServerTrustSetting::Denied);
    }
}

fn set_string_in_array(
    settings: &mut Map<String, Value>,
    key: &str,
    value: &str,
    present: bool,
) -> bool {
    if present {
        let array = string_array_mut(settings, key);
        if array.iter().any(|item| item.as_str() == Some(value)) {
            false
        } else {
            array.push(Value::String(value.to_string()));
            true
        }
    } else {
        match settings.get_mut(key).and_then(Value::as_array_mut) {
            Some(array) => {
                let before = array.len();
                array.retain(|item| item.as_str() != Some(value));
                before != array.len()
            }
            None => false,
        }
    }
}

fn string_array_mut<'a>(settings: &'a mut Map<String, Value>, key: &str) -> &'a mut Vec<Value> {
    let entry = settings
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !entry.is_array() {
        *entry = Value::Array(Vec::new());
    }
    entry.as_array_mut().expect("string array")
}

fn settings_string_array(settings: &Map<String, Value>, key: &str) -> Vec<String> {
    settings
        .get(key)
        .and_then(Value::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub async fn update_model_setting(home_dir: &Path, model: Option<&str>) -> Result<(), ConfigError> {
    let path = home_dir.join("settings.json");
    tokio::fs::create_dir_all(home_dir).await?;

    let mut settings = if tokio::fs::try_exists(&path).await? {
        let contents = tokio::fs::read_to_string(&path).await?;
        match serde_json::from_str::<Value>(&contents)? {
            Value::Object(settings) => settings,
            _ => Map::new(),
        }
    } else {
        Map::new()
    };

    match model.map(str::trim).filter(|model| !model.is_empty()) {
        Some(model) => {
            settings.insert("model".to_string(), Value::String(model.to_string()));
        }
        None => {
            settings.remove("model");
        }
    }

    let payload = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(settings))?
    );
    let tmp_path = path.with_extension("json.tmp");
    tokio::fs::write(&tmp_path, payload).await?;
    tokio::fs::rename(tmp_path, path).await?;
    Ok(())
}

pub async fn update_theme_setting(home_dir: &Path, theme: ThemeSetting) -> Result<(), ConfigError> {
    let path = home_dir.join("settings.json");
    tokio::fs::create_dir_all(home_dir).await?;

    let mut settings = if tokio::fs::try_exists(&path).await? {
        let contents = tokio::fs::read_to_string(&path).await?;
        match serde_json::from_str::<Value>(&contents)? {
            Value::Object(settings) => settings,
            _ => Map::new(),
        }
    } else {
        Map::new()
    };

    settings.insert(
        "theme".to_string(),
        Value::String(theme.as_str().to_string()),
    );

    let payload = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(settings))?
    );
    let tmp_path = path.with_extension("json.tmp");
    tokio::fs::write(&tmp_path, payload).await?;
    tokio::fs::rename(tmp_path, path).await?;
    Ok(())
}

pub async fn update_editor_mode_setting(
    home_dir: &Path,
    mode: EditorModeSetting,
) -> Result<(), ConfigError> {
    let path = home_dir.join("settings.json");
    tokio::fs::create_dir_all(home_dir).await?;

    let mut settings = if tokio::fs::try_exists(&path).await? {
        let contents = tokio::fs::read_to_string(&path).await?;
        match serde_json::from_str::<Value>(&contents)? {
            Value::Object(settings) => settings,
            _ => Map::new(),
        }
    } else {
        Map::new()
    };

    settings.insert(
        "editorMode".to_string(),
        Value::String(mode.as_str().to_string()),
    );

    let payload = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(settings))?
    );
    let tmp_path = path.with_extension("json.tmp");
    tokio::fs::write(&tmp_path, payload).await?;
    tokio::fs::rename(tmp_path, path).await?;
    Ok(())
}

pub async fn update_auto_memory_setting(home_dir: &Path, enabled: bool) -> Result<(), ConfigError> {
    let path = home_dir.join("settings.json");
    tokio::fs::create_dir_all(home_dir).await?;

    let mut settings = if tokio::fs::try_exists(&path).await? {
        let contents = tokio::fs::read_to_string(&path).await?;
        match serde_json::from_str::<Value>(&contents)? {
            Value::Object(settings) => settings,
            _ => Map::new(),
        }
    } else {
        Map::new()
    };

    settings.insert("autoMemoryEnabled".to_string(), Value::Bool(enabled));

    let payload = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(settings))?
    );
    let tmp_path = path.with_extension("json.tmp");
    tokio::fs::write(&tmp_path, payload).await?;
    tokio::fs::rename(tmp_path, path).await?;
    Ok(())
}

pub async fn load_output_style_setting(home_dir: &Path, cwd: &Path) -> Result<String, ConfigError> {
    if let Some(style) = read_settings_string(&local_settings_path(cwd), "outputStyle").await? {
        return Ok(style);
    }
    if let Some(style) =
        read_settings_string(&home_dir.join("settings.json"), "outputStyle").await?
    {
        return Ok(style);
    }
    Ok(DEFAULT_OUTPUT_STYLE_NAME.to_string())
}

pub async fn output_style_options(
    home_dir: &Path,
    cwd: &Path,
) -> Result<Vec<OutputStyleOption>, ConfigError> {
    let current = load_output_style_setting(home_dir, cwd).await?;
    Ok(load_output_style_definitions(home_dir, cwd)
        .await?
        .into_iter()
        .map(|definition| OutputStyleOption {
            current: definition.name.eq_ignore_ascii_case(&current),
            value: definition.name.clone(),
            label: output_style_option_label(&definition.name),
            description: definition.description,
        })
        .collect())
}

pub async fn update_output_style_setting(cwd: &Path, style: &str) -> Result<PathBuf, ConfigError> {
    update_local_settings(cwd, |settings| {
        settings.insert("outputStyle".to_string(), Value::String(style.to_string()));
    })
    .await
}

pub async fn load_sandbox_local_settings(cwd: &Path) -> Result<SandboxLocalSettings, ConfigError> {
    let path = local_settings_path(cwd);
    if !tokio::fs::try_exists(&path).await? {
        return Ok(SandboxLocalSettings::default());
    }

    let contents = tokio::fs::read_to_string(path).await?;
    let settings = match serde_json::from_str::<Value>(&contents)? {
        Value::Object(settings) => settings,
        _ => return Ok(SandboxLocalSettings::default()),
    };
    let Some(sandbox) = settings.get("sandbox").and_then(Value::as_object) else {
        return Ok(SandboxLocalSettings::default());
    };

    Ok(SandboxLocalSettings {
        enabled: sandbox
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        auto_allow_bash_if_sandboxed: sandbox
            .get("autoAllowBashIfSandboxed")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        allow_unsandboxed_commands: sandbox
            .get("allowUnsandboxedCommands")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        excluded_commands: sandbox
            .get("excludedCommands")
            .and_then(Value::as_array)
            .map(|commands| {
                commands
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        filesystem: parse_sandbox_filesystem_settings(sandbox),
        network: parse_sandbox_network_settings(sandbox),
    })
}

fn parse_sandbox_filesystem_settings(
    sandbox: &Map<String, Value>,
) -> SandboxFilesystemLocalSettings {
    let Some(filesystem) = sandbox.get("filesystem").and_then(Value::as_object) else {
        return SandboxFilesystemLocalSettings::default();
    };
    SandboxFilesystemLocalSettings {
        allow_write: string_array_setting(filesystem, "allowWrite"),
        deny_write: string_array_setting(filesystem, "denyWrite"),
        deny_read: string_array_setting(filesystem, "denyRead"),
        allow_read: string_array_setting(filesystem, "allowRead"),
    }
}

fn parse_sandbox_network_settings(sandbox: &Map<String, Value>) -> SandboxNetworkLocalSettings {
    let Some(network) = sandbox.get("network").and_then(Value::as_object) else {
        return SandboxNetworkLocalSettings::default();
    };
    SandboxNetworkLocalSettings {
        allowed_domains: string_array_setting(network, "allowedDomains"),
        allow_unix_sockets: string_array_setting(network, "allowUnixSockets"),
        allow_all_unix_sockets: network.get("allowAllUnixSockets").and_then(Value::as_bool),
        allow_local_binding: network.get("allowLocalBinding").and_then(Value::as_bool),
        http_proxy_port: network.get("httpProxyPort").and_then(Value::as_u64),
        socks_proxy_port: network.get("socksProxyPort").and_then(Value::as_u64),
    }
}

fn string_array_setting(settings: &Map<String, Value>, key: &str) -> Vec<String> {
    settings
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub async fn update_sandbox_settings(
    cwd: &Path,
    update: SandboxSettingsUpdate,
) -> Result<PathBuf, ConfigError> {
    update_local_settings(cwd, |settings| {
        let sandbox = sandbox_settings_object(settings);
        if let Some(enabled) = update.enabled {
            sandbox.insert("enabled".to_string(), Value::Bool(enabled));
        }
        if let Some(auto_allow) = update.auto_allow_bash_if_sandboxed {
            sandbox.insert(
                "autoAllowBashIfSandboxed".to_string(),
                Value::Bool(auto_allow),
            );
        }
        if let Some(allow_unsandboxed) = update.allow_unsandboxed_commands {
            sandbox.insert(
                "allowUnsandboxedCommands".to_string(),
                Value::Bool(allow_unsandboxed),
            );
        }
    })
    .await
}

pub async fn add_sandbox_excluded_command(
    cwd: &Path,
    command_pattern: &str,
) -> Result<PathBuf, ConfigError> {
    update_local_settings(cwd, |settings| {
        let sandbox = sandbox_settings_object(settings);
        let excluded_commands = sandbox
            .entry("excludedCommands".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if !excluded_commands.is_array() {
            *excluded_commands = Value::Array(Vec::new());
        }
        let excluded_commands = excluded_commands
            .as_array_mut()
            .expect("excludedCommands array");

        if !excluded_commands
            .iter()
            .any(|value| value.as_str() == Some(command_pattern))
        {
            excluded_commands.push(Value::String(command_pattern.to_string()));
        }
    })
    .await
}

fn local_settings_path(cwd: &Path) -> PathBuf {
    cwd.join(".claude").join("settings.local.json")
}

fn project_settings_path(cwd: &Path) -> PathBuf {
    cwd.join(".claude").join("settings.json")
}

async fn read_settings_string(path: &Path, key: &str) -> Result<Option<String>, ConfigError> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(None);
    }
    let contents = tokio::fs::read_to_string(path).await?;
    let Value::Object(settings) = serde_json::from_str::<Value>(&contents)? else {
        return Ok(None);
    };
    Ok(settings
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

fn output_style_option_label(name: &str) -> String {
    if name == DEFAULT_OUTPUT_STYLE_NAME {
        "Default".to_string()
    } else {
        name.to_string()
    }
}

async fn update_local_settings(
    cwd: &Path,
    update: impl FnOnce(&mut Map<String, Value>),
) -> Result<PathBuf, ConfigError> {
    let path = local_settings_path(cwd);
    let settings_dir = path
        .parent()
        .ok_or_else(|| ConfigError::Config("failed to resolve local settings directory".into()))?;
    tokio::fs::create_dir_all(settings_dir).await?;

    let mut settings = if tokio::fs::try_exists(&path).await? {
        let contents = tokio::fs::read_to_string(&path).await?;
        match serde_json::from_str::<Value>(&contents)? {
            Value::Object(settings) => settings,
            _ => Map::new(),
        }
    } else {
        Map::new()
    };

    update(&mut settings);

    let payload = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(settings))?
    );
    let tmp_path = path.with_extension("json.tmp");
    tokio::fs::write(&tmp_path, payload).await?;
    tokio::fs::rename(tmp_path, &path).await?;
    Ok(path)
}

fn sandbox_settings_object(settings: &mut Map<String, Value>) -> &mut Map<String, Value> {
    let sandbox = settings
        .entry("sandbox".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !sandbox.is_object() {
        *sandbox = Value::Object(Map::new());
    }
    sandbox.as_object_mut().expect("sandbox object")
}

pub fn resolve_env_value(settings: &ClaudeSettings, key: &str) -> Option<String> {
    resolve_env_value_with(key, &settings.env, |k| env::var(k).ok())
}

/// Resolve an env key through the alias table with an injectable process-env
/// lookup. Production callers use [`resolve_env_value`]; tests inject a
/// closure to avoid global env mutation and parallel-test races.
///
/// Precedence: canonical process env → legacy process env → canonical
/// settings env → legacy settings env. Empty strings are treated as unset
/// in both layers.
pub fn resolve_env_value_with(
    key: &str,
    settings_env: &std::collections::BTreeMap<String, String>,
    process_env: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let keys = crate::env_compat::resolve_keys(key);
    for k in &keys {
        if let Some(value) = process_env(k).filter(|v| !v.trim().is_empty()) {
            return Some(value);
        }
    }
    for k in &keys {
        if let Some(value) = settings_env.get(*k).filter(|v| !v.trim().is_empty()) {
            return Some(value.clone());
        }
    }
    None
}

pub fn sanitize_path(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    if sanitized.len() <= MAX_SANITIZED_LENGTH {
        return sanitized;
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    let hash = radix36(hasher.finish());
    format!("{}-{hash}", &sanitized[..MAX_SANITIZED_LENGTH])
}

fn radix36(mut value: u64) -> String {
    if value == 0 {
        return "0".to_string();
    }

    let mut digits = Vec::new();
    while value > 0 {
        let digit = (value % 36) as u8;
        digits.push(match digit {
            0..=9 => (b'0' + digit) as char,
            _ => (b'a' + digit - 10) as char,
        });
        value /= 36;
    }
    digits.iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Neither directory exists yet — a fresh machine keeps sharing `~/.claude`
    /// with the TypeScript CLI rather than silently starting a separate store.
    #[test]
    fn default_home_prefers_claude_when_orbcode_dir_is_absent() {
        let home = tempfile::tempdir().expect("temp home");
        assert_eq!(
            default_home_dir(home.path()),
            home.path().join(".claude"),
            "no ~/.orbcode means stay on the shared home"
        );
    }

    /// The opt-in: once `~/.orbcode` exists, it wins.
    #[test]
    fn default_home_prefers_orbcode_dir_when_present() {
        let home = tempfile::tempdir().expect("temp home");
        std::fs::create_dir(home.path().join(".orbcode")).expect("create ~/.orbcode");
        std::fs::create_dir(home.path().join(".claude")).expect("create ~/.claude");
        assert_eq!(
            default_home_dir(home.path()),
            home.path().join(".orbcode"),
            "~/.orbcode takes precedence even when ~/.claude also exists"
        );
    }

    /// `default_home_dir` must not create anything; the opt-in is the user's move.
    #[test]
    fn default_home_never_creates_the_orbcode_dir() {
        let home = tempfile::tempdir().expect("temp home");
        let _ = default_home_dir(home.path());
        assert!(
            !home.path().join(".orbcode").exists(),
            "probing must not create ~/.orbcode"
        );
        assert!(
            !home.path().join(".claude").exists(),
            "probing must not create ~/.claude either"
        );
    }

    /// A stray *file* named `~/.orbcode` is not a home directory; fall through
    /// rather than resolving to something unusable.
    #[test]
    fn default_home_ignores_a_file_named_orbcode() {
        let home = tempfile::tempdir().expect("temp home");
        std::fs::write(home.path().join(".orbcode"), b"not a directory").expect("write file");
        assert_eq!(
            default_home_dir(home.path()),
            home.path().join(".claude"),
            "a file at ~/.orbcode must not be treated as a home dir"
        );
    }

    fn populate(dir: &Path) {
        std::fs::create_dir_all(dir).expect("create dir");
        std::fs::write(dir.join("settings.json"), b"{}").expect("write settings");
    }

    /// The case worth warning about: opted in, still empty, real data next door.
    #[test]
    fn shadowed_home_flags_empty_orbcode_beside_populated_claude() {
        let home = tempfile::tempdir().expect("temp home");
        let orbcode = home.path().join(".orbcode");
        std::fs::create_dir(&orbcode).expect("create ~/.orbcode");
        populate(&home.path().join(".claude"));

        let shadow = shadowed_home(&orbcode).expect("should flag the shadowed home");
        assert_eq!(shadow.active, orbcode);
        assert_eq!(shadow.shadowed, home.path().join(".claude"));
    }

    /// Once `~/.orbcode` is actually in use, the opt-in is working — stay quiet.
    #[test]
    fn shadowed_home_silent_once_orbcode_has_state() {
        let home = tempfile::tempdir().expect("temp home");
        let orbcode = home.path().join(".orbcode");
        populate(&orbcode);
        populate(&home.path().join(".claude"));
        assert!(shadowed_home(&orbcode).is_none());
    }

    /// Nothing to migrate from, so nothing to say.
    #[test]
    fn shadowed_home_silent_when_claude_is_empty_or_absent() {
        let home = tempfile::tempdir().expect("temp home");
        let orbcode = home.path().join(".orbcode");
        std::fs::create_dir(&orbcode).expect("create ~/.orbcode");
        assert!(shadowed_home(&orbcode).is_none(), "no ~/.claude at all");

        std::fs::create_dir(home.path().join(".claude")).expect("create empty ~/.claude");
        assert!(
            shadowed_home(&orbcode).is_none(),
            "~/.claude present but empty"
        );
    }

    /// An explicit ORBCODE_HOME elsewhere is a deliberate choice, not this mistake.
    #[test]
    fn shadowed_home_silent_for_a_home_that_is_not_dot_orbcode() {
        let home = tempfile::tempdir().expect("temp home");
        populate(&home.path().join(".claude"));
        let explicit = home.path().join("somewhere-else");
        std::fs::create_dir(&explicit).expect("create explicit home");
        assert!(shadowed_home(&explicit).is_none());
    }

    /// Any of the file markers counts on its own.
    #[test]
    fn shadowed_home_recognises_each_state_file() {
        for marker in HOME_STATE_FILES {
            let home = tempfile::tempdir().expect("temp home");
            let orbcode = home.path().join(".orbcode");
            std::fs::create_dir(&orbcode).expect("create ~/.orbcode");
            let claude = home.path().join(".claude");
            std::fs::create_dir(&claude).expect("create ~/.claude");
            std::fs::write(claude.join(marker), b"x").expect("write marker");
            assert!(
                shadowed_home(&orbcode).is_some(),
                "file marker {marker} should count as state"
            );
        }
    }

    /// A real transcript counts as state.
    #[test]
    fn shadowed_home_recognises_a_transcript_as_state() {
        let home = tempfile::tempdir().expect("temp home");
        let orbcode = home.path().join(".orbcode");
        std::fs::create_dir(&orbcode).expect("create ~/.orbcode");
        let project = home.path().join(".claude").join("projects").join("-repo");
        std::fs::create_dir_all(&project).expect("create project dir");
        std::fs::write(project.join("abc.jsonl"), b"{}\n").expect("write transcript");
        assert!(shadowed_home(&orbcode).is_some());
    }

    /// The trap this check kept falling into: bootstrap creates `projects/<slug>/`
    /// on EVERY run, so neither `projects/` existing nor `projects/` being
    /// non-empty says anything about whether a home has been used. Only a
    /// transcript inside does. Without this, both sides always look populated and
    /// the warning never fires.
    #[test]
    fn shadowed_home_ignores_bootstrap_created_project_dirs() {
        let home = tempfile::tempdir().expect("temp home");
        let orbcode = home.path().join(".orbcode");
        let claude = home.path().join(".claude");
        for dir in [&orbcode, &claude] {
            std::fs::create_dir_all(dir.join("projects").join("-repo"))
                .expect("bootstrap project dir");
            std::fs::create_dir_all(dir.join("sessions")).expect("bootstrap sessions dir");
        }
        assert!(
            shadowed_home(&orbcode).is_none(),
            "two freshly-bootstrapped homes are not a shadowing problem"
        );

        // Give ~/.claude a real transcript; ~/.orbcode still only has bootstrap dirs.
        std::fs::write(
            claude.join("projects").join("-repo").join("s.jsonl"),
            b"{}\n",
        )
        .expect("write transcript");
        assert!(
            shadowed_home(&orbcode).is_some(),
            "bootstrap dirs must not mask an unused ~/.orbcode"
        );
    }

    async fn write_text(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .expect("create parent dir");
        }
        tokio::fs::write(path, contents).await.expect("write file");
    }

    fn installed_plugin_index(plugin_root: &Path) -> String {
        format!(
            r#"{{"version":2,"plugins":{{"demo@market":[{{"scope":"user","installPath":"{}","version":"1.0.0"}}]}}}}"#,
            plugin_root.display()
        )
    }
    use crate::hooks::HookCommand;

    #[tokio::test]
    async fn load_settings_parses_pre_tool_command_hooks() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [
                                {
                                    "type": "command",
                                    "command": "printf '{}'",
                                    "if": "Bash(echo:*)",
                                    "timeout": 1.5
                                }
                            ]
                        }
                    ]
                }
            }"#,
        )
        .await
        .expect("write settings");

        let settings = load_settings(dir.path()).await.expect("load settings");
        assert_eq!(settings.model, None);
        let matcher = settings
            .hooks
            .get("PreToolUse")
            .and_then(|matchers| matchers.first())
            .expect("pre tool matcher");
        assert_eq!(matcher.matcher.as_deref(), Some("Bash"));
        let hook = matcher.hooks.first().expect("hook command");
        let HookCommand::Command {
            command,
            r#if,
            timeout,
        } = hook
        else {
            panic!("expected command hook");
        };
        assert_eq!(command, "printf '{}'");
        assert_eq!(r#if.as_deref(), Some("Bash(echo:*)"));
        assert_eq!(*timeout, Some(1.5));
    }

    #[tokio::test]
    async fn load_settings_parses_additional_directories() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{
                "permissions": {
                    "additionalDirectories": ["/tmp/other", "../sibling"]
                }
            }"#,
        )
        .await
        .expect("write settings");

        let settings = load_settings(dir.path()).await.expect("load settings");
        assert_eq!(
            settings.additional_directories,
            vec!["/tmp/other".to_string(), "../sibling".to_string()]
        );
    }

    #[tokio::test]
    async fn load_settings_parses_permission_allow_and_deny_rules() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{
                "permissions": {
                    "allow": ["Bash(cargo test:*)", "Read(src/**)"],
                    "deny": ["Bash(rm:*)"]
                }
            }"#,
        )
        .await
        .expect("write settings");

        let settings = load_settings(dir.path()).await.expect("load settings");
        assert_eq!(
            settings.allowed_tools,
            vec!["Bash(cargo test:*)".to_string(), "Read(src/**)".to_string()]
        );
        assert_eq!(settings.disallowed_tools, vec!["Bash(rm:*)".to_string()]);
    }

    #[tokio::test]
    async fn update_permission_rule_setting_preserves_unknown_fields_and_deduplicates() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{"custom":true,"permissions":{"allow":["Read(src/**)"]}}"#,
        )
        .await
        .expect("write settings");

        let added = add_permission_rule_setting(
            dir.path(),
            PermissionRuleSettingKind::Allow,
            "Bash(git:*)",
        )
        .await
        .expect("add rule");
        assert!(added.changed);
        let duplicate = add_permission_rule_setting(
            dir.path(),
            PermissionRuleSettingKind::Allow,
            "Bash(git:*)",
        )
        .await
        .expect("add duplicate");
        assert!(!duplicate.changed);
        let removed = remove_permission_rule_setting(
            dir.path(),
            PermissionRuleSettingKind::Allow,
            "Read(src/**)",
        )
        .await
        .expect("remove rule");
        assert!(removed.changed);

        let contents = tokio::fs::read_to_string(dir.path().join("settings.json"))
            .await
            .expect("read settings");
        let value: Value = serde_json::from_str(&contents).expect("json");
        assert_eq!(value["custom"], true);
        assert_eq!(
            value["permissions"]["allow"],
            serde_json::json!(["Bash(git:*)"])
        );
        assert!(contents.ends_with('\n'));
    }

    #[tokio::test]
    async fn load_settings_parses_top_level_model() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{"model":"sonnet","theme":"dark-ansi","editorMode":"vim"}"#,
        )
        .await
        .expect("write settings");

        let settings = load_settings(dir.path()).await.expect("load settings");
        assert_eq!(settings.model.as_deref(), Some("sonnet"));
        assert_eq!(settings.theme, ThemeSetting::DarkAnsi);
        assert_eq!(settings.editor_mode, EditorModeSetting::Vim);
    }

    #[tokio::test]
    async fn load_settings_treats_legacy_emacs_editor_mode_as_normal() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{"editorMode":"emacs"}"#,
        )
        .await
        .expect("write settings");

        let settings = load_settings(dir.path()).await.expect("load settings");
        assert_eq!(settings.editor_mode, EditorModeSetting::Normal);
    }

    #[tokio::test]
    async fn update_model_setting_preserves_unknown_fields() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_MODEL":"env-model"},"custom":{"nested":true},"model":"old"}"#,
        )
        .await
        .expect("write settings");

        update_model_setting(dir.path(), Some(" sonnet "))
            .await
            .expect("update model");

        let contents = tokio::fs::read_to_string(dir.path().join("settings.json"))
            .await
            .expect("read settings");
        let value: Value = serde_json::from_str(&contents).expect("json");
        assert_eq!(value["model"], "sonnet");
        assert_eq!(value["env"]["ANTHROPIC_MODEL"], "env-model");
        assert_eq!(value["custom"]["nested"], true);
        assert!(contents.ends_with('\n'));
    }

    #[tokio::test]
    async fn update_model_setting_removes_top_level_model() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{"model":"sonnet","env":{"ANTHROPIC_MODEL":"env-model"}}"#,
        )
        .await
        .expect("write settings");

        update_model_setting(dir.path(), None)
            .await
            .expect("remove model");

        let contents = tokio::fs::read_to_string(dir.path().join("settings.json"))
            .await
            .expect("read settings");
        let value: Value = serde_json::from_str(&contents).expect("json");
        assert!(value.get("model").is_none());
        assert_eq!(value["env"]["ANTHROPIC_MODEL"], "env-model");
    }

    #[tokio::test]
    async fn update_model_setting_rejects_invalid_json_without_overwrite() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("settings.json");
        tokio::fs::write(&path, "{ invalid")
            .await
            .expect("write settings");

        let error = update_model_setting(dir.path(), Some("sonnet"))
            .await
            .expect_err("invalid json");

        assert!(matches!(error, ConfigError::Json(_)));
        assert_eq!(
            tokio::fs::read_to_string(path)
                .await
                .expect("read settings"),
            "{ invalid"
        );
    }

    #[tokio::test]
    async fn update_theme_setting_preserves_unknown_fields() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{"custom":{"nested":true},"theme":"light"}"#,
        )
        .await
        .expect("write settings");

        update_theme_setting(dir.path(), ThemeSetting::DarkDaltonized)
            .await
            .expect("update theme");

        let contents = tokio::fs::read_to_string(dir.path().join("settings.json"))
            .await
            .expect("read settings");
        let value: Value = serde_json::from_str(&contents).expect("json");
        assert_eq!(value["theme"], "dark-daltonized");
        assert_eq!(value["custom"]["nested"], true);
        assert!(contents.ends_with('\n'));
    }

    #[tokio::test]
    async fn update_theme_setting_rejects_invalid_json_without_overwrite() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("settings.json");
        tokio::fs::write(&path, "{ invalid")
            .await
            .expect("write settings");

        let error = update_theme_setting(dir.path(), ThemeSetting::Dark)
            .await
            .expect_err("invalid json");

        assert!(matches!(error, ConfigError::Json(_)));
        assert_eq!(
            tokio::fs::read_to_string(path)
                .await
                .expect("read settings"),
            "{ invalid"
        );
    }

    #[tokio::test]
    async fn update_editor_mode_setting_preserves_unknown_fields() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{"custom":{"nested":true},"editorMode":"normal"}"#,
        )
        .await
        .expect("write settings");

        update_editor_mode_setting(dir.path(), EditorModeSetting::Vim)
            .await
            .expect("update editor mode");

        let contents = tokio::fs::read_to_string(dir.path().join("settings.json"))
            .await
            .expect("read settings");
        let value: Value = serde_json::from_str(&contents).expect("json");
        assert_eq!(value["editorMode"], "vim");
        assert_eq!(value["custom"]["nested"], true);
        assert!(contents.ends_with('\n'));
    }

    #[tokio::test]
    async fn add_sandbox_excluded_command_creates_local_settings() {
        let dir = tempfile::tempdir().expect("temp dir");

        let path = add_sandbox_excluded_command(dir.path(), "npm run test:*")
            .await
            .expect("add excluded command");

        assert_eq!(path, dir.path().join(".claude/settings.local.json"));
        let contents = tokio::fs::read_to_string(path)
            .await
            .expect("read local settings");
        let value: Value = serde_json::from_str(&contents).expect("json");
        assert_eq!(value["sandbox"]["excludedCommands"][0], "npm run test:*");
        assert!(contents.ends_with('\n'));
    }

    #[tokio::test]
    async fn add_sandbox_excluded_command_preserves_unknown_fields() {
        let dir = tempfile::tempdir().expect("temp dir");
        let settings_dir = dir.path().join(".claude");
        tokio::fs::create_dir_all(&settings_dir)
            .await
            .expect("create settings dir");
        tokio::fs::write(
            settings_dir.join("settings.local.json"),
            r#"{"custom":{"nested":true},"sandbox":{"enabled":true,"excludedCommands":["make:*"]}}"#,
        )
        .await
        .expect("write local settings");

        add_sandbox_excluded_command(dir.path(), "npm run test:*")
            .await
            .expect("add excluded command");

        let contents = tokio::fs::read_to_string(settings_dir.join("settings.local.json"))
            .await
            .expect("read local settings");
        let value: Value = serde_json::from_str(&contents).expect("json");
        assert_eq!(value["custom"]["nested"], true);
        assert_eq!(value["sandbox"]["enabled"], true);
        assert_eq!(value["sandbox"]["excludedCommands"][0], "make:*");
        assert_eq!(value["sandbox"]["excludedCommands"][1], "npm run test:*");
    }

    #[tokio::test]
    async fn add_sandbox_excluded_command_deduplicates_patterns() {
        let dir = tempfile::tempdir().expect("temp dir");

        add_sandbox_excluded_command(dir.path(), "npm run test:*")
            .await
            .expect("add excluded command");
        add_sandbox_excluded_command(dir.path(), "npm run test:*")
            .await
            .expect("add duplicate excluded command");

        let contents = tokio::fs::read_to_string(dir.path().join(".claude/settings.local.json"))
            .await
            .expect("read local settings");
        let value: Value = serde_json::from_str(&contents).expect("json");
        assert_eq!(
            value["sandbox"]["excludedCommands"]
                .as_array()
                .expect("excluded commands")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn add_sandbox_excluded_command_rejects_invalid_json_without_overwrite() {
        let dir = tempfile::tempdir().expect("temp dir");
        let settings_dir = dir.path().join(".claude");
        tokio::fs::create_dir_all(&settings_dir)
            .await
            .expect("create settings dir");
        let path = settings_dir.join("settings.local.json");
        tokio::fs::write(&path, "{ invalid")
            .await
            .expect("write local settings");

        let error = add_sandbox_excluded_command(dir.path(), "npm run test:*")
            .await
            .expect_err("invalid json");

        assert!(matches!(error, ConfigError::Json(_)));
        assert_eq!(
            tokio::fs::read_to_string(path)
                .await
                .expect("read local settings"),
            "{ invalid"
        );
    }

    #[tokio::test]
    async fn load_sandbox_local_settings_uses_typescript_defaults() {
        let dir = tempfile::tempdir().expect("temp dir");

        let settings = load_sandbox_local_settings(dir.path())
            .await
            .expect("load sandbox settings");

        assert!(!settings.enabled);
        assert!(settings.auto_allow_bash_if_sandboxed);
        assert!(settings.allow_unsandboxed_commands);
        assert!(settings.excluded_commands.is_empty());
    }

    #[tokio::test]
    async fn load_sandbox_local_settings_reads_config_diagnostics() {
        let dir = tempfile::tempdir().expect("temp dir");
        let settings_dir = dir.path().join(".claude");
        tokio::fs::create_dir_all(&settings_dir)
            .await
            .expect("create settings dir");
        tokio::fs::write(
            settings_dir.join("settings.local.json"),
            r#"{
              "sandbox": {
                "enabled": true,
                "excludedCommands": ["npm run test:*"],
                "filesystem": {
                  "allowWrite": ["./tmp"],
                  "denyWrite": ["./secrets"],
                  "denyRead": ["./private"],
                  "allowRead": ["./private/public.md"]
                },
                "network": {
                  "allowedDomains": ["example.com"],
                  "allowUnixSockets": ["/tmp/service.sock"],
                  "allowAllUnixSockets": false,
                  "allowLocalBinding": true,
                  "httpProxyPort": 8080,
                  "socksProxyPort": 1080
                }
              }
            }"#,
        )
        .await
        .expect("write local settings");

        let settings = load_sandbox_local_settings(dir.path())
            .await
            .expect("load sandbox settings");

        assert!(settings.enabled);
        assert_eq!(settings.excluded_commands, vec!["npm run test:*"]);
        assert_eq!(settings.filesystem.allow_write, vec!["./tmp"]);
        assert_eq!(settings.filesystem.deny_write, vec!["./secrets"]);
        assert_eq!(settings.filesystem.deny_read, vec!["./private"]);
        assert_eq!(settings.filesystem.allow_read, vec!["./private/public.md"]);
        assert_eq!(settings.network.allowed_domains, vec!["example.com"]);
        assert_eq!(
            settings.network.allow_unix_sockets,
            vec!["/tmp/service.sock"]
        );
        assert_eq!(settings.network.allow_all_unix_sockets, Some(false));
        assert_eq!(settings.network.allow_local_binding, Some(true));
        assert_eq!(settings.network.http_proxy_port, Some(8080));
        assert_eq!(settings.network.socks_proxy_port, Some(1080));
    }

    #[tokio::test]
    async fn update_sandbox_settings_preserves_unknown_fields() {
        let dir = tempfile::tempdir().expect("temp dir");
        let settings_dir = dir.path().join(".claude");
        tokio::fs::create_dir_all(&settings_dir)
            .await
            .expect("create settings dir");
        tokio::fs::write(
            settings_dir.join("settings.local.json"),
            r#"{"custom":{"nested":true},"sandbox":{"excludedCommands":["make:*"]}}"#,
        )
        .await
        .expect("write local settings");

        update_sandbox_settings(
            dir.path(),
            SandboxSettingsUpdate {
                enabled: Some(true),
                auto_allow_bash_if_sandboxed: Some(false),
                allow_unsandboxed_commands: Some(false),
            },
        )
        .await
        .expect("update sandbox settings");

        let contents = tokio::fs::read_to_string(settings_dir.join("settings.local.json"))
            .await
            .expect("read local settings");
        let value: Value = serde_json::from_str(&contents).expect("json");
        assert_eq!(value["custom"]["nested"], true);
        assert_eq!(value["sandbox"]["enabled"], true);
        assert_eq!(value["sandbox"]["autoAllowBashIfSandboxed"], false);
        assert_eq!(value["sandbox"]["allowUnsandboxedCommands"], false);
        assert_eq!(value["sandbox"]["excludedCommands"][0], "make:*");
        assert!(contents.ends_with('\n'));
    }

    #[tokio::test]
    async fn update_sandbox_settings_rejects_invalid_json_without_overwrite() {
        let dir = tempfile::tempdir().expect("temp dir");
        let settings_dir = dir.path().join(".claude");
        tokio::fs::create_dir_all(&settings_dir)
            .await
            .expect("create settings dir");
        let path = settings_dir.join("settings.local.json");
        tokio::fs::write(&path, "{ invalid")
            .await
            .expect("write local settings");

        let error = update_sandbox_settings(
            dir.path(),
            SandboxSettingsUpdate {
                enabled: Some(true),
                auto_allow_bash_if_sandboxed: Some(true),
                allow_unsandboxed_commands: None,
            },
        )
        .await
        .expect_err("invalid json");

        assert!(matches!(error, ConfigError::Json(_)));
        assert_eq!(
            tokio::fs::read_to_string(path)
                .await
                .expect("read local settings"),
            "{ invalid"
        );
    }

    #[tokio::test]
    async fn update_output_style_setting_preserves_unknown_fields() {
        let dir = tempfile::tempdir().expect("temp dir");
        let settings_dir = dir.path().join(".claude");
        tokio::fs::create_dir_all(&settings_dir)
            .await
            .expect("create settings dir");
        tokio::fs::write(
            settings_dir.join("settings.local.json"),
            r#"{"custom":{"nested":true},"sandbox":{"enabled":true}}"#,
        )
        .await
        .expect("write local settings");

        update_output_style_setting(dir.path(), "Learning")
            .await
            .expect("update output style");

        let contents = tokio::fs::read_to_string(settings_dir.join("settings.local.json"))
            .await
            .expect("read local settings");
        let value: Value = serde_json::from_str(&contents).expect("json");
        assert_eq!(value["outputStyle"], "Learning");
        assert_eq!(value["custom"]["nested"], true);
        assert_eq!(value["sandbox"]["enabled"], true);
        assert!(contents.ends_with('\n'));
    }

    #[tokio::test]
    async fn update_output_style_setting_rejects_invalid_json_without_overwrite() {
        let dir = tempfile::tempdir().expect("temp dir");
        let settings_dir = dir.path().join(".claude");
        tokio::fs::create_dir_all(&settings_dir)
            .await
            .expect("create settings dir");
        let path = settings_dir.join("settings.local.json");
        tokio::fs::write(&path, "{ invalid")
            .await
            .expect("write local settings");

        let error = update_output_style_setting(dir.path(), "Learning")
            .await
            .expect_err("invalid json");

        assert!(matches!(error, ConfigError::Json(_)));
        assert_eq!(
            tokio::fs::read_to_string(path)
                .await
                .expect("read local settings"),
            "{ invalid"
        );
    }

    #[tokio::test]
    async fn output_style_options_include_custom_project_styles() {
        let home = tempfile::tempdir().expect("home dir");
        let cwd = tempfile::tempdir().expect("cwd dir");
        let styles_dir = cwd.path().join(".claude/output-styles");
        tokio::fs::create_dir_all(&styles_dir)
            .await
            .expect("create styles dir");
        tokio::fs::write(styles_dir.join("Concise.md"), "# Concise\nBrief answers")
            .await
            .expect("write style");
        update_output_style_setting(cwd.path(), "Concise")
            .await
            .expect("set output style");

        let options = output_style_options(home.path(), cwd.path())
            .await
            .expect("load options");

        assert!(options.iter().any(|option| option.value == "default"));
        let concise = options
            .iter()
            .find(|option| option.value == "Concise")
            .expect("custom style");
        assert_eq!(concise.label, "Concise");
        assert_eq!(concise.description, "Concise");
        assert!(concise.current);
    }

    #[tokio::test]
    async fn output_style_options_use_frontmatter_name_as_value() {
        let home = tempfile::tempdir().expect("home dir");
        let cwd = tempfile::tempdir().expect("cwd dir");
        write_text(
            &cwd.path()
                .join(".claude")
                .join("output-styles")
                .join("file-stem.md"),
            "---\nname: Frontmatter Name\ndescription: canonical description\n---\nbody",
        )
        .await;
        update_output_style_setting(cwd.path(), "Frontmatter Name")
            .await
            .expect("set output style");

        let options = output_style_options(home.path(), cwd.path())
            .await
            .expect("load options");

        assert!(!options.iter().any(|option| option.value == "file-stem"));
        let option = options
            .iter()
            .find(|option| option.value == "Frontmatter Name")
            .expect("frontmatter-named option");
        assert_eq!(option.label, "Frontmatter Name");
        assert_eq!(option.description, "canonical description");
        assert!(option.current);
    }

    #[tokio::test]
    async fn output_style_options_include_only_enabled_plugin_styles() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo");
        write_text(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .await;
        write_text(
            &plugin_root.join("output-styles").join("Concise.md"),
            "---\nname: Concise\ndescription: plugin concise\n---\nplugin body",
        )
        .await;
        write_text(
            &home.join("plugins").join("installed_plugins.json"),
            &installed_plugin_index(&plugin_root),
        )
        .await;
        write_text(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;

        let enabled = output_style_options(&home, &cwd)
            .await
            .expect("load enabled options");
        let plugin_option = enabled
            .iter()
            .find(|option| option.value == "demo:Concise")
            .expect("enabled plugin style option");
        assert_eq!(plugin_option.label, "demo:Concise");
        assert_eq!(plugin_option.description, "plugin concise");

        write_text(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":false}}"#,
        )
        .await;
        let disabled = output_style_options(&home, &cwd)
            .await
            .expect("load disabled options");
        assert!(!disabled.iter().any(|option| option.value == "demo:Concise"));
    }

    #[tokio::test]
    async fn output_style_options_collapse_duplicates_using_loader_precedence() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        write_text(
            &home.join("output-styles").join("Shared.md"),
            "---\nname: Shared\ndescription: user version\n---\nuser body",
        )
        .await;
        write_text(
            &cwd.join(".claude").join("output-styles").join("a.md"),
            "---\nname: Shared\ndescription: project first\n---\nfirst body",
        )
        .await;
        write_text(
            &cwd.join(".claude").join("output-styles").join("b.md"),
            "---\nname: Shared\ndescription: project second\n---\nsecond body",
        )
        .await;

        let options = output_style_options(&home, &cwd)
            .await
            .expect("load options");

        assert_eq!(
            options
                .iter()
                .filter(|option| option.value == "Shared")
                .count(),
            1
        );
        let shared = options
            .iter()
            .find(|option| option.value == "Shared")
            .expect("shared option");
        assert_eq!(shared.description, "project second");
    }

    #[tokio::test]
    async fn output_style_options_omit_malformed_and_do_not_synthesize_unknown_current() {
        let home = tempfile::tempdir().expect("home dir");
        let cwd = tempfile::tempdir().expect("cwd dir");
        write_text(
            &cwd.path()
                .join(".claude")
                .join("output-styles")
                .join("Broken.md"),
            "---\nname: Broken\ndescription: invalid\nmissing closing delimiter",
        )
        .await;
        update_output_style_setting(cwd.path(), "Broken")
            .await
            .expect("set stale output style");

        let options = output_style_options(home.path(), cwd.path())
            .await
            .expect("load options");

        assert!(!options.iter().any(|option| option.value == "Broken"));
        assert!(!options.iter().any(|option| option.current));
    }

    #[tokio::test]
    async fn load_settings_parses_statusline_command() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{"statusline":{"command":"git rev-parse --short HEAD","refreshInterval":15}}"#,
        )
        .await
        .expect("write settings");

        let settings = load_settings(dir.path()).await.expect("load settings");
        assert_eq!(
            settings.statusline_command.as_deref(),
            Some("git rev-parse --short HEAD")
        );
        assert_eq!(settings.statusline_refresh_interval_secs, Some(15));
    }

    #[tokio::test]
    async fn merge_project_local_settings_overrides_statusline() {
        let dir = tempfile::tempdir().expect("temp dir");
        tokio::fs::write(
            dir.path().join("settings.json"),
            r#"{"statusline":{"command":"echo base","refreshInterval":60}}"#,
        )
        .await
        .expect("write settings");
        let cwd = tempfile::tempdir().expect("cwd dir");
        let settings_dir = cwd.path().join(".claude");
        tokio::fs::create_dir_all(&settings_dir)
            .await
            .expect("create settings dir");
        tokio::fs::write(
            settings_dir.join("settings.local.json"),
            r#"{"statusline":{"command":"echo override"}}"#,
        )
        .await
        .expect("write local settings");

        let mut settings = load_settings(dir.path()).await.expect("load settings");
        merge_project_local_settings(&mut settings, cwd.path())
            .await
            .expect("merge");

        assert_eq!(
            settings.statusline_command.as_deref(),
            Some("echo override")
        );
        assert_eq!(
            settings.statusline_refresh_interval_secs,
            Some(60),
            "interval should remain from base when local does not override"
        );
    }
}
