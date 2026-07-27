//! End-to-end integration test exercising the full config-layer flow:
//!
//! 1. Set up a temp home with a broken agent file, a valid agent file, a
//!    malformed plugin manifest, and a managed layer that locks `outputStyle`.
//! 2. Load settings layers → build effective policy.
//! 3. Load agent definitions with warnings.
//! 4. Load plugin registry.
//! 5. Verify:
//!    - Agent warnings surface with correct kinds and are serializable.
//!    - Plugin warnings surface and are serializable.
//!    - `ensure_setting_mutable("outputStyle")` fails.
//!    - `managed_forced_output_style` returns the forced value.
//!    - Valid agents still load correctly despite broken siblings.

use std::path::Path;

use crate::agents::{AgentWarningKind, load_agent_definitions_with_warnings};
use crate::output_styles::{
    built_in_output_style_definitions, managed_forced_output_style, resolve_active_output_style,
};
use crate::plugins::{PluginLoadWarning, load_plugin_registry};
use crate::policy::{ManagedPathGuard, effective_policy, load_settings_layers};

async fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.unwrap();
    }
    tokio::fs::write(path, contents).await.unwrap();
}

// ManagedPathGuard holds a std Mutex across .await to serialize tests that
// mutate the CLAUDE_CODE_MANAGED_SETTINGS_PATH env var.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn full_flow_warnings_and_managed_output_style_lock() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let cwd = temp.path().join("project");
    let managed_dir = temp.path().join("managed");

    // -- Managed layer: force outputStyle to "Explanatory" --
    write_file(
        &managed_dir.join("managed-settings.json"),
        r#"{"outputStyle": "Explanatory"}"#,
    )
    .await;
    let _guard = ManagedPathGuard::set(&managed_dir);

    // -- Agent files: one valid, one broken (missing description) --
    let agents_dir = home.join("agents");
    write_file(
        &agents_dir.join("good-agent.md"),
        "---\nname: good-agent\ndescription: A valid agent\ntools: Read, Grep\n---\nYou are a good agent.\n",
    )
    .await;
    write_file(
        &agents_dir.join("broken-agent.md"),
        "---\nname: broken-agent\n---\nmissing description field\n",
    )
    .await;

    // -- Plugin setup: enabled plugin with malformed manifest --
    let plugin_root = temp.path().join("cache").join("bad-plugin").join("1.0.0");
    write_file(
        &plugin_root.join(".claude-plugin").join("plugin.json"),
        "{ this is not valid JSON at all",
    )
    .await;
    let index = format!(
        r#"{{"version":2,"plugins":{{"bad-plugin@market":[{{"scope":"user","installPath":"{}","version":"1.0.0"}}]}}}}"#,
        plugin_root.display(),
    );
    write_file(&home.join("plugins").join("installed_plugins.json"), &index).await;
    write_file(
        &home.join("settings.json"),
        r#"{"enabledPlugins":{"bad-plugin@market":true}}"#,
    )
    .await;

    // === Step 1: Load settings layers and build effective policy ===
    let layers = load_settings_layers(&home, &cwd).await.expect("layers");
    let policy = effective_policy(&layers);

    // Verify outputStyle is locked by managed policy.
    assert!(
        policy.managed_locked_keys.contains("outputStyle"),
        "outputStyle must be in managed_locked_keys"
    );
    let lock_err = policy
        .ensure_setting_mutable("outputStyle")
        .expect_err("outputStyle must be locked");
    assert_eq!(lock_err.key, "outputStyle");
    assert!(lock_err.message.contains("locked by managed policy"));

    // Verify managed_forced_output_style extracts the value.
    let forced = managed_forced_output_style(&layers);
    assert_eq!(forced, Some("Explanatory".to_string()));

    // Verify the forced style resolves to an actual definition.
    let definitions = built_in_output_style_definitions();
    let resolved = resolve_active_output_style(&definitions, forced.as_deref().unwrap());
    assert!(resolved.matched);
    assert_eq!(resolved.definition.name, "Explanatory");

    // === Step 2: Load agent definitions with warnings ===
    let agent_outcome = load_agent_definitions_with_warnings(&home, &cwd)
        .await
        .expect("agent load must not panic");

    // Valid agent loaded successfully.
    assert!(
        agent_outcome
            .definitions
            .iter()
            .any(|a| a.agent_type == "good-agent"),
        "valid agent must be present"
    );
    // Broken agent was skipped.
    assert!(
        !agent_outcome
            .definitions
            .iter()
            .any(|a| a.agent_type == "broken-agent"),
        "broken agent must not appear in definitions"
    );
    // Warning was emitted for the broken agent.
    let agent_warnings: Vec<_> = agent_outcome
        .warnings
        .iter()
        .filter(|w| w.kind == AgentWarningKind::MissingField)
        .collect();
    assert!(
        !agent_warnings.is_empty(),
        "must have at least one MissingField warning"
    );
    // Warnings are serializable.
    for w in &agent_outcome.warnings {
        let json = serde_json::to_value(w).expect("warning must serialize");
        assert!(json.get("kind").is_some());
        assert!(json.get("message").is_some());
    }

    // === Step 3: Load plugin registry ===
    let registry = load_plugin_registry(&home, &cwd).await.expect("registry");

    // Plugin loaded despite malformed manifest (no panic).
    assert_eq!(registry.plugins.len(), 1);
    assert!(registry.plugins[0].enabled);

    // Warning was emitted for the bad manifest.
    assert!(
        !registry.warnings.is_empty(),
        "malformed manifest must produce a warning"
    );
    let plugin_warning: &PluginLoadWarning = &registry.warnings[0];
    assert_eq!(plugin_warning.plugin_id, "bad-plugin@market");
    assert!(
        plugin_warning.message.contains("not valid JSON"),
        "warning message: {}",
        plugin_warning.message
    );

    // Plugin warnings are serializable.
    let json = serde_json::to_value(plugin_warning).expect("plugin warning must serialize");
    assert_eq!(json["plugin_id"], "bad-plugin@market");
}
