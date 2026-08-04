//! End-to-end coverage for the cross-settings-layer hook discovery surfaced by
//! `orbcode doctor`'s `extension_load` check. Each test writes hook configuration
//! to disk in an isolated `CLAUDE_CONFIG_DIR` (and an isolated managed-settings
//! path), runs the real `orbcode` binary, and asserts the rendered `extension_load`
//! line reports the expected source/trust/validation for the discovered hooks.
//!
//! These guard the CLI -> AppConfig -> `discover_hooks` -> doctor wiring that the
//! `config::hooks` and `doctor` unit tests cannot exercise on their own (settings
//! layer loading, policy resolution, and agent-frontmatter parsing all run for
//! real here).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs `orbcode doctor` against an isolated home/cwd and returns
/// `(exited_zero, extension_load_line, full_stdout)`.
///
/// The managed-settings path is always pinned to an isolated (usually empty)
/// directory so the host's real `/Library/Application Support/ClaudeCode`
/// (or platform equivalent) never leaks into the result.
fn run_doctor(home: &Path, cwd: &Path, managed_dir: &Path) -> (bool, String, String) {
    let binary = env!("CARGO_BIN_EXE_orbcode");
    let output = Command::new(binary)
        .current_dir(cwd)
        .env("CLAUDE_CONFIG_DIR", home)
        .env("CLAUDE_CODE_MANAGED_SETTINGS_PATH", managed_dir)
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("PROVIDER_TYPE", "anthropic")
        .env_remove("ORBCODE_HOME")
        .env_remove("CLAUDE_CODE_USE_OPENAI")
        .arg("doctor")
        .output()
        .expect("run orbcode doctor");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stdout
        .lines()
        .find(|line| line.contains("extension_load"))
        .unwrap_or_else(|| {
            panic!("extension_load line missing from doctor output:\nstderr:\n{stderr}\nstdout:\n{stdout}")
        })
        .to_string();
    (output.status.success(), line, stdout)
}

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().expect("path has parent")).expect("create parent dir");
    fs::write(path, contents).expect("write file");
}

/// Sets up the standard isolated `home`, `cwd`, and (empty) managed dir under a
/// fresh tempdir, returning all three paths plus the tempdir guard.
fn scratch() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("temp scratch dir");
    let home = dir.path().join("home");
    let cwd = dir.path().join("cwd");
    let managed = dir.path().join("managed");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&cwd).expect("create cwd");
    fs::create_dir_all(&managed).expect("create managed");
    (dir, home, cwd, managed)
}

/// A valid project-layer command hook is discovered, listed with its layer, and
/// marked trusted + valid; with no warnings the check stays PASS.
#[test]
fn doctor_lists_trusted_valid_project_hook() {
    let (_dir, home, cwd, managed) = scratch();
    write(
        cwd.join(".claude").join("settings.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
    );

    let (ok, line, stdout) = run_doctor(&home, &cwd, &managed);

    assert!(
        ok,
        "doctor should exit 0 for an all-valid hook setup:\n{stdout}"
    );
    assert!(
        line.contains("extension_load") && line.trim_start().starts_with("PASS"),
        "expected a PASS extension_load line:\n{line}",
    );
    assert!(
        line.contains("hook(s) discovered"),
        "missing discovery segment:\n{line}"
    );
    assert!(
        line.contains("[project]"),
        "missing project-layer label:\n{line}"
    );
    assert!(line.contains("echo hi"), "missing command text:\n{line}");
    assert!(
        line.contains("(trusted, valid)"),
        "expected trusted+valid:\n{line}"
    );
}

/// An empty-command hook in the local layer fails load-time validation: the
/// check flips to WARN, emits a discovery warning, and still lists the hook with
/// an `invalid` marker.
#[test]
fn doctor_warns_on_invalid_local_hook_command() {
    let (_dir, home, cwd, managed) = scratch();
    write(
        cwd.join(".claude").join("settings.local.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Read","hooks":[{"type":"command","command":"  "}]}]}}"#,
    );

    let (ok, line, stdout) = run_doctor(&home, &cwd, &managed);

    // An invalid hook is recoverable (Warn), so doctor still exits 0.
    assert!(
        ok,
        "doctor should exit 0 when the only issue is a Warn:\n{stdout}"
    );
    assert!(
        line.trim_start().starts_with("WARN"),
        "expected a WARN extension_load line:\n{line}",
    );
    assert!(
        line.contains("[local]"),
        "missing local-layer label:\n{line}"
    );
    assert!(
        line.contains("invalid"),
        "expected an invalid marker:\n{line}"
    );
    assert!(
        line.contains("command is empty"),
        "expected the empty-command reason:\n{line}",
    );
}

/// Hooks declared in an agent's frontmatter are discovered as a contributed
/// source and retain their `agent:<name>` provenance.
#[test]
fn doctor_lists_agent_contributed_hook_with_provenance() {
    let (_dir, home, cwd, managed) = scratch();
    write(
        home.join("agents").join("worker.md"),
        "---\nname: worker\ndescription: do work\nhooks: {\"PreToolUse\": [{\"matcher\": \"Bash\", \"hooks\": [{\"type\": \"command\", \"command\": \"echo agent\"}]}]}\n---\nbody\n",
    );

    let (ok, line, stdout) = run_doctor(&home, &cwd, &managed);

    assert!(ok, "doctor should exit 0 for a valid agent hook:\n{stdout}");
    assert!(
        line.contains("hook(s) discovered"),
        "missing discovery segment:\n{line}"
    );
    assert!(
        line.contains("[agent:worker]"),
        "agent provenance must survive discovery:\n{line}",
    );
    assert!(
        line.contains("echo agent"),
        "missing agent command text:\n{line}"
    );
    assert!(
        line.contains("(trusted, valid)"),
        "expected trusted+valid:\n{line}"
    );
}

/// With `allowManagedHooksOnly` set in managed settings, only managed-layer
/// hooks stay trusted; a user-layer hook is still listed but marked untrusted.
#[test]
fn doctor_managed_only_policy_marks_user_hook_untrusted() {
    let (_dir, home, cwd, managed) = scratch();
    // User-layer hook: surfaced but gated to untrusted by the managed-only policy.
    write(
        home.join("settings.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo user"}]}]}}"#,
    );
    // Managed settings: enable the policy and contribute a managed hook.
    write(
        managed.join("managed-settings.json"),
        r#"{"allowManagedHooksOnly":true,"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo managed"}]}]}}"#,
    );

    let (_ok, line, stdout) = run_doctor(&home, &cwd, &managed);

    assert!(
        line.contains("hook(s) discovered"),
        "missing discovery segment:\n{stdout}"
    );
    // Order of discovery is User -> ... -> Managed, so split on the user hook.
    assert!(
        line.contains("[user] PreToolUse (Bash) -> echo user (untrusted, valid)"),
        "user hook must be untrusted under managed-only policy:\n{line}",
    );
    assert!(
        line.contains("[managed] PreToolUse (Bash) -> echo managed (trusted, valid)"),
        "managed hook must stay trusted under managed-only policy:\n{line}",
    );
}

/// Helpers for plugin-based e2e tests: sets up the installed_plugins.json V2
/// index and user-level enabledPlugins entry pointing at `plugin_root`.
fn setup_plugin(home: &Path, plugin_root: &Path) {
    let index = format!(
        r#"{{"version":2,"plugins":{{"demo@market":[{{"scope":"user","installPath":"{}","version":"1.0.0"}}]}}}}"#,
        plugin_root.display(),
    );
    write(home.join("plugins").join("installed_plugins.json"), &index);
    write(
        plugin_root.join(".claude-plugin").join("plugin.json"),
        r#"{"name":"demo","version":"1.0.0"}"#,
    );
    write(
        home.join("settings.json"),
        r#"{"enabledPlugins":{"demo@market":true}}"#,
    );
}

/// A valid plugin hook is discovered with `plugin:<id>` provenance, marked
/// trusted and valid; doctor stays PASS.
#[test]
fn doctor_lists_plugin_contributed_hook_with_provenance() {
    let (_dir, home, cwd, managed) = scratch();
    let plugin_root = _dir.path().join("cache").join("demo").join("1.0.0");
    setup_plugin(&home, &plugin_root);
    write(
        plugin_root.join("hooks").join("hooks.json"),
        r#"{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo plugin"}]}]}"#,
    );

    let (ok, line, stdout) = run_doctor(&home, &cwd, &managed);

    assert!(
        ok,
        "doctor should exit 0 for a valid plugin hook:\n{stdout}"
    );
    assert!(
        line.contains("hook(s) discovered"),
        "missing discovery segment:\n{line}"
    );
    assert!(
        line.contains("[plugin:demo@market]"),
        "plugin provenance must survive discovery:\n{line}",
    );
    assert!(
        line.contains("echo plugin"),
        "missing plugin command text:\n{line}"
    );
    assert!(
        line.contains("(trusted, valid)"),
        "expected trusted+valid:\n{line}"
    );
}

/// A malformed hooks.json in a plugin directory produces a WARN with a
/// readable warning message; doctor still exits 0.
#[test]
fn doctor_warns_on_malformed_plugin_hooks_json() {
    let (_dir, home, cwd, managed) = scratch();
    let plugin_root = _dir.path().join("cache").join("demo").join("1.0.0");
    setup_plugin(&home, &plugin_root);
    write(
        plugin_root.join("hooks").join("hooks.json"),
        "NOT VALID JSON {{{",
    );

    let (ok, line, stdout) = run_doctor(&home, &cwd, &managed);

    assert!(
        ok,
        "doctor should exit 0 (warn, not fail) for malformed hooks.json:\n{stdout}"
    );
    assert!(
        line.trim_start().starts_with("WARN"),
        "expected WARN status for malformed plugin hooks:\n{line}",
    );
    assert!(
        line.contains("demo@market"),
        "warning should identify the plugin:\n{line}",
    );
    assert!(
        line.contains("malformed hooks.json"),
        "warning should describe the problem:\n{line}",
    );
}
