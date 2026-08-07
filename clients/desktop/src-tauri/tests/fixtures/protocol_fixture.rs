use std::io::{BufRead, Write};
use std::time::Duration;

use orbcode_app_server_client::{
    ClientMessage, InitializeResult, ResponseResult, ServerCapabilities, ServerInfo, ServerMessage,
    ServerNotificationEnvelope, ServerRequestEnvelope, ServerResponseEnvelope,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("desktop protocol fixture failed: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| format!("read request: {error}"))?;
        let message: ClientMessage =
            serde_json::from_str(&line).map_err(|error| format!("decode request: {error}"))?;
        match message {
            ClientMessage::Request(request) => match request.method.as_str() {
                "initialize" => write_success(
                    &mut stdout,
                    request.id,
                    Some(
                        serde_json::to_value(InitializeResult {
                            protocol_version: "1.0".into(),
                            server_info: ServerInfo {
                                name: "orbcode-desktop-protocol-fixture".into(),
                                version: env!("CARGO_PKG_VERSION").into(),
                            },
                            capabilities: ServerCapabilities {
                                streaming: true,
                                stable_methods: vec!["initialize".into(), "test/notify".into()],
                                experimental_methods: Vec::new(),
                                server_notification_methods: vec!["test/event".into()],
                                server_request_methods: vec!["test/request".into()],
                            },
                        })
                        .map_err(|error| format!("encode initialize result: {error}"))?,
                    ),
                )?,
                "test/notify" => {
                    write_message(
                        &mut stdout,
                        &ServerMessage::Notification(ServerNotificationEnvelope {
                            method: "test/event".into(),
                            params: serde_json::json!({ "source": "fixture" }),
                        }),
                    )?;
                    write_success(
                        &mut stdout,
                        request.id,
                        Some(serde_json::json!({ "ok": true })),
                    )?;
                }
                "test/server-request" => {
                    write_message(
                        &mut stdout,
                        &ServerMessage::Request(ServerRequestEnvelope {
                            id: "fixture-server-request".into(),
                            method: "test/request".into(),
                            params: serde_json::json!({ "question": "fixture" }),
                        }),
                    )?;
                    write_success(
                        &mut stdout,
                        request.id,
                        Some(serde_json::json!({ "ok": true })),
                    )?;
                }
                "test/slow" => {
                    std::thread::sleep(Duration::from_millis(500));
                    write_success(
                        &mut stdout,
                        request.id,
                        Some(serde_json::json!({ "slow": true })),
                    )?;
                }
                "test/pending" => {}
                "test/crash" => std::process::exit(17),
                _ => write_success(&mut stdout, request.id, request.params)?,
            },
            ClientMessage::Response(_) => {}
            _ => return Err("unsupported client message".into()),
        }
    }
    Ok(())
}

fn write_success(
    writer: &mut impl Write,
    id: String,
    data: Option<serde_json::Value>,
) -> Result<(), String> {
    write_message(
        writer,
        &ServerMessage::Response(ServerResponseEnvelope {
            id,
            result: ResponseResult::Success { data },
        }),
    )
}

fn write_message(writer: &mut impl Write, message: &ServerMessage) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, message)
        .map_err(|error| format!("encode response: {error}"))?;
    writer
        .write_all(b"\n")
        .and_then(|_| writer.flush())
        .map_err(|error| format!("write response: {error}"))
}
