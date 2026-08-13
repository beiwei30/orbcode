use std::path::PathBuf;

use orbcode_config::{
    AppConfig, EditorModeSetting, ModelCapability, ModelOption, PermissionMode,
    PermissionRuleSettingKind, PermissionRuleSettingsUpdate, RuntimeModelOverride, ThemeSetting,
    add_permission_rule_setting, remove_permission_rule_setting, update_editor_mode_setting,
    update_model_setting, update_theme_setting,
};
use orbcode_model_provider::{ProviderDescriptor, supported_providers};
use orbcode_protocol::{EffortLevel, ModelPermissionPolicy, ModelPermissionPreset};
use orbcode_tools::FileReadState;
use std::sync::Arc;

use super::{SessionControlOverrides, SessionManager};
use crate::{
    CoreError,
    permissions::{PermissionContext, normalize_permission_rule_for_edit},
};

impl SessionManager {
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn effective_config(&self) -> AppConfig {
        self.runtime_state.effective_config(&self.config)
    }

    /// Resolve the immutable configuration view used by a particular session's
    /// next turn. Session controls never modify process-global settings.
    pub fn effective_config_for_session(&self, session_id: &str) -> AppConfig {
        let mut config = self.effective_config();
        let Some(controls) = self.session_controls_read().get(session_id).cloned() else {
            return config;
        };
        if !controls.model.is_inherit() {
            config.apply_runtime_model_override(controls.model.requested_model());
        }
        config.permission_mode = Some(controls.permission_mode);
        if controls.permission_preset.is_some() {
            let explicit = config.explicit_permission_overrides;
            if controls.permission_preset_overridden || !explicit.sandbox_mode {
                config.sandbox_mode = controls.permission_policy.sandbox_mode;
            }
            if controls.permission_preset_overridden || !explicit.sandbox_allow_network {
                config.sandbox_allow_network = controls.permission_policy.sandbox_allow_network;
            }
            if controls.permission_preset_overridden || !explicit.allow_tools {
                config.allow_tools = true;
            }
            if controls.permission_preset_overridden || !explicit.allow_network {
                config.allow_network = matches!(
                    controls.permission_policy.preset,
                    ModelPermissionPreset::FullAccess
                );
            }
        } else {
            if let Some(allow_tools) = controls.permission_mode.default_allow_tools() {
                config.allow_tools = allow_tools;
            }
            if let Some(allow_network) = controls.permission_mode.default_allow_network() {
                config.allow_network = allow_network;
            }
        }
        config
    }

    pub fn register_session_controls(&self, session: &orbcode_protocol::SessionRecord) {
        let config = self.effective_config();
        let configured_permission_mode = config.permission_mode.unwrap_or(PermissionMode::Default);
        let permission_mode = if configured_permission_mode == PermissionMode::BypassPermissions
            && config.policy.disable_bypass_permissions_mode
        {
            PermissionMode::Default
        } else {
            configured_permission_mode
        };
        let permission_preset = permission_preset_for_legacy_mode(permission_mode);
        let permission_policy = permission_preset
            .unwrap_or(ModelPermissionPreset::AskForApproval)
            .policy();
        let effort = session
            .session_effort
            .or_else(|| self.runtime_effort_override());
        self.session_controls_write()
            .entry(session.session_id.clone())
            .or_insert(SessionControlOverrides {
                permission_mode,
                permission_mode_overridden: false,
                permission_preset,
                permission_policy,
                permission_preset_overridden: false,
                model: RuntimeModelOverride::Inherit,
                effort,
            });
    }

    pub fn remove_session_controls(&self, session_id: &str) {
        self.session_controls_write().remove(session_id);
    }

    pub fn session_permission_mode(&self, session_id: &str) -> Result<PermissionMode, CoreError> {
        self.session_controls_read()
            .get(session_id)
            .map(|controls| controls.permission_mode)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))
    }

    pub fn session_permission_preset(
        &self,
        session_id: &str,
    ) -> Result<Option<ModelPermissionPreset>, CoreError> {
        self.session_controls_read()
            .get(session_id)
            .map(|controls| controls.permission_preset)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))
    }

    pub fn session_permission_policy(
        &self,
        session_id: &str,
    ) -> Result<ModelPermissionPolicy, CoreError> {
        self.session_controls_read()
            .get(session_id)
            .map(|controls| controls.permission_policy)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))
    }

    pub fn permission_preset_disabled_reason(
        &self,
        preset: ModelPermissionPreset,
    ) -> Option<String> {
        if preset == ModelPermissionPreset::FullAccess
            && self
                .effective_config()
                .policy
                .disable_bypass_permissions_mode
        {
            Some("Full Access is disabled by managed policy".to_string())
        } else {
            None
        }
    }

    pub fn session_effort_level(&self, session_id: &str) -> Result<Option<EffortLevel>, CoreError> {
        self.session_controls_read()
            .get(session_id)
            .map(|controls| controls.effort)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))
    }

    pub fn session_model_options(&self, session_id: &str) -> Result<Vec<ModelOption>, CoreError> {
        let controls = self
            .session_controls_read()
            .get(session_id)
            .cloned()
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;
        let mut config = self.effective_config();
        if !controls.model.is_inherit() {
            config.apply_runtime_model_override(controls.model.requested_model());
        }
        let current_model = if controls.model.is_inherit() {
            config.provider_model_setting()
        } else {
            controls.model.requested_model().map(str::to_string)
        };
        Ok(config.model_options_for_setting(current_model.as_deref()))
    }

    pub fn session_effort_options(&self, session_id: &str) -> Result<Vec<EffortLevel>, CoreError> {
        let config = self.effective_config_for_session(session_id);
        let capabilities = config
            .provider_model_resolution(config.default_provider)
            .capabilities;
        if !capabilities.iter().any(|capability| {
            matches!(
                capability,
                ModelCapability::Effort
                    | ModelCapability::MaxEffort
                    | ModelCapability::XHighEffort
                    | ModelCapability::Thinking
                    | ModelCapability::AdaptiveThinking
            )
        }) {
            return Ok(Vec::new());
        }
        let mut options = vec![EffortLevel::Low, EffortLevel::Medium, EffortLevel::High];
        if capabilities.iter().any(|capability| {
            matches!(
                capability,
                ModelCapability::MaxEffort
                    | ModelCapability::XHighEffort
                    | ModelCapability::Thinking
                    | ModelCapability::AdaptiveThinking
            )
        }) {
            options.push(EffortLevel::Max);
        }
        Ok(options)
    }

    pub async fn set_session_permission_mode(
        &self,
        session_id: &str,
        mode: PermissionMode,
    ) -> Result<(), CoreError> {
        if mode == PermissionMode::BypassPermissions
            && let Some(reason) =
                self.permission_preset_disabled_reason(ModelPermissionPreset::FullAccess)
        {
            return Err(CoreError::PermissionDenied(reason));
        }
        self.ensure_session_controls_mutable(session_id).await?;
        let mut controls = self.session_controls_write();
        let controls = controls
            .get_mut(session_id)
            .expect("session controls validated before mutation");
        controls.permission_mode = mode;
        controls.permission_mode_overridden = true;
        controls.permission_preset = permission_preset_for_legacy_mode(mode);
        controls.permission_policy = controls
            .permission_preset
            .unwrap_or(ModelPermissionPreset::AskForApproval)
            .policy();
        controls.permission_preset_overridden = false;
        Ok(())
    }

    pub async fn set_session_permission_preset(
        &self,
        session_id: &str,
        preset: ModelPermissionPreset,
    ) -> Result<(), CoreError> {
        if let Some(reason) = self.permission_preset_disabled_reason(preset) {
            return Err(CoreError::PermissionDenied(reason));
        }
        self.ensure_session_controls_mutable(session_id).await?;
        let mut controls = self.session_controls_write();
        let controls = controls
            .get_mut(session_id)
            .expect("session controls validated before mutation");
        controls.permission_mode = legacy_mode_for_permission_preset(preset);
        controls.permission_mode_overridden = true;
        controls.permission_preset = Some(preset);
        controls.permission_policy = preset.policy();
        controls.permission_preset_overridden = true;
        Ok(())
    }

    pub async fn set_session_model(
        &self,
        session_id: &str,
        model: Option<String>,
    ) -> Result<(), CoreError> {
        // The active provider request already owns an immutable model. Updating
        // this session control during a turn is therefore safe and applies only
        // to the next request, matching the stream-json SDK contract.
        self.ensure_session_controls_exist(session_id)?;
        let model = model.and_then(|model| {
            let trimmed = model.trim();
            (!trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("default"))
                .then(|| trimmed.to_string())
        });
        if let Some(model) = model.as_deref() {
            if model.chars().any(char::is_control) {
                return Err(CoreError::Config(
                    "model name must not contain control characters".to_string(),
                ));
            }
            let config = self.effective_config();
            if !config.default_provider.is_active() {
                return Err(CoreError::Config(format!(
                    "provider '{}' cannot serve model requests",
                    config.default_provider
                )));
            }
            if !config.policy.model_allowed(model) {
                return Err(CoreError::PermissionDenied(format!(
                    "model `{model}` is not allowed by managed policy"
                )));
            }
            let resolved = config.resolve_model_setting(config.default_provider, Some(model));
            if resolved.trim().is_empty() {
                return Err(CoreError::Config(format!(
                    "model `{model}` resolved to an empty provider model"
                )));
            }
        }
        {
            let mut controls = self.session_controls_write();
            let controls = controls
                .get_mut(session_id)
                .expect("session controls validated before mutation");
            controls.model = RuntimeModelOverride::from_model(model);
        }

        let available_effort = self.session_effort_options(session_id)?;
        let mut controls = self.session_controls_write();
        let controls = controls
            .get_mut(session_id)
            .expect("session controls validated before mutation");
        if controls
            .effort
            .is_some_and(|effort| !available_effort.contains(&effort))
        {
            controls.effort = None;
        }
        Ok(())
    }

    /// Clear the session-local choice and resume the layered global model.
    /// Selecting the provider default remains `set_session_model(..., None)`.
    pub async fn clear_session_model_override(&self, session_id: &str) -> Result<(), CoreError> {
        self.ensure_session_controls_exist(session_id)?;
        {
            let mut controls = self.session_controls_write();
            let controls = controls
                .get_mut(session_id)
                .expect("session controls validated before mutation");
            controls.model = RuntimeModelOverride::Inherit;
        }

        let available_effort = self.session_effort_options(session_id)?;
        let mut controls = self.session_controls_write();
        let controls = controls
            .get_mut(session_id)
            .expect("session controls validated before mutation");
        if controls
            .effort
            .is_some_and(|effort| !available_effort.contains(&effort))
        {
            controls.effort = None;
        }
        Ok(())
    }

    pub async fn set_session_effort(
        &self,
        session_id: &str,
        effort: Option<EffortLevel>,
    ) -> Result<(), CoreError> {
        self.ensure_session_controls_mutable(session_id).await?;
        let available = self.session_effort_options(session_id)?;
        if effort.is_some_and(|effort| !available.contains(&effort)) {
            return Err(CoreError::Config(format!(
                "effort option {} is not available for session {session_id}",
                effort.expect("checked Some").as_str()
            )));
        }
        self.session_controls_write()
            .get_mut(session_id)
            .expect("session controls validated before mutation")
            .effort = effort;
        Ok(())
    }

    pub fn session_exposes_tools(&self, session_id: &str) -> bool {
        self.effective_config_for_session(session_id).allow_tools
    }

    async fn ensure_session_controls_mutable(&self, session_id: &str) -> Result<(), CoreError> {
        self.ensure_session_controls_exist(session_id)?;
        if self.active_turns.has_active_session(session_id).await {
            return Err(CoreError::ActiveTurn(session_id.to_string()));
        }
        Ok(())
    }

    fn ensure_session_controls_exist(&self, session_id: &str) -> Result<(), CoreError> {
        if self.session_controls_read().contains_key(session_id) {
            Ok(())
        } else {
            Err(CoreError::SessionNotFound(session_id.to_string()))
        }
    }

    pub(super) fn session_controls_read(
        &self,
    ) -> std::sync::RwLockReadGuard<'_, std::collections::HashMap<String, SessionControlOverrides>>
    {
        match self.session_controls.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn session_controls_write(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, std::collections::HashMap<String, SessionControlOverrides>>
    {
        match self.session_controls.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn runtime_model_override(&self) -> RuntimeModelOverride {
        self.runtime_state.model_override()
    }

    pub fn set_runtime_model_override(&self, model: Option<String>) {
        self.runtime_state.set_model_override(model);
    }

    pub async fn set_persisted_model_setting(
        &self,
        model: Option<String>,
    ) -> Result<(), CoreError> {
        update_model_setting(&self.config.home_dir, model.as_deref()).await?;
        self.runtime_state.refresh_persisted_model_setting(model);
        Ok(())
    }

    pub fn theme_setting(&self) -> ThemeSetting {
        self.runtime_state.theme_setting(&self.config)
    }

    pub fn client_preferences(&self) -> orbcode_config::ClientPreferences {
        self.runtime_state.client_preferences(&self.config)
    }

    pub fn set_runtime_theme_setting(&self, theme: ThemeSetting) {
        self.runtime_state.set_theme_setting(theme);
    }

    pub async fn set_theme_setting(&self, theme: ThemeSetting) -> Result<(), CoreError> {
        update_theme_setting(&self.config.home_dir, theme).await?;
        self.set_runtime_theme_setting(theme);
        Ok(())
    }

    pub fn editor_mode_setting(&self) -> EditorModeSetting {
        self.runtime_state.editor_mode_setting(&self.config)
    }

    pub fn set_runtime_editor_mode_setting(&self, mode: EditorModeSetting) {
        self.runtime_state.set_editor_mode_setting(mode);
    }

    pub async fn set_editor_mode_setting(&self, mode: EditorModeSetting) -> Result<(), CoreError> {
        update_editor_mode_setting(&self.config.home_dir, mode).await?;
        self.set_runtime_editor_mode_setting(mode);
        Ok(())
    }

    pub fn runtime_effort_override(&self) -> Option<EffortLevel> {
        self.runtime_state.effort_override()
    }

    pub fn set_runtime_effort_override(&self, effort: Option<EffortLevel>) {
        self.runtime_state.set_effort_override(effort);
    }

    pub fn max_thinking_tokens(&self) -> Option<u32> {
        self.runtime_state.max_thinking_tokens()
    }

    pub fn set_max_thinking_tokens(&self, max_thinking_tokens: Option<u32>) {
        self.runtime_state
            .set_max_thinking_tokens(max_thinking_tokens);
    }

    pub fn read_state(&self) -> Arc<FileReadState> {
        Arc::clone(&self.read_state)
    }

    pub async fn set_effort_override_for_session(
        &self,
        session_id: &str,
        effort: Option<EffortLevel>,
    ) -> Result<(), CoreError> {
        self.set_runtime_effort_override(effort);
        self.persist_runtime_session_context(session_id).await
    }

    pub fn model_display_name(&self) -> String {
        let config = self.effective_config();
        if self.uses_chatgpt_subscription() && !config.provider_model_is_explicit() {
            "gpt-5.6-sol".to_string()
        } else {
            config.model_display_name()
        }
    }

    pub fn model_options(&self) -> Vec<ModelOption> {
        let runtime_setting = self.runtime_model_override();
        let effective = self.effective_config();
        let base_setting = effective.provider_model_setting();
        let current_setting = match &runtime_setting {
            RuntimeModelOverride::Model(model) => Some(model.as_str()),
            RuntimeModelOverride::Default => None,
            RuntimeModelOverride::Inherit => base_setting.as_deref(),
        };
        let mut options = effective.model_options_for_setting(current_setting);
        for option in effective.model_options_for_setting(current_setting) {
            if !options
                .iter()
                .any(|existing| existing.value == option.value)
            {
                options.push(option);
            }
        }
        if !options.iter().any(|option| option.current) {
            let current_model = effective.provider_model_name(effective.default_provider);
            options.insert(
                0,
                ModelOption {
                    value: current_setting.map(str::to_string),
                    label: current_model.clone(),
                    description: "Current configured model".to_string(),
                    current: true,
                },
            );
        }
        options
    }

    pub fn supported_providers(&self) -> Vec<ProviderDescriptor> {
        supported_providers()
    }

    pub fn permission_context(&self) -> PermissionContext {
        self.permission_runtime
            .permission_context(&self.effective_config(), self.additional_directories())
    }

    pub fn permission_context_for_session(&self, session_id: &str) -> PermissionContext {
        let config = self.effective_config_for_session(session_id);
        let mut permissions = self
            .permission_runtime
            .permission_context(&config, self.additional_directories());
        if self
            .session_controls_read()
            .get(session_id)
            .is_some_and(|controls| {
                controls.permission_mode_overridden || controls.permission_preset.is_some()
            })
        {
            // A session-scoped mode must not inherit another client's
            // process-global allow-all switch.
            permissions.allow_tools = config.allow_tools;
            permissions.allow_network = config.allow_network;
            permissions.provider_allow_network = config.provider_allow_network;
        }
        permissions
    }

    pub async fn add_configured_permission_rule(
        &self,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
        self.ensure_permission_rules_mutable()?;
        let normalized = normalize_permission_rule_for_edit(rule).map_err(CoreError::Config)?;
        let update = add_permission_rule_setting(&self.config.home_dir, kind, &normalized).await?;
        if update.changed {
            self.permission_runtime.record_runtime_permission_rule_add(
                &self.config,
                kind,
                update.rule.clone(),
            );
        }
        Ok(update)
    }

    pub async fn remove_configured_permission_rule(
        &self,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
        self.ensure_permission_rules_mutable()?;
        let normalized = normalize_permission_rule_for_edit(rule).map_err(CoreError::Config)?;
        let update =
            remove_permission_rule_setting(&self.config.home_dir, kind, &normalized).await?;
        if update.changed {
            self.permission_runtime
                .record_runtime_permission_rule_remove(&self.config, kind, update.rule.clone());
        }
        Ok(update)
    }

    #[allow(clippy::unused_async)] // Public facade mirrors async permission APIs; this path is in-memory today.
    pub async fn add_session_permission_rule(
        &self,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
        self.ensure_permission_rules_mutable()?;
        let normalized = normalize_permission_rule_for_edit(rule).map_err(CoreError::Config)?;
        let changed = self
            .permission_runtime
            .record_session_permission_rule_add(kind, normalized.clone());
        Ok(PermissionRuleSettingsUpdate {
            path: self.config.settings_path.clone(),
            rule: normalized,
            changed,
        })
    }

    pub async fn add_session_permission_rule_for_session(
        &self,
        session_id: &str,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
        let update = self.add_session_permission_rule(kind, rule).await?;
        if update.changed {
            self.persist_runtime_session_context(session_id).await?;
        }
        Ok(update)
    }

    pub async fn remove_session_permission_rule(
        &self,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
        self.ensure_permission_rules_mutable()?;
        let normalized = normalize_permission_rule_for_edit(rule).map_err(CoreError::Config)?;
        let changed = self
            .permission_runtime
            .record_unscoped_session_permission_rule_remove(kind, &normalized);
        Ok(PermissionRuleSettingsUpdate {
            path: self.config.settings_path.clone(),
            rule: normalized,
            changed,
        })
    }

    pub async fn remove_session_permission_rule_for_session(
        &self,
        session_id: &str,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
        self.ensure_permission_rules_mutable()?;
        let normalized = normalize_permission_rule_for_edit(rule).map_err(CoreError::Config)?;
        let changed = self
            .permission_runtime
            .record_session_permission_rule_remove(session_id, kind, &normalized)
            .await;
        let update = PermissionRuleSettingsUpdate {
            path: self.config.settings_path.clone(),
            rule: normalized,
            changed,
        };
        if update.changed {
            self.persist_runtime_session_context(session_id).await?;
        }
        Ok(update)
    }

    fn ensure_permission_rules_mutable(&self) -> Result<(), CoreError> {
        self.config
            .ensure_setting_mutable("permissions")
            .map_err(|error| CoreError::PermissionDenied(error.message))
    }

    pub fn additional_directories(&self) -> Vec<PathBuf> {
        self.runtime_state
            .additional_directories(&self.effective_config())
    }

    pub fn configured_additional_directories(&self) -> Vec<PathBuf> {
        self.effective_config().additional_directories
    }

    pub fn runtime_additional_directories(&self) -> Vec<PathBuf> {
        self.runtime_state.runtime_additional_directories()
    }

    pub async fn add_runtime_additional_directory(
        &self,
        session_id: &str,
        directory: PathBuf,
    ) -> Result<bool, CoreError> {
        let changed = self
            .runtime_state
            .add_runtime_additional_directory(&self.effective_config(), directory);
        if changed {
            self.persist_runtime_session_context(session_id).await?;
        }
        Ok(changed)
    }

    async fn persist_runtime_session_context(&self, session_id: &str) -> Result<(), CoreError> {
        self.transcript_store
            .append_session_context_line(
                session_id,
                &self.runtime_additional_directories(),
                &self.session_permission_rules(PermissionRuleSettingKind::Allow),
                &self.session_permission_rules(PermissionRuleSettingKind::Deny),
                self.runtime_effort_override(),
            )
            .await?;
        Ok(())
    }

    pub async fn runtime_permission_rules(&self, session_id: &str) -> Vec<String> {
        self.permission_runtime
            .runtime_permission_rules_for_session(session_id)
            .await
    }

    pub fn settings_permission_rules(&self, kind: PermissionRuleSettingKind) -> Vec<String> {
        self.permission_runtime
            .settings_permission_rules(&self.config, kind)
    }

    pub fn startup_permission_rules(&self, kind: PermissionRuleSettingKind) -> Vec<String> {
        self.permission_runtime
            .startup_permission_rules(&self.config, kind)
    }

    pub fn session_permission_rules(&self, kind: PermissionRuleSettingKind) -> Vec<String> {
        self.permission_runtime.session_permission_rules(kind)
    }

    pub fn runtime_added_permission_rules(&self, kind: PermissionRuleSettingKind) -> Vec<String> {
        self.permission_runtime.runtime_added_permission_rules(kind)
    }
}

fn permission_preset_for_legacy_mode(mode: PermissionMode) -> Option<ModelPermissionPreset> {
    match mode {
        PermissionMode::Default => Some(ModelPermissionPreset::AskForApproval),
        PermissionMode::Auto => Some(ModelPermissionPreset::ApproveForMe),
        PermissionMode::BypassPermissions => Some(ModelPermissionPreset::FullAccess),
        PermissionMode::Plan => None,
    }
}

fn legacy_mode_for_permission_preset(preset: ModelPermissionPreset) -> PermissionMode {
    match preset {
        ModelPermissionPreset::AskForApproval => PermissionMode::Default,
        ModelPermissionPreset::ApproveForMe => PermissionMode::Auto,
        ModelPermissionPreset::FullAccess => PermissionMode::BypassPermissions,
    }
}
