use std::path::PathBuf;

use orbcode_app_server_protocol::{
    AddDirectoryCandidate, AddedDirectory, EffectivePermissionRules, PermissionOverview,
    PermissionRuleEffect, PermissionRuleGroup, PermissionRuleTargetScope, SettingSource,
    SourcedPermissionRuleGroup,
};
use orbcode_config::{
    PermissionMode, PermissionRuleSettingKind, PermissionRuleSettingsUpdate, SettingsSource,
    parse_tool_rule_list,
};
use orbcode_core::CoreError;

use super::AppServer;
use crate::protocol_conversion::permission_context_to_wire;

#[derive(Clone, Copy)]
pub(crate) enum PermissionRuleMutation {
    Add,
    Remove,
}

impl AppServer {
    pub fn permissions(&self) -> orbcode_core::PermissionContext {
        self.active_session_id().map_or_else(
            || self.sessions.permission_context(),
            |session_id| self.sessions.permission_context_for_session(&session_id),
        )
    }

    pub fn permission_mode(&self) -> PermissionMode {
        self.active_session_id()
            .and_then(|session_id| self.sessions.session_permission_mode(&session_id).ok())
            .unwrap_or(PermissionMode::Default)
    }

    pub async fn permission_overview(&self) -> PermissionOverview {
        let active_session_id = self.active_session_id();
        self.permission_overview_for_session(active_session_id.as_deref())
            .await
    }

    pub(crate) async fn permission_overview_for_session(
        &self,
        session_id: Option<&str>,
    ) -> PermissionOverview {
        let remembered_allowed_rules = match session_id {
            Some(session_id) => self.sessions.runtime_permission_rules(session_id).await,
            None => Vec::new(),
        };
        let session_allowed_rules = self
            .sessions
            .session_permission_rules(PermissionRuleSettingKind::Allow);
        let mut runtime_allowed_rules = remembered_allowed_rules.clone();
        for rule in &session_allowed_rules {
            if !runtime_allowed_rules
                .iter()
                .any(|existing| existing == rule)
            {
                runtime_allowed_rules.push(rule.clone());
            }
        }
        let settings_allowed_rules = self
            .sessions
            .settings_permission_rules(PermissionRuleSettingKind::Allow);
        let settings_denied_rules = self
            .sessions
            .settings_permission_rules(PermissionRuleSettingKind::Deny);
        let effective_rules = self.effective_permission_rules(
            &settings_allowed_rules,
            &settings_denied_rules,
            &session_allowed_rules,
            &remembered_allowed_rules,
        );
        PermissionOverview {
            permissions: permission_context_to_wire(session_id.map_or_else(
                || self.sessions.permission_context(),
                |session_id| self.sessions.permission_context_for_session(session_id),
            )),
            effective_rules,
            settings_allowed_rules,
            settings_denied_rules,
            startup_allowed_rules: self
                .sessions
                .startup_permission_rules(PermissionRuleSettingKind::Allow),
            startup_denied_rules: self
                .sessions
                .startup_permission_rules(PermissionRuleSettingKind::Deny),
            edited_allowed_rules: self
                .sessions
                .runtime_added_permission_rules(PermissionRuleSettingKind::Allow),
            edited_denied_rules: self
                .sessions
                .runtime_added_permission_rules(PermissionRuleSettingKind::Deny),
            runtime_allowed_rules,
            runtime_denied_rules: self
                .sessions
                .session_permission_rules(PermissionRuleSettingKind::Deny),
            configured_additional_directories: self.sessions.configured_additional_directories(),
            session_additional_directories: self.sessions.runtime_additional_directories(),
        }
    }

    fn effective_permission_rules(
        &self,
        active_settings_allow: &[String],
        active_settings_deny: &[String],
        session_allow: &[String],
        remembered_allow: &[String],
    ) -> EffectivePermissionRules {
        let config = self.sessions.config();
        let settings_locked = config.ensure_setting_mutable("permissions").is_err();
        let managed_only = config.policy.allow_managed_permission_rules_only;
        let mut managed = PermissionRuleGroup::default();
        let mut settings = Vec::new();
        for group in config.settings_layers.permission_rules().groups {
            let rules = normalized_rule_group(group.allow, group.deny, group.ask);
            if group.source == SettingsSource::Managed {
                managed = rules;
                continue;
            }
            let mut rules = rules;
            rules
                .allow
                .retain(|rule| active_settings_allow.iter().any(|active| active == rule));
            rules
                .deny
                .retain(|rule| active_settings_deny.iter().any(|active| active == rule));
            settings.push(SourcedPermissionRuleGroup {
                source: settings_source_to_wire(group.source),
                active: !managed_only,
                mutable: group.source == SettingsSource::User && !settings_locked,
                rules,
            });
        }

        let settings_ask = normalized_rules(&config.settings.ask_tools);
        let managed_ask = managed.ask.clone();
        let startup_ask = normalized_rules(&config.ask_tools)
            .into_iter()
            .filter(|rule| {
                !settings_ask.iter().any(|existing| existing == rule)
                    && !managed_ask.iter().any(|existing| existing == rule)
            })
            .collect();
        EffectivePermissionRules {
            managed,
            settings,
            startup: PermissionRuleGroup {
                allow: self
                    .sessions
                    .startup_permission_rules(PermissionRuleSettingKind::Allow),
                deny: self
                    .sessions
                    .startup_permission_rules(PermissionRuleSettingKind::Deny),
                ask: startup_ask,
            },
            session: PermissionRuleGroup {
                allow: session_allow.to_vec(),
                deny: self
                    .sessions
                    .session_permission_rules(PermissionRuleSettingKind::Deny),
                ask: Vec::new(),
            },
            runtime_added: PermissionRuleGroup {
                allow: self
                    .sessions
                    .runtime_added_permission_rules(PermissionRuleSettingKind::Allow),
                deny: self
                    .sessions
                    .runtime_added_permission_rules(PermissionRuleSettingKind::Deny),
                ask: Vec::new(),
            },
            remembered: PermissionRuleGroup {
                allow: remembered_allow.to_vec(),
                deny: Vec::new(),
                ask: Vec::new(),
            },
            settings_locked,
            allow_managed_permission_rules_only: managed_only,
            precedence: vec![
                PermissionRuleEffect::Deny,
                PermissionRuleEffect::Ask,
                PermissionRuleEffect::Allow,
            ],
        }
    }

    pub async fn add_permission_rule(
        &self,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
        self.ensure_setting_mutable("permissions")?;
        self.sessions
            .add_configured_permission_rule(kind, rule)
            .await
    }

    pub async fn remove_permission_rule(
        &self,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
        self.ensure_setting_mutable("permissions")?;
        self.sessions
            .remove_configured_permission_rule(kind, rule)
            .await
    }

    pub async fn add_session_permission_rule(
        &self,
        session_id: &str,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
        self.sessions
            .add_session_permission_rule_for_session(session_id, kind, rule)
            .await
    }

    pub async fn remove_session_permission_rule(
        &self,
        session_id: &str,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
        self.sessions
            .remove_session_permission_rule_for_session(session_id, kind, rule)
            .await
    }

    /// Route a validated caller intent to the existing storage owner. Parsing,
    /// normalization, lock checks, and unknown-field-preserving writes stay in
    /// config/core; target scope is resolved before either path is opened.
    pub(crate) async fn mutate_permission_rule(
        &self,
        mutation: PermissionRuleMutation,
        target: PermissionRuleTargetScope,
        session_id: Option<&str>,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> Result<PermissionRuleSettingsUpdate, CoreError> {
        match (mutation, target, session_id) {
            (PermissionRuleMutation::Add, PermissionRuleTargetScope::Settings, None) => {
                self.add_permission_rule(kind, rule).await
            }
            (PermissionRuleMutation::Remove, PermissionRuleTargetScope::Settings, None) => {
                self.remove_permission_rule(kind, rule).await
            }
            (PermissionRuleMutation::Add, PermissionRuleTargetScope::Session, Some(session_id)) => {
                self.add_session_permission_rule(session_id, kind, rule)
                    .await
            }
            (
                PermissionRuleMutation::Remove,
                PermissionRuleTargetScope::Session,
                Some(session_id),
            ) => {
                self.remove_session_permission_rule(session_id, kind, rule)
                    .await
            }
            _ => Err(CoreError::Config(
                "permission rule target and session id do not match".to_string(),
            )),
        }
    }

    pub async fn validate_add_directory(
        &self,
        raw_path: &str,
    ) -> Result<AddDirectoryCandidate, CoreError> {
        let raw_path = raw_path.trim();
        if raw_path.is_empty() {
            return Err(CoreError::Config(
                "Please provide a directory path.".to_string(),
            ));
        }
        let expanded = expand_add_dir_path(raw_path);
        let config = self.sessions.effective_config();
        let candidate = if expanded.is_absolute() {
            expanded
        } else {
            config.cwd.join(expanded)
        };
        let canonical = tokio::fs::canonicalize(&candidate).await.map_err(|_| {
            CoreError::Config(format!("Path {} was not found.", candidate.display()))
        })?;
        let metadata = tokio::fs::metadata(&canonical).await.map_err(|_| {
            CoreError::Config(format!("Path {} was not found.", candidate.display()))
        })?;
        if !metadata.is_dir() {
            let parent = candidate.parent().map_or_else(
                || candidate.display().to_string(),
                |path| path.display().to_string(),
            );
            return Err(CoreError::Config(format!(
                "{raw_path} is not a directory. Did you mean to add the parent directory {parent}?"
            )));
        }

        let mut working_dirs = vec![tokio::fs::canonicalize(&config.cwd).await?];
        working_dirs.extend(self.sessions.additional_directories());
        if let Some(existing) = working_dirs
            .iter()
            .find(|working_dir| path_in_directory(&canonical, working_dir))
        {
            return Err(CoreError::Config(format!(
                "{raw_path} is already accessible within the existing working directory {}.",
                existing.display()
            )));
        }

        Ok(AddDirectoryCandidate { path: canonical })
    }

    pub async fn add_directory(
        &self,
        session_id: &str,
        directory: PathBuf,
    ) -> Result<AddedDirectory, CoreError> {
        self.sessions
            .add_runtime_additional_directory(session_id, directory.clone())
            .await?;
        Ok(AddedDirectory { path: directory })
    }

    /// Apply the legacy SDK mode endpoint to the currently active session.
    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<(), CoreError> {
        let session_id = self
            .active_session_id()
            .ok_or_else(|| CoreError::SessionNotFound("no active session".to_string()))?;
        self.sessions
            .set_session_permission_mode(&session_id, mode)
            .await
    }
}

fn normalized_rules(rules: &[String]) -> Vec<String> {
    parse_tool_rule_list(rules)
        .into_iter()
        .map(|rule| orbcode_config::PermissionRule::parse(&rule).raw)
        .collect()
}

fn normalized_rule_group(
    allow: Vec<String>,
    deny: Vec<String>,
    ask: Vec<String>,
) -> PermissionRuleGroup {
    PermissionRuleGroup {
        allow: normalized_rules(&allow),
        deny: normalized_rules(&deny),
        ask: normalized_rules(&ask),
    }
}

fn settings_source_to_wire(source: SettingsSource) -> SettingSource {
    match source {
        SettingsSource::User => SettingSource::User,
        SettingsSource::Project => SettingSource::Project,
        SettingsSource::Local => SettingSource::Local,
        SettingsSource::Managed => SettingSource::Managed,
    }
}

fn expand_add_dir_path(raw_path: &str) -> PathBuf {
    if let Some(rest) = raw_path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    if raw_path == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home);
    }
    PathBuf::from(raw_path)
}

fn path_in_directory(path: &std::path::Path, directory: &std::path::Path) -> bool {
    path == directory || path.starts_with(directory)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use orbcode_app_server_protocol::{PermissionRuleEffect, PermissionRuleKind, SettingSource};
    use orbcode_config::AppConfigOverrides;

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

    #[tokio::test]
    async fn add_directory_records_session_working_directory() {
        let home = test_path("add-dir-home");
        let cwd = test_path("add-dir-cwd");
        let extra = test_path("add-dir-extra");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::create_dir_all(&extra).await.expect("extra");

        let session_id = "add-dir-session";
        let app = AppServer::new(
            cwd.clone(),
            AppConfigOverrides {
                home_dir: Some(home.clone()),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let candidate = app
            .validate_add_directory(extra.to_str().expect("extra path"))
            .await
            .expect("validate directory");
        let added = app
            .add_directory(session_id, candidate.path)
            .await
            .expect("add directory");
        let extra = std::fs::canonicalize(extra).expect("canonical extra");

        assert_eq!(added.path, extra);
        assert_eq!(
            app.permissions().additional_directories,
            vec![extra.clone()]
        );
        assert_eq!(
            app.permission_overview()
                .await
                .session_additional_directories,
            vec![extra.clone()]
        );
        assert_eq!(
            app.context_preview()
                .await
                .additional_directories
                .as_slice(),
            [extra.display().to_string()]
        );

        let resumed_app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("resumed app server");
        let bootstrap = resumed_app
            .bootstrap(Some(session_id))
            .await
            .expect("resume add-dir session");
        assert_eq!(
            bootstrap.session.additional_directories,
            vec![extra.display().to_string()]
        );
        assert_eq!(
            resumed_app
                .permission_overview()
                .await
                .session_additional_directories,
            vec![extra.clone()]
        );
    }

    #[tokio::test]
    async fn validate_add_directory_reports_existing_working_directory() {
        let home = test_path("add-dir-existing-home");
        let cwd = test_path("add-dir-existing-cwd");
        let child = cwd.join("child");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&child).await.expect("child");

        let app = AppServer::new(
            cwd.clone(),
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let err = app
            .validate_add_directory(child.to_str().expect("child path"))
            .await
            .expect_err("child should already be accessible");

        assert!(err.to_string().contains("already accessible"));
        assert!(app.permissions().additional_directories.is_empty());
    }

    #[tokio::test]
    async fn validate_add_directory_reports_typescript_style_errors() {
        let home = test_path("add-dir-validation-home");
        let cwd = test_path("add-dir-validation-cwd");
        let missing = cwd.join("missing");
        let file = cwd.join("file.txt");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::write(&file, "not a directory")
            .await
            .expect("file");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        let empty = app
            .validate_add_directory(" ")
            .await
            .expect_err("empty path should fail")
            .to_string();
        let not_found = app
            .validate_add_directory(missing.to_str().expect("missing path"))
            .await
            .expect_err("missing path should fail")
            .to_string();
        let not_dir = app
            .validate_add_directory(file.to_str().expect("file path"))
            .await
            .expect_err("file path should fail")
            .to_string();

        assert!(empty.contains("Please provide a directory path."));
        assert!(not_found.contains("was not found."));
        assert!(not_dir.contains("is not a directory. Did you mean to add the parent directory"));
    }

    #[tokio::test]
    async fn permission_overview_preserves_sources_ask_rules_and_legacy_effective_order() {
        let home = test_path("permission-sources-home");
        let cwd = test_path("permission-sources-cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(cwd.join(".claude"))
            .await
            .expect("project settings dir");
        tokio::fs::write(
            home.join("settings.json"),
            r#"{"permissions":{"allow":["Read(user/**)"],"ask":["Write(user/**)"]}}"#,
        )
        .await
        .expect("user settings");
        tokio::fs::write(
            cwd.join(".claude/settings.json"),
            r#"{"permissions":{"deny":["Bash(rm:*)"]}}"#,
        )
        .await
        .expect("project settings");
        tokio::fs::write(
            cwd.join(".claude/settings.local.json"),
            r#"{"permissions":{"allow":["Grep(src/**)"]}}"#,
        )
        .await
        .expect("local settings");

        let app = AppServer::new(
            cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                env_overrides: orbcode_config::sealed_provider_env_overrides(),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");
        let overview = app.permission_overview().await;

        assert_eq!(
            overview.effective_rules.precedence,
            vec![
                PermissionRuleEffect::Deny,
                PermissionRuleEffect::Ask,
                PermissionRuleEffect::Allow,
            ]
        );
        assert_eq!(
            overview
                .effective_rules
                .settings
                .iter()
                .map(|group| group.source)
                .collect::<Vec<_>>(),
            vec![
                SettingSource::User,
                SettingSource::Project,
                SettingSource::Local
            ]
        );
        assert_eq!(
            overview.effective_rules.settings[0].rules.ask,
            vec!["Write(user/**)".to_string()]
        );
        assert!(
            overview
                .effective_rules
                .settings
                .iter()
                .all(|group| group.active)
        );
        assert!(overview.effective_rules.settings[0].mutable);
        assert!(!overview.effective_rules.settings[1].mutable);
        assert_eq!(
            overview.configured_rules(PermissionRuleKind::Allow),
            overview.settings_allowed_rules
        );
        assert_eq!(
            overview.configured_rules(PermissionRuleKind::Deny),
            overview.settings_denied_rules
        );
    }
}
