//! End-to-end test for the minimal local web host shell.
//!
//! The host is TypeScript/Node and talks to `orbcode serve --websocket` using
//! only the generated app-server protocol. It must not link Rust `AppServer`.
//!
//! Ignored by default because it requires Node/npm and runs external TypeScript
//! tooling. Run with:
//!
//! `cargo test -p orbcode --test local_web_host_e2e -- --ignored`

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
fn local_web_host_smoke() {
    let dir = client_dir();
    if !ensure_npm_install(&dir) {
        return;
    }

    let result = Command::new("npx")
        .args(["tsx", "src/local-web-host.ts", "--smoke"])
        .current_dir(&dir)
        .env("ORBCODE_BIN", ORBCODE_BIN)
        .output()
        .expect("run local web host smoke test");

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    if !result.status.success() {
        panic!(
            "local web host smoke test failed.\n\
             stdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    assert!(
        stdout.contains("All local web host smoke tests passed"),
        "expected success message in output:\n{stdout}"
    );
}
