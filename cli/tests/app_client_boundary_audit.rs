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

fn production_source(source: &str) -> &str {
    source
        .split_once("#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production)
}

#[test]
fn app_client_public_methods_never_return_raw_json() {
    let source = cli_source("../../app-server-client/src/lib.rs");
    let source = production_source(&source);
    let mut violations = Vec::new();
    for marker in ["pub async fn ", "pub fn "] {
        let mut rest = source;
        while let Some(start) = rest.find(marker) {
            let declaration = &rest[start..];
            let end = declaration
                .find('{')
                .unwrap_or_else(|| panic!("unterminated public function declaration"));
            let signature = &declaration[..end];
            let name = signature[marker.len()..]
                .split(['(', '<'])
                .next()
                .unwrap_or("<unknown>")
                .trim();
            let return_type = signature.rsplit_once("->").map(|(_, value)| value);
            if return_type.is_some_and(|value| {
                value.contains("Result<Value")
                    || value.contains("Result<serde_json::Value")
                    || value.trim_start().starts_with("Value")
                    || value.trim_start().starts_with("serde_json::Value")
            }) {
                violations.push(name.to_string());
            }
            rest = &declaration[end + 1..];
        }
    }
    assert!(
        violations.is_empty(),
        "public AppClient methods must return named DTOs, not raw JSON: {violations:?}"
    );
}

#[test]
fn production_clients_do_not_read_settings_files_directly() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let roots = [
        manifest.join("../app-server-client/src"),
        manifest.join("../tui/src"),
        manifest.join("src"),
    ];
    let mut files = Vec::new();
    for root in roots {
        collect_rs_files(&root, &mut files);
    }
    let mut violations = Vec::new();
    for path in files {
        if path.components().any(|part| part.as_os_str() == "tests") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read client source");
        let source = production_source(&source);
        for pattern in [
            "join(\"settings.json\")",
            "join(\".claude/settings",
            "join(\"managed-settings.json\")",
        ] {
            if source.contains(pattern) {
                violations.push(format!("{} contains {pattern}", path.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "clients must obtain settings through typed AppClient projections:\n{}",
        violations.join("\n")
    );
}

#[test]
fn stable_settings_consumers_do_not_reparse_typed_results() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = [
        manifest.join("../tui/src/app.rs"),
        manifest.join("../tui/src/state.rs"),
        manifest.join("../tui/src/editor_mode.rs"),
        manifest.join("../tui/src/overlays/appearance_picker.rs"),
        manifest.join("../tui/src/overlays/config_picker.rs"),
        manifest.join("../tui/src/overlays/model_picker.rs"),
        manifest.join("src/headless.rs"),
        manifest.join("src/control.rs"),
        manifest.join("src/acp_sdk/session_controls.rs"),
    ];
    let forbidden = [
        "ThemeSetting::parse(",
        "EditorModeSetting::parse(",
        "PermissionMode::parse(&result.mode)",
        ".settings_allowed_rules",
        ".settings_denied_rules",
        ".statusline_refresh_interval_secs",
    ];
    let mut violations = Vec::new();
    for path in files {
        let source = std::fs::read_to_string(&path).expect("read settings consumer");
        let source = production_source(&source);
        for pattern in forbidden {
            if source.contains(pattern) {
                violations.push(format!("{} contains {pattern}", path.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "stable settings consumers must use typed DTOs:\n{}",
        violations.join("\n")
    );
}

#[test]
fn statusline_execution_is_client_owned() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let server_files = [
        manifest.join("../config/src/claude_home.rs"),
        manifest.join("../app-server/src/bootstrap.rs"),
        manifest.join("../app-server/src/settings.rs"),
        manifest.join("../app-server/src/protocol_handler/settings.rs"),
    ];
    for path in server_files {
        let source = std::fs::read_to_string(&path).expect("read server settings source");
        assert!(
            !source.contains("run_statusline_command")
                && !source.contains("statusline_command).output")
                && !source.contains("Command::new(statusline"),
            "statusline execution leaked outside the client: {}",
            path.display()
        );
    }
    let tui = std::fs::read_to_string(manifest.join("../tui/src/app.rs")).expect("read TUI app");
    assert!(
        tui.contains("run_statusline_command"),
        "the TUI must remain the explicit statusline execution owner"
    );
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
    let main = cli_source("main.rs");
    assert!(
        source.contains("run_acp_adapter(client: Arc<AppClient>)"),
        "ACP must receive the canonical client instead of constructing or retaining AppServer"
    );
    assert!(
        main.contains(
            "Command::Acp => AppClient::new_option_only_persistent_goals(app_server.clone()).await"
        ),
        "the CLI composition root must declare ACP's option-only persistent-goal capability"
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
