use std::sync::Arc;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use orbcode_app_server::AppServer;
use orbcode_app_server::message_processor::{CHANNEL_SINK_CAPACITY, ChannelSink, MessageProcessor};
use orbcode_app_server_protocol::{ClientMessage, ServerMessage};

use crate::TransportError;

/// Configuration for the NDJSON transport.
#[derive(Clone)]
pub struct StdioTransportConfig {
    /// Maximum size in bytes for a single incoming JSON line. Lines exceeding
    /// this limit are logged and skipped rather than parsed.
    pub max_payload_bytes: usize,

    /// Optional authentication token for socket transports.
    ///
    /// When set, the transport expects the very first line from the client to
    /// be this exact token (trimmed of leading/trailing whitespace). If the
    /// token is missing (immediate EOF) or does not match, the connection is
    /// rejected with [`TransportError::AuthenticationFailed`] before any
    /// protocol messages are processed.
    ///
    /// Stdio transports leave this as `None` since they are implicitly trusted
    /// (the parent process controls both ends of the pipe).
    pub auth_token: Option<String>,
}

impl Default for StdioTransportConfig {
    fn default() -> Self {
        Self {
            max_payload_bytes: 10 * 1024 * 1024,
            auth_token: None,
        }
    }
}

/// Run the NDJSON transport over real stdin/stdout.
///
/// This is the primary entry point for launching a stdio-based transport
/// server. It reads newline-delimited JSON from stdin, passes each message
/// to a [`MessageProcessor`], and writes server responses as NDJSON to
/// stdout.
///
/// Returns when stdin reaches EOF or an unrecoverable I/O error occurs.
pub async fn run_stdio_transport(
    app_server: AppServer,
    config: StdioTransportConfig,
) -> Result<(), TransportError> {
    run_transport(tokio::io::stdin(), tokio::io::stdout(), app_server, config).await
}

/// Read bytes until a newline or EOF, with a hard cap on memory usage.
///
/// Reads into `buf` up to `max_bytes`. If the line (content before newline)
/// exceeds `max_bytes`, the excess is consumed and discarded, and the return
/// value will be > `max_bytes` to signal overflow. Returns 0 on EOF.
async fn read_capped_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max_bytes: usize,
) -> Result<usize, TransportError> {
    let mut total = 0usize;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(total);
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            let to_consume = pos + 1;
            if total + pos <= max_bytes {
                buf.extend_from_slice(&available[..pos]);
            }
            // Count content bytes only, not the trailing newline: including it
            // made a line whose content is exactly `max_bytes` report
            // `max_bytes + 1` and be rejected, so the effective limit was
            // `max_bytes - 1`.
            total += pos;
            reader.consume(to_consume);
            return Ok(total);
        }
        let chunk_len = available.len();
        if total + chunk_len <= max_bytes {
            buf.extend_from_slice(available);
        }
        total += chunk_len;
        reader.consume(chunk_len);
        if total > max_bytes {
            loop {
                let avail = reader.fill_buf().await?;
                if avail.is_empty() {
                    return Ok(total);
                }
                if let Some(pos) = avail.iter().position(|&b| b == b'\n') {
                    total += pos; // content bytes only (consistent with above)
                    reader.consume(pos + 1);
                    return Ok(total);
                }
                let len = avail.len();
                total += len;
                reader.consume(len);
            }
        }
    }
}

/// Auth handshake timeout — a client that connects but doesn't send the
/// token within this window is rejected so it can't block the accept loop.
const AUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Compare two auth tokens in (near-)constant time.
///
/// A plain `a != b` short-circuits on the first differing byte, leaking token
/// bytes through response timing. Length is compared first (the length itself
/// is observable — the standard, accepted tradeoff for token auth), then all
/// bytes are XOR-folded so the per-byte comparison time does not depend on where
/// a mismatch is. Comparing lengths up front also avoids the subtle bug of
/// folding a *truncated* length difference (`(a.len() ^ b.len()) as u8` is 0
/// when the lengths differ by a multiple of 256, e.g. token + 256 trailing NULs).
pub(crate) fn constant_time_token_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Read the first line from the stream and validate it against the expected
/// authentication token. Returns the [`BufReader`] wrapping the stream for
/// continued reading on success. Times out after [`AUTH_TIMEOUT`].
///
/// Fails with [`TransportError::AuthenticationFailed`] when the stream
/// reaches EOF, the token doesn't match, or the timeout elapses.
async fn validate_auth_token<R: tokio::io::AsyncRead + Unpin + Send>(
    reader: R,
    expected_token: &str,
) -> Result<BufReader<R>, TransportError> {
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    let read_result = tokio::time::timeout(AUTH_TIMEOUT, buf_reader.read_line(&mut line)).await;
    match read_result {
        Ok(Ok(0)) => {
            return Err(TransportError::AuthenticationFailed(
                "connection closed before auth token was received".into(),
            ));
        }
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(TransportError::Io(e)),
        Err(_) => {
            return Err(TransportError::AuthenticationFailed(
                "auth token not received within 30 seconds".into(),
            ));
        }
    }
    if !constant_time_token_eq(line.trim(), expected_token) {
        return Err(TransportError::AuthenticationFailed(
            "invalid auth token".into(),
        ));
    }
    Ok(buf_reader)
}

/// Run the NDJSON transport over arbitrary async reader/writer streams.
///
/// This is the generic implementation that powers [`run_stdio_transport`]
/// and can be used directly in tests with in-memory streams.
///
/// The transport:
/// 1. Reads NDJSON lines from `reader` (one [`ClientMessage`] per line)
/// 2. Passes each message to a [`MessageProcessor`]
/// 3. Receives [`ServerMessage`] values from the processor via channels
/// 4. Writes each [`ServerMessage`] as a NDJSON line to `writer`
/// 5. Enforces a maximum payload size (skipping oversized lines)
/// 6. Treats EOF on the reader as a clean disconnect
pub async fn run_transport<R, W>(
    reader: R,
    writer: W,
    app_server: AppServer,
    config: StdioTransportConfig,
) -> Result<(), TransportError>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    // If an auth token is configured, validate the first line before
    // processing any protocol messages.
    let reader = if let Some(ref expected_token) = config.auth_token {
        validate_auth_token(reader, expected_token).await?
    } else {
        BufReader::new(reader)
    };

    // Create two-tier channels for the sink.
    let (lossless_tx, mut lossless_rx) = mpsc::unbounded_channel::<ServerMessage>();
    let (best_effort_tx, mut best_effort_rx) =
        mpsc::channel::<ServerMessage>(CHANNEL_SINK_CAPACITY);
    let sink = Arc::new(ChannelSink::new(lossless_tx, best_effort_tx));
    let mut processor = MessageProcessor::new(app_server, sink);

    // Writer task: drain server messages and write NDJSON to the output.
    // Returns Err on I/O failure so the transport can propagate it.
    let mut write_handle: tokio::task::JoinHandle<Result<(), TransportError>> =
        tokio::spawn(async move {
            let mut writer = writer;
            loop {
                let msg = tokio::select! {
                    biased;
                    msg = lossless_rx.recv() => msg,
                    msg = best_effort_rx.recv() => msg,
                };
                let Some(msg) = msg else { break };
                let Ok(line) = serde_json::to_string(&msg) else {
                    continue;
                };
                writer.write_all(line.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
            }
            Ok(())
        });

    // Reader loop: read NDJSON lines with a hard memory cap per line.
    // Spawned as a task so reader and writer are fully independent —
    // if the writer fails, the reader task is aborted.
    let max_line = config.max_payload_bytes;
    let mut reader_handle = tokio::spawn(async move {
        let mut buf_reader = reader;
        let mut line_buf: Vec<u8> = Vec::with_capacity(4096);
        loop {
            line_buf.clear();
            let bytes_read = read_capped_line(&mut buf_reader, &mut line_buf, max_line).await?;
            if bytes_read == 0 {
                break; // EOF
            }
            if bytes_read > max_line {
                tracing::warn!(max = max_line, "line exceeds max_payload_bytes, skipping");
                continue;
            }
            let trimmed = match std::str::from_utf8(&line_buf) {
                Ok(s) => s.trim(),
                Err(_) => continue,
            };
            if trimmed.is_empty() {
                continue;
            }
            let message: ClientMessage = match serde_json::from_str(trimmed) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "malformed input, skipping line");
                    continue;
                }
            };
            processor.handle_message(message).await;
        }
        // Drop processor here to close sink channels, signaling writer to exit.
        drop(processor);
        Ok::<(), TransportError>(())
    });

    // Wait for either side to finish. If the writer fails first (I/O error),
    // abort the reader. If the reader finishes first (EOF), let the writer
    // drain remaining messages.
    tokio::select! {
        reader_res = &mut reader_handle => {
            // Reader done (EOF or error). Wait for writer to drain.
            match write_handle.await {
                Ok(Err(e)) => Err(e),
                _ => match reader_res {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(TransportError::ConnectionClosed),
                },
            }
        }
        writer_res = &mut write_handle => {
            // Writer finished first — abort the reader task so
            // MessageProcessor, subscriptions, and pending requests are
            // dropped immediately instead of living until reader EOF.
            reader_handle.abort();
            match writer_res {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(TransportError::ConnectionClosed),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use tokio::io::duplex;

    #[test]
    fn constant_time_token_eq_matches_semantics() {
        assert!(constant_time_token_eq("secret-token", "secret-token"));
        assert!(!constant_time_token_eq("secret-token", "secret-tokeo"));
        assert!(!constant_time_token_eq("secret", "secret-token"));
        assert!(!constant_time_token_eq("", "x"));
        assert!(constant_time_token_eq("", ""));
        // A length difference that is a multiple of 256 must still be rejected:
        // the correct token followed by 256 trailing NULs is NOT equal. (The old
        // `(a.len() ^ b.len()) as u8` folded this difference to 0.)
        let padded = format!("secret\0{}", "\0".repeat(255));
        assert_eq!(padded.len(), "secret".len() + 256);
        assert!(!constant_time_token_eq("secret", &padded));
        assert!(!constant_time_token_eq(&padded, "secret"));
    }

    #[tokio::test]
    async fn read_capped_line_keeps_line_of_exactly_max_bytes() {
        // A line whose *content* is exactly `max_bytes` must be kept: the cap
        // previously counted the trailing newline, so the effective limit was
        // `max_bytes - 1` and the boundary line was dropped.
        let max_bytes = 8usize;
        let input = b"12345678\n".to_vec(); // 8 content bytes + newline
        let mut reader = &input[..];
        let mut buf = Vec::new();
        let total = read_capped_line(&mut reader, &mut buf, max_bytes)
            .await
            .expect("read line");
        assert_eq!(total, max_bytes, "reported length must exclude the newline");
        assert!(total <= max_bytes, "boundary line must not be rejected");
        assert_eq!(buf, b"12345678");
    }

    /// Helper to build an initialize request JSON line.
    fn initialize_line() -> String {
        let msg = json!({
            "type": "request",
            "id": "init-1",
            "method": "initialize",
            "params": {
                "protocol_version": "1.0",
                "client_info": { "name": "test-transport", "version": "0.1" }
            }
        });
        format!("{}\n", serde_json::to_string(&msg).unwrap())
    }

    /// Helper to build a session/list request JSON line.
    fn session_list_line() -> String {
        let msg = json!({
            "type": "request",
            "id": "sl-1",
            "method": "session/list"
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
            "orbcode-transport-{label}-{}-{unique}",
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

    /// Read all available output lines from the reader half of a duplex,
    /// waiting briefly for responses to arrive.
    async fn read_responses(
        reader: tokio::io::DuplexStream,
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
                Ok(Ok(0)) => break, // EOF
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(msg) = serde_json::from_str::<ServerMessage>(trimmed) {
                        results.push(msg);
                    }
                }
                Ok(Err(_)) => break, // IO error
                Err(_) => break,     // timeout
            }
        }
        results
    }

    // -------------------------------------------------------------------
    // 1. Initialize over pipe
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn initialize_over_pipe() {
        let app = test_app("init-pipe").await;

        // Create a duplex pair: we write to `client_writer`, transport reads from
        // `transport_reader`. Transport writes to `transport_writer`, we read
        // from `client_reader`.
        let (client_writer, transport_reader) = duplex(8192);
        let (transport_writer, client_reader) = duplex(8192);

        let config = StdioTransportConfig::default();

        // Spawn the transport.
        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        // Write initialize request and close.
        let mut writer = client_writer;
        writer
            .write_all(initialize_line().as_bytes())
            .await
            .unwrap();
        writer.shutdown().await.unwrap();
        drop(writer);

        // Read responses.
        let responses = read_responses(client_reader, Duration::from_secs(5)).await;

        // Wait for transport to finish.
        let result = transport_handle.await.unwrap();
        assert!(result.is_ok());

        // Verify the initialize response.
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
    }

    // -------------------------------------------------------------------
    // 2. Session list over pipe
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn session_list_over_pipe() {
        let app = test_app("session-list-pipe").await;

        let (client_writer, transport_reader) = duplex(8192);
        let (transport_writer, client_reader) = duplex(8192);

        let config = StdioTransportConfig::default();

        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        // Write initialize then session/list, then close.
        let mut writer = client_writer;
        writer
            .write_all(initialize_line().as_bytes())
            .await
            .unwrap();
        writer
            .write_all(session_list_line().as_bytes())
            .await
            .unwrap();
        writer.shutdown().await.unwrap();
        drop(writer);

        let responses = read_responses(client_reader, Duration::from_secs(5)).await;

        let result = transport_handle.await.unwrap();
        assert!(result.is_ok());

        // Should have two responses: initialize + session/list.
        assert_eq!(responses.len(), 2);
        match &responses[1] {
            ServerMessage::Response(resp) => {
                assert_eq!(resp.id, "sl-1");
                match &resp.result {
                    orbcode_app_server_protocol::ResponseResult::Success { data: Some(data) } => {
                        // Session list returns a JSON array directly for a fresh server.
                        assert!(data.as_array().is_some(), "expected array, got: {data:?}");
                    }
                    other => panic!("expected Success with data, got: {other:?}"),
                }
            }
            other => panic!("expected Response, got: {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // 3. Unknown method returns MethodNotFound error
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn unknown_method_returns_error() {
        let app = test_app("unknown-method").await;

        let (client_writer, transport_reader) = duplex(8192);
        let (transport_writer, client_reader) = duplex(8192);

        let config = StdioTransportConfig::default();

        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        // Initialize first, then send unknown method.
        let unknown_line = format!(
            "{}\n",
            serde_json::to_string(&json!({
                "type": "request",
                "id": "bad-1",
                "method": "totally/bogus"
            }))
            .unwrap()
        );

        let mut writer = client_writer;
        writer
            .write_all(initialize_line().as_bytes())
            .await
            .unwrap();
        writer.write_all(unknown_line.as_bytes()).await.unwrap();
        writer.shutdown().await.unwrap();
        drop(writer);

        let responses = read_responses(client_reader, Duration::from_secs(5)).await;

        let result = transport_handle.await.unwrap();
        assert!(result.is_ok());

        assert_eq!(responses.len(), 2);
        match &responses[1] {
            ServerMessage::Response(resp) => {
                assert_eq!(resp.id, "bad-1");
                match &resp.result {
                    orbcode_app_server_protocol::ResponseResult::Error(err) => {
                        assert_eq!(
                            err.code,
                            orbcode_app_server_protocol::ErrorCode::MethodNotFound
                        );
                    }
                    other => panic!("expected Error, got: {other:?}"),
                }
            }
            other => panic!("expected Response, got: {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // 4. Payload too large is skipped
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn payload_too_large_skipped() {
        let app = test_app("payload-large").await;

        let (client_writer, transport_reader) = duplex(1024 * 1024);
        let (transport_writer, client_reader) = duplex(1024 * 1024);

        // Use a payload limit large enough for the initialize and session/list
        // requests but small enough that the oversized payload is rejected.
        let config = StdioTransportConfig {
            max_payload_bytes: 256,
            ..StdioTransportConfig::default()
        };

        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        // Build a line that exceeds 256 bytes.
        let big_payload = format!(
            "{}\n",
            serde_json::to_string(&json!({
                "type": "request",
                "id": "big-1",
                "method": "session/list",
                "params": {"data": "x".repeat(500)}
            }))
            .unwrap()
        );

        let mut writer = client_writer;
        // Send initialize (fits within 256 bytes).
        writer
            .write_all(initialize_line().as_bytes())
            .await
            .unwrap();
        // Send the oversized payload -- should be skipped.
        writer.write_all(big_payload.as_bytes()).await.unwrap();
        // Send a valid session/list -- should still work.
        writer
            .write_all(session_list_line().as_bytes())
            .await
            .unwrap();
        writer.shutdown().await.unwrap();
        drop(writer);

        let responses = read_responses(client_reader, Duration::from_secs(5)).await;

        let result = transport_handle.await.unwrap();
        assert!(result.is_ok());

        // Should have 2 responses: initialize + session/list (the big one was skipped).
        assert_eq!(responses.len(), 2);
        assert!(matches!(&responses[0], ServerMessage::Response(r) if r.id == "init-1"));
        assert!(matches!(&responses[1], ServerMessage::Response(r) if r.id == "sl-1"));
    }

    // -------------------------------------------------------------------
    // 5. EOF closes transport cleanly
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn eof_closes_transport() {
        let app = test_app("eof-close").await;

        let (client_writer, transport_reader) = duplex(8192);
        let (transport_writer, client_reader) = duplex(8192);

        let config = StdioTransportConfig::default();

        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        // Write initialize, then immediately close.
        let mut writer = client_writer;
        writer
            .write_all(initialize_line().as_bytes())
            .await
            .unwrap();
        writer.shutdown().await.unwrap();
        drop(writer);

        // Read whatever comes back.
        let _responses = read_responses(client_reader, Duration::from_secs(5)).await;

        // Transport should exit cleanly.
        let result = tokio::time::timeout(Duration::from_secs(5), transport_handle)
            .await
            .expect("transport should finish")
            .expect("transport task should not panic");

        assert!(result.is_ok(), "transport should exit cleanly on EOF");
    }

    // -------------------------------------------------------------------
    // 6. No-newline data exceeding cap is discarded without OOM
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn no_newline_capped_does_not_oom() {
        let app = test_app("no-newline").await;

        let (client_writer, transport_reader) = duplex(64 * 1024);
        let (transport_writer, client_reader) = duplex(64 * 1024);

        // 256 bytes: large enough for the initialize request (~146 bytes)
        // but small enough that the 300-byte junk line is rejected.
        let config = StdioTransportConfig {
            max_payload_bytes: 256,
            ..StdioTransportConfig::default()
        };

        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        let mut writer = client_writer;
        // 300 bytes without newline (exceeds 256 cap), then newline
        writer.write_all(&[b'x'; 300]).await.unwrap();
        writer.write_all(b"\n").await.unwrap();
        // Valid initialize after the oversized junk
        writer
            .write_all(initialize_line().as_bytes())
            .await
            .unwrap();
        writer.shutdown().await.unwrap();
        drop(writer);

        let responses = read_responses(client_reader, Duration::from_secs(5)).await;
        let result = transport_handle.await.unwrap();
        assert!(result.is_ok());
        // Oversized line skipped, initialize succeeds
        assert_eq!(responses.len(), 1);
        assert!(matches!(&responses[0], ServerMessage::Response(r) if r.id == "init-1"));
    }

    // -------------------------------------------------------------------
    // 7. Disconnect cleans up (writer dropped without shutdown)
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn disconnect_cleans_up() {
        let app = test_app("disconnect-cleanup").await;

        let (client_writer, transport_reader) = duplex(8192);
        let (transport_writer, client_reader) = duplex(8192);

        let config = StdioTransportConfig::default();

        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        // Write initialize request.
        let mut writer = client_writer;
        writer
            .write_all(initialize_line().as_bytes())
            .await
            .unwrap();

        // Wait for the response to arrive so the transport is actively running.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Drop the writer WITHOUT calling shutdown -- simulates an abrupt
        // client disconnect (broken pipe / process exit).
        drop(writer);

        // Also drop the client reader so the transport writer side sees a
        // broken pipe if it tries to write after the reader loop exits.
        drop(client_reader);

        // The transport must exit within 2 seconds -- no leaked tasks.
        let result = tokio::time::timeout(Duration::from_secs(2), transport_handle)
            .await
            .expect("transport should exit within 2s after disconnect")
            .expect("transport task should not panic");

        // Clean exit (EOF on reader) is acceptable; so is a broken-pipe error.
        match result {
            Ok(()) => {}
            Err(TransportError::Io(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(e) => panic!("unexpected transport error after disconnect: {e}"),
        }
    }

    // -------------------------------------------------------------------
    // 9. Permission server-request round-trip over duplex transport
    // -------------------------------------------------------------------
    //
    // Exercises the full `run_transport` path with a mock provider that
    // emits a tool_use block (bash), triggering a permission request.
    // The test:
    // 1. Initializes and bootstraps a session.
    // 2. Submits a turn that provokes a permission server-request.
    // 3. Reads the outgoing `permission/request` server-request.
    // 4. Sends a deny response back on the input side.
    // 5. Verifies the turn completes with a `tool_use_completed` event.
    #[tokio::test]
    async fn permission_server_request_deny_roundtrip_over_transport() {
        use std::time::{SystemTime, UNIX_EPOCH};
        use tokio::io::AsyncBufReadExt as _;

        // Build an AppServer with mock-provider tool_use scenario.
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "orbcode-transport-perm-rt-{}-{unique}",
            std::process::id()
        ));
        let home = base.join("home");
        let cwd = base.join("cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        let mut env = orbcode_config::sealed_provider_env_overrides();
        env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            "mock://anthropic?scenario=tool_use&key=bash&command=echo+hi".to_string(),
        );
        env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());

        let app = AppServer::new(
            cwd,
            orbcode_config::AppConfigOverrides {
                home_dir: Some(home),
                env_overrides: env,
                ..orbcode_config::AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        // Set up duplex streams.
        let (client_writer, transport_reader) = duplex(64 * 1024);
        let (transport_writer, client_reader) = duplex(64 * 1024);

        let config = StdioTransportConfig::default();

        // Spawn the transport.
        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        let mut writer = client_writer;
        let mut reader = BufReader::new(client_reader);

        // Helper closure: send a JSON line.
        async fn send_line(writer: &mut tokio::io::DuplexStream, msg: &serde_json::Value) {
            let line = serde_json::to_string(msg).unwrap();
            writer.write_all(line.as_bytes()).await.unwrap();
            writer.write_all(b"\n").await.unwrap();
            writer.flush().await.unwrap();
        }

        // Helper closure: read one JSON line with timeout.
        async fn recv_line(
            reader: &mut BufReader<tokio::io::DuplexStream>,
            timeout: Duration,
        ) -> Option<serde_json::Value> {
            let mut line = String::new();
            match tokio::time::timeout(timeout, reader.read_line(&mut line)).await {
                Ok(Ok(0)) => None,
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    Some(serde_json::from_str(trimmed).expect("valid JSON"))
                }
                Ok(Err(e)) => panic!("read error: {e}"),
                Err(_) => None,
            }
        }

        // 1. Initialize
        send_line(
            &mut writer,
            &json!({
                "type": "request",
                "id": "init-1",
                "method": "initialize",
                "params": {
                    "protocol_version": "1.0",
                    "client_info": { "name": "perm-roundtrip-test", "version": "0.1" }
                }
            }),
        )
        .await;
        let init_resp = recv_line(&mut reader, Duration::from_secs(5))
            .await
            .expect("init response");
        assert_eq!(init_resp["type"], "response");
        assert_eq!(init_resp["id"], "init-1");

        // 2. Bootstrap
        send_line(
            &mut writer,
            &json!({
                "type": "request",
                "id": "bs-1",
                "method": "session/bootstrap"
            }),
        )
        .await;
        let bs_resp = recv_line(&mut reader, Duration::from_secs(5))
            .await
            .expect("bootstrap response");
        assert_eq!(bs_resp["type"], "response");
        let session_id = bs_resp["result"]["data"]["session"]["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        // 3. Submit turn (triggers tool_use -> permission request)
        send_line(
            &mut writer,
            &json!({
                "type": "request",
                "id": "turn-1",
                "method": "turn/submit",
                "params": { "session_id": session_id, "prompt": "echo hi" }
            }),
        )
        .await;
        let turn_resp = recv_line(&mut reader, Duration::from_secs(5))
            .await
            .expect("turn response");
        assert_eq!(turn_resp["type"], "response");
        assert_eq!(turn_resp["id"], "turn-1");

        // 4. Read messages until we see the permission/request server-request.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let perm_req_id = loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for permission/request server-request");
            }
            let msg = recv_line(&mut reader, remaining)
                .await
                .expect("should receive messages");
            if msg["type"].as_str() == Some("request")
                && msg["method"].as_str() == Some("permission/request")
            {
                break msg["id"].as_str().expect("id").to_string();
            }
        };

        // 5. Send deny response back on the input side.
        send_line(
            &mut writer,
            &json!({
                "type": "response",
                "id": perm_req_id,
                "result": {
                    "status": "success",
                    "data": { "decision": "deny" }
                }
            }),
        )
        .await;

        // 6. Read messages until we see tool_use_completed notification.
        let mut saw_tool_completed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let Some(msg) = recv_line(&mut reader, remaining).await else {
                break;
            };
            if msg["type"].as_str() == Some("notification")
                && msg["method"].as_str() == Some("stream/event")
            {
                let event = &msg["params"]["event"];
                if event["event"].as_str() == Some("tool_use_completed") {
                    saw_tool_completed = true;
                    break;
                }
            }
        }

        assert!(
            saw_tool_completed,
            "tool_use_completed should arrive after deny, proving permission resolved"
        );

        // Close the writer to signal EOF and let the transport shut down.
        writer.shutdown().await.ok();
        drop(writer);
        drop(reader);

        // Wait for the transport to finish.
        let result = tokio::time::timeout(Duration::from_secs(10), transport_handle)
            .await
            .expect("transport should finish")
            .expect("transport task should not panic");
        // The transport may return Ok or an error (broken pipe / turn error).
        // Either is acceptable -- the test validates the permission round-trip.
        let _ = result;
    }

    // -------------------------------------------------------------------
    // 8. Writer I/O failure propagates as error (not Ok)
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn writer_failure_propagates_error() {
        use std::io;
        use std::pin::Pin;
        use std::task::{Context, Poll};

        /// A writer that always fails.
        struct FailWriter;
        impl tokio::io::AsyncWrite for FailWriter {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                _buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "test")))
            }
            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "test")))
            }
            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }

        let app = test_app("writer-fail").await;
        let (client_writer, transport_reader) = duplex(8192);
        let config = StdioTransportConfig::default();

        let transport_handle =
            tokio::spawn(
                async move { run_transport(transport_reader, FailWriter, app, config).await },
            );

        let mut writer = client_writer;
        writer
            .write_all(initialize_line().as_bytes())
            .await
            .unwrap();
        // Give the transport time to process and hit the writer error.
        tokio::time::sleep(Duration::from_millis(500)).await;
        drop(writer);

        let result = tokio::time::timeout(Duration::from_secs(5), transport_handle)
            .await
            .expect("should finish")
            .expect("should not panic");

        assert!(result.is_err(), "writer failure must propagate as Err");
    }

    // -------------------------------------------------------------------
    // 9. Auth: missing token rejected (EOF before token line)
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn missing_token_rejected() {
        let app = test_app("auth-missing").await;

        let (client_writer, transport_reader) = duplex(8192);
        let (transport_writer, _client_reader) = duplex(8192);

        let config = StdioTransportConfig {
            auth_token: Some("secret-token-123".into()),
            ..StdioTransportConfig::default()
        };

        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        // Close the writer immediately without sending anything.
        drop(client_writer);

        let result = tokio::time::timeout(Duration::from_secs(5), transport_handle)
            .await
            .expect("transport should finish")
            .expect("transport task should not panic");

        match result {
            Err(TransportError::AuthenticationFailed(reason)) => {
                assert!(
                    reason.contains("closed"),
                    "expected 'closed' in reason, got: {reason}"
                );
            }
            other => panic!("expected AuthenticationFailed, got: {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // 10. Auth: invalid token rejected
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn invalid_token_rejected() {
        let app = test_app("auth-invalid").await;

        let (client_writer, transport_reader) = duplex(8192);
        let (transport_writer, _client_reader) = duplex(8192);

        let config = StdioTransportConfig {
            auth_token: Some("secret-token-123".into()),
            ..StdioTransportConfig::default()
        };

        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        // Send a wrong token.
        let mut writer = client_writer;
        writer.write_all(b"wrong-token\n").await.unwrap();
        writer.shutdown().await.unwrap();
        drop(writer);

        let result = tokio::time::timeout(Duration::from_secs(5), transport_handle)
            .await
            .expect("transport should finish")
            .expect("transport task should not panic");

        match result {
            Err(TransportError::AuthenticationFailed(reason)) => {
                assert!(
                    reason.contains("invalid"),
                    "expected 'invalid' in reason, got: {reason}"
                );
            }
            other => panic!("expected AuthenticationFailed, got: {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // 11. Auth: valid token accepted, protocol works normally
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn valid_token_accepted() {
        let app = test_app("auth-valid").await;

        let (client_writer, transport_reader) = duplex(8192);
        let (transport_writer, client_reader) = duplex(8192);

        let token = "secret-token-123";
        let config = StdioTransportConfig {
            auth_token: Some(token.into()),
            ..StdioTransportConfig::default()
        };

        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        // Send the correct token first, then an initialize request.
        let mut writer = client_writer;
        writer
            .write_all(format!("{token}\n").as_bytes())
            .await
            .unwrap();
        writer
            .write_all(initialize_line().as_bytes())
            .await
            .unwrap();
        writer.shutdown().await.unwrap();
        drop(writer);

        // Read responses.
        let responses = read_responses(client_reader, Duration::from_secs(5)).await;

        // Wait for transport to finish.
        let result = transport_handle.await.unwrap();
        assert!(result.is_ok(), "transport should succeed with valid token");

        // Verify the initialize response.
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
    }

    // -------------------------------------------------------------------
    // 12. Slow consumer: best-effort deltas dropped, terminal event arrives
    // -------------------------------------------------------------------
    //
    // Connects via `run_transport`, initializes, bootstraps, and submits a
    // turn using the mock `many_deltas` provider (2000 deltas). Reads the
    // output side slowly through a tiny duplex buffer so the writer side
    // experiences backpressure, which causes the bounded best-effort channel
    // to fill and `try_send` to drop messages. Verifies:
    //   (a) the terminal `turn_finished` event arrives (lossless channel),
    //   (b) fewer best-effort deltas were received than were produced,
    //       proving messages were actually dropped.
    #[tokio::test]
    async fn slow_consumer_best_effort_dropped() {
        use std::time::{SystemTime, UNIX_EPOCH};
        use tokio::io::AsyncBufReadExt as _;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "orbcode-transport-slow-consumer-{}-{unique}",
            std::process::id()
        ));
        let home = base.join("home");
        let cwd = base.join("cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        // Use many_deltas with 2000 deltas — far exceeds the 1024 channel capacity.
        let mut env = orbcode_config::sealed_provider_env_overrides();
        env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            "mock://anthropic?scenario=many_deltas&attempts=2000".to_string(),
        );
        env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());

        let app = AppServer::new(
            cwd,
            orbcode_config::AppConfigOverrides {
                home_dir: Some(home),
                env_overrides: env,
                ..orbcode_config::AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        // Small duplex buffer (2KB) — large enough for handshake messages
        // but fills quickly during the 2000-delta flood, causing the writer
        // to block and the bounded channel to fill.
        let (client_writer, transport_reader) = duplex(2048);
        let (transport_writer, client_reader) = duplex(2048);

        let config = StdioTransportConfig::default();

        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        let mut writer = client_writer;
        let mut reader = BufReader::new(client_reader);

        async fn send_line(writer: &mut tokio::io::DuplexStream, msg: &serde_json::Value) {
            let line = serde_json::to_string(msg).unwrap();
            writer.write_all(line.as_bytes()).await.unwrap();
            writer.write_all(b"\n").await.unwrap();
            writer.flush().await.unwrap();
        }

        async fn recv_line_slow(
            reader: &mut BufReader<tokio::io::DuplexStream>,
            timeout: Duration,
        ) -> Option<serde_json::Value> {
            // Deliberate delay to simulate a slow consumer.
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut line = String::new();
            match tokio::time::timeout(timeout, reader.read_line(&mut line)).await {
                Ok(Ok(0)) => None,
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    serde_json::from_str(trimmed).ok()
                }
                Ok(Err(_)) | Err(_) => None,
            }
        }

        // 1. Initialize
        send_line(
            &mut writer,
            &json!({
                "type": "request",
                "id": "init-1",
                "method": "initialize",
                "params": {
                    "protocol_version": "1.0",
                    "client_info": { "name": "slow-consumer-test", "version": "0.1" }
                }
            }),
        )
        .await;
        let init_resp = recv_line_slow(&mut reader, Duration::from_secs(5))
            .await
            .expect("init response");
        assert_eq!(init_resp["type"], "response");
        assert_eq!(init_resp["id"], "init-1");

        // 2. Bootstrap
        send_line(
            &mut writer,
            &json!({
                "type": "request",
                "id": "bs-1",
                "method": "session/bootstrap"
            }),
        )
        .await;
        let bs_resp = recv_line_slow(&mut reader, Duration::from_secs(5))
            .await
            .expect("bootstrap response");
        assert_eq!(bs_resp["type"], "response");
        let session_id = bs_resp["result"]["data"]["session"]["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        // 3. Submit turn (many_deltas: produces 2000 deltas + turn_finished)
        send_line(
            &mut writer,
            &json!({
                "type": "request",
                "id": "turn-1",
                "method": "turn/submit",
                "params": { "session_id": session_id, "prompt": "hello" }
            }),
        )
        .await;
        let turn_resp = recv_line_slow(&mut reader, Duration::from_secs(5))
            .await
            .expect("turn response");
        assert_eq!(turn_resp["type"], "response");
        assert_eq!(turn_resp["id"], "turn-1");

        // 4. Drain ALL messages. Read slowly at first (to create
        //    backpressure that causes try_send drops), then fast once the
        //    terminal event arrives (to drain remaining buffered messages
        //    quickly). After turn_finished + a short idle timeout (no more
        //    messages arriving), delta_count is the true total delivered.
        let mut saw_turn_finished = false;
        let mut delta_count = 0_usize;

        // Helper: read one line with a short timeout (no artificial delay).
        async fn recv_line_fast(
            reader: &mut BufReader<tokio::io::DuplexStream>,
            timeout: Duration,
        ) -> Option<serde_json::Value> {
            let mut line = String::new();
            match tokio::time::timeout(timeout, reader.read_line(&mut line)).await {
                Ok(Ok(0)) => None,
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    serde_json::from_str(trimmed).ok()
                }
                Ok(Err(_)) | Err(_) => None,
            }
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            // Slow reads before terminal (creates backpressure);
            // fast reads after terminal (drains remaining buffer).
            // After terminal, use a 1-second timeout: if no message
            // arrives within 1s, the writer is idle and we have the
            // true total.
            let msg = if saw_turn_finished {
                recv_line_fast(&mut reader, Duration::from_secs(1)).await
            } else {
                recv_line_slow(&mut reader, remaining).await
            };
            let Some(msg) = msg else {
                break; // timeout or EOF — drain complete
            };
            if msg["type"].as_str() == Some("notification")
                && msg["method"].as_str() == Some("stream/event")
            {
                let event = &msg["params"]["event"];
                let event_type = event["event"].as_str().unwrap_or("");
                if event_type == "turn_finished" {
                    saw_turn_finished = true;
                }
                if event_type == "assistant_delta" {
                    delta_count += 1;
                }
            }
        }

        assert!(
            saw_turn_finished,
            "terminal turn_finished event must arrive even when the consumer is slow \
             (lossless channel guarantees delivery)"
        );

        // delta_count is the true total delivered (drained until idle).
        // The mock produced 2000 deltas; the slow consumer filled the 2KB
        // duplex, blocking the writer while the pump's try_send on the
        // bounded channel (capacity 1024) dropped excess messages.
        assert!(
            delta_count < 2000,
            "total delivered deltas ({delta_count}) must be less than total \
             produced (2000), proving try_send dropped under backpressure"
        );
        assert!(
            delta_count > 0,
            "at least some deltas should be delivered (got 0 — the bounded \
             channel has capacity 1024 so early deltas must survive)"
        );

        // Clean up.
        writer.shutdown().await.ok();
        drop(writer);
        drop(reader);
        let _ = tokio::time::timeout(Duration::from_secs(5), transport_handle).await;
    }

    // -------------------------------------------------------------------
    // 13. Overloaded notification channel does not block response
    // -------------------------------------------------------------------
    //
    // Uses `many_deltas` (2000 deltas) with a small duplex buffer so
    // notifications are being actively written when a `session/list`
    // request is submitted. The response must arrive before all
    // notifications are drained, proving that the lossless channel
    // (unbounded, biased-priority in select!) is not starved by the
    // best-effort notification flood.
    #[tokio::test]
    async fn overloaded_notification_does_not_block_response() {
        use std::time::{SystemTime, UNIX_EPOCH};
        use tokio::io::AsyncBufReadExt as _;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "orbcode-transport-overload-{}-{unique}",
            std::process::id()
        ));
        let home = base.join("home");
        let cwd = base.join("cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");

        // many_deltas produces 2000 notifications — enough to keep the
        // writer busy and the bounded channel populated.
        let mut env = orbcode_config::sealed_provider_env_overrides();
        env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            "mock://anthropic?scenario=many_deltas&attempts=2000".to_string(),
        );
        env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());

        let app = AppServer::new(
            cwd,
            orbcode_config::AppConfigOverrides {
                home_dir: Some(home),
                env_overrides: env,
                ..orbcode_config::AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        // Small buffer (4KB) so the writer is frequently blocked on I/O,
        // which keeps the bounded channel populated.
        let (client_writer, transport_reader) = duplex(4096);
        let (transport_writer, client_reader) = duplex(4096);

        let config = StdioTransportConfig::default();

        let transport_handle = tokio::spawn(async move {
            run_transport(transport_reader, transport_writer, app, config).await
        });

        let mut writer = client_writer;
        let mut reader = BufReader::new(client_reader);

        async fn send_json(writer: &mut tokio::io::DuplexStream, msg: &serde_json::Value) {
            let line = serde_json::to_string(msg).unwrap();
            writer.write_all(line.as_bytes()).await.unwrap();
            writer.write_all(b"\n").await.unwrap();
            writer.flush().await.unwrap();
        }

        async fn recv_json(
            reader: &mut BufReader<tokio::io::DuplexStream>,
            timeout: Duration,
        ) -> Option<serde_json::Value> {
            let mut line = String::new();
            match tokio::time::timeout(timeout, reader.read_line(&mut line)).await {
                Ok(Ok(0)) => None,
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    serde_json::from_str(trimmed).ok()
                }
                Ok(Err(_)) | Err(_) => None,
            }
        }

        // 1. Initialize
        send_json(
            &mut writer,
            &json!({
                "type": "request",
                "id": "init-1",
                "method": "initialize",
                "params": {
                    "protocol_version": "1.0",
                    "client_info": { "name": "overload-test", "version": "0.1" }
                }
            }),
        )
        .await;
        let init_resp = recv_json(&mut reader, Duration::from_secs(5))
            .await
            .expect("init response");
        assert_eq!(init_resp["id"], "init-1");

        // 2. Bootstrap
        send_json(
            &mut writer,
            &json!({
                "type": "request",
                "id": "bs-1",
                "method": "session/bootstrap"
            }),
        )
        .await;
        let bs_resp = recv_json(&mut reader, Duration::from_secs(5))
            .await
            .expect("bootstrap response");
        let session_id = bs_resp["result"]["data"]["session"]["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        // 3. Submit turn to generate a flood of notifications.
        send_json(
            &mut writer,
            &json!({
                "type": "request",
                "id": "turn-1",
                "method": "turn/submit",
                "params": { "session_id": session_id, "prompt": "hello" }
            }),
        )
        .await;
        let turn_resp = recv_json(&mut reader, Duration::from_secs(5))
            .await
            .expect("turn response");
        assert_eq!(turn_resp["id"], "turn-1");

        // 4. Wait briefly for notifications to start flowing, then send a
        //    new request while the notification flood is in progress.
        tokio::time::sleep(Duration::from_millis(100)).await;

        send_json(
            &mut writer,
            &json!({
                "type": "request",
                "id": "sl-overload",
                "method": "session/list"
            }),
        )
        .await;

        // 5. Read lines, counting how many notifications arrive BEFORE the
        //    session/list response. The lossless channel (biased select!)
        //    gives priority to responses, so the response should arrive well
        //    before all 2000 notifications have been drained.
        let mut found_session_list = false;
        let mut notifications_before_response = 0_usize;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let Some(msg) = recv_json(&mut reader, remaining).await else {
                break;
            };
            if msg["type"].as_str() == Some("response") && msg["id"].as_str() == Some("sl-overload")
            {
                found_session_list = true;
                break;
            }
            if msg["type"].as_str() == Some("notification") {
                notifications_before_response += 1;
            }
        }

        assert!(
            found_session_list,
            "session/list response must arrive while notifications are still streaming, \
             proving that lossless responses are not blocked by best-effort backpressure"
        );

        // The response arrived before all notifications were drained.
        // With 2000 deltas, seeing the response after fewer than 2000
        // notifications confirms priority delivery.
        assert!(
            notifications_before_response < 2000,
            "response should arrive before all notifications are drained \
             (saw {notifications_before_response} notifications before response)"
        );

        // Clean up.
        writer.shutdown().await.ok();
        drop(writer);
        drop(reader);
        let _ = tokio::time::timeout(Duration::from_secs(5), transport_handle).await;
    }
}
