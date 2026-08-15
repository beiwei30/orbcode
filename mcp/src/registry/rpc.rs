use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::McpError;
use crate::registry::{
    McpRegistry, RestartBackoff, SharedStdioClient, SharedStreamableHttpClient, StdioClientSlot,
    StreamableHttpClientSlot, canonicalize_auth_server, is_real_http_server, is_real_stdio_server,
    is_real_websocket_server,
};
use crate::transport::streamable_http::is_streamable_http_session_expired;
use crate::transport::{
    effective_http_headers, should_restart_stdio, spawn_stdio_client, streamable_http_client,
    websocket_client,
};
use crate::types::{McpServerConfig, McpServerStatus};

pub(super) fn should_retry_http<T>(result: &Result<T, McpError>) -> bool {
    matches!(
        result,
        Err(McpError::Http(_) | McpError::HttpWithSource { .. } | McpError::Io(_))
    )
}

/// Whether a transport error occurred *before the connection to the server was
/// established* — so the request bytes could not have been sent and the server
/// could not have executed the call. Only such CONNECT-phase failures make it
/// safe to replay a non-idempotent method.
///
/// This deliberately excludes send-phase and post-send failures. reqwest's
/// `is_request()` / "error sending request" and "connection reset" can all occur
/// *after* the connection opened and the request (or part of it) was written —
/// the server may already have executed `tools/call` and only the response was
/// lost, so replaying would double-execute. Likewise a stdio `BrokenPipe` /
/// `ConnectionReset` can follow a write the peer already consumed.
fn request_provably_not_delivered(error: &McpError) -> bool {
    match error {
        // When the reqwest error is preserved, ask it directly: only the connect
        // phase proves non-delivery (`is_request()` is broader and includes
        // send-phase failures, so it is NOT sufficient).
        McpError::HttpWithSource { source, .. } => source.is_connect(),
        // Stringified reqwest errors: only connection-establishment markers.
        // "error sending request" and "connection reset" are excluded — they can
        // occur after the request reached the server.
        McpError::Http(message) => {
            let message = message.to_ascii_lowercase();
            message.contains("connection refused")
                || message.contains("tcp connect error")
                || message.contains("dns error")
        }
        // Only a refused connection proves nothing was sent. Reset/aborted/
        // broken-pipe can follow an already-delivered write, so they are excluded.
        McpError::Io(error) => matches!(error.kind(), std::io::ErrorKind::ConnectionRefused),
        _ => false,
    }
}

/// Whether an MCP method is safe to transparently retry after a transport
/// failure. Read-only methods can be replayed freely; `tools/call` (and other
/// mutating methods) must not be, because the server may have already executed
/// the call and only the response was lost — a retry would double-execute.
fn is_idempotent_mcp_method(method: &str) -> bool {
    matches!(
        method,
        // Session handshake / health checks — side-effect free, safe to replay
        // after a transport drop.
        "initialize"
            | "ping"
            // Read-only queries.
            | "tools/list"
            | "resources/list"
            | "resources/templates/list"
            | "resources/read"
            | "prompts/list"
            | "prompts/get"
    )
}

impl McpRegistry {
    /// Record Ready/Failed status from a real transport RPC outcome.
    pub(crate) async fn finish_probe<T>(
        &self,
        server_id: &str,
        result: Result<T, McpError>,
    ) -> Result<T, McpError> {
        match result {
            Ok(value) => {
                self.set_server_status(server_id, McpServerStatus::Ready, None)
                    .await;
                Ok(value)
            }
            Err(error) => {
                let error = canonicalize_auth_server(error, server_id);
                let status = if matches!(error, McpError::AuthRequired { .. }) {
                    McpServerStatus::Unauthorized
                } else {
                    McpServerStatus::Failed
                };
                self.set_server_status(server_id, status, Some(error.to_string()))
                    .await;
                Err(error)
            }
        }
    }

    /// Dispatch a JSON-RPC request to whichever real transport backs `config`.
    /// Returns `None` for modeled servers so callers fall back to seeded data.
    pub(crate) async fn transport_rpc<T: DeserializeOwned>(
        &self,
        config: &McpServerConfig,
        method: &str,
        params: Value,
    ) -> Option<Result<T, McpError>> {
        if is_real_stdio_server(config) {
            Some(self.stdio_rpc(config, method, params).await)
        } else if is_real_http_server(config) {
            Some(self.http_rpc(config, method, params).await)
        } else if is_real_websocket_server(config) {
            Some(self.websocket_rpc(config, method, params).await)
        } else {
            None
        }
    }

    pub(crate) async fn stdio_rpc<T: DeserializeOwned>(
        &self,
        config: &McpServerConfig,
        method: &str,
        params: Value,
    ) -> Result<T, McpError> {
        let result = self.stdio_request::<T>(config, method, &params).await;
        match result {
            Ok(value) => Ok(value),
            Err(error) if self.should_restart(&config.id, &error).await => {
                // Restart the crashed client, but only replay the request when
                // it is safe: the method is idempotent, or the request provably
                // never reached the server. A `tools/call` that may have
                // executed before the transport died is surfaced, not replayed,
                // to avoid running the mutating call twice.
                let retryable =
                    is_idempotent_mcp_method(method) || request_provably_not_delivered(&error);
                let cause = error.to_string();
                self.restart_stdio_client(config, error).await?;
                if retryable {
                    self.stdio_request::<T>(config, method, &params).await
                } else {
                    Err(McpError::Protocol(format!(
                        "MCP request `{method}` failed ({cause}) and was not retried to avoid \
                         re-executing a non-idempotent call; the server was restarted"
                    )))
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn stdio_request<T: DeserializeOwned>(
        &self,
        config: &McpServerConfig,
        method: &str,
        params: &Value,
    ) -> Result<T, McpError> {
        let slot = self.stdio_client(config).await?;
        let (_permit, mut client) = slot.acquire().await?;
        let result = client.request::<T>(method, params.clone()).await;
        slot.return_client(client);
        result
    }

    pub(crate) async fn http_rpc<T: DeserializeOwned>(
        &self,
        config: &McpServerConfig,
        method: &str,
        params: Value,
    ) -> Result<T, McpError> {
        let result = self
            .streamable_http_request::<T>(config, method, &params)
            .await;
        let session_expired = result
            .as_ref()
            .err()
            .is_some_and(is_streamable_http_session_expired);
        // Retry a transport error only when it is safe: the method is idempotent,
        // or the request provably never reached the server (so a `tools/call`
        // could not have executed). A post-send failure on a mutating method is
        // surfaced rather than replayed, to avoid double execution.
        let idempotent = is_idempotent_mcp_method(method);
        let transport_retryable = should_retry_http(&result)
            && (idempotent
                || result
                    .as_ref()
                    .err()
                    .is_some_and(request_provably_not_delivered));
        let should_retry = transport_retryable || (idempotent && session_expired);
        if should_retry {
            self.restart_streamable_http_client(config).await;
            return self
                .streamable_http_request::<T>(config, method, &params)
                .await;
        }
        if session_expired {
            self.restart_streamable_http_client(config).await;
        }
        result
    }

    pub(crate) async fn websocket_rpc<T: DeserializeOwned>(
        &self,
        config: &McpServerConfig,
        method: &str,
        params: Value,
    ) -> Result<T, McpError> {
        let access_token = self.mcp_oauth_access_token(&config.id).await?;
        let mut client = websocket_client(config, access_token.as_deref()).await?;
        client.initialize().await?;
        client.request::<T>(method, params).await
    }

    pub(crate) async fn stdio_client(
        &self,
        config: &McpServerConfig,
    ) -> Result<SharedStdioClient, McpError> {
        if let Some(client) = self.stdio_clients.lock().await.get(&config.id).cloned() {
            return Ok(client);
        }

        self.set_server_status(&config.id, McpServerStatus::Starting, None)
            .await;
        let client = spawn_stdio_client(config).await?;
        let client = Arc::new(StdioClientSlot::new(client));
        self.stdio_clients
            .lock()
            .await
            .insert(config.id.clone(), client.clone());
        self.restart_backoffs
            .lock()
            .await
            .entry(config.id.clone())
            .or_insert_with(RestartBackoff::new)
            .reset();
        self.set_server_status(&config.id, McpServerStatus::Ready, None)
            .await;
        Ok(client)
    }

    async fn streamable_http_request<T: DeserializeOwned>(
        &self,
        config: &McpServerConfig,
        method: &str,
        params: &Value,
    ) -> Result<T, McpError> {
        let access_token = self.mcp_oauth_access_token(&config.id).await?;
        let headers = effective_http_headers(config, access_token.as_deref())?;
        let slot = self.streamable_http_client(config, headers.clone()).await?;
        let mut lease = slot.acquire().await?;
        lease.client_mut().set_headers(headers);
        lease
            .client_mut()
            .request::<T>(method, params.clone())
            .await
    }

    pub(crate) async fn streamable_http_client(
        &self,
        config: &McpServerConfig,
        headers: reqwest::header::HeaderMap,
    ) -> Result<SharedStreamableHttpClient, McpError> {
        if let Some(client) = self
            .streamable_http_clients
            .lock()
            .await
            .get(&config.id)
            .cloned()
        {
            return Ok(client);
        }

        let start_lock = {
            let mut locks = self.streamable_http_start_locks.lock().await;
            locks
                .entry(config.id.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _start_guard = start_lock.lock().await;
        if let Some(client) = self
            .streamable_http_clients
            .lock()
            .await
            .get(&config.id)
            .cloned()
        {
            return Ok(client);
        }

        self.set_server_status(&config.id, McpServerStatus::Starting, None)
            .await;
        let mut client = streamable_http_client(config, None)?;
        client.set_headers(headers);
        client.initialize().await?;
        let client = Arc::new(StreamableHttpClientSlot::new(client));
        self.streamable_http_clients
            .lock()
            .await
            .insert(config.id.clone(), client.clone());
        self.restart_backoffs
            .lock()
            .await
            .entry(config.id.clone())
            .or_insert_with(RestartBackoff::new)
            .reset();
        self.set_server_status(&config.id, McpServerStatus::Ready, None)
            .await;
        Ok(client)
    }

    pub(crate) async fn restart_streamable_http_client(&self, config: &McpServerConfig) {
        self.streamable_http_clients.lock().await.remove(&config.id);
        self.set_server_status(&config.id, McpServerStatus::Restarting, None)
            .await;
    }

    pub(crate) async fn should_restart(&self, server_id: &str, error: &McpError) -> bool {
        if !should_restart_stdio(error) {
            return false;
        }
        let mut backoffs = self.restart_backoffs.lock().await;
        let backoff = backoffs
            .entry(server_id.to_string())
            .or_insert_with(RestartBackoff::new);
        backoff.is_allowed()
    }

    pub(crate) async fn restart_stdio_client(
        &self,
        config: &McpServerConfig,
        cause: McpError,
    ) -> Result<(), McpError> {
        let delay = {
            let mut backoffs = self.restart_backoffs.lock().await;
            let backoff = backoffs
                .entry(config.id.clone())
                .or_insert_with(RestartBackoff::new);
            let delay = backoff.next_delay();
            backoff.record_attempt();
            delay
        };
        self.set_server_status(
            &config.id,
            McpServerStatus::Restarting,
            Some(cause.to_string()),
        )
        .await;
        tokio::time::sleep(delay).await;
        self.shutdown_stdio_client(&config.id).await;
        Ok(())
    }

    pub(crate) async fn shutdown_stdio_client(&self, server_id: &str) {
        let slot = self.stdio_clients.lock().await.remove(server_id);
        let Some(slot) = slot else {
            return;
        };
        if let Ok((_permit, mut client)) = slot.acquire().await {
            let _ = client.shutdown().await;
        }
        self.set_server_status(server_id, McpServerStatus::Stopped, None)
            .await;
    }

    pub(crate) async fn shutdown_streamable_http_client(&self, server_id: &str) {
        let slot = self.streamable_http_clients.lock().await.remove(server_id);
        let Some(slot) = slot else {
            return;
        };
        if let Ok(mut lease) = slot.acquire().await {
            let _ = lease.client_mut().shutdown().await;
        }
        self.set_server_status(server_id, McpServerStatus::Stopped, None)
            .await;
    }
}

#[cfg(test)]
mod rpc_retry_tests {
    use super::{is_idempotent_mcp_method, request_provably_not_delivered};
    use crate::error::McpError;

    #[test]
    fn tools_call_is_not_idempotent_but_reads_are() {
        assert!(!is_idempotent_mcp_method("tools/call"));
        assert!(is_idempotent_mcp_method("tools/list"));
        assert!(is_idempotent_mcp_method("initialize"));
        assert!(is_idempotent_mcp_method("resources/read"));
    }

    #[test]
    fn not_delivered_is_connect_phase_only() {
        // CONNECT-phase failures: the connection was never established, so the
        // request bytes could not have been sent — even a mutating call is safe
        // to retry.
        assert!(request_provably_not_delivered(&McpError::Http(
            "tcp connect error: Connection refused".into()
        )));
        assert!(request_provably_not_delivered(&McpError::Http(
            "dns error: failed to lookup address".into()
        )));
        assert!(request_provably_not_delivered(&McpError::Io(
            std::io::Error::from(std::io::ErrorKind::ConnectionRefused)
        )));

        // Send-phase and post-send failures: the connection opened and the
        // request (or part of it) may already have reached the server, which
        // could have executed `tools/call`. Replaying would double-execute, so
        // these must NOT be treated as safe — this is the correctness fix.
        assert!(!request_provably_not_delivered(&McpError::Http(
            "error sending request for url (http://host/mcp)".into()
        )));
        assert!(!request_provably_not_delivered(&McpError::Http(
            "connection reset by peer".into()
        )));
        assert!(!request_provably_not_delivered(&McpError::Io(
            std::io::Error::from(std::io::ErrorKind::BrokenPipe)
        )));
        assert!(!request_provably_not_delivered(&McpError::Io(
            std::io::Error::from(std::io::ErrorKind::ConnectionReset)
        )));

        // Response-phase failures: likewise unsafe to replay.
        assert!(!request_provably_not_delivered(&McpError::Http(
            "error decoding response body".into()
        )));
        assert!(!request_provably_not_delivered(&McpError::Http(
            "connection closed before message completed".into()
        )));
    }

    #[tokio::test]
    async fn request_provably_not_delivered_accepts_typed_reqwest_connect_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local listener");
        let addr = listener.local_addr().expect("local listener address");
        drop(listener);
        let source = reqwest::Client::new()
            .get(format!("http://{addr}"))
            .send()
            .await
            .expect_err("closed local port should fail during connect");

        assert!(request_provably_not_delivered(&McpError::http(source)));
    }
}
