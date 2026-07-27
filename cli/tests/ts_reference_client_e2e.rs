//! End-to-end test that runs the TypeScript reference client against
//! `orbcode serve --stdio`, proving the protocol contract is consumable
//! from an external language without Rust facade linking.
//!
//! Ignored by default because it requires Node/npm and runs external TypeScript
//! tooling. Run with:
//!
//! `cargo test -p orbcode --test ts_reference_client_e2e -- --ignored`
//!
//! When explicitly run, skipped if Node.js/npx is not installed (unless
//! `ORBCODE_REQUIRE_NODE` is set, in which case missing Node panics the test).

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

fn require_node() -> bool {
    std::env::var("ORBCODE_REQUIRE_NODE").is_ok()
}

fn skip_or_fail(reason: &str) {
    if require_node() {
        panic!("FAIL (ORBCODE_REQUIRE_NODE set): {reason}");
    }
    eprintln!("SKIP: {reason}");
}

fn has_npx() -> bool {
    Command::new("npx")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn client_dir() -> PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap().join("clients/typescript")
}

static NPM_INSTALL_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

/// Returns true if setup succeeded. Runs `npm install` at most once
/// across all tests in this binary (safe under concurrent test threads).
fn ensure_npm_install(dir: &PathBuf) -> bool {
    if !has_npx() {
        skip_or_fail("npx not found");
        return false;
    }
    let result = NPM_INSTALL_RESULT.get_or_init(|| {
        let install = Command::new("npm")
            .args(["install", "--silent"])
            .current_dir(dir)
            .output()
            .expect("npm install");
        if install.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&install.stderr).trim().to_string())
        }
    });
    match result {
        Ok(()) => true,
        Err(msg) => {
            skip_or_fail(&format!("npm install failed: {msg}"));
            false
        }
    }
}

#[test]
#[ignore = "requires Node/npm and TypeScript tooling"]
fn ts_client_tsc_type_check() {
    let dir = client_dir();
    if !ensure_npm_install(&dir) {
        return;
    }

    let result = Command::new("npx")
        .args(["tsc", "--noEmit"])
        .current_dir(&dir)
        .output()
        .expect("tsc --noEmit");

    if !result.status.success() {
        let stdout = String::from_utf8_lossy(&result.stdout);
        panic!("tsc --noEmit failed:\n{stdout}");
    }
}

#[test]
#[ignore = "requires Node/npm and TypeScript tooling"]
fn ts_generated_protocol_tsc_type_check() {
    let dir = client_dir();
    if !ensure_npm_install(&dir) {
        return;
    }

    let protocol_ts = dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("app-server-protocol/tests/generated/typescript/protocol.ts");

    let result = Command::new("npx")
        .args(["tsc", "--noEmit"])
        .arg(&protocol_ts)
        .current_dir(&dir)
        .output()
        .expect("tsc --noEmit protocol.ts");

    if !result.status.success() {
        let stdout = String::from_utf8_lossy(&result.stdout);
        panic!("Generated protocol.ts fails tsc:\n{stdout}");
    }
}

#[test]
#[ignore = "requires Node/npm and TypeScript tooling"]
fn ts_reference_client_stdio_smoke() {
    let dir = client_dir();
    if !ensure_npm_install(&dir) {
        return;
    }

    let result = Command::new("npx")
        .args(["tsx", "src/smoke-stdio.ts"])
        .current_dir(&dir)
        .env("ORBCODE_BIN", ORBCODE_BIN)
        .output()
        .expect("run TS smoke test");

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    if !result.status.success() {
        panic!(
            "TS reference client stdio smoke test failed.\n\
             stdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    assert!(
        stdout.contains("All stdio smoke tests passed"),
        "expected success message in output:\n{stdout}"
    );
}

#[test]
#[ignore = "requires Node/npm and TypeScript tooling"]
fn ts_reference_client_websocket_smoke() {
    let dir = client_dir();
    if !ensure_npm_install(&dir) {
        return;
    }

    let result = Command::new("npx")
        .args(["tsx", "src/smoke-ws.ts"])
        .current_dir(&dir)
        .env("ORBCODE_BIN", ORBCODE_BIN)
        .output()
        .expect("run TS WS smoke test");

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    if !result.status.success() {
        if stdout.contains("SKIP:") {
            eprintln!("TS WS test skipped (runtime): {stdout}");
            return;
        }
        panic!(
            "TS reference client WS smoke test failed.\n\
             stdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    assert!(
        stdout.contains("All WebSocket smoke tests passed"),
        "expected success message in output:\n{stdout}"
    );
}

#[test]
#[ignore = "requires Node/npm and TypeScript tooling"]
fn ts_server_request_permission_deny_smoke() {
    let dir = client_dir();
    if !ensure_npm_install(&dir) {
        return;
    }

    let result = Command::new("npx")
        .args(["tsx", "src/smoke-server-requests.ts"])
        .current_dir(&dir)
        .env("ORBCODE_BIN", ORBCODE_BIN)
        .output()
        .expect("run TS server-request smoke test");

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    if !result.status.success() {
        if stdout.contains("SKIP:") {
            eprintln!("TS server-request test skipped (runtime): {stdout}");
            return;
        }
        panic!(
            "TS server-request smoke test failed.\n\
             stdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    assert!(
        stdout.contains("All server-request smoke tests passed"),
        "expected success message in output:\n{stdout}"
    );
}

#[test]
#[ignore = "requires Node/npm and TypeScript tooling"]
fn ts_host_productization_smoke() {
    let dir = client_dir();
    if !ensure_npm_install(&dir) {
        return;
    }

    let result = Command::new("npx")
        .args(["tsx", "src/smoke-host-productization.ts"])
        .current_dir(&dir)
        .env("ORBCODE_BIN", ORBCODE_BIN)
        .output()
        .expect("run TS host productization smoke test");

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    if !result.status.success() {
        if stdout.contains("SKIP:") {
            eprintln!("TS host productization test skipped (runtime): {stdout}");
            return;
        }
        panic!(
            "TS host productization smoke test failed.\n\
             stdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    assert!(
        stdout.contains("All host productization smoke tests passed"),
        "expected success message in output:\n{stdout}"
    );
}
