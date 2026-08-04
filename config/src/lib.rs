mod agents;
mod auth;
mod claude_home;
mod config;
mod env_compat;
mod hooks;
mod keybindings;
mod layers;
mod memory;
mod model_resolver;
mod openai_oauth;
mod output_styles;
mod permission_rules;
mod plugins;
mod policy;
mod proxy;
mod runtime_state;
mod settings_resolution;
mod token_accounting;
mod tool_rules;

#[cfg(test)]
mod e2e_tests;

pub use agents::{
    AgentDefinition, AgentLoadOutcome, AgentLoadWarning, AgentSource, AgentWarningKind,
    built_in_agent_definitions, load_agent_definitions, load_agent_definitions_with_warnings,
};
pub use auth::{
    AuthManager, AuthMethod, AuthOverview, AuthStatusEntry, ClaudeAiOAuth, load_chatgpt_oauth,
    load_claude_ai_oauth, parse_forced_login_method,
};
pub use claude_home::resolve_env_value_with;
pub use claude_home::{
    ClaudeSettings, EditorModeSetting, HookSource, McpServerTrustSetting, OutputStyleOption,
    PermissionRuleSettingKind, PermissionRuleSettingsUpdate, SandboxFilesystemLocalSettings,
    SandboxLocalSettings, SandboxNetworkLocalSettings, SandboxSettingsUpdate, ShadowedHome,
    ThemeSetting, add_permission_rule_setting, add_sandbox_excluded_command,
    load_mcp_trust_overrides, load_output_style_setting, load_sandbox_local_settings,
    output_style_options, remove_permission_rule_setting, sanitize_path,
    set_mcp_server_trust_setting, shadowed_home, update_auto_memory_setting,
    update_editor_mode_setting, update_model_setting, update_output_style_setting,
    update_sandbox_settings, update_theme_setting,
};
pub use config::{
    AppConfig, AppConfigOverrides, ContextWindowOptions, MaxOutputTokenOptions, ModelOption,
    PermissionMode, TokenWarningOptions, parse_tool_rule_list, sealed_provider_env_overrides,
};
pub use env_compat::resolve_process_env;
pub use hooks::{
    ContributedHookSource, DiscoveredHook, HookCommand, HookDiscovery, HookDiscoveryWarning,
    HookLayer, HookMatcher, HookProvenance, HookValidationStatus, discover_hooks,
};
pub use keybindings::{
    KeyChord, KeyToken, KeybindingContext, ResolvedKeybindings, load_keybindings,
};
pub use layers::{ResolvedSettings, SettingOrigin, SettingWarning, SettingWarningKind};
pub use memory::{managed_memory_file, managed_rules_dir, user_memory_file, user_rules_dir};
pub use model_resolver::{
    ModelCapabilities, ModelCapability, ModelFamily, ProviderModelResolution, canonical_model_name,
    model_capabilities,
};
pub use openai_oauth::{
    CHATGPT_CODEX_BASE_URL, ChatGptBrowserLoginSession, ChatGptDeviceLoginSession,
    ChatGptOAuthCredentials, OpenAiOAuthOptions,
};
pub use output_styles::{
    DEFAULT_OUTPUT_STYLE_NAME, OutputStyleDefinition, OutputStyleLoadOutcome,
    OutputStyleLoadWarning, OutputStyleSource, OutputStyleWarningKind, ResolvedOutputStyle,
    built_in_output_style_definitions, load_output_style_definitions,
    load_output_style_definitions_with_warnings, resolve_active_output_style,
};
pub use permission_rules::{
    PermissionRule, PermissionRuleMatchMode, bash_command_allowed_by_rules, canonical_tool_name,
    mcp_permission_target, normalize_permission_rule_for_edit, suggested_bash_permission_rules,
    tool_path_allowed_by_additional_directory,
};
pub use plugins::{
    LoadedPlugin, PluginContributions, PluginInstallation, PluginLoadError, PluginLoadWarning,
    PluginManifest, PluginMcpConfigSource, PluginMcpConfigSourceKind, PluginRegistry, PluginScope,
    PluginSkillRoot, PluginToolDefinition, bundled_skills_dir, load_plugin_registry,
    plugin_contributed_hooks, plugin_mcp_config_sources, plugin_skill_roots,
    plugin_tool_definitions,
};
pub use policy::{
    EffectivePolicy, EffectiveValue, ManagedOrigin, PolicyConflict, PolicyConflictKind,
    PolicyLockError, SettingsLayer, SettingsLayerError, SettingsLayers, SettingsSource,
    StrictPluginOnly,
};
pub use proxy::{OutboundProxyConfig, OutboundProxyRoute};
pub use runtime_state::{RuntimeModelOverride, RuntimeSessionState};
pub use token_accounting::{
    AUTOCOMPACT_BUFFER_TOKENS, COMPACT_MAX_OUTPUT_TOKENS, ERROR_THRESHOLD_BUFFER_TOKENS,
    MANUAL_COMPACT_BUFFER_TOKENS, MAX_OUTPUT_TOKENS_DEFAULT, MAX_OUTPUT_TOKENS_UPPER_LIMIT,
    MODEL_CONTEXT_WINDOW_DEFAULT, ModelMaxOutputTokens, TokenWarningState,
    WARNING_THRESHOLD_BUFFER_TOKENS, auto_compact_threshold, calculate_token_warning_state,
    effective_context_window_size, has_1m_context, model_supports_1m_context,
    prompt_too_long_preflight_message, resolve_context_window, resolve_max_output_tokens,
    resolve_model_max_output_tokens,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
