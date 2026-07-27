use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue, WWW_AUTHENTICATE};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::time::timeout;

use crate::error::McpError;
use crate::wire::{
    StdioInitializeResult, StdioListToolsResult, StdioToolCallResult, parse_http_json_rpc_response,
    parse_json_rpc_result,
};

pub(crate) const STREAMABLE_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const MCP_SESSION_ID: &str = "mcp-session-id";
const MCP_PROTOCOL_VERSION: &str = "mcp-protocol-version";

pub(crate) struct StreamableHttpMcpClient {
    client: reqwest::Client,
    endpoint: String,
    headers: HeaderMap,
    next_id: u64,
    request_timeout: Duration,
    session_id: Option<String>,
    protocol_version: Option<String>,
}

impl StreamableHttpMcpClient {
    pub(crate) fn new(
        endpoint: impl Into<String>,
        headers: HeaderMap,
        request_timeout: Duration,
    ) -> Result<Self, McpError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| McpError::Http(error.to_string()))?,
            endpoint: endpoint.into(),
            headers,
            next_id: 1,
            request_timeout,
            session_id: None,
            protocol_version: None,
        })
    }

    pub(crate) fn set_headers(&mut self, headers: HeaderMap) {
        self.headers = headers;
    }

    pub(crate) async fn initialize(&mut self) -> Result<StdioInitializeResult, McpError> {
        self.session_id = None;
        self.protocol_version = None;
        let result: StdioInitializeResult = self
            .request(
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
            .await?;
        self.protocol_version = Some(result.protocol_version.clone());
        Ok(result)
    }

    pub(crate) async fn list_tools(&mut self) -> Result<StdioListToolsResult, McpError> {
        self.request("tools/list", json!({})).await
    }

    #[cfg_attr(not(test), allow(dead_code))]
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
        let id = self.next_id;
        self.next_id += 1;

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let response = timeout(
            self.request_timeout,
            self.client
                .post(&self.endpoint)
                .headers(self.headers.clone())
                .headers(self.session_headers()?)
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json, text/event-stream")
                .json(&request)
                .send(),
        )
        .await
        .map_err(|_| McpError::Timeout(format!("http {method}")))?
        .map_err(|error| McpError::Http(error.to_string()))?;

        if let Some(session_id) = response
            .headers()
            .get(MCP_SESSION_ID)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
        {
            self.session_id = Some(session_id.to_string());
        }

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            let authenticate = response
                .headers()
                .get(WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .map(|value| format!("; WWW-Authenticate: {value}"))
                .unwrap_or_default();
            return Err(McpError::AuthRequired {
                server: self.endpoint.clone(),
                reason: format!(
                    "remote server returned HTTP {}{authenticate}",
                    response.status()
                ),
            });
        }
        if response.status() == reqwest::StatusCode::NOT_FOUND && self.session_id.is_some() {
            return Err(McpError::Protocol(format!(
                "remote server returned HTTP 404 for {method}; Streamable HTTP session expired"
            )));
        }
        if response.status() == reqwest::StatusCode::ACCEPTED
            || response.status() == reqwest::StatusCode::NO_CONTENT
        {
            return Err(McpError::Protocol(format!(
                "remote server returned HTTP {} without a JSON-RPC response for {method}",
                response.status()
            )));
        }
        if !response.status().is_success() {
            return Err(McpError::Protocol(format!(
                "remote server returned HTTP {} for {method}",
                response.status()
            )));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let text = timeout(self.request_timeout, response.text())
            .await
            .map_err(|_| McpError::Timeout(format!("http {method} body")))?
            .map_err(|error| McpError::Http(error.to_string()))?;
        let media_type = content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let response = match media_type.as_str() {
            "application/json" | "text/event-stream" => parse_http_json_rpc_response(&text, id)?,
            _ => {
                return Err(McpError::Protocol(format!(
                    "remote server returned unsupported content type `{}` for {method}",
                    if content_type.is_empty() {
                        "<missing>"
                    } else {
                        content_type.as_str()
                    }
                )));
            }
        };
        parse_json_rpc_result(response, id, method)
    }

    pub(crate) async fn shutdown(&mut self) -> Result<(), McpError> {
        let Some(session_id) = self.session_id.take() else {
            return Ok(());
        };
        let response = timeout(
            self.request_timeout,
            self.client
                .delete(&self.endpoint)
                .headers(self.headers.clone())
                .headers(self.protocol_headers()?)
                .header(MCP_SESSION_ID, session_id)
                .send(),
        )
        .await
        .map_err(|_| McpError::Timeout("http shutdown".to_string()))?
        .map_err(|error| McpError::Http(error.to_string()))?;

        if response.status().is_success()
            || response.status() == reqwest::StatusCode::NOT_FOUND
            || response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED
            || response.status() == reqwest::StatusCode::NOT_IMPLEMENTED
        {
            return Ok(());
        }
        Err(McpError::Protocol(format!(
            "remote server returned HTTP {} for Streamable HTTP shutdown",
            response.status()
        )))
    }

    fn session_headers(&self) -> Result<HeaderMap, McpError> {
        let mut headers = self.protocol_headers()?;
        if let Some(session_id) = &self.session_id {
            let value = HeaderValue::from_str(session_id).map_err(|error| {
                McpError::Protocol(format!("invalid Streamable HTTP session id: {error}"))
            })?;
            headers.insert(MCP_SESSION_ID, value);
        }
        Ok(headers)
    }

    fn protocol_headers(&self) -> Result<HeaderMap, McpError> {
        let mut headers = HeaderMap::new();
        if let Some(protocol_version) = &self.protocol_version {
            let value = HeaderValue::from_str(protocol_version).map_err(|error| {
                McpError::Protocol(format!("invalid MCP protocol version header: {error}"))
            })?;
            headers.insert(MCP_PROTOCOL_VERSION, value);
        }
        Ok(headers)
    }
}

pub(crate) fn is_streamable_http_session_expired(error: &McpError) -> bool {
    matches!(error, McpError::Protocol(message) if message.contains("Streamable HTTP session expired"))
}
