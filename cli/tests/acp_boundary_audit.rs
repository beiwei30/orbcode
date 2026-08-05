//! Architectural guardrails for the ACP edge adapter.

#[test]
fn acp_production_code_uses_only_the_app_client_boundary() {
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/acp_sdk"));
    let forbidden = [
        "orbcode_core",
        "orbcode_tools",
        "orbcode_mcp",
        "orbcode_model_provider",
        "orbcode_config",
        "orbcode_session_store",
        "orbcode_app_server::",
    ];
    let mut violations = Vec::new();
    collect_rs_violations(root, &forbidden, &mut violations);
    assert!(
        violations.is_empty(),
        "ACP production code must reach canonical behavior only through AppClient. Found:\n{}",
        violations.join("\n")
    );
}

#[test]
fn acp_state_does_not_store_app_server() {
    let source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/acp_sdk/mod.rs"))
            .expect("read ACP adapter");
    let state_start = source.find("struct AcpSdkState").expect("ACP state");
    let state_body = &source[state_start..];
    let state_end = state_body.find("\n}").expect("ACP state end");
    assert!(
        !state_body[..state_end].contains("AppServer"),
        "AcpSdkState must contain only AppClient and adapter-local state"
    );
}

fn collect_rs_violations(root: &std::path::Path, patterns: &[&str], out: &mut Vec<String>) {
    for entry in std::fs::read_dir(root).expect("read ACP source directory") {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_violations(&path, patterns, out);
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let contents = std::fs::read_to_string(&path).expect("read ACP source file");
        for (line_number, line) in contents.lines().enumerate() {
            if patterns.iter().any(|pattern| line.contains(pattern)) {
                out.push(format!("{}:{}:{line}", path.display(), line_number + 1));
            }
        }
    }
}
