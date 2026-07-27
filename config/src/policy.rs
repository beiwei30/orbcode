use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::ConfigError;
use crate::memory::managed_memory_dir;

const MANAGED_SETTINGS_FILE: &str = "managed-settings.json";
const MANAGED_DROP_IN_DIR: &str = "managed-settings.d";

/// A logical layer of settings, ordered by merge priority.
///
/// Order in [`SettingsLayers::layers`] is low-to-high priority. `Managed`
/// always wins when present, matching the TypeScript merge order:
/// `user → project → local → managed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SettingsSource {
    User,
    Project,
    Local,
    Managed,
}

impl SettingsSource {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::User => "user settings",
            Self::Project => "shared project settings",
            Self::Local => "project local settings",
            Self::Managed => "enterprise managed settings",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
            Self::Managed => "managed",
        }
    }

    /// Whether end users may edit this source. `Managed` is always read-only.
    pub fn is_read_only(self) -> bool {
        matches!(self, Self::Managed)
    }
}

#[derive(Clone, Debug)]
pub struct SettingsLayerError {
    pub path: PathBuf,
    pub message: String,
}

/// One source's contribution to the effective settings.
#[derive(Clone, Debug)]
pub struct SettingsLayer {
    pub source: SettingsSource,
    /// Primary path for this layer (e.g. `managed-settings.json`, user
    /// `settings.json`, project `.claude/settings.json`).
    pub primary_path: PathBuf,
    /// All paths that contributed values to this layer. For Managed, this
    /// includes the base file plus any sorted drop-in files that existed.
    pub contributing_paths: Vec<PathBuf>,
    /// Merged raw settings for this layer, or `None` if no file existed.
    pub raw: Option<Map<String, Value>>,
    pub errors: Vec<SettingsLayerError>,
}

impl SettingsLayer {
    pub fn is_present(&self) -> bool {
        self.raw.as_ref().is_some_and(|object| !object.is_empty())
    }
}

#[derive(Clone, Debug, Default)]
pub struct SettingsLayers {
    pub layers: Vec<SettingsLayer>,
}

impl SettingsLayers {
    pub fn get(&self, source: SettingsSource) -> Option<&SettingsLayer> {
        self.layers.iter().find(|layer| layer.source == source)
    }

    /// Collect sandbox excluded commands across all layers (deduplicated,
    /// preserving first-seen order). Handles both camelCase and snake_case
    /// variants of the field name for TS compatibility.
    pub fn sandbox_excluded_commands(&self) -> Vec<String> {
        let mut commands = Vec::new();
        for layer in &self.layers {
            let Some(raw) = layer.raw.as_ref() else {
                continue;
            };
            let Some(sandbox) = raw.get("sandbox").and_then(Value::as_object) else {
                continue;
            };
            let excluded = sandbox
                .get("excludedCommands")
                .or_else(|| sandbox.get("excluded_commands"));
            let Some(values) = excluded.and_then(Value::as_array) else {
                continue;
            };
            for value in values {
                let Some(command) = value.as_str() else {
                    continue;
                };
                if !commands.iter().any(|existing: &String| existing == command) {
                    commands.push(command.to_string());
                }
            }
        }
        commands
    }
}

/// Where the active managed (policy) settings came from. Today only file-based
/// sources are modeled; future remote/MDM origins would extend this enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedOrigin {
    File,
    DropIn,
    FileAndDropIn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StrictPluginOnly {
    All,
    Surfaces(Vec<String>),
}

impl StrictPluginOnly {
    pub fn covers(&self, surface: &str) -> bool {
        match self {
            Self::All => true,
            Self::Surfaces(values) => values.iter().any(|value| value == surface),
        }
    }
}

/// Subset of resolved settings that drive policy enforcement. Stored
/// separately so callers can read enterprise-relevant state without re-parsing
/// the raw JSON each time.
#[derive(Clone, Debug, Default)]
pub struct EffectivePolicy {
    pub available_models: Option<Vec<String>>,
    pub model_overrides: BTreeMap<String, String>,
    pub allowed_mcp_servers: Option<Vec<Value>>,
    pub denied_mcp_servers: Vec<Value>,
    pub allow_managed_hooks_only: bool,
    pub allow_managed_permission_rules_only: bool,
    pub allow_managed_mcp_servers_only: bool,
    pub disable_bypass_permissions_mode: bool,
    pub strict_plugin_only_customization: Option<StrictPluginOnly>,
    pub force_login_method: Option<String>,
    pub allowed_http_hook_urls: Option<Vec<String>>,
    pub http_hook_allowed_env_vars: Option<Vec<String>>,
    pub effective_model: Option<EffectiveValue<String>>,
    pub managed_origin: Option<ManagedOrigin>,
    /// Top-level keys the managed layer pins. User/project/local edits to these
    /// keys are futile (managed wins) so in-app mutation is rejected.
    pub managed_locked_keys: BTreeSet<String>,
}

/// Raised when a settings mutation targets a surface the managed policy locks.
/// The message is enterprise-safe: it names the locked key but never the
/// managed file path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyLockError {
    pub key: String,
    pub message: String,
}

impl std::fmt::Display for PolicyLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PolicyLockError {}

/// Managed-layer permission rules, extracted for deny-wins enforcement.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManagedPermissionRules {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub ask: Vec<String>,
}

impl EffectivePolicy {
    /// Whether an MCP server may be registered. The denied list always wins;
    /// when an allowlist is present only listed servers pass; when
    /// `allowManagedMcpServersOnly` is set without an allowlist nothing
    /// non-managed is permitted.
    pub fn mcp_server_allowed(&self, server_name: &str) -> bool {
        if self
            .denied_mcp_servers
            .iter()
            .any(|value| mcp_value_matches(value, server_name))
        {
            return false;
        }
        match &self.allowed_mcp_servers {
            Some(allowed) => allowed
                .iter()
                .any(|value| mcp_value_matches(value, server_name)),
            None => !self.allow_managed_mcp_servers_only,
        }
    }

    /// The login method the enterprise forces, if any (raw policy string;
    /// callers map it onto their auth method enum).
    pub fn forced_login_method(&self) -> Option<&str> {
        self.force_login_method.as_deref()
    }

    /// Reject mutation of a managed-locked top-level settings key. Returns the
    /// locking reason so the caller can surface "locked by managed policy".
    pub fn ensure_setting_mutable(&self, key: &str) -> Result<(), PolicyLockError> {
        let flag_locked = match key {
            "permissions" => self.allow_managed_permission_rules_only,
            "hooks" => self.allow_managed_hooks_only,
            "allowedMcpServers" | "mcpServers" | "enabledMcpjsonServers" => {
                self.allow_managed_mcp_servers_only
            }
            _ => false,
        };
        let surface_locked = self
            .strict_plugin_only_customization
            .as_ref()
            .is_some_and(|strict| strict.covers(key));
        if flag_locked || surface_locked || self.managed_locked_keys.contains(key) {
            return Err(PolicyLockError {
                key: key.to_string(),
                message: format!("`{key}` is locked by managed policy and cannot be changed"),
            });
        }
        Ok(())
    }

    /// Whether a model id is permitted by `availableModels`. When the list is
    /// unset every model is allowed.
    pub fn model_allowed(&self, model: &str) -> bool {
        match &self.available_models {
            Some(available) => {
                let allowed: Vec<&str> = available.iter().map(String::as_str).collect();
                model_matches_available(model, &allowed)
            }
            None => true,
        }
    }
}

fn mcp_value_matches(value: &Value, server_name: &str) -> bool {
    match value {
        Value::String(name) => name == server_name,
        Value::Object(object) => {
            object.get("serverName").and_then(Value::as_str) == Some(server_name)
        }
        _ => false,
    }
}

/// Extract the managed layer's permission rules (allow/deny/ask).
pub fn managed_permission_rules(layers: &SettingsLayers) -> ManagedPermissionRules {
    let mut rules = ManagedPermissionRules::default();
    let Some(layer) = layers.get(SettingsSource::Managed) else {
        return rules;
    };
    let Some(object) = layer.raw.as_ref() else {
        return rules;
    };
    let Some(permissions) = object.get("permissions").and_then(Value::as_object) else {
        return rules;
    };
    rules.allow = string_array_field(permissions, "allow").unwrap_or_default();
    rules.deny = string_array_field(permissions, "deny").unwrap_or_default();
    rules.ask = string_array_field(permissions, "ask").unwrap_or_default();
    rules
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveValue<T> {
    pub value: T,
    pub source: SettingsSource,
}

#[derive(Clone, Debug)]
pub struct PolicyConflict {
    pub source: SettingsSource,
    pub source_path: PathBuf,
    pub kind: PolicyConflictKind,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyConflictKind {
    HooksIgnoredByPolicy {
        events: Vec<String>,
    },
    PermissionRulesIgnoredByPolicy {
        kinds: Vec<String>,
    },
    AllowedMcpServersOverriddenByPolicy,
    ModelNotInAvailable {
        model: String,
        available: Vec<String>,
    },
    SurfaceLockedByPolicy {
        surface: String,
    },
}

pub fn managed_settings_file() -> PathBuf {
    managed_memory_dir().join(MANAGED_SETTINGS_FILE)
}

pub fn managed_settings_drop_in_dir() -> PathBuf {
    managed_memory_dir().join(MANAGED_DROP_IN_DIR)
}

/// Load all settings layers (managed, user, project, local) for diagnostic and
/// policy resolution. The returned layers are ordered low-to-high priority.
pub async fn load_settings_layers(
    home_dir: &Path,
    cwd: &Path,
) -> Result<SettingsLayers, ConfigError> {
    let user = load_user_layer(home_dir).await?;
    let project = load_project_layer(cwd).await?;
    let local = load_local_layer(cwd).await?;
    let managed = load_managed_layer().await?;

    Ok(SettingsLayers {
        layers: vec![user, project, local, managed],
    })
}

async fn load_user_layer(home_dir: &Path) -> Result<SettingsLayer, ConfigError> {
    let path = home_dir.join("settings.json");
    let (raw, errors) = read_settings_object(&path).await;
    Ok(SettingsLayer {
        source: SettingsSource::User,
        primary_path: path.clone(),
        contributing_paths: if raw.is_some() {
            vec![path]
        } else {
            Vec::new()
        },
        raw,
        errors,
    })
}

async fn load_project_layer(cwd: &Path) -> Result<SettingsLayer, ConfigError> {
    let path = cwd.join(".claude").join("settings.json");
    let (raw, errors) = read_settings_object(&path).await;
    Ok(SettingsLayer {
        source: SettingsSource::Project,
        primary_path: path.clone(),
        contributing_paths: if raw.is_some() {
            vec![path]
        } else {
            Vec::new()
        },
        raw,
        errors,
    })
}

async fn load_local_layer(cwd: &Path) -> Result<SettingsLayer, ConfigError> {
    let path = cwd.join(".claude").join("settings.local.json");
    let (raw, errors) = read_settings_object(&path).await;
    Ok(SettingsLayer {
        source: SettingsSource::Local,
        primary_path: path.clone(),
        contributing_paths: if raw.is_some() {
            vec![path]
        } else {
            Vec::new()
        },
        raw,
        errors,
    })
}

async fn load_managed_layer() -> Result<SettingsLayer, ConfigError> {
    let base_path = managed_settings_file();
    let drop_in_dir = managed_settings_drop_in_dir();

    let mut merged: Option<Map<String, Value>> = None;
    let mut contributing = Vec::new();
    let mut errors = Vec::new();

    let (base_raw, base_errors) = read_settings_object(&base_path).await;
    errors.extend(base_errors);
    if let Some(object) = base_raw {
        if !object.is_empty() {
            contributing.push(base_path.clone());
        }
        merged = Some(object);
    }

    match collect_drop_in_files(&drop_in_dir).await {
        Ok(files) => {
            for path in files {
                let (raw, drop_errors) = read_settings_object(&path).await;
                errors.extend(drop_errors);
                let Some(object) = raw else { continue };
                if object.is_empty() {
                    continue;
                }
                contributing.push(path);
                merged = Some(match merged {
                    Some(existing) => merge_settings_objects(existing, object),
                    None => object,
                });
            }
        }
        Err(error) => {
            errors.push(SettingsLayerError {
                path: drop_in_dir.clone(),
                message: format!("failed to read managed drop-in directory: {error}"),
            });
        }
    }

    Ok(SettingsLayer {
        source: SettingsSource::Managed,
        primary_path: base_path,
        contributing_paths: contributing,
        raw: merged,
        errors,
    })
}

async fn collect_drop_in_files(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound) => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };

    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !(file_type.is_file() || file_type.is_symlink()) {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        files.push(entry.path());
    }
    files.sort();
    Ok(files)
}

async fn read_settings_object(
    path: &Path,
) -> (Option<Map<String, Value>>, Vec<SettingsLayerError>) {
    let mut errors = Vec::new();
    let exists = match tokio::fs::try_exists(path).await {
        Ok(value) => value,
        Err(error) => {
            errors.push(SettingsLayerError {
                path: path.to_path_buf(),
                message: format!("failed to check settings file: {error}"),
            });
            return (None, errors);
        }
    };
    if !exists {
        return (None, errors);
    }
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(value) => value,
        Err(error) => {
            errors.push(SettingsLayerError {
                path: path.to_path_buf(),
                message: format!("failed to read settings file: {error}"),
            });
            return (None, errors);
        }
    };
    if contents.trim().is_empty() {
        return (Some(Map::new()), errors);
    }
    match serde_json::from_str::<Value>(&contents) {
        Ok(Value::Object(object)) => (Some(object), errors),
        Ok(_) => {
            errors.push(SettingsLayerError {
                path: path.to_path_buf(),
                message: "settings file did not contain a JSON object".to_string(),
            });
            (None, errors)
        }
        Err(error) => {
            errors.push(SettingsLayerError {
                path: path.to_path_buf(),
                message: format!("settings file is not valid JSON: {error}"),
            });
            (None, errors)
        }
    }
}

/// Recursively merge `src` into `dst`, matching the TypeScript
/// `settingsMergeCustomizer` semantics: arrays concatenate and deduplicate
/// (by JSON value identity), objects deep-merge, scalars from `src` win.
pub fn merge_settings_objects(
    mut dst: Map<String, Value>,
    src: Map<String, Value>,
) -> Map<String, Value> {
    for (key, value) in src {
        match dst.remove(&key) {
            Some(existing) => {
                dst.insert(key, merge_values(existing, value));
            }
            None => {
                dst.insert(key, value);
            }
        }
    }
    dst
}

fn merge_values(dst: Value, src: Value) -> Value {
    match (dst, src) {
        (Value::Object(dst_obj), Value::Object(src_obj)) => {
            Value::Object(merge_settings_objects(dst_obj, src_obj))
        }
        (Value::Array(mut dst_arr), Value::Array(src_arr)) => {
            for value in src_arr {
                if !dst_arr.contains(&value) {
                    dst_arr.push(value);
                }
            }
            Value::Array(dst_arr)
        }
        (_, src) => src,
    }
}

/// Build the [`EffectivePolicy`] view from the loaded settings layers.
pub fn effective_policy(layers: &SettingsLayers) -> EffectivePolicy {
    let mut policy = EffectivePolicy::default();
    let managed = layers.get(SettingsSource::Managed);
    let managed_object = managed.and_then(|layer| layer.raw.as_ref());

    // Policy-only fields read exclusively from the managed layer. User /
    // project settings cannot promote themselves into policy by setting
    // these keys; they would simply be ignored here (the diagnostic surface
    // is responsible for flagging that scenario).
    if let Some(object) = managed_object {
        policy.available_models = string_array_field(object, "availableModels");
        policy.model_overrides = string_string_map_field(object, "modelOverrides");
        policy.allowed_mcp_servers = value_array_field(object, "allowedMcpServers");
        policy.allow_managed_hooks_only = bool_field(object, "allowManagedHooksOnly");
        policy.allow_managed_permission_rules_only =
            bool_field(object, "allowManagedPermissionRulesOnly");
        policy.allow_managed_mcp_servers_only = bool_field(object, "allowManagedMcpServersOnly");
        policy.strict_plugin_only_customization =
            parse_strict_plugin_only(object.get("strictPluginOnlyCustomization"));
        policy.force_login_method = object
            .get("forceLoginMethod")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(permissions) = object.get("permissions").and_then(Value::as_object) {
            policy.disable_bypass_permissions_mode = permissions
                .get("disableBypassPermissionsMode")
                .and_then(Value::as_str)
                == Some("disable");
        }
        policy.managed_locked_keys = object.keys().cloned().collect();
    }

    // `deniedMcpServers` concatenates across sources — denies always merge.
    for layer in &layers.layers {
        let Some(object) = layer.raw.as_ref() else {
            continue;
        };
        if let Some(values) = value_array_field(object, "deniedMcpServers") {
            for value in values {
                if !policy.denied_mcp_servers.contains(&value) {
                    policy.denied_mcp_servers.push(value);
                }
            }
        }
    }

    // `allowedHttpHookUrls` and `httpHookAllowedEnvVars` merge across sources
    // (same semantics as `allowedMcpServers` for non-strict mode).
    policy.allowed_http_hook_urls = merge_string_arrays(layers, "allowedHttpHookUrls");
    policy.http_hook_allowed_env_vars = merge_string_arrays(layers, "httpHookAllowedEnvVars");

    // Effective model: managed wins, then local, project, user.
    for source in [
        SettingsSource::Managed,
        SettingsSource::Local,
        SettingsSource::Project,
        SettingsSource::User,
    ] {
        let Some(layer) = layers.get(source) else {
            continue;
        };
        let Some(object) = layer.raw.as_ref() else {
            continue;
        };
        if let Some(model) = object.get("model").and_then(Value::as_str) {
            policy.effective_model = Some(EffectiveValue {
                value: model.to_string(),
                source,
            });
            break;
        }
    }

    policy.managed_origin = managed.and_then(managed_origin_of);
    policy
}

fn managed_origin_of(layer: &SettingsLayer) -> Option<ManagedOrigin> {
    if layer.contributing_paths.is_empty() {
        return None;
    }
    let base = layer
        .contributing_paths
        .iter()
        .any(|path| path == &layer.primary_path);
    let drop_ins = layer
        .contributing_paths
        .iter()
        .any(|path| path != &layer.primary_path);
    Some(match (base, drop_ins) {
        (true, true) => ManagedOrigin::FileAndDropIn,
        (true, false) => ManagedOrigin::File,
        (false, true) => ManagedOrigin::DropIn,
        (false, false) => return None,
    })
}

/// Detect actionable conflicts between the active policy and user/project
/// settings. These are enterprise-safe messages — they describe what will be
/// ignored rather than disclosing managed file paths.
pub fn policy_conflicts(layers: &SettingsLayers, policy: &EffectivePolicy) -> Vec<PolicyConflict> {
    let mut conflicts = Vec::new();

    if policy.allow_managed_hooks_only {
        for source in [
            SettingsSource::User,
            SettingsSource::Project,
            SettingsSource::Local,
        ] {
            let Some(layer) = layers.get(source) else {
                continue;
            };
            let Some(object) = layer.raw.as_ref() else {
                continue;
            };
            let Some(hooks) = object.get("hooks").and_then(Value::as_object) else {
                continue;
            };
            if hooks.is_empty() {
                continue;
            }
            let mut events: Vec<String> = hooks.keys().cloned().collect();
            events.sort();
            conflicts.push(PolicyConflict {
                source,
                source_path: layer.primary_path.clone(),
                kind: PolicyConflictKind::HooksIgnoredByPolicy {
                    events: events.clone(),
                },
                message: format!(
                    "{} hooks are ignored because the active policy restricts hooks to managed settings (events: {})",
                    source.display_name(),
                    events.join(", ")
                ),
            });
        }
    }

    if policy.allow_managed_permission_rules_only {
        for source in [
            SettingsSource::User,
            SettingsSource::Project,
            SettingsSource::Local,
        ] {
            let Some(layer) = layers.get(source) else {
                continue;
            };
            let Some(object) = layer.raw.as_ref() else {
                continue;
            };
            let Some(permissions) = object.get("permissions").and_then(Value::as_object) else {
                continue;
            };
            let mut kinds: Vec<String> = Vec::new();
            for key in ["allow", "deny", "ask"] {
                let count = permissions
                    .get(key)
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                if count > 0 {
                    kinds.push(format!("{key} ({count})"));
                }
            }
            if kinds.is_empty() {
                continue;
            }
            conflicts.push(PolicyConflict {
                source,
                source_path: layer.primary_path.clone(),
                kind: PolicyConflictKind::PermissionRulesIgnoredByPolicy {
                    kinds: kinds.clone(),
                },
                message: format!(
                    "{} permission rules are ignored because the active policy restricts rules to managed settings ({})",
                    source.display_name(),
                    kinds.join(", ")
                ),
            });
        }
    }

    if policy.allow_managed_mcp_servers_only {
        for source in [
            SettingsSource::User,
            SettingsSource::Project,
            SettingsSource::Local,
        ] {
            let Some(layer) = layers.get(source) else {
                continue;
            };
            let Some(object) = layer.raw.as_ref() else {
                continue;
            };
            if object
                .get("allowedMcpServers")
                .and_then(Value::as_array)
                .is_some_and(|values| !values.is_empty())
            {
                conflicts.push(PolicyConflict {
                    source,
                    source_path: layer.primary_path.clone(),
                    kind: PolicyConflictKind::AllowedMcpServersOverriddenByPolicy,
                    message: format!(
                        "{} allowedMcpServers entries are ignored because the active policy controls the MCP allowlist",
                        source.display_name()
                    ),
                });
            }
        }
    }

    if let Some(available) = &policy.available_models {
        let allowed: Vec<&str> = available.iter().map(String::as_str).collect();
        for source in [
            SettingsSource::Local,
            SettingsSource::Project,
            SettingsSource::User,
        ] {
            let Some(layer) = layers.get(source) else {
                continue;
            };
            let Some(object) = layer.raw.as_ref() else {
                continue;
            };
            let Some(model) = object.get("model").and_then(Value::as_str) else {
                continue;
            };
            if !model_matches_available(model, &allowed) {
                conflicts.push(PolicyConflict {
                    source,
                    source_path: layer.primary_path.clone(),
                    kind: PolicyConflictKind::ModelNotInAvailable {
                        model: model.to_string(),
                        available: available.clone(),
                    },
                    message: format!(
                        "{} requests model `{model}` which is not in the enterprise availableModels list ({})",
                        source.display_name(),
                        available.join(", ")
                    ),
                });
            }
        }
    }

    if let Some(strict) = &policy.strict_plugin_only_customization {
        for source in [
            SettingsSource::User,
            SettingsSource::Project,
            SettingsSource::Local,
        ] {
            let Some(layer) = layers.get(source) else {
                continue;
            };
            let Some(object) = layer.raw.as_ref() else {
                continue;
            };
            if strict.covers("hooks") && object.contains_key("hooks") {
                conflicts.push(make_surface_conflict(source, layer, "hooks"));
            }
            // mcp surface: project settings can carry MCP via `mcpServers` /
            // `enabledMcpjsonServers`. User settings rarely do but check both.
            if strict.covers("mcp")
                && (object.contains_key("mcpServers")
                    || object.contains_key("enabledMcpjsonServers"))
            {
                conflicts.push(make_surface_conflict(source, layer, "mcp"));
            }
            // agents/skills are typically directories outside settings.json,
            // so the conflict comes from contributions; settings-level keys
            // for these are uncommon but we still surface them if present.
            for surface in ["agents", "skills"] {
                if strict.covers(surface) && object.contains_key(surface) {
                    conflicts.push(make_surface_conflict(source, layer, surface));
                }
            }
        }
    }

    conflicts
}

fn make_surface_conflict(
    source: SettingsSource,
    layer: &SettingsLayer,
    surface: &str,
) -> PolicyConflict {
    PolicyConflict {
        source,
        source_path: layer.primary_path.clone(),
        kind: PolicyConflictKind::SurfaceLockedByPolicy {
            surface: surface.to_string(),
        },
        message: format!(
            "{} `{surface}` customizations are ignored because the active policy requires plugin-only customization for that surface",
            source.display_name()
        ),
    }
}

fn model_matches_available(model: &str, available: &[&str]) -> bool {
    available.iter().any(|allowed| {
        let allowed = *allowed;
        if allowed == model {
            return true;
        }
        // TS treats `availableModels` as accepting family aliases ("opus"
        // matches any opus model) and version prefixes ("opus-4-5" matches
        // any model whose id starts with that prefix). Prefix match is
        // sufficient for the diagnostic; exact enforcement still happens at
        // the model resolver if/when this list is enforced.
        model.starts_with(allowed)
    })
}

fn bool_field(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn string_array_field(object: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    object.get(key).and_then(Value::as_array).map(|values| {
        values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    })
}

fn value_array_field(object: &Map<String, Value>, key: &str) -> Option<Vec<Value>> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(|values| values.to_vec())
}

fn string_string_map_field(object: &Map<String, Value>, key: &str) -> BTreeMap<String, String> {
    object
        .get(key)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.into())))
                .collect()
        })
        .unwrap_or_default()
}

fn merge_string_arrays(layers: &SettingsLayers, key: &str) -> Option<Vec<String>> {
    let mut any = false;
    let mut result: Vec<String> = Vec::new();
    for layer in &layers.layers {
        let Some(object) = layer.raw.as_ref() else {
            continue;
        };
        let Some(values) = object.get(key).and_then(Value::as_array) else {
            continue;
        };
        any = true;
        for value in values {
            if let Some(value) = value.as_str()
                && !result.iter().any(|existing| existing == value)
            {
                result.push(value.to_string());
            }
        }
    }
    any.then_some(result)
}

fn parse_strict_plugin_only(value: Option<&Value>) -> Option<StrictPluginOnly> {
    let value = value?;
    if let Some(flag) = value.as_bool() {
        return flag.then_some(StrictPluginOnly::All);
    }
    let array = value.as_array()?;
    let mut surfaces = Vec::new();
    for entry in array {
        if let Some(entry) = entry.as_str() {
            // TS preprocess drops unknown surfaces to keep forward compat;
            // mirror that here so an unrecognized future surface name does
            // not poison the entire diagnostic.
            if matches!(entry, "skills" | "agents" | "hooks" | "mcp") {
                surfaces.push(entry.to_string());
            }
        }
    }
    Some(StrictPluginOnly::Surfaces(surfaces))
}

#[cfg(test)]
static MANAGED_PATH_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct ManagedPathGuard<'a> {
    _lock: std::sync::MutexGuard<'a, ()>,
    previous: Option<std::ffi::OsString>,
}

#[cfg(test)]
impl<'a> ManagedPathGuard<'a> {
    pub(crate) fn set(path: &Path) -> Self {
        let lock = MANAGED_PATH_GUARD.lock().expect("managed path lock");
        let previous = std::env::var_os("CLAUDE_CODE_MANAGED_SETTINGS_PATH");
        // SAFETY: the static mutex serializes env mutation across tests.
        unsafe { std::env::set_var("CLAUDE_CODE_MANAGED_SETTINGS_PATH", path) };
        Self {
            _lock: lock,
            previous,
        }
    }
}

#[cfg(test)]
impl Drop for ManagedPathGuard<'_> {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var("CLAUDE_CODE_MANAGED_SETTINGS_PATH", value) },
            None => unsafe { std::env::remove_var("CLAUDE_CODE_MANAGED_SETTINGS_PATH") },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn write_json(path: &Path, value: &Value) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.expect("mkdir");
        }
        tokio::fs::write(path, serde_json::to_string_pretty(value).unwrap())
            .await
            .expect("write json");
    }

    #[tokio::test]
    async fn merge_settings_objects_concats_arrays_and_deep_merges() {
        let base = json!({
            "permissions": {"allow": ["A"], "deny": ["X"]},
            "env": {"FOO": "1"},
            "hooks": {"PreToolUse": [{"matcher": "Bash"}]}
        });
        let overlay = json!({
            "permissions": {"allow": ["B"], "ask": ["?"]},
            "env": {"BAR": "2"},
            "hooks": {"PostToolUse": [{"matcher": "Edit"}]}
        });
        let merged = merge_settings_objects(
            base.as_object().unwrap().clone(),
            overlay.as_object().unwrap().clone(),
        );
        let merged = Value::Object(merged);
        assert_eq!(merged["permissions"]["allow"], json!(["A", "B"]));
        assert_eq!(merged["permissions"]["deny"], json!(["X"]));
        assert_eq!(merged["permissions"]["ask"], json!(["?"]));
        assert_eq!(merged["env"]["FOO"], json!("1"));
        assert_eq!(merged["env"]["BAR"], json!("2"));
        assert!(merged["hooks"]["PreToolUse"].is_array());
        assert!(merged["hooks"]["PostToolUse"].is_array());
    }

    #[tokio::test]
    async fn merge_settings_objects_deduplicates_arrays() {
        let base = json!({"arr": ["a", "b"]});
        let overlay = json!({"arr": ["b", "c"]});
        let merged = merge_settings_objects(
            base.as_object().unwrap().clone(),
            overlay.as_object().unwrap().clone(),
        );
        assert_eq!(Value::Object(merged)["arr"], json!(["a", "b", "c"]));
    }

    #[tokio::test]
    async fn load_managed_layer_reads_base_and_sorted_drop_ins() {
        let managed_dir = tempfile::tempdir().expect("managed dir");
        write_json(
            &managed_dir.path().join("managed-settings.json"),
            &json!({"availableModels": ["opus"], "permissions": {"allow": ["A"]}}),
        )
        .await;
        let drop_in_dir = managed_dir.path().join("managed-settings.d");
        write_json(
            &drop_in_dir.join("20-security.json"),
            &json!({"permissions": {"allow": ["C"]}, "availableModels": ["sonnet"]}),
        )
        .await;
        write_json(
            &drop_in_dir.join("10-otel.json"),
            &json!({"permissions": {"allow": ["B"]}}),
        )
        .await;
        // Hidden / non-json files must be skipped.
        tokio::fs::write(drop_in_dir.join(".hidden.json"), "{}")
            .await
            .unwrap();
        tokio::fs::write(drop_in_dir.join("README.txt"), "skip me")
            .await
            .unwrap();

        let _guard = ManagedPathGuard::set(managed_dir.path());
        let layer = load_managed_layer().await.expect("managed layer");
        let raw = layer.raw.expect("managed contents");
        assert_eq!(
            raw["permissions"]["allow"],
            json!(["A", "B", "C"]),
            "drop-ins must merge in alphabetical order on top of the base"
        );
        assert_eq!(raw["availableModels"], json!(["opus", "sonnet"]));
        assert_eq!(layer.contributing_paths.len(), 3);
        assert_eq!(
            layer
                .contributing_paths
                .iter()
                .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
                .collect::<Vec<_>>(),
            vec!["managed-settings.json", "10-otel.json", "20-security.json"]
        );
    }

    #[tokio::test]
    async fn load_managed_layer_returns_absent_when_no_files() {
        let managed_dir = tempfile::tempdir().expect("managed dir");
        let _guard = ManagedPathGuard::set(managed_dir.path());

        let layer = load_managed_layer().await.expect("managed layer");
        assert!(layer.raw.is_none());
        assert!(layer.contributing_paths.is_empty());
        assert!(layer.errors.is_empty());
    }

    #[tokio::test]
    async fn load_managed_layer_records_invalid_json() {
        let managed_dir = tempfile::tempdir().expect("managed dir");
        tokio::fs::write(
            managed_dir.path().join("managed-settings.json"),
            "{ not json",
        )
        .await
        .unwrap();
        let _guard = ManagedPathGuard::set(managed_dir.path());

        let layer = load_managed_layer().await.expect("managed layer");
        assert!(layer.raw.is_none());
        assert_eq!(layer.errors.len(), 1);
        assert!(layer.errors[0].message.contains("valid JSON"));
    }

    #[tokio::test]
    async fn effective_policy_reads_managed_flags() {
        let managed_dir = tempfile::tempdir().expect("managed dir");
        write_json(
            &managed_dir.path().join("managed-settings.json"),
            &json!({
                "availableModels": ["opus-4-7"],
                "allowManagedHooksOnly": true,
                "allowManagedPermissionRulesOnly": true,
                "allowManagedMcpServersOnly": true,
                "allowedMcpServers": [{"serverName": "alpha"}],
                "deniedMcpServers": [{"serverName": "beta"}],
                "strictPluginOnlyCustomization": ["hooks", "skills", "unknown"],
                "permissions": {"disableBypassPermissionsMode": "disable"},
                "forceLoginMethod": "console",
                "modelOverrides": {"claude-opus-4-7": "bedrock-arn"}
            }),
        )
        .await;
        let _guard = ManagedPathGuard::set(managed_dir.path());

        let layers = load_settings_layers(
            tempfile::tempdir().expect("home").path(),
            tempfile::tempdir().expect("cwd").path(),
        )
        .await
        .expect("layers");
        let policy = effective_policy(&layers);

        assert_eq!(
            policy.available_models.as_deref(),
            Some(&["opus-4-7".to_string()][..])
        );
        assert!(policy.allow_managed_hooks_only);
        assert!(policy.allow_managed_permission_rules_only);
        assert!(policy.allow_managed_mcp_servers_only);
        assert!(policy.disable_bypass_permissions_mode);
        assert_eq!(policy.force_login_method.as_deref(), Some("console"));
        assert_eq!(
            policy.model_overrides.get("claude-opus-4-7"),
            Some(&"bedrock-arn".to_string())
        );
        assert_eq!(
            policy.strict_plugin_only_customization,
            Some(StrictPluginOnly::Surfaces(vec![
                "hooks".to_string(),
                "skills".to_string()
            ]))
        );
        assert_eq!(policy.managed_origin, Some(ManagedOrigin::File));
        assert!(
            policy
                .allowed_mcp_servers
                .as_ref()
                .map_or(0, std::vec::Vec::len)
                == 1
        );
        assert_eq!(policy.denied_mcp_servers.len(), 1);
    }

    #[tokio::test]
    async fn effective_policy_picks_model_in_priority_order() {
        let managed_dir = tempfile::tempdir().expect("managed dir");
        let _guard = ManagedPathGuard::set(managed_dir.path());
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        write_json(
            &home.path().join("settings.json"),
            &json!({"model": "user-model"}),
        )
        .await;
        write_json(
            &cwd.path().join(".claude/settings.json"),
            &json!({"model": "project-model"}),
        )
        .await;
        write_json(
            &cwd.path().join(".claude/settings.local.json"),
            &json!({"model": "local-model"}),
        )
        .await;

        let layers = load_settings_layers(home.path(), cwd.path())
            .await
            .expect("layers");
        let policy = effective_policy(&layers);
        let effective = policy.effective_model.expect("effective model");
        assert_eq!(effective.value, "local-model");
        assert_eq!(effective.source, SettingsSource::Local);
    }

    #[tokio::test]
    async fn policy_conflicts_flag_ignored_hooks_and_rules() {
        let managed_dir = tempfile::tempdir().expect("managed dir");
        write_json(
            &managed_dir.path().join("managed-settings.json"),
            &json!({
                "allowManagedHooksOnly": true,
                "allowManagedPermissionRulesOnly": true,
                "allowManagedMcpServersOnly": true,
                "availableModels": ["opus", "sonnet"]
            }),
        )
        .await;
        let _guard = ManagedPathGuard::set(managed_dir.path());

        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        write_json(
            &home.path().join("settings.json"),
            &json!({
                "model": "grok-beta",
                "hooks": {"PreToolUse": [{"matcher": "Bash"}]},
                "permissions": {"allow": ["Read(src/**)", "Bash(ls:*)"]},
                "allowedMcpServers": [{"serverName": "user-srv"}]
            }),
        )
        .await;

        let layers = load_settings_layers(home.path(), cwd.path())
            .await
            .expect("layers");
        let policy = effective_policy(&layers);
        let conflicts = policy_conflicts(&layers, &policy);

        let kinds: Vec<_> = conflicts.iter().map(|c| &c.kind).collect();
        assert!(kinds.iter().any(|kind| matches!(
            kind,
            PolicyConflictKind::HooksIgnoredByPolicy { events } if events == &vec!["PreToolUse".to_string()]
        )));
        assert!(kinds.iter().any(|kind| matches!(
            kind,
            PolicyConflictKind::PermissionRulesIgnoredByPolicy { .. }
        )));
        assert!(kinds.iter().any(|kind| matches!(
            kind,
            PolicyConflictKind::AllowedMcpServersOverriddenByPolicy
        )));
        assert!(kinds.iter().any(|kind| matches!(
            kind,
            PolicyConflictKind::ModelNotInAvailable { model, .. } if model == "grok-beta"
        )));
        for conflict in &conflicts {
            assert_eq!(conflict.source, SettingsSource::User);
        }
    }

    #[tokio::test]
    async fn policy_conflicts_match_available_model_by_prefix() {
        let managed_dir = tempfile::tempdir().expect("managed dir");
        write_json(
            &managed_dir.path().join("managed-settings.json"),
            &json!({"availableModels": ["opus"]}),
        )
        .await;
        let _guard = ManagedPathGuard::set(managed_dir.path());

        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        write_json(
            &home.path().join("settings.json"),
            &json!({"model": "opus-4-7"}),
        )
        .await;

        let layers = load_settings_layers(home.path(), cwd.path())
            .await
            .expect("layers");
        let policy = effective_policy(&layers);
        let conflicts = policy_conflicts(&layers, &policy);
        assert!(
            conflicts.is_empty(),
            "opus-4-7 should match the `opus` family alias"
        );
    }

    #[test]
    fn mcp_server_allowed_honors_allowlist_and_denylist() {
        let mut policy = EffectivePolicy {
            allowed_mcp_servers: Some(vec![json!({"serverName": "alpha"}), json!("beta")]),
            denied_mcp_servers: vec![json!("beta")],
            ..EffectivePolicy::default()
        };
        // alpha is allowed; beta is denied even though it is in the allowlist.
        assert!(policy.mcp_server_allowed("alpha"));
        assert!(!policy.mcp_server_allowed("beta"));
        // gamma is not in the allowlist.
        assert!(!policy.mcp_server_allowed("gamma"));

        // No allowlist and no managed-only flag → everything is allowed.
        policy.allowed_mcp_servers = None;
        policy.denied_mcp_servers.clear();
        assert!(policy.mcp_server_allowed("anything"));

        // No allowlist but managed-only flag → nothing non-managed allowed.
        policy.allow_managed_mcp_servers_only = true;
        assert!(!policy.mcp_server_allowed("anything"));
    }

    #[test]
    fn ensure_setting_mutable_locks_managed_pinned_keys() {
        let mut policy = EffectivePolicy::default();
        policy.managed_locked_keys.insert("theme".to_string());
        let error = policy
            .ensure_setting_mutable("theme")
            .expect_err("theme must be locked");
        assert_eq!(error.key, "theme");
        assert!(error.message.contains("locked by managed policy"));
        // Unpinned keys remain mutable.
        assert!(policy.ensure_setting_mutable("outputStyle").is_ok());
    }

    #[test]
    fn ensure_setting_mutable_locks_permission_rules_under_managed_only_flag() {
        let policy = EffectivePolicy {
            allow_managed_permission_rules_only: true,
            ..EffectivePolicy::default()
        };
        assert!(policy.ensure_setting_mutable("permissions").is_err());
        assert!(policy.ensure_setting_mutable("theme").is_ok());
    }

    #[test]
    fn ensure_setting_mutable_locks_strict_plugin_surfaces() {
        let policy = EffectivePolicy {
            strict_plugin_only_customization: Some(StrictPluginOnly::Surfaces(vec![
                "hooks".to_string(),
            ])),
            ..EffectivePolicy::default()
        };
        assert!(policy.ensure_setting_mutable("hooks").is_err());
        assert!(policy.ensure_setting_mutable("mcp").is_ok());
    }

    #[test]
    fn model_allowed_uses_available_models_prefix_match() {
        let policy = EffectivePolicy {
            available_models: Some(vec!["opus".to_string()]),
            ..EffectivePolicy::default()
        };
        assert!(policy.model_allowed("opus-4-7"));
        assert!(!policy.model_allowed("grok-beta"));

        let unrestricted = EffectivePolicy::default();
        assert!(unrestricted.model_allowed("anything"));
    }

    #[test]
    fn forced_login_method_returns_raw_policy_string() {
        let policy = EffectivePolicy {
            force_login_method: Some("console".to_string()),
            ..EffectivePolicy::default()
        };
        assert_eq!(policy.forced_login_method(), Some("console"));
        assert_eq!(EffectivePolicy::default().forced_login_method(), None);
    }

    #[tokio::test]
    async fn managed_permission_rules_extracts_allow_deny_ask() {
        let managed_dir = tempfile::tempdir().expect("managed dir");
        write_json(
            &managed_dir.path().join("managed-settings.json"),
            &json!({
                "permissions": {
                    "allow": ["Read(src/**)"],
                    "deny": ["Bash(rm:*)"],
                    "ask": ["Write(**)"]
                }
            }),
        )
        .await;
        let _guard = ManagedPathGuard::set(managed_dir.path());

        let layers = load_settings_layers(
            tempfile::tempdir().expect("home").path(),
            tempfile::tempdir().expect("cwd").path(),
        )
        .await
        .expect("layers");
        let rules = managed_permission_rules(&layers);
        assert_eq!(rules.allow, vec!["Read(src/**)".to_string()]);
        assert_eq!(rules.deny, vec!["Bash(rm:*)".to_string()]);
        assert_eq!(rules.ask, vec!["Write(**)".to_string()]);
    }

    #[tokio::test]
    async fn effective_policy_records_managed_locked_keys() {
        let managed_dir = tempfile::tempdir().expect("managed dir");
        write_json(
            &managed_dir.path().join("managed-settings.json"),
            &json!({"theme": "dark", "model": "opus"}),
        )
        .await;
        let _guard = ManagedPathGuard::set(managed_dir.path());

        let layers = load_settings_layers(
            tempfile::tempdir().expect("home").path(),
            tempfile::tempdir().expect("cwd").path(),
        )
        .await
        .expect("layers");
        let policy = effective_policy(&layers);
        assert!(policy.managed_locked_keys.contains("theme"));
        assert!(policy.managed_locked_keys.contains("model"));
        assert!(policy.ensure_setting_mutable("theme").is_err());
    }

    #[tokio::test]
    async fn strict_plugin_only_flags_user_hooks_and_mcp() {
        let managed_dir = tempfile::tempdir().expect("managed dir");
        write_json(
            &managed_dir.path().join("managed-settings.json"),
            &json!({"strictPluginOnlyCustomization": true}),
        )
        .await;
        let _guard = ManagedPathGuard::set(managed_dir.path());

        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        write_json(
            &cwd.path().join(".claude/settings.json"),
            &json!({
                "hooks": {"PreToolUse": []},
                "mcpServers": {"alpha": {}}
            }),
        )
        .await;

        let layers = load_settings_layers(home.path(), cwd.path())
            .await
            .expect("layers");
        let policy = effective_policy(&layers);
        let conflicts = policy_conflicts(&layers, &policy);
        let surfaces: Vec<String> = conflicts
            .iter()
            .filter_map(|c| match &c.kind {
                PolicyConflictKind::SurfaceLockedByPolicy { surface } => Some(surface.clone()),
                _ => None,
            })
            .collect();
        assert!(surfaces.contains(&"hooks".to_string()));
        assert!(surfaces.contains(&"mcp".to_string()));
    }

    #[tokio::test]
    async fn managed_output_style_locks_key_and_rejects_mutation() {
        let managed_dir = tempfile::tempdir().expect("managed dir");
        write_json(
            &managed_dir.path().join("managed-settings.json"),
            &json!({"outputStyle": "Explanatory"}),
        )
        .await;
        let _guard = ManagedPathGuard::set(managed_dir.path());

        let layers = load_settings_layers(
            tempfile::tempdir().expect("home").path(),
            tempfile::tempdir().expect("cwd").path(),
        )
        .await
        .expect("layers");
        let policy = effective_policy(&layers);
        assert!(policy.managed_locked_keys.contains("outputStyle"));
        let err = policy
            .ensure_setting_mutable("outputStyle")
            .expect_err("outputStyle must be locked");
        assert_eq!(err.key, "outputStyle");
        assert!(err.message.contains("locked by managed policy"));
    }
}
