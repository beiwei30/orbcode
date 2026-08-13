use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};

use orbcode_protocol::EffortLevel;

use crate::{
    AppConfig, EditorModeSetting, PersistedModelSetting, SettingOrigin, ThemeSetting,
    claude_home::sanitize_path,
};

/// Effective process-wide client preferences. The current runtime owner is the
/// shared `RuntimeSessionState`, so these values intentionally apply to every
/// client/session in this process; they are not session control overrides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientPreferences {
    pub theme: ThemeSetting,
    pub editor_mode: EditorModeSetting,
}

/// Session-local model selection. This is deliberately distinct from the
/// layered persisted `model` setting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RuntimeModelOverride {
    /// Follow environment and persisted settings.
    #[default]
    Inherit,
    /// Explicitly select the provider default, bypassing configured model ids.
    Default,
    /// Use the requested model/family id for this runtime scope.
    Model(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum RefreshedPersistedModelSetting {
    #[default]
    Unchanged,
    Value(Option<String>),
}

impl RuntimeModelOverride {
    pub fn from_model(model: Option<String>) -> Self {
        match model
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty())
        {
            Some(model) => Self::Model(model),
            None => Self::Default,
        }
    }

    pub fn is_inherit(&self) -> bool {
        matches!(self, Self::Inherit)
    }

    pub fn requested_model(&self) -> Option<&str> {
        match self {
            Self::Model(model) => Some(model),
            Self::Inherit | Self::Default => None,
        }
    }
}

#[derive(Clone, Default)]
pub struct RuntimeSessionState {
    cwd_override: Arc<RwLock<Option<PathBuf>>>,
    additional_directories: Arc<RwLock<Vec<PathBuf>>>,
    model_override: Arc<RwLock<RuntimeModelOverride>>,
    refreshed_persisted_model_setting: Arc<RwLock<RefreshedPersistedModelSetting>>,
    theme_override: Arc<RwLock<Option<ThemeSetting>>>,
    editor_mode_override: Arc<RwLock<Option<EditorModeSetting>>>,
    effort_override: Arc<RwLock<Option<EffortLevel>>>,
    max_thinking_tokens: Arc<RwLock<Option<u32>>>,
}

impl RuntimeSessionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn effective_config(&self, config: &AppConfig) -> AppConfig {
        let mut config = config.clone();
        if let Some(cwd) = self.cwd_override() {
            config.cwd = cwd;
            config.current_project_dir = config
                .projects_dir
                .join(sanitize_path(&config.cwd.display().to_string()));
        }
        if let RefreshedPersistedModelSetting::Value(value) = self.refreshed_persisted_model() {
            config.settings.model = value.clone();
            config.refreshed_persisted_model_setting = Some(PersistedModelSetting {
                value,
                source: Some(SettingOrigin::User),
                locked: config.ensure_setting_mutable("model").is_err(),
            });
        }
        let model_override = self.model_override();
        if !model_override.is_inherit() {
            config.apply_runtime_model_override(model_override.requested_model());
        }
        config
    }

    pub fn cwd_override(&self) -> Option<PathBuf> {
        match self.cwd_override.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn set_cwd_override(&self, cwd: Option<PathBuf>) {
        let mut override_guard = match self.cwd_override.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *override_guard = cwd;
    }

    pub fn model_override(&self) -> RuntimeModelOverride {
        match self.model_override.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn set_model_override(&self, model: Option<String>) {
        let mut override_guard = match self.model_override.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *override_guard = RuntimeModelOverride::from_model(model);
    }

    /// Remove the runtime override and resume layered env/persisted selection.
    /// This is deliberately different from `set_model_override(None)`, which
    /// explicitly selects the provider default.
    pub fn clear_model_override(&self) {
        let mut override_guard = match self.model_override.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *override_guard = RuntimeModelOverride::Inherit;
    }

    fn refreshed_persisted_model(&self) -> RefreshedPersistedModelSetting {
        match self.refreshed_persisted_model_setting.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Refresh immutable base config after a successful atomic settings write
    /// without conflating the persisted value with a runtime override.
    pub fn refresh_persisted_model_setting(&self, model: Option<String>) {
        let mut setting = match self.refreshed_persisted_model_setting.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *setting = RefreshedPersistedModelSetting::Value(model);
    }

    pub fn theme_setting(&self, config: &AppConfig) -> ThemeSetting {
        match self.theme_override.read() {
            Ok(guard) => guard.unwrap_or(config.settings.theme),
            Err(poisoned) => poisoned.into_inner().unwrap_or(config.settings.theme),
        }
    }

    pub fn client_preferences(&self, config: &AppConfig) -> ClientPreferences {
        ClientPreferences {
            theme: self.theme_setting(config),
            editor_mode: self.editor_mode_setting(config),
        }
    }

    pub fn set_theme_setting(&self, theme: ThemeSetting) {
        let mut override_guard = match self.theme_override.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *override_guard = Some(theme);
    }

    pub fn editor_mode_setting(&self, config: &AppConfig) -> EditorModeSetting {
        match self.editor_mode_override.read() {
            Ok(guard) => guard.unwrap_or(config.settings.editor_mode),
            Err(poisoned) => poisoned.into_inner().unwrap_or(config.settings.editor_mode),
        }
    }

    pub fn set_editor_mode_setting(&self, mode: EditorModeSetting) {
        let mut override_guard = match self.editor_mode_override.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *override_guard = Some(mode);
    }

    pub fn effort_override(&self) -> Option<EffortLevel> {
        match self.effort_override.read() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    pub fn set_effort_override(&self, effort: Option<EffortLevel>) {
        let mut override_guard = match self.effort_override.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *override_guard = effort;
    }

    pub fn max_thinking_tokens(&self) -> Option<u32> {
        match self.max_thinking_tokens.read() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    pub fn set_max_thinking_tokens(&self, max_thinking_tokens: Option<u32>) {
        let mut override_guard = match self.max_thinking_tokens.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *override_guard = max_thinking_tokens;
    }

    pub fn additional_directories(&self, config: &AppConfig) -> Vec<PathBuf> {
        let mut directories = config.additional_directories.clone();
        let runtime = match self.additional_directories.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for directory in runtime.iter() {
            if !directories.iter().any(|existing| existing == directory) {
                directories.push(directory.clone());
            }
        }
        directories
    }

    pub fn runtime_additional_directories(&self) -> Vec<PathBuf> {
        match self.additional_directories.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn set_runtime_additional_directories(&self, directories: Vec<PathBuf>) {
        let mut runtime = match self.additional_directories.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *runtime = directories;
    }

    pub fn add_runtime_additional_directory(&self, config: &AppConfig, directory: PathBuf) -> bool {
        let mut runtime = match self.additional_directories.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if config
            .additional_directories
            .iter()
            .chain(runtime.iter())
            .any(|existing| existing == &directory)
        {
            return false;
        }
        runtime.push(directory);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use orbcode_protocol::{ProviderId, SandboxMode};

    use super::*;
    use crate::{ClaudeSettings, EffectivePolicy, SettingsLayers};

    fn config() -> AppConfig {
        let root = PathBuf::from("/tmp/orbcode-runtime-state");
        AppConfig {
            cwd: root.clone(),
            home_dir: root.clone(),
            sessions_dir: root.join("sessions"),
            projects_dir: root.join("projects"),
            current_project_dir: root.join("projects/current"),
            history_path: root.join("history.jsonl"),
            settings_path: root.join("settings.json"),
            default_provider: ProviderId::Anthropic,
            fallback_provider: None,
            max_retries: 2,
            sandbox_mode: SandboxMode::WorkspaceWrite,
            sandbox_allow_network: false,
            allow_network: true,
            provider_allow_network: true,
            allow_tools: false,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            ask_tools: Vec::new(),
            additional_directories: vec![root.join("configured")],
            mcp_config_inputs: Vec::new(),
            settings: ClaudeSettings::default(),
            settings_layers: SettingsLayers::default(),
            resolved_settings: Default::default(),
            settings_warnings: Vec::new(),
            policy: EffectivePolicy::default(),
            policy_conflicts: Vec::new(),
            runtime_model_override: RuntimeModelOverride::Inherit,
            refreshed_persisted_model_setting: None,
            env_overrides: std::collections::HashMap::new(),
            append_system_prompt: None,
            permission_mode: None,
            explicit_permission_overrides: Default::default(),
            trusted_project: true,
        }
    }

    #[test]
    fn runtime_model_override_updates_effective_config() {
        let runtime = RuntimeSessionState::new();
        let config = config();

        runtime.set_model_override(Some("sonnet".to_string()));
        assert_eq!(
            runtime.effective_config(&config).runtime_model_override,
            RuntimeModelOverride::Model("sonnet".to_string())
        );

        runtime.set_model_override(None);
        assert_eq!(
            runtime.effective_config(&config).runtime_model_override,
            RuntimeModelOverride::Default
        );
    }

    #[test]
    fn runtime_cwd_override_updates_effective_config_project_dir() {
        let runtime = RuntimeSessionState::new();
        let config = config();
        let resumed = config.home_dir.join("resumed project");

        runtime.set_cwd_override(Some(resumed.clone()));

        let effective = runtime.effective_config(&config);
        assert_eq!(effective.cwd, resumed);
        assert_eq!(
            effective.current_project_dir,
            effective
                .projects_dir
                .join(crate::sanitize_path(&effective.cwd.display().to_string()))
        );
    }

    #[test]
    fn runtime_additional_directories_are_deduplicated_after_configured_dirs() {
        let runtime = RuntimeSessionState::new();
        let config = config();
        let configured = config.additional_directories[0].clone();
        let session = config.cwd.join("session");

        assert!(!runtime.add_runtime_additional_directory(&config, configured.clone()));
        assert!(runtime.add_runtime_additional_directory(&config, session.clone()));
        assert!(!runtime.add_runtime_additional_directory(&config, session.clone()));

        assert_eq!(
            runtime.additional_directories(&config),
            vec![configured, session]
        );
    }
}
