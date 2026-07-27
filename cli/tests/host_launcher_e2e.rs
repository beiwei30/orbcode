//! Host launcher contract tests — verify that `orbcode serve` emits
//! structured connection info JSON, allowing hosts to discover
//! the transport endpoint and auth token programmatically.
//!
//! Covers WebSocket and Unix socket transports on stdout. Stdio keeps its
//! connection info on stderr because stdout is the protocol stream.

use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

async fn read_connection_info_from<R>(mut reader: R, stream_name: &str) -> Value
where
    R: AsyncBufRead + Unpin,
{
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await.expect("read stream");
            if n == 0 {
                panic!("{stream_name} EOF before connection info");
            }
            if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                && v.get("transport").is_some()
            {
                return v;
            }
        }
    })
    .await
    .expect("connection info JSON within 15s")
}

async fn read_connection_info_stdout(child: &mut tokio::process::Child) -> Value {
    let stdout = child.stdout.take().expect("stdout");
    read_connection_info_from(BufReader::new(stdout), "stdout").await
}

async fn read_connection_info_stderr(child: &mut tokio::process::Child) -> Value {
    let stderr = child.stderr.take().expect("stderr");
    read_connection_info_from(BufReader::new(stderr), "stderr").await
}

fn spawn_serve(args: &[&str]) -> (tokio::process::Child, TempDir, TempDir) {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    let child = Command::new(ORBCODE_BIN)
        .arg("serve")
        .args(args)
        .current_dir(cwd.path())
        .env_clear()
        .env("ORBCODE_HOME", home.path())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("RUST_LOG", "warn")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn orbcode serve");

    (child, home, cwd)
}

// =========================================================================
// WebSocket
// =========================================================================

#[tokio::test]
async fn websocket_connection_info_has_required_fields() {
    let (mut child, _home, _cwd) = spawn_serve(&["--websocket", "127.0.0.1:0"]);
    let info = read_connection_info_stdout(&mut child).await;

    assert_eq!(info["transport"], "websocket");
    assert!(info["addr"].as_str().is_some(), "addr field present");
    assert!(
        info["auth_token"].as_str().is_some(),
        "auth_token field present"
    );

    let addr = info["addr"].as_str().unwrap();
    assert!(
        !addr.ends_with(":0"),
        "addr should have a real port: {addr}"
    );

    child.kill().await.ok();
}

#[tokio::test]
async fn websocket_connection_info_enables_full_handshake() {
    let (mut child, _home, _cwd) = spawn_serve(&["--websocket", "127.0.0.1:0"]);
    let info = read_connection_info_stdout(&mut child).await;

    let addr = info["addr"].as_str().unwrap();
    let token = info["auth_token"].as_str().unwrap();

    let url = format!("ws://{addr}");
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    .expect("connect timeout")
    .expect("ws connect");

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    ws.send(Message::Text(token.to_string().into()))
        .await
        .unwrap();

    let init_req = json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "host-launcher-test", "version": "0.1" },
            "capabilities": { "streaming": true }
        }
    });
    ws.send(Message::Text(
        serde_json::to_string(&init_req).unwrap().into(),
    ))
    .await
    .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("timeout")
        .expect("no msg")
        .expect("ws error");
    let resp: Value = serde_json::from_str(resp.to_text().unwrap()).unwrap();
    assert_eq!(resp["result"]["status"], "success");
    assert_eq!(resp["result"]["data"]["server_info"]["name"], "orbcode");

    ws.close(None).await.ok();
    child.kill().await.ok();
}

// =========================================================================
// Unix socket
// =========================================================================

#[tokio::test]
async fn socket_connection_info_has_required_fields() {
    let sock_dir = tempfile::tempdir().expect("sock dir");
    let sock_path = sock_dir.path().join("test.sock");

    let (mut child, _home, _cwd) = spawn_serve(&["--socket", sock_path.to_str().unwrap()]);
    let info = read_connection_info_stdout(&mut child).await;

    assert_eq!(info["transport"], "socket");
    assert_eq!(
        info["path"].as_str().unwrap(),
        sock_path.to_str().unwrap(),
        "path matches requested socket path"
    );
    assert!(
        info["auth_token"].as_str().is_some(),
        "auth_token field present"
    );

    child.kill().await.ok();
}

#[tokio::test]
async fn socket_connection_info_enables_full_handshake() {
    let sock_dir = tempfile::tempdir().expect("sock dir");
    let sock_path = sock_dir.path().join("test.sock");

    let (mut child, _home, _cwd) = spawn_serve(&["--socket", sock_path.to_str().unwrap()]);
    let info = read_connection_info_stdout(&mut child).await;

    let token = info["auth_token"].as_str().unwrap();
    let path = info["path"].as_str().unwrap();

    let stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::UnixStream::connect(path),
    )
    .await
    .expect("connect timeout")
    .expect("socket connect");

    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    // Send auth token as first line (raw string, not JSON)
    writer
        .write_all(format!("{token}\n").as_bytes())
        .await
        .unwrap();

    // Initialize
    let init_req = json!({
        "type": "request",
        "id": "init-1",
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "socket-launcher-test", "version": "0.1" },
            "capabilities": { "streaming": true }
        }
    });
    writer
        .write_all(format!("{}\n", serde_json::to_string(&init_req).unwrap()).as_bytes())
        .await
        .unwrap();

    let mut line = String::new();
    // Read until we get a response (skip non-response lines)
    let resp = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            line.clear();
            reader.read_line(&mut line).await.expect("read");
            if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                && v["type"] == "response"
            {
                return v;
            }
        }
    })
    .await
    .expect("response timeout");

    assert_eq!(resp["result"]["status"], "success");
    assert_eq!(resp["result"]["data"]["server_info"]["name"], "orbcode");

    // Session list
    let list_req = json!({"type":"request","id":"sl-1","method":"session/list"});
    writer
        .write_all(format!("{}\n", serde_json::to_string(&list_req).unwrap()).as_bytes())
        .await
        .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            line.clear();
            reader.read_line(&mut line).await.expect("read");
            if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                && v["type"] == "response"
            {
                return v;
            }
        }
    })
    .await
    .expect("list response timeout");

    assert_eq!(resp["result"]["status"], "success");

    drop(writer);
    child.kill().await.ok();
}

// =========================================================================
// Stdio
// =========================================================================

#[tokio::test]
async fn stdio_connection_info_has_transport_field() {
    let (mut child, _home, _cwd) = {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");

        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
        )
        .expect("write settings");

        let child = Command::new(ORBCODE_BIN)
            .arg("serve")
            .arg("--stdio")
            .current_dir(cwd.path())
            .env_clear()
            .env("ORBCODE_HOME", home.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", home.path())
            .env("RUST_LOG", "warn")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn orbcode serve --stdio");

        (child, home, cwd)
    };

    let info = read_connection_info_stderr(&mut child).await;
    assert_eq!(info["transport"], "stdio");

    child.kill().await.ok();
}
