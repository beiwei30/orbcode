mod runtime_fault_fixtures;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::HeaderMap;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::oauth::{register_oauth_client, unix_timestamp_now};
use crate::registry::canonicalize_auth_server;
use crate::transport::http::HttpMcpClient;
use crate::transport::websocket::{WEBSOCKET_ACCEPT, WebSocketMcpClient, read_websocket_frame};
use crate::transport::{effective_http_headers, effective_stdio_env};
use crate::wire::RawContentBlock;
use crate::*;

fn temp_paths(label: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("orbcode-mcp-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let cwd = root.join("cwd");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&cwd).expect("create cwd");
    (home, cwd)
}

#[test]
fn embedded_resource_text_block_flattens_nested_payload() {
    let raw: RawContentBlock = serde_json::from_value(serde_json::json!({
        "type": "resource",
        "resource": {
            "uri": "res://text/1",
            "mimeType": "text/plain",
            "text": "embedded body"
        }
    }))
    .expect("deserialize embedded text block");
    let content = McpContent::from(raw);
    assert_eq!(content.kind, "resource");
    assert_eq!(content.text.as_deref(), Some("embedded body"));
    assert!(!content.is_binary);
    assert_eq!(content.binary, None);
    assert_eq!(content.mime_type, "text/plain");
}

#[test]
fn embedded_resource_blob_block_marks_binary() {
    let raw: RawContentBlock = serde_json::from_value(serde_json::json!({
        "type": "resource",
        "resource": {
            "uri": "res://binary/1",
            "mimeType": "application/octet-stream",
            "blob": "aGVsbG8="
        }
    }))
    .expect("deserialize embedded blob block");
    let content = McpContent::from(raw);
    assert_eq!(content.kind, "resource");
    assert!(content.is_binary);
    assert_eq!(content.binary.as_deref(), Some("aGVsbG8="));
    assert_eq!(content.text, None);
    assert_eq!(content.mime_type, "application/octet-stream");
}

struct FakeHttpResponse {
    status: &'static str,
    content_type: &'static str,
    headers: Vec<(String, String)>,
    body: String,
    delay: Option<Duration>,
}

impl FakeHttpResponse {
    fn ok(body: String) -> Self {
        Self {
            status: "200 OK",
            content_type: "application/json",
            headers: Vec::new(),
            body,
            delay: None,
        }
    }

    fn status(status: &'static str, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/plain",
            headers: Vec::new(),
            body: body.into(),
            delay: None,
        }
    }

    fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    fn with_content_type(mut self, content_type: &'static str) -> Self {
        self.content_type = content_type;
        self
    }

    fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }
}

fn spawn_fake_http_mcp_server(
    requests: usize,
    handler: impl Fn(usize, String) -> FakeHttpResponse + Send + 'static,
) -> String {
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake HTTP MCP server");
    let endpoint = format!("http://{}/mcp", listener.local_addr().expect("local addr"));
    thread::spawn(move || {
        for index in 0..requests {
            let (mut stream, _) = listener.accept().expect("accept fake HTTP request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let request = read_http_request(&mut stream);
            let response = handler(index, request);
            if let Some(delay) = response.delay {
                thread::sleep(delay);
            }
            let headers = response
                .headers
                .into_iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect::<String>();
            let payload = format!(
                "HTTP/1.1 {}\r\n{}Content-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.status,
                headers,
                response.content_type,
                response.body.len(),
                response.body
            );
            stream
                .write_all(payload.as_bytes())
                .expect("write fake HTTP response");
        }
    });
    endpoint
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

fn http_json_rpc_id(request: &str) -> u64 {
    let body = request.split("\r\n\r\n").nth(1).expect("http request body");
    serde_json::from_str::<Value>(body)
        .expect("JSON-RPC request body")
        .get("id")
        .and_then(Value::as_u64)
        .expect("numeric JSON-RPC id")
}

fn json_rpc_response(request: &str, result: Value) -> String {
    json!({"jsonrpc":"2.0","id":http_json_rpc_id(request),"result":result}).to_string()
}

fn spawn_fake_websocket_mcp_server(connections: usize) -> String {
    spawn_fake_websocket_mcp_server_with_auth_checks(connections, true)
}

fn spawn_fake_websocket_mcp_server_with_auth_checks(
    connections: usize,
    require_auth: bool,
) -> String {
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake WebSocket MCP server");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("local addr"));
    thread::spawn(move || {
        for _ in 0..connections {
            let (mut stream, _) = listener.accept().expect("accept fake WebSocket request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let request = read_http_request(&mut stream);
            if require_auth {
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("x-api-key: static-secret"),
                    "{request}"
                );
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer runtime-token"),
                    "{request}"
                );
            }
            let handshake = format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                     Upgrade: websocket\r\n\
                     Connection: Upgrade\r\n\
                     Sec-WebSocket-Accept: {WEBSOCKET_ACCEPT}\r\n\r\n"
            );
            stream
                .write_all(handshake.as_bytes())
                .expect("write fake WebSocket handshake");

            for _ in 0..2 {
                let message = read_blocking_websocket_text(&mut stream);
                let response = fake_websocket_mcp_response(&message);
                let frame = websocket_server_text_frame(response.as_bytes());
                stream
                    .write_all(&frame)
                    .expect("write fake WebSocket frame");
            }
        }
    });
    endpoint
}

fn spawn_slow_fake_websocket_mcp_server(delay: Duration) -> String {
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind slow fake WebSocket MCP server");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("local addr"));
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set slow read timeout");
            thread::sleep(delay);
            drop(stream);
        }
    });
    endpoint
}

fn spawn_fake_websocket_mcp_server_with_handshake_status(status_line: &'static str) -> String {
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    let listener =
        TcpListener::bind("127.0.0.1:0").expect("bind fake WebSocket MCP server (status)");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("local addr"));
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let _ = read_http_request(&mut stream);
            let _ = stream.write_all(status_line.as_bytes());
        }
    });
    endpoint
}

fn spawn_fake_websocket_mcp_server_with_responses(responses: Vec<String>) -> String {
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    let listener =
        TcpListener::bind("127.0.0.1:0").expect("bind fake WebSocket MCP server (responses)");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("local addr"));
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept fake WebSocket request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let _ = read_http_request(&mut stream);
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
                 Upgrade: websocket\r\n\
                 Connection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {WEBSOCKET_ACCEPT}\r\n\r\n"
        );
        stream
            .write_all(handshake.as_bytes())
            .expect("write fake WebSocket handshake");

        for response in responses {
            let _ = read_blocking_websocket_text(&mut stream);
            let frame = websocket_server_text_frame(response.as_bytes());
            stream
                .write_all(&frame)
                .expect("write fake WebSocket response frame");
        }
    });
    endpoint
}

async fn spawn_fake_tls_websocket_mcp_server(connections: usize) -> (String, Vec<u8>) {
    let cert_der = decode_base64(TEST_WSS_CERT_DER_BASE64);
    let key_der = decode_base64(TEST_WSS_KEY_DER_BASE64);
    let std_listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake TLS WebSocket MCP server");
    std_listener
        .set_nonblocking(true)
        .expect("set fake TLS listener nonblocking");
    let listener = TcpListener::from_std(std_listener).expect("tokio fake TLS listener");
    let endpoint = format!(
        "wss://localhost:{}/mcp",
        listener.local_addr().expect("addr").port()
    );
    let cert_for_server = cert_der.clone();
    tokio::spawn(async move {
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls_pki_types::CertificateDer::from(cert_for_server)],
                rustls_pki_types::PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(
                    key_der,
                )),
            )
            .expect("TLS server config");
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        for _ in 0..connections {
            let (stream, _) = listener.accept().await.expect("accept fake TLS WebSocket");
            let mut stream = acceptor.accept(stream).await.expect("accept TLS");
            let request = read_async_http_request(&mut stream).await;
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("x-api-key: static-secret"),
                "{request}"
            );
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer runtime-token"),
                "{request}"
            );
            let handshake = format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                     Upgrade: websocket\r\n\
                     Connection: Upgrade\r\n\
                     Sec-WebSocket-Accept: {WEBSOCKET_ACCEPT}\r\n\r\n"
            );
            stream
                .write_all(handshake.as_bytes())
                .await
                .expect("write fake TLS WebSocket handshake");

            for _ in 0..2 {
                let frame = read_websocket_frame(&mut stream)
                    .await
                    .expect("read ws frame");
                let message = String::from_utf8(frame.payload).expect("utf8 ws text");
                let response = fake_websocket_mcp_response(&message);
                let frame = websocket_server_text_frame(response.as_bytes());
                stream
                    .write_all(&frame)
                    .await
                    .expect("write fake TLS WebSocket frame");
            }
        }
    });
    (endpoint, cert_der)
}

async fn read_async_http_request<S>(stream: &mut S) -> String
where
    S: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer).await.expect("read http request");
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&bytes).to_string()
}

fn read_blocking_websocket_text(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read;

    let mut header = [0_u8; 2];
    stream.read_exact(&mut header).expect("read ws header");
    let opcode = header[0] & 0x0f;
    assert_eq!(opcode, 0x1, "expected text frame");
    let masked = header[1] & 0x80 != 0;
    let mut len = (header[1] & 0x7f) as u64;
    if len == 126 {
        let mut extended = [0_u8; 2];
        stream.read_exact(&mut extended).expect("read ws len16");
        len = u16::from_be_bytes(extended) as u64;
    } else if len == 127 {
        let mut extended = [0_u8; 8];
        stream.read_exact(&mut extended).expect("read ws len64");
        len = u64::from_be_bytes(extended);
    }
    let mask = if masked {
        let mut mask = [0_u8; 4];
        stream.read_exact(&mut mask).expect("read ws mask");
        Some(mask)
    } else {
        None
    };
    let mut payload = vec![0_u8; len as usize];
    stream.read_exact(&mut payload).expect("read ws payload");
    if let Some(mask) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    String::from_utf8(payload).expect("utf8 ws text")
}

fn websocket_server_text_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.push(0x81);
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

fn fake_websocket_mcp_response(request: &str) -> String {
    if request.contains(r#""method":"initialize""#) {
        return json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake-websocket", "version": "0.1.0"}
            }
        })
        .to_string();
    }
    if request.contains(r#""method":"tools/list""#) {
        return json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [{
                    "name": "echo",
                    "description": "Echo over WebSocket.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"text": {"type": "string"}}
                    }
                }]
            }
        })
        .to_string();
    }
    assert!(request.contains(r#""method":"tools/call""#), "{request}");
    assert!(request.contains(r#""text":"runtime""#), "{request}");
    json!({
        "jsonrpc": "2.0",
        "id": 2,
        "result": {
            "content": [{"type": "text", "text": "websocket echo: runtime"}],
            "isError": false
        }
    })
    .to_string()
}

const DISCOVERY_INITIALIZE_RESULT: &str = r#"{"protocolVersion":"2024-11-05","capabilities":{"tools":{},"resources":{},"prompts":{}},"serverInfo":{"name":"fake-discovery","version":"0.1.0"}}"#;

/// Result payloads for the discovery "success" dataset, shared by the HTTP and
/// WebSocket fixtures so both transports exercise identical wire shapes.
fn discovery_success_result(request: &str) -> Value {
    if request.contains(r#""method":"resources/templates/list""#) {
        json!({"resourceTemplates":[{"uriTemplate":"res://items/{itemId}","name":"Item","mimeType":"application/json","description":"An item by id.","annotations":{"priority":0.5}}]})
    } else if request.contains(r#""method":"resources/list""#) {
        json!({"resources":[
            {"uri":"res://text","name":"Text Resource","mimeType":"text/plain","description":"A text resource.","annotations":{"audience":["user","assistant"],"priority":0.8}},
            {"uri":"res://binary","name":"Binary Resource","mimeType":"application/octet-stream","description":"A binary resource."}
        ]})
    } else if request.contains(r#""method":"resources/read""#) {
        if request.contains(r#""uri":"res://binary""#) {
            json!({"contents":[{"uri":"res://binary","mimeType":"application/octet-stream","blob":"aGVsbG8="}]})
        } else {
            json!({"contents":[{"uri":"res://text","mimeType":"text/plain","text":"hello text","annotations":{"audience":["user"]}}]})
        }
    } else if request.contains(r#""method":"prompts/list""#) {
        json!({"prompts":[{"name":"greet","description":"Greet someone.","_meta":{"skill":true},"arguments":[{"name":"name","description":"Who to greet.","required":true}]}]})
    } else if request.contains(r#""method":"prompts/get""#) {
        json!({"description":"A greeting.","messages":[{"role":"user","content":{"type":"text","text":"Hello there"}},{"role":"assistant","content":{"type":"image","data":"aW1hZ2U=","mimeType":"image/png"}}]})
    } else {
        json!({})
    }
}

/// Build the JSON-RPC text frame a discovery WebSocket connection should return
/// for a single request, using the shared success dataset.
fn discovery_ws_response(request: &str) -> String {
    if request.contains(r#""method":"initialize""#) {
        return format!(r#"{{"jsonrpc":"2.0","id":1,"result":{DISCOVERY_INITIALIZE_RESULT}}}"#);
    }
    json!({"jsonrpc":"2.0","id":2,"result":discovery_success_result(request)}).to_string()
}

/// Accept `connections` WebSocket clients, each performing a handshake then
/// answering exactly two messages (initialize + one discovery method) from the
/// shared success dataset. Each registry discovery call opens a fresh connection.
fn spawn_fake_websocket_discovery_server(connections: usize) -> String {
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake WebSocket discovery server");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("local addr"));
    thread::spawn(move || {
        for _ in 0..connections {
            let (mut stream, _) = listener.accept().expect("accept fake WebSocket request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let _ = read_http_request(&mut stream);
            let handshake = format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                     Upgrade: websocket\r\n\
                     Connection: Upgrade\r\n\
                     Sec-WebSocket-Accept: {WEBSOCKET_ACCEPT}\r\n\r\n"
            );
            stream
                .write_all(handshake.as_bytes())
                .expect("write fake WebSocket handshake");
            for _ in 0..2 {
                let message = read_blocking_websocket_text(&mut stream);
                let response = discovery_ws_response(&message);
                let frame = websocket_server_text_frame(response.as_bytes());
                stream
                    .write_all(&frame)
                    .expect("write fake WebSocket discovery frame");
            }
        }
    });
    endpoint
}

fn decode_base64(input: &str) -> Vec<u8> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 4];
    let mut chunk_len = 0;
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            other => panic!("invalid base64 byte {other}"),
        };
        chunk[chunk_len] = value;
        chunk_len += 1;
        if chunk_len == 4 {
            output.push((chunk[0] << 2) | (chunk[1] >> 4));
            if chunk[2] != 64 {
                output.push((chunk[1] << 4) | (chunk[2] >> 2));
            }
            if chunk[3] != 64 {
                output.push((chunk[2] << 6) | chunk[3]);
            }
            chunk_len = 0;
        }
    }
    output
}

// Throwaway TLS keypair, generated for this test file and used nowhere else. It exists only so the
// `wss://` transport test can complete a real handshake against a loopback listener: self-signed,
// CN/SAN `localhost` + 127.0.0.1, and never installed in any trust store. Publishing the private
// half is deliberate — it authenticates nothing, and a secret scanner flagging it should land here
// and stop. Do not reuse it for anything that is not this test.
//
// Keep this self-signed fixture long-lived so the transport test does not depend on wall-clock
// proximity to the certificate generation date.
const TEST_WSS_CERT_DER_BASE64: &str = "MIIDAzCCAeugAwIBAgIUcgfQ/QpKEv8m9fGu84VjSusre74wDQYJKoZIhvcNAQELBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MCAXDTIwMDEwMTAwMDAwMFoYDzIxMjAwMTAxMDAwMDAwWjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQCbdtMHOysGHDruos+zHqpRvxiPXipIjpTsBVUcKGCa9vuN9UQnirq2VpX9KLdBxiTKKjfm9T4Lj0uZQTChxTp3wWjhCnh2NH2lyGyjmpbimwVHI4XePqYOaaBADaJiD3J6B28evPVErW19SD1qgD5oGLr764PdnkpZIpiq+e8dTz9uQ/eT/DpdJs8l/QVXKAbcwlZT0NHfU1oFZnu/dnh+rAPz7QxoJprtM+dyzEPDPGMVc2FopURYTZGHmM9h+GQNb0WSE/gPkbsk/8djry41p49kGfPqLZE1fg2FddTQM4Jv9AIpph8GgHN0iYQwaJeyrKbJX6Aj8m1V2fmKoucpAgMBAAGjSzBJMBoGA1UdEQQTMBGCCWxvY2FsaG9zdIcEfwAAATAMBgNVHRMBAf8EAjAAMB0GA1UdDgQWBBQEupXH9JDDP77VPxtcQUj6iT9FwzANBgkqhkiG9w0BAQsFAAOCAQEASDpu7hx5FNCgwEd22Jo7xmkO6b32MFWPgOqrYx9CI257cO4iBqWsRwVdZFb9DcuKMv/hfVuFiG0nEYnNgb2X3OcIpzO+0nCis/Hugm+Acf4p6RZXDQJ7tijjK0xgq5swOE+gVwLvWPU6g3SnH7BqmJbkIIfj1UyKjWXJkYvdyoS1tyvnaPecqnXMkJP6rL8P/anm3U/tx8Z+hMTg3vVQ8Ez0O54ar/NbFzK6LgEnxuAn1Q9vi/RDd+w9rQnbmlGDYw7uzfmLq7yG1sX7Y4DI2jJVDBczEAZtVF3TMQ3t/+xYr7Pbuuu0oDB3EXy52vCLMh8nt0fDYyhUP+gyq+HcrQ==";

const TEST_WSS_KEY_DER_BASE64: &str = "MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCbdtMHOysGHDruos+zHqpRvxiPXipIjpTsBVUcKGCa9vuN9UQnirq2VpX9KLdBxiTKKjfm9T4Lj0uZQTChxTp3wWjhCnh2NH2lyGyjmpbimwVHI4XePqYOaaBADaJiD3J6B28evPVErW19SD1qgD5oGLr764PdnkpZIpiq+e8dTz9uQ/eT/DpdJs8l/QVXKAbcwlZT0NHfU1oFZnu/dnh+rAPz7QxoJprtM+dyzEPDPGMVc2FopURYTZGHmM9h+GQNb0WSE/gPkbsk/8djry41p49kGfPqLZE1fg2FddTQM4Jv9AIpph8GgHN0iYQwaJeyrKbJX6Aj8m1V2fmKoucpAgMBAAECgf8jhoOyo1KxksHkxk+wHtHM3F5AZMRE0FA3nwBT7uYkg0v4pJNudcU05ZRgxW0bGqxNhlg/7sq+2X/tBXiXfvpdY1UUF9BvMo+D0skAmdLg9Yu/Nd7ham+H25tDB9qTjfQa7pf17jgd+YOLnXZrX+Li5sPTzX3UptdWhxFcAMEjcR1gGBSGUulEpw1BfIfT9+zWhrjQVq5IEhm5XYeG559LGMzTlCwNZNqlG7oiKj/NYwkRmyiUGRDhbwRb95bBVX7qSPhZbFSwCY4ZRyaW1+/AukXEO8PdrAoq3q62we2KsIdpT1zHyXquxts1/00lBoYRTUb7KzDb0SXSBOd4XMECgYEAyK1wLLCWkFZ3N61Q9xylSkfGO4lqShLmnorBE24m4sVAukoCzftqIosffVIBr7arrfNU61IGm1ZhZq3MTz3XHnu/voHhXsU7JWox0WpsKP7Hm/q59tW3aM+Y8wBYJ5U+gPXErvuL1VrPA9GUPvBXTdFktdaWul7bqe8ccy68c1ECgYEAxlKDB6ItDkXcI+gYNDyz9CE7S6CQIG9tP6i+jfxD/eIIatHUXqvmc9Y1y4gZ3GWt3LZJOjQhT2RixOtfU2hL7yU+QHVQcWu8XD7RPsKWDYt9YNbAH6dVwBMlNpkSHbLI74gEjdx9zRXJVW8WCY1oEsWZk9ADOMueZLb3nnld0FkCgYEAqZOF+v2t/YJTc8UNagPW2RIVvTG9k6KtJsPxq82lJnOw4rqv7AfMBCy0C15E9orSQEgjNkc2NgWkgPPUdhG3upavzPhLzZ21AUTfnCrmAy5o1rHke2TVe8gRYyajV6+SBb+o2ITQARafYIa1UwodfDC5fb8713lY/hyEWgDgVkECgYEAsU2NRWLRAySjxho2oWTvwT+AgoFeuRDFTBdxnQC+TJkqy00inyzxz/ffikH4VNk2kc8KMpXufcsSnliLlcb1tCzItdnr/CrEcHfcnE5c1mkxw0Ta6LGycRDswR4iWxi+mZ8x6x4H/jUPFWlF+25HcBjmz4Y1iy5HQmVthWmd3KkCgYB0ZI0dBP48Dvc+Z8LvRAMTJScsewpmPzisJlo2NrKJgG7Mj8ksWsL811D4p9aT3EBT0v/eXQlkoNuFu3xd/7amG30TdpMBpxTEjoDuK+/PRMKyJaZmx8bveXX0xz6mn5B5ZC/cSmR/WJB09vCjB6WTzLC1/mtyBTYXIQW2SfuPGA==";

fn http_request_host(request: &str) -> &str {
    request
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("host").then_some(value.trim())
        })
        .expect("Host header")
}

#[tokio::test]
async fn diagnose_http_server_sends_static_headers_and_parses_sse() {
    let endpoint = spawn_fake_http_mcp_server(2, |index, request| {
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer static-token"),
            "{request}"
        );
        let body = if index == 0 {
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                }
            })
            .to_string()
        } else {
            format!(
                "event: message\ndata: {}\n\n",
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "result": {
                        "tools": [{
                            "name": "echo",
                            "description": "Echo over HTTP.",
                            "inputSchema": {"type": "object"}
                        }]
                    }
                })
            )
        };
        FakeHttpResponse::ok(body).with_content_type(if index == 0 {
            "application/json"
        } else {
            "text/event-stream"
        })
    });
    let (home, cwd) = temp_paths("diagnose-http");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: [(
                "Authorization".to_string(),
                "Bearer static-token".to_string(),
            )]
            .into(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Unknown,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");
    registry
        .set_server_trust("remote", McpServerTrust::Unknown)
        .await
        .expect("reset trust");

    let checks = registry
        .diagnose_server("remote")
        .await
        .expect("diagnose remote");

    assert!(checks.iter().any(|check| {
        check.name == "trust"
            && check.status == McpDiagnosticStatus::Warn
            && check.detail.contains("runtime calls remain blocked")
    }));
    assert!(checks.iter().any(|check| {
        check.name == "probe"
            && check.status == McpDiagnosticStatus::Pass
            && check.detail.contains("1 tool")
    }));
}

#[tokio::test]
async fn diagnose_http_server_reports_missing_bearer_env_without_trust() {
    let env_var = "ORBCODE_MCP_HTTP_TEST_TOKEN_MISSING_DEFINITELY";
    // SAFETY: this test owns this uniquely named environment variable.
    unsafe { std::env::remove_var(env_var) };
    let (home, cwd) = temp_paths("diagnose-http-auth");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "needs-auth".to_string(),
            transport: McpTransport::Http,
            endpoint: "http://127.0.0.1:1/mcp".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Needs auth".to_string(),
            auth: McpAuth::BearerEnv {
                env_var: env_var.to_string(),
            },
            trust: McpServerTrust::Unknown,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");
    registry
        .set_server_trust("needs-auth", McpServerTrust::Unknown)
        .await
        .expect("reset trust");

    let checks = registry
        .diagnose_server("needs-auth")
        .await
        .expect("diagnose remote");
    let probe = checks
        .iter()
        .find(|check| check.name == "probe")
        .expect("probe check");
    assert_eq!(probe.status, McpDiagnosticStatus::Fail);
    assert!(probe.detail.contains(env_var), "{probe:?}");
}

#[tokio::test]
async fn diagnose_http_server_discovers_oauth_metadata() {
    let endpoint = spawn_fake_http_mcp_server(4, |_index, request| {
        let host = http_request_host(&request);
        let base = format!("http://{host}");
        if request.starts_with("POST /mcp ") {
            return FakeHttpResponse::status("401 Unauthorized", "")
                    .with_header(
                        "WWW-Authenticate",
                        format!(
                            r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource", scope="tools.read""#
                        ),
                    );
        }
        if request.starts_with("GET /.well-known/oauth-protected-resource ") {
            return FakeHttpResponse::ok(
                json!({
                    "resource": format!("{base}/mcp"),
                    "authorization_servers": [format!("{base}/auth")],
                    "scopes_supported": ["tools.read"]
                })
                .to_string(),
            );
        }
        assert!(
            request.starts_with("GET /.well-known/oauth-authorization-server/auth "),
            "{request}"
        );
        FakeHttpResponse::ok(
            json!({
                "issuer": format!("{base}/auth"),
                "authorization_endpoint": format!("{base}/auth/authorize"),
                "token_endpoint": format!("{base}/auth/token"),
                "device_authorization_endpoint": format!("{base}/auth/device"),
                "registration_endpoint": format!("{base}/auth/register"),
                "scopes_supported": ["tools.read", "tools.write"]
            })
            .to_string(),
        )
    });
    let (home, cwd) = temp_paths("diagnose-http-oauth");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "oauth".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "OAuth MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");

    let checks = registry
        .diagnose_server("oauth")
        .await
        .expect("diagnose remote");
    let oauth = checks
        .iter()
        .find(|check| check.name == "oauth")
        .expect("oauth check");
    assert_eq!(oauth.status, McpDiagnosticStatus::Pass);
    assert!(
        oauth.detail.contains("authorization_endpoint="),
        "{oauth:?}"
    );
    assert!(
        oauth.detail.contains("device_authorization_endpoint="),
        "{oauth:?}"
    );
    assert!(oauth.detail.contains("registration_endpoint="), "{oauth:?}");
    assert!(oauth.detail.contains("scopes=tools.read"), "{oauth:?}");
}

#[tokio::test]
async fn diagnose_http_server_warns_when_oauth_metadata_is_missing() {
    let endpoint = spawn_fake_http_mcp_server(2, |_index, _request| {
        FakeHttpResponse::status("401 Unauthorized", "")
    });
    let (home, cwd) = temp_paths("diagnose-http-oauth-missing");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "oauth-missing".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "OAuth missing".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");

    let checks = registry
        .diagnose_server("oauth-missing")
        .await
        .expect("diagnose remote");
    let oauth = checks
        .iter()
        .find(|check| check.name == "oauth")
        .expect("oauth check");
    assert_eq!(oauth.status, McpDiagnosticStatus::Warn);
    assert!(
        oauth
            .detail
            .contains("did not advertise OAuth resource metadata"),
        "{oauth:?}"
    );
}

#[tokio::test]
async fn diagnose_http_server_reports_unreachable_endpoint() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind temporary port");
    let endpoint = format!("http://{}/mcp", listener.local_addr().expect("local addr"));
    drop(listener);
    let (home, cwd) = temp_paths("diagnose-http-unreachable");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "unreachable".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Unreachable".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");

    let checks = registry
        .diagnose_server("unreachable")
        .await
        .expect("diagnose remote");
    let probe = checks
        .iter()
        .find(|check| check.name == "probe")
        .expect("probe check");
    assert_eq!(probe.status, McpDiagnosticStatus::Fail);
    assert!(
        probe.detail.contains("MCP HTTP transport error"),
        "{probe:?}"
    );
}

#[tokio::test]
async fn registry_lists_calls_and_exposes_http_tools_from_real_transport() {
    let env_var = "ORBCODE_MCP_HTTP_RUNTIME_TOKEN_PRESENT";
    // SAFETY: this test owns this uniquely named environment variable.
    unsafe { std::env::set_var(env_var, "runtime-token") };
    let endpoint = spawn_fake_http_mcp_server(6, |_index, request| {
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-api-key: static-secret"),
            "{request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer runtime-token"),
            "{request}"
        );
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                    }
                })
                .to_string(),
            );
        }
        if request.contains(r#""method":"tools/list""#) {
            return FakeHttpResponse::ok(json_rpc_response(
                &request,
                json!({
                        "tools": [{
                            "name": "echo",
                            "description": "Echo over HTTP.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"text": {"type": "string"}}
                            }
                        }]
                }),
            ));
        }
        assert!(request.contains(r#""method":"tools/call""#), "{request}");
        assert!(request.contains(r#""name":"echo""#), "{request}");
        assert!(request.contains(r#""text":"runtime""#), "{request}");
        FakeHttpResponse::ok(json_rpc_response(
            &request,
            json!({
                    "content": [{"type": "text", "text": "http echo: runtime"}],
                    "isError": false
            }),
        ))
    });
    let (home, cwd) = temp_paths("registry-http-runtime");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: [("X-Api-Key".to_string(), "static-secret".to_string())].into(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote MCP".to_string(),
            auth: McpAuth::BearerEnv {
                env_var: env_var.to_string(),
            },
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");

    let tools = registry.list_tools("remote").await.expect("list tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].summary, "Echo over HTTP.");

    let result = registry
        .invoke_tool("remote", "echo", r#"{"text":"runtime"}"#)
        .await
        .expect("invoke HTTP tool");
    assert_eq!(result.output, "http echo: runtime");
    assert!(!result.is_error);

    let provider_tools = registry.list_provider_tools().await;
    let echo = provider_tools
        .iter()
        .find(|tool| tool.server_id == "remote" && tool.tool_name == "echo")
        .expect("HTTP provider tool");
    assert_eq!(echo.input_schema["properties"]["text"]["type"], "string");
    assert!(
        provider_tools
            .iter()
            .all(|tool| !(tool.server_id == "remote" && tool.tool_name == "inspect")),
        "HTTP runtime should not expose seeded modeled tools: {provider_tools:?}"
    );
    // SAFETY: cleanup for other tests.
    unsafe { std::env::remove_var(env_var) };
}

#[tokio::test]
async fn stored_mcp_oauth_token_is_persisted_and_used_for_http_transport() {
    let endpoint = spawn_fake_http_mcp_server(2, |_index, request| {
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer stored-oauth-token"),
            "{request}"
        );
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                    }
                })
                .to_string(),
            );
        }
        assert!(request.contains(r#""method":"tools/list""#), "{request}");
        FakeHttpResponse::ok(json_rpc_response(
            &request,
            json!({
                    "tools": [{
                        "name": "echo",
                        "description": "Echo over stored OAuth.",
                        "inputSchema": {"type": "object"}
                    }]
            }),
        ))
    });
    let (home, cwd) = temp_paths("registry-http-oauth-token");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");
    let entry = registry
        .store_mcp_oauth_token(
            "remote",
            McpOAuthTokenInput {
                access_token: "stored-oauth-token".to_string(),
                refresh_token: Some("refresh-token".to_string()),
                token_endpoint: None,
                client_id: None,
                expires_at: Some(unix_timestamp_now() + 3600),
                scopes: vec!["tools.read".to_string()],
            },
        )
        .await
        .expect("store MCP OAuth token");

    assert!(entry.usable, "{entry:?}");
    assert!(entry.has_refresh_token, "{entry:?}");
    assert_eq!(entry.scopes, vec!["tools.read"]);
    assert!(
        !entry.source_summary.contains("stored-oauth-token"),
        "{entry:?}"
    );

    let reloaded = McpRegistry::load(&home, &cwd)
        .await
        .expect("reload registry");
    let tools = reloaded.list_tools("remote").await.expect("list tools");
    assert_eq!(tools[0].name, "echo");

    assert!(
        reloaded
            .logout_mcp_oauth_token("remote")
            .await
            .expect("logout MCP OAuth token")
    );
    let overview = reloaded
        .mcp_oauth_overview(Some("remote"))
        .await
        .expect("MCP OAuth overview");
    assert!(overview.entries.is_empty(), "{overview:?}");
}

#[tokio::test]
async fn oauth_login_freezes_enforcement_across_endpoint_change() {
    use crate::store::McpOAuthTokenStore;

    let (home, cwd) = temp_paths("oauth-enforce-freeze");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    // The server's CURRENT endpoint is local, so a config-derived enforcement
    // decision would be `false`.
    let local_server = |id: &str| McpServerConfig {
        id: id.to_string(),
        transport: McpTransport::Http,
        endpoint: "http://127.0.0.1:8080/mcp".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        headers: BTreeMap::new(),
        enabled: true,
        status: McpServerStatus::Ready,
        error: None,
        summary: "Remote MCP".to_string(),
        auth: McpAuth::None,
        trust: McpServerTrust::Trusted,
        transport_type_hint: None,
        source: None,
    };
    let token_input = || McpOAuthTokenInput {
        access_token: "tok".to_string(),
        refresh_token: Some("refresh".to_string()),
        token_endpoint: Some("https://as.example.com/token".to_string()),
        client_id: None,
        expires_at: Some(unix_timestamp_now() + 3600),
        scopes: vec!["tools.read".to_string()],
    };
    registry
        .upsert_server(local_server("remote"))
        .await
        .expect("upsert remote");

    let read_enforce = |store_path: std::path::PathBuf| async move {
        let raw = tokio::fs::read_to_string(&store_path)
            .await
            .expect("read oauth_tokens.json");
        let store: McpOAuthTokenStore = serde_json::from_str(&raw).expect("parse token store");
        store.servers["remote"].enforce_ssrf
    };
    let store_path = registry
        .mcp_oauth_overview(Some("remote"))
        .await
        .expect("overview")
        .store_path;

    // A login that STARTED while the endpoint was public froze enforce_ssrf=true.
    // Completing it (endpoint since edited to local) must PERSIST that frozen
    // decision, not re-derive `false` from the now-local config.
    registry
        .store_mcp_oauth_token_inner("remote", token_input(), Some(true))
        .await
        .expect("store frozen token");
    assert!(
        read_enforce(store_path.clone()).await,
        "login-start enforcement (true) must be frozen through the token store despite a local endpoint"
    );

    // The manual store path (no login context) still derives from the current
    // (local) config, i.e. `false`.
    registry
        .store_mcp_oauth_token("remote", token_input())
        .await
        .expect("manual store");
    assert!(
        !read_enforce(store_path).await,
        "manual token store derives enforcement from the current (local) endpoint"
    );
}

#[tokio::test]
async fn expired_mcp_oauth_token_refreshes_before_http_transport_use() {
    let endpoint = spawn_fake_http_mcp_server(3, |_index, request| {
        if request.starts_with("POST /token ") {
            assert!(request.contains("grant_type=refresh_token"), "{request}");
            assert!(request.contains("refresh_token=stale-refresh"), "{request}");
            assert!(request.contains("client_id=orbcode-test"), "{request}");
            return FakeHttpResponse::ok(
                json!({
                    "access_token": "refreshed-token",
                    "refresh_token": "new-refresh",
                    "expires_in": 3600,
                    "scope": "tools.read tools.call"
                })
                .to_string(),
            );
        }
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer refreshed-token"),
            "{request}"
        );
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                    }
                })
                .to_string(),
            );
        }
        assert!(request.contains(r#""method":"tools/list""#), "{request}");
        FakeHttpResponse::ok(json_rpc_response(
            &request,
            json!({
                    "tools": [{
                        "name": "echo",
                        "description": "Echo after refresh.",
                        "inputSchema": {"type": "object"}
                    }]
            }),
        ))
    });
    let token_endpoint = format!("{}/token", endpoint.trim_end_matches("/mcp"));
    let (home, cwd) = temp_paths("registry-http-oauth-refresh");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");
    let entry = registry
        .store_mcp_oauth_token(
            "remote",
            McpOAuthTokenInput {
                access_token: "expired-token".to_string(),
                refresh_token: Some("stale-refresh".to_string()),
                token_endpoint: Some(token_endpoint),
                client_id: Some("orbcode-test".to_string()),
                expires_at: Some(unix_timestamp_now() - 1),
                scopes: vec!["tools.read".to_string()],
            },
        )
        .await
        .expect("store expired MCP OAuth token");
    assert!(entry.expired, "{entry:?}");
    assert!(entry.has_token_endpoint, "{entry:?}");

    let tools = registry.list_tools("remote").await.expect("list tools");
    assert_eq!(tools[0].summary, "Echo after refresh.");

    let overview = registry
        .mcp_oauth_overview(Some("remote"))
        .await
        .expect("MCP OAuth overview");
    let entry = overview.entries.first().expect("refreshed entry");
    assert!(entry.usable, "{entry:?}");
    assert!(!entry.expired, "{entry:?}");
    assert_eq!(entry.scopes, vec!["tools.read", "tools.call"]);
}

#[tokio::test]
async fn mcp_oauth_device_login_polls_and_stores_token() {
    let endpoint = spawn_fake_http_mcp_server(5, |_index, request| {
        let host = http_request_host(&request);
        let base = format!("http://{host}");
        if request.starts_with("POST /mcp ") {
            return FakeHttpResponse::status("401 Unauthorized", "").with_header(
                "WWW-Authenticate",
                format!(
                    r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource""#
                ),
            );
        }
        if request.starts_with("GET /.well-known/oauth-protected-resource ") {
            return FakeHttpResponse::ok(
                json!({
                    "resource": format!("{base}/mcp"),
                    "authorization_servers": [format!("{base}/auth")],
                    "scopes_supported": ["tools.read"]
                })
                .to_string(),
            );
        }
        if request.starts_with("GET /.well-known/oauth-authorization-server/auth ") {
            return FakeHttpResponse::ok(
                json!({
                    "issuer": format!("{base}/auth"),
                    "token_endpoint": format!("{base}/token"),
                    "device_authorization_endpoint": format!("{base}/device"),
                    "scopes_supported": ["tools.read"]
                })
                .to_string(),
            );
        }
        if request.starts_with("POST /device ") {
            assert!(request.contains("client_id=orbcode-test"), "{request}");
            assert!(request.contains("scope=tools.read"), "{request}");
            return FakeHttpResponse::ok(
                json!({
                    "device_code": "device-code",
                    "user_code": "ABCD-EFGH",
                    "verification_uri": "https://auth.example/device",
                    "verification_uri_complete": "https://auth.example/device?user_code=ABCD-EFGH",
                    "expires_in": 600,
                    "interval": 1
                })
                .to_string(),
            );
        }
        assert!(request.starts_with("POST /token "), "{request}");
        assert!(
            request.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"),
            "{request}"
        );
        assert!(request.contains("device_code=device-code"), "{request}");
        FakeHttpResponse::ok(
            json!({
                "access_token": "device-access-token",
                "refresh_token": "device-refresh-token",
                "expires_in": 3600,
                "scope": "tools.read"
            })
            .to_string(),
        )
    });
    let (home, cwd) = temp_paths("registry-http-oauth-device");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");

    let session = registry
        .start_mcp_oauth_device_login(
            "remote",
            McpOAuthDeviceLoginInput {
                device_authorization_endpoint: None,
                token_endpoint: None,
                client_id: "orbcode-test".to_string(),
                scopes: Vec::new(),
            },
        )
        .await
        .expect("start device login");
    assert_eq!(session.user_code, "ABCD-EFGH");
    assert_eq!(
        session.verification_uri_complete.as_deref(),
        Some("https://auth.example/device?user_code=ABCD-EFGH")
    );

    let entry = registry
        .complete_mcp_oauth_device_login(session)
        .await
        .expect("complete device login");
    assert!(entry.usable, "{entry:?}");
    assert!(entry.has_refresh_token, "{entry:?}");
    assert!(entry.has_token_endpoint, "{entry:?}");
    assert_eq!(entry.scopes, vec!["tools.read"]);
    assert!(
        !entry.source_summary.contains("device-access-token"),
        "{entry:?}"
    );
}

#[tokio::test]
async fn mcp_oauth_browser_login_accepts_callback_and_stores_token() {
    let endpoint = spawn_fake_http_mcp_server(4, |_index, request| {
        let host = http_request_host(&request);
        let base = format!("http://{host}");
        if request.starts_with("POST /mcp ") {
            return FakeHttpResponse::status("401 Unauthorized", "").with_header(
                "WWW-Authenticate",
                format!(
                    r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource""#
                ),
            );
        }
        if request.starts_with("GET /.well-known/oauth-protected-resource ") {
            return FakeHttpResponse::ok(
                json!({
                    "resource": format!("{base}/mcp"),
                    "authorization_servers": [format!("{base}/auth")],
                    "scopes_supported": ["tools.read"]
                })
                .to_string(),
            );
        }
        if request.starts_with("GET /.well-known/oauth-authorization-server/auth ") {
            return FakeHttpResponse::ok(
                json!({
                    "issuer": format!("{base}/auth"),
                    "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"),
                    "scopes_supported": ["tools.read"]
                })
                .to_string(),
            );
        }
        assert!(request.starts_with("POST /token "), "{request}");
        assert!(
            request.contains("grant_type=authorization_code"),
            "{request}"
        );
        assert!(request.contains("code=browser-code"), "{request}");
        FakeHttpResponse::ok(
            json!({
                "access_token": "browser-access-token",
                "refresh_token": "browser-refresh-token",
                "expires_in": 3600,
                "scope": "tools.read"
            })
            .to_string(),
        )
    });
    let (home, cwd) = temp_paths("registry-http-oauth-browser");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");

    let session = registry
        .start_mcp_oauth_browser_login(
            "remote",
            McpOAuthBrowserLoginInput {
                authorization_endpoint: None,
                token_endpoint: None,
                client_id: Some("orbcode-test".to_string()),
                registration_endpoint: None,
                scopes: Vec::new(),
                redirect_port: None,
            },
        )
        .await
        .expect("start browser login");
    let authorization_url =
        reqwest::Url::parse(&session.authorization_url).expect("authorization URL");
    let redirect_uri = authorization_url
        .query_pairs()
        .find(|(name, _)| name == "redirect_uri")
        .map(|(_, value)| value.to_string())
        .expect("redirect_uri");
    let state = authorization_url
        .query_pairs()
        .find(|(name, _)| name == "state")
        .map(|(_, value)| value.to_string())
        .expect("state");
    assert_eq!(
        authorization_url
            .query_pairs()
            .find(|(name, _)| name == "scope")
            .map(|(_, value)| value.to_string())
            .as_deref(),
        Some("tools.read")
    );

    let registry_for_login = registry.clone();
    let login = tokio::spawn(async move {
        registry_for_login
            .complete_mcp_oauth_browser_login(session)
            .await
    });
    let callback = format!("{redirect_uri}?code=browser-code&state={state}");
    let callback_response = reqwest::get(callback).await.expect("browser callback");
    assert!(callback_response.status().is_success());

    let entry = login
        .await
        .expect("join browser login")
        .expect("complete browser login");
    assert!(entry.usable, "{entry:?}");
    assert!(entry.has_refresh_token, "{entry:?}");
    assert!(entry.has_token_endpoint, "{entry:?}");
    assert_eq!(entry.scopes, vec!["tools.read"]);
    assert!(
        !entry.source_summary.contains("browser-access-token"),
        "{entry:?}"
    );
}

#[tokio::test]
async fn register_oauth_client_posts_metadata_and_parses_credentials() {
    let endpoint = spawn_fake_http_mcp_server(1, |_index, request| {
        assert!(request.starts_with("POST /register "), "{request}");
        assert!(request.contains("\"redirect_uris\""), "{request}");
        assert!(request.contains("http://127.0.0.1:9/callback"), "{request}");
        assert!(request.contains("\"grant_types\""), "{request}");
        assert!(request.contains("authorization_code"), "{request}");
        assert!(request.contains("refresh_token"), "{request}");
        assert!(request.contains("\"response_types\""), "{request}");
        assert!(
            request.contains("\"token_endpoint_auth_method\":\"none\""),
            "{request}"
        );
        assert!(request.contains("\"scope\":\"tools.read\""), "{request}");
        FakeHttpResponse::ok(
            json!({
                "client_id": "dynamic-client-id",
                "client_secret": "dynamic-client-secret"
            })
            .to_string(),
        )
    });
    let base = endpoint.trim_end_matches("/mcp");
    let registration = register_oauth_client(
        &format!("{base}/register"),
        "http://127.0.0.1:9/callback",
        &["tools.read".to_string()],
        // The fake server binds to loopback, a deliberately-local test target.
        false,
    )
    .await
    .expect("dynamic client registration");
    assert_eq!(registration.client_id, "dynamic-client-id");
    assert_eq!(
        registration.client_secret.as_deref(),
        Some("dynamic-client-secret")
    );
}

#[tokio::test]
async fn register_oauth_client_maps_error_status_to_protocol_error() {
    let endpoint = spawn_fake_http_mcp_server(1, |_index, _request| {
        FakeHttpResponse::status("400 Bad Request", r#"{"error":"invalid_redirect_uri"}"#)
    });
    let base = endpoint.trim_end_matches("/mcp");
    let error = register_oauth_client(
        &format!("{base}/register"),
        "http://127.0.0.1:9/callback",
        &[],
        false,
    )
    .await
    .expect_err("registration should fail");
    match error {
        McpError::Protocol(message) => {
            assert!(message.contains("400"), "{message}");
            assert!(message.contains("invalid_redirect_uri"), "{message}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn register_oauth_client_rejects_response_without_client_id() {
    let endpoint = spawn_fake_http_mcp_server(1, |_index, _request| {
        FakeHttpResponse::ok(json!({ "client_secret": "secret" }).to_string())
    });
    let base = endpoint.trim_end_matches("/mcp");
    let error = register_oauth_client(
        &format!("{base}/register"),
        "http://127.0.0.1:9/callback",
        &[],
        false,
    )
    .await
    .expect_err("registration without client_id should fail");
    match error {
        McpError::Protocol(message) => {
            assert!(message.contains("client_id"), "{message}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_oauth_browser_login_dynamically_registers_client() {
    let endpoint = spawn_fake_http_mcp_server(5, |_index, request| {
        let host = http_request_host(&request);
        let base = format!("http://{host}");
        if request.starts_with("POST /mcp ") {
            return FakeHttpResponse::status("401 Unauthorized", "").with_header(
                "WWW-Authenticate",
                format!(
                    r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource""#
                ),
            );
        }
        if request.starts_with("GET /.well-known/oauth-protected-resource ") {
            return FakeHttpResponse::ok(
                json!({
                    "resource": format!("{base}/mcp"),
                    "authorization_servers": [format!("{base}/auth")],
                    "scopes_supported": ["tools.read"]
                })
                .to_string(),
            );
        }
        if request.starts_with("GET /.well-known/oauth-authorization-server/auth ") {
            return FakeHttpResponse::ok(
                json!({
                    "issuer": format!("{base}/auth"),
                    "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"),
                    "registration_endpoint": format!("{base}/register"),
                    "scopes_supported": ["tools.read"]
                })
                .to_string(),
            );
        }
        if request.starts_with("POST /register ") {
            assert!(request.contains("\"redirect_uris\""), "{request}");
            assert!(request.contains("127.0.0.1"), "{request}");
            assert!(request.contains("authorization_code"), "{request}");
            return FakeHttpResponse::ok(
                json!({
                    "client_id": "dynamic-client-id",
                    "client_secret": "dynamic-client-secret"
                })
                .to_string(),
            );
        }
        assert!(request.starts_with("POST /token "), "{request}");
        assert!(
            request.contains("grant_type=authorization_code"),
            "{request}"
        );
        assert!(request.contains("code=browser-code"), "{request}");
        assert!(request.contains("client_id=dynamic-client-id"), "{request}");
        assert!(
            request.contains("client_secret=dynamic-client-secret"),
            "{request}"
        );
        FakeHttpResponse::ok(
            json!({
                "access_token": "browser-access-token",
                "refresh_token": "browser-refresh-token",
                "expires_in": 3600,
                "scope": "tools.read"
            })
            .to_string(),
        )
    });
    let (home, cwd) = temp_paths("registry-http-oauth-browser-dynamic");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");

    let session = registry
        .start_mcp_oauth_browser_login(
            "remote",
            McpOAuthBrowserLoginInput {
                authorization_endpoint: None,
                token_endpoint: None,
                client_id: None,
                registration_endpoint: None,
                scopes: Vec::new(),
                redirect_port: None,
            },
        )
        .await
        .expect("start browser login");
    let authorization_url =
        reqwest::Url::parse(&session.authorization_url).expect("authorization URL");
    assert_eq!(
        authorization_url
            .query_pairs()
            .find(|(name, _)| name == "client_id")
            .map(|(_, value)| value.to_string())
            .as_deref(),
        Some("dynamic-client-id"),
        "authorization URL should carry the dynamically registered client id"
    );
    let redirect_uri = authorization_url
        .query_pairs()
        .find(|(name, _)| name == "redirect_uri")
        .map(|(_, value)| value.to_string())
        .expect("redirect_uri");
    let state = authorization_url
        .query_pairs()
        .find(|(name, _)| name == "state")
        .map(|(_, value)| value.to_string())
        .expect("state");

    let registry_for_login = registry.clone();
    let login = tokio::spawn(async move {
        registry_for_login
            .complete_mcp_oauth_browser_login(session)
            .await
    });
    let callback = format!("{redirect_uri}?code=browser-code&state={state}");
    let callback_response = reqwest::get(callback).await.expect("browser callback");
    assert!(callback_response.status().is_success());

    let entry = login
        .await
        .expect("join browser login")
        .expect("complete browser login");
    assert!(entry.usable, "{entry:?}");
    assert!(entry.has_refresh_token, "{entry:?}");
    assert!(entry.has_token_endpoint, "{entry:?}");
    assert_eq!(entry.scopes, vec!["tools.read"]);
}

#[tokio::test]
async fn mcp_oauth_browser_login_without_client_id_or_registration_endpoint_errors() {
    let endpoint = spawn_fake_http_mcp_server(3, |_index, request| {
        let host = http_request_host(&request);
        let base = format!("http://{host}");
        if request.starts_with("POST /mcp ") {
            return FakeHttpResponse::status("401 Unauthorized", "").with_header(
                "WWW-Authenticate",
                format!(
                    r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource""#
                ),
            );
        }
        if request.starts_with("GET /.well-known/oauth-protected-resource ") {
            return FakeHttpResponse::ok(
                json!({
                    "resource": format!("{base}/mcp"),
                    "authorization_servers": [format!("{base}/auth")],
                    "scopes_supported": ["tools.read"]
                })
                .to_string(),
            );
        }
        assert!(
            request.starts_with("GET /.well-known/oauth-authorization-server/auth "),
            "{request}"
        );
        // No registration_endpoint advertised.
        FakeHttpResponse::ok(
            json!({
                "issuer": format!("{base}/auth"),
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "scopes_supported": ["tools.read"]
            })
            .to_string(),
        )
    });
    let (home, cwd) = temp_paths("registry-http-oauth-browser-no-registration");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");

    let result = registry
        .start_mcp_oauth_browser_login(
            "remote",
            McpOAuthBrowserLoginInput {
                authorization_endpoint: None,
                token_endpoint: None,
                client_id: None,
                registration_endpoint: None,
                scopes: Vec::new(),
                redirect_port: None,
            },
        )
        .await;
    match result {
        Ok(_) => panic!("missing client id and registration endpoint should fail"),
        Err(error) => assert!(
            matches!(error, McpError::InvalidConfig(_)),
            "unexpected error: {error:?}"
        ),
    }
}

#[tokio::test]
async fn registry_lists_calls_and_exposes_websocket_tools_from_real_transport() {
    let env_var = "ORBCODE_MCP_WS_RUNTIME_TOKEN_PRESENT";
    // SAFETY: this test owns this uniquely named environment variable.
    unsafe { std::env::set_var(env_var, "runtime-token") };
    let endpoint = spawn_fake_websocket_mcp_server(3);
    let (home, cwd) = temp_paths("registry-websocket-runtime");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "remote-ws".to_string(),
            transport: McpTransport::WebSocket,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: [("X-Api-Key".to_string(), "static-secret".to_string())].into(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "WebSocket MCP".to_string(),
            auth: McpAuth::BearerEnv {
                env_var: env_var.to_string(),
            },
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");

    let tools = registry
        .list_tools("remote-ws")
        .await
        .expect("list WebSocket tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].summary, "Echo over WebSocket.");

    let result = registry
        .invoke_tool("remote-ws", "echo", r#"{"text":"runtime"}"#)
        .await
        .expect("invoke WebSocket tool");
    assert_eq!(result.output, "websocket echo: runtime");
    assert!(!result.is_error);

    let provider_tools = registry.list_provider_tools().await;
    let echo = provider_tools
        .iter()
        .find(|tool| tool.server_id == "remote-ws" && tool.tool_name == "echo")
        .expect("WebSocket provider tool");
    assert_eq!(echo.input_schema["properties"]["text"]["type"], "string");
    assert!(
        provider_tools
            .iter()
            .all(|tool| !(tool.server_id == "remote-ws" && tool.tool_name == "inspect")),
        "WebSocket runtime should not expose seeded modeled tools: {provider_tools:?}"
    );
    // SAFETY: cleanup for other tests.
    unsafe { std::env::remove_var(env_var) };
}

async fn discovery_remote_registry(
    label: &str,
    transport: McpTransport,
    endpoint: String,
) -> McpRegistry {
    let (home, cwd) = temp_paths(label);
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");
    registry
}

fn assert_discovery_success_dataset(
    resources: &[McpResourceSummary],
    templates: &[McpResourceTemplate],
    text: &McpResourceContent,
    binary: &McpResourceContent,
    prompts: &[McpPrompt],
    prompt: &McpPromptResult,
) {
    assert_eq!(resources.len(), 2);
    let text_summary = resources
        .iter()
        .find(|resource| resource.uri == "res://text")
        .expect("text resource");
    let annotations = text_summary.annotations.as_ref().expect("annotations");
    assert_eq!(annotations.audience, vec!["user", "assistant"]);
    assert_eq!(annotations.priority, Some(0.8));
    assert!(
        resources
            .iter()
            .find(|resource| resource.uri == "res://binary")
            .expect("binary resource")
            .annotations
            .is_none()
    );

    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0].uri_template, "res://items/{itemId}");
    assert_eq!(
        templates[0]
            .annotations
            .as_ref()
            .expect("priority")
            .priority,
        Some(0.5)
    );

    assert!(!text.is_binary);
    assert_eq!(text.contents, "hello text");
    assert!(text.blob.is_none());

    assert!(binary.is_binary);
    assert!(binary.contents.is_empty(), "binary must not pollute text");
    assert_eq!(binary.blob.as_deref(), Some("aGVsbG8="));

    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "greet");
    assert!(prompts[0].skill);
    assert!(prompts[0].arguments[0].required);

    assert_eq!(prompt.messages.len(), 2);
    assert_eq!(
        prompt.messages[0].content.text.as_deref(),
        Some("Hello there")
    );
    assert!(!prompt.messages[0].content.is_binary);
    assert!(prompt.messages[1].content.is_binary);
    assert_eq!(
        prompt.messages[1].content.binary.as_deref(),
        Some("aW1hZ2U=")
    );
    assert_eq!(prompt.messages[1].content.mime_type, "image/png");
}

fn discovery_http_response(request: &str) -> FakeHttpResponse {
    if request.contains(r#""method":"initialize""#) {
        return FakeHttpResponse::ok(format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{DISCOVERY_INITIALIZE_RESULT}}}"#
        ));
    }
    FakeHttpResponse::ok(json_rpc_response(
        request,
        discovery_success_result(request),
    ))
}

#[tokio::test]
async fn registry_http_discovery_success_dataset() {
    // 6 discovery calls share one initialized Streamable HTTP client.
    let endpoint =
        spawn_fake_http_mcp_server(12, |_index, request| discovery_http_response(&request));
    let registry =
        discovery_remote_registry("http-discovery-success", McpTransport::Http, endpoint).await;

    let resources = registry
        .discover_resources("remote")
        .await
        .expect("resources");
    let templates = registry
        .discover_resource_templates("remote")
        .await
        .expect("templates");
    let text = registry
        .read_resource_content("remote", "res://text")
        .await
        .expect("text");
    let binary = registry
        .read_resource_content("remote", "res://binary")
        .await
        .expect("binary");
    let prompts = registry.list_prompts("remote").await.expect("prompts");
    let prompt = registry
        .get_prompt("remote", "greet", json!({ "name": "world" }))
        .await
        .expect("prompt");

    assert_discovery_success_dataset(&resources, &templates, &text, &binary, &prompts, &prompt);
}

#[tokio::test]
async fn session_scoped_discovery_apis_resolve_resources_and_prompts() {
    // 6 discovery calls share one initialized Streamable HTTP client.
    let endpoint =
        spawn_fake_http_mcp_server(12, |_index, request| discovery_http_response(&request));
    let (home, cwd) = temp_paths("session-discovery-success");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    let accepted = registry
        .upsert_session_servers(
            "session-a",
            vec![test_mcp_server_config_with_transport(
                "remote",
                McpTransport::Http,
                &endpoint,
            )],
        )
        .await;
    let server_id = accepted[0].id.clone();
    registry
        .set_server_trust_for_session("session-a", &server_id, McpServerTrust::Trusted)
        .await
        .expect("trust session server");

    assert!(matches!(
        registry.discover_resources(&server_id).await,
        Err(McpError::UnknownServer(_))
    ));
    assert!(matches!(
        registry.list_prompts(&server_id).await,
        Err(McpError::UnknownServer(_))
    ));

    let resources = registry
        .discover_resources_for_session("session-a", &server_id)
        .await
        .expect("resources");
    let templates = registry
        .discover_resource_templates_for_session("session-a", &server_id)
        .await
        .expect("templates");
    let text = registry
        .read_resource_content_for_session("session-a", &server_id, "res://text")
        .await
        .expect("text");
    let binary = registry
        .read_resource_content_for_session("session-a", &server_id, "res://binary")
        .await
        .expect("binary");
    let prompts = registry
        .list_prompts_for_session("session-a", &server_id)
        .await
        .expect("prompts");
    let prompt = registry
        .get_prompt_for_session("session-a", &server_id, "greet", json!({ "name": "world" }))
        .await
        .expect("prompt");

    assert_discovery_success_dataset(&resources, &templates, &text, &binary, &prompts, &prompt);
}

#[tokio::test]
async fn registry_http_discovery_handles_empty_lists() {
    let endpoint = spawn_fake_http_mcp_server(6, |_index, request| {
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{DISCOVERY_INITIALIZE_RESULT}}}"#
            ));
        }
        let result = if request.contains(r#""method":"resources/templates/list""#) {
            json!({"resourceTemplates": []})
        } else if request.contains(r#""method":"resources/list""#) {
            json!({"resources": []})
        } else {
            json!({"prompts": []})
        };
        FakeHttpResponse::ok(json_rpc_response(&request, result))
    });
    let registry =
        discovery_remote_registry("http-discovery-empty", McpTransport::Http, endpoint).await;

    assert!(
        registry
            .discover_resources("remote")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        registry
            .discover_resource_templates("remote")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(registry.list_prompts("remote").await.unwrap().is_empty());
}

#[tokio::test]
async fn registry_http_discovery_reports_schema_mismatch() {
    let endpoint = spawn_fake_http_mcp_server(2, |_index, request| {
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{DISCOVERY_INITIALIZE_RESULT}}}"#
            ));
        }
        FakeHttpResponse::ok(json_rpc_response(
            &request,
            json!({"resources":"not-an-array"}),
        ))
    });
    let registry =
        discovery_remote_registry("http-discovery-schema", McpTransport::Http, endpoint).await;

    let error = registry
        .discover_resources("remote")
        .await
        .expect_err("malformed resources/list should fail");
    assert!(matches!(error, McpError::Json(_)), "{error}");
    let remote = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "remote")
        .expect("remote server");
    assert_eq!(remote.status, McpServerStatus::Failed);
}

#[tokio::test]
async fn registry_http_discovery_surfaces_resource_error() {
    let endpoint = spawn_fake_http_mcp_server(2, |_index, request| {
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{DISCOVERY_INITIALIZE_RESULT}}}"#
            ));
        }
        FakeHttpResponse::ok(
            json!({
                "jsonrpc":"2.0",
                "id":http_json_rpc_id(&request),
                "error":{"code":-32002,"message":"resource not found"}
            })
            .to_string(),
        )
    });
    let registry =
        discovery_remote_registry("http-discovery-error", McpTransport::Http, endpoint).await;

    let error = registry
        .read_resource_content("remote", "res://missing")
        .await
        .expect_err("unknown resource should error");
    assert!(
        matches!(error, McpError::JsonRpc { code: -32002, .. }),
        "{error}"
    );
    let remote = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "remote")
        .expect("remote server");
    assert_eq!(remote.status, McpServerStatus::Failed);
}

#[tokio::test]
async fn registry_websocket_discovery_success_dataset() {
    let endpoint = spawn_fake_websocket_discovery_server(6);
    let registry =
        discovery_remote_registry("ws-discovery-success", McpTransport::WebSocket, endpoint).await;

    let resources = registry
        .discover_resources("remote")
        .await
        .expect("resources");
    let templates = registry
        .discover_resource_templates("remote")
        .await
        .expect("templates");
    let text = registry
        .read_resource_content("remote", "res://text")
        .await
        .expect("text");
    let binary = registry
        .read_resource_content("remote", "res://binary")
        .await
        .expect("binary");
    let prompts = registry.list_prompts("remote").await.expect("prompts");
    let prompt = registry
        .get_prompt("remote", "greet", json!({ "name": "world" }))
        .await
        .expect("prompt");

    assert_discovery_success_dataset(&resources, &templates, &text, &binary, &prompts, &prompt);
}

#[tokio::test]
async fn registry_websocket_discovery_handles_empty_list() {
    let endpoint = spawn_fake_websocket_mcp_server_with_responses(vec![
        format!(r#"{{"jsonrpc":"2.0","id":1,"result":{DISCOVERY_INITIALIZE_RESULT}}}"#),
        json!({"jsonrpc":"2.0","id":2,"result":{"resources":[]}}).to_string(),
    ]);
    let registry =
        discovery_remote_registry("ws-discovery-empty", McpTransport::WebSocket, endpoint).await;

    assert!(
        registry
            .discover_resources("remote")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn registry_websocket_discovery_reports_schema_mismatch() {
    let endpoint = spawn_fake_websocket_mcp_server_with_responses(vec![
        format!(r#"{{"jsonrpc":"2.0","id":1,"result":{DISCOVERY_INITIALIZE_RESULT}}}"#),
        json!({"jsonrpc":"2.0","id":2,"result":{"prompts":"not-an-array"}}).to_string(),
    ]);
    let registry =
        discovery_remote_registry("ws-discovery-schema", McpTransport::WebSocket, endpoint).await;

    let error = registry
        .list_prompts("remote")
        .await
        .expect_err("malformed prompts/list should fail");
    assert!(matches!(error, McpError::Json(_)), "{error}");
    let remote = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "remote")
        .expect("remote server");
    assert_eq!(remote.status, McpServerStatus::Failed);
}

#[tokio::test]
async fn registry_websocket_discovery_surfaces_resource_error() {
    let endpoint = spawn_fake_websocket_mcp_server_with_responses(vec![
        format!(r#"{{"jsonrpc":"2.0","id":1,"result":{DISCOVERY_INITIALIZE_RESULT}}}"#),
        json!({"jsonrpc":"2.0","id":2,"error":{"code":-32002,"message":"resource not found"}})
            .to_string(),
    ]);
    let registry =
        discovery_remote_registry("ws-discovery-error", McpTransport::WebSocket, endpoint).await;

    let error = registry
        .read_resource_content("remote", "res://missing")
        .await
        .expect_err("unknown resource should error");
    assert!(
        matches!(error, McpError::JsonRpc { code: -32002, .. }),
        "{error}"
    );
    let remote = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "remote")
        .expect("remote server");
    assert_eq!(remote.status, McpServerStatus::Failed);
}

#[tokio::test]
async fn websocket_client_uses_tls_for_wss_transport() {
    let (endpoint, cert_der) = spawn_fake_tls_websocket_mcp_server(1).await;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls_pki_types::CertificateDer::from(cert_der))
        .expect("add test root");
    let env_var = "ORBCODE_MCP_WSS_RUNTIME_TOKEN_PRESENT";
    // SAFETY: this test owns this uniquely named environment variable.
    unsafe { std::env::set_var(env_var, "runtime-token") };
    let config = McpServerConfig {
        id: "remote-wss".to_string(),
        transport: McpTransport::WebSocket,
        endpoint,
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        headers: [("X-Api-Key".to_string(), "static-secret".to_string())].into(),
        enabled: true,
        status: McpServerStatus::Ready,
        error: None,
        summary: "TLS WebSocket MCP".to_string(),
        auth: McpAuth::BearerEnv {
            env_var: env_var.to_string(),
        },
        trust: McpServerTrust::Trusted,
        transport_type_hint: None,
        source: None,
    };
    let headers = effective_http_headers(&config, None).expect("headers");
    let mut client = WebSocketMcpClient::connect_with_root_store(
        &config.endpoint,
        headers,
        Duration::from_secs(2),
        roots,
    )
    .await
    .expect("connect wss client");

    client.initialize().await.expect("initialize");
    let tools = client.list_tools().await.expect("list tools").tools;
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].description, "Echo over WebSocket.");
    // SAFETY: cleanup for other tests.
    unsafe { std::env::remove_var(env_var) };
}

#[tokio::test]
async fn http_client_reports_auth_required_status() {
    let endpoint = spawn_fake_http_mcp_server(1, |_index, _request| {
        FakeHttpResponse::status("401 Unauthorized", "")
    });
    let mut client = HttpMcpClient::new(endpoint, HeaderMap::new(), Duration::from_secs(1))
        .expect("http client");

    let error = client
        .initialize()
        .await
        .expect_err("401 should require auth");

    assert!(matches!(error, McpError::AuthRequired { .. }), "{error}");
}

#[tokio::test]
async fn http_client_times_out_slow_response() {
    let endpoint = spawn_fake_http_mcp_server(1, |_index, _request| {
        FakeHttpResponse::ok(json!({"jsonrpc":"2.0","id":1,"result":{}}).to_string())
            .with_delay(Duration::from_millis(200))
    });
    let mut client = HttpMcpClient::new(endpoint, HeaderMap::new(), Duration::from_millis(20))
        .expect("http client");

    let error = client
        .initialize()
        .await
        .expect_err("slow response should time out");

    assert!(matches!(error, McpError::Timeout(method) if method == "http initialize"));
}

#[tokio::test]
async fn http_client_reports_schema_errors() {
    let endpoint = spawn_fake_http_mcp_server(2, |index, _request| {
        let body = if index == 0 {
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                }
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {"tools": "not-an-array"}
            })
        };
        FakeHttpResponse::ok(body.to_string())
    });
    let mut client = HttpMcpClient::new(endpoint, HeaderMap::new(), Duration::from_secs(1))
        .expect("http client");

    client.initialize().await.expect("initialize");
    let error = client
        .list_tools()
        .await
        .expect_err("invalid schema should fail");

    assert!(matches!(error, McpError::Json(_)), "{error}");
}

#[tokio::test]
async fn http_client_propagates_tool_error_with_is_error_true() {
    let endpoint = spawn_fake_http_mcp_server(2, |index, _request| {
        let body = if index == 0 {
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                }
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "content": [{"type": "text", "text": "boom: bad input"}],
                    "isError": true
                }
            })
        };
        FakeHttpResponse::ok(body.to_string())
    });
    let mut client = HttpMcpClient::new(endpoint, HeaderMap::new(), Duration::from_secs(1))
        .expect("http client");

    client.initialize().await.expect("initialize");
    let result = client
        .call_tool("echo", json!({"text": "hello"}))
        .await
        .expect("call_tool transport must succeed even when isError=true");

    assert!(result.is_error, "expected isError=true to propagate");
    assert_eq!(result.content.len(), 1);
    assert_eq!(result.content[0].kind, "text");
    assert_eq!(result.content[0].text.as_deref(), Some("boom: bad input"));
}

#[tokio::test]
async fn websocket_client_times_out_slow_handshake() {
    let endpoint = spawn_slow_fake_websocket_mcp_server(Duration::from_millis(200));
    let outcome =
        WebSocketMcpClient::connect(&endpoint, HeaderMap::new(), Duration::from_millis(20)).await;
    match outcome {
        Err(McpError::Timeout(method)) => {
            assert!(
                method == "websocket handshake" || method == "websocket connect",
                "unexpected timeout method: {method}"
            );
        }
        Err(other) => panic!("expected handshake/connect timeout, got {other}"),
        Ok(_) => panic!("expected timeout, got connected client"),
    }
}

#[tokio::test]
async fn websocket_client_reports_auth_required_status() {
    let endpoint = spawn_fake_websocket_mcp_server_with_handshake_status(
        "HTTP/1.1 401 Unauthorized\r\n\
             WWW-Authenticate: Bearer realm=\"mcp\"\r\n\
             Content-Length: 0\r\n\
             Connection: close\r\n\r\n",
    );
    let outcome =
        WebSocketMcpClient::connect(&endpoint, HeaderMap::new(), Duration::from_secs(1)).await;
    match outcome {
        Err(McpError::AuthRequired { server, reason }) => {
            assert_eq!(server, endpoint);
            assert!(reason.contains("401"), "{reason}");
            assert!(reason.contains("WWW-Authenticate"), "{reason}");
        }
        Err(other) => panic!("expected AuthRequired, got {other}"),
        Ok(_) => panic!("expected AuthRequired, got connected client"),
    }
}

#[tokio::test]
async fn websocket_client_reports_schema_errors() {
    let endpoint = spawn_fake_websocket_mcp_server_with_responses(vec![
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake-ws", "version": "0.1.0"}
            }
        })
        .to_string(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {"tools": "not-an-array"}
        })
        .to_string(),
    ]);
    let mut client =
        WebSocketMcpClient::connect(&endpoint, HeaderMap::new(), Duration::from_secs(1))
            .await
            .expect("connect ws");
    client.initialize().await.expect("initialize");
    let error = client
        .list_tools()
        .await
        .expect_err("invalid WebSocket tools/list schema should fail");
    assert!(matches!(error, McpError::Json(_)), "{error}");
}

#[tokio::test]
async fn websocket_client_propagates_tool_error_with_is_error_true() {
    let endpoint = spawn_fake_websocket_mcp_server_with_responses(vec![
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake-ws", "version": "0.1.0"}
            }
        })
        .to_string(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "content": [{"type": "text", "text": "ws boom"}],
                "isError": true
            }
        })
        .to_string(),
    ]);
    let mut client =
        WebSocketMcpClient::connect(&endpoint, HeaderMap::new(), Duration::from_secs(1))
            .await
            .expect("connect ws");
    client.initialize().await.expect("initialize");
    let result = client
        .call_tool("echo", json!({"text": "ignored"}))
        .await
        .expect("call_tool transport must succeed even when isError=true");
    assert!(result.is_error, "expected isError=true to propagate");
    assert_eq!(result.content.len(), 1);
    assert_eq!(result.content[0].text.as_deref(), Some("ws boom"));
}

#[tokio::test]
async fn plugin_mcp_sources_are_scoped_and_keep_metadata() {
    let (home, cwd) = temp_paths("plugin-sources");
    let plugin_root = home.join("plugins").join("demo");
    let mcp_path = plugin_root.join(".mcp.json");
    let endpoint = spawn_fake_websocket_mcp_server_with_auth_checks(1, false);
    std::fs::create_dir_all(&plugin_root).expect("plugin root");
    std::fs::write(
            &mcp_path,
            format!(
                r#"{{"mcpServers":{{"docs":{{"type":"ws","url":"{endpoint}","summary":"Plugin docs"}}}}}}"#
            ),
        )
        .expect("write plugin mcp");

    let registry = McpRegistry::load_with_options(
        &home,
        &cwd,
        McpLoadOptions {
            plugin_sources: vec![McpPluginConfigSource {
                plugin_id: "demo@market".to_string(),
                plugin_name: "demo".to_string(),
                label: mcp_path.display().to_string(),
                kind: McpPluginConfigSourceKind::File(mcp_path.clone()),
            }],
            ..McpLoadOptions::default()
        },
    )
    .await
    .expect("load registry");

    let server_id = scoped_plugin_server_id("demo@market", "docs");
    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|server| server.id == server_id)
        .expect("plugin server");

    assert_eq!(server.trust, McpServerTrust::Trusted);
    assert!(matches!(
        server.source.as_ref(),
        Some(McpServerSource::Plugin(source))
            if source.plugin_id == "demo@market"
                && source.plugin_name == "demo"
                && source.server_name == "docs"
                && source.source == mcp_path.display().to_string()
    ));

    let provider_tools = registry.list_provider_tools().await;
    let echo = provider_tools
        .iter()
        .find(|tool| tool.server_id == server_id && tool.tool_name == "echo")
        .expect("plugin provider tool");
    assert!(matches!(
        echo.source.as_ref(),
        Some(McpServerSource::Plugin(source)) if source.plugin_id == "demo@market"
    ));
}

#[tokio::test]
async fn persists_servers_and_lists_resources() {
    let (home, cwd) = temp_paths("persist");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "docs".to_string(),
            transport: McpTransport::Https,
            endpoint: "https://example.com/mcp".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Docs MCP".to_string(),
            auth: McpAuth::BearerEnv {
                env_var: "DOCS_TOKEN".to_string(),
            },
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert server");

    let servers = registry.list_servers().await;
    assert!(servers.iter().any(|server| server.id == "docs"));

    let resources = registry
        .list_resources("docs")
        .await
        .expect("list resources");
    assert!(
        resources
            .iter()
            .any(|resource| resource.uri == "mcp://docs/info")
    );
}

#[tokio::test]
async fn invokes_persisted_tools() {
    let (home, cwd) = temp_paths("tools");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");

    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::WebSocket,
            endpoint: "modeled://localhost:8080/mcp".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert remote");
    let remote_result = registry
        .invoke_tool("remote", "inspect", "")
        .await
        .expect("call remote inspect");
    assert!(remote_result.output.contains("server=remote"));
}

#[tokio::test]
async fn loads_mcp_json_settings_and_cli_config_with_precedence() {
    let (home, cwd) = temp_paths("config-layers");
    std::fs::create_dir_all(cwd.join(".claude")).expect("create settings dir");
    std::fs::write(
        home.join("settings.json"),
        r#"{
                "env": {"TOKEN": "settings-token"},
                "mcpServers": {
                    "shared": {"type": "http", "url": "https://user.example/mcp"},
                    "user_stdio": {"command": "uvx", "args": ["tool"]}
                }
            }"#,
    )
    .expect("write user settings");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{
                "mcpServers": {
                    "shared": {
                        "type": "stdio",
                        "command": "node"
                    },
                    "project_stdio": {
                        "type": "stdio",
                        "command": "node",
                        "args": ["${TOKEN}", "${MISSING:-fallback}"],
                        "env": {"API_TOKEN": "${TOKEN}"},
                        "cwd": "server"
                    },
                    "disabled_project": {
                        "type": "http",
                        "url": "https://disabled.example/mcp"
                    }
                },
                "disabledMcpjsonServers": ["disabled_project"]
            }"#,
    )
    .expect("write mcp json");
    std::fs::write(
        cwd.join(".claude/settings.json"),
        r#"{
                "mcpServers": {
                    "project_remote": {
                        "type": "http",
                        "url": "https://project.example/mcp"
                    }
                }
            }"#,
    )
    .expect("write project settings");
    std::fs::write(
        cwd.join(".claude/settings.local.json"),
        r#"{
                "mcpServers": {
                    "local_remote": {
                        "type": "http",
                        "url": "https://local.example/${TOKEN}",
                        "headers": {"Authorization": "Bearer ${TOKEN}"}
                    },
                    "disabled_flag": {
                        "type": "http",
                        "url": "https://flag.example/mcp",
                        "disabled": true
                    }
                }
            }"#,
    )
    .expect("write local settings");
    std::fs::write(
        cwd.join("cli-mcp.json"),
        r#"{
                "mcpServers": {
                    "cli_file": {
                        "type": "ws",
                        "url": "wss://example.com/mcp"
                    }
                }
            }"#,
    )
    .expect("write cli config file");

    let registry = McpRegistry::load_with_options(
        &home,
        &cwd,
        McpLoadOptions {
            config_inputs: vec![
                "cli-mcp.json".to_string(),
                r#"{"mcpServers":{"shared":{"type":"http","url":"https://cli.example/mcp"}}}"#
                    .to_string(),
            ],
            env: [("TOKEN".to_string(), "settings-token".to_string())].into(),
            plugin_sources: Vec::new(),
        },
    )
    .await
    .expect("load registry");

    let servers = registry.list_servers().await;
    let shared = servers
        .iter()
        .find(|server| server.id == "shared")
        .expect("shared server");
    assert_eq!(shared.transport, McpTransport::StreamableHttp);
    assert_eq!(shared.endpoint, "https://cli.example/mcp");

    let user_stdio = servers
        .iter()
        .find(|server| server.id == "user_stdio")
        .expect("user stdio server");
    assert_eq!(user_stdio.endpoint, "uvx");
    assert_eq!(user_stdio.args, vec!["tool"]);

    let project_stdio = servers
        .iter()
        .find(|server| server.id == "project_stdio")
        .expect("project stdio server");
    assert_eq!(project_stdio.endpoint, "node");
    assert_eq!(project_stdio.args, vec!["settings-token", "fallback"]);
    assert_eq!(
        project_stdio.env.get("API_TOKEN").map(String::as_str),
        Some("settings-token")
    );
    assert_eq!(project_stdio.cwd.as_deref(), Some("server"));

    let local_remote = servers
        .iter()
        .find(|server| server.id == "local_remote")
        .expect("local remote server");
    assert_eq!(
        local_remote.endpoint,
        "https://local.example/settings-token"
    );
    assert_eq!(
        local_remote
            .headers
            .get("Authorization")
            .map(String::as_str),
        Some("Bearer settings-token")
    );
    assert!(
        servers.iter().any(|server| server.id == "project_remote"),
        "project settings MCP entry should load"
    );
    let cli_file = servers
        .iter()
        .find(|server| server.id == "cli_file")
        .expect("--mcp-config file server");
    assert_eq!(cli_file.transport, McpTransport::WebSocket);
    assert_eq!(cli_file.endpoint, "wss://example.com/mcp");

    let disabled = servers
        .iter()
        .find(|server| server.id == "disabled_project")
        .expect("disabled project server");
    assert!(!disabled.enabled);
    let disabled_flag = servers
        .iter()
        .find(|server| server.id == "disabled_flag")
        .expect("disabled flag server");
    assert!(!disabled_flag.enabled);
}

#[tokio::test]
async fn reports_invalid_mcp_config_diagnostics() {
    let (home, cwd) = temp_paths("config-errors");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{"mcpServers":{"bad":{"type":"stdio","args":["missing-command"]}}}"#,
    )
    .expect("write invalid config");

    let error = match McpRegistry::load(&home, &cwd).await {
        Ok(_) => panic!("invalid config should fail"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(message.contains("invalid MCP configuration"), "{message}");
    assert!(message.contains("mcpServers.bad.command"), "{message}");
    assert!(
        message.contains("stdio MCP servers require a non-empty command"),
        "{message}"
    );
}

#[tokio::test]
async fn reports_missing_env_vars_in_mcp_config() {
    let (home, cwd) = temp_paths("config-env-errors");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{"mcpServers":{"bad":{"type":"http","url":"https://${MISSING}.example/mcp"}}}"#,
    )
    .expect("write invalid config");

    let error = match McpRegistry::load(&home, &cwd).await {
        Ok(_) => panic!("missing env should fail"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(message.contains("missing environment variable `MISSING`"));
}

#[tokio::test]
async fn mcp_json_servers_default_to_unknown_trust_user_settings_to_trusted() {
    let (home, cwd) = temp_paths("trust-default");
    std::fs::create_dir_all(cwd.join(".claude")).expect("create settings dir");
    std::fs::write(
        home.join("settings.json"),
        r#"{"mcpServers":{"from_user":{"command":"echo"}}}"#,
    )
    .expect("write user settings");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{"mcpServers":{"from_project":{"command":"echo"}}}"#,
    )
    .expect("write mcp json");

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    assert_eq!(
        registry.server_trust("from_project").await,
        McpServerTrust::Unknown
    );
    assert_eq!(
        registry.server_trust("from_user").await,
        McpServerTrust::Trusted
    );
}

#[tokio::test]
async fn untrusted_server_tool_calls_return_server_untrusted_error() {
    let (home, cwd) = temp_paths("trust-gate");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{"mcpServers":{"docs":{"type":"ws","url":"wss://docs.example/mcp"}}}"#,
    )
    .expect("write mcp json");

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    let list_error = registry
        .list_tools("docs")
        .await
        .expect_err("list_tools should refuse untrusted server");
    match list_error {
        McpError::ServerUntrusted { server, status } => {
            assert_eq!(server, "docs");
            assert_eq!(status, "unknown");
        }
        other => panic!("expected ServerUntrusted, got {other}"),
    }
    let invoke_error = registry
        .invoke_tool("docs", "inspect", "")
        .await
        .expect_err("invoke_tool should refuse untrusted server");
    assert!(matches!(invoke_error, McpError::ServerUntrusted { .. }));
    let resource_error = registry
        .list_resources("docs")
        .await
        .expect_err("list_resources should refuse untrusted server");
    assert!(matches!(resource_error, McpError::ServerUntrusted { .. }));
}

#[tokio::test]
async fn set_server_trust_unblocks_calls_and_persists_across_reload() {
    let (home, cwd) = temp_paths("trust-persist");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .upsert_server(McpServerConfig {
            id: "docs".to_string(),
            transport: McpTransport::WebSocket,
            endpoint: "modeled://docs.example/mcp".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Docs MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("seed docs");
    registry
        .set_server_trust("docs", McpServerTrust::Unknown)
        .await
        .expect("reset trust docs");
    registry
        .set_server_trust("docs", McpServerTrust::Trusted)
        .await
        .expect("trust docs");
    let result = registry
        .invoke_tool("docs", "inspect", "")
        .await
        .expect("invoke trusted server");
    assert!(result.output.contains("server=docs"));

    let reloaded = McpRegistry::load(&home, &cwd).await.expect("reload");
    assert_eq!(
        reloaded.server_trust("docs").await,
        McpServerTrust::Trusted,
        "trust state should survive a registry reload"
    );

    reloaded
        .set_server_trust("docs", McpServerTrust::Denied)
        .await
        .expect("deny docs");
    let denied_error = reloaded
        .invoke_tool("docs", "inspect", "")
        .await
        .expect_err("denied server refuses calls");
    match denied_error {
        McpError::ServerUntrusted { server, status } => {
            assert_eq!(server, "docs");
            assert_eq!(status, "denied");
        }
        other => panic!("expected ServerUntrusted denied, got {other}"),
    }
}

#[tokio::test]
async fn set_server_trust_persists_to_settings_layer_without_trust_json() {
    let (home, cwd) = temp_paths("trust-settings-layer");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{"mcpServers":{"docs":{"type":"ws","url":"wss://docs.example/mcp"}}}"#,
    )
    .expect("write mcp json");

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    assert_eq!(
        registry.server_trust("docs").await,
        McpServerTrust::Unknown,
        ".mcp.json servers start untrusted"
    );
    registry
        .set_server_trust("docs", McpServerTrust::Trusted)
        .await
        .expect("trust docs");

    // The decision is mirrored into the User settings layer (TS-compatible key).
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("settings.json")).unwrap())
            .expect("parse settings");
    let enabled = settings
        .get("enabledMcpjsonServers")
        .and_then(Value::as_array)
        .expect("enabledMcpjsonServers array");
    assert!(enabled.iter().any(|value| value.as_str() == Some("docs")));

    // Remove the legacy trust store: a fresh registry must still see the
    // trust from the settings layer alone.
    std::fs::remove_file(home.join("mcp").join("trust.json")).expect("remove trust.json");
    let reloaded = McpRegistry::load(&home, &cwd).await.expect("reload");
    assert_eq!(
        reloaded.server_trust("docs").await,
        McpServerTrust::Trusted,
        "settings-layer trust survives without trust.json"
    );

    // Denial likewise persists through the settings layer.
    reloaded
        .set_server_trust("docs", McpServerTrust::Denied)
        .await
        .expect("deny docs");
    std::fs::remove_file(home.join("mcp").join("trust.json")).expect("remove trust.json again");
    let denied = McpRegistry::load(&home, &cwd).await.expect("reload denied");
    assert_eq!(denied.server_trust("docs").await, McpServerTrust::Denied);
    let error = denied
        .list_tools("docs")
        .await
        .expect_err("denied server refuses listing");
    assert!(matches!(error, McpError::ServerUntrusted { .. }));
}

#[test]
fn bearer_env_missing_returns_auth_required() {
    let env_var = "ORBCODE_MCP_TEST_TOKEN_MISSING_DEFINITELY";
    // SAFETY: tests are single-threaded per process and we restore the var below.
    unsafe { std::env::remove_var(env_var) };
    let config = McpServerConfig {
        id: "needs-auth".to_string(),
        transport: McpTransport::Stdio,
        endpoint: "fake-bin".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        headers: BTreeMap::new(),
        enabled: true,
        status: McpServerStatus::Ready,
        error: None,
        summary: "needs auth".to_string(),
        auth: McpAuth::BearerEnv {
            env_var: env_var.to_string(),
        },
        trust: McpServerTrust::Trusted,
        transport_type_hint: None,
        source: None,
    };
    let error = effective_stdio_env(&config).expect_err("missing env var should error");
    match &error {
        McpError::AuthRequired { server, reason } => {
            assert_eq!(server, "needs-auth");
            assert!(reason.contains(env_var), "{reason}");
        }
        other => panic!("expected AuthRequired, got {other}"),
    }
    // The user-facing message must name the server, the underlying reason, and
    // an actionable fix command so it never collapses into opaque text.
    let rendered = error.to_string();
    assert!(rendered.contains("needs-auth"), "{rendered}");
    assert!(rendered.contains(env_var), "{rendered}");
    assert!(
        rendered.contains("orbcode mcp auth login needs-auth"),
        "auth error should guide the user to sign in: {rendered}"
    );
    assert!(
        rendered.contains("orbcode mcp diagnose needs-auth"),
        "auth error should point at diagnose for details: {rendered}"
    );
}

#[test]
fn canonicalize_auth_server_rewrites_endpoint_to_configured_id() {
    // HTTP/WS transports key AuthRequired on the endpoint URL; the registry must
    // rewrite it to the configured server id so fix guidance is actionable.
    let endpoint_error = McpError::AuthRequired {
        server: "https://docs.example/mcp".to_string(),
        reason: "remote server returned HTTP 401 Unauthorized".to_string(),
    };
    match canonicalize_auth_server(endpoint_error, "docs") {
        McpError::AuthRequired { server, reason } => {
            assert_eq!(server, "docs");
            assert!(reason.contains("401"), "reason preserved: {reason}");
        }
        other => panic!("expected AuthRequired, got {other}"),
    }

    // Non-auth errors and already-canonical ids pass through untouched.
    let untouched = canonicalize_auth_server(McpError::Protocol("boom".to_string()), "docs");
    assert!(matches!(untouched, McpError::Protocol(_)));
}

#[test]
fn server_untrusted_display_guides_user_to_trust() {
    let error = McpError::ServerUntrusted {
        server: "docs".to_string(),
        status: "unknown",
    };
    let rendered = error.to_string();
    assert!(rendered.contains("docs"), "{rendered}");
    assert!(rendered.contains("unknown"), "{rendered}");
    assert!(
        rendered.contains("orbcode mcp trust docs"),
        "untrusted error should guide the user to trust: {rendered}"
    );
}

#[test]
fn bearer_env_present_injects_canonical_token_var() {
    let env_var = "ORBCODE_MCP_TEST_TOKEN_PRESENT";
    // SAFETY: tests are single-threaded per process and we restore the var below.
    unsafe { std::env::set_var(env_var, "secret-token") };
    let config = McpServerConfig {
        id: "needs-auth".to_string(),
        transport: McpTransport::Stdio,
        endpoint: "fake-bin".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        headers: BTreeMap::new(),
        enabled: true,
        status: McpServerStatus::Ready,
        error: None,
        summary: "needs auth".to_string(),
        auth: McpAuth::BearerEnv {
            env_var: env_var.to_string(),
        },
        trust: McpServerTrust::Trusted,
        transport_type_hint: None,
        source: None,
    };
    let env = effective_stdio_env(&config).expect("build env");
    assert_eq!(env.get(env_var).map(String::as_str), Some("secret-token"));
    assert_eq!(
        env.get("MCP_BEARER_TOKEN").map(String::as_str),
        Some("secret-token")
    );
    // SAFETY: cleanup for other tests.
    unsafe { std::env::remove_var(env_var) };
}

#[test]
fn header_auth_injects_normalized_env_for_stdio() {
    let config = McpServerConfig {
        id: "h".to_string(),
        transport: McpTransport::Stdio,
        endpoint: "fake-bin".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        headers: BTreeMap::new(),
        enabled: true,
        status: McpServerStatus::Ready,
        error: None,
        summary: "header auth".to_string(),
        auth: McpAuth::Header {
            name: "X-Api-Key".to_string(),
            value: "k".to_string(),
        },
        trust: McpServerTrust::Trusted,
        transport_type_hint: None,
        source: None,
    };
    let env = effective_stdio_env(&config).expect("build env");
    assert_eq!(
        env.get("MCP_HEADER_X_API_KEY").map(String::as_str),
        Some("k")
    );
}

#[tokio::test]
async fn concurrent_oauth_refresh_only_fires_once() {
    let refresh_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let refresh_count_inner = refresh_count.clone();
    let endpoint = spawn_fake_http_mcp_server(7, move |_index, request| {
        if request.starts_with("POST /token ") {
            refresh_count_inner.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return FakeHttpResponse::ok(
                json!({
                    "access_token": "refreshed-token",
                    "refresh_token": "new-refresh",
                    "expires_in": 3600,
                    "scope": "tools.read"
                })
                .to_string(),
            );
        }
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                    }
                })
                .to_string(),
            );
        }
        FakeHttpResponse::ok(json_rpc_response(
            &request,
            json!({
                    "tools": [{
                        "name": "echo",
                        "description": "Echo.",
                        "inputSchema": {"type": "object"}
                    }]
            }),
        ))
    });
    let token_endpoint = format!("{}/token", endpoint.trim_end_matches("/mcp"));
    let (home, cwd) = temp_paths("concurrent-refresh");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert");
    registry
        .store_mcp_oauth_token(
            "remote",
            McpOAuthTokenInput {
                access_token: "expired-token".to_string(),
                refresh_token: Some("stale-refresh".to_string()),
                token_endpoint: Some(token_endpoint),
                client_id: Some("orbcode-test".to_string()),
                expires_at: Some(unix_timestamp_now() - 1),
                scopes: vec!["tools.read".to_string()],
            },
        )
        .await
        .expect("store expired token");

    let r1 = registry.clone();
    let r2 = registry.clone();
    let r3 = registry.clone();
    let (a, b, c) = tokio::join!(
        r1.list_tools("remote"),
        r2.list_tools("remote"),
        r3.list_tools("remote"),
    );
    a.expect("list_tools a");
    b.expect("list_tools b");
    c.expect("list_tools c");
    assert_eq!(
        refresh_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "concurrent callers should trigger exactly one refresh"
    );
}

#[tokio::test]
async fn oauth_refresh_scope_downgrade_warns_but_succeeds() {
    let endpoint = spawn_fake_http_mcp_server(3, |_index, request| {
        if request.starts_with("POST /token ") {
            return FakeHttpResponse::ok(
                json!({
                    "access_token": "refreshed-narrow",
                    "refresh_token": "new-refresh",
                    "expires_in": 3600,
                    "scope": "tools.read"
                })
                .to_string(),
            );
        }
        if request.contains(r#""method":"initialize""#) {
            return FakeHttpResponse::ok(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                    }
                })
                .to_string(),
            );
        }
        FakeHttpResponse::ok(json_rpc_response(
            &request,
            json!({
                    "tools": [{
                        "name": "echo",
                        "description": "Echo after scope downgrade.",
                        "inputSchema": {"type": "object"}
                    }]
            }),
        ))
    });
    let token_endpoint = format!("{}/token", endpoint.trim_end_matches("/mcp"));
    let (home, cwd) = temp_paths("scope-downgrade");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert");
    registry
        .store_mcp_oauth_token(
            "remote",
            McpOAuthTokenInput {
                access_token: "expired-token".to_string(),
                refresh_token: Some("stale-refresh".to_string()),
                token_endpoint: Some(token_endpoint),
                client_id: Some("orbcode-test".to_string()),
                expires_at: Some(unix_timestamp_now() - 1),
                scopes: vec!["tools.read".to_string(), "tools.call".to_string()],
            },
        )
        .await
        .expect("store expired token with wide scopes");

    let tools = registry
        .list_tools("remote")
        .await
        .expect("list_tools succeeds despite scope downgrade");
    assert_eq!(tools[0].summary, "Echo after scope downgrade.");

    let overview = registry
        .mcp_oauth_overview(Some("remote"))
        .await
        .expect("overview");
    let entry = overview.entries.first().expect("refreshed entry");
    assert!(entry.usable, "token should still be usable");
    assert_eq!(
        entry.scopes,
        vec!["tools.read"],
        "scopes should reflect the narrower set from the server"
    );
}

#[tokio::test]
async fn http_transport_retries_once_on_connection_drop() {
    use std::io::Write;
    use std::net::TcpListener as StdTcpListener;
    use std::thread;

    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let endpoint = format!("http://{addr}/mcp");
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept first");
        drop(stream);

        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept retry");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("timeout");
            let request = read_http_request(&mut stream);
            let body = if request.contains(r#""method":"initialize""#) {
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                    }
                })
                .to_string()
            } else {
                json!({
                    "jsonrpc": "2.0",
                    "id": http_json_rpc_id(&request),
                    "result": {
                        "tools": [{
                            "name": "recovered",
                            "description": "Recovered after drop.",
                            "inputSchema": {"type": "object"}
                        }]
                    }
                })
                .to_string()
            };
            let payload = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(payload.as_bytes())
                .expect("write response");
        }
    });
    let (home, cwd) = temp_paths("http-reconnect");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(McpServerConfig {
            id: "remote".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Remote".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert");

    let tools = registry.list_tools("remote").await.expect("list_tools");
    assert_eq!(tools[0].name, "recovered");
}

#[tokio::test]
async fn streamable_http_reuses_session_and_protocol_header() {
    let endpoint = spawn_fake_http_mcp_server(3, |index, request| {
        if index == 0 {
            assert!(request.contains(r#""method":"initialize""#), "{request}");
            assert!(
                !request.to_ascii_lowercase().contains("mcp-session-id:"),
                "initialize must not send a stale session id: {request}"
            );
            return FakeHttpResponse::ok(
                json!({
                    "jsonrpc": "2.0",
                    "id": http_json_rpc_id(&request),
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                    }
                })
                .to_string(),
            )
            .with_header("Mcp-Session-Id", "session-1");
        }
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("mcp-session-id: session-1"), "{request}");
        assert!(
            lower.contains("mcp-protocol-version: 2024-11-05"),
            "{request}"
        );
        FakeHttpResponse::ok(json_rpc_response(
            &request,
            json!({
                "tools": [{
                    "name": format!("echo-{index}"),
                    "description": "Echo over Streamable HTTP.",
                    "inputSchema": {"type": "object"}
                }]
            }),
        ))
    });
    let (home, cwd) = temp_paths("streamable-http-session");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(test_mcp_server_config_with_transport(
            "remote",
            McpTransport::StreamableHttp,
            &endpoint,
        ))
        .await
        .expect("upsert");

    let first = registry.list_tools("remote").await.expect("first list");
    let second = registry.list_tools("remote").await.expect("second list");
    assert_eq!(first[0].name, "echo-1");
    assert_eq!(second[0].name, "echo-2");
}

#[tokio::test]
async fn streamable_http_shutdown_sends_delete_on_remove() {
    let saw_delete = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let saw_delete_inner = saw_delete.clone();
    let endpoint = spawn_fake_http_mcp_server(3, move |index, request| {
        if index == 0 {
            return FakeHttpResponse::ok(
                json!({
                    "jsonrpc": "2.0",
                    "id": http_json_rpc_id(&request),
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                    }
                })
                .to_string(),
            )
            .with_header("Mcp-Session-Id", "session-delete");
        }
        if request.starts_with("DELETE /mcp ") {
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("mcp-session-id: session-delete"),
                "{request}"
            );
            saw_delete_inner.store(true, std::sync::atomic::Ordering::SeqCst);
            return FakeHttpResponse::status("204 No Content", "");
        }
        FakeHttpResponse::ok(json_rpc_response(
            &request,
            json!({
                "tools": [{
                    "name": "echo",
                    "description": "Echo.",
                    "inputSchema": {"type": "object"}
                }]
            }),
        ))
    });
    let (home, cwd) = temp_paths("streamable-http-delete");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(test_mcp_server_config_with_transport(
            "remote",
            McpTransport::StreamableHttp,
            &endpoint,
        ))
        .await
        .expect("upsert");

    registry.list_tools("remote").await.expect("list tools");
    assert!(registry.remove_server("remote").await.expect("remove"));
    assert!(
        saw_delete.load(std::sync::atomic::Ordering::SeqCst),
        "registry remove must send Streamable HTTP DELETE when a session exists"
    );
}

#[tokio::test]
async fn streamable_http_shutdown_sends_delete_on_reload_remove() {
    let saw_delete = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let saw_delete_inner = saw_delete.clone();
    let endpoint = spawn_fake_http_mcp_server(3, move |index, request| {
        if index == 0 {
            return FakeHttpResponse::ok(
                json!({
                    "jsonrpc": "2.0",
                    "id": http_json_rpc_id(&request),
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                    }
                })
                .to_string(),
            )
            .with_header("Mcp-Session-Id", "session-reload-delete");
        }
        if request.starts_with("DELETE /mcp ") {
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("mcp-session-id: session-reload-delete"),
                "{request}"
            );
            saw_delete_inner.store(true, std::sync::atomic::Ordering::SeqCst);
            return FakeHttpResponse::status("204 No Content", "");
        }
        FakeHttpResponse::ok(json_rpc_response(
            &request,
            json!({
                "tools": [{
                    "name": "echo",
                    "description": "Echo.",
                    "inputSchema": {"type": "object"}
                }]
            }),
        ))
    });
    let (home, cwd) = temp_paths("streamable-http-reload-delete");
    write_mcp_json(
        &cwd,
        json!({
            "remote": {
                "type": "streamable_http",
                "url": endpoint
            }
        }),
    );
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .set_server_trust("remote", McpServerTrust::Trusted)
        .await
        .expect("trust remote");

    registry.list_tools("remote").await.expect("list tools");
    write_mcp_json(&cwd, json!({}));
    let result = registry
        .reload_config(McpLoadOptions::default())
        .await
        .expect("reload");

    assert_eq!(result.removed, vec!["remote"]);
    assert!(
        saw_delete.load(std::sync::atomic::Ordering::SeqCst),
        "config reload removal must send Streamable HTTP DELETE when a session exists"
    );
}

#[tokio::test]
async fn streamable_http_reinitializes_once_after_session_expiry_for_list() {
    let endpoint = spawn_fake_http_mcp_server(4, |index, request| match index {
        0 | 2 => FakeHttpResponse::ok(
            json!({
                "jsonrpc": "2.0",
                "id": http_json_rpc_id(&request),
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                }
            })
            .to_string(),
        )
        .with_header("Mcp-Session-Id", format!("session-{index}")),
        1 => FakeHttpResponse::status("404 Not Found", "expired"),
        _ => FakeHttpResponse::ok(json_rpc_response(
            &request,
            json!({
                "tools": [{
                    "name": "recovered",
                    "description": "Recovered after session expiry.",
                    "inputSchema": {"type": "object"}
                }]
            }),
        )),
    });
    let (home, cwd) = temp_paths("streamable-http-expired-session");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(test_mcp_server_config_with_transport(
            "remote",
            McpTransport::StreamableHttp,
            &endpoint,
        ))
        .await
        .expect("upsert");

    let tools = registry.list_tools("remote").await.expect("list tools");
    assert_eq!(tools[0].name, "recovered");
}

#[tokio::test]
async fn streamable_http_unsupported_content_type_sets_failed() {
    let endpoint = spawn_fake_http_mcp_server(2, |index, request| {
        if index == 0 {
            return FakeHttpResponse::ok(
                json!({
                    "jsonrpc": "2.0",
                    "id": http_json_rpc_id(&request),
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "fake-http", "version": "0.1.0"}
                    }
                })
                .to_string(),
            );
        }
        FakeHttpResponse::ok(json_rpc_response(&request, json!({"tools": []})))
            .with_content_type("text/plain")
    });
    let (home, cwd) = temp_paths("streamable-http-content-type");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(test_mcp_server_config_with_transport(
            "remote",
            McpTransport::StreamableHttp,
            &endpoint,
        ))
        .await
        .expect("upsert");

    let error = registry
        .list_tools("remote")
        .await
        .expect_err("text/plain response must fail");
    assert!(
        error.to_string().contains("unsupported content type"),
        "{error}"
    );
    let remote = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "remote")
        .expect("remote server");
    assert_eq!(remote.status, McpServerStatus::Failed);
}

fn read_blocking_websocket_frame(stream: &mut std::net::TcpStream) -> (u8, Vec<u8>) {
    use std::io::Read;

    let mut header = [0_u8; 2];
    stream.read_exact(&mut header).expect("read ws header");
    let opcode = header[0] & 0x0f;
    let masked = header[1] & 0x80 != 0;
    let mut len = (header[1] & 0x7f) as u64;
    if len == 126 {
        let mut extended = [0_u8; 2];
        stream.read_exact(&mut extended).expect("read ws len16");
        len = u16::from_be_bytes(extended) as u64;
    } else if len == 127 {
        let mut extended = [0_u8; 8];
        stream.read_exact(&mut extended).expect("read ws len64");
        len = u64::from_be_bytes(extended);
    }
    let mask = if masked {
        let mut mask = [0_u8; 4];
        stream.read_exact(&mut mask).expect("read ws mask");
        Some(mask)
    } else {
        None
    };
    let mut payload = vec![0_u8; len as usize];
    stream.read_exact(&mut payload).expect("read ws payload");
    if let Some(mask) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    (opcode, payload)
}

fn websocket_server_pong_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.push(0x8A);
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

#[tokio::test]
async fn websocket_ping_timeout_closes_connection() {
    use crate::transport::websocket::WebSocketMcpClient;

    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake ws server for ping timeout");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("local addr"));
    let endpoint_clone = endpoint.clone();

    std::thread::spawn(move || {
        use std::io::Write;

        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set read timeout");
        let _ = read_http_request(&mut stream);
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {WEBSOCKET_ACCEPT}\r\n\r\n"
        );
        stream
            .write_all(handshake.as_bytes())
            .expect("write handshake");

        let message = read_blocking_websocket_text(&mut stream);
        let response = fake_websocket_mcp_response(&message);
        let frame = websocket_server_text_frame(response.as_bytes());
        stream.write_all(&frame).expect("write initialize response");

        let (opcode, _payload) = read_blocking_websocket_frame(&mut stream);
        assert_eq!(opcode, 0x9, "expected ping frame");
        // Deliberately do NOT send a pong — simulate a hung server.
        std::thread::sleep(Duration::from_secs(5));
    });

    let mut client =
        WebSocketMcpClient::connect(&endpoint_clone, HeaderMap::new(), Duration::from_secs(10))
            .await
            .expect("connect");
    client.initialize().await.expect("initialize");
    client.set_ping_timeout(Duration::from_secs(1));
    client.send_ping().await.expect("send ping");

    let result = tokio::time::timeout(Duration::from_secs(5), client.check_ping_timeout()).await;
    match result {
        Ok(Err(McpError::Timeout(msg))) => {
            assert!(
                msg.contains("ping timeout"),
                "expected ping timeout message, got: {msg}"
            );
        }
        Ok(Err(other)) => {
            assert!(
                other.to_string().contains("ping timeout")
                    || other.to_string().contains("timed out"),
                "expected ping timeout error, got: {other}"
            );
        }
        Ok(Ok(())) => panic!("expected ping timeout error, but check_ping_timeout succeeded"),
        Err(_) => panic!("test itself timed out waiting for check_ping_timeout"),
    }
}

#[tokio::test]
async fn websocket_ping_pong_succeeds_when_server_responds() {
    use crate::transport::websocket::WebSocketMcpClient;

    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake ws server for ping-pong");
    let endpoint = format!("ws://{}/mcp", listener.local_addr().expect("local addr"));
    let endpoint_clone = endpoint.clone();

    std::thread::spawn(move || {
        use std::io::Write;

        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set read timeout");
        let _ = read_http_request(&mut stream);
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {WEBSOCKET_ACCEPT}\r\n\r\n"
        );
        stream
            .write_all(handshake.as_bytes())
            .expect("write handshake");

        let message = read_blocking_websocket_text(&mut stream);
        let response = fake_websocket_mcp_response(&message);
        let frame = websocket_server_text_frame(response.as_bytes());
        stream.write_all(&frame).expect("write initialize response");

        let (opcode, payload) = read_blocking_websocket_frame(&mut stream);
        assert_eq!(opcode, 0x9, "expected ping frame");
        let pong = websocket_server_pong_frame(&payload);
        stream.write_all(&pong).expect("write pong");
    });

    let mut client =
        WebSocketMcpClient::connect(&endpoint_clone, HeaderMap::new(), Duration::from_secs(10))
            .await
            .expect("connect");
    client.initialize().await.expect("initialize");
    client.send_ping().await.expect("send ping");
    client
        .check_ping_timeout()
        .await
        .expect("check_ping_timeout should succeed when pong is received");
}

// ─── transportType hint tests ───

#[tokio::test]
async fn transport_type_sse_forces_http_even_for_ws_url() {
    let (home, cwd) = temp_paths("transport-type-sse");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{
            "mcpServers": {
                "remote": {
                    "type": "http",
                    "transportType": "sse",
                    "url": "https://example.com/mcp"
                }
            }
        }"#,
    )
    .expect("write config");

    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    let servers = registry.list_servers().await;
    let remote = servers
        .iter()
        .find(|s| s.id == "remote")
        .expect("find remote");
    assert_eq!(remote.transport, McpTransport::StreamableHttp);
    assert_eq!(
        remote.transport_type_hint.as_deref(),
        Some("sse"),
        "transportType hint should be preserved"
    );
}

#[tokio::test]
async fn transport_type_sse_overrides_ws_url_to_http() {
    let (home, cwd) = temp_paths("transport-type-sse-ws");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{
            "mcpServers": {
                "force_sse": {
                    "type": "http",
                    "transportType": "sse",
                    "url": "wss://example.com/mcp"
                }
            }
        }"#,
    )
    .expect("write config");

    let error = match McpRegistry::load(&home, &cwd).await {
        Ok(_) => panic!("ws:// URL with transportType sse should fail"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(
        message.contains("remote MCP server URL must start with http://"),
        "expected typed diagnostic about invalid URL scheme, got: {message}"
    );
}

#[tokio::test]
async fn transport_type_websocket_overrides_http_url() {
    let (home, cwd) = temp_paths("transport-type-ws-override");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{
            "mcpServers": {
                "force_ws": {
                    "type": "http",
                    "transportType": "websocket",
                    "url": "https://example.com/mcp"
                }
            }
        }"#,
    )
    .expect("write config");

    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|s| s.id == "force_ws")
        .expect("find server");
    assert_eq!(
        server.transport,
        McpTransport::WebSocket,
        "transportType websocket should override HTTPS URL"
    );
}

#[tokio::test]
async fn transport_type_absent_falls_back_to_url_inference() {
    let (home, cwd) = temp_paths("transport-type-absent");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{
            "mcpServers": {
                "inferred": {
                    "type": "http",
                    "url": "wss://example.com/mcp"
                }
            }
        }"#,
    )
    .expect("write config");

    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|s| s.id == "inferred")
        .expect("find server");
    assert_eq!(
        server.transport,
        McpTransport::WebSocket,
        "without transportType, wss:// URL should infer WebSocket"
    );
    assert!(server.transport_type_hint.is_none());
}

#[tokio::test]
async fn config_streamable_http_explicit_type() {
    let (home, cwd) = temp_paths("config-streamable-http");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{
            "mcpServers": {
                "remote": {
                    "type": "streamable_http",
                    "url": "https://example.com/mcp"
                }
            }
        }"#,
    )
    .expect("write config");

    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    let servers = registry.list_servers().await;
    let server = servers.iter().find(|s| s.id == "remote").expect("find");
    assert_eq!(server.transport, McpTransport::StreamableHttp);
}

#[tokio::test]
async fn config_http_alias_normalizes_to_streamable_http() {
    let (home, cwd) = temp_paths("config-http-alias");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{
            "mcpServers": {
                "legacy": {
                    "type": "http",
                    "url": "https://example.com/mcp"
                }
            }
        }"#,
    )
    .expect("write config");

    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    let servers = registry.list_servers().await;
    let server = servers.iter().find(|s| s.id == "legacy").expect("find");
    assert_eq!(
        server.transport,
        McpTransport::StreamableHttp,
        "type http with https:// URL should normalize to StreamableHttp"
    );
}

#[tokio::test]
async fn config_streamable_http_rejects_ws_url() {
    let (home, cwd) = temp_paths("config-streamable-ws");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{
            "mcpServers": {
                "bad": {
                    "type": "streamable_http",
                    "url": "wss://example.com/mcp"
                }
            }
        }"#,
    )
    .expect("write config");

    let error = match McpRegistry::load(&home, &cwd).await {
        Ok(_) => panic!("streamable_http with ws URL should fail"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(
        message.contains("remote MCP server URL must start with http://"),
        "expected typed diagnostic, got: {message}"
    );
}

#[tokio::test]
async fn config_transport_type_hint_streamable_http() {
    let (home, cwd) = temp_paths("config-hint-streamable");
    std::fs::write(
        cwd.join(".mcp.json"),
        r#"{
            "mcpServers": {
                "hinted": {
                    "type": "http",
                    "transportType": "streamable_http",
                    "url": "https://example.com/mcp"
                }
            }
        }"#,
    )
    .expect("write config");

    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    let servers = registry.list_servers().await;
    let server = servers.iter().find(|s| s.id == "hinted").expect("find");
    assert_eq!(server.transport, McpTransport::StreamableHttp);
    assert_eq!(
        server.transport_type_hint.as_deref(),
        Some("streamable_http")
    );
}
// ─── post-init 401 → Unauthorized status tests ───

#[tokio::test]
async fn finish_probe_sets_unauthorized_for_auth_required() {
    let (home, cwd) = temp_paths("finish-probe-unauth");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(McpServerConfig {
            id: "test-server".to_string(),
            transport: McpTransport::Http,
            endpoint: "https://example.com/mcp".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "test".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert");

    let result: Result<(), McpError> = Err(McpError::AuthRequired {
        server: "test-server".to_string(),
        reason: "HTTP 401 Unauthorized".to_string(),
    });
    let _ = registry.finish_probe("test-server", result).await;

    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|s| s.id == "test-server")
        .expect("find server");
    assert_eq!(
        server.status,
        McpServerStatus::Unauthorized,
        "AuthRequired error should set Unauthorized status, not Failed"
    );
    assert!(
        server.error.as_ref().unwrap().contains("401"),
        "error message should be preserved"
    );
}

#[tokio::test]
async fn finish_probe_sets_failed_for_non_auth_errors() {
    let (home, cwd) = temp_paths("finish-probe-failed");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(McpServerConfig {
            id: "fail-server".to_string(),
            transport: McpTransport::Http,
            endpoint: "https://example.com/mcp".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "test".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert");

    let result: Result<(), McpError> = Err(McpError::Protocol("connection reset".to_string()));
    let _ = registry.finish_probe("fail-server", result).await;

    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|s| s.id == "fail-server")
        .expect("find server");
    assert_eq!(
        server.status,
        McpServerStatus::Failed,
        "non-auth errors should still set Failed status"
    );
}

#[tokio::test]
async fn http_401_during_probe_sets_unauthorized_status() {
    let endpoint = spawn_fake_http_mcp_server(1, |_, _| {
        FakeHttpResponse::status("401 Unauthorized", "not authenticated")
    });

    let (home, cwd) = temp_paths("http-401-probe");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    registry
        .upsert_server(McpServerConfig {
            id: "auth-server".to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "test".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("upsert");

    let result = registry.list_tools("auth-server").await;
    assert!(result.is_err(), "should fail for 401");

    let servers = registry.list_servers().await;
    let server = servers
        .iter()
        .find(|s| s.id == "auth-server")
        .expect("find server");
    assert_eq!(
        server.status,
        McpServerStatus::Unauthorized,
        "HTTP 401 should result in Unauthorized status"
    );
}

// ─── exponential backoff tests ───

#[test]
fn restart_backoff_delays_increase_exponentially() {
    use crate::registry::RestartBackoff;

    let mut backoff = RestartBackoff::new();
    assert!(backoff.is_allowed(), "initial state should allow restart");

    let mut delays = Vec::new();
    for _ in 0..6 {
        let delay = backoff.next_delay();
        delays.push(delay.as_secs_f64());
        backoff.record_attempt();
    }

    // With jitter ±20%, base 1s: ~0.8-1.2, ~1.6-2.4, ~3.2-4.8, ~6.4-9.6, ~12.8-19.2, ~24-30
    assert!(
        delays[0] >= 0.7 && delays[0] <= 1.3,
        "first delay ~1s: {}",
        delays[0]
    );
    assert!(
        delays[1] >= 1.5 && delays[1] <= 2.5,
        "second delay ~2s: {}",
        delays[1]
    );
    assert!(
        delays[2] >= 3.0 && delays[2] <= 5.0,
        "third delay ~4s: {}",
        delays[2]
    );
    // Last delays should be capped at ~30s
    assert!(
        delays[5] <= 36.5,
        "delays should be capped near 30s: {}",
        delays[5]
    );

    for i in 1..delays.len() {
        assert!(
            delays[i] >= delays[i - 1] * 0.5,
            "delays should generally increase: {} < {} * 0.5",
            delays[i],
            delays[i - 1]
        );
    }
}

#[test]
fn restart_backoff_reset_clears_state() {
    use crate::registry::RestartBackoff;

    let mut backoff = RestartBackoff::new();
    for _ in 0..5 {
        backoff.record_attempt();
    }
    assert!(
        !backoff.is_allowed(),
        "after several attempts, should not be immediately allowed"
    );

    backoff.reset();
    assert!(
        backoff.is_allowed(),
        "after reset, should be immediately allowed"
    );
    let delay = backoff.next_delay();
    assert!(
        delay.as_secs_f64() < 1.5,
        "after reset, delay should be back to base (~1s): {}",
        delay.as_secs_f64()
    );
}

#[tokio::test]
async fn should_restart_respects_backoff_state() {
    let (home, cwd) = temp_paths("backoff-should-restart");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");

    let io_error = McpError::Io(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "broken",
    ));
    assert!(
        registry.should_restart("test", &io_error).await,
        "first restart should be allowed"
    );

    {
        let mut backoffs = registry.restart_backoffs.lock().await;
        let backoff = backoffs
            .entry("test".to_string())
            .or_insert_with(crate::registry::RestartBackoff::new);
        backoff.record_attempt();
    }

    let allowed = registry.should_restart("test", &io_error).await;
    assert!(!allowed, "should not allow restart while in backoff period");

    let auth_error = McpError::AuthRequired {
        server: "test".to_string(),
        reason: "unauthorized".to_string(),
    };
    assert!(
        !registry.should_restart("test", &auth_error).await,
        "auth errors should never trigger restart"
    );
}

#[test]
fn unauthorized_status_as_str() {
    assert_eq!(McpServerStatus::Unauthorized.as_str(), "unauthorized");
}

// ---------------------------------------------------------------------------
// Trust approval flow tests
// ---------------------------------------------------------------------------

mod trust_approval_tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::Mutex;

    use crate::types::{
        ErasedTrustApprovalHandler, McpServerSource, TrustApprovalHandler, TrustApprovalRequest,
        TrustApprovalResponse,
    };
    use crate::{
        McpAuth, McpRegistry, McpServerConfig, McpServerStatus, McpServerTrust, McpTransport,
    };

    use super::temp_paths;

    struct FixedApprovalHandler {
        response: TrustApprovalResponse,
        calls: AtomicUsize,
        last_request: Mutex<Option<TrustApprovalRequest>>,
    }

    impl FixedApprovalHandler {
        fn new(response: TrustApprovalResponse) -> Self {
            Self {
                response,
                calls: AtomicUsize::new(0),
                last_request: Mutex::new(None),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl TrustApprovalHandler for FixedApprovalHandler {
        async fn request_trust_approval(
            &self,
            request: TrustApprovalRequest,
        ) -> Option<TrustApprovalResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_request.lock().await = Some(request);
            Some(self.response)
        }
    }

    fn stored_server_config(id: &str) -> McpServerConfig {
        McpServerConfig {
            id: id.to_string(),
            transport: McpTransport::WebSocket,
            endpoint: "modeled://fake".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Test server".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        }
    }

    async fn setup_registry_with_trust(
        label: &str,
        server_id: &str,
        trust: McpServerTrust,
    ) -> McpRegistry {
        let (home, cwd) = temp_paths(label);
        let registry = McpRegistry::load(&home, &cwd).await.expect("load");
        registry
            .upsert_server(stored_server_config(server_id))
            .await
            .expect("upsert");
        if trust != McpServerTrust::Trusted {
            registry
                .set_server_trust(server_id, trust)
                .await
                .expect("set trust");
        }
        registry
    }

    #[tokio::test]
    async fn invoke_tool_unknown_trust_emits_approval_request() {
        let registry =
            setup_registry_with_trust("trust-approval-emit", "srv", McpServerTrust::Unknown).await;

        let handler = Arc::new(FixedApprovalHandler::new(TrustApprovalResponse::Trusted));
        registry
            .set_trust_approval_handler(handler.clone() as Arc<dyn ErasedTrustApprovalHandler>)
            .await;

        let result = registry.invoke_tool("srv", "echo", "hello").await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(handler.call_count(), 1);

        let last = handler.last_request.lock().await;
        let req = last.as_ref().expect("should have captured request");
        assert_eq!(req.server_id, "srv");
        assert_eq!(req.tool_name, "echo");
        assert!(!req.request_id.is_empty());
    }

    #[tokio::test]
    async fn invoke_tool_approval_trusted_sets_trust_and_retries() {
        let registry =
            setup_registry_with_trust("trust-approval-trusted", "srv", McpServerTrust::Unknown)
                .await;

        let handler = Arc::new(FixedApprovalHandler::new(TrustApprovalResponse::Trusted));
        registry
            .set_trust_approval_handler(handler.clone() as Arc<dyn ErasedTrustApprovalHandler>)
            .await;

        let result = registry
            .invoke_tool("srv", "echo", "payload")
            .await
            .expect("should succeed after approval");
        assert!(result.output.contains("payload"));
        assert_eq!(registry.server_trust("srv").await, McpServerTrust::Trusted);
    }

    #[tokio::test]
    async fn invoke_tool_approval_denied_sets_trust_and_returns_error() {
        let registry =
            setup_registry_with_trust("trust-approval-denied", "srv", McpServerTrust::Unknown)
                .await;

        let handler = Arc::new(FixedApprovalHandler::new(TrustApprovalResponse::Denied));
        registry
            .set_trust_approval_handler(handler.clone() as Arc<dyn ErasedTrustApprovalHandler>)
            .await;

        let err = registry
            .invoke_tool("srv", "echo", "payload")
            .await
            .expect_err("should fail after denial");
        assert!(err.to_string().contains("not trusted"));
        assert_eq!(registry.server_trust("srv").await, McpServerTrust::Denied);
    }

    #[tokio::test]
    async fn invoke_tool_trusted_server_skips_approval() {
        let registry =
            setup_registry_with_trust("trust-approval-skip", "srv", McpServerTrust::Trusted).await;

        let handler = Arc::new(FixedApprovalHandler::new(TrustApprovalResponse::Denied));
        registry
            .set_trust_approval_handler(handler.clone() as Arc<dyn ErasedTrustApprovalHandler>)
            .await;

        let result = registry
            .invoke_tool("srv", "echo", "direct")
            .await
            .expect("trusted server should work directly");
        assert!(result.output.contains("direct"));
        assert_eq!(handler.call_count(), 0);
    }

    #[tokio::test]
    async fn invoke_tool_no_handler_returns_error_for_unknown() {
        let registry =
            setup_registry_with_trust("trust-approval-no-handler", "srv", McpServerTrust::Unknown)
                .await;

        let err = registry
            .invoke_tool("srv", "echo", "payload")
            .await
            .expect_err("should fail without handler");
        assert!(err.to_string().contains("not trusted"));
    }

    #[tokio::test]
    async fn invoke_tool_denied_server_does_not_trigger_approval() {
        let registry =
            setup_registry_with_trust("trust-approval-denied-skip", "srv", McpServerTrust::Denied)
                .await;

        let handler = Arc::new(FixedApprovalHandler::new(TrustApprovalResponse::Trusted));
        registry
            .set_trust_approval_handler(handler.clone() as Arc<dyn ErasedTrustApprovalHandler>)
            .await;

        let err = registry
            .invoke_tool("srv", "echo", "payload")
            .await
            .expect_err("denied server should fail immediately");
        assert!(err.to_string().contains("not trusted"));
        assert_eq!(handler.call_count(), 0);
    }

    #[tokio::test]
    async fn approval_request_includes_server_source() {
        let (home, cwd) = temp_paths("trust-approval-source");
        let registry = McpRegistry::load(&home, &cwd).await.expect("load");

        let mut config = stored_server_config("srv");
        config.source = Some(McpServerSource::Plugin(crate::types::McpPluginSource {
            plugin_id: "plugin-1".to_string(),
            plugin_name: "Test Plugin".to_string(),
            server_name: "test-server".to_string(),
            source: "test-source".to_string(),
        }));
        registry.upsert_server(config).await.expect("upsert");
        registry
            .set_server_trust("srv", McpServerTrust::Unknown)
            .await
            .expect("set trust");

        let handler = Arc::new(FixedApprovalHandler::new(TrustApprovalResponse::Trusted));
        registry
            .set_trust_approval_handler(handler.clone() as Arc<dyn ErasedTrustApprovalHandler>)
            .await;

        let _ = registry.invoke_tool("srv", "echo", "x").await;
        let last = handler.last_request.lock().await;
        let req = last.as_ref().unwrap();
        match req.server_source.as_ref().unwrap() {
            McpServerSource::Plugin(source) => {
                assert_eq!(source.plugin_id, "plugin-1");
            }
        }
    }
}

fn write_mcp_json(cwd: &std::path::Path, servers: serde_json::Value) {
    let content = json!({ "mcpServers": servers });
    std::fs::write(cwd.join(".mcp.json"), content.to_string()).expect("write .mcp.json");
}

fn test_mcp_server_config(id: &str, endpoint: &str) -> McpServerConfig {
    test_mcp_server_config_with_transport(id, McpTransport::WebSocket, endpoint)
}

fn test_mcp_server_config_with_transport(
    id: &str,
    transport: McpTransport,
    endpoint: &str,
) -> McpServerConfig {
    McpServerConfig {
        id: id.to_string(),
        transport,
        endpoint: endpoint.to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        headers: BTreeMap::new(),
        enabled: true,
        status: McpServerStatus::Ready,
        error: None,
        summary: "Test server".to_string(),
        auth: McpAuth::None,
        trust: McpServerTrust::Trusted,
        transport_type_hint: None,
        source: None,
    }
}

fn persisted_server_ids(home: &std::path::Path) -> Vec<String> {
    let contents =
        std::fs::read_to_string(home.join("mcp").join("servers.json")).expect("servers.json");
    let value: serde_json::Value = serde_json::from_str(&contents).expect("servers json");
    value["servers"]
        .as_array()
        .expect("servers array")
        .iter()
        .map(|server| {
            server["config"]["id"]
                .as_str()
                .expect("server id")
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn session_mcp_servers_are_not_persisted_during_global_mutations() {
    let (home, cwd) = temp_paths("session-overlay-global-mutation");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    let accepted = registry
        .upsert_session_servers(
            "session-a",
            vec![test_mcp_server_config(
                "session-docs",
                "modeled://session-secret",
            )],
        )
        .await;
    assert_eq!(accepted.len(), 1);

    registry
        .upsert_server(test_mcp_server_config("global-docs", "modeled://global"))
        .await
        .expect("global upsert");

    let contents =
        std::fs::read_to_string(home.join("mcp").join("servers.json")).expect("servers.json");
    assert_eq!(persisted_server_ids(&home), vec!["global-docs"]);
    assert!(
        !contents.contains("session-docs") && !contents.contains("session-secret"),
        "session-scoped MCP config must not be persisted by global add"
    );

    assert!(registry.remove_server("global-docs").await.expect("remove"));
    let contents =
        std::fs::read_to_string(home.join("mcp").join("servers.json")).expect("servers.json");
    assert!(persisted_server_ids(&home).is_empty());
    assert!(
        !contents.contains("session-docs") && !contents.contains("session-secret"),
        "session-scoped MCP config must not be persisted by global remove"
    );
}

#[tokio::test]
async fn global_mcp_management_cannot_modify_session_owned_servers() {
    let (home, cwd) = temp_paths("session-overlay-global-isolation");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    let accepted = registry
        .upsert_session_servers(
            "session-a",
            vec![test_mcp_server_config(
                "session-docs",
                "modeled://session-original",
            )],
        )
        .await;
    let session_server_id = accepted[0].id.clone();

    let err = registry
        .upsert_server(test_mcp_server_config(
            &session_server_id,
            "modeled://session-hijack",
        ))
        .await
        .expect_err("global upsert must not modify a session-owned server");
    assert!(matches!(err, McpError::UnknownServer(_)));
    assert!(
        !registry
            .remove_server(&session_server_id)
            .await
            .expect("global remove"),
        "global remove should treat session-owned ids as unknown"
    );

    assert!(
        registry
            .list_servers()
            .await
            .iter()
            .all(|server| server.id != session_server_id)
    );
    let session_servers = registry.list_servers_for_session("session-a").await;
    let session_server = session_servers
        .iter()
        .find(|server| server.id == session_server_id)
        .expect("session-owned server remains visible to owner");
    assert_eq!(session_server.endpoint, "modeled://session-original");
}

#[tokio::test]
async fn reload_config_does_not_override_session_owned_server_with_same_id() {
    let (home, cwd) = temp_paths("session-overlay-reload-collision");
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    let accepted = registry
        .upsert_session_servers(
            "session-a",
            vec![test_mcp_server_config(
                "session-docs",
                "modeled://session-original",
            )],
        )
        .await;
    assert_eq!(accepted.len(), 1);

    write_mcp_json(
        &cwd,
        json!({
            "session-docs": {
                "type": "stdio",
                "command": "echo",
                "args": ["global-hijack"],
                "env": { "SESSION_SECRET": "must-not-persist" }
            },
            "global-docs": { "type": "stdio", "command": "echo", "args": ["global"] }
        }),
    );

    let result = registry
        .reload_config(McpLoadOptions::default())
        .await
        .expect("reload");
    assert_eq!(result.added, vec!["global-docs"]);
    assert!(result.removed.is_empty());
    assert!(result.restarted.is_empty());

    let global_servers = registry.list_servers().await;
    assert!(
        global_servers
            .iter()
            .any(|server| server.id == "global-docs")
    );
    assert!(
        global_servers
            .iter()
            .all(|server| server.id != "session-docs"),
        "colliding global config must not become visible while session-owned id is active"
    );

    let session_servers = registry.list_servers_for_session("session-a").await;
    let session_server = session_servers
        .iter()
        .find(|server| server.id == "session-docs")
        .expect("session-owned server remains visible to owner");
    assert_eq!(session_server.endpoint, "modeled://session-original");
    assert!(session_server.args.is_empty());

    let contents =
        std::fs::read_to_string(home.join("mcp").join("servers.json")).expect("servers.json");
    assert_eq!(persisted_server_ids(&home), vec!["global-docs"]);
    assert!(
        !contents.contains("session-docs")
            && !contents.contains("global-hijack")
            && !contents.contains("SESSION_SECRET")
            && !contents.contains("must-not-persist"),
        "colliding session-scoped MCP config must not be persisted by reload"
    );
}

#[tokio::test]
async fn reload_config_adds_new_server() {
    let (home, cwd) = temp_paths("reload-add");
    write_mcp_json(
        &cwd,
        json!({
            "alpha": { "type": "stdio", "command": "echo", "args": ["alpha"] }
        }),
    );
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    let servers = registry.list_servers().await;
    assert!(servers.iter().any(|s| s.id == "alpha"));
    assert!(!servers.iter().any(|s| s.id == "beta"));

    write_mcp_json(
        &cwd,
        json!({
            "alpha": { "type": "stdio", "command": "echo", "args": ["alpha"] },
            "beta": { "type": "stdio", "command": "echo", "args": ["beta"] }
        }),
    );
    let result = registry
        .reload_config(McpLoadOptions::default())
        .await
        .expect("reload");
    assert_eq!(result.added, vec!["beta"]);
    assert!(result.removed.is_empty());
    assert!(result.restarted.is_empty());

    let servers = registry.list_servers().await;
    assert!(servers.iter().any(|s| s.id == "alpha"));
    assert!(servers.iter().any(|s| s.id == "beta"));
}

#[tokio::test]
async fn reload_config_removes_deleted_server() {
    let (home, cwd) = temp_paths("reload-remove");
    write_mcp_json(
        &cwd,
        json!({
            "alpha": { "type": "stdio", "command": "echo", "args": ["alpha"] },
            "beta": { "type": "stdio", "command": "echo", "args": ["beta"] }
        }),
    );
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    assert_eq!(registry.list_servers().await.len(), 2);

    write_mcp_json(
        &cwd,
        json!({
            "alpha": { "type": "stdio", "command": "echo", "args": ["alpha"] }
        }),
    );
    let result = registry
        .reload_config(McpLoadOptions::default())
        .await
        .expect("reload");
    assert!(result.added.is_empty());
    assert_eq!(result.removed, vec!["beta"]);
    assert!(result.restarted.is_empty());

    let servers = registry.list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].id, "alpha");
}

#[tokio::test]
async fn reload_config_restarts_changed_server() {
    let (home, cwd) = temp_paths("reload-restart");
    write_mcp_json(
        &cwd,
        json!({
            "alpha": { "type": "stdio", "command": "echo", "args": ["v1"] }
        }),
    );
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");
    let servers = registry.list_servers().await;
    assert_eq!(servers[0].args, vec!["v1"]);

    write_mcp_json(
        &cwd,
        json!({
            "alpha": { "type": "stdio", "command": "echo", "args": ["v2"] }
        }),
    );
    let result = registry
        .reload_config(McpLoadOptions::default())
        .await
        .expect("reload");
    assert!(result.added.is_empty());
    assert!(result.removed.is_empty());
    assert_eq!(result.restarted, vec!["alpha"]);

    let servers = registry.list_servers().await;
    assert_eq!(servers[0].args, vec!["v2"]);
}

#[tokio::test]
async fn reload_config_is_noop_when_unchanged() {
    let (home, cwd) = temp_paths("reload-noop");
    write_mcp_json(
        &cwd,
        json!({
            "alpha": { "type": "stdio", "command": "echo", "args": ["stable"] }
        }),
    );
    let registry = McpRegistry::load(&home, &cwd).await.expect("load");

    let result = registry
        .reload_config(McpLoadOptions::default())
        .await
        .expect("reload");
    assert!(result.added.is_empty());
    assert!(result.removed.is_empty());
    assert!(result.restarted.is_empty());

    let servers = registry.list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].id, "alpha");
}
