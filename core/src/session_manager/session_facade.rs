use std::path::PathBuf;

use orbcode_config::{
    AppConfig, EditorModeSetting, ModelOption, PermissionRuleSettingKind,
    PermissionRuleSettingsUpdate, RuntimeModelOverride, ThemeSetting, add_permission_rule_setting,
    remove_permission_rule_setting, update_editor_mode_setting, update_model_setting,
    update_theme_setting,
};
use orbcode_model_provider::{ProviderDescriptor, supported_providers};
use orbcode_protocol::EffortLevel;

use super::SessionManager;
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
