//! Integration coverage for managed-policy enforcement surfaces.
//!
//! Each test drives a real `AppConfig::load` against a temporary HOME, CWD,
//! and managed-settings directory (via `CLAUDE_CODE_MANAGED_SETTINGS_PATH`) and
//! asserts that the policy is actually enforced at the registration boundary:
//! permission rules, bypass mode, MCP allowlisting, hook restriction, forced
//! login method, and mutation locking.
//!
//! Run serially (`--test-threads=1`): the managed-settings path is process-wide
//! environment state. The guard below also serializes mutation defensively.

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use orbcode_config::{
    AppConfig, AppConfigOverrides, AuthManager, AuthMethod, PermissionMode,
    parse_forced_login_method,
};
use orbcode_core::PermissionContext;
use orbcode_protocol::ProviderId;
use serde_json::{Value, json};
use tempfile::TempDir;

static MANAGED_PATH_GUARD: Mutex<()> = Mutex::new(());

/// Pins `CLAUDE_CODE_MANAGED_SETTINGS_PATH` to a managed directory for the
/// duration of a test, restoring the previous value on drop. Holds a process
/// lock so concurrent tests never observe a half-written env var.
struct ManagedPathGuard<'a> {
    _lock: MutexGuard<'a, ()>,
    previous: Option<OsString>,
}

impl<'a> ManagedPathGuard<'a> {
    fn set(path: &Path) -> Self {
        let lock = MANAGED_PATH_GUARD.lock().expect("managed path lock");
        let previous = std::env::var_os("CLAUDE_CODE_MANAGED_SETTINGS_PATH");
        // SAFETY: the static mutex serializes env mutation across tests.
        unsafe { std::env::set_var("CLAUDE_CODE_MANAGED_SETTINGS_PATH", path) };
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for ManagedPathGuard<'_> {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var("CLAUDE_CODE_MANAGED_SETTINGS_PATH", value) },
            None => unsafe { std::env::remove_var("CLAUDE_CODE_MANAGED_SETTINGS_PATH") },
        }
    }
}

async fn write_managed_settings(managed_dir: &Path, value: &Value) {
    tokio::fs::create_dir_all(managed_dir)
        .await
        .expect("mkdir managed");
    tokio::fs::write(
        managed_dir.join("managed-settings.json"),
        serde_json::to_string_pretty(value).unwrap(),
    )
    .await
    .expect("write managed settings");
}

/// Load an `AppConfig` against isolated temp HOME/CWD with the given managed
/// settings already pinned via the env guard. Returns the config plus the temp
/// dirs (kept alive by the caller).
async fn load_with_managed(
    managed: &Value,
    overrides: impl FnOnce(&mut AppConfigOverrides),
) -> (AppConfig, TempDir, TempDir, TempDir) {
    let home = tempfile::tempdir().expect("home dir");
    let cwd = tempfile::tempdir().expect("cwd dir");
    let managed_dir = tempfile::tempdir().expect("managed dir");
    write_managed_settings(managed_dir.path(), managed).await;

    let _guard = ManagedPathGuard::set(managed_dir.path());

    let mut opts = AppConfigOverrides {
        home_dir: Some(home.path().to_path_buf()),
        ..Default::default()
    };
    overrides(&mut opts);

    let config = AppConfig::load(cwd.path().to_path_buf(), opts)
        .await
        .expect("load app config");
    (config, home, cwd, managed_dir)
}

#[tokio::test]
async fn managed_policy_deny_rule_wins_over_user_allow() {
    let managed = json!({
        "permissions": { "deny": ["Bash"] }
    });
    let (config, _home, _cwd, _managed) = load_with_managed(&managed, |opts| {
        // The user explicitly allows Bash; the managed deny must still win.
        opts.allowed_tools = vec!["Bash".to_string()];
    })
    .await;

    assert!(
        config.disallowed_tools.iter().any(|rule| rule == "Bash"),
        "managed deny rule must be merged into disallowed_tools"
    );

    let ctx = PermissionContext::from_config(&config);
    assert!(
        ctx.tool_denied("Bash", "{\"command\":\"ls\"}").is_some(),
        "Bash must be denied even though the user allowlisted it"
    );
}

#[tokio::test]
async fn managed_policy_rules_only_drops_user_rules() {
    let managed = json!({
        "allowManagedPermissionRulesOnly": true,
        "permissions": { "deny": ["WebFetch"] }
    });
    let (config, _home, _cwd, _managed) = load_with_managed(&managed, |opts| {
        opts.allowed_tools = vec!["Bash".to_string()];
        opts.disallowed_tools = vec!["Read".to_string()];
    })
    .await;

    assert!(
        !config.allowed_tools.iter().any(|rule| rule == "Bash"),
        "user allow rules must be cleared when only managed rules are permitted"
    );
    assert!(
        !config.disallowed_tools.iter().any(|rule| rule == "Read"),
        "user deny rules must be cleared when only managed rules are permitted"
    );
    assert!(
        config
            .disallowed_tools
            .iter()
            .any(|rule| rule == "WebFetch"),
        "managed deny rule must survive the clear"
    );
}

#[tokio::test]
async fn managed_policy_bypass_mode_downgraded_when_disabled() {
    let managed = json!({
        "permissions": { "disableBypassPermissionsMode": "disable" }
    });
    let (config, _home, _cwd, _managed) = load_with_managed(&managed, |opts| {
        opts.permission_mode = Some(PermissionMode::BypassPermissions);
    })
    .await;

    assert_eq!(
        config.permission_mode,
        Some(PermissionMode::Default),
        "bypass mode must be downgraded to default under managed policy"
    );
    assert!(config.policy.disable_bypass_permissions_mode);
}

#[tokio::test]
async fn managed_policy_bypass_mode_preserved_without_policy() {
    let managed = json!({});
    let (config, _home, _cwd, _managed) = load_with_managed(&managed, |opts| {
        opts.permission_mode = Some(PermissionMode::BypassPermissions);
    })
    .await;

    assert_eq!(
        config.permission_mode,
        Some(PermissionMode::BypassPermissions),
        "bypass mode must be honored when no policy forbids it"
    );
}

#[tokio::test]
async fn managed_policy_mcp_allowlist_rejects_outside_servers() {
    let managed = json!({
        "allowedMcpServers": [{ "serverName": "alpha" }]
    });
    let (config, _home, _cwd, _managed) = load_with_managed(&managed, |_| {}).await;

    assert!(
        config.policy.mcp_server_allowed("alpha"),
        "server on the managed allowlist must be permitted"
    );
    assert!(
        !config.policy.mcp_server_allowed("beta"),
        "server outside the managed allowlist must be rejected at registration"
    );
}

#[tokio::test]
async fn managed_policy_mcp_denied_server_rejected() {
    let managed = json!({
        "allowedMcpServers": [{ "serverName": "alpha" }, { "serverName": "beta" }],
        "deniedMcpServers": [{ "serverName": "beta" }]
    });
    let (config, _home, _cwd, _managed) = load_with_managed(&managed, |_| {}).await;

    assert!(config.policy.mcp_server_allowed("alpha"));
    assert!(
        !config.policy.mcp_server_allowed("beta"),
        "a denied server must lose even if it also appears on the allowlist"
    );
}

#[tokio::test]
async fn managed_policy_hooks_only_flag_enforced_at_load() {
    let managed = json!({ "allowManagedHooksOnly": true });
    let (config, _home, _cwd, _managed) = load_with_managed(&managed, |_| {}).await;

    // `session_hooks::trusted_hook_matchers` returns no matchers whenever this
    // flag is set, so loading it correctly is the enforcement contract.
    assert!(
        config.policy.allow_managed_hooks_only,
        "allowManagedHooksOnly must be parsed so user/project hooks are dropped"
    );
}

#[tokio::test]
async fn managed_policy_hooks_only_defaults_off() {
    let (config, _home, _cwd, _managed) = load_with_managed(&json!({}), |_| {}).await;
    assert!(!config.policy.allow_managed_hooks_only);
}

#[tokio::test]
async fn managed_policy_forced_login_rejects_mismatch() {
    let managed = json!({ "forceLoginMethod": "claudeai" });
    let (config, home, _cwd, _managed) = load_with_managed(&managed, |_| {}).await;

    let forced = config
        .forced_login_method()
        .and_then(parse_forced_login_method);
    assert_eq!(forced, Some(AuthMethod::OAuthDevice));

    let manager = AuthManager::new(home.path().to_path_buf()).with_forced_login_method(forced);

    let rejected = manager
        .login(
            ProviderId::Anthropic,
            AuthMethod::ApiKey,
            Some("sk-test".to_string()),
            None,
        )
        .await;
    let error = rejected.expect_err("mismatched login method must be rejected");
    assert!(
        error.to_string().contains("locked by managed policy"),
        "rejection must cite managed policy: {error}"
    );

    let accepted = manager
        .login(
            ProviderId::Anthropic,
            AuthMethod::OAuthDevice,
            Some("oauth-token".to_string()),
            None,
        )
        .await;
    assert!(
        accepted.is_ok(),
        "the forced method itself must still be allowed to log in"
    );
}

#[tokio::test]
async fn managed_policy_locked_key_blocks_mutation() {
    let managed = json!({ "theme": "dark" });
    let (config, _home, _cwd, _managed) = load_with_managed(&managed, |_| {}).await;

    let error = config
        .ensure_setting_mutable("theme")
        .expect_err("a managed-pinned key must be locked");
    assert!(
        error.message.contains("locked by managed policy"),
        "lock error must cite managed policy: {error}"
    );
    // A key the managed layer never touches stays mutable.
    assert!(config.ensure_setting_mutable("editorMode").is_ok());
}
