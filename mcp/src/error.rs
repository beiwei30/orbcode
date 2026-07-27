use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("invalid MCP configuration: {0}")]
    InvalidConfig(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unknown MCP server: {0}")]
    UnknownServer(String),
    #[error("unknown MCP resource: {0}")]
    UnknownResource(String),
    #[error("unknown MCP tool: {0}")]
    UnknownTool(String),
    #[error("MCP server is disabled: {0}")]
    DisabledServer(String),
    #[error("MCP request timed out: {0}")]
    Timeout(String),
    #[error("MCP protocol error: {0}")]
    Protocol(String),
    #[error("MCP HTTP transport error: {0}")]
    Http(String),
    #[error("MCP HTTP transport error: {message}")]
    HttpWithSource {
        message: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("MCP JSON-RPC error {code}: {message}")]
    JsonRpc { code: i64, message: String },
    #[error(
        "MCP server `{server}` is not trusted ({status}); approve it with `orbcode mcp trust {server}`"
    )]
    ServerUntrusted {
        server: String,
        status: &'static str,
    },
    #[error(
        "MCP server `{server}` requires authentication: {reason}; sign in with `orbcode mcp auth login {server}` (run `orbcode mcp diagnose {server}` for details)"
    )]
    AuthRequired { server: String, reason: String },
    #[error("MCP request cancelled")]
    Cancelled,
}

impl McpError {
    pub(crate) fn http(source: reqwest::Error) -> Self {
        Self::HttpWithSource {
            message: source.to_string(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::time::Duration;

    use super::McpError;

    #[tokio::test]
    async fn http_error_retains_reqwest_source() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local listener");
        let addr = listener.local_addr().expect("local listener address");
        drop(listener);

        let source = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .expect("build client")
            .get(format!("http://{addr}"))
            .send()
            .await
            .expect_err("closed local port should fail");
        let error = McpError::http(source);

        assert!(error.source().is_some());
    }
}
