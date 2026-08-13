//! End-to-end test that bash cwd tracking propagates across consecutive tool
//! calls. Uses `#tool:` + `#then:` stub provider chaining so the first bash
//! call changes the directory and the second observes the updated cwd.

use std::process::Command;

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn consecutive_bash_calls_propagate_cwd() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");

    let prompt = concat!(
        r#"#tool:bash {"command":"cd /tmp"}"#,
        r#" #then:bash {"command":"pwd"}"#,
    );

    let output = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("PROVIDER_TYPE", "anthropic")
        .env("ORBCODE_ALLOW_TOOLS", "true")
        .env_remove("ORBCODE_HOME")
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
        stdout.contains("Tool `bash` completed."),
        "stdout should include completed tool result, got:\n{stdout}"
    );
    assert!(
        stdout.contains("tmp"),
        "second bash pwd should report /tmp (cwd propagated from first call), got:\n{stdout}"
    );
}
