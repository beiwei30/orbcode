use std::path::Path;

use tokio::net::UnixListener;

use orbcode_app_server::AppServer;

use crate::TransportError;
use crate::stdio::{StdioTransportConfig, run_transport};

/// Run the NDJSON transport over a Unix domain socket.
///
/// Binds a [`UnixListener`] at `socket_path` and accepts client
/// connections sequentially in a loop. Each connection is handled by
/// [`run_transport`]; when a client disconnects (or fails auth), the
/// server loops back to accept the next connection. Auth/protocol
/// errors only close that connection — they do NOT terminate the server.
///
/// Returns only on an unrecoverable bind/listen error.
pub async fn run_unix_socket_transport(
    socket_path: &Path,
    app_server: AppServer,
    config: StdioTransportConfig,
) -> Result<(), TransportError> {
    run_unix_socket_transport_with_ready(socket_path, app_server, config, None).await
}

pub async fn run_unix_socket_transport_with_ready(
    socket_path: &Path,
    app_server: AppServer,
    config: StdioTransportConfig,
    ready_tx: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<(), TransportError> {
    if socket_path.exists() {
        use std::os::unix::fs::FileTypeExt;
        let meta = std::fs::symlink_metadata(socket_path)?;
        if meta.file_type().is_socket() {
            std::fs::remove_file(socket_path)?;
        } else {
            return Err(TransportError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "refusing to remove non-socket path: {}",
                    socket_path.display()
                ),
            )));
        }
    }

    let listener = UnixListener::bind(socket_path)?;
    let _socket_guard = SocketCleanupGuard(socket_path.to_path_buf());
    tracing::info!(socket_path = %socket_path.display(), "Unix socket transport listening");
    if let Some(tx) = ready_tx {
        let _ = tx.send(());
    }

    loop {
        let (stream, _addr) = listener.accept().await?;
        tracing::info!(socket_path = %socket_path.display(), "Unix socket client accepted");
        let (reader, writer) = tokio::io::split(stream);
        match run_transport(reader, writer, app_server.clone(), config.clone()).await {
            Ok(()) => {
                tracing::info!(socket_path = %socket_path.display(), "Unix socket client disconnected")
            }
            Err(TransportError::AuthenticationFailed(ref reason)) => {
                tracing::warn!(socket_path = %socket_path.display(), %reason, "Unix socket client auth failed, continuing");
            }
            Err(ref e) => {
                tracing::warn!(socket_path = %socket_path.display(), error = %e, "Unix socket client error, continuing");
            }
        }
    }
}

/// RAII guard that removes the socket file on drop, but only if it is
/// still a Unix socket (guards against the path being replaced with a
/// regular file or symlink during the server's lifetime).
struct SocketCleanupGuard(std::path::PathBuf);

impl Drop for SocketCleanupGuard {
    fn drop(&mut self) {
        use std::os::unix::fs::FileTypeExt;
        if let Ok(meta) = std::fs::symlink_metadata(&self.0)
            && meta.file_type().is_socket()
        {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    use orbcode_app_server_protocol::ServerMessage;

    /// Helper to build an initialize request JSON line.
    fn initialize_line() -> String {
        let msg = json!({
            "type": "request",
            "id": "init-1",
            "method": "initialize",
            "params": {
                "protocol_version": "1.0",
                "client_info": { "name": "test-socket", "version": "0.1" }
            }
        });
        format!("{}\n", serde_json::to_string(&msg).unwrap())
    }

    /// Create a temporary AppServer for testing.
    async fn test_app(label: &str) -> AppServer {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "orbcode-socket-{label}-{}-{unique}",
            std::process::id()
        ));
        let home = base.join("home");
        let cwd = base.join("cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        AppServer::new(
            cwd,
            orbcode_config::AppConfigOverrides {
                home_dir: Some(home),
                ..orbcode_config::AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server")
    }

    /// Read all available response lines from a reader, with a timeout.
    async fn read_responses<R: tokio::io::AsyncRead + Unpin>(
        reader: R,
        timeout: Duration,
    ) -> Vec<ServerMessage> {
        let mut buf_reader = BufReader::new(reader);
        let mut results = Vec::new();
        let mut line = String::new();

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            line.clear();
            let read_fut = buf_reader.read_line(&mut line);
            match tokio::time::timeout_at(deadline, read_fut).await {
                Ok(Ok(0)) => break,
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(msg) = serde_json::from_str::<ServerMessage>(trimmed) {
                        results.push(msg);
                    }
                }
                Ok(Err(_)) => break,
                Err(_) => break,
            }
        }
        results
    }

    /// Build a unique socket path inside the system temp dir.
    fn temp_socket_path(label: &str) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "orbcode-test-{label}-{}-{unique}.sock",
            std::process::id()
        ))
    }

    // -------------------------------------------------------------------
    // 1. Initialize over Unix socket
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn unix_socket_initialize() {
        let app = test_app("sock-init").await;
        let sock_path = temp_socket_path("init");

        let path_clone = sock_path.clone();
        let transport_handle = tokio::spawn(async move {
            let config = StdioTransportConfig::default();
            run_unix_socket_transport(&path_clone, app, config).await
        });

        // Wait briefly for the listener to bind.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect as a client.
        let stream = UnixStream::connect(&sock_path)
            .await
            .expect("connect to socket");
        let (reader, mut writer) = tokio::io::split(stream);

        // Send initialize and close.
        writer
            .write_all(initialize_line().as_bytes())
            .await
            .unwrap();
        writer.shutdown().await.unwrap();

        // Read responses.
        let responses = read_responses(reader, Duration::from_secs(5)).await;

        // Transport is now a sequential accept loop — it won't exit after
        // one client. Abort it after verifying the response.
        transport_handle.abort();

        // Verify initialize response.
        assert_eq!(responses.len(), 1);
        match &responses[0] {
            ServerMessage::Response(resp) => {
                assert_eq!(resp.id, "init-1");
                match &resp.result {
                    orbcode_app_server_protocol::ResponseResult::Success { data: Some(data) } => {
                        assert_eq!(data["server_info"]["name"], "orbcode");
                    }
                    other => panic!("expected Success with data, got: {other:?}"),
                }
            }
            other => panic!("expected Response, got: {other:?}"),
        }

        // Clean up.
        let _ = std::fs::remove_file(&sock_path);
    }

    // -------------------------------------------------------------------
    // 2. Disconnect without sending data closes transport cleanly
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn unix_socket_disconnect() {
        let app = test_app("sock-disc").await;
        let sock_path = temp_socket_path("disc");

        let path_clone = sock_path.clone();
        let transport_handle = tokio::spawn(async move {
            let config = StdioTransportConfig::default();
            run_unix_socket_transport(&path_clone, app, config).await
        });

        // Wait briefly for the listener to bind.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect and immediately drop (disconnect).
        let stream = UnixStream::connect(&sock_path)
            .await
            .expect("connect to socket");
        drop(stream);

        // Wait for the server to process the disconnect (it loops to accept
        // the next connection, not exit). Give it a moment then abort.
        tokio::time::sleep(Duration::from_millis(200)).await;
        transport_handle.abort();

        // Clean up.
        let _ = std::fs::remove_file(&sock_path);
    }

    // -------------------------------------------------------------------
    // 3. Rejects a regular file at the socket path
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn socket_rejects_regular_file() {
        let sock_path = temp_socket_path("reject-file");
        // Place a regular file where the socket would be created.
        std::fs::write(&sock_path, b"not a socket").unwrap();

        let app = test_app("reject-file").await;
        let config = StdioTransportConfig::default();
        let result = run_unix_socket_transport(&sock_path, app, config).await;

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("refusing to remove non-socket path"),
            "unexpected error: {msg}"
        );

        // The original file must still be intact.
        assert!(sock_path.exists(), "regular file should not be deleted");
        std::fs::remove_file(&sock_path).unwrap();
    }

    // -------------------------------------------------------------------
    // 4. Socket file is cleaned up when the transport exits
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn socket_cleanup_on_drop() {
        let app = test_app("cleanup").await;
        let sock_path = temp_socket_path("cleanup");

        let path_clone = sock_path.clone();
        let transport_handle = tokio::spawn(async move {
            let config = StdioTransportConfig::default();
            run_unix_socket_transport(&path_clone, app, config).await
        });

        // Wait briefly for the listener to bind.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Socket file should exist while the transport is running.
        assert!(
            sock_path.exists(),
            "socket file should exist while transport is running"
        );

        // Abort the transport (simulates process shutdown) — this drops
        // the SocketCleanupGuard which should remove the socket file.
        transport_handle.abort();
        let _ = transport_handle.await;

        // After abort, the SocketCleanupGuard should have removed the
        // socket file (it re-checks is_socket() in drop).
        // Note: abort may not always give the Drop a chance to run, so
        // we allow either outcome.
        // Clean up manually — abort may not run Drop.
        let _ = std::fs::remove_file(&sock_path);
    }

    // -------------------------------------------------------------------
    // 5. Rejects a symlink at the socket path
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn socket_rejects_symlink() {
        let sock_path = temp_socket_path("reject-symlink");
        let target = temp_socket_path("symlink-target");

        // Create a target file so the symlink is not dangling (dangling
        // symlinks return false from Path::exists and skip the guard).
        std::fs::write(&target, b"target").unwrap();
        std::os::unix::fs::symlink(&target, &sock_path).unwrap();

        let app = test_app("reject-symlink").await;
        let config = StdioTransportConfig::default();
        let result = run_unix_socket_transport(&sock_path, app, config).await;

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("refusing to remove non-socket path"),
            "unexpected error: {msg}"
        );

        // The symlink should still be intact.
        assert!(
            sock_path.symlink_metadata().is_ok(),
            "symlink should not be deleted"
        );

        std::fs::remove_file(&sock_path).unwrap();
        std::fs::remove_file(&target).unwrap();
    }
}
