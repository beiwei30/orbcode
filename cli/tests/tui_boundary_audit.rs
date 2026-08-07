//! TUI boundary audit — verifies the TUI protocol migration is complete.
//!
//! Acceptance criteria:
//! - run_tui accepts Arc<AppClient> ✓
//! - AppClient is transport-polymorphic ✓
//! - Split-core smoke tests pass ✓ (proven by split_core_smoke_e2e)
//! - remote CLI returns before local core construction
//! - TUI production code has no direct AppServer/CoreError/app_server fallback
//! - TUI production code has no `orbcode_app_server` imports or paths
//! - AppClient has no production embedded AppServer fallback
//! - TuiState production client handle is not optional

/// Prove that `run_tui` accepts `Arc<AppClient>` at compile time.
/// If `run_tui`'s signature regresses to accept `AppServer` directly,
/// this test will fail to compile.
#[allow(dead_code, unused_imports)]
fn compile_time_signature_check() {
    use orbcode_app_server_client::AppClient;
    use std::sync::Arc;
    // This function reference proves the signature accepts Arc<AppClient>.
    let _: fn(Arc<AppClient>, Option<String>) -> _ = orbcode_tui::run_tui;
}

/// Verify no TUI production code bypasses the protocol with direct server
/// access symbols. Production TUI must not reference the AppServer facade,
/// CoreError, app_server() accessors, or direct typed submit helpers.
#[test]
fn tui_no_direct_app_server_symbols_in_production() {
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../tui/src/"));
    let mut violations = Vec::new();
    collect_rs_violations(
        root,
        &["AppServer", "CoreError", "app_server(", "submit_turn_typed"],
        &mut violations,
    );
    assert!(
        violations.is_empty(),
        "TUI production code must route through AppClient protocol methods. Found:\n{}",
        violations.join("\n")
    );
}

#[test]
fn tui_no_app_server_re_exports_in_production() {
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../tui/src/"));
    let mut violations = Vec::new();
    collect_rs_violations(
        root,
        &["orbcode_app_server::", "use orbcode_app_server::"],
        &mut violations,
    );
    assert!(
        violations.is_empty(),
        "TUI production code must import DTOs/helpers from protocol, config, tools, or app-client crates. Found:\n{}",
        violations.join("\n")
    );
}

#[test]
fn app_client_has_no_production_embedded_fallback() {
    let lib = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../app-server-client/src/lib.rs"
    ))
    .expect("read app-server-client lib");
    assert!(
        !lib.contains("embedded_server"),
        "AppClient must not keep a production embedded_server fallback"
    );
    assert!(
        !lib.contains("remote submit_turn_stream requires"),
        "submit_turn_stream must work through protocol notifications for remote clients"
    );
}

#[test]
fn remote_cli_returns_before_local_core_construction() {
    let main = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("read cli main");
    let remote_pos = main
        .find("if let Command::Remote")
        .expect("remote early-return branch");
    let local_core_pos = main
        .find("AppServer::new")
        .expect("local AppServer construction");
    assert!(
        remote_pos < local_core_pos,
        "remote TUI must connect before any local AppServer/core construction"
    );
    let ssh_launcher_pos = main
        .find("SshRemoteConnection::connect")
        .expect("SSH remote launcher branch");
    assert!(
        ssh_launcher_pos < local_core_pos,
        "SSH remote TUI must launch before any local AppServer/core construction"
    );
}

#[test]
fn production_tui_state_client_is_not_optional() {
    let state =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../tui/src/state.rs"))
            .expect("read tui state");
    assert!(
        state.contains("#[cfg(not(test))]\npub(crate) type TuiClientHandle = Arc<AppClient>;"),
        "production TuiState client handle must be Arc<AppClient>"
    );
    assert!(
        !state.contains("pub(crate) client: Option<Arc<AppClient>>"),
        "production TuiState must not carry Option<Arc<AppClient>>"
    );
}

fn collect_rs_violations(root: &std::path::Path, patterns: &[&str], out: &mut Vec<String>) {
    for entry in std::fs::read_dir(root).expect("read dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some("tests") {
                continue;
            }
            collect_rs_violations(&path, patterns, out);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in contents.lines().enumerate() {
            if patterns.iter().any(|pattern| line.contains(pattern)) {
                out.push(format!("{}:{}:{line}", path.display(), index + 1));
            }
        }
    }
}
