//! End-to-end coverage for provider/model resolution.
//!
//! Each test spawns the compiled `orbcode` binary against an isolated
//! `ORBCODE_HOME`, then asserts on `providers` subcommand output. No network or
//! live provider is required — only the read-only `providers` inspection path.
//!
//! Environment is sealed so a developer's shell variables (`ANTHROPIC_MODEL`,
//! `ANTHROPIC_DEFAULT_OPUS_MODEL`, etc.) do not bleed into assertions.

use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_orbcode");

struct Harness {
    home: TempDir,
    cwd: TempDir,
}

impl Harness {
    fn new(settings_json: &str) -> Self {
        let home = tempfile::tempdir().expect("home tempdir");
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        std::fs::write(home.path().join("settings.json"), settings_json)
            .expect("write settings.json");
        Self { home, cwd }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with_env(args, &[])
    }

    fn run_with_env(&self, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .current_dir(self.cwd.path())
            .env_clear()
            .env("ORBCODE_HOME", self.home.path())
            .env("HOME", self.home.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        cmd.output().expect("spawn orbcode")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

// --- Alias resolution ---

#[test]
fn sonnet_alias_resolves_to_default_sonnet_model() {
    let h = Harness::new(r#"{"model":"sonnet"}"#);
    let out = stdout(&h.run(&["providers"]));
    assert!(
        out.contains("Sonnet"),
        "sonnet alias must resolve to the Sonnet marketing label: {out}"
    );
    assert!(
        out.contains("thinking"),
        "sonnet must report thinking capability: {out}"
    );
}

#[test]
fn opus_alias_resolves_to_default_opus_model() {
    let h = Harness::new(r#"{"model":"opus"}"#);
    let out = stdout(&h.run(&["providers"]));
    assert!(
        out.contains("Opus 4.7"),
        "opus alias must resolve to Opus 4.7: {out}"
    );
}

#[test]
fn haiku_alias_resolves_to_default_haiku_model() {
    let h = Harness::new(r#"{"model":"haiku"}"#);
    let out = stdout(&h.run(&["providers"]));
    assert!(
        out.contains("Haiku"),
        "haiku alias must resolve to Haiku marketing label: {out}"
    );
}

#[test]
fn best_alias_resolves_to_opus() {
    let h = Harness::new(r#"{"model":"best"}"#);
    let out = stdout(&h.run(&["providers"]));
    assert!(
        out.contains("Opus 4.7"),
        "best alias must resolve to Opus: {out}"
    );
}

// --- Explicit model ID pass-through ---

#[test]
fn explicit_model_id_passes_through_without_marketing_label() {
    let h = Harness::new(r#"{"model":"claude-sonnet-4-5-20250929"}"#);
    let out = stdout(&h.run(&["providers"]));
    assert!(
        out.contains("claude-sonnet-4-5-20250929"),
        "explicit model id must appear in provider output: {out}"
    );
}

// --- Env override precedence ---

#[test]
fn anthropic_model_env_overrides_settings_model() {
    let h = Harness::new(r#"{"model":"haiku"}"#);
    let out = stdout(&h.run_with_env(&["providers"], &[("ANTHROPIC_MODEL", "opus")]));
    assert!(
        out.contains("Opus 4.7"),
        "ANTHROPIC_MODEL env must override settings.model: {out}"
    );
}

#[test]
fn provider_specific_family_env_overrides_builtin_default() {
    let h = Harness::new(r#"{"model":"sonnet"}"#);
    let out = stdout(&h.run_with_env(
        &["providers"],
        &[("ANTHROPIC_DEFAULT_SONNET_MODEL", "my-custom-sonnet-v2")],
    ));
    assert!(
        out.contains("my-custom-sonnet-v2"),
        "ANTHROPIC_DEFAULT_SONNET_MODEL must override the builtin: {out}"
    );
}

// --- OpenAI provider resolution ---

#[test]
fn openai_provider_resolves_family_defaults() {
    let h = Harness::new(r#"{"model":"sonnet","env":{"CLAUDE_CODE_USE_OPENAI":"true"}}"#);
    let out = stdout(&h.run(&["providers"]));
    assert!(
        out.contains("provider=openai"),
        "CLAUDE_CODE_USE_OPENAI must select openai as default provider: {out}"
    );
    assert!(
        out.contains("effort"),
        "openai provider must report effort capability: {out}"
    );
}

// --- Default (no model setting) ---

#[test]
fn no_model_setting_defaults_to_sonnet() {
    let h = Harness::new(r"{}");
    let out = stdout(&h.run(&["providers"]));
    assert!(
        out.contains("Sonnet"),
        "when no model is set, default must be Sonnet: {out}"
    );
}

// --- Custom model option env ---

#[test]
fn settings_env_custom_model_option_does_not_break_providers() {
    let h = Harness::new(
        r#"{"env":{"ANTHROPIC_CUSTOM_MODEL_OPTION":"my-org-model","ANTHROPIC_CUSTOM_MODEL_OPTION_NAME":"OrgModel"}}"#,
    );
    let out = stdout(&h.run(&["providers"]));
    assert!(
        out.contains("Sonnet"),
        "custom model option should not change default resolution: {out}"
    );
}

// --- Multi-provider routing (primary + fallback) ---

#[test]
fn fallback_provider_appears_in_provider_chain() {
    let h = Harness::new(r"{}");
    let out = stdout(&h.run_with_env(&["providers"], &[("ORBCODE_FALLBACK_PROVIDER", "openai")]));
    assert!(
        out.contains(r#"fallback=Some("openai")"#),
        "fallback provider must be reported in the active chain: {out}"
    );
}

// --- Disabled provider resolution ---

#[test]
fn disabled_provider_gemini_shows_in_active_chain_without_panic() {
    let h = Harness::new(r"{}");
    let output = h.run_with_env(&["providers"], &[("ORBCODE_PROVIDER", "gemini")]);
    assert!(
        output.status.success(),
        "providers must succeed with a disabled provider: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = stdout(&output);
    assert!(
        out.contains("provider=gemini"),
        "active chain must report gemini as the selected provider: {out}"
    );
}

#[test]
fn disabled_provider_grok_shows_in_active_chain_without_panic() {
    let h = Harness::new(r"{}");
    let output = h.run_with_env(&["providers"], &[("ORBCODE_PROVIDER", "grok")]);
    assert!(
        output.status.success(),
        "providers must succeed with a disabled provider: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = stdout(&output);
    assert!(
        out.contains("provider=grok"),
        "active chain must report grok as the selected provider: {out}"
    );
}

// --- availableModels policy filtering (via managed settings) ---

#[test]
fn available_models_policy_restricts_displayed_providers() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let managed = tempfile::tempdir().expect("managed");

    std::fs::write(home.path().join("settings.json"), r#"{"model":"haiku"}"#)
        .expect("write settings");
    std::fs::write(
        managed.path().join("managed-settings.json"),
        r#"{"availableModels":["opus","sonnet"]}"#,
    )
    .expect("write managed");

    let output = Command::new(BIN)
        .args(["providers"])
        .current_dir(cwd.path())
        .env_clear()
        .env("ORBCODE_HOME", home.path())
        .env("HOME", home.path())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("CLAUDE_CODE_MANAGED_SETTINGS_PATH", managed.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn");

    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "providers subcommand must succeed even with restricted models: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!out.is_empty(), "providers output must not be empty");
}
