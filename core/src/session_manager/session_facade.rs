use std::path::PathBuf;

use orbcode_config::{
    AppConfig, EditorModeSetting, ModelCapability, ModelOption, PermissionMode,
    PermissionRuleSettingKind, PermissionRuleSettingsUpdate, RuntimeModelOverride, ThemeSetting,
    add_permission_rule_setting, remove_permission_rule_setting, update_editor_mode_setting,
    update_model_setting, update_theme_setting,
};
use orbcode_model_provider::{ProviderDescriptor, supported_providers};
use orbcode_protocol::EffortLevel;

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
        config.apply_runtime_model_override(controls.model.as_deref());
        config.permission_mode = Some(controls.permission_mode);
        if let Some(allow_tools) = controls.permission_mode.default_allow_tools() {
            config.allow_tools = allow_tools;
        }
        if let Some(allow_network) = controls.permission_mode.default_allow_network() {
            config.allow_network = allow_network;
        }
        config
    }

    pub fn register_session_controls(&self, session: &orbcode_protocol::SessionRecord) {
        let config = self.effective_config();
        let configured_permission_mode = config.permission_mode.unwrap_or(PermissionMode::Default);
        let permission_mode = match configured_permission_mode {
            PermissionMode::Default
            | PermissionMode::AcceptEdits
            | PermissionMode::Plan
            | PermissionMode::DontAsk => configured_permission_mode,
            PermissionMode::BypassPermissions | PermissionMode::Auto => PermissionMode::Default,
        };
        let model = config.provider_model_setting();
        let effort = session
            .session_effort
            .or_else(|| self.runtime_effort_override());
        self.session_controls_write()
            .entry(session.session_id.clone())
            .or_insert(SessionControlOverrides {
                permission_mode,
                model,
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
        config.apply_runtime_model_override(controls.model.as_deref());
        Ok(config.model_options_for_setting(controls.model.as_deref()))
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
        if !matches!(
            mode,
            PermissionMode::Default
                | PermissionMode::AcceptEdits
                | PermissionMode::Plan
                | PermissionMode::DontAsk
        ) {
            return Err(CoreError::Config(format!(
                "permission mode {} is not available for session controls",
                mode.as_str()
            )));
        }
        self.ensure_session_controls_mutable(session_id).await?;
        self.session_controls_write()
            .get_mut(session_id)
            .expect("session controls validated before mutation")
            .permission_mode = mode;
        Ok(())
    }

    pub async fn set_session_model(
        &self,
        session_id: &str,
        model: Option<String>,
    ) -> Result<(), CoreError> {
        self.ensure_session_controls_mutable(session_id).await?;
        let model = model.and_then(|model| {
            let trimmed = model.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
        if !self
            .session_model_options(session_id)?
            .iter()
            .any(|option| option.value == model)
        {
            return Err(CoreError::Config(format!(
                "model option {} is not available for session {session_id}",
                model.as_deref().unwrap_or("default")
            )));
        }
        self.session_controls_write()
            .get_mut(session_id)
            .expect("session controls validated before mutation")
            .model = model;

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
        !matches!(
            self.session_controls_read()
                .get(session_id)
                .map(|controls| controls.permission_mode),
            Some(PermissionMode::Plan)
        )
    }

    async fn ensure_session_controls_mutable(&self, session_id: &str) -> Result<(), CoreError> {
        if !self.session_controls_read().contains_key(session_id) {
            return Err(CoreError::SessionNotFound(session_id.to_string()));
        }
        if self.active_turns.has_active_session(session_id).await {
            return Err(CoreError::ActiveTurn(session_id.to_string()));
        }
        Ok(())
    }

    fn session_controls_read(
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

    pub async fn set_model_override(&self, model: Option<String>) -> Result<(), CoreError> {
        update_model_setting(&self.config.home_dir, model.as_deref()).await?;
        self.set_runtime_model_override(model);
        Ok(())
    }

    pub fn theme_setting(&self) -> ThemeSetting {
        self.runtime_state.theme_setting(&self.config)
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
        let base_setting = self.config.provider_model_setting();
        let current_setting = match runtime_setting.as_ref() {
            Some(Some(model)) => Some(model.as_str()),
            Some(None) => None,
            None => base_setting.as_deref(),
        };
        let effective = self.effective_config();
        let mut options = self.config.model_options_for_setting(current_setting);
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
        if self.session_controls_read().contains_key(session_id) {
            // A session-scoped mode must not inherit another client's
            // process-global allow-all switch.
            permissions.allow_tools = config.allow_tools;
            permissions.allow_network = config.allow_network;
        }
        permissions
    }

    pub async fn add_configured_permission_rule(
        &self,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
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
        let normalized = normalize_permission_rule_for_edit(rule).map_err(CoreError::Config)?;
        let changed = self
            .permission_runtime
            .record_session_permission_rule_remove(kind, &normalized)
            .await;
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
        let update = self.remove_session_permission_rule(kind, rule).await?;
        if update.changed {
            self.persist_runtime_session_context(session_id).await?;
        }
        Ok(update)
    }

    pub fn set_allow_all(&self, enabled: bool) {
        self.permission_runtime.set_allow_all(enabled);
    }

    pub fn allow_all(&self) -> bool {
        self.permission_runtime.allow_all()
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

    pub async fn runtime_permission_rules(&self) -> Vec<String> {
        self.permission_runtime.runtime_permission_rules().await
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
