use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use orbcode_app_server_client::{
    ClientCapabilities, ClientInfo, ClientMessage, ClientRequestEnvelope, InitializeParams,
    InitializeResult, ResponseResult, ServerMessage,
};
use orbcode_desktop::{
    BinarySource, ConnectionKind, ConnectionStatus, DesktopHostPolicy, HostExitDiagnostics,
    ProtocolReply, configure_builder, shutdown_children,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tauri::Listener;
use tauri::test::MockRuntime;
use tauri::webview::{InvokeRequest, WebviewWindowBuilder};

const PROTOCOL_EVENT: &str = "orbcode://protocol";
const CONNECTION_EXIT_EVENT: &str = "orbcode://connection-exit";

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_orbcode-desktop-protocol-fixture"))
}

fn build_test_app() -> tauri::App<MockRuntime> {
    let fixture = fixture_path();
    let mut app = configure_builder(
        tauri::test::mock_builder(),
        DesktopHostPolicy::test_harness(&fixture, &fixture),
    )
    .build(tauri::test::mock_context(tauri::test::noop_assets()))
    .expect("build mock desktop host");
    // Tauri runs setup on the first event-loop iteration. The mock runtime's
    // single iteration does not busy-loop, which is the deprecation concern.
    #[allow(deprecated)]
    app.run_iteration(|_, _| {});
    app
}

fn build_webview(app: &tauri::App<MockRuntime>) -> tauri::WebviewWindow<MockRuntime> {
    WebviewWindowBuilder::new(app, "main", Default::default())
        .build()
        .expect("build mock main WebView")
}

fn invoke<T: DeserializeOwned>(
    webview: &tauri::WebviewWindow<MockRuntime>,
    command: &str,
    body: Value,
) -> Result<T, String> {
    tauri::test::get_ipc_response(
        webview,
        InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().expect("parse packaged origin"),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .map_err(|error| error.to_string())?
    .deserialize::<T>()
    .map_err(|error| error.to_string())
}

fn initialize_message(id: &str) -> ClientMessage {
    ClientMessage::Request(ClientRequestEnvelope {
        id: id.into(),
        method: "initialize".into(),
        params: Some(
            serde_json::to_value(InitializeParams {
                protocol_version: "1.0".into(),
                client_info: ClientInfo {
                    name: "desktop-host-ipc-test".into(),
                    version: "0.0.0".into(),
                },
                capabilities: ClientCapabilities {
                    streaming: true,
                    ..ClientCapabilities::default()
                },
            })
            .expect("encode initialize params"),
        ),
    })
}

#[derive(Deserialize)]
struct ObservedProtocolEvent {
    generation: u64,
    message: ServerMessage,
}

#[derive(Deserialize)]
struct ObservedConnectionExitEvent {
    generation: u64,
    diagnostics: HostExitDiagnostics,
}

#[test]
fn unexpected_child_exit_emits_sanitized_generation_diagnostics() {
    let app = build_test_app();
    let webview = build_webview(&app);
    let cwd = std::env::current_dir().expect("current directory");
    let (event_tx, event_rx) = mpsc::sync_channel(1);
    app.listen(CONNECTION_EXIT_EVENT, move |event| {
        let _ = event_tx.send(event.payload().to_string());
    });

    let connected: ConnectionStatus = invoke(
        &webview,
        "connect_local",
        json!({ "input": { "cwd": cwd } }),
    )
    .expect("connect crash fixture");
    let pid = connected.child_pid.expect("child pid");
    let crashed = invoke::<ProtocolReply>(
        &webview,
        "protocol_send",
        json!({
            "generation": connected.generation,
            "message": ClientMessage::Request(ClientRequestEnvelope {
                id: "crash-child".into(),
                method: "test/crash".into(),
                params: None,
            })
        }),
    )
    .expect_err("crashed child cannot answer request");
    assert!(crashed.contains("cancelled") || crashed.contains("closed"));

    let payload = event_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receive connection-exit event");
    let event: ObservedConnectionExitEvent =
        serde_json::from_str(&payload).expect("decode connection-exit event");
    assert_eq!(event.generation, connected.generation);
    assert_eq!(event.diagnostics.pid, pid);
    assert_eq!(event.diagnostics.exit_code, Some(17));
    assert!(!event.diagnostics.success);
    assert_process_reaped(pid);

    let _: Option<HostExitDiagnostics> = invoke(
        &webview,
        "disconnect",
        json!({ "generation": connected.generation }),
    )
    .expect("cleanup exited child record");
}

#[test]
fn local_ipc_reconnect_preserves_protocol_and_rejects_stale_generation() {
    let app = build_test_app();
    let webview = build_webview(&app);
    let cwd = std::env::current_dir().expect("current directory");

    let first: ConnectionStatus = invoke(
        &webview,
        "connect_local",
        json!({ "input": { "cwd": cwd } }),
    )
    .expect("connect first local child");
    assert!(first.active);
    assert_eq!(first.generation, 1);
    assert_eq!(first.binary_source, Some(BinarySource::Bundled));
    let first_pid = first.child_pid.expect("first child PID");

    let initialized: ProtocolReply = invoke(
        &webview,
        "protocol_send",
        json!({
            "generation": first.generation,
            "message": initialize_message("renderer-initialize-1")
        }),
    )
    .expect("relay initialize through IPC");
    assert_eq!(initialized.generation, first.generation);
    let Some(ServerMessage::Response(response)) = initialized.message else {
        panic!("expected canonical initialize response");
    };
    assert_eq!(response.id, "renderer-initialize-1");
    let ResponseResult::Success { data: Some(data) } = response.result else {
        panic!("expected successful initialize response");
    };
    let result: InitializeResult = serde_json::from_value(data).expect("decode initialize result");
    assert_eq!(result.server_info.name, "orbcode-desktop-protocol-fixture");

    let (event_tx, event_rx) = mpsc::sync_channel(1);
    app.listen(PROTOCOL_EVENT, move |event| {
        let _ = event_tx.send(event.payload().to_string());
    });
    let notified: ProtocolReply = invoke(
        &webview,
        "protocol_send",
        json!({
            "generation": first.generation,
            "message": ClientMessage::Request(ClientRequestEnvelope {
                id: "renderer-notify-1".into(),
                method: "test/notify".into(),
                params: None,
            })
        }),
    )
    .expect("request fixture notification");
    assert!(matches!(notified.message, Some(ServerMessage::Response(_))));
    let payload = event_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receive desktop protocol event");
    let event: ObservedProtocolEvent =
        serde_json::from_str(&payload).expect("decode protocol event");
    assert_eq!(event.generation, first.generation);
    assert!(matches!(event.message, ServerMessage::Notification(_)));

    let requested: ProtocolReply = invoke(
        &webview,
        "protocol_send",
        json!({
            "generation": first.generation,
            "message": ClientMessage::Request(ClientRequestEnvelope {
                id: "renderer-server-request-1".into(),
                method: "test/server-request".into(),
                params: None,
            })
        }),
    )
    .expect("request fixture server-request");
    assert!(matches!(
        requested.message,
        Some(ServerMessage::Response(_))
    ));
    let payload = event_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receive desktop server-request event");
    let event: ObservedProtocolEvent =
        serde_json::from_str(&payload).expect("decode server-request event");
    assert_eq!(event.generation, first.generation);
    assert!(matches!(event.message, ServerMessage::Request(_)));

    let second: ConnectionStatus = invoke(
        &webview,
        "connect_local",
        json!({ "input": { "cwd": cwd } }),
    )
    .expect("replace local child");
    assert_eq!(second.generation, 2);
    assert_ne!(second.child_pid, Some(first_pid));
    assert_process_reaped(first_pid);

    let stale = invoke::<ProtocolReply>(
        &webview,
        "protocol_send",
        json!({
            "generation": first.generation,
            "message": initialize_message("stale-renderer-request")
        }),
    )
    .expect_err("stale generation must not reach replacement child");
    assert!(stale.contains("stale desktop connection generation"));

    let diagnostics: Option<HostExitDiagnostics> = invoke(
        &webview,
        "disconnect",
        json!({ "generation": second.generation }),
    )
    .expect("disconnect replacement child");
    let diagnostics = diagnostics.expect("replacement diagnostics");
    assert_eq!(diagnostics.pid, second.child_pid.expect("second child PID"));
    assert_process_reaped(diagnostics.pid);

    let inactive: ConnectionStatus =
        invoke(&webview, "connection_status", json!({})).expect("read inactive status");
    assert!(!inactive.active);
    assert_eq!(inactive.generation, second.generation);
}

#[test]
fn ssh_ipc_uses_shared_relay_and_shutdown_hook_reaps_child() {
    let app = build_test_app();
    let webview = build_webview(&app);

    let connected: ConnectionStatus = invoke(
        &webview,
        "connect_ssh",
        json!({
            "input": {
                "target": "fixture.example",
                "remote_cwd": "/workspace",
                "remote_orbcode_path": "/opt/orbcode",
                "options": ["Port=2222", "IdentitiesOnly=yes"]
            }
        }),
    )
    .expect("connect fake SSH child");
    assert_eq!(connected.kind, Some(ConnectionKind::Ssh));
    assert!(connected.binary_source.is_none());

    let initialized: ProtocolReply = invoke(
        &webview,
        "protocol_send",
        json!({
            "generation": connected.generation,
            "message": initialize_message("ssh-initialize-1")
        }),
    )
    .expect("initialize fake SSH relay");
    assert!(matches!(
        initialized.message,
        Some(ServerMessage::Response(_))
    ));

    let pid = connected.child_pid.expect("SSH child PID");
    let diagnostics = tauri::async_runtime::block_on(shutdown_children(app.handle()))
        .expect("run app/window shutdown hook")
        .expect("SSH shutdown diagnostics");
    assert_eq!(diagnostics.pid, pid);
    assert_process_reaped(pid);

    let inactive: ConnectionStatus =
        invoke(&webview, "connection_status", json!({})).expect("read shutdown status");
    assert!(!inactive.active);
}

#[test]
fn boundary_sources_pin_thin_host_and_both_exit_paths() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("default-features = false"));
    assert!(
        !manifest
            .lines()
            .any(|line| line.trim_start().starts_with("orbcode-app-server ="))
    );
    assert!(
        !manifest
            .lines()
            .any(|line| line.trim_start().starts_with("orbcode-core ="))
    );

    let source = include_str!("../src/lib.rs");
    assert!(source.contains("tauri::WindowEvent::CloseRequested"));
    assert!(source.contains("tauri::RunEvent::ExitRequested"));
    assert!(source.matches("shutdown_children").count() >= 3);
    assert!(!source.contains("AppServer::"));
}

#[cfg(unix)]
fn assert_process_reaped(pid: u32) {
    let result = unsafe { libc::kill(pid as i32, 0) };
    assert_eq!(result, -1, "process {pid} is still present");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH),
        "process check for {pid} did not report ESRCH"
    );
}

#[cfg(not(unix))]
fn assert_process_reaped(_pid: u32) {}
