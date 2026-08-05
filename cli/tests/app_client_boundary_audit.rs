//! Boundary audit for CLI adapters that must consume the canonical app protocol.

use std::path::{Path, PathBuf};

fn cli_source(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(path))
        .unwrap_or_else(|error| panic!("read CLI source {path}: {error}"))
}

fn collect_rs_files(root: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(root).expect("read source directory") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            collect_rs_files(&path, files);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

#[test]
fn headless_business_operations_use_app_client() {
    let source = cli_source("headless.rs");
    let compact = source.split_whitespace().collect::<String>();
    for required in [
        "client.bootstrap(",
        "client.submit_turn_stream(",
        "client.permission_overview(",
        "client.respond_to_permission_request(",
        "client.set_permission_mode(",
    ] {
        assert!(
            compact.contains(required),
            "headless path must contain {required}"
        );
    }
    for forbidden in [
        "app_server.bootstrap(",
        "app_server.submit_turn(",
        "app_server.permission_overview(",
        "app_server.respond_to_permission_request(",
        "app_server.set_permission_mode(",
    ] {
        assert!(
            !compact.contains(forbidden),
            "headless path bypasses AppClient through {forbidden}"
        );
    }
}

#[test]
fn headless_stream_json_and_acp_have_no_raw_protocol_requests() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![
        manifest.join("src/headless.rs"),
        manifest.join("src/stream_json.rs"),
        manifest.join("src/control.rs"),
    ];
    collect_rs_files(&manifest.join("src/acp_sdk"), &mut files);

    let mut violations = Vec::new();
    for path in files {
        let source = std::fs::read_to_string(&path).expect("read adapter source");
        for forbidden in [
            ".request_typed(",
            ".transport().request(",
            "ClientTransport::request",
        ] {
            if source.contains(forbidden) {
                violations.push(format!("{} contains {forbidden}", path.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "production adapters must use named AppClient methods:\n{}",
        violations.join("\n")
    );
}

#[test]
fn acp_accepts_a_preconstructed_app_client() {
    let source = cli_source("acp_sdk/mod.rs");
    assert!(
        source.contains("run_acp_adapter(client: Arc<AppClient>)"),
        "ACP must receive the canonical client instead of constructing or retaining AppServer"
    );
    assert!(
        !source.contains("AppClient::new(app_server)"),
        "ACP client construction belongs at the CLI composition root"
    );
    let compact = source.split_whitespace().collect::<String>();
    assert!(
        compact.contains("state.client.acp_close_session("),
        "ACP cleanup must use its named protocol method"
    );
}

#[test]
fn stream_json_imports_canonical_events_directly() {
    let source = cli_source("stream_json.rs");
    assert!(source.contains("use orbcode_protocol::{"));
    assert!(
        !source.contains("use orbcode_app_server::{"),
        "stream-json adapter must not import client event/data types through AppServer"
    );
}
