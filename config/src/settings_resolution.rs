use std::collections::HashMap;
use std::env;

use orbcode_protocol::ProviderId;

use crate::ConfigError;
use crate::claude_home::{ClaudeSettings, resolve_env_value};
use crate::config::PermissionMode;
use crate::layers::{LayerInput, ResolvedSettings, SettingOrigin, SettingWarning};
use crate::policy::{SettingsLayers, SettingsSource};

/// Map permission mode to the default `allow_tools` override it implies.
pub(crate) fn permission_mode_default_allow_tools(mode: Option<PermissionMode>) -> Option<bool> {
    match mode? {
        PermissionMode::Default => None,
        PermissionMode::AcceptEdits
        | PermissionMode::BypassPermissions
        | PermissionMode::DontAsk
        | PermissionMode::Auto => Some(true),
        PermissionMode::Plan => Some(false),
    }
}

/// Map permission mode to the default `allow_network` override it implies.
pub(crate) fn permission_mode_default_allow_network(mode: Option<PermissionMode>) -> Option<bool> {
    match mode? {
        PermissionMode::BypassPermissions | PermissionMode::DontAsk => Some(true),
        PermissionMode::Default
        | PermissionMode::AcceptEdits
        | PermissionMode::Plan
        | PermissionMode::Auto => None,
    }
}

/// Merge a raw inline JSON settings overlay (`--settings`) into `ClaudeSettings`.
pub(crate) fn apply_inline_settings_overlay(
    settings: &mut ClaudeSettings,
    raw: &str,
) -> Result<(), ConfigError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|error| ConfigError::Config(format!("--settings is not valid JSON: {error}")))?;
    let Some(object) = value.as_object() else {
        return Err(ConfigError::Config(
            "--settings JSON must be an object".to_string(),
        ));
    };
    if let Some(model) = object.get("model").and_then(|value| value.as_str()) {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            settings.model = Some(trimmed.to_string());
        }
    }
    if let Some(env) = object.get("env").and_then(|value| value.as_object()) {
        for (key, value) in env {
            if let Some(value) = value.as_str() {
                settings.env.insert(key.clone(), value.to_string());
            }
        }
    }
    if let Some(allowed) = object
        .get("allowedTools")
        .or_else(|| object.get("allowed_tools"))
        .and_then(|value| value.as_array())
    {
        extend_unique_str(&mut settings.allowed_tools, allowed);
    }
    if let Some(disallowed) = object
        .get("disallowedTools")
        .or_else(|| object.get("disallowed_tools"))
        .and_then(|value| value.as_array())
    {
        extend_unique_str(&mut settings.disallowed_tools, disallowed);
    }
    if let Some(ask) = object
        .get("askTools")
        .or_else(|| object.get("ask_tools"))
        .and_then(|value| value.as_array())
    {
        extend_unique_str(&mut settings.ask_tools, ask);
    }
    if let Some(dirs) = object
        .get("additionalDirectories")
        .or_else(|| object.get("additional_directories"))
        .and_then(|value| value.as_array())
    {
        extend_unique_str(&mut settings.additional_directories, dirs);
    }
    if let Some(max_budget_usd) = object
        .get("maxBudgetUsd")
        .or_else(|| object.get("max_budget_usd"))
        .and_then(serde_json::Value::as_f64)
    {
        settings.max_budget_usd = Some(max_budget_usd);
    }
    if let Some(strict) = object
        .get("maxBudgetUsdStrictUnknownPricing")
        .or_else(|| object.get("max_budget_usd_strict_unknown_pricing"))
        .and_then(serde_json::Value::as_bool)
    {
        settings.max_budget_strict_unknown_pricing = Some(strict);
    }
    Ok(())
}

fn extend_unique_str(target: &mut Vec<String>, source: &[serde_json::Value]) {
    for entry in source {
        if let Some(value) = entry.as_str() {
            let trimmed = value.trim();
            if !trimmed.is_empty() && !target.iter().any(|existing| existing == trimmed) {
                target.push(trimmed.to_string());
            }
        }
    }
}

/// Map a policy [`SettingsSource`] to its broader [`SettingOrigin`] layer.
fn origin_for_settings_source(source: SettingsSource) -> SettingOrigin {
    match source {
        SettingsSource::User => SettingOrigin::User,
        SettingsSource::Project => SettingOrigin::Project,
        SettingsSource::Local => SettingOrigin::Local,
        SettingsSource::Managed => SettingOrigin::Managed,
    }
}

/// Build the session-override layer from CLI/runtime overrides. Only keys that
/// map onto real settings fields are included so the resolver does not emit
/// spurious unknown-key warnings for runtime-only flags.
pub(crate) fn runtime_session_override_layer(
    permission_mode: Option<PermissionMode>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mode = permission_mode?;
    let mut object = serde_json::Map::new();
    object.insert(
        "defaultMode".to_string(),
        serde_json::Value::String(mode.as_str().to_string()),
    );
    Some(object)
}

/// Resolve the eight-layer precedence from the file-backed settings layers plus
/// an optional runtime session-override layer. Returns the resolved view and
/// its accumulated key warnings.
pub(crate) fn resolve_layered_settings(
    settings_layers: &SettingsLayers,
    session_override: Option<serde_json::Map<String, serde_json::Value>>,
) -> (ResolvedSettings, Vec<SettingWarning>) {
    let mut inputs: Vec<LayerInput> = Vec::new();
    for layer in &settings_layers.layers {
        let Some(raw) = layer.raw.as_ref() else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }
        inputs.push(LayerInput::new(
            origin_for_settings_source(layer.source),
            Some(layer.primary_path.clone()),
            raw.clone(),
        ));
    }
    if let Some(values) = session_override {
        inputs.push(LayerInput::new(
            SettingOrigin::SessionOverride,
            None,
            values,
        ));
    }
    let resolved = ResolvedSettings::resolve(&inputs);
    let warnings = resolved.warnings.clone();
    (resolved, warnings)
}

/// Resolve the default provider from overrides, env, and settings.
pub(crate) fn resolve_default_provider(
    override_provider: Option<ProviderId>,
    settings: &ClaudeSettings,
) -> ProviderId {
    override_provider
        .or_else(|| {
            env::var("ORBCODE_PROVIDER")
                .ok()
                .and_then(|value| ProviderId::parse(&value))
        })
        .or_else(|| {
            parse_bool_value(resolve_env_value(settings, "CLAUDE_CODE_USE_OPENAI"))
                .filter(|enabled| *enabled)
                .map(|_| ProviderId::OpenAi)
        })
        .unwrap_or(ProviderId::Anthropic)
}

/// Parse a boolean from an environment variable.
pub(crate) fn parse_bool_env(key: &str) -> Option<bool> {
    parse_bool_value(env::var(key).ok())
}

/// Parse a boolean from an optional string value.
pub(crate) fn parse_bool_value(value: Option<String>) -> Option<bool> {
    value.and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    })
}

/// Build a `resolve_env` seal that blocks the inherited shell from leaking
/// provider-related env vars into tests. Each entry maps a provider env key
/// to an empty string, which `resolve_env` treats as "explicitly cleared":
/// the matching `std::env::var` lookup is skipped, but `settings.env` can
/// still serve a value. Use this anywhere a fixture needs deterministic
/// model/provider resolution regardless of the developer's shell.
pub fn sealed_provider_env_overrides() -> HashMap<String, String> {
    let legacy_keys = [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_CUSTOM_MODEL_OPTION",
        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
        "OPENAI_BASE_URL",
        "OPENAI_API_KEY",
        "OPENAI_MODEL",
        "OPENAI_SMALL_FAST_MODEL",
        "OPENAI_DEFAULT_OPUS_MODEL",
        "OPENAI_DEFAULT_SONNET_MODEL",
        "OPENAI_DEFAULT_HAIKU_MODEL",
    ];
    legacy_keys
        .into_iter()
        .map(|key| (key.to_string(), String::new()))
        .chain(crate::env_compat::canonical_keys().map(|key| (key.to_string(), String::new())))
        .collect()
}

#[cfg(test)]
mod tests {
    use orbcode_protocol::ProviderId;

    use crate::claude_home::ClaudeSettings;
    use crate::config::PermissionMode;

    use super::{
        apply_inline_settings_overlay, permission_mode_default_allow_network,
        permission_mode_default_allow_tools,
    };

    #[test]
    fn apply_inline_settings_overlay_merges_model_and_tool_lists() {
        let mut settings = ClaudeSettings::default();
        settings.allowed_tools.push("Read".to_string());
        apply_inline_settings_overlay(
            &mut settings,
            r#"{"model":"sonnet","allowedTools":["Bash"],"disallowedTools":["Write"],"env":{"FOO":"bar"}}"#,
        )
        .expect("overlay");
        assert_eq!(settings.model.as_deref(), Some("sonnet"));
        assert_eq!(
            settings.allowed_tools,
            vec!["Read".to_string(), "Bash".to_string()]
        );
        assert_eq!(settings.disallowed_tools, vec!["Write".to_string()]);
        assert_eq!(settings.env.get("FOO").map(String::as_str), Some("bar"));
    }

    #[test]
    fn apply_inline_settings_overlay_rejects_non_object() {
        let mut settings = ClaudeSettings::default();
        assert!(apply_inline_settings_overlay(&mut settings, "[1,2,3]").is_err());
    }

    #[test]
    fn apply_inline_settings_overlay_parses_budget_fields() {
        let mut settings = ClaudeSettings::default();
        apply_inline_settings_overlay(
            &mut settings,
            r#"{"maxBudgetUsd": 12.5, "maxBudgetUsdStrictUnknownPricing": true}"#,
        )
        .expect("overlay");
        assert_eq!(settings.max_budget_usd, Some(12.5));
        assert_eq!(settings.max_budget_strict_unknown_pricing, Some(true));
    }

    #[test]
    fn permission_mode_defaults_map_to_allow_tools_and_network() {
        assert_eq!(
            permission_mode_default_allow_tools(Some(PermissionMode::BypassPermissions)),
            Some(true)
        );
        assert_eq!(
            permission_mode_default_allow_network(Some(PermissionMode::BypassPermissions)),
            Some(true)
        );
        assert_eq!(
            permission_mode_default_allow_tools(Some(PermissionMode::Plan)),
            Some(false)
        );
        assert_eq!(
            permission_mode_default_allow_network(Some(PermissionMode::Plan)),
            None
        );
        assert_eq!(
            permission_mode_default_allow_tools(Some(PermissionMode::Default)),
            None
        );
        assert_eq!(
            permission_mode_default_allow_tools(Some(PermissionMode::AcceptEdits)),
            Some(true)
        );
    }

    #[test]
    fn resolve_default_provider_respects_openai_env_flag() {
        let mut settings = ClaudeSettings::default();
        settings
            .env
            .insert("CLAUDE_CODE_USE_OPENAI".to_string(), "true".to_string());
        let provider = super::resolve_default_provider(None, &settings);
        assert_eq!(provider, ProviderId::OpenAi);
    }
}
