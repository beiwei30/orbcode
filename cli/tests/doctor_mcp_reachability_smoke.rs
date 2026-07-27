use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;

/// Configures a trusted remote MCP server pointing at a definitely-closed local
/// port, enables the opt-in probe with `ORBCODE_DOCTOR_MCP_PROBE=1`, runs the real
/// `orbcode doctor`, and asserts the `mcp_reachability` check reports WARN naming
/// the server and reason. Guards the CLI -> AppServer -> McpRegistry wiring that
/// the `mcp_reachability_check` unit tests don't exercise end to end.
#[test]
fn doctor_mcp_reachability_warns_for_unreachable_server() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let home = scratch.path().join("home");
    let cwd = scratch.path().join("cwd");
    fs::create_dir_all(&home).expect("create home dir");
    fs::create_dir_all(&cwd).expect("create cwd dir");

    // Claim then drop a local port so the endpoint is closed -> connection
    // refused, which fails fast instead of stalling the probe timeout.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind temp port");
    let endpoint = format!("http://{}/mcp", listener.local_addr().expect("local addr"));
    drop(listener);

    fs::write(
        cwd.join(".mcp.json"),
        format!(
            r#"{{
  "mcpServers": {{
    "dead-remote": {{
      "type": "http",
      "url": "{endpoint}"
    }}
  }}
}}"#
        ),
    )
    .expect("write .mcp.json");

    let binary = env!("CARGO_BIN_EXE_orbcode");
    trust_mcp_server(binary, &home, &cwd, "dead-remote");

    let output = Command::new(binary)
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("ORBCODE_DOCTOR_MCP_PROBE", "1")
        .env_remove("ORBCODE_HOME")
        .arg("doctor")
        .output()
        .expect("run orbcode doctor");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Reachability problems are Warn, so doctor still exits 0.
    assert!(
        output.status.success(),
        "doctor should exit 0 when the only issue is an unreachable MCP server\nstatus: {:?}\nstderr:\n{stderr}\nstdout:\n{stdout}",
        output.status.code(),
    );

    let line = stdout
        .lines()
        .find(|line| line.trim_start().contains("mcp_reachability"))
        .unwrap_or_else(|| panic!("mcp_reachability line missing from doctor output:\n{stdout}"));

    assert!(
        line.trim_start().starts_with("WARN"),
        "expected WARN: {line}"
    );
    assert!(
        line.contains("dead-remote"),
        "should name the server: {line}"
    );
    assert!(
        line.contains("unreachable"),
        "should explain reason: {line}"
    );
}

fn trust_mcp_server(binary: &str, home: &Path, cwd: &Path, server_id: &str) {
    let output = Command::new(binary)
        .current_dir(cwd)
        .env("CLAUDE_CONFIG_DIR", home)
        .env_remove("ORBCODE_HOME")
        .arg("mcp")
        .arg("trust")
        .arg(server_id)
        .output()
        .expect("trust MCP server");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "mcp trust failed\nstderr:\n{stderr}\nstdout:\n{stdout}"
    );
}
