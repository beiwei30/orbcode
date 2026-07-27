use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

#[test]
fn mcp_stdio_config_server_lists_and_calls_real_tools() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    write_mcp_json(workspace.path(), fake_stdio_server_binary());
    trust_mcp_server(binary, home.path(), workspace.path(), "fake");

    let tools = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("--allow-tools")
        .arg("true")
        .arg("mcp")
        .arg("tools")
        .arg("fake")
        .output()
        .expect("run orbcode mcp tools");
    let stdout = String::from_utf8_lossy(&tools.stdout);
    let stderr = String::from_utf8_lossy(&tools.stderr);
    assert!(
        tools.status.success(),
        "mcp tools failed\nstderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(stdout.contains("echo  Echo test input."), "{stdout}");
    assert!(
        !stdout.contains("inspect"),
        "configured stdio tools should come from real tools/list, not modeled stored tools: {stdout}"
    );

    let call = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("--allow-tools")
        .arg("true")
        .arg("mcp")
        .arg("call")
        .arg("fake")
        .arg("echo")
        .arg(r#"{"text":"cli"}"#)
        .output()
        .expect("run orbcode mcp call");
    let stdout = String::from_utf8_lossy(&call.stdout);
    let stderr = String::from_utf8_lossy(&call.stderr);
    assert!(
        call.status.success(),
        "mcp call failed\nstderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(stdout.contains("echo: cli"), "{stdout}");
}

#[test]
fn mcp_stdio_config_server_lists_and_reads_resources_and_prompts() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    write_mcp_json(workspace.path(), fake_stdio_server_binary());
    trust_mcp_server(binary, home.path(), workspace.path(), "fake");

    let resources = run_mcp_command(
        binary,
        home.path(),
        workspace.path(),
        &["mcp", "resources", "fake"],
    );
    assert!(
        resources.contains("res://text") && resources.contains("res://binary"),
        "resources should list real remote entries: {resources}"
    );

    let text = run_mcp_command(
        binary,
        home.path(),
        workspace.path(),
        &["mcp", "read", "fake", "res://text"],
    );
    assert!(text.contains("resource body"), "{text}");
    assert!(
        !text.contains("binary"),
        "text resource must stay on the text path: {text}"
    );

    let binary_out = run_mcp_command(
        binary,
        home.path(),
        workspace.path(),
        &["mcp", "read", "fake", "res://binary"],
    );
    assert!(
        binary_out.contains("[binary application/octet-stream") && binary_out.contains("aGVsbG8="),
        "binary resource must surface base64 blob via the is_binary path: {binary_out}"
    );

    let prompts = run_mcp_command(
        binary,
        home.path(),
        workspace.path(),
        &["mcp", "prompts", "fake"],
    );
    assert!(
        prompts.contains("greet") && prompts.contains("name*"),
        "prompts should list real prompts with required args: {prompts}"
    );

    let prompt = run_mcp_command(
        binary,
        home.path(),
        workspace.path(),
        &["mcp", "prompt", "fake", "greet", r#"{"name":"world"}"#],
    );
    assert!(
        prompt.contains("user: Hello, world!"),
        "prompt should render with substituted arguments: {prompt}"
    );

    let embedded = run_mcp_command(
        binary,
        home.path(),
        workspace.path(),
        &["mcp", "prompt", "fake", "embed"],
    );
    assert!(
        embedded.contains("user: embedded body"),
        "embedded resource content should render its nested text: {embedded}"
    );
}

#[test]
fn mcp_resources_and_prompts_blocked_until_trusted() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    write_mcp_json(workspace.path(), fake_stdio_server_binary());
    // No `mcp trust`: a `.mcp.json`-sourced server defaults to Unknown trust.

    for args in [["mcp", "resources", "fake"], ["mcp", "prompts", "fake"]] {
        let output = Command::new(binary)
            .current_dir(workspace.path())
            .env("CLAUDE_CONFIG_DIR", home.path())
            .arg("--allow-tools")
            .arg("true")
            .args(args)
            .output()
            .expect("run orbcode mcp command");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "untrusted server should fail for {args:?}\nstdout:\n{stdout}"
        );
        assert!(
            stderr.contains("not trusted") || stdout.contains("not trusted"),
            "untrusted server should report a trust hint for {args:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            !stdout.contains("res://") && !stdout.contains("greet"),
            "untrusted server must not leak content for {args:?}\nstdout:\n{stdout}"
        );
    }
}

#[test]
fn mcp_trust_persists_in_settings_layer_across_fresh_app_servers() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    write_mcp_json(workspace.path(), fake_stdio_server_binary());

    // A `.mcp.json`-sourced server is untrusted until approved.
    let before = run_mcp_command(binary, home.path(), workspace.path(), &["mcp", "servers"]);
    assert!(
        before.contains("fake") && before.contains("trust=unknown"),
        "server should start untrusted\n{before}"
    );

    // Approve it: this writes both the legacy trust.json and the settings layer.
    trust_mcp_server(binary, home.path(), workspace.path(), "fake");

    // The decision lands in the User settings layer using the TS-compatible key.
    let settings = std::fs::read_to_string(home.path().join("settings.json"))
        .expect("read settings.json after trust");
    let settings: serde_json::Value = serde_json::from_str(&settings).expect("parse settings.json");
    let enabled = settings
        .get("enabledMcpjsonServers")
        .and_then(|value| value.as_array())
        .expect("enabledMcpjsonServers array");
    assert!(
        enabled.iter().any(|value| value.as_str() == Some("fake")),
        "trust should be recorded in settings layer: {settings}"
    );

    // Remove the legacy store: a fresh process must rebuild trust from settings.
    std::fs::remove_file(home.path().join("mcp").join("trust.json"))
        .expect("remove legacy trust.json");

    let after = run_mcp_command(binary, home.path(), workspace.path(), &["mcp", "servers"]);
    assert!(
        after.contains("fake") && after.contains("trust=trusted"),
        "settings-layer trust should survive a fresh AppServer without trust.json\n{after}"
    );

    // Trust is functional end-to-end: the trusted server lists its real tools.
    let tools = run_mcp_command(
        binary,
        home.path(),
        workspace.path(),
        &["mcp", "tools", "fake"],
    );
    assert!(
        tools.contains("echo"),
        "trusted server should expose tools after settings-layer reload\n{tools}"
    );
}

#[test]
fn mcp_diagnose_reports_auth_without_tool_permission() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    let env_var = "ORBCODE_CLI_MCP_DIAGNOSE_TOKEN_MISSING_DEFINITELY";
    // SAFETY: this test owns this uniquely named environment variable.
    unsafe { std::env::remove_var(env_var) };

    let add = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("add")
        .arg("needs-auth")
        .arg("--transport")
        .arg("http")
        .arg("--endpoint")
        .arg("http://127.0.0.1:1/mcp")
        .arg("--auth")
        .arg(format!("bearer-env:{env_var}"))
        .output()
        .expect("add MCP server");
    assert!(
        add.status.success(),
        "mcp add failed\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&add.stderr),
        String::from_utf8_lossy(&add.stdout)
    );

    let diagnose = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("diagnose")
        .arg("needs-auth")
        .output()
        .expect("diagnose MCP server");
    let stdout = String::from_utf8_lossy(&diagnose.stdout);
    let stderr = String::from_utf8_lossy(&diagnose.stderr);
    assert!(!diagnose.status.success(), "diagnose should fail\n{stdout}");
    assert!(stdout.contains(env_var), "{stdout}");
    assert!(
        !stdout.contains("permission denied") && !stderr.contains("permission denied"),
        "diagnose should bypass runtime tool permission gate\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn mcp_call_reports_auth_required_with_server_and_fix_guidance() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    let env_var = "ORBCODE_CLI_MCP_CALL_AUTH_MISSING_DEFINITELY";
    // SAFETY: this test owns this uniquely named environment variable.
    unsafe { std::env::remove_var(env_var) };

    // A trusted stdio server whose bearer token env var is absent. Trust is granted
    // (so we clear the trust gate) but the call must still fail with actionable auth
    // guidance rather than collapsing into opaque text.
    let add = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("add")
        .arg("needs-auth")
        .arg("--transport")
        .arg("stdio")
        .arg("--endpoint")
        .arg(fake_stdio_server_binary())
        .arg("--auth")
        .arg(format!("bearer-env:{env_var}"))
        .output()
        .expect("add MCP server");
    assert!(
        add.status.success(),
        "mcp add failed\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&add.stderr),
        String::from_utf8_lossy(&add.stdout)
    );

    let call = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env_remove(env_var)
        .arg("--allow-tools")
        .arg("true")
        .arg("mcp")
        .arg("call")
        .arg("needs-auth")
        .arg("echo")
        .arg(r#"{"text":"cli"}"#)
        .output()
        .expect("run orbcode mcp call");
    let stdout = String::from_utf8_lossy(&call.stdout);
    let stderr = String::from_utf8_lossy(&call.stderr);
    assert!(
        !call.status.success(),
        "mcp call against an unauthenticated server should fail\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("needs-auth"),
        "auth failure must name the server\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        combined.contains("requires authentication"),
        "auth failure must explain the cause\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        combined.contains("orbcode mcp auth login needs-auth"),
        "auth failure must give an actionable fix\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !combined.contains("permission denied"),
        "trusted+allowed server should surface auth, not a permission gate\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn mcp_diagnose_reports_websocket_auth_required() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    let endpoint = spawn_fake_websocket_401_server();

    let add = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("add")
        .arg("ws-needs-auth")
        .arg("--transport")
        .arg("websocket")
        .arg("--endpoint")
        .arg(&endpoint)
        .output()
        .expect("add MCP WebSocket server");
    assert!(
        add.status.success(),
        "mcp add failed\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&add.stderr),
        String::from_utf8_lossy(&add.stdout)
    );

    let diagnose = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("diagnose")
        .arg("ws-needs-auth")
        .output()
        .expect("diagnose WebSocket MCP server");
    let stdout = String::from_utf8_lossy(&diagnose.stdout);
    let stderr = String::from_utf8_lossy(&diagnose.stderr);
    assert!(
        !diagnose.status.success(),
        "diagnose should fail for WS 401\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("requires authentication"),
        "expected AuthRequired text in probe detail\n{stdout}"
    );
    assert!(
        stdout.contains("401"),
        "expected 401 in probe detail\n{stdout}"
    );
    assert!(
        stdout.contains("WWW-Authenticate"),
        "expected WWW-Authenticate header echo in probe detail\n{stdout}"
    );
    assert!(
        !stdout.contains("permission denied") && !stderr.contains("permission denied"),
        "diagnose should bypass runtime tool permission gate\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn mcp_auth_login_status_and_logout_manage_stored_token() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");

    let add = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("add")
        .arg("remote")
        .arg("--transport")
        .arg("http")
        .arg("--endpoint")
        .arg("http://127.0.0.1:1/mcp")
        .output()
        .expect("add MCP server");
    assert!(
        add.status.success(),
        "mcp add failed\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&add.stderr),
        String::from_utf8_lossy(&add.stdout)
    );

    let login = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("auth")
        .arg("login")
        .arg("remote")
        .arg("--access-token")
        .arg("stored-oauth-token")
        .arg("--refresh-token")
        .arg("refresh-token")
        .arg("--scope")
        .arg("tools.read")
        .arg("--token-endpoint")
        .arg("http://127.0.0.1:1/token")
        .arg("--client-id")
        .arg("orbcode-cli-test")
        .output()
        .expect("store MCP OAuth token");
    let stdout = String::from_utf8_lossy(&login.stdout);
    let stderr = String::from_utf8_lossy(&login.stderr);
    assert!(
        login.status.success(),
        "mcp auth login failed\nstderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("stored MCP OAuth token for remote"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("stored-oauth-token"),
        "token output should be redacted: {stdout}"
    );

    let status = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("auth")
        .arg("status")
        .arg("remote")
        .output()
        .expect("show MCP OAuth status");
    let stdout = String::from_utf8_lossy(&status.stdout);
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(
        status.status.success(),
        "mcp auth status failed\nstderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(stdout.contains("remote"), "{stdout}");
    assert!(stdout.contains("ready"), "{stdout}");
    assert!(stdout.contains("token_endpoint=true"), "{stdout}");
    assert!(stdout.contains("scopes=tools.read"), "{stdout}");
    assert!(
        !stdout.contains("stored-oauth-token"),
        "status output should be redacted: {stdout}"
    );

    let logout = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("auth")
        .arg("logout")
        .arg("remote")
        .output()
        .expect("logout MCP OAuth token");
    let stdout = String::from_utf8_lossy(&logout.stdout);
    let stderr = String::from_utf8_lossy(&logout.stderr);
    assert!(
        logout.status.success(),
        "mcp auth logout failed\nstderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(stdout.contains("removed MCP OAuth token"), "{stdout}");

    let status = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("auth")
        .arg("status")
        .arg("remote")
        .output()
        .expect("show MCP OAuth status after logout");
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(status.status.success(), "{stdout}");
    assert!(
        stdout.contains("no MCP OAuth tokens configured"),
        "{stdout}"
    );
}

#[test]
fn mcp_auth_device_login_stores_token() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    let base = spawn_fake_oauth_device_server();

    let add = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("add")
        .arg("remote")
        .arg("--transport")
        .arg("http")
        .arg("--endpoint")
        .arg(format!("{base}/mcp"))
        .output()
        .expect("add MCP server");
    assert!(
        add.status.success(),
        "mcp add failed\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&add.stderr),
        String::from_utf8_lossy(&add.stdout)
    );

    let login = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("auth")
        .arg("device-login")
        .arg("remote")
        .arg("--client-id")
        .arg("orbcode-cli-test")
        .output()
        .expect("run MCP OAuth device login");
    let stdout = String::from_utf8_lossy(&login.stdout);
    let stderr = String::from_utf8_lossy(&login.stderr);
    assert!(
        login.status.success(),
        "mcp auth device-login failed\nstderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("open https://auth.example/device"),
        "{stdout}"
    );
    assert!(stdout.contains("code ABCD-EFGH"), "{stdout}");
    assert!(
        stdout.contains("stored MCP OAuth token for remote"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("device-access-token"),
        "device token output should be redacted: {stdout}"
    );

    let status = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("auth")
        .arg("status")
        .arg("remote")
        .output()
        .expect("show MCP OAuth status");
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(status.status.success(), "{stdout}");
    assert!(stdout.contains("ready"), "{stdout}");
    assert!(stdout.contains("refresh=true"), "{stdout}");
    assert!(stdout.contains("token_endpoint=true"), "{stdout}");
}

#[test]
fn mcp_auth_browser_login_dynamically_registers_and_stores_token() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    let base = spawn_fake_oauth_browser_server();

    let add = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("add")
        .arg("remote")
        .arg("--transport")
        .arg("http")
        .arg("--endpoint")
        .arg(format!("{base}/mcp"))
        .output()
        .expect("add MCP server");
    assert!(
        add.status.success(),
        "mcp add failed\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&add.stderr),
        String::from_utf8_lossy(&add.stdout)
    );

    // No `--client-id`: the CLI must dynamically register (RFC 7591) first.
    // `ORBCODE_NO_BROWSER` keeps CI headless; the printed URL is the fallback.
    let mut child = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .env("ORBCODE_NO_BROWSER", "1")
        .arg("mcp")
        .arg("auth")
        .arg("browser-login")
        .arg("remote")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn orbcode mcp auth browser-login");

    let mut reader = BufReader::new(child.stdout.take().expect("child stdout"));
    let mut authorization_url = String::new();
    let mut redirect_uri = String::new();
    let mut disabled_notice = false;
    let mut collected = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .expect("read browser-login stdout");
        assert!(read > 0, "browser-login exited early\n{collected}");
        collected.push_str(&line);
        let trimmed = line.trim();
        if let Some(url) = trimmed.strip_prefix("open ") {
            authorization_url = url.to_string();
        } else if let Some(uri) = trimmed.strip_prefix("listening ") {
            redirect_uri = uri.to_string();
        } else if trimmed.contains("browser auto-launch disabled") {
            disabled_notice = true;
        }
        if !authorization_url.is_empty() && !redirect_uri.is_empty() && disabled_notice {
            break;
        }
    }

    assert!(
        authorization_url.contains("client_id=dynamic-client-id"),
        "authorization URL should carry the dynamically registered client id: {authorization_url}"
    );
    let state = query_param(&authorization_url, "state").expect("state in authorization URL");

    // The CLI is now blocked on its loopback listener; deliver the callback.
    let callback = format!("{redirect_uri}?code=cli-browser-code&state={state}");
    let callback_response = http_get(&callback);
    assert!(
        callback_response.contains("200 OK"),
        "callback should succeed: {callback_response}"
    );

    let mut rest = String::new();
    reader.read_to_string(&mut rest).expect("drain stdout");
    collected.push_str(&rest);
    let status = child.wait().expect("await browser-login");
    let mut stderr = String::new();
    if let Some(mut handle) = child.stderr.take() {
        let _ = handle.read_to_string(&mut stderr);
    }
    assert!(
        status.success(),
        "browser-login failed\nstdout:\n{collected}\nstderr:\n{stderr}"
    );
    assert!(
        collected.contains("browser auto-launch disabled"),
        "should report the disabled fallback: {collected}"
    );
    assert!(
        collected.contains("stored MCP OAuth token for remote"),
        "{collected}"
    );
    assert!(
        !collected.contains("browser-access-token"),
        "token output should be redacted: {collected}"
    );

    let status = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("auth")
        .arg("status")
        .arg("remote")
        .output()
        .expect("show MCP OAuth status");
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(status.status.success(), "{stdout}");
    assert!(stdout.contains("ready"), "{stdout}");
    assert!(stdout.contains("refresh=true"), "{stdout}");
    assert!(stdout.contains("token_endpoint=true"), "{stdout}");
}

#[test]
fn mcp_http_server_lists_and_calls_real_tools_with_static_header_auth() {
    let home = tempfile::tempdir().expect("temp Claude home");
    let workspace = tempfile::tempdir().expect("temp workspace");
    let binary = env!("CARGO_BIN_EXE_orbcode");
    let endpoint = spawn_fake_http_mcp_server(4);

    let add = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("mcp")
        .arg("add")
        .arg("remote")
        .arg("--transport")
        .arg("http")
        .arg("--endpoint")
        .arg(endpoint)
        .arg("--auth")
        .arg("header:X-Api-Key=static-secret")
        .output()
        .expect("add MCP HTTP server");
    assert!(
        add.status.success(),
        "mcp add failed\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&add.stderr),
        String::from_utf8_lossy(&add.stdout)
    );

    let tools = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("--allow-tools")
        .arg("true")
        .arg("mcp")
        .arg("tools")
        .arg("remote")
        .output()
        .expect("run orbcode mcp tools");
    let stdout = String::from_utf8_lossy(&tools.stdout);
    let stderr = String::from_utf8_lossy(&tools.stderr);
    assert!(
        tools.status.success(),
        "mcp tools failed\nstderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(stdout.contains("echo  Echo over HTTP."), "{stdout}");
    assert!(
        !stdout.contains("inspect"),
        "HTTP tools should come from real tools/list, not modeled stored tools: {stdout}"
    );

    let call = Command::new(binary)
        .current_dir(workspace.path())
        .env("CLAUDE_CONFIG_DIR", home.path())
        .arg("--allow-tools")
        .arg("true")
        .arg("mcp")
        .arg("call")
        .arg("remote")
        .arg("echo")
        .arg(r#"{"text":"cli-http"}"#)
        .output()
        .expect("run orbcode mcp call");
    let stdout = String::from_utf8_lossy(&call.stdout);
    let stderr = String::from_utf8_lossy(&call.stderr);
    assert!(
        call.status.success(),
        "mcp call failed\nstderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(stdout.contains("http echo: cli-http"), "{stdout}");
}

fn trust_mcp_server(binary: &str, home: &Path, workspace: &Path, server_id: &str) {
    let output = Command::new(binary)
        .current_dir(workspace)
        .env("CLAUDE_CONFIG_DIR", home)
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

fn run_mcp_command(binary: &str, home: &Path, workspace: &Path, args: &[&str]) -> String {
    let output = Command::new(binary)
        .current_dir(workspace)
        .env("CLAUDE_CONFIG_DIR", home)
        .arg("--allow-tools")
        .arg("true")
        .args(args)
        .output()
        .expect("run orbcode mcp command");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "command {args:?} failed\nstderr:\n{stderr}\nstdout:\n{stdout}"
    );
    stdout.into_owned()
}

fn spawn_fake_http_mcp_server(requests: usize) -> String {
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake HTTP MCP server");
    let endpoint = format!("http://{}/mcp", listener.local_addr().expect("local addr"));
    thread::spawn(move || {
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().expect("accept fake HTTP request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let request = read_http_request(&mut stream);
            let body = fake_http_mcp_response(&request);
            let status = if body.contains("missing static auth header") {
                "400 Bad Request"
            } else {
                "200 OK"
            };
            let payload = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(payload.as_bytes())
                .expect("write fake HTTP response");
        }
    });
    endpoint
}

fn spawn_fake_websocket_401_server() -> String {
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake WebSocket 401 server");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("local addr"));
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let _ = read_http_request(&mut stream);
            let response = "HTTP/1.1 401 Unauthorized\r\n\
                            WWW-Authenticate: Bearer realm=\"mcp\"\r\n\
                            Content-Length: 0\r\n\
                            Connection: close\r\n\r\n";
            let _ = stream.write_all(response.as_bytes());
        }
    });
    endpoint
}

fn spawn_fake_oauth_device_server() -> String {
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake OAuth server");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    let base_for_thread = base.clone();
    thread::spawn(move || {
        for _ in 0..5 {
            let (mut stream, _) = listener.accept().expect("accept fake OAuth request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let request = read_http_request(&mut stream);
            let (status, body, headers) = if request.starts_with("POST /mcp ") {
                (
                    "401 Unauthorized",
                    String::new(),
                    format!(
                        "WWW-Authenticate: Bearer resource_metadata=\"{base_for_thread}/.well-known/oauth-protected-resource\"\r\n"
                    ),
                )
            } else if request.starts_with("GET /.well-known/oauth-protected-resource ") {
                (
                    "200 OK",
                    format!(
                        r#"{{"resource":"{base_for_thread}/mcp","authorization_servers":["{base_for_thread}/auth"],"scopes_supported":["tools.read"]}}"#
                    ),
                    String::new(),
                )
            } else if request.starts_with("GET /.well-known/oauth-authorization-server/auth ") {
                (
                    "200 OK",
                    format!(
                        r#"{{"issuer":"{base_for_thread}/auth","token_endpoint":"{base_for_thread}/token","device_authorization_endpoint":"{base_for_thread}/device","scopes_supported":["tools.read"]}}"#
                    ),
                    String::new(),
                )
            } else if request.starts_with("POST /device ") {
                assert!(request.contains("client_id=orbcode-cli-test"), "{request}");
                assert!(request.contains("scope=tools.read"), "{request}");
                (
                    "200 OK",
                    r#"{"device_code":"device-code","user_code":"ABCD-EFGH","verification_uri":"https://auth.example/device","verification_uri_complete":"https://auth.example/device?user_code=ABCD-EFGH","expires_in":600,"interval":1}"#.to_string(),
                    String::new(),
                )
            } else {
                assert!(request.starts_with("POST /token "), "{request}");
                assert!(request.contains("device_code=device-code"), "{request}");
                (
                    "200 OK",
                    r#"{"access_token":"device-access-token","refresh_token":"device-refresh-token","expires_in":3600,"scope":"tools.read"}"#.to_string(),
                    String::new(),
                )
            };
            let payload = format!(
                "HTTP/1.1 {status}\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(payload.as_bytes())
                .expect("write fake OAuth response");
        }
    });
    base
}

fn spawn_fake_oauth_browser_server() -> String {
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake OAuth browser server");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    let base_for_thread = base.clone();
    thread::spawn(move || {
        for _ in 0..5 {
            let (mut stream, _) = listener.accept().expect("accept fake OAuth request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let request = read_http_request(&mut stream);
            let base = &base_for_thread;
            let (status, body, headers) = if request.starts_with("POST /mcp ") {
                (
                    "401 Unauthorized",
                    String::new(),
                    format!(
                        "WWW-Authenticate: Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource\"\r\n"
                    ),
                )
            } else if request.starts_with("GET /.well-known/oauth-protected-resource ") {
                (
                    "200 OK",
                    format!(
                        r#"{{"resource":"{base}/mcp","authorization_servers":["{base}/auth"],"scopes_supported":["tools.read"]}}"#
                    ),
                    String::new(),
                )
            } else if request.starts_with("GET /.well-known/oauth-authorization-server/auth ") {
                (
                    "200 OK",
                    format!(
                        r#"{{"issuer":"{base}/auth","authorization_endpoint":"{base}/authorize","token_endpoint":"{base}/token","registration_endpoint":"{base}/register","scopes_supported":["tools.read"]}}"#
                    ),
                    String::new(),
                )
            } else if request.starts_with("POST /register ") {
                assert!(request.contains("\"redirect_uris\""), "{request}");
                assert!(request.contains("127.0.0.1"), "{request}");
                assert!(request.contains("authorization_code"), "{request}");
                (
                    "200 OK",
                    r#"{"client_id":"dynamic-client-id","client_secret":"dynamic-client-secret"}"#
                        .to_string(),
                    String::new(),
                )
            } else {
                assert!(request.starts_with("POST /token "), "{request}");
                assert!(
                    request.contains("grant_type=authorization_code"),
                    "{request}"
                );
                assert!(request.contains("code=cli-browser-code"), "{request}");
                assert!(request.contains("client_id=dynamic-client-id"), "{request}");
                (
                    "200 OK",
                    r#"{"access_token":"browser-access-token","refresh_token":"browser-refresh-token","expires_in":3600,"scope":"tools.read"}"#.to_string(),
                    String::new(),
                )
            };
            let payload = format!(
                "HTTP/1.1 {status}\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(payload.as_bytes())
                .expect("write fake OAuth response");
        }
    });
    base
}

fn query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        if key == name {
            Some(value.to_string())
        } else {
            None
        }
    })
}

fn http_get(url: &str) -> String {
    let without_scheme = url.strip_prefix("http://").expect("http url");
    let (authority, path_and_query) = match without_scheme.split_once('/') {
        Some((authority, rest)) => (authority, format!("/{rest}")),
        None => (without_scheme, "/".to_string()),
    };
    let mut stream = TcpStream::connect(authority).expect("connect callback listener");
    let request =
        format!("GET {path_and_query} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("send callback request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read callback response");
    response
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read;

    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer).expect("read fake HTTP request");
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        let header_end = bytes.windows(4).position(|window| window == b"\r\n\r\n");
        if let Some(header_end) = header_end {
            let headers = String::from_utf8_lossy(&bytes[..header_end + 4]);
            let content_length = headers
                .lines()
                .find_map(|line| line.split_once(':'))
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let expected = header_end + 4 + content_length;
            while bytes.len() < expected {
                let read = stream.read(&mut buffer).expect("read fake HTTP body");
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..read]);
            }
            break;
        }
    }
    String::from_utf8_lossy(&bytes).to_string()
}

fn fake_http_mcp_response(request: &str) -> String {
    if !request
        .to_ascii_lowercase()
        .contains("x-api-key: static-secret")
    {
        return r#"{"error":"missing static auth header"}"#.to_string();
    }
    if request.contains(r#""method":"initialize""#) {
        return r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"orbcode-cli-fake-http","version":"0.1.0"}}}"#.to_string();
    }
    if request.contains(r#""method":"tools/list""#) {
        return r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo over HTTP.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}"#.to_string();
    }
    assert!(request.contains(r#""method":"tools/call""#), "{request}");
    assert!(request.contains(r#""text":"cli-http""#), "{request}");
    r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"http echo: cli-http"}],"isError":false}}"#.to_string()
}

fn write_mcp_json(workspace: &Path, server_binary: &Path) {
    let config = format!(
        r#"{{
  "mcpServers": {{
    "fake": {{
      "type": "stdio",
      "command": "{}"
    }}
  }}
}}"#,
        json_escape(&server_binary.display().to_string())
    );
    std::fs::write(workspace.join(".mcp.json"), config).expect("write .mcp.json");
}

fn fake_stdio_server_binary() -> &'static Path {
    static SERVER_BINARY: OnceLock<PathBuf> = OnceLock::new();
    SERVER_BINARY
        .get_or_init(compile_fake_stdio_server)
        .as_path()
}

fn compile_fake_stdio_server() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "orbcode-cli-fake-stdio-server-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create fake MCP server temp dir");

    let source = dir.join("fake_stdio_server.rs");
    let binary = dir.join(if cfg!(windows) {
        "fake_stdio_server.exe"
    } else {
        "fake_stdio_server"
    });
    std::fs::write(&source, FAKE_STDIO_SERVER_SOURCE).expect("write fake MCP server source");

    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc)
        .arg("--edition=2021")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("compile fake MCP stdio server");

    assert!(
        output.status.success(),
        "compile fake MCP stdio server\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    binary
}

fn json_escape(input: &str) -> String {
    input
        .replace(
            '\\',
            r#"\\")
        .replace('"', r#"\""#,
        )
        .replace('\n', r"\n")
        .replace('\r', r"\r")
        .replace('\t', r"\t")
}

const FAKE_STDIO_SERVER_SOURCE: &str = r##"
use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }

        let response = fake_stdio_response(&line);
        writeln!(stdout, "{response}").expect("write fake MCP response");
        stdout.flush().expect("flush fake MCP response");
    }
}

fn fake_stdio_response(request: &str) -> String {
    let id = extract_id(request);

    if request.contains(r#""method":"initialize""#) {
        return success_response(
            &id,
            r#"{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"orbcode-cli-fake-stdio","version":"0.1.0"}}"#,
        );
    }

    if request.contains(r#""method":"tools/list""#) {
        return success_response(
            &id,
            r#"{"tools":[{"name":"echo","description":"Echo test input.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}"#,
        );
    }

    if request.contains(r#""method":"tools/call""#) && request.contains(r#""name":"echo""#) {
        let text = extract_text_argument(request);
        let echoed = escape_json_string(&format!("echo: {text}"));
        return success_response(
            &id,
            &format!(r#"{{"content":[{{"type":"text","text":"{echoed}"}}],"isError":false}}"#),
        );
    }

    if request.contains(r#""method":"resources/list""#) {
        return success_response(
            &id,
            r#"{"resources":[{"uri":"res://text","name":"Text","mimeType":"text/plain","description":"A text resource"},{"uri":"res://binary","name":"Bin","mimeType":"application/octet-stream","description":"A binary resource"}]}"#,
        );
    }

    if request.contains(r#""method":"resources/read""#) {
        if request.contains(r#""uri":"res://binary""#) {
            return success_response(
                &id,
                r#"{"contents":[{"uri":"res://binary","mimeType":"application/octet-stream","blob":"aGVsbG8="}]}"#,
            );
        }
        if request.contains(r#""uri":"res://text""#) {
            return success_response(
                &id,
                r#"{"contents":[{"uri":"res://text","mimeType":"text/plain","text":"resource body"}]}"#,
            );
        }
        return error_response(&id, -32602, "unknown fake resource");
    }

    if request.contains(r#""method":"prompts/list""#) {
        return success_response(
            &id,
            r#"{"prompts":[{"name":"greet","description":"Greet someone.","arguments":[{"name":"name","description":"Who to greet.","required":true}]},{"name":"embed","description":"Embed a resource.","arguments":[]}]}"#,
        );
    }

    if request.contains(r#""method":"prompts/get""#) && request.contains(r#""name":"greet""#) {
        let who = extract_name_argument(request);
        let message = escape_json_string(&format!("Hello, {who}!"));
        return success_response(
            &id,
            &format!(
                r#"{{"description":"Greet someone.","messages":[{{"role":"user","content":{{"type":"text","text":"{message}"}}}}]}}"#
            ),
        );
    }

    if request.contains(r#""method":"prompts/get""#) && request.contains(r#""name":"embed""#) {
        return success_response(
            &id,
            r#"{"description":"Embed a resource.","messages":[{"role":"user","content":{"type":"resource","resource":{"uri":"res://text","mimeType":"text/plain","text":"embedded body"}}}]}"#,
        );
    }

    error_response(&id, -32602, "unknown fake tool")
}

fn extract_id(request: &str) -> String {
    let Some(index) = request.find(r#""id":"#) else {
        return "null".to_string();
    };
    let rest = request[index + r#""id":"#.len()..].trim_start();
    let id: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
        .collect();
    if id.is_empty() { "null".to_string() } else { id }
}

fn extract_text_argument(request: &str) -> String {
    let pattern = "\"text\":\"";
    let Some(index) = request.find(pattern) else {
        return String::new();
    };
    let rest = &request[index + pattern.len()..];
    read_json_string_value(rest).unwrap_or_default()
}

fn extract_name_argument(request: &str) -> String {
    let Some(args_index) = request.find("\"arguments\"") else {
        return String::new();
    };
    let scope = &request[args_index..];
    let pattern = "\"name\":\"";
    let Some(index) = scope.find(pattern) else {
        return String::new();
    };
    let rest = &scope[index + pattern.len()..];
    read_json_string_value(rest).unwrap_or_default()
}

fn read_json_string_value(input: &str) -> Option<String> {
    let mut value = String::new();
    let mut chars = input.chars();

    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                '"' => value.push('"'),
                '\\' => value.push('\\'),
                '/' => value.push('/'),
                'b' => value.push('\u{0008}'),
                'f' => value.push('\u{000c}'),
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                other => value.push(other),
            },
            other => value.push(other),
        }
    }

    None
}

fn success_response(id: &str, result: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#)
}

fn error_response(id: &str, code: i64, message: &str) -> String {
    let message = escape_json_string(message);
    format!(r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":{code},"message":"{message}"}}}}"#)
}

fn escape_json_string(input: &str) -> String {
    let mut output = String::new();
    for ch in input.chars() {
        match ch {
            '"' => output.push_str(r#"\""#),
            '\\' => output.push_str(r#"\\"#),
            '\n' => output.push_str(r#"\n"#),
            '\r' => output.push_str(r#"\r"#),
            '\t' => output.push_str(r#"\t"#),
            other => output.push(other),
        }
    }
    output
}
"##;
