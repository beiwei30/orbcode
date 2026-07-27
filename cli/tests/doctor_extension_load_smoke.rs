use std::fs;
use std::process::Command;

/// Drops a missing-field agent, an unclosed-frontmatter output style, and an
/// enabled plugin with an unparseable `plugin.json` into an isolated
/// `CLAUDE_CONFIG_DIR`, runs `orbcode doctor`, and asserts the `extension_load`
/// check reports WARN with one diagnostic per loader. Guards the CLI →
/// AppServer → extension-loader wiring that the `extension_load_check` unit
/// tests alone don't exercise.
#[test]
fn doctor_extension_load_check_surfaces_warnings() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let home = scratch.path().join("home");
    let cwd = scratch.path().join("cwd");
    fs::create_dir_all(&home).expect("create home dir");
    fs::create_dir_all(&cwd).expect("create cwd dir");

    // Agent: frontmatter present but `description` missing -> skipped + warning.
    let agents = home.join("agents");
    fs::create_dir_all(&agents).expect("create agents dir");
    fs::write(agents.join("broken.md"), "---\nname: broken\n---\nbody\n")
        .expect("write broken agent");

    // Output style: `---` block opened but never closed -> skipped + warning.
    let styles = home.join("output-styles");
    fs::create_dir_all(&styles).expect("create output-styles dir");
    fs::write(
        styles.join("Broken.md"),
        "---\nname: Broken\ndescription: d\nno closing delimiter\n",
    )
    .expect("write broken output style");

    // Plugin: enabled but `plugin.json` is not valid JSON -> loaded + warning.
    let plugin_root = scratch.path().join("cache").join("demo").join("1.0.0");
    fs::create_dir_all(plugin_root.join(".claude-plugin")).expect("create plugin manifest dir");
    fs::write(
        plugin_root.join(".claude-plugin").join("plugin.json"),
        "{ this is not valid json",
    )
    .expect("write malformed plugin manifest");
    fs::create_dir_all(home.join("plugins")).expect("create plugins dir");
    fs::write(
        home.join("plugins").join("installed_plugins.json"),
        format!(
            r#"{{"version":2,"plugins":{{"demo@market":[{{"scope":"user","installPath":"{}","version":"1.0.0"}}]}}}}"#,
            plugin_root.display()
        ),
    )
    .expect("write installed_plugins.json");
    fs::write(
        home.join("settings.json"),
        r#"{"enabledPlugins":{"demo@market":true}}"#,
    )
    .expect("write settings.json");

    let binary = env!("CARGO_BIN_EXE_orbcode");
    let output = Command::new(binary)
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("ORBCODE_PROVIDER", "anthropic")
        .env_remove("ORBCODE_HOME")
        .env_remove("CLAUDE_CODE_USE_OPENAI")
        .arg("doctor")
        .output()
        .expect("run orbcode doctor");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Doctor exits non-zero only on Fail; these are all recoverable Warns.
    assert!(
        output.status.success(),
        "doctor should exit 0 when issues are recoverable\nstatus: {:?}\nstderr:\n{}\nstdout:\n{}",
        output.status.code(),
        stderr,
        stdout,
    );

    let line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("WARN extension_load"))
        .unwrap_or_else(|| {
            panic!("WARN extension_load line missing from doctor output:\n{stdout}")
        });

    let assert_contains = |needle: &str, label: &str| {
        assert!(
            line.contains(needle),
            "{label}: expected substring '{needle}' in extension_load line:\n{line}\nfull stdout:\n{stdout}",
        );
    };

    assert_contains("3 extension load warning(s)", "aggregate count");
    assert_contains("agent [", "agent source line");
    assert_contains("plugin [", "plugin source line");
    assert_contains("output-style [", "output-style source line");
    assert_contains("description", "agent missing-field reason");
    assert_contains("not valid JSON", "plugin manifest reason");
}
