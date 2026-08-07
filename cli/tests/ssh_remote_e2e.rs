//! Black-box checks for the public `orbcode remote --ssh` entry point.

use std::process::Command;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

#[test]
fn ssh_remote_rejects_unsafe_target_before_launching_openssh() {
    let output = Command::new(ORBCODE_BIN)
        .args(["remote", "--ssh=-oProxyCommand=touch /tmp/pwn"])
        .output()
        .expect("run orbcode remote --ssh");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid SSH remote configuration")
            && stderr.contains("must not start with '-'"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn ssh_remote_rejects_relative_remote_cwd_before_launching_openssh() {
    let output = Command::new(ORBCODE_BIN)
        .args([
            "remote",
            "--ssh",
            "example.invalid",
            "--remote-cwd",
            "relative/project",
        ])
        .output()
        .expect("run orbcode remote --ssh");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("remote cwd must be an absolute path"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn ssh_remote_help_documents_tokenless_stdio_path() {
    let output = Command::new(ORBCODE_BIN)
        .args(["remote", "--help"])
        .output()
        .expect("run remote help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "--ssh <TARGET>",
        "--remote-cwd <PATH>",
        "--remote-orbcode <PATH>",
        "--ssh-option <KEY=VALUE>",
    ] {
        assert!(
            stdout.contains(expected),
            "help missing {expected}: {stdout}"
        );
    }
}
