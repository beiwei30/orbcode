use std::sync::Arc;
use std::time::Duration;

use reqwest::header::HeaderMap;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::error::McpError;
use crate::wire::{
    StdioInitializeResult, StdioListToolsResult, StdioToolCallResult, parse_json_rpc_result,
};

pub(crate) const WEBSOCKET_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const WEBSOCKET_PING_TIMEOUT: Duration = Duration::from_secs(30);
const WEBSOCKET_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
pub(crate) const WEBSOCKET_ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

trait WebSocketStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> WebSocketStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

pub(crate) struct WebSocketMcpClient {
    stream: Box<dyn WebSocketStream>,
    next_id: u64,
    request_timeout: Duration,
    #[allow(dead_code)]
    ping_timeout: Duration,
    awaiting_pong: bool,
}

impl WebSocketMcpClient {
    pub(crate) async fn connect(
        endpoint: &str,
        headers: HeaderMap,
        request_timeout: Duration,
    ) -> Result<Self, McpError> {
        Self::connect_with_root_store(endpoint, headers, request_timeout, default_root_store())
            .await
    }

    pub(crate) async fn connect_with_root_store(
        endpoint: &str,
        headers: HeaderMap,
        request_timeout: Duration,
        root_store: rustls::RootCertStore,
    ) -> Result<Self, McpError> {
        let url = reqwest::Url::parse(endpoint).map_err(|error| {
            McpError::InvalidConfig(format!("invalid WebSocket endpoint `{endpoint}`: {error}"))
        })?;
        if !matches!(url.scheme(), "ws" | "wss") {
            return Err(McpError::Protocol(format!(
                "WebSocket transport requires ws:// or wss:// endpoint, got `{}`",
                url.scheme()
            )));
        }
        let host = url
            .host_str()
            .ok_or_else(|| McpError::InvalidConfig("WebSocket endpoint requires a host".into()))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| McpError::InvalidConfig("WebSocket endpoint requires a port".into()))?;
        let tcp = timeout(request_timeout, TcpStream::connect((host, port)))
            .await
            .map_err(|_| McpError::Timeout("websocket connect".to_string()))??;
        let mut stream: Box<dyn WebSocketStream> = if url.scheme() == "wss" {
            Box::new(connect_websocket_tls(host, tcp, root_store, request_timeout).await?)
        } else {
            Box::new(tcp)
        };
        let request = websocket_handshake_request(&url, &headers)?;
        stream.write_all(request.as_bytes()).await?;
        stream.flush().await?;

        let mut response = Vec::new();
        timeout(request_timeout, async {
            let mut buffer = [0_u8; 1024];
            loop {
                let bytes = stream.read(&mut buffer).await?;
                if bytes == 0 {
                    break;
                }
                response.extend_from_slice(&buffer[..bytes]);
                if response.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Ok::<(), std::io::Error>(())
        })
        .await
        .map_err(|_| McpError::Timeout("websocket handshake".to_string()))??;
        validate_websocket_handshake(endpoint, &response)?;

        Ok(Self {
            stream,
            next_id: 1,
            request_timeout,
            ping_timeout: WEBSOCKET_PING_TIMEOUT,
            awaiting_pong: false,
        })
    }

    pub(crate) async fn initialize(&mut self) -> Result<StdioInitializeResult, McpError> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "orbcode",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
        )
        .await
    }

    pub(crate) async fn list_tools(&mut self) -> Result<StdioListToolsResult, McpError> {
        self.request("tools/list", json!({})).await
    }

    pub(crate) async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<StdioToolCallResult, McpError> {
        self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
            }),
        )
        .await
    }

    pub(crate) async fn request<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<T, McpError> {
        if self.awaiting_pong {
            return Err(McpError::Protocol(
                "WebSocket ping timeout: server did not respond to ping within deadline".into(),
            ));
        }
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let payload = serde_json::to_string(&request)?;
        timeout(self.request_timeout, self.send_text(&payload))
            .await
            .map_err(|_| McpError::Timeout(format!("websocket {method} send")))??;
        let response = timeout(self.request_timeout, self.read_text())
            .await
            .map_err(|_| McpError::Timeout(format!("websocket {method}")))??;
        let response: Value = serde_json::from_str(&response)?;
        parse_json_rpc_result(response, id, method)
    }

    #[cfg(test)]
    pub(crate) fn set_ping_timeout(&mut self, timeout: Duration) {
        self.ping_timeout = timeout;
    }

    #[cfg(test)]
    pub(crate) async fn send_ping(&mut self) -> Result<(), McpError> {
        let frame = websocket_client_frame(0x9, b"ping");
        self.stream.write_all(&frame).await?;
        self.stream.flush().await?;
        self.awaiting_pong = true;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn check_ping_timeout(&mut self) -> Result<(), McpError> {
        if !self.awaiting_pong {
            return Ok(());
        }
        match timeout(self.ping_timeout, self.wait_for_pong()).await {
            Ok(Ok(())) => {
                self.awaiting_pong = false;
                Ok(())
            }
            Ok(Err(error)) => Err(error),
            Err(_) => {
                let _ = self.send_close().await;
                Err(McpError::Timeout(
                    "WebSocket ping timeout: no pong received within deadline".into(),
                ))
            }
        }
    }

    #[cfg(test)]
    async fn wait_for_pong(&mut self) -> Result<(), McpError> {
        loop {
            let frame = read_websocket_frame(&mut self.stream).await?;
            match frame.opcode {
                0xA => return Ok(()),
                0x8 => {
                    return Err(McpError::Protocol(
                        "WebSocket server closed while awaiting pong".into(),
                    ));
                }
                0x9 => self.send_pong(&frame.payload).await?,
                _ => {}
            }
        }
    }

    #[cfg(test)]
    async fn send_close(&mut self) -> Result<(), McpError> {
        let frame = websocket_client_frame(0x8, &[]);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn send_text(&mut self, payload: &str) -> Result<(), McpError> {
        let frame = websocket_client_frame(0x1, payload.as_bytes());
        self.stream.write_all(&frame).await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn send_pong(&mut self, payload: &[u8]) -> Result<(), McpError> {
        let frame = websocket_client_frame(0xA, payload);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn read_text(&mut self) -> Result<String, McpError> {
        loop {
            let frame = read_websocket_frame(&mut self.stream).await?;
            match frame.opcode {
                0x1 => {
                    return String::from_utf8(frame.payload).map_err(|error| {
                        McpError::Protocol(format!("WebSocket text frame was not UTF-8: {error}"))
                    });
                }
                0x8 => {
                    return Err(McpError::Protocol(
                        "WebSocket server closed before response".to_string(),
                    ));
                }
                0x9 => self.send_pong(&frame.payload).await?,
                0xA => {
                    self.awaiting_pong = false;
                }
                other => {
                    return Err(McpError::Protocol(format!(
                        "unsupported WebSocket frame opcode {other}"
                    )));
                }
            }
        }
    }
}

pub(crate) struct WebSocketFrame {
    pub(crate) opcode: u8,
    pub(crate) payload: Vec<u8>,
}

fn default_root_store() -> rustls::RootCertStore {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    root_store
}

async fn connect_websocket_tls(
    host: &str,
    stream: TcpStream,
    root_store: rustls::RootCertStore,
    request_timeout: Duration,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, McpError> {
    let server_name =
        rustls_pki_types::ServerName::try_from(host.to_string()).map_err(|error| {
            McpError::InvalidConfig(format!(
                "invalid WebSocket TLS server name `{host}`: {error}"
            ))
        })?;
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    timeout(
        request_timeout,
        tokio_rustls::TlsConnector::from(Arc::new(config)).connect(server_name, stream),
    )
    .await
    .map_err(|_| McpError::Timeout("websocket tls handshake".to_string()))?
    .map_err(|error| McpError::Http(format!("WebSocket TLS error: {error}")))
}

fn websocket_handshake_request(
    url: &reqwest::Url,
    headers: &HeaderMap,
) -> Result<String, McpError> {
    let host = url
        .host_str()
        .ok_or_else(|| McpError::InvalidConfig("WebSocket endpoint requires a host".into()))?;
    let host_header = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let path = websocket_request_path(url);
    let mut request = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {host_header}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {WEBSOCKET_KEY}\r\n\
         Sec-WebSocket-Version: 13\r\n"
    );
    for (name, value) in headers {
        let value = value.to_str().map_err(|error| {
            McpError::InvalidConfig(format!("invalid WebSocket header `{name}`: {error}"))
        })?;
        request.push_str(name.as_str());
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    Ok(request)
}

fn websocket_request_path(url: &reqwest::Url) -> String {
    let path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    match url.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    }
}

pub(crate) fn validate_websocket_handshake(
    endpoint: &str,
    response: &[u8],
) -> Result<(), McpError> {
    let response = String::from_utf8_lossy(response);
    let mut lines = response.lines();
    let status = lines.next().unwrap_or_default();
    if status.contains(" 401 ") || status.contains(" 403 ") {
        let authenticate = response
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("www-authenticate") {
                    Some(format!("; WWW-Authenticate: {}", value.trim()))
                } else {
                    None
                }
            })
            .unwrap_or_default();
        return Err(McpError::AuthRequired {
            server: endpoint.to_string(),
            reason: format!("remote server returned {status}{authenticate}"),
        });
    }
    if !status.contains(" 101 ") {
        return Err(McpError::Protocol(format!(
            "WebSocket handshake failed: {status}"
        )));
    }
    let mut saw_upgrade = false;
    let mut saw_accept = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("upgrade") && value.trim().eq_ignore_ascii_case("websocket") {
            saw_upgrade = true;
        }
        if name.eq_ignore_ascii_case("sec-websocket-accept") && value.trim() == WEBSOCKET_ACCEPT {
            saw_accept = true;
        }
    }
    if !saw_upgrade || !saw_accept {
        return Err(McpError::Protocol(
            "WebSocket handshake response did not include expected upgrade headers".to_string(),
        ));
    }
    Ok(())
}

fn websocket_client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        frame.push(0x80 | len as u8);
    } else if len <= u16::MAX as usize {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    let mask = websocket_mask();
    frame.extend_from_slice(&mask);
    for (index, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[index % 4]);
    }
    frame
}

fn websocket_mask() -> [u8; 4] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.subsec_nanos());
    let pid = std::process::id();
    (nanos ^ pid).to_be_bytes()
}

pub(crate) async fn read_websocket_frame<S>(stream: &mut S) -> Result<WebSocketFrame, McpError>
where
    S: AsyncRead + Unpin + ?Sized,
{
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header).await?;
    let fin = header[0] & 0x80 != 0;
    let opcode = header[0] & 0x0f;
    if !fin {
        return Err(McpError::Protocol(
            "fragmented WebSocket frames are not supported yet".to_string(),
        ));
    }
    let masked = header[1] & 0x80 != 0;
    let mut len = u64::from(header[1] & 0x7f);
    if len == 126 {
        let mut extended = [0_u8; 2];
        stream.read_exact(&mut extended).await?;
        len = u64::from(u16::from_be_bytes(extended));
    } else if len == 127 {
        let mut extended = [0_u8; 8];
        stream.read_exact(&mut extended).await?;
        len = u64::from_be_bytes(extended);
    }
    if len > 8 * 1024 * 1024 {
        return Err(McpError::Protocol(
            "WebSocket frame exceeded 8 MiB limit".to_string(),
        ));
    }
    let mask = if masked {
        let mut mask = [0_u8; 4];
        stream.read_exact(&mut mask).await?;
        Some(mask)
    } else {
        None
    };
    let mut payload = vec![0_u8; len as usize];
    stream.read_exact(&mut payload).await?;
    if let Some(mask) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok(WebSocketFrame { opcode, payload })
}
