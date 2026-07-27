use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use orbcode_config::AppConfigOverrides;
use orbcode_protocol::StreamEvent;
use orbcode_tools::{
    BackgroundTaskStatus, cancel_background_task, read_background_task_record,
    register_background_task_cancel_flag, unregister_background_task_cancel_flag,
};
use tokio::time::sleep;

use super::support::test_manager_with_overrides;

fn set_anthropic_server_env(manager: &mut super::SessionManager, base_url: String) {
    manager
        .config
        .settings
        .env
        .insert("ANTHROPIC_BASE_URL".to_string(), base_url);
    manager
        .config
        .settings
        .env
        .insert("ANTHROPIC_API_KEY".to_string(), "test-api-key".to_string());
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut reader = BufReader::new(&*stream);
    let mut request = String::new();
    let mut content_length = 0_usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            _ => {}
        }
        if line.to_ascii_lowercase().starts_with("content-length:") {
            content_length = line
                .split(':')
                .nth(1)
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
        }
        let end_of_headers = line == "\r\n";
        request.push_str(&line);
        if end_of_headers {
            break;
        }
    }
    if content_length > 0 {
        let mut body = vec![0_u8; content_length];
        if reader.read_exact(&mut body).is_ok() {
            request.push_str(&String::from_utf8_lossy(&body));
        }
    }
    request
}

fn is_count_tokens_request(request: &str) -> bool {
    request.starts_with("POST /v1/messages/count_tokens ")
}

fn write_count_tokens_response(stream: &mut std::net::TcpStream) {
    let body = r#"{"input_tokens":42}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    stream
        .write_all(response.as_bytes())
        .expect("write count-tokens");
    let _ = stream.flush();
}

fn write_agent_tool_use_response(stream: &mut std::net::TcpStream) {
    let input = r#"{"prompt":"bg task","description":"bg test agent","run_in_background":true,"subagent_type":"general-purpose"}"#;
    let escaped_input = serde_json::to_string(input).expect("escape input json");
    let body = format!(
        concat!(
            "event: message_start\n",
            "data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":2,\"output_tokens\":0}}}}}}\n\n",
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"toolu_bg_agent\",\"name\":\"Agent\",\"input\":{{}}}}}}\n\n",
            "event: content_block_delta\n",
            "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":{}}}}}\n\n",
            "event: content_block_stop\n",
            "data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n",
            "event: message_delta\n",
            "data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}},\"usage\":{{\"output_tokens\":5}}}}\n\n",
            "event: message_stop\n",
            "data: {{\"type\":\"message_stop\"}}\n\n",
        ),
        escaped_input,
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    stream
        .write_all(response.as_bytes())
        .expect("write agent tool_use");
    let _ = stream.flush();
}

fn write_bash_tool_use_response(stream: &mut std::net::TcpStream) {
    let input = r#"{"command":"sleep 30"}"#;
    let escaped_input = serde_json::to_string(input).expect("escape bash input");
    let body = format!(
        concat!(
            "event: message_start\n",
            "data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":2,\"output_tokens\":0}}}}}}\n\n",
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"toolu_bg_bash\",\"name\":\"bash\",\"input\":{{}}}}}}\n\n",
            "event: content_block_delta\n",
            "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":{}}}}}\n\n",
            "event: content_block_stop\n",
            "data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n",
            "event: message_delta\n",
            "data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}},\"usage\":{{\"output_tokens\":3}}}}\n\n",
            "event: message_stop\n",
            "data: {{\"type\":\"message_stop\"}}\n\n",
        ),
        escaped_input,
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    stream
        .write_all(response.as_bytes())
        .expect("write bash tool_use");
    let _ = stream.flush();
}

fn write_text_response(stream: &mut std::net::TcpStream, text: &str) {
    let escaped = serde_json::to_string(text).expect("escape text");
    let body = format!(
        concat!(
            "event: message_start\n",
            "data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":2,\"output_tokens\":0}}}}}}\n\n",
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n",
            "event: content_block_delta\n",
            "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{}}}}}\n\n",
            "event: message_delta\n",
            "data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":1}}}}\n\n",
            "event: message_stop\n",
            "data: {{\"type\":\"message_stop\"}}\n\n",
        ),
        escaped,
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    stream
        .write_all(response.as_bytes())
        .expect("write text response");
    let _ = stream.flush();
}

fn write_hanging_sse(stream: &mut std::net::TcpStream) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
        )
        .expect("write hanging headers");
    let keepalive = b": keepalive\n\n";
    let chunk_prefix = format!("{:X}\r\n", keepalive.len());
    stream
        .write_all(chunk_prefix.as_bytes())
        .expect("write chunk prefix");
    stream.write_all(keepalive).expect("write keepalive");
    stream.write_all(b"\r\n").expect("write chunk suffix");
    let _ = stream.flush();
}

/// Server that returns an Agent tool_use (with run_in_background) for the
/// parent turn, then hangs on the child agent's streaming request.
///
/// Dispatches responses by inspecting request body content (not a counter)
/// to avoid races between the parent continuation and the child agent's
/// first request under single-threaded tokio runtimes.
fn start_agent_background_then_hang_server()
-> (String, std::sync::mpsc::Sender<()>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind bg-cancel test server");
    listener.set_nonblocking(true).expect("set nonblocking");
    let address = listener.local_addr().expect("server addr");
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();

    let handle = thread::spawn(move || {
        let mut parent_agent_tool_sent = false;
        let mut _hanging: Vec<std::net::TcpStream> = Vec::new();
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));
                    let request = read_http_request(&mut stream);
                    if is_count_tokens_request(&request) {
                        write_count_tokens_response(&mut stream);
                        continue;
                    }
                    let is_parent_request = request.contains("spawn a background agent");
                    if is_parent_request && !parent_agent_tool_sent {
                        parent_agent_tool_sent = true;
                        write_agent_tool_use_response(&mut stream);
                    } else if is_parent_request {
                        write_text_response(&mut stream, "background agent launched");
                    } else {
                        write_hanging_sse(&mut stream);
                        _hanging.push(stream);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if shutdown_rx.try_recv().is_ok() {
                        return;
                    }
                    thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });

    (format!("http://{address}"), shutdown_tx, handle)
}

/// Server that returns an Agent tool_use for the parent, a text response
/// for the parent's continuation, a bash tool_use (sleep 30) for the child
/// agent's first request, then hangs on the child's next request.
///
/// Like the sibling above, dispatches by request content to avoid
/// counter-based ordering races.
fn start_agent_background_with_bash_then_hang_server()
-> (String, std::sync::mpsc::Sender<()>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind bg-bash-cancel test server");
    listener.set_nonblocking(true).expect("set nonblocking");
    let address = listener.local_addr().expect("server addr");
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();

    let handle = thread::spawn(move || {
        let mut parent_agent_tool_sent = false;
        let mut child_messages_requests = 0_usize;
        let mut _hanging: Vec<std::net::TcpStream> = Vec::new();
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));
                    let request = read_http_request(&mut stream);
                    if is_count_tokens_request(&request) {
                        write_count_tokens_response(&mut stream);
                        continue;
                    }
                    let is_parent_request = request.contains("spawn agent with tool");
                    if is_parent_request && !parent_agent_tool_sent {
                        parent_agent_tool_sent = true;
                        write_agent_tool_use_response(&mut stream);
                    } else if is_parent_request {
                        write_text_response(&mut stream, "background agent launched");
                    } else {
                        child_messages_requests += 1;
                        if child_messages_requests == 1 {
                            write_bash_tool_use_response(&mut stream);
                        } else {
                            write_hanging_sse(&mut stream);
                            _hanging.push(stream);
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if shutdown_rx.try_recv().is_ok() {
                        return;
                    }
                    thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });

    (format!("http://{address}"), shutdown_tx, handle)
}

fn extract_background_task_id(events: &[StreamEvent]) -> Option<String> {
    for event in events {
        if let StreamEvent::UserMessage { message } = event {
            for block in &message.blocks {
                if let orbcode_protocol::TranscriptBlock::ToolResult {
                    content, metadata, ..
                } = block
                    && content.contains("Background subagent started")
                    && let Some(meta_str) = metadata
                    && let Ok(meta) = serde_json::from_str::<serde_json::Value>(meta_str)
                {
                    return meta["task_id"].as_str().map(String::from);
                }
            }
        }
    }
    None
}

#[tokio::test]
async fn cancel_interrupts_streaming_background_agent() {
    let (base_url, shutdown_tx, server_handle) = start_agent_background_then_hang_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        allow_network: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    set_anthropic_server_env(&mut manager, base_url);

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let home_dir = manager.config().home_dir.clone();

    let mut rx = manager
        .submit_turn(&session_id, "spawn a background agent")
        .await
        .expect("submit turn");

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        let is_finished = matches!(event, StreamEvent::TurnFinished { .. });
        events.push(event);
        if is_finished {
            break;
        }
    }

    let task_id = extract_background_task_id(&events)
        .expect("background agent task_id must appear in events");
    assert!(task_id.starts_with("agent-"), "task_id format: {task_id}");

    let signalled = cancel_background_task(&task_id);
    assert!(
        signalled,
        "cancel_background_task must return true for in-process agent"
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let final_record = loop {
        let record = read_background_task_record(&home_dir, &task_id)
            .await
            .expect("read record")
            .expect("record present");
        if !record.status.is_active() {
            break record;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "background agent did not cancel: status={:?}",
                record.status
            );
        }
        sleep(Duration::from_millis(25)).await;
    };

    assert_eq!(
        final_record.status,
        BackgroundTaskStatus::Cancelled,
        "background agent streaming during cancel must reach Cancelled status"
    );

    let _ = shutdown_tx.send(());
    let _ = server_handle.join();
}

#[tokio::test]
async fn cancel_interrupts_tool_execution_in_background_agent() {
    let (base_url, shutdown_tx, server_handle) =
        start_agent_background_with_bash_then_hang_server();
    let mut manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        allow_network: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    set_anthropic_server_env(&mut manager, base_url);

    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let home_dir = manager.config().home_dir.clone();

    let mut rx = manager
        .submit_turn(&session_id, "spawn agent with tool")
        .await
        .expect("submit turn");

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        let is_finished = matches!(event, StreamEvent::TurnFinished { .. });
        events.push(event);
        if is_finished {
            break;
        }
    }

    let task_id = extract_background_task_id(&events)
        .expect("background agent task_id must appear in events");

    // Wait briefly for the bash tool to start executing sleep 30.
    sleep(Duration::from_millis(200)).await;

    let signalled = cancel_background_task(&task_id);
    assert!(
        signalled,
        "cancel_background_task must return true while tool is executing"
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let final_record = loop {
        let record = read_background_task_record(&home_dir, &task_id)
            .await
            .expect("read record")
            .expect("record present");
        if !record.status.is_active() {
            break record;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "background agent did not cancel during tool exec: status={:?}",
                record.status
            );
        }
        sleep(Duration::from_millis(25)).await;
    };

    assert_eq!(
        final_record.status,
        BackgroundTaskStatus::Cancelled,
        "background agent with running tool must reach Cancelled status on cancel"
    );

    let _ = shutdown_tx.send(());
    let _ = server_handle.join();
}

#[tokio::test]
async fn cancel_of_completed_background_agent_is_noop() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        allow_network: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();
    let home_dir = manager.config().home_dir.clone();

    let mut rx = manager
        .submit_turn(
            &session_id,
            r#"#tool:Agent {"description":"Quick task","prompt":"summarize the workspace","subagent_type":"general-purpose","run_in_background":true}"#,
        )
        .await
        .expect("submit turn");

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        let is_finished = matches!(event, StreamEvent::TurnFinished { .. });
        events.push(event);
        if is_finished {
            break;
        }
    }

    let task_id = extract_background_task_id(&events)
        .expect("background agent task_id must appear in events");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let record = read_background_task_record(&home_dir, &task_id)
            .await
            .expect("read record")
            .expect("record present");
        if record.status == BackgroundTaskStatus::Completed {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "background agent did not complete: status={:?}",
                record.status
            );
        }
        sleep(Duration::from_millis(25)).await;
    }

    let signalled = cancel_background_task(&task_id);
    assert!(
        !signalled,
        "cancel_background_task must return false for completed agent (flag unregistered)"
    );

    let record = read_background_task_record(&home_dir, &task_id)
        .await
        .expect("read record after cancel")
        .expect("record still present");
    assert_eq!(
        record.status,
        BackgroundTaskStatus::Completed,
        "completed record must remain Completed after cancel attempt"
    );
}

#[tokio::test]
async fn concurrent_background_agents_cancel_independently() {
    let id_a = format!(
        "agent-cancel-iso-a-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let id_b = format!(
        "agent-cancel-iso-b-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let flag_a = Arc::new(AtomicBool::new(false));
    let flag_b = Arc::new(AtomicBool::new(false));

    register_background_task_cancel_flag(&id_a, flag_a.clone());
    register_background_task_cancel_flag(&id_b, flag_b.clone());

    assert!(cancel_background_task(&id_a), "cancel A must return true");
    assert!(flag_a.load(Ordering::SeqCst), "flag A must be set");
    assert!(
        !flag_b.load(Ordering::SeqCst),
        "flag B must NOT be affected by cancelling A"
    );

    assert!(cancel_background_task(&id_b), "cancel B must return true");
    assert!(flag_b.load(Ordering::SeqCst), "flag B must now be set");

    unregister_background_task_cancel_flag(&id_a);
    unregister_background_task_cancel_flag(&id_b);

    assert!(
        !cancel_background_task(&id_a),
        "cancel A after unregister must return false"
    );
}

#[tokio::test]
async fn two_stub_background_agents_complete_independently() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        allow_network: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let home_dir = manager.config().home_dir.clone();

    let (session_a, _) = manager
        .start_or_resume(None)
        .await
        .expect("create session A");
    let session_id_a = session_a.session_id.clone();
    let mut rx1 = manager
        .submit_turn(
            &session_id_a,
            r#"#tool:Agent {"description":"Agent A","prompt":"summarize A","subagent_type":"general-purpose","run_in_background":true}"#,
        )
        .await
        .expect("submit turn 1");
    let mut events1 = Vec::new();
    while let Some(event) = rx1.recv().await {
        let is_finished = matches!(event, StreamEvent::TurnFinished { .. });
        events1.push(event);
        if is_finished {
            break;
        }
    }
    let task_a = extract_background_task_id(&events1).expect("task A id");

    let (session_b, _) = manager
        .start_or_resume(None)
        .await
        .expect("create session B");
    let session_id_b = session_b.session_id.clone();
    let mut rx2 = manager
        .submit_turn(
            &session_id_b,
            r#"#tool:Agent {"description":"Agent B","prompt":"summarize B","subagent_type":"general-purpose","run_in_background":true}"#,
        )
        .await
        .expect("submit turn 2");
    let mut events2 = Vec::new();
    while let Some(event) = rx2.recv().await {
        let is_finished = matches!(event, StreamEvent::TurnFinished { .. });
        events2.push(event);
        if is_finished {
            break;
        }
    }
    let task_b = extract_background_task_id(&events2).expect("task B id");

    assert_ne!(task_a, task_b, "two agents must have distinct task ids");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ra = read_background_task_record(&home_dir, &task_a)
            .await
            .expect("read A")
            .expect("A present");
        let rb = read_background_task_record(&home_dir, &task_b)
            .await
            .expect("read B")
            .expect("B present");
        if ra.status == BackgroundTaskStatus::Completed
            && rb.status == BackgroundTaskStatus::Completed
        {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "agents did not both complete: A={:?}, B={:?}",
                ra.status, rb.status
            );
        }
        sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn background_agent_over_cap_emits_execution_failed_terminal() {
    use orbcode_protocol::ToolUseCompletionKind;

    // When the concurrency cap is hit the spawn appends an error tool_result;
    // the terminal completion must be ExecutionFailed, not the contradictory
    // Success that stream-json / TUI / ACP consumers previously saw.
    let manager = test_manager_with_overrides(AppConfigOverrides::default()).await;
    let (session, _) = manager.start_or_resume(None).await.expect("create session");
    let session_id = session.session_id.clone();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    manager
        .reject_background_agent_over_cap(&session_id, "tool-use-cap", &tx)
        .await
        .expect("cap rejection returns Continue");
    drop(tx);

    let mut completion_kind = None;
    while let Some(event) = rx.recv().await {
        if let StreamEvent::ToolUseCompleted {
            tool_name, kind, ..
        } = event
            && tool_name == "Agent"
        {
            completion_kind = Some(kind);
        }
    }
    assert_eq!(
        completion_kind,
        Some(ToolUseCompletionKind::ExecutionFailed),
        "cap rejection must emit an ExecutionFailed terminal (not Success)"
    );
}
