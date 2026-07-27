use std::path::PathBuf;

use orbcode_app_server_protocol::{AddDirectoryCandidate, AddedDirectory, PermissionOverview};
use orbcode_config::{PermissionMode, PermissionRuleSettingKind, PermissionRuleSettingsUpdate};
use orbcode_core::CoreError;

use super::AppServer;

impl AppServer {
    pub fn permissions(&self) -> orbcode_core::PermissionContext {
        self.sessions.permission_context()
    }

    pub fn permission_mode(&self) -> PermissionMode {
        self.sessions
            .effective_config()
            .permission_mode
            .unwrap_or(PermissionMode::Default)
    }

    pub async fn permission_overview(&self) -> PermissionOverview {
        let mut runtime_allowed_rules = self.sessions.runtime_permission_rules().await;
        for rule in self
            .sessions
            .session_permission_rules(PermissionRuleSettingKind::Allow)
        {
            if !runtime_allowed_rules
                .iter()
                .any(|existing| existing == &rule)
            {
                runtime_allowed_rules.push(rule);
            }
        }
        PermissionOverview {
            permissions: self.permissions(),
            allow_all: self.allow_all(),
            settings_allowed_rules: self
                .sessions
                .settings_permission_rules(PermissionRuleSettingKind::Allow),
            settings_denied_rules: self
                .sessions
                .settings_permission_rules(PermissionRuleSettingKind::Deny),
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

    pub fn set_allow_all(&self, enabled: bool) {
        self.sessions.set_allow_all(enabled);
    }

    pub fn allow_all(&self) -> bool {
        self.sessions.allow_all()
    }

    /// Apply an SDK `set_permission_mode` control request at runtime.
    ///
    /// Reuses the existing allow-all escalation rather than threading a new
    /// runtime permission mode through core: modes that grant unprompted tool
    /// access (`acceptEdits` / `bypassPermissions` / `dontAsk` / `auto`) enable
    /// allow-all so subsequent tool calls run without a permission prompt, while
    /// `default` / `plan` disable it so the normal allow/deny gating applies. The
    /// mapping mirrors `permission_mode_default_allow_tools` in config.
    pub fn set_permission_mode(&self, mode: PermissionMode) {
        let allow_all = matches!(
            mode,
            PermissionMode::AcceptEdits
                | PermissionMode::BypassPermissions
                | PermissionMode::DontAsk
                | PermissionMode::Auto
        );
        self.set_allow_all(allow_all);
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
}
