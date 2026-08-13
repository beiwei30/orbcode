use orbcode_app_server_protocol::{
    ActiveOutputStyleResult, EditorModeParams, EditorModeResult, EffortParams, EffortResult,
    EnabledParams, EnabledResult, KeybindingsFileResult, KeybindingsLoadResult, ModelNameResult,
    ModelOptionOverview, ModelOptionsResult, OutputStyleOptionOverview, OutputStyleOptionsResult,
    OutputStyleParams, OutputStyleResult, PathResult, ProviderResolutionOverview, ProvidersResult,
    ResponseResult, SandboxExcludedCommandParams, SandboxExcludedCommandResult,
    SandboxSettingsUpdate, SessionIdParams, SetModelParams, SetModelResult,
    SetThinkingBudgetParams, SettingKeyParams, SettingLockedResult, StringPathParams, ThemeParams,
    ThemeResult,
};
use serde_json::Value;

use super::{core_error, success, try_parse};
use crate::AppServer;
use crate::protocol_conversion::{
    editor_mode_setting_from_wire, editor_mode_setting_to_wire, effective_model_selection_to_wire,
    sandbox_local_settings_to_wire, sandbox_settings_update_from_wire, theme_setting_from_wire,
    theme_setting_to_wire,
};

impl AppServer {
    pub(super) fn handle_settings_model_name(&self, _params: Option<Value>) -> ResponseResult {
        success(ModelNameResult {
            model_name: self.model_name(),
        })
    }

    pub(super) fn handle_settings_model_options(&self, _params: Option<Value>) -> ResponseResult {
        let options = self
            .model_options()
            .into_iter()
            .map(|option| ModelOptionOverview {
                value: option.value,
                label: option.label,
                description: option.description,
                current: option.current,
            })
            .collect();
        success(ModelOptionsResult(options))
    }

    pub(super) async fn handle_settings_set_model(&self, params: Option<Value>) -> ResponseResult {
        let p: SetModelParams = try_parse!(params);
        match self.set_persisted_model_setting(p.model).await {
            Ok(display_name) => success(SetModelResult {
                display_name,
                model_selection: effective_model_selection_to_wire(
                    self.sessions.effective_config().effective_model_selection(),
                ),
            }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_set_thinking_budget(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: SetThinkingBudgetParams = try_parse!(params);
        match self
            .set_max_thinking_tokens(&p.session_id, p.max_thinking_tokens)
            .await
        {
            Ok(result) => success(result),
            Err(e) => core_error(e),
        }
    }

    pub(super) fn handle_settings_providers(&self, _params: Option<Value>) -> ResponseResult {
        let default_provider = self.default_provider();
        let fallback_provider = self.fallback_provider();
        let max_retries = self.max_retries();
        let resolutions = self
            .provider_model_resolutions()
            .into_iter()
            .map(|resolution| ProviderResolutionOverview {
                provider: resolution.provider.as_str().to_string(),
                model: resolution.model,
                request_model: resolution.request_model,
                display_name: resolution.display_name,
                capabilities: resolution.capabilities,
            })
            .collect();
        success(ProvidersResult {
            default_provider: default_provider.as_str().to_string(),
            fallback_provider: fallback_provider.map(|provider| provider.as_str().to_string()),
            max_retries,
            resolutions,
        })
    }

    pub(super) fn handle_settings_theme(&self, _params: Option<Value>) -> ResponseResult {
        success(ThemeResult {
            theme: theme_setting_to_wire(self.theme_setting()),
        })
    }

    pub(super) async fn handle_settings_set_theme(&self, params: Option<Value>) -> ResponseResult {
        let p: ThemeParams = try_parse!(params);
        let theme = theme_setting_from_wire(p.theme);
        match self.set_theme_setting(theme).await {
            Ok(theme) => success(ThemeResult {
                theme: theme_setting_to_wire(theme),
            }),
            Err(e) => core_error(e),
        }
    }

    pub(super) fn handle_settings_effort(&self, _params: Option<Value>) -> ResponseResult {
        success(EffortResult {
            effort: self.effort_level(),
        })
    }

    pub(super) async fn handle_settings_set_effort(&self, params: Option<Value>) -> ResponseResult {
        let p: EffortParams = try_parse!(params);
        match self.set_effort_override(&p.session_id, p.effort).await {
            Ok(effort) => success(EffortResult { effort }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_output_style(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        match self.output_style_setting().await {
            Ok(style) => success(OutputStyleResult { style }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_set_output_style(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: OutputStyleParams = try_parse!(params);
        match self.set_output_style_setting(&p.style).await {
            Ok(style) => success(OutputStyleResult { style }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_sandbox(&self, _params: Option<Value>) -> ResponseResult {
        match self.sandbox_local_settings().await {
            Ok(settings) => success(sandbox_local_settings_to_wire(settings)),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_update_sandbox(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let update: SandboxSettingsUpdate = try_parse!(params);
        match self
            .update_sandbox_settings(sandbox_settings_update_from_wire(update))
            .await
        {
            Ok(path) => success(PathResult { path }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_keybindings(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        match self.ensure_keybindings_file().await {
            Ok(file) => success(KeybindingsFileResult {
                path: file.path,
                created: file.created,
            }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_context_preview(&self, _params: Option<Value>) -> ResponseResult {
        let context = self.context_preview().await;
        success(context)
    }

    pub(super) async fn handle_context_overview(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        match self.context_overview(&p.session_id).await {
            Ok(overview) => success(overview),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_usage_overview(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        match self.usage_overview(&p.session_id).await {
            Ok(usage) => success(usage),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_usage_cost(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        match self.cost_overview(&p.session_id).await {
            Ok(cost) => success(cost),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_usage_stats(&self, _params: Option<Value>) -> ResponseResult {
        match self.stats_overview().await {
            Ok(stats) => success(stats),
            Err(e) => core_error(e),
        }
    }

    pub(super) fn handle_settings_load_keybindings(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        let kb = self.load_keybindings();
        success(KeybindingsLoadResult {
            warnings: kb.warnings().to_vec(),
        })
    }

    pub(super) fn handle_settings_editor_mode(&self, _params: Option<Value>) -> ResponseResult {
        let mode = self.editor_mode_setting();
        success(EditorModeResult {
            editor_mode: editor_mode_setting_to_wire(mode),
        })
    }

    pub(super) async fn handle_settings_set_editor_mode(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: EditorModeParams = try_parse!(params);
        let mode = editor_mode_setting_from_wire(p.mode);
        match self.set_editor_mode_setting(mode).await {
            Ok(mode) => success(EditorModeResult {
                editor_mode: editor_mode_setting_to_wire(mode),
            }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_output_style_options(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        match self.output_style_options().await {
            Ok(options) => {
                let options = options
                    .into_iter()
                    .map(|option| OutputStyleOptionOverview {
                        value: option.value,
                        label: option.label,
                        description: option.description,
                        current: option.current,
                    })
                    .collect();
                success(OutputStyleOptionsResult(options))
            }
            Err(e) => core_error(e),
        }
    }

    pub(super) fn handle_settings_active_output_style(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        success(ActiveOutputStyleResult {
            name: self.active_output_style_name(),
            matched: self.active_output_style_matched(),
        })
    }

    pub(super) fn handle_settings_is_locked(&self, params: Option<Value>) -> ResponseResult {
        let p: SettingKeyParams = try_parse!(params);
        let locked = self.is_setting_locked(&p.key);
        success(SettingLockedResult { locked })
    }

    pub(super) async fn handle_settings_set_auto_memory(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: EnabledParams = try_parse!(params);
        match self.set_auto_memory_enabled(p.enabled).await {
            Ok(()) => success(EnabledResult { enabled: p.enabled }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_ensure_memory_file(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: StringPathParams = try_parse!(params);
        match self
            .ensure_memory_file(std::path::PathBuf::from(&p.path))
            .await
        {
            Ok(()) => success(PathResult {
                path: std::path::PathBuf::from(p.path),
            }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_add_sandbox_excluded(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: SandboxExcludedCommandParams = try_parse!(params);
        match self.add_sandbox_excluded_command(&p.command).await {
            Ok(command) => success(SandboxExcludedCommandResult {
                pattern: command.pattern,
                path: command.path,
            }),
            Err(e) => core_error(e),
        }
    }
}
