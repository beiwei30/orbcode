use super::*;

/// Handle to the provider interrupt smoke server. `accepted` counts served
/// `messages` requests (not `count_tokens`); `shutdown` tells the accept loop to
/// stop so the test can join it deterministically once its assertions are done.
pub struct ProviderSmokeServer {
    pub base_url: String,
    pub accepted: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ProviderSmokeServer {
    /// Signal the accept loop to stop and wait for it to finish.
    pub fn shutdown_and_join(mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.join().expect("provider smoke server joins");
        }
    }
}

impl Drop for ProviderSmokeServer {
    fn drop(&mut self) {
        // Ensure the accept loop always terminates even if a test panics before
        // calling `shutdown_and_join`, so the server thread never lingers.
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn start_provider_interrupt_smoke_server() -> ProviderSmokeServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind provider smoke server");
    listener
        .set_nonblocking(true)
        .expect("set provider smoke server nonblocking");
    let address = listener.local_addr().expect("provider smoke server addr");
    let accepted = Arc::new(AtomicUsize::new(0));
    let shutdown = Arc::new(AtomicBool::new(false));
    let accepted_for_thread = accepted.clone();
    let shutdown_for_thread = shutdown.clone();
    // Keep accepting until the test signals shutdown, rather than stopping after
    // exactly two requests. Under heavy load the followup turn can open extra
    // connections (a trailing `count_tokens`, or a retry) whose ordering relative
    // to the `messages` request is not fixed; a fixed 2-request budget would race
    // and refuse one of them, which surfaced as the client's turn ending without
    // a `TurnFinished` (an intermittent `recv() -> None` in the test).
    let handle = thread::spawn(move || {
        while !shutdown_for_thread.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let accepted_for_request = accepted_for_thread.clone();
                    thread::spawn(move || {
                        // The listener is non-blocking so the accept loop can poll
                        // the shutdown flag; on BSD/macOS the accepted socket
                        // INHERITS that flag, which makes `set_read_timeout` a
                        // no-op and `read()` return `WouldBlock` (0 bytes) before a
                        // slow client's request arrives — truncating/misclassifying
                        // it under load. Restore blocking mode first so the timeout
                        // below actually applies.
                        let _ = stream.set_nonblocking(false);
                        // Generous read timeout: under a saturated CI host the
                        // request bytes can take far longer than a few hundred ms
                        // to arrive; a short timeout truncates the request and the
                        // client's turn then fails mid-flight.
                        let _ = stream.set_read_timeout(Some(StdDuration::from_secs(5)));
                        let request = read_test_http_request(&mut stream);
                        if is_anthropic_count_tokens_request(&request) {
                            write_anthropic_count_tokens_response(&mut stream);
                            return;
                        }
                        let request_index = accepted_for_request.fetch_add(1, Ordering::SeqCst) + 1;
                        if request_index == 1 {
                            stream
                                    .write_all(
                                        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                                    )
                                    .expect("write hanging response headers");
                            let keepalive = b": keepalive\n\n";
                            let chunk_prefix = format!("{:X}\r\n", keepalive.len());
                            stream
                                .write_all(chunk_prefix.as_bytes())
                                .expect("write keepalive chunk prefix");
                            stream.write_all(keepalive).expect("write keepalive");
                            stream
                                .write_all(b"\r\n")
                                .expect("write keepalive chunk suffix");
                            let _ = stream.flush();
                            thread::sleep(StdDuration::from_secs(2));
                            return;
                        }

                        // Any `messages` request after the interrupted first one
                        // is the followup (or a retry of it): answer it the same
                        // way so ordering under load cannot leave it unserved.
                        let body = concat!(
                            "event: message_start\n",
                            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                            "event: content_block_start\n",
                            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"provider followup ok\"}}\n\n",
                            "event: message_delta\n",
                            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
                        );
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body,
                        );
                        // The client may have already gone away (e.g. the turn
                        // was cancelled); a failed write must not crash the server.
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.flush();
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(StdDuration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });

    ProviderSmokeServer {
        base_url: format!("http://{address}"),
        accepted,
        shutdown,
        handle: Some(handle),
    }
}

pub fn read_test_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut request = [0_u8; 4096];
    let bytes = stream.read(&mut request).unwrap_or_default();
    String::from_utf8_lossy(&request[..bytes]).into_owned()
}

pub fn is_anthropic_count_tokens_request(request: &str) -> bool {
    request.starts_with("POST /v1/messages/count_tokens ")
}

pub fn write_anthropic_count_tokens_response(stream: &mut std::net::TcpStream) {
    let body = r#"{"input_tokens":42}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .expect("write Anthropic count-tokens response");
    let _ = stream.flush();
}
