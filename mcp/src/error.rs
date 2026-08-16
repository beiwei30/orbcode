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
        let message = sanitized_reqwest_error_message(&source);
        Self::HttpWithSource {
            message,
            source: source.without_url(),
        }
    }
}

fn sanitized_reqwest_error_message(source: &reqwest::Error) -> String {
    let message = source.to_string();
    let Some(url) = source.url() else {
        return message;
    };
    let mut sanitized = url.clone();
    let _ = sanitized.set_password(None);
    let _ = sanitized.set_username("");
    sanitized.set_query(None);
    sanitized.set_fragment(None);
    message.replace(url.as_str(), sanitized.as_str())
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
        let source_message = source.to_string();
        let error = McpError::http(source);

        assert!(error.source().is_some());
        assert_eq!(
            error.to_string(),
            format!("MCP HTTP transport error: {source_message}")
        );
    }

    #[tokio::test]
    async fn http_error_redacts_url_secrets_from_display_and_debug() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local listener");
        let addr = listener.local_addr().expect("local listener address");
        drop(listener);

        let source = reqwest::Client::new()
            .get(format!(
                "http://canary-user:canary-password@{addr}/mcp?access_token=canary-token"
            ))
            .send()
            .await
            .expect_err("closed local port should fail");
        let error = McpError::http(source);
        let rendered = error.to_string();
        let debug = format!("{error:?}");

        for secret in [
            "canary-user",
            "canary-password",
            "access_token",
            "canary-token",
        ] {
            assert!(!rendered.contains(secret), "Display leaked {secret}");
            assert!(!debug.contains(secret), "Debug leaked {secret}");
        }
        assert!(error.source().is_some());
    }
}
