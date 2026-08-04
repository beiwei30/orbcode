use std::process::Command;

#[test]
fn large_bash_output_stub_preview_is_bounded() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    let prompt = r#"#tool:bash {"command":"yes line | head -n 12000"}"#;

    let output = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("PROVIDER_TYPE", "anthropic")
        .env("ORBCODE_ALLOW_TOOLS", "true")
        .env_remove("CLAUDE_CODE_USE_OPENAI")
        .env_remove("ORBCODE_ALLOWED_TOOLS")
        .env_remove("ORBCODE_DISALLOWED_TOOLS")
        .arg("prompt")
        .arg(prompt)
        .output()
        .expect("run orbcode prompt");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "orbcode prompt failed\nstatus: {:?}\nstderr:\n{}\nstdout:\n{}",
        output.status.code(),
        stderr,
        stdout
    );
    assert!(
        stdout.len() < 6_000,
        "stub preview should stay bounded, got {} bytes",
        stdout.len()
    );
    assert!(
        stdout.lines().filter(|line| *line == "line").count() < 500,
        "stub preview should not replay the full Bash output"
    );
    assert!(
        stdout.contains("Tool `bash` completed."),
        "stdout should include completed tool result"
    );
    assert!(
        stdout.contains("Stub tool result preview truncated"),
        "stdout should explain stub-preview truncation"
    );
    assert!(
        stdout.contains("Bash output truncated for transcript safety"),
        "stdout should preserve Bash truncation diagnostic"
    );
    assert!(
        stdout.contains("Omitted 30139 characters."),
        "stdout should preserve the Bash omitted-character count"
    );
}
