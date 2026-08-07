//! Release-tier desktop UI E2E over the generated TypeScript client.
//!
//! Cargo owns the `orbcode` test binary so the test-only mock provider feature
//! is present. Node drives the same controller used by the Tauri renderer over
//! a sequential WebSocket transport, covering product state independently of
//! privileged host IPC (which has its own Rust black-box suite).

use std::path::PathBuf;
use std::process::Command;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

#[test]
#[ignore = "release-tier test requires Node dependencies in clients/desktop"]
fn desktop_ui_productization_e2e() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf();
    let desktop = workspace.join("clients/desktop");
    if !desktop.join("node_modules/.bin/tsx").exists() {
        eprintln!("desktop UI E2E skipped: run npm install in clients/desktop");
        return;
    }

    let output = Command::new("npm")
        .args(["run", "test:ui-e2e"])
        .current_dir(&desktop)
        .env("ORBCODE_BIN", ORBCODE_BIN)
        .output()
        .expect("run desktop UI E2E");
    assert!(
        output.status.success(),
        "desktop UI E2E failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("desktop UI productization E2E passed"),
        "desktop UI E2E did not report completion",
    );
}
