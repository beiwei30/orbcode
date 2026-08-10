use std::io::{self, BufRead, Write};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

fn main() {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "normal".to_string());

    if mode == "early-exit" {
        eprintln!("fixture exited before reading stdin");
        std::process::exit(23);
    }

    if mode == "stderr" {
        eprintln!("{}", "bounded-diagnostic-".repeat(128));
        eprintln!("OPENAI_API_KEY=fixture-api-key");
        eprintln!("Authorization: Bearer fixture-bearer");
        eprintln!("custom=fixture-prompt-secret");
        eprintln!("url=https://example.invalid/callback?token=fixture-query-secret");
    }

    #[cfg(unix)]
    if mode == "broken-stdin" {
        // SAFETY: this disposable fixture intentionally closes only its own
        // inherited stdin descriptor to make the parent's next write fail.
        let result = unsafe { libc::close(libc::STDIN_FILENO) };
        assert_eq!(result, 0, "close fixture stdin");
        let mut stdout = io::stdout().lock();
        write_message(
            &mut stdout,
            json!({"type": "notification", "method": "fixture/ready", "params": {}}),
        );
        thread::sleep(Duration::from_secs(30));
        return;
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut deferred_request_id = None;
    for line in stdin.lock().lines() {
        let line = line.expect("read stdin");
        let message: Value = serde_json::from_str(&line).expect("valid client message");
        match message.get("type").and_then(Value::as_str) {
            Some("request") => {
                let id = message["id"].as_str().expect("request id");
                let method = message["method"].as_str().expect("request method");
                match mode.as_str() {
                    "malformed" => {
                        writeln!(stdout, "{{not-json}}").expect("write malformed line");
                        stdout.flush().expect("flush malformed line");
                        continue;
                    }
                    "oversized" => {
                        writeln!(stdout, "{}", "x".repeat(4096)).expect("write oversized line");
                        stdout.flush().expect("flush oversized line");
                        continue;
                    }
                    _ => {}
                }

                if method == "initialize" {
                    respond(
                        &mut stdout,
                        id,
                        json!({
                            "protocol_version": "1.0",
                            "server_info": {"name": "child-stdio-fixture", "version": "0.1.0"},
                            "capabilities": {
                                "streaming": true,
                                "stable_methods": ["fixture/echo", "fixture/ordered", "fixture/server-request", "fixture/notification-backpressure"],
                                "experimental_methods": [],
                                "server_notification_methods": ["fixture/notification"],
                                "server_request_methods": ["fixture/question"]
                            }
                        }),
                    );
                } else if method == "fixture/ordered" {
                    write_message(
                        &mut stdout,
                        json!({"type": "notification", "method": "fixture/notification", "params": {"order": 1}}),
                    );
                    write_message(
                        &mut stdout,
                        json!({"type": "notification", "method": "fixture/notification", "params": {"order": 2}}),
                    );
                    respond(&mut stdout, id, json!({"order": 3}));
                } else if method == "fixture/server-request" {
                    deferred_request_id = Some(id.to_string());
                    write_message(
                        &mut stdout,
                        json!({
                            "type": "request",
                            "id": "fixture-server-request-1",
                            "method": "fixture/question",
                            "params": {"question": "continue?"}
                        }),
                    );
                } else if method == "fixture/notification-backpressure" {
                    for index in 0..300 {
                        write_message(
                            &mut stdout,
                            json!({
                                "type": "notification",
                                "method": "stream/event",
                                "params": {
                                    "subscription_id": "fixture-backpressure",
                                    "event": {
                                        "event": "assistant_delta",
                                        "session_id": "fixture-session",
                                        "delta": format!("chunk-{index}")
                                    }
                                }
                            }),
                        );
                    }
                    // Put the response ahead of the durable notification so
                    // the test can begin draining a saturated notification
                    // receiver without coupling response delivery to it.
                    respond(&mut stdout, id, json!({"notifications_sent": 300}));
                    write_message(
                        &mut stdout,
                        json!({
                            "type": "notification",
                            "method": "stream/event",
                            "params": {
                                "subscription_id": "fixture-backpressure",
                                "event": {
                                    "event": "turn_finished",
                                    "session_id": "fixture-session",
                                    "provider": "anthropic",
                                    "usage": {}
                                }
                            }
                        }),
                    );
                } else {
                    respond(
                        &mut stdout,
                        id,
                        message.get("params").cloned().unwrap_or(Value::Null),
                    );
                }
            }
            Some("response") => {
                assert_eq!(message["id"], "fixture-server-request-1");
                let request_id = deferred_request_id
                    .take()
                    .expect("server response follows fixture/server-request");
                respond(
                    &mut stdout,
                    &request_id,
                    json!({"client_result": message["result"].clone()}),
                );
            }
            other => panic!("unexpected client message type: {other:?}"),
        }
    }

    if mode == "slow-shutdown" {
        thread::sleep(Duration::from_secs(30));
    }
}

fn respond(stdout: &mut impl Write, id: &str, data: Value) {
    write_message(
        stdout,
        json!({"type": "response", "id": id, "result": {"status": "success", "data": data}}),
    );
}

fn write_message(stdout: &mut impl Write, message: Value) {
    serde_json::to_writer(&mut *stdout, &message).expect("serialize server message");
    writeln!(stdout).expect("terminate server message");
    stdout.flush().expect("flush server message");
}
