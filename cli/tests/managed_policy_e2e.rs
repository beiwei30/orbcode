//! Black-box end-to-end coverage for managed-policy enforcement.
//!
//! Each test spawns the real compiled `orbcode` binary against an isolated
//! `ORBCODE_HOME` (user settings) and `CLAUDE_CODE_MANAGED_SETTINGS_PATH`
//! (enterprise policy), then asserts on the binary's own stdout/stderr. This
//! exercises the full `AppServer::new` load path exactly as a user would, with
//! no network or live provider required — only the read-only inspection
//! subcommands (`providers`, `mcp servers`, `auth login`) are used.
//!
//! Because every assertion runs in a separate child process with its own
//! environment, these tests are self-isolating and safe to run in parallel.

use std::process::{Command, Output};

use tempfile::TempDir;

const USER_SETTINGS: &str = r#"{
  "permissions": { "allow": ["Bash"] },
  "mcpServers": {
    "alpha": { "type": "http", "url": "https://alpha.example/mcp" },
    "beta":  { "type": "http", "url": "https://beta.example/mcp" }
  }
}"#;

/// A full enterprise policy: deny Bash, forbid bypass mode, force the
/// claudeai (OAuth) login method, and restrict MCP to the `alpha` server.
const MANAGED_POLICY: &str = r#"{
  "permissions": { "deny": ["Bash"], "disableBypassPermissionsMode": "disable" },
  "forceLoginMethod": "claudeai",
  "allowedMcpServers": [{ "serverName": "alpha" }],
  "allowManagedMcpServersOnly": true,
  "theme": "dark"
}"#;

struct Fixture {
    home: TempDir,
    cwd: TempDir,
    managed: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("home dir");
        let cwd = tempfile::tempdir().expect("cwd dir");
        let managed = tempfile::tempdir().expect("managed dir");
        std::fs::write(home.path().join("settings.json"), USER_SETTINGS).expect("user settings");
        std::fs::write(managed.path().join("managed-settings.json"), MANAGED_POLICY)
            .expect("managed settings");
        Self { home, cwd, managed }
    }

    /// Build a `orbcode` invocation rooted in the isolated home/cwd. When
    /// `with_policy` is false the managed path is unset, giving a clean
    /// baseline that proves the policy — not the fixture — drives behavior.
    fn command(&self, with_policy: bool, args: &[&str]) -> Command {
        let exe = env!("CARGO_BIN_EXE_orbcode");
        let mut cmd = Command::new(exe);
        cmd.current_dir(self.cwd.path())
            .env("ORBCODE_HOME", self.home.path())
            // Keep a developer's real config from leaking into the child.
            .env_remove("CLAUDE_CONFIG_DIR")
            .args(args);
        if with_policy {
            cmd.env("CLAUDE_CODE_MANAGED_SETTINGS_PATH", self.managed.path());
        } else {
            cmd.env_remove("CLAUDE_CODE_MANAGED_SETTINGS_PATH");
        }
        cmd
    }

    fn run(&self, with_policy: bool, args: &[&str]) -> Output {
        self.command(with_policy, args)
            .output()
            .expect("spawn orbcode")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn managed_deny_rule_is_enforced_via_providers() {
    let fixture = Fixture::new();

    let with = stdout(&fixture.run(true, &["providers"]));
    assert!(
        with.contains("deny_rules=1"),
        "managed deny rule must merge into the permission context: {with}"
    );

    let baseline = stdout(&fixture.run(false, &["providers"]));
    assert!(
        baseline.contains("deny_rules=0"),
        "without policy there is no managed deny rule: {baseline}"
    );
}

#[test]
fn bypass_permissions_mode_is_downgraded_under_policy() {
    let fixture = Fixture::new();
    let args = &["--permission-mode", "bypassPermissions", "providers"];

    let with = stdout(&fixture.run(true, args));
    assert!(
        with.contains("tools=false"),
        "bypass must be downgraded to default (tools stay gated): {with}"
    );

    let baseline = stdout(&fixture.run(false, args));
    assert!(
        baseline.contains("tools=true"),
        "without policy bypass actually elevates tool access: {baseline}"
    );
}

#[test]
fn mcp_servers_outside_allowlist_are_pruned() {
    let fixture = Fixture::new();

    let with = stdout(&fixture.run(true, &["mcp", "servers"]));
    assert!(
        with.contains("alpha"),
        "allowlisted server must remain registered: {with}"
    );
    assert!(
        !with.contains("beta"),
        "server outside the managed allowlist must be pruned: {with}"
    );

    let baseline = stdout(&fixture.run(false, &["mcp", "servers"]));
    assert!(
        baseline.contains("alpha") && baseline.contains("beta"),
        "without policy both configured servers are present: {baseline}"
    );
}

#[test]
fn forced_login_method_rejects_mismatch_but_allows_match() {
    let fixture = Fixture::new();

    let rejected = fixture.run(
        true,
        &[
            "auth",
            "login",
            "--provider",
            "anthropic",
            "--method",
            "api-key",
            "--token",
            "sk-test",
        ],
    );
    assert!(
        !rejected.status.success(),
        "a login method that violates policy must fail"
    );
    assert!(
        stderr(&rejected).contains("locked by managed policy"),
        "rejection must cite managed policy: {}",
        stderr(&rejected)
    );

    let accepted = fixture.run(
        true,
        &[
            "auth",
            "login",
            "--provider",
            "anthropic",
            "--method",
            "o-auth-device",
            "--token",
            "oauth-tok",
        ],
    );
    assert!(
        accepted.status.success(),
        "the forced method itself must still be able to log in: {}",
        stderr(&accepted)
    );
    assert!(stdout(&accepted).contains("oauth_device"));
}

#[test]
fn api_key_login_is_allowed_without_policy() {
    let fixture = Fixture::new();
    let accepted = fixture.run(
        false,
        &[
            "auth",
            "login",
            "--provider",
            "anthropic",
            "--method",
            "api-key",
            "--token",
            "sk-test",
        ],
    );
    assert!(
        accepted.status.success(),
        "without a forced-login policy api-key login must succeed: {}",
        stderr(&accepted)
    );
}
