use std::collections::HashMap;

use orbcode_protocol::{ProviderId, SandboxMode};

use crate::claude_home::{ClaudeSettings, EditorModeSetting, ThemeSetting};
use crate::hooks::HookCommand;
use crate::model_resolver::ModelCapability;
use crate::model_resolver::{format_provider_model_display_name, normalize_model_string_for_api};
use crate::settings_resolution::sealed_provider_env_overrides;
use crate::{AuthManager, AuthMethod, ModelSelectionSource, RuntimeModelOverride, SettingOrigin};

use super::{
    AppConfig, AppConfigOverrides, EffectivePolicy, PermissionMode, SettingsLayers,
    resolve_openai_model,
};

fn test_config(default_provider: ProviderId, settings: ClaudeSettings) -> AppConfig {
    let root = std::env::temp_dir().join("orbcode-config-test");
    // Seal off any inherited shell env that would otherwise win over
    // `settings.env` via `resolve_env`. Empty values flag "explicitly
    // cleared" so the model resolver falls through to the test's
    // `settings.env` entries instead of picking up a developer's
    // local provider configuration.
    let env_overrides = sealed_provider_env_overrides();
    AppConfig {
        cwd: root.join("cwd"),
        home_dir: root.join("home"),
        sessions_dir: root.join("home/sessions"),
        projects_dir: root.join("home/projects"),
        current_project_dir: root.join("home/projects/current"),
        history_path: root.join("home/history.jsonl"),
        settings_path: root.join("home/settings.json"),
        default_provider,
        fallback_provider: None,
        max_retries: 1,
        sandbox_mode: SandboxMode::DangerFullAccess,
        sandbox_allow_network: true,
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        ask_tools: Vec::new(),
        additional_directories: Vec::new(),
        mcp_config_inputs: Vec::new(),
        settings,
        settings_layers: SettingsLayers::default(),
        resolved_settings: Default::default(),
        settings_warnings: Vec::new(),
        policy: EffectivePolicy::default(),
        policy_conflicts: Vec::new(),
        runtime_model_override: crate::RuntimeModelOverride::Inherit,
        refreshed_persisted_model_setting: None,
        env_overrides,
        append_system_prompt: None,
        permission_mode: None,
        trusted_project: true,
    }
}

/// Seal the `maxBudgetUsd` env keys so a developer's shell env cannot leak
/// into the test: an empty override blocks `std::env::var` while still
/// letting `settings.env` serve a value.
fn seal_budget_env(config: &mut AppConfig) {
    for key in [
        "CLAUDE_CODE_MAX_BUDGET_USD",
        "ORBCODE_MAX_BUDGET_USD",
        "ORBCODE_MAX_BUDGET_STRICT_UNKNOWN",
    ] {
        config.env_overrides.insert(key.to_string(), String::new());
    }
}

#[test]
fn max_budget_usd_is_none_by_default() {
    let mut config = test_config(ProviderId::Anthropic, ClaudeSettings::default());
    seal_budget_env(&mut config);
    assert_eq!(config.max_budget_usd(), None);
}

#[test]
fn max_budget_usd_reads_settings_value_when_no_env() {
    let mut config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            max_budget_usd: Some(7.5),
            ..ClaudeSettings::default()
        },
    );
    seal_budget_env(&mut config);
    assert_eq!(config.max_budget_usd(), Some(7.5));
}

#[test]
fn max_budget_usd_env_override_wins_over_settings() {
    let mut config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            max_budget_usd: Some(99.0),
            ..ClaudeSettings::default()
        },
    );
    seal_budget_env(&mut config);
    config
        .env_overrides
        .insert("CLAUDE_CODE_MAX_BUDGET_USD".to_string(), "5".to_string());
    assert_eq!(config.max_budget_usd(), Some(5.0));
}

#[test]
fn max_budget_usd_canonical_env_wins_over_legacy() {
    let mut config = test_config(ProviderId::Anthropic, ClaudeSettings::default());
    seal_budget_env(&mut config);
    config
        .env_overrides
        .insert("CLAUDE_CODE_MAX_BUDGET_USD".to_string(), "3".to_string());
    config
        .env_overrides
        .insert("ORBCODE_MAX_BUDGET_USD".to_string(), "9".to_string());
    assert_eq!(config.max_budget_usd(), Some(9.0));
}

#[test]
fn max_budget_usd_ignores_non_positive_and_non_finite() {
    for invalid in [Some(0.0), Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
        let mut config = test_config(
            ProviderId::Anthropic,
            ClaudeSettings {
                max_budget_usd: invalid,
                ..ClaudeSettings::default()
            },
        );
        seal_budget_env(&mut config);
        assert_eq!(config.max_budget_usd(), None, "invalid value: {invalid:?}");
    }
}

#[test]
fn max_budget_strict_unknown_pricing_precedence() {
    // Default: not strict.
    let mut config = test_config(ProviderId::Anthropic, ClaudeSettings::default());
    seal_budget_env(&mut config);
    assert!(!config.max_budget_strict_unknown_pricing());

    // Settings value honored when no env override.
    let mut config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            max_budget_strict_unknown_pricing: Some(true),
            ..ClaudeSettings::default()
        },
    );
    seal_budget_env(&mut config);
    assert!(config.max_budget_strict_unknown_pricing());

    // Env override wins over the settings value.
    let mut config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            max_budget_strict_unknown_pricing: Some(true),
            ..ClaudeSettings::default()
        },
    );
    seal_budget_env(&mut config);
    config.env_overrides.insert(
        "ORBCODE_MAX_BUDGET_STRICT_UNKNOWN".to_string(),
        "false".to_string(),
    );
    assert!(!config.max_budget_strict_unknown_pricing());
}

#[test]
fn permission_mode_parses_camel_and_kebab() {
    assert_eq!(
        PermissionMode::parse("bypassPermissions"),
        Some(PermissionMode::BypassPermissions)
    );
    assert_eq!(
        PermissionMode::parse("bypass-permissions"),
        Some(PermissionMode::BypassPermissions)
    );
    assert_eq!(PermissionMode::parse("unknown"), None);
}

#[test]
fn strips_context_suffix_for_api_requests() {
    assert_eq!(
        normalize_model_string_for_api("Qwen3.6-Plus-DogFooding[1m]"),
        "Qwen3.6-Plus-DogFooding"
    );
    assert_eq!(
        normalize_model_string_for_api("claude-sonnet-4-5[2M]"),
        "claude-sonnet-4-5"
    );
    assert_eq!(
        normalize_model_string_for_api("claude-sonnet-4-5"),
        "claude-sonnet-4-5"
    );
}

#[test]
fn resolves_openai_model_with_env_priority() {
    let mut env = HashMap::from([
        ("OPENAI_MODEL".to_string(), "custom-model".to_string()),
        (
            "OPENAI_DEFAULT_SONNET_MODEL".to_string(),
            "sonnet-model".to_string(),
        ),
    ]);
    assert_eq!(
        resolve_openai_model("claude-sonnet-4-6", |key| env.get(key).cloned()),
        "custom-model"
    );

    env.remove("OPENAI_MODEL");
    assert_eq!(
        resolve_openai_model("claude-sonnet-4-6", |key| env.get(key).cloned()),
        "sonnet-model"
    );
}

#[test]
fn resolves_openai_model_from_builtin_map_or_passthrough() {
    assert_eq!(
        resolve_openai_model("claude-sonnet-4-6[1m]", |_| None),
        "gpt-4o"
    );
    assert_eq!(
        resolve_openai_model("unknown-model", |_| None),
        "unknown-model"
    );
}

#[test]
fn formats_provider_model_display_name() {
    assert_eq!(
        format_provider_model_display_name("glm-4.7", ProviderId::Anthropic),
        "glm-4.7"
    );
    assert_eq!(
        format_provider_model_display_name("glm-4.7", ProviderId::OpenAi),
        "glm-4.7"
    );
}

#[test]
fn provider_model_setting_uses_top_level_model_after_env() {
    let config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            model: Some("sonnet".to_string()),
            ..ClaudeSettings::default()
        },
    );

    assert_eq!(config.provider_model_setting().as_deref(), Some("sonnet"));
}

#[test]
fn provider_env_model_wins_over_top_level_model() {
    let config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env: std::iter::once(("ANTHROPIC_MODEL".to_string(), "env-model".to_string()))
                .collect(),
            model: Some("settings-model".to_string()),
            ..ClaudeSettings::default()
        },
    );

    assert_eq!(
        config.provider_model_setting().as_deref(),
        Some("env-model")
    );
}

#[test]
fn runtime_default_override_clears_top_level_model() {
    let mut config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            model: Some("sonnet".to_string()),
            ..ClaudeSettings::default()
        },
    );

    config.apply_runtime_model_override(None);

    assert_eq!(config.provider_model_setting(), None);
}

#[test]
fn runtime_model_override_wins_over_env_model() {
    let mut config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env: std::iter::once(("ANTHROPIC_MODEL".to_string(), "env-model".to_string()))
                .collect(),
            ..ClaudeSettings::default()
        },
    );

    config.apply_runtime_model_override(Some("sonnet"));

    assert_eq!(config.provider_model_setting().as_deref(), Some("sonnet"));
}

#[test]
fn effective_model_selection_distinguishes_persisted_env_runtime_and_clear() {
    use crate::layers::LayerInput;

    let mut config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env: std::iter::once(("ANTHROPIC_MODEL".to_string(), "env-model".to_string()))
                .collect(),
            model: Some("persisted-model".to_string()),
            ..ClaudeSettings::default()
        },
    );
    config.resolved_settings = crate::ResolvedSettings::resolve(&[LayerInput::new(
        SettingOrigin::Project,
        Some(config.cwd.join(".claude/settings.json")),
        serde_json::json!({"model": "persisted-model"})
            .as_object()
            .expect("object")
            .clone(),
    )]);

    let env = config.effective_model_selection();
    assert_eq!(env.persisted.value.as_deref(), Some("persisted-model"));
    assert_eq!(env.persisted.source, Some(SettingOrigin::Project));
    assert_eq!(env.source, ModelSelectionSource::Environment);
    assert_eq!(env.requested_model.as_deref(), Some("env-model"));
    assert_eq!(env.runtime_override, RuntimeModelOverride::Inherit);

    config.apply_runtime_model_override(Some("sonnet"));
    let runtime = config.effective_model_selection();
    assert_eq!(runtime.source, ModelSelectionSource::Runtime);
    assert_eq!(runtime.requested_model.as_deref(), Some("sonnet"));
    assert_eq!(
        runtime.runtime_override,
        RuntimeModelOverride::Model("sonnet".to_string())
    );

    config.apply_runtime_model_override(None);
    let provider_default = config.effective_model_selection();
    assert_eq!(provider_default.source, ModelSelectionSource::Runtime);
    assert_eq!(provider_default.requested_model, None);
    assert_eq!(
        provider_default.runtime_override,
        RuntimeModelOverride::Default
    );

    config.clear_runtime_model_override();
    let cleared = config.effective_model_selection();
    assert_eq!(cleared.source, ModelSelectionSource::Environment);
    assert_eq!(cleared.requested_model.as_deref(), Some("env-model"));
    assert_eq!(cleared.runtime_override, RuntimeModelOverride::Inherit);
}

#[test]
fn effective_model_selection_reports_persisted_source_and_managed_lock() {
    use crate::layers::LayerInput;

    let mut config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            model: Some("opus".to_string()),
            ..ClaudeSettings::default()
        },
    );
    config.resolved_settings = crate::ResolvedSettings::resolve(&[LayerInput::new(
        SettingOrigin::Managed,
        Some(config.home_dir.join("managed-settings.json")),
        serde_json::json!({"model": "opus"})
            .as_object()
            .expect("object")
            .clone(),
    )]);
    config
        .policy
        .managed_locked_keys
        .insert("model".to_string());

    let selection = config.effective_model_selection();
    assert_eq!(selection.source, ModelSelectionSource::Persisted);
    assert_eq!(selection.requested_model.as_deref(), Some("opus"));
    assert_eq!(selection.persisted.source, Some(SettingOrigin::Managed));
    assert!(selection.persisted.locked);
}

#[test]
fn openai_settings_env_resolves_base_url_api_key_and_model() {
    let config = test_config(
        ProviderId::OpenAi,
        ClaudeSettings {
            env: [
                (
                    "OPENAI_BASE_URL".to_string(),
                    "http://localhost:11434/v1".to_string(),
                ),
                ("OPENAI_API_KEY".to_string(), "local-key".to_string()),
                ("OPENAI_MODEL".to_string(), "qwen-coder".to_string()),
            ]
            .into_iter()
            .collect(),
            ..ClaudeSettings::default()
        },
    );

    assert_eq!(config.openai_base_url(), "http://localhost:11434/v1");
    assert_eq!(config.openai_api_key().as_deref(), Some("local-key"));
    assert_eq!(
        config.provider_model_setting().as_deref(),
        Some("qwen-coder")
    );
    assert_eq!(config.provider_model_name(ProviderId::OpenAi), "qwen-coder");
}

#[test]
fn provider_resolution_exposes_display_and_capability_metadata() {
    let config = test_config(
        ProviderId::OpenAi,
        ClaudeSettings {
            model: Some("opus".to_string()),
            env: [
                (
                    "OPENAI_DEFAULT_OPUS_MODEL".to_string(),
                    "o3-proxy".to_string(),
                ),
                (
                    "OPENAI_DEFAULT_OPUS_MODEL_NAME".to_string(),
                    "Reasoning Opus".to_string(),
                ),
                (
                    "OPENAI_DEFAULT_OPUS_MODEL_DESCRIPTION".to_string(),
                    "Custom reasoning model".to_string(),
                ),
                (
                    "OPENAI_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES".to_string(),
                    "effort,thinking".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            ..ClaudeSettings::default()
        },
    );

    let resolution = config.provider_model_resolution(ProviderId::OpenAi);
    assert_eq!(resolution.request_model, "o3-proxy");
    assert_eq!(resolution.display_label, "Reasoning Opus");
    assert_eq!(resolution.display_name, "Reasoning Opus");
    assert_eq!(
        resolution.capabilities,
        vec![ModelCapability::Effort, ModelCapability::Thinking]
    );

    let opus = config
        .model_options()
        .into_iter()
        .find(|option| option.value.as_deref() == Some("opus"))
        .expect("opus option");
    assert_eq!(opus.label, "Reasoning Opus");
    assert_eq!(opus.description, "Custom reasoning model");
    assert!(opus.current);
}

#[test]
fn small_fast_model_uses_provider_env_then_haiku_default() {
    let config = test_config(
        ProviderId::OpenAi,
        ClaudeSettings {
            env: [
                (
                    "OPENAI_SMALL_FAST_MODEL".to_string(),
                    "gpt-fast".to_string(),
                ),
                (
                    "OPENAI_DEFAULT_HAIKU_MODEL".to_string(),
                    "gpt-haiku".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            ..ClaudeSettings::default()
        },
    );

    assert_eq!(config.small_fast_model_name(ProviderId::OpenAi), "gpt-fast");

    let config = test_config(
        ProviderId::OpenAi,
        ClaudeSettings {
            env: std::iter::once((
                "OPENAI_DEFAULT_HAIKU_MODEL".to_string(),
                "gpt-haiku".to_string(),
            ))
            .collect(),
            ..ClaudeSettings::default()
        },
    );

    assert_eq!(
        config.small_fast_model_name(ProviderId::OpenAi),
        "gpt-haiku"
    );
}

#[test]
fn anthropic_oauth_token_prefers_managed_env_over_credentials() {
    let mut config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env: std::iter::once((
                "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
                "managed-env-token".to_string(),
            ))
            .collect(),
            ..ClaudeSettings::default()
        },
    );
    let home = tempfile::tempdir().expect("home");
    config.home_dir = home.path().to_path_buf();

    std::fs::create_dir_all(&config.home_dir).expect("home");
    std::fs::write(
        config.home_dir.join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"stored-token","expiresAt":null,"scopes":["user:inference"]}}"#,
    )
    .expect("credentials");

    assert_eq!(
        config.anthropic_oauth_token().as_deref(),
        Some("managed-env-token")
    );
}

#[test]
fn anthropic_oauth_token_ignores_expired_credentials() {
    let mut config = test_config(ProviderId::Anthropic, ClaudeSettings::default());
    let home = tempfile::tempdir().expect("home");
    config.home_dir = home.path().to_path_buf();
    std::fs::create_dir_all(&config.home_dir).expect("home");
    std::fs::write(
        config.home_dir.join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"stored-token","expiresAt":1,"scopes":["user:inference"]}}"#,
    )
    .expect("credentials");

    assert_eq!(config.anthropic_oauth_token(), None);
}

#[tokio::test]
async fn anthropic_api_key_uses_stored_login_after_env() {
    let mut config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env: std::iter::once(("ANTHROPIC_API_KEY".to_string(), "env-api-key".to_string()))
                .collect(),
            ..ClaudeSettings::default()
        },
    );
    let home = tempfile::tempdir().expect("home");
    config.home_dir = home.path().to_path_buf();
    AuthManager::new(config.home_dir.clone())
        .login(
            ProviderId::Anthropic,
            AuthMethod::ApiKey,
            Some("stored-api-key".to_string()),
            None,
        )
        .await
        .expect("login");

    assert_eq!(config.anthropic_api_key().as_deref(), Some("env-api-key"));

    config
        .env_overrides
        .insert("ANTHROPIC_API_KEY".to_string(), String::new());
    config.settings.env.remove("ANTHROPIC_API_KEY");

    assert_eq!(
        config.anthropic_api_key().as_deref(),
        Some("stored-api-key")
    );
}

#[tokio::test]
async fn anthropic_oauth_token_uses_stored_login_after_managed_sources() {
    let mut config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env: std::iter::once((
                "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
                "managed-oauth".to_string(),
            ))
            .collect(),
            ..ClaudeSettings::default()
        },
    );
    let home = tempfile::tempdir().expect("home");
    config.home_dir = home.path().to_path_buf();
    AuthManager::new(config.home_dir.clone())
        .login(
            ProviderId::Anthropic,
            AuthMethod::OAuthDevice,
            Some("stored-oauth".to_string()),
            None,
        )
        .await
        .expect("login");

    assert_eq!(
        config.anthropic_oauth_token().as_deref(),
        Some("managed-oauth")
    );

    config
        .env_overrides
        .insert("CLAUDE_CODE_OAUTH_TOKEN".to_string(), String::new());
    config.settings.env.remove("CLAUDE_CODE_OAUTH_TOKEN");

    assert_eq!(
        config.anthropic_oauth_token().as_deref(),
        Some("stored-oauth")
    );
}

#[test]
fn model_options_include_explicit_custom_option_env() {
    let config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env: [
                (
                    "ANTHROPIC_CUSTOM_MODEL_OPTION".to_string(),
                    "claude-custom".to_string(),
                ),
                (
                    "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME".to_string(),
                    "Custom Claude".to_string(),
                ),
                (
                    "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION".to_string(),
                    "Custom routed Claude model".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            model: Some("claude-custom".to_string()),
            ..ClaudeSettings::default()
        },
    );

    let custom_options = config
        .model_options()
        .into_iter()
        .filter(|option| option.value.as_deref() == Some("claude-custom"))
        .collect::<Vec<_>>();

    assert_eq!(custom_options.len(), 1);
    assert_eq!(custom_options[0].label, "Custom Claude");
    assert_eq!(custom_options[0].description, "Custom routed Claude model");
    assert!(custom_options[0].current);
}

#[test]
fn model_options_use_known_model_labels_for_current_custom_model() {
    let config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            model: Some("claude-sonnet-4-5-20250929".to_string()),
            ..ClaudeSettings::default()
        },
    );

    let custom_option = config
        .model_options()
        .into_iter()
        .find(|option| option.value.as_deref() == Some("claude-sonnet-4-5-20250929"))
        .expect("current custom model option");

    assert_eq!(custom_option.label, "Sonnet 4.5");
    assert_eq!(
        custom_option.description,
        "Newer version available - select Sonnet for Sonnet 4.6"
    );
    assert!(custom_option.current);
}

#[test]
fn model_options_filtered_by_available_models_policy() {
    let mut config = test_config(ProviderId::Anthropic, ClaudeSettings::default());
    config.policy.available_models = Some(vec!["opus".to_string(), "sonnet".to_string()]);

    let options = config.model_options();
    assert!(options.iter().any(|o| o.value.is_none()));
    assert!(options.iter().any(|o| o.value.as_deref() == Some("opus")));
    assert!(options.iter().any(|o| o.value.as_deref() == Some("sonnet")));
    assert!(!options.iter().any(|o| o.value.as_deref() == Some("haiku")));
}

#[test]
fn token_accounting_options_read_settings_env() {
    let config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env: [
                (
                    "CLAUDE_CODE_MAX_CONTEXT_TOKENS".to_string(),
                    "123456".to_string(),
                ),
                (
                    "CLAUDE_CODE_AUTO_COMPACT_WINDOW".to_string(),
                    "100000".to_string(),
                ),
                (
                    "CLAUDE_CODE_MAX_OUTPUT_TOKENS".to_string(),
                    "8192".to_string(),
                ),
                (
                    "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE".to_string(),
                    "50".to_string(),
                ),
                ("DISABLE_AUTO_COMPACT".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            ..ClaudeSettings::default()
        },
    );

    assert_eq!(
        config.context_window_options().max_context_tokens_override,
        Some(123_456)
    );
    assert_eq!(
        config.context_window_options().auto_compact_window_override,
        Some(100_000)
    );
    assert_eq!(
        config.max_output_token_options().max_output_tokens_override,
        Some(8_192)
    );
    assert!(!config.token_warning_options().auto_compact_enabled);
    assert_eq!(
        config.token_warning_options().auto_compact_percent_override,
        Some(50)
    );
}

#[tokio::test]
async fn load_honors_settings_env_provider_type() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"PROVIDER_TYPE":"openai"}}"#,
    )
    .expect("settings");

    let config = AppConfig::load(
        workspace.path(),
        AppConfigOverrides {
            home_dir: Some(home.path().to_path_buf()),
            // Seal provider/model process variables before load so the
            // settings-level PROVIDER_TYPE assertion is deterministic.
            env_overrides: sealed_provider_env_overrides(),
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("load config");

    assert_eq!(config.default_provider, ProviderId::OpenAi);
    assert_eq!(config.model_display_name(), "Sonnet");
    assert_eq!(config.provider_model_name(ProviderId::OpenAi), "gpt-4o");
}

#[tokio::test]
async fn load_merges_settings_and_cli_additional_directories() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let settings_dir = workspace.path().join("settings-extra");
    let cli_dir = workspace.path().join("cli-extra");
    std::fs::create_dir_all(&settings_dir).expect("settings extra");
    std::fs::create_dir_all(&cli_dir).expect("cli extra");
    std::fs::write(
        home.path().join("settings.json"),
        r#"{"permissions":{"additionalDirectories":["settings-extra","missing-extra"]}}"#,
    )
    .expect("settings");

    let config = AppConfig::load(
        workspace.path(),
        AppConfigOverrides {
            home_dir: Some(home.path().to_path_buf()),
            add_dirs: vec![cli_dir.clone(), settings_dir.clone()],
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("load config");

    let settings_dir = std::fs::canonicalize(settings_dir).expect("canonical settings");
    let cli_dir = std::fs::canonicalize(cli_dir).expect("canonical cli");
    assert_eq!(config.additional_directories, vec![settings_dir, cli_dir]);
}

#[tokio::test]
async fn load_merges_settings_permission_rules_with_cli_rules() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        home.path().join("settings.json"),
        r#"{
                "permissions": {
                    "allow": ["Bash(cargo test:*)"],
                    "deny": ["Bash(rm:*)"]
                }
            }"#,
    )
    .expect("settings");

    let config = AppConfig::load(
        workspace.path(),
        AppConfigOverrides {
            home_dir: Some(home.path().to_path_buf()),
            allowed_tools: vec!["Read(src/**)".to_string()],
            disallowed_tools: vec!["Bash(git clean:*)".to_string()],
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("load config");

    assert_eq!(
        config.allowed_tools,
        vec!["Bash(cargo test:*)".to_string(), "Read(src/**)".to_string()]
    );
    assert_eq!(
        config.disallowed_tools,
        vec!["Bash(rm:*)".to_string(), "Bash(git clean:*)".to_string()]
    );
}

#[tokio::test]
async fn load_appends_project_local_hooks_after_home_hooks() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(workspace.path().join(".claude")).expect("local settings dir");
    std::fs::write(
        home.path().join("settings.json"),
        r#"{
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [
                                {
                                    "type": "command",
                                    "command": "printf home"
                                }
                            ]
                        }
                    ]
                }
            }"#,
    )
    .expect("home settings");
    std::fs::write(
        workspace.path().join(".claude/settings.local.json"),
        r#"{
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Read",
                            "hooks": [
                                {
                                    "type": "command",
                                    "command": "printf local"
                                }
                            ]
                        }
                    ]
                }
            }"#,
    )
    .expect("local settings");

    let config = AppConfig::load(
        workspace.path(),
        AppConfigOverrides {
            home_dir: Some(home.path().to_path_buf()),
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("load config");

    let hooks = config
        .settings
        .hooks
        .get("PreToolUse")
        .expect("PreToolUse hooks");
    assert_eq!(hooks.len(), 2);
    assert_eq!(hooks[0].matcher.as_deref(), Some("Bash"));
    assert_eq!(hooks[1].matcher.as_deref(), Some("Read"));
    let HookCommand::Command { command, .. } = &hooks[1].hooks[0] else {
        panic!("expected local command hook");
    };
    assert_eq!(command, "printf local");
}

#[tokio::test]
async fn load_merges_project_local_settings_after_home_settings() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(workspace.path().join(".claude")).expect("local settings dir");
    std::fs::write(
        home.path().join("settings.json"),
        r#"{
                "env": {
                    "ANTHROPIC_MODEL": "home-model",
                    "HOME_ONLY": "home"
                },
                "model": "home-top-level",
                "theme": "dark",
                "editorMode": "normal",
                "alwaysThinkingEnabled": false,
                "permissions": {
                    "allow": ["Bash(home:*)"],
                    "deny": ["Bash(home-deny:*)"],
                    "additionalDirectories": ["/home-extra"]
                }
            }"#,
    )
    .expect("home settings");
    std::fs::write(
        workspace.path().join(".claude/settings.local.json"),
        r#"{
                "env": {
                    "ANTHROPIC_MODEL": "local-model",
                    "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE": "1"
                },
                "model": "local-top-level",
                "theme": "light",
                "editorMode": "vim",
                "alwaysThinkingEnabled": true,
                "permissions": {
                    "allow": ["Read(local/**)"],
                    "deny": ["Bash(local-deny:*)"],
                    "additionalDirectories": ["/local-extra"]
                }
            }"#,
    )
    .expect("local settings");

    let config = AppConfig::load(
        workspace.path(),
        AppConfigOverrides {
            home_dir: Some(home.path().to_path_buf()),
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("load config");

    assert_eq!(
        config
            .settings
            .env
            .get("ANTHROPIC_MODEL")
            .map(String::as_str),
        Some("local-model")
    );
    assert_eq!(
        config.settings.env.get("HOME_ONLY").map(String::as_str),
        Some("home")
    );
    assert_eq!(
        config
            .settings
            .env
            .get("CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(config.settings.model.as_deref(), Some("local-top-level"));
    assert_eq!(config.settings.theme, ThemeSetting::Light);
    assert_eq!(config.settings.editor_mode, EditorModeSetting::Vim);
    assert_eq!(config.settings.always_thinking_enabled, Some(true));
    assert_eq!(
        config.allowed_tools,
        vec!["Bash(home:*)".to_string(), "Read(local/**)".to_string()]
    );
    assert_eq!(
        config.disallowed_tools,
        vec![
            "Bash(home-deny:*)".to_string(),
            "Bash(local-deny:*)".to_string()
        ]
    );
    assert_eq!(
        config.settings.additional_directories,
        vec!["/home-extra".to_string(), "/local-extra".to_string()]
    );
}

#[tokio::test]
async fn load_merges_project_settings_between_user_and_local() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(workspace.path().join(".claude")).expect("project settings dir");
    std::fs::write(
        home.path().join("settings.json"),
        r#"{
                "model": "home-model",
                "permissions": {
                    "allow": ["Bash(home:*)"],
                    "deny": ["Bash(home-deny:*)"],
                    "ask": ["Bash(home-ask:*)"]
                }
            }"#,
    )
    .expect("home settings");
    std::fs::write(
        workspace.path().join(".claude/settings.json"),
        r#"{
                "model": "project-model",
                "permissions": {
                    "allow": ["Read(project/**)"],
                    "deny": ["Bash(project-deny:*)"],
                    "ask": ["Bash(project-ask:*)"]
                }
            }"#,
    )
    .expect("project settings");
    std::fs::write(
        workspace.path().join(".claude/settings.local.json"),
        r#"{
                "model": "local-model",
                "permissions": {
                    "allow": ["Read(local/**)"]
                }
            }"#,
    )
    .expect("local settings");

    let config = AppConfig::load(
        workspace.path(),
        AppConfigOverrides {
            home_dir: Some(home.path().to_path_buf()),
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("load config");

    // Local wins the scalar override; project sits above user.
    assert_eq!(config.settings.model.as_deref(), Some("local-model"));
    // Project-layer permission rules now take runtime effect (previously dropped).
    assert_eq!(
        config.allowed_tools,
        vec![
            "Bash(home:*)".to_string(),
            "Read(project/**)".to_string(),
            "Read(local/**)".to_string(),
        ]
    );
    assert_eq!(
        config.disallowed_tools,
        vec![
            "Bash(home-deny:*)".to_string(),
            "Bash(project-deny:*)".to_string()
        ]
    );
    // `ask` rules are now deserialized from every layer.
    assert_eq!(
        config.ask_tools,
        vec![
            "Bash(home-ask:*)".to_string(),
            "Bash(project-ask:*)".to_string()
        ]
    );
}

#[test]
fn extra_body_parses_object_and_ignores_garbage() {
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "CLAUDE_CODE_EXTRA_BODY".to_string(),
        r#"{"output_config":{"verbosity":"low"},"flag":true}"#.to_string(),
    );
    let config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env,
            ..ClaudeSettings::default()
        },
    );
    let body = config.extra_body();
    assert_eq!(body["flag"], serde_json::json!(true));
    assert_eq!(body["output_config"]["verbosity"], serde_json::json!("low"));

    let mut env = std::collections::BTreeMap::new();
    env.insert("CLAUDE_CODE_EXTRA_BODY".to_string(), "not-json".to_string());
    let config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env,
            ..ClaudeSettings::default()
        },
    );
    assert!(config.extra_body().is_empty());
}

#[test]
fn anthropic_betas_splits_on_separators() {
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "CLAUDE_CODE_BETAS".to_string(),
        "context-1m-2025-01-14, prompt-caching-2024-07-31\nrandom-flag".to_string(),
    );
    let config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env,
            ..ClaudeSettings::default()
        },
    );
    assert_eq!(
        config.anthropic_betas(),
        vec![
            "context-1m-2025-01-14".to_string(),
            "prompt-caching-2024-07-31".to_string(),
            "random-flag".to_string(),
        ]
    );
}

#[test]
fn custom_headers_parses_one_per_line() {
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "ANTHROPIC_CUSTOM_HEADERS".to_string(),
        "X-One: alpha\n  X-Two : beta\nignored-line-without-colon\nX-Three: a:b:c".to_string(),
    );
    let config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env,
            ..ClaudeSettings::default()
        },
    );
    assert_eq!(
        config.custom_headers(),
        vec![
            ("X-One".to_string(), "alpha".to_string()),
            ("X-Two".to_string(), "beta".to_string()),
            ("X-Three".to_string(), "a:b:c".to_string()),
        ]
    );
}

#[test]
fn api_timeout_parses_milliseconds() {
    let mut env = std::collections::BTreeMap::new();
    env.insert("API_TIMEOUT_MS".to_string(), "5000".to_string());
    let config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env,
            ..ClaudeSettings::default()
        },
    );
    assert_eq!(
        config.api_timeout(),
        Some(std::time::Duration::from_millis(5000))
    );
}

#[test]
fn anthropic_metadata_includes_session_id_in_user_id_envelope() {
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "CLAUDE_CODE_EXTRA_METADATA".to_string(),
        r#"{"device_id":"dev-1"}"#.to_string(),
    );
    let config = test_config(
        ProviderId::Anthropic,
        ClaudeSettings {
            env,
            ..ClaudeSettings::default()
        },
    );
    let metadata = config.anthropic_metadata("session-xyz");
    let user_id = metadata["user_id"].as_str().expect("user_id is a string");
    let parsed: serde_json::Value = serde_json::from_str(user_id).expect("user_id is JSON-encoded");
    assert_eq!(parsed["device_id"], serde_json::json!("dev-1"));
    assert_eq!(parsed["session_id"], serde_json::json!("session-xyz"));
}

#[test]
fn provider_disabled_diagnostic_returns_none_for_active_providers() {
    let config = test_config(ProviderId::Anthropic, ClaudeSettings::default());
    assert!(config.provider_disabled_diagnostic().is_none());

    let config = test_config(ProviderId::OpenAi, ClaudeSettings::default());
    assert!(config.provider_disabled_diagnostic().is_none());
}

#[test]
fn provider_disabled_diagnostic_returns_message_for_stub_providers() {
    let config = test_config(ProviderId::Gemini, ClaudeSettings::default());
    let diagnostic = config
        .provider_disabled_diagnostic()
        .expect("gemini should have diagnostic");
    assert!(diagnostic.contains("gemini"));
    assert!(diagnostic.contains("not supported"));

    let config = test_config(ProviderId::Grok, ClaudeSettings::default());
    let diagnostic = config
        .provider_disabled_diagnostic()
        .expect("grok should have diagnostic");
    assert!(diagnostic.contains("grok"));
    assert!(diagnostic.contains("not supported"));
}

#[test]
fn sealed_overrides_resolve_canonical_settings_env_via_legacy_key() {
    let mut settings = ClaudeSettings::default();
    settings.env.insert(
        "ORBCODE_ANTHROPIC_MODEL".to_string(),
        "model-from-canonical".to_string(),
    );
    let config = test_config(ProviderId::Anthropic, settings);
    let result = config.resolve_env("ANTHROPIC_MODEL");
    assert_eq!(
        result.as_deref(),
        Some("model-from-canonical"),
        "sealed env_overrides + canonical settings.env must be reachable via legacy lookup"
    );
}

#[test]
fn sealed_overrides_resolve_legacy_settings_env_via_canonical_key() {
    let mut settings = ClaudeSettings::default();
    settings.env.insert(
        "ANTHROPIC_MODEL".to_string(),
        "model-from-legacy".to_string(),
    );
    let config = test_config(ProviderId::Anthropic, settings);
    let result = config.resolve_env("ORBCODE_ANTHROPIC_MODEL");
    assert_eq!(
        result.as_deref(),
        Some("model-from-legacy"),
        "sealed env_overrides + legacy settings.env must be reachable via canonical lookup"
    );
}

#[test]
fn sealed_override_does_not_shadow_nonempty_override_in_same_alias_group() {
    let mut config = test_config(ProviderId::Anthropic, ClaudeSettings::default());
    // Simulate the common fixture pattern: seal everything, then set a
    // specific legacy key to a mock value (e.g. mock base URL).
    config
        .env_overrides
        .insert("ANTHROPIC_BASE_URL".to_string(), "mock://test".to_string());
    // ORBCODE_ANTHROPIC_BASE_URL is already sealed (empty) from
    // sealed_provider_env_overrides(). The non-empty legacy override
    // must win instead of being shadowed by the empty canonical seal.
    let result = config.resolve_env("ANTHROPIC_BASE_URL");
    assert_eq!(
        result.as_deref(),
        Some("mock://test"),
        "non-empty legacy override must not be shadowed by empty canonical seal"
    );
    let result_via_canonical = config.resolve_env("ORBCODE_ANTHROPIC_BASE_URL");
    assert_eq!(
        result_via_canonical.as_deref(),
        Some("mock://test"),
        "non-empty legacy override must be visible via canonical key too"
    );
}

// ── hooks_for_event / hook_sources_for_event accessor tests ───────────

#[test]
fn hooks_for_event_returns_empty_for_missing_event() {
    let config = test_config(ProviderId::Anthropic, ClaudeSettings::default());
    assert!(config.hooks_for_event("NonExistent").is_empty());
}

#[test]
fn hook_sources_for_event_returns_empty_for_missing_event() {
    let config = test_config(ProviderId::Anthropic, ClaudeSettings::default());
    assert!(config.hook_sources_for_event("NonExistent").is_empty());
}

#[test]
fn hooks_for_event_returns_matchers_for_registered_event() {
    let mut settings = ClaudeSettings::default();
    settings.hooks.insert(
        "PreToolUse".to_string(),
        vec![crate::hooks::HookMatcher {
            matcher: Some("bash".to_string()),
            hooks: vec![HookCommand::Command {
                command: "echo pre".to_string(),
                r#if: None,
                timeout: None,
            }],
        }],
    );
    let config = test_config(ProviderId::Anthropic, settings);
    let matchers = config.hooks_for_event("PreToolUse");
    assert_eq!(matchers.len(), 1);
    assert_eq!(matchers[0].matcher.as_deref(), Some("bash"));
}

#[test]
fn hook_sources_for_event_matches_hooks_registration() {
    use crate::claude_home::HookSource;
    let mut settings = ClaudeSettings::default();
    settings.hooks.insert(
        "PostToolUse".to_string(),
        vec![crate::hooks::HookMatcher {
            matcher: Some("grep".to_string()),
            hooks: vec![HookCommand::Command {
                command: "echo post".to_string(),
                r#if: None,
                timeout: None,
            }],
        }],
    );
    settings
        .hook_sources
        .insert("PostToolUse".to_string(), vec![HookSource::LocalSettings]);
    let config = test_config(ProviderId::Anthropic, settings);
    let sources = config.hook_sources_for_event("PostToolUse");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0], HookSource::LocalSettings);
}
