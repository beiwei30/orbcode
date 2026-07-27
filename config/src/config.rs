use std::collections::{BTreeMap, HashMap};
use std::env;
use std::path::PathBuf;

use orbcode_protocol::{ProviderId, SandboxMode};

use crate::ConfigError;
use crate::auth::{stored_api_key, stored_auth_token, usable_claude_ai_oauth_token};
use crate::claude_home::{
    ClaudeSettings, HookSource, load_settings, merge_project_local_settings,
    merge_project_settings, resolve_env_value, resolve_home_dir, sanitize_path,
};
use crate::hooks::HookMatcher;
use crate::layers::{ResolvedSettings, SettingWarning};
use crate::model_resolver::{
    ProviderModelResolution, family_model_options, known_model_option, resolve_openai_model,
    resolve_provider_model, resolve_small_fast_model,
};
use crate::policy::{
    EffectivePolicy, PolicyConflict, PolicyLockError, SettingsLayers, effective_policy,
    load_settings_layers, managed_permission_rules, policy_conflicts,
};
use crate::runtime_state::RuntimeModelOverride;
use crate::settings_resolution::{
    apply_inline_settings_overlay, parse_bool_env, parse_bool_value,
    permission_mode_default_allow_network, permission_mode_default_allow_tools,
    resolve_default_provider, resolve_layered_settings, runtime_session_override_layer,
};
use crate::tool_rules::{parse_list_env, resolve_additional_directories};

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContextWindowOptions {
    pub disable_1m_context: bool,
    pub max_context_tokens_override: Option<u32>,
    pub auto_compact_window_override: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MaxOutputTokenOptions {
    pub max_output_tokens_override: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TokenWarningOptions {
    pub auto_compact_enabled: bool,
    pub auto_compact_percent_override: Option<u32>,
    pub blocking_limit_override: Option<u32>,
}

impl Default for TokenWarningOptions {
    fn default() -> Self {
        Self {
            auto_compact_enabled: true,
            auto_compact_percent_override: None,
            blocking_limit_override: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AppConfigOverrides {
    pub home_dir: Option<PathBuf>,
    pub default_provider: Option<ProviderId>,
    pub fallback_provider: Option<ProviderId>,
    pub max_retries: Option<usize>,
    pub sandbox_mode: Option<SandboxMode>,
    pub sandbox_allow_network: Option<bool>,
    pub allow_network: Option<bool>,
    pub provider_allow_network: Option<bool>,
    pub allow_tools: Option<bool>,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub add_dirs: Vec<PathBuf>,
    pub mcp_config_inputs: Vec<String>,
    /// Test-only seal applied to `resolve_env`. Empty entries block the
    /// matching `std::env::var` lookup so a developer's shell env never
    /// bleeds into fixtures that drive resolution through `settings.env`.
    pub env_overrides: HashMap<String, String>,
    /// Optional extra system-prompt text appended for headless / SDK callers.
    pub append_system_prompt: Option<String>,
    /// Headless permission-mode preset (TS-compatible).
    pub permission_mode: Option<PermissionMode>,
    /// Optional inline settings override (raw JSON string from `--settings`).
    /// Accepted at the CLI boundary and validated to JSON; merging into
    /// `ClaudeSettings` is performed for a subset of fields below.
    pub settings_json: Option<String>,
    /// When `Some(false)`, project-local settings (`.claude/settings.local.json`)
    /// are treated as untrusted: their hook matchers are filtered out at the
    /// session boundary. Defaults to `true` (trusted) when unset.
    pub trusted_project: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    BypassPermissions,
    DontAsk,
    Plan,
    Auto,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::BypassPermissions => "bypassPermissions",
            Self::DontAsk => "dontAsk",
            Self::Plan => "plan",
            Self::Auto => "auto",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "default" => Some(Self::Default),
            "acceptEdits" | "accept-edits" => Some(Self::AcceptEdits),
            "bypassPermissions" | "bypass-permissions" => Some(Self::BypassPermissions),
            "dontAsk" | "dont-ask" => Some(Self::DontAsk),
            "plan" => Some(Self::Plan),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    /// The `allow_tools` override this mode implies, or `None` to inherit the
    /// ambient value. `plan` disables tool execution; the always-on modes
    /// enable it. Used both for the top-level session and for sub-agents that
    /// declare a `permissionMode`.
    pub fn default_allow_tools(self) -> Option<bool> {
        crate::settings_resolution::permission_mode_default_allow_tools(Some(self))
    }

    /// The `allow_network` override this mode implies, or `None` to inherit.
    pub fn default_allow_network(self) -> Option<bool> {
        crate::settings_resolution::permission_mode_default_allow_network(Some(self))
    }
}

impl serde::Serialize for PermissionMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for PermissionMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| {
            serde::de::Error::unknown_variant(
                value.as_str(),
                &[
                    "default",
                    "acceptEdits",
                    "accept-edits",
                    "bypassPermissions",
                    "bypass-permissions",
                    "dontAsk",
                    "dont-ask",
                    "plan",
                    "auto",
                ],
            )
        })
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub cwd: PathBuf,
    pub home_dir: PathBuf,
    pub sessions_dir: PathBuf,
    pub projects_dir: PathBuf,
    pub current_project_dir: PathBuf,
    pub history_path: PathBuf,
    pub settings_path: PathBuf,
    pub default_provider: ProviderId,
    pub fallback_provider: Option<ProviderId>,
    pub max_retries: usize,
    pub sandbox_mode: SandboxMode,
    pub sandbox_allow_network: bool,
    pub allow_network: bool,
    pub provider_allow_network: bool,
    pub allow_tools: bool,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    /// Rules that force an interactive prompt even when an allow rule (or the
    /// blanket tools-permission) would otherwise auto-approve. Deny > ask > allow.
    pub ask_tools: Vec<String>,
    pub additional_directories: Vec<PathBuf>,
    pub mcp_config_inputs: Vec<String>,
    pub settings: ClaudeSettings,
    pub settings_layers: SettingsLayers,
    /// Full eight-layer precedence resolution with per-value source
    /// attribution. Drives `/status` diagnostics and answers "which layer set
    /// this value".
    pub resolved_settings: ResolvedSettings,
    /// Non-fatal unknown/deprecated key warnings gathered while resolving the
    /// settings layers. Surfaced through `/status`.
    pub settings_warnings: Vec<SettingWarning>,
    pub policy: EffectivePolicy,
    pub policy_conflicts: Vec<PolicyConflict>,
    pub runtime_model_override: RuntimeModelOverride,
    pub append_system_prompt: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    /// Test-only environment overrides that take precedence over both
    /// `std::env::var` and `settings.env` for `resolve_env` lookups.
    /// Always empty for real users (default-constructed). Tests populate
    /// this to seal off inherited shell env vars (`ANTHROPIC_BASE_URL`,
    /// `ANTHROPIC_AUTH_TOKEN`, etc.) so a developer's real provider config
    /// does not bleed into the stub-backed test fixtures.
    pub env_overrides: HashMap<String, String>,
    /// When `false`, hook matchers sourced from `.claude/settings.local.json`
    /// are filtered out at the session boundary. Defaults to `true`.
    pub trusted_project: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelOption {
    pub value: Option<String>,
    pub label: String,
    pub description: String,
    pub current: bool,
}

impl AppConfig {
    pub async fn load(
        cwd: impl Into<PathBuf>,
        overrides: AppConfigOverrides,
    ) -> Result<Self, ConfigError> {
        let cwd = cwd.into();
        let home_dir = match overrides.home_dir {
            Some(home_dir) => home_dir,
            None => resolve_home_dir()?,
        };
        let mut settings = load_settings(&home_dir).await?;
        merge_project_settings(&mut settings, &cwd).await?;
        merge_project_local_settings(&mut settings, &cwd).await?;
        if let Some(raw) = overrides.settings_json.as_deref() {
            apply_inline_settings_overlay(&mut settings, raw)?;
        }
        let settings_layers = load_settings_layers(&home_dir, &cwd).await?;
        let policy = effective_policy(&settings_layers);
        let policy_conflicts = policy_conflicts(&settings_layers, &policy);
        let sessions_dir = home_dir.join("sessions");
        let projects_dir = home_dir.join("projects");
        let current_project_dir = projects_dir.join(sanitize_path(&cwd.display().to_string()));
        let history_path = home_dir.join("history.jsonl");
        let settings_path = home_dir.join("settings.json");
        tokio::fs::create_dir_all(&sessions_dir).await?;
        tokio::fs::create_dir_all(&current_project_dir).await?;
        tokio::fs::create_dir_all(&projects_dir).await?;

        let default_provider = resolve_default_provider(overrides.default_provider, &settings);

        let fallback_provider = overrides
            .fallback_provider
            .or_else(|| {
                env::var("ORBCODE_FALLBACK_PROVIDER")
                    .ok()
                    .and_then(|value| ProviderId::parse(&value))
            })
            .filter(|provider| provider != &default_provider);

        let max_retries = overrides
            .max_retries
            .or_else(|| {
                env::var("ORBCODE_MAX_RETRIES")
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or(2);

        let sandbox_mode = overrides
            .sandbox_mode
            .or_else(|| {
                env::var("ORBCODE_SANDBOX_MODE")
                    .ok()
                    .and_then(|value| SandboxMode::parse(&value))
            })
            .unwrap_or_default();

        // Managed policy can forbid bypass-permissions mode. When it does, a
        // requested bypass preset is downgraded to the default mode so the rest
        // of the resolution behaves as if bypass was never requested.
        let permission_mode = match overrides.permission_mode {
            Some(PermissionMode::BypassPermissions) if policy.disable_bypass_permissions_mode => {
                Some(PermissionMode::Default)
            }
            other => other,
        };
        let allow_tools_default = permission_mode_default_allow_tools(permission_mode);
        let allow_network_default = permission_mode_default_allow_network(permission_mode);

        let allow_network = overrides
            .allow_network
            .or(allow_network_default)
            .unwrap_or_else(|| parse_bool_env("ORBCODE_ALLOW_NETWORK").unwrap_or(true));

        let sandbox_allow_network = overrides
            .sandbox_allow_network
            .unwrap_or_else(|| parse_bool_env("ORBCODE_SANDBOX_NETWORK").unwrap_or(allow_network));

        let provider_allow_network = overrides
            .provider_allow_network
            .unwrap_or_else(|| parse_bool_env("ORBCODE_PROVIDER_NETWORK").unwrap_or(true));

        let allow_tools = overrides
            .allow_tools
            .or(allow_tools_default)
            .unwrap_or_else(|| parse_bool_env("ORBCODE_ALLOW_TOOLS").unwrap_or(false));

        let mut allowed_tools = settings.allowed_tools.clone();
        if overrides.allowed_tools.is_empty() {
            allowed_tools.extend(parse_list_env("ORBCODE_ALLOWED_TOOLS"));
        } else {
            allowed_tools.extend(overrides.allowed_tools);
        }

        let mut disallowed_tools = settings.disallowed_tools.clone();
        if overrides.disallowed_tools.is_empty() {
            disallowed_tools.extend(parse_list_env("ORBCODE_DISALLOWED_TOOLS"));
        } else {
            disallowed_tools.extend(overrides.disallowed_tools);
        }
        // Apply managed permission-rule enforcement. When the policy restricts
        // rules to managed settings, user/project/local rules are dropped
        // entirely. Managed deny rules always win because `PermissionContext`
        // checks the deny list before the allow list.
        let mut ask_tools = settings.ask_tools.clone();
        let managed_rules = managed_permission_rules(&settings_layers);
        if policy.allow_managed_permission_rules_only {
            allowed_tools.clear();
            disallowed_tools.clear();
            ask_tools.clear();
        }
        for rule in &managed_rules.deny {
            if !disallowed_tools.contains(rule) {
                disallowed_tools.push(rule.clone());
            }
        }
        for rule in &managed_rules.ask {
            if !ask_tools.contains(rule) {
                ask_tools.push(rule.clone());
            }
        }
        for rule in &managed_rules.allow {
            if !allowed_tools.contains(rule) {
                allowed_tools.push(rule.clone());
            }
        }

        let additional_directories = resolve_additional_directories(
            &cwd,
            &settings.additional_directories,
            overrides.add_dirs,
        );

        let (resolved_settings, settings_warnings) = resolve_layered_settings(
            &settings_layers,
            runtime_session_override_layer(permission_mode),
        );

        Ok(Self {
            cwd,
            home_dir,
            sessions_dir,
            projects_dir,
            current_project_dir,
            history_path,
            settings_path,
            default_provider,
            fallback_provider,
            max_retries,
            sandbox_mode,
            sandbox_allow_network,
            allow_network,
            provider_allow_network,
            allow_tools,
            allowed_tools,
            disallowed_tools,
            ask_tools,
            additional_directories,
            mcp_config_inputs: overrides.mcp_config_inputs,
            settings,
            settings_layers,
            resolved_settings,
            settings_warnings,
            policy,
            policy_conflicts,
            runtime_model_override: None,
            env_overrides: overrides.env_overrides,
            append_system_prompt: overrides
                .append_system_prompt
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            permission_mode,
            trusted_project: overrides
                .trusted_project
                .unwrap_or_else(|| parse_bool_env("ORBCODE_TRUSTED_PROJECT").unwrap_or(true)),
        })
    }

    /// The login method the managed policy forces, if any (raw policy string).
    pub fn forced_login_method(&self) -> Option<&str> {
        self.policy.forced_login_method()
    }

    /// Reject mutation of a top-level settings key locked by managed policy.
    pub fn ensure_setting_mutable(&self, key: &str) -> Result<(), PolicyLockError> {
        self.policy.ensure_setting_mutable(key)
    }

    pub fn resolve_env(&self, key: &str) -> Option<String> {
        let keys = crate::env_compat::resolve_keys(key);
        // Two-pass scan: a non-empty override anywhere in the alias
        // group wins (canonical-first priority). Only after confirming
        // no non-empty override do empty seals block the process-env
        // layer — this prevents sealed_provider_env_overrides() from
        // shadowing a later non-empty override in the same group.
        let mut sealed = false;
        for k in &keys {
            if let Some(value) = self.env_overrides.get(*k) {
                if !value.trim().is_empty() {
                    return Some(value.clone());
                }
                sealed = true;
            }
        }
        if sealed {
            for k in &keys {
                if let Some(v) = self.settings.env.get(*k).filter(|v| !v.trim().is_empty()) {
                    return Some(v.clone());
                }
            }
            return None;
        }
        resolve_env_value(&self.settings, key)
    }

    pub fn anthropic_model(&self) -> String {
        self.provider_model_resolution(ProviderId::Anthropic).model
    }

    pub fn anthropic_api_model(&self) -> String {
        self.provider_model_resolution(ProviderId::Anthropic)
            .request_model
    }

    pub fn anthropic_base_url(&self) -> String {
        self.resolve_env("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|| "https://api.anthropic.com".to_string())
    }

    pub fn anthropic_api_key(&self) -> Option<String> {
        self.resolve_env("ANTHROPIC_API_KEY")
            .or_else(|| stored_api_key(&self.home_dir, ProviderId::Anthropic))
    }

    pub fn anthropic_auth_token(&self) -> Option<String> {
        self.resolve_env("ANTHROPIC_AUTH_TOKEN")
    }

    pub fn anthropic_oauth_token(&self) -> Option<String> {
        self.resolve_env("CLAUDE_CODE_OAUTH_TOKEN")
            .or_else(|| usable_claude_ai_oauth_token(&self.home_dir))
            .or_else(|| stored_auth_token(&self.home_dir, ProviderId::Anthropic))
    }

    pub fn openai_model(&self, requested_model: &str) -> String {
        resolve_openai_model(requested_model, |key| self.resolve_env(key))
    }

    pub fn openai_base_url(&self) -> String {
        self.resolve_env("OPENAI_BASE_URL")
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
    }

    pub fn openai_api_key(&self) -> Option<String> {
        self.resolve_env("OPENAI_API_KEY")
            .or_else(|| stored_api_key(&self.home_dir, ProviderId::OpenAi))
    }

    pub fn context_window_options(&self) -> ContextWindowOptions {
        ContextWindowOptions {
            disable_1m_context: parse_bool_value(
                self.resolve_env("CLAUDE_CODE_DISABLE_1M_CONTEXT"),
            )
            .unwrap_or(false),
            max_context_tokens_override: self
                .resolve_positive_u32("CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            auto_compact_window_override: self
                .resolve_positive_u32("CLAUDE_CODE_AUTO_COMPACT_WINDOW"),
        }
    }

    pub fn max_output_token_options(&self) -> MaxOutputTokenOptions {
        MaxOutputTokenOptions {
            max_output_tokens_override: self.resolve_positive_u32("CLAUDE_CODE_MAX_OUTPUT_TOKENS"),
        }
    }

    /// The configured spend cap in USD (`maxBudgetUsd`), if any. An environment
    /// override (`CLAUDE_CODE_MAX_BUDGET_USD`, or the `ORBCODE_` alias) wins over
    /// the settings value. Only finite, positive caps are honored; anything else
    /// disables enforcement.
    pub fn max_budget_usd(&self) -> Option<f64> {
        self.resolve_positive_f64("CLAUDE_CODE_MAX_BUDGET_USD")
            .or_else(|| {
                self.settings
                    .max_budget_usd
                    .filter(|value| value.is_finite() && *value > 0.0)
            })
    }

    /// Whether to block (rather than warn and proceed) when the running cost
    /// cannot be priced. Defaults to `false` (warn-but-proceed).
    pub fn max_budget_strict_unknown_pricing(&self) -> bool {
        parse_bool_value(self.resolve_env("ORBCODE_MAX_BUDGET_STRICT_UNKNOWN"))
            .or(self.settings.max_budget_strict_unknown_pricing)
            .unwrap_or(false)
    }

    /// Returns the user-supplied `CLAUDE_CODE_EXTRA_BODY` parsed as a JSON
    /// object. Anything that isn't an object is dropped to match the
    /// TypeScript behavior (it emits a `console.error` and continues).
    pub fn extra_body(&self) -> serde_json::Map<String, serde_json::Value> {
        self.resolve_env("CLAUDE_CODE_EXTRA_BODY")
            .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            .and_then(|value| match value {
                serde_json::Value::Object(map) => Some(map),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Returns the analytics metadata envelope appended to Anthropic request
    /// bodies. Mirrors `getAPIMetadata` in the TypeScript client: a
    /// `user_id` field whose value is a JSON-encoded string carrying any
    /// `CLAUDE_CODE_EXTRA_METADATA` keys alongside session metadata.
    pub fn anthropic_metadata(&self, session_id: &str) -> serde_json::Value {
        let mut envelope = self
            .resolve_env("CLAUDE_CODE_EXTRA_METADATA")
            .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            .and_then(|value| match value {
                serde_json::Value::Object(map) => Some(map),
                _ => None,
            })
            .unwrap_or_default();
        envelope.insert(
            "session_id".to_string(),
            serde_json::Value::String(session_id.to_string()),
        );
        serde_json::json!({
            "user_id": serde_json::Value::String(serde_json::to_string(&envelope).unwrap_or_default()),
        })
    }

    /// Comma- or whitespace-separated list of Anthropic beta gates.
    pub fn anthropic_betas(&self) -> Vec<String> {
        self.resolve_env("CLAUDE_CODE_BETAS")
            .map(|value| {
                value
                    .split([',', ' ', '\t', '\n'])
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Parse `ANTHROPIC_CUSTOM_HEADERS` (one `Name: Value` pair per line) to
    /// match the TypeScript client's behavior.
    pub fn custom_headers(&self) -> Vec<(String, String)> {
        let raw = match self.resolve_env("ANTHROPIC_CUSTOM_HEADERS") {
            Some(value) => value,
            None => return Vec::new(),
        };
        raw.lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let (name, value) = trimmed.split_once(':')?;
                let name = name.trim();
                let value = value.trim();
                if name.is_empty() {
                    None
                } else {
                    Some((name.to_string(), value.to_string()))
                }
            })
            .collect()
    }

    pub fn provider_user_agent(&self) -> Option<String> {
        self.resolve_env("CLAUDE_CODE_USER_AGENT")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    pub fn provider_proxy_url(&self) -> Option<String> {
        for key in [
            "CLAUDE_CODE_PROXY",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
        ] {
            if let Some(value) = self.resolve_env(key)
                && !value.trim().is_empty()
            {
                return Some(value.trim().to_string());
            }
        }
        None
    }

    pub fn api_timeout(&self) -> Option<std::time::Duration> {
        self.resolve_env("API_TIMEOUT_MS")
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .map(std::time::Duration::from_millis)
    }

    pub fn api_max_retries(&self) -> Option<u32> {
        self.resolve_env("API_MAX_RETRIES")
            .and_then(|value| value.trim().parse::<u32>().ok())
    }

    /// Base delay (ms) for the provider retry backoff schedule. Mirrors the
    /// TypeScript client's `BASE_DELAY_MS` (500); overridable via
    /// `CLAUDE_CODE_RETRY_BASE_DELAY_MS` so tests can collapse the backoff to 0.
    pub fn retry_base_delay_ms(&self) -> u64 {
        self.resolve_env("CLAUDE_CODE_RETRY_BASE_DELAY_MS")
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(500)
    }

    /// Ceiling (ms) for the exponential retry backoff. Mirrors the TypeScript
    /// client's default `maxDelayMs` (32000); overridable via
    /// `CLAUDE_CODE_RETRY_MAX_DELAY_MS`.
    pub fn retry_max_delay_ms(&self) -> u64 {
        self.resolve_env("CLAUDE_CODE_RETRY_MAX_DELAY_MS")
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(32_000)
    }

    pub fn token_warning_options(&self) -> TokenWarningOptions {
        TokenWarningOptions {
            auto_compact_enabled: !parse_bool_value(self.resolve_env("DISABLE_COMPACT"))
                .unwrap_or(false)
                && !parse_bool_value(self.resolve_env("DISABLE_AUTO_COMPACT")).unwrap_or(false),
            auto_compact_percent_override: self
                .resolve_env("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE")
                .and_then(|value| value.trim().parse::<u32>().ok())
                .filter(|value| (1..=100).contains(value)),
            blocking_limit_override: self
                .resolve_positive_u32("CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE"),
        }
    }

    pub fn model_display_name(&self) -> String {
        self.provider_model_display_name(self.default_provider)
    }

    pub fn provider_model_display_name(&self, provider: ProviderId) -> String {
        self.provider_model_resolution(provider).display_name
    }

    pub fn provider_model_name(&self, provider: ProviderId) -> String {
        self.provider_model_resolution(provider).request_model
    }

    pub fn provider_model_resolution(&self, provider: ProviderId) -> ProviderModelResolution {
        resolve_provider_model(provider, self.provider_model_setting().as_deref(), |key| {
            self.resolve_env(key)
        })
    }

    pub fn small_fast_model_resolution(&self, provider: ProviderId) -> ProviderModelResolution {
        resolve_small_fast_model(provider, |key| self.resolve_env(key))
    }

    pub fn small_fast_model_name(&self, provider: ProviderId) -> String {
        self.small_fast_model_resolution(provider).request_model
    }

    fn resolve_positive_u32(&self, key: &str) -> Option<u32> {
        self.resolve_env(key)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|value| *value > 0)
    }

    fn resolve_positive_f64(&self, key: &str) -> Option<f64> {
        self.resolve_env(key)
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
    }

    pub fn model_options(&self) -> Vec<ModelOption> {
        self.model_options_for_setting(self.provider_model_setting().as_deref())
    }

    pub fn model_options_for_setting(&self, current_setting: Option<&str>) -> Vec<ModelOption> {
        let mut options = vec![ModelOption {
            value: None,
            label: "Default (recommended)".to_string(),
            description: format!(
                "Use the default model (currently {})",
                self.provider_model_resolution_for_setting(self.default_provider, None)
                    .display_label
            ),
            current: current_setting.is_none(),
        }];
        for option in family_model_options(self.default_provider, |key| self.resolve_env(key)) {
            options.push(ModelOption {
                value: Some(option.value.clone()),
                label: option.label,
                description: option.description,
                current: current_setting == Some(option.value.as_str()),
            });
        }
        if let Some(custom_model) = self.resolve_env("ANTHROPIC_CUSTOM_MODEL_OPTION")
            && !options
                .iter()
                .any(|option| option.value.as_deref() == Some(custom_model.as_str()))
        {
            options.push(ModelOption {
                value: Some(custom_model.clone()),
                label: self
                    .resolve_env("ANTHROPIC_CUSTOM_MODEL_OPTION_NAME")
                    .unwrap_or_else(|| custom_model.clone()),
                description: self
                    .resolve_env("ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION")
                    .unwrap_or_else(|| format!("Custom model ({custom_model})")),
                current: current_setting == Some(custom_model.as_str()),
            });
        }
        if let Some(custom_model) = self.provider_model_setting()
            && !options
                .iter()
                .any(|option| option.value.as_deref() == Some(custom_model.as_str()))
        {
            let known_option = known_model_option(self.default_provider, &custom_model, |key| {
                self.resolve_env(key)
            });
            options.push(ModelOption {
                value: Some(custom_model.clone()),
                label: known_option
                    .as_ref()
                    .map_or_else(|| custom_model.clone(), |option| option.label.clone()),
                description: known_option
                    .map_or_else(|| "Custom model".to_string(), |option| option.description),
                current: current_setting == Some(custom_model.as_str()),
            });
        }
        if self.policy.available_models.is_some() {
            options.retain(|option| {
                option.value.is_none()
                    || self.policy.model_allowed(option.value.as_deref().unwrap())
            });
        }
        options
    }

    pub fn provider_model_setting(&self) -> Option<String> {
        if let Some(model) = &self.runtime_model_override {
            return model.clone();
        }

        let provider_env_model = match self.default_provider {
            ProviderId::OpenAi => self.resolve_env("OPENAI_MODEL"),
            _ => self.resolve_env("ANTHROPIC_MODEL"),
        };
        provider_env_model.or_else(|| self.settings.model.clone())
    }

    pub fn resolve_model_setting(&self, provider: ProviderId, setting: Option<&str>) -> String {
        self.provider_model_resolution_for_setting(provider, setting)
            .request_model
    }

    fn provider_model_resolution_for_setting(
        &self,
        provider: ProviderId,
        setting: Option<&str>,
    ) -> ProviderModelResolution {
        resolve_provider_model(provider, setting, |key| self.resolve_env(key))
    }

    pub fn provider_disabled_diagnostic(&self) -> Option<String> {
        if !self.default_provider.is_active() {
            Some(format!(
                "Provider '{}' is not supported. Only {} are active providers. \
                 '{}' is retained for config/transcript compatibility but cannot serve requests.",
                self.default_provider,
                ProviderId::ACTIVE
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                self.default_provider,
            ))
        } else {
            None
        }
    }

    pub fn apply_runtime_model_override(&mut self, model: Option<&str>) {
        self.runtime_model_override = Some(
            model
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string),
        );
    }

    pub fn settings_env(&self) -> &BTreeMap<String, String> {
        &self.settings.env
    }

    pub fn hooks_for_event(&self, event: &str) -> &[HookMatcher] {
        self.settings
            .hooks
            .get(event)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn hook_sources_for_event(&self, event: &str) -> &[HookSource] {
        self.settings
            .hook_sources
            .get(event)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

// Re-export public items that were moved to submodules but are still part of
// the crate's public API surface via lib.rs.
pub use crate::settings_resolution::sealed_provider_env_overrides;
pub use crate::tool_rules::parse_tool_rule_list;

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
