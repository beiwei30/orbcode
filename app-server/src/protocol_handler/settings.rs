use orbcode_app_server_protocol::ResponseResult;
use orbcode_config::ThemeSetting;
use orbcode_protocol::EffortLevel;
use serde::Deserialize;
use serde_json::Value;

use super::{core_error, success, try_parse};
use crate::AppServer;

impl AppServer {
    pub(super) fn handle_settings_model_name(&self, _params: Option<Value>) -> ResponseResult {
        success(serde_json::json!({ "model_name": self.model_name() }))
    }

    pub(super) fn handle_settings_model_options(&self, _params: Option<Value>) -> ResponseResult {
        let options: Vec<Value> = self
            .model_options()
            .into_iter()
            .map(|o| {
                serde_json::json!({
                    "value": o.value,
                    "label": o.label,
                    "description": o.description,
                    "current": o.current,
                })
            })
            .collect();
        success(options)
    }

    pub(super) async fn handle_settings_set_model(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            model: Option<String>,
        }
        let p: Params = try_parse!(params);
        match self.set_model_override(p.model).await {
            Ok(display_name) => success(serde_json::json!({ "display_name": display_name })),
            Err(e) => core_error(e),
        }
    }

    pub(super) fn handle_settings_providers(&self, _params: Option<Value>) -> ResponseResult {
        let default_provider = self.default_provider();
        let fallback_provider = self.fallback_provider();
        let max_retries = self.max_retries();
        let resolutions: Vec<Value> = self
            .provider_model_resolutions()
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    // Use as_str() so the wire representation matches the
                    // canonical form ("openai") rather than serde's snake_case
                    // ("open_ai").
                    "provider": r.provider.as_str(),
                    "model": r.model,
                    "request_model": r.request_model,
                    "display_name": r.display_name,
                    "capabilities": r.capabilities,
                })
            })
            .collect();
        success(serde_json::json!({
            "default_provider": default_provider.as_str(),
            "fallback_provider": fallback_provider.map(orbcode_protocol::ProviderId::as_str),
            "max_retries": max_retries,
            "resolutions": resolutions,
        }))
    }

    pub(super) fn handle_settings_theme(&self, _params: Option<Value>) -> ResponseResult {
        success(serde_json::json!({
            "theme": self.theme_setting().as_str()
        }))
    }

    pub(super) async fn handle_settings_set_theme(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            theme: String,
        }
        let p: Params = try_parse!(params);
        let Some(theme) = ThemeSetting::parse(&p.theme) else {
            return super::invalid_params(format!("unknown theme: {}", p.theme));
        };
        match self.set_theme_setting(theme).await {
            Ok(t) => success(serde_json::json!({ "theme": t.as_str() })),
            Err(e) => core_error(e),
        }
    }

    pub(super) fn handle_settings_effort(&self, _params: Option<Value>) -> ResponseResult {
        success(serde_json::json!({
            "effort": self.effort_level()
        }))
    }

    pub(super) async fn handle_settings_set_effort(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
            effort: Option<EffortLevel>,
        }
        let p: Params = try_parse!(params);
        match self.set_effort_override(&p.session_id, p.effort).await {
            Ok(effort) => success(serde_json::json!({ "effort": effort })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_output_style(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        match self.output_style_setting().await {
            Ok(style) => success(serde_json::json!({ "style": style })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_set_output_style(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            style: String,
        }
        let p: Params = try_parse!(params);
        match self.set_output_style_setting(&p.style).await {
            Ok(style) => success(serde_json::json!({ "style": style })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_sandbox(&self, _params: Option<Value>) -> ResponseResult {
        match self.sandbox_local_settings().await {
            Ok(settings) => success(settings),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_update_sandbox(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            enabled: Option<bool>,
            auto_allow_bash_if_sandboxed: Option<bool>,
            allow_unsandboxed_commands: Option<bool>,
        }
        let p: Params = try_parse!(params);
        let update = orbcode_config::SandboxSettingsUpdate {
            enabled: p.enabled,
            auto_allow_bash_if_sandboxed: p.auto_allow_bash_if_sandboxed,
            allow_unsandboxed_commands: p.allow_unsandboxed_commands,
        };
        match self.update_sandbox_settings(update).await {
            Ok(path) => success(serde_json::json!({ "path": path })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_keybindings(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        match self.ensure_keybindings_file().await {
            Ok(file) => success(serde_json::json!({
                "path": file.path,
                "created": file.created,
            })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_context_preview(&self, _params: Option<Value>) -> ResponseResult {
        let context = self.context_preview().await;
        success(context)
    }

    pub(super) async fn handle_context_overview(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
        }
        let p: Params = try_parse!(params);
        match self.context_overview(&p.session_id).await {
            Ok(overview) => success(overview),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_usage_overview(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
        }
        let p: Params = try_parse!(params);
        match self.usage_overview(&p.session_id).await {
            Ok(usage) => success(usage),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_usage_cost(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
        }
        let p: Params = try_parse!(params);
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
        success(serde_json::json!({
            "warnings": kb.warnings(),
        }))
    }

    pub(super) fn handle_settings_editor_mode(&self, _params: Option<Value>) -> ResponseResult {
        let mode = self.editor_mode_setting();
        success(serde_json::json!({ "editor_mode": mode.as_str() }))
    }

    pub(super) async fn handle_settings_set_editor_mode(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            mode: String,
        }
        let p: Params = try_parse!(params);
        let Some(mode) = orbcode_config::EditorModeSetting::parse(&p.mode) else {
            return super::invalid_params(format!("unknown editor mode: {}", p.mode));
        };
        match self.set_editor_mode_setting(mode).await {
            Ok(m) => success(serde_json::json!({ "editor_mode": m.as_str() })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_output_style_options(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        match self.output_style_options().await {
            Ok(options) => {
                let arr: Vec<Value> = options
                    .iter()
                    .map(|o| {
                        serde_json::json!({
                            "value": o.value,
                            "label": o.label,
                            "description": o.description,
                            "current": o.current,
                        })
                    })
                    .collect();
                success(arr)
            }
            Err(e) => core_error(e),
        }
    }

    pub(super) fn handle_settings_active_output_style(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        success(serde_json::json!({
            "name": self.active_output_style_name(),
            "matched": self.active_output_style_matched(),
        }))
    }

    pub(super) fn handle_settings_is_locked(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            key: String,
        }
        let p: Params = try_parse!(params);
        let locked = self.is_setting_locked(&p.key);
        success(serde_json::json!({ "locked": locked }))
    }

    pub(super) async fn handle_settings_set_auto_memory(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            enabled: bool,
        }
        let p: Params = try_parse!(params);
        match self.set_auto_memory_enabled(p.enabled).await {
            Ok(()) => success(serde_json::json!({ "enabled": p.enabled })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_ensure_memory_file(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            path: String,
        }
        let p: Params = try_parse!(params);
        match self
            .ensure_memory_file(std::path::PathBuf::from(&p.path))
            .await
        {
            Ok(()) => success(serde_json::json!({ "path": p.path })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_settings_add_sandbox_excluded(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            command: String,
        }
        let p: Params = try_parse!(params);
        match self.add_sandbox_excluded_command(&p.command).await {
            Ok(cmd) => success(serde_json::json!({
                "pattern": cmd.pattern,
                "path": cmd.path,
            })),
            Err(e) => core_error(e),
        }
    }

    pub(super) fn handle_settings_allow_all(&self, _params: Option<Value>) -> ResponseResult {
        success(serde_json::json!({ "allow_all": self.allow_all() }))
    }

    pub(super) fn handle_settings_set_allow_all(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            allow_all: bool,
        }
        let p: Params = try_parse!(params);
        self.set_allow_all(p.allow_all);
        success(serde_json::json!({ "allow_all": p.allow_all }))
    }
}
