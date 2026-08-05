use std::path::PathBuf;

use orbcode_app_server_protocol::{ModelChangeResult, ThinkingBudgetResult};
use orbcode_config::{
    EditorModeSetting, ModelOption, OutputStyleOption, ProviderModelResolution,
    ResolvedKeybindings, SandboxLocalSettings, SandboxSettingsUpdate, ThemeSetting,
    add_sandbox_excluded_command, load_keybindings, load_output_style_setting,
    load_sandbox_local_settings, model_capabilities, output_style_options,
    update_output_style_setting, update_sandbox_settings,
};
use orbcode_core::{CoreError, ProviderDescriptor};
use orbcode_protocol::{EffortLevel, ProviderId};

use super::AppServer;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelResolutionOverview {
    pub provider: ProviderId,
    pub model: String,
    pub request_model: String,
    pub display_name: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeybindingsFile {
    pub path: PathBuf,
    pub created: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxExcludedCommand {
    pub pattern: String,
    pub path: PathBuf,
}

pub(crate) static AUTO_MEMORY_OVERRIDE: std::sync::atomic::AtomicI8 =
    std::sync::atomic::AtomicI8::new(-1);

const KEYBINDINGS_TEMPLATE: &str = r#"{
  "$schema": "https://www.schemastore.org/claude-code-keybindings.json",
  "$docs": "https://code.claude.com/docs/en/keybindings",
  "bindings": [
    {
      "context": "Global",
      "bindings": {
        "ctrl+l": "app:redraw",
        "ctrl+t": "app:toggleTodos",
        "ctrl+o": "app:toggleTranscript",
        "ctrl+j": "app:toggleBackgroundJobs",
        "ctrl+r": "history:search"
      }
    },
    {
      "context": "Chat",
      "bindings": {
        "escape": "chat:cancel",
        "ctrl+x ctrl+k": "chat:killAgents",
        "shift+tab": "chat:cycleMode",
        "meta+p": "chat:modelPicker",
        "meta+o": "chat:fastMode",
        "meta+t": "chat:thinkingToggle",
        "enter": "chat:submit",
        "up": "history:previous",
        "down": "history:next",
        "ctrl+_": "chat:undo",
        "ctrl+shift+-": "chat:undo",
        "ctrl+x ctrl+e": "chat:externalEditor",
        "ctrl+g": "chat:externalEditor",
        "ctrl+s": "chat:stash",
        "ctrl+v": "chat:imagePaste"
      }
    },
    {
      "context": "Autocomplete",
      "bindings": {
        "tab": "autocomplete:accept",
        "escape": "autocomplete:dismiss",
        "up": "autocomplete:previous",
        "down": "autocomplete:next"
      }
    },
    {
      "context": "Settings",
      "bindings": {
        "escape": "confirm:no",
        "up": "select:previous",
        "down": "select:next",
        "k": "select:previous",
        "j": "select:next",
        "ctrl+p": "select:previous",
        "ctrl+n": "select:next",
        "space": "select:accept",
        "enter": "settings:close",
        "/": "settings:search",
        "r": "settings:retry"
      }
    },
    {
      "context": "Confirmation",
      "bindings": {
        "y": "confirm:yes",
        "n": "confirm:no",
        "enter": "confirm:yes",
        "escape": "confirm:no",
        "up": "confirm:previous",
        "down": "confirm:next",
        "tab": "confirm:nextField",
        "space": "confirm:toggle",
        "shift+tab": "confirm:cycleMode",
        "ctrl+e": "confirm:toggleExplanation",
        "ctrl+d": "permission:toggleDebug"
      }
    },
    {
      "context": "ThemePicker",
      "bindings": {
        "ctrl+t": "theme:toggleSyntaxHighlighting"
      }
    },
    {
      "context": "Transcript",
      "bindings": {
        "ctrl+e": "transcript:toggleShowAll",
        "escape": "transcript:exit",
        "q": "transcript:exit"
      }
    },
    {
      "context": "Select",
      "bindings": {
        "up": "select:previous",
        "down": "select:next",
        "j": "select:next",
        "k": "select:previous",
        "ctrl+n": "select:next",
        "ctrl+p": "select:previous",
        "enter": "select:accept",
        "escape": "select:cancel"
      }
    },
    {
      "context": "DiffDialog",
      "bindings": {
        "escape": "diff:dismiss",
        "left": "diff:previousSource",
        "right": "diff:nextSource",
        "up": "diff:previousFile",
        "down": "diff:nextFile",
        "enter": "diff:viewDetails"
      }
    },
    {
      "context": "ModelPicker",
      "bindings": {
        "left": "modelPicker:decreaseEffort",
        "right": "modelPicker:increaseEffort"
      }
    },
    {
      "context": "MessageSelector",
      "bindings": {
        "up": "messageSelector:up",
        "down": "messageSelector:down",
        "k": "messageSelector:up",
        "j": "messageSelector:down",
        "ctrl+p": "messageSelector:up",
        "ctrl+n": "messageSelector:down",
        "enter": "messageSelector:select"
      }
    }
  ]
}
"#;

impl AppServer {
    /// Reject a settings mutation when the managed policy locks the target key.
    /// The error message names the locked key and states it is managed-locked.
    pub(crate) fn ensure_setting_mutable(&self, key: &str) -> Result<(), CoreError> {
        self.sessions
            .config()
            .ensure_setting_mutable(key)
            .map_err(|error| CoreError::PermissionDenied(error.message))
    }

    pub fn is_setting_locked(&self, key: &str) -> bool {
        self.sessions.config().ensure_setting_mutable(key).is_err()
    }

    pub fn supported_providers(&self) -> Vec<ProviderDescriptor> {
        self.sessions.supported_providers()
    }

    pub fn default_provider(&self) -> ProviderId {
        self.sessions.config().default_provider
    }

    pub fn fallback_provider(&self) -> Option<ProviderId> {
        self.sessions.config().fallback_provider
    }

    pub fn max_retries(&self) -> usize {
        self.sessions.config().max_retries
    }

    pub fn model_options(&self) -> Vec<ModelOption> {
        self.sessions.model_options()
    }

    pub fn model_name(&self) -> String {
        let config = self.sessions.effective_config();
        config.provider_model_name(config.default_provider)
    }

    pub fn provider_model_resolutions(&self) -> Vec<ModelResolutionOverview> {
        let config = self.sessions.effective_config();
        self.supported_providers()
            .into_iter()
            .map(|provider| {
                model_resolution_overview(config.provider_model_resolution(provider.id))
            })
            .collect()
    }

    pub async fn set_model_override(&self, model: Option<String>) -> Result<String, CoreError> {
        self.ensure_setting_mutable("model")?;
        let model = self.validate_model_override(model)?;
        self.sessions.set_model_override(model).await?;
        Ok(self.sessions.model_display_name())
    }

    fn validate_model_override(&self, model: Option<String>) -> Result<Option<String>, CoreError> {
        let Some(model) = model else {
            return Ok(None);
        };
        let trimmed = model.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
            return Ok(None);
        }
        if trimmed.chars().any(char::is_control) {
            return Err(CoreError::Config(
                "model name must not contain control characters".to_string(),
            ));
        }
        let config = self.sessions.effective_config();
        if !config.default_provider.is_active() {
            return Err(CoreError::Config(format!(
                "provider '{}' cannot serve model requests",
                config.default_provider
            )));
        }
        if !config.policy.model_allowed(trimmed) {
            return Err(CoreError::PermissionDenied(format!(
                "model `{trimmed}` is not allowed by managed policy"
            )));
        }
        let resolved = config.resolve_model_setting(config.default_provider, Some(trimmed));
        if resolved.trim().is_empty() {
            return Err(CoreError::Config(format!(
                "model `{trimmed}` resolved to an empty provider model"
            )));
        }
        Ok(Some(trimmed.to_string()))
    }

    /// Change the model through the same validated resolver used by the TUI,
    /// returning the effective state observed by the next provider request.
    pub async fn set_session_model_override(
        &self,
        session_id: &str,
        model: Option<String>,
    ) -> Result<ModelChangeResult, CoreError> {
        self.ensure_active_session(session_id)?;
        self.set_model_override(model).await?;
        let config = self.sessions.effective_config();
        let resolution = config.provider_model_resolution(config.default_provider);
        Ok(ModelChangeResult {
            provider: config.default_provider,
            model: resolution.request_model,
            display_name: resolution.display_name,
        })
    }

    /// Set or clear a numeric thinking budget for subsequent provider requests.
    /// A request already being streamed owns an immutable ProviderRequest and is
    /// therefore unaffected.
    pub async fn set_max_thinking_tokens(
        &self,
        session_id: &str,
        max_thinking_tokens: Option<u32>,
    ) -> Result<ThinkingBudgetResult, CoreError> {
        self.ensure_active_session(session_id)?;
        if let Some(value) = max_thinking_tokens {
            let config = self.sessions.effective_config();
            if config.default_provider != ProviderId::Anthropic {
                return Err(CoreError::Config(format!(
                    "provider '{}' does not support a numeric thinking-token budget",
                    config.default_provider
                )));
            }
            let resolution = config.provider_model_resolution(config.default_provider);
            let capabilities =
                model_capabilities(&resolution.request_model, config.default_provider);
            if !capabilities.supports_thinking {
                return Err(CoreError::Config(format!(
                    "model '{}' does not support extended thinking",
                    resolution.request_model
                )));
            }
            let max = capabilities
                .max_output_tokens_upper_limit
                .saturating_sub(4_096);
            if !(1_024..=max).contains(&value) {
                return Err(CoreError::Config(format!(
                    "max_thinking_tokens must be between 1024 and {max} for model '{}'",
                    resolution.request_model
                )));
            }
        }
        self.sessions.set_max_thinking_tokens(max_thinking_tokens);
        Ok(ThinkingBudgetResult {
            session_id: session_id.to_string(),
            max_thinking_tokens: self.sessions.max_thinking_tokens(),
        })
    }

    pub fn editor_mode_setting(&self) -> EditorModeSetting {
        self.sessions.editor_mode_setting()
    }

    pub fn theme_setting(&self) -> ThemeSetting {
        self.sessions.theme_setting()
    }

    pub async fn set_theme_setting(&self, theme: ThemeSetting) -> Result<ThemeSetting, CoreError> {
        self.ensure_setting_mutable("theme")?;
        self.sessions.set_theme_setting(theme).await?;
        Ok(self.sessions.theme_setting())
    }

    pub async fn set_editor_mode_setting(
        &self,
        mode: EditorModeSetting,
    ) -> Result<EditorModeSetting, CoreError> {
        self.ensure_setting_mutable("editorMode")?;
        self.sessions.set_editor_mode_setting(mode).await?;
        Ok(self.sessions.editor_mode_setting())
    }

    pub fn effort_level(&self) -> Option<EffortLevel> {
        self.sessions.runtime_effort_override()
    }

    pub async fn set_effort_override(
        &self,
        session_id: &str,
        effort: Option<EffortLevel>,
    ) -> Result<Option<EffortLevel>, CoreError> {
        self.sessions
            .set_effort_override_for_session(session_id, effort)
            .await?;
        Ok(self.sessions.runtime_effort_override())
    }

    pub async fn set_auto_memory_enabled(&self, enabled: bool) -> Result<(), CoreError> {
        let home_dir = &self.sessions.effective_config().home_dir;
        orbcode_config::update_auto_memory_setting(home_dir, enabled)
            .await
            .map_err(|e| CoreError::Config(e.to_string()))?;
        AUTO_MEMORY_OVERRIDE.store(
            if enabled { 1 } else { 0 },
            std::sync::atomic::Ordering::Relaxed,
        );
        Ok(())
    }

    pub async fn output_style_setting(&self) -> Result<String, CoreError> {
        let config = self.sessions.effective_config();
        Ok(load_output_style_setting(&config.home_dir, &config.cwd).await?)
    }

    pub async fn output_style_options(&self) -> Result<Vec<OutputStyleOption>, CoreError> {
        let config = self.sessions.effective_config();
        Ok(output_style_options(&config.home_dir, &config.cwd).await?)
    }

    pub async fn set_output_style_setting(&self, style: &str) -> Result<String, CoreError> {
        self.ensure_setting_mutable("outputStyle")?;
        let config = self.sessions.effective_config();
        update_output_style_setting(&config.cwd, style).await?;
        self.sessions.refresh_output_styles().await;
        self.output_style_setting().await
    }

    pub fn active_output_style_name(&self) -> String {
        self.sessions.active_output_style().definition.name.clone()
    }

    pub fn active_output_style_matched(&self) -> bool {
        self.sessions.active_output_style().matched
    }

    pub async fn refresh_output_styles(&self) {
        self.sessions.refresh_output_styles().await;
    }

    pub async fn ensure_keybindings_file(&self) -> Result<KeybindingsFile, CoreError> {
        let path = self.sessions.config().home_dir.join("keybindings.json");
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                use tokio::io::AsyncWriteExt;
                file.write_all(KEYBINDINGS_TEMPLATE.as_bytes()).await?;
                file.flush().await?;
                Ok(KeybindingsFile {
                    path,
                    created: true,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok(KeybindingsFile {
                    path,
                    created: false,
                })
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn add_sandbox_excluded_command(
        &self,
        pattern: &str,
    ) -> Result<SandboxExcludedCommand, CoreError> {
        self.ensure_setting_mutable("sandbox")?;
        let config = self.sessions.effective_config();
        let path = add_sandbox_excluded_command(&config.cwd, pattern).await?;
        Ok(SandboxExcludedCommand {
            pattern: pattern.to_string(),
            path,
        })
    }

    pub fn load_keybindings(&self) -> ResolvedKeybindings {
        load_keybindings(&self.sessions.config().home_dir)
    }

    pub async fn sandbox_local_settings(&self) -> Result<SandboxLocalSettings, CoreError> {
        let config = self.sessions.effective_config();
        Ok(load_sandbox_local_settings(&config.cwd).await?)
    }

    pub async fn update_sandbox_settings(
        &self,
        update: SandboxSettingsUpdate,
    ) -> Result<PathBuf, CoreError> {
        self.ensure_setting_mutable("sandbox")?;
        let config = self.sessions.effective_config();
        Ok(update_sandbox_settings(&config.cwd, update).await?)
    }
}

fn model_resolution_overview(resolution: ProviderModelResolution) -> ModelResolutionOverview {
    let capabilities = capability_names(&resolution);
    ModelResolutionOverview {
        provider: resolution.provider,
        model: resolution.model,
        request_model: resolution.request_model,
        display_name: resolution.display_name,
        capabilities,
    }
}

pub(crate) fn capability_names(resolution: &ProviderModelResolution) -> Vec<String> {
    resolution
        .capabilities
        .iter()
        .map(|capability| capability.as_str().to_string())
        .collect()
}

pub(crate) fn auto_memory_enabled() -> bool {
    let override_val = AUTO_MEMORY_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
    if override_val >= 0 {
        return override_val != 0;
    }
    !env_truthy("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
        && !env_truthy("CLAUDE_CODE_SIMPLE")
        && (!env_truthy("CLAUDE_CODE_REMOTE")
            || std::env::var_os("CLAUDE_CODE_REMOTE_MEMORY_DIR").is_some())
}

fn env_truthy(key: &str) -> bool {
    std::env::var(key).is_ok_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use orbcode_config::AppConfigOverrides;
    use orbcode_protocol::{EffortLevel, ProviderId};

    use super::super::AppServer;

    fn test_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "orbcode-app-server-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    async fn write_text(path: &std::path::Path, contents: &str) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .expect("create parent dir");
        }
        tokio::fs::write(path, contents).await.expect("write file");
    }

    #[tokio::test]
    async fn runtime_model_override_updates_status_and_options() {
        let home = test_path("model-home");
        let cwd = test_path("model-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::write(
            home.join("settings.json"),
            r#"{"env":{"ANTHROPIC_MODEL":"glm-4.7","ANTHROPIC_SMALL_FAST_MODEL":"glm-4.7","ANTHROPIC_DEFAULT_HAIKU_MODEL":"glm-4.7","ANTHROPIC_DEFAULT_SONNET_MODEL":"glm-4.7","ANTHROPIC_DEFAULT_SONNET_MODEL_SUPPORTED_CAPABILITIES":"thinking,interleaved_thinking","ANTHROPIC_DEFAULT_OPUS_MODEL":"glm-4.7"}}"#,
        )
        .await
        .expect("settings");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home.clone()),
                env_overrides: orbcode_config::sealed_provider_env_overrides(),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let initial_options = app.model_options();
        assert_eq!(initial_options.len(), 5);
        assert_eq!(initial_options[0].value, None);
        assert_eq!(initial_options[0].label, "Default (recommended)");
        assert_eq!(
            initial_options[0].description,
            "Use the default model (currently glm-4.7)"
        );
        assert!(!initial_options[0].current);
        assert_eq!(initial_options[1].value.as_deref(), Some("sonnet"));
        assert_eq!(initial_options[1].label, "glm-4.7");
        assert_eq!(initial_options[1].description, "Custom Sonnet model");
        assert_eq!(initial_options[2].value.as_deref(), Some("opus"));
        assert_eq!(initial_options[2].label, "glm-4.7");
        assert_eq!(initial_options[2].description, "Custom Opus model");
        assert_eq!(initial_options[3].value.as_deref(), Some("haiku"));
        assert_eq!(initial_options[3].label, "glm-4.7");
        assert_eq!(initial_options[3].description, "Custom Haiku model");
        assert_eq!(initial_options[4].value.as_deref(), Some("glm-4.7"));
        assert_eq!(initial_options[4].label, "glm-4.7");
        assert_eq!(initial_options[4].description, "Custom model");
        assert!(initial_options[4].current);

        let display = app
            .set_model_override(Some("sonnet".to_string()))
            .await
            .expect("set model");
        assert_eq!(display, "glm-4.7");
        let settings = tokio::fs::read_to_string(home.join("settings.json"))
            .await
            .expect("read settings");
        assert!(settings.contains(r#""model": "sonnet""#), "{settings}");

        let overview = app.status_overview("session").await.expect("status");
        assert_eq!(overview.model_display_name, "glm-4.7");
        assert_eq!(overview.model_name, "glm-4.7");
        assert_eq!(
            overview.model_capabilities,
            vec!["thinking".to_string(), "interleaved_thinking".to_string()]
        );
        assert_eq!(overview.small_fast_model_display_name, "glm-4.7");
        assert!(app.provider_model_resolutions().iter().any(|resolution| {
            resolution.provider == ProviderId::Anthropic
                && resolution.request_model == "glm-4.7"
                && resolution.capabilities == overview.model_capabilities
        }));
        assert!(
            app.model_options()
                .iter()
                .any(|option| option.value.as_deref() == Some("sonnet") && option.current)
        );
        assert!(
            app.model_options()
                .iter()
                .any(|option| option.value.as_deref() == Some("glm-4.7") && !option.current)
        );
        assert!(
            app.model_options()
                .iter()
                .any(|option| option.value.as_deref() == Some("opus") && !option.current)
        );

        let display = app.set_model_override(None).await.expect("clear model");
        assert_eq!(display, "glm-4.7");
        let settings = tokio::fs::read_to_string(home.join("settings.json"))
            .await
            .expect("read settings");
        assert!(!settings.contains(r#""model""#), "{settings}");
        let default_options = app.model_options();
        assert!(default_options[0].current);
        assert!(
            default_options
                .iter()
                .any(|option| option.value.as_deref() == Some("glm-4.7") && !option.current)
        );
    }

    #[tokio::test]
    async fn runtime_effort_override_updates_status() {
        let home = test_path("effort-home");
        let cwd = test_path("effort-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        assert_eq!(app.effort_level(), None);
        assert_eq!(
            app.set_effort_override("session", Some(EffortLevel::High))
                .await
                .expect("set effort"),
            Some(EffortLevel::High)
        );
        assert_eq!(
            app.status_overview("session")
                .await
                .expect("status")
                .effort_level,
            Some(EffortLevel::High)
        );
        assert_eq!(
            app.set_effort_override("session", None)
                .await
                .expect("clear effort"),
            None
        );
    }

    #[tokio::test]
    async fn output_style_options_use_canonical_definition_names() {
        let home = test_path("output-style-home");
        let cwd = test_path("output-style-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        write_text(
            &cwd.join(".claude").join("output-styles").join("stem.md"),
            "---\nname: Canonical Name\ndescription: canonical option\n---\nbody",
        )
        .await;

        let app = AppServer::new(
            cwd.clone(),
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let options = app.output_style_options().await.expect("output styles");
        assert!(!options.iter().any(|option| option.value == "stem"));
        assert!(options.iter().any(|option| option.value == "Canonical Name"
            && option.label == "Canonical Name"
            && option.description == "canonical option"));

        let stored = app
            .set_output_style_setting("Canonical Name")
            .await
            .expect("set output style");
        assert_eq!(stored, "Canonical Name");
        assert_eq!(app.active_output_style_name(), "Canonical Name");

        let settings = tokio::fs::read_to_string(cwd.join(".claude/settings.local.json"))
            .await
            .expect("read local settings");
        assert!(settings.contains(r#""outputStyle": "Canonical Name""#));
    }
}
