use std::path::PathBuf;

use orbcode_app_server_protocol::{
    ClientCapabilities, ClientInfo, ClientMessage, ClientRequestEnvelope, InitializeParams,
    InitializeResult, ResponseResult, ServerMessage,
};
use orbcode_desktop_spike::{ProbeChild, ProbeResult, ProbeTermination, configure_builder};
use tauri::test::MockRuntime;
use tauri::webview::{InvokeRequest, WebviewWindowBuilder};

fn build_test_app(child: ProbeChild) -> tauri::App<MockRuntime> {
    configure_builder(tauri::test::mock_builder(), child)
        .build(tauri::generate_context!())
        .expect("build mock Tauri app")
}

#[test]
fn canonical_initialize_crosses_ipc_and_child_is_reaped() {
    let child_path = PathBuf::from(env!("CARGO_BIN_EXE_orbcode-desktop-spike"));
    let app = build_test_app(ProbeChild::new(child_path));
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build mock main WebView");

    let initialize = ClientMessage::Request(ClientRequestEnvelope {
        id: "desktop-spike-init-1".to_string(),
        method: "initialize".to_string(),
        params: Some(
            serde_json::to_value(InitializeParams {
                protocol_version: "1.0".to_string(),
                client_info: ClientInfo {
                    name: "desktop-spike-test".to_string(),
                    version: "0.0.0".to_string(),
                },
                capabilities: ClientCapabilities {
                    streaming: true,
                    ..ClientCapabilities::default()
                },
            })
            .expect("serialize initialize params"),
        ),
    });
    let request = serde_json::to_string(&initialize).expect("serialize initialize envelope");

    let response = tauri::test::get_ipc_response(
        &webview,
        InvokeRequest {
            cmd: "run_initialize_probe".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().expect("parse packaged origin"),
            body: tauri::ipc::InvokeBody::Json(serde_json::json!({ "request": request })),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .expect("invoke fixed child probe through Tauri IPC")
    .deserialize::<ProbeResult>()
    .expect("deserialize probe result");

    assert_eq!(response.termination, ProbeTermination::Graceful);
    assert_eq!(response.exit_code, Some(0));
    assert!(response.stderr_tail.is_empty());

    let message: ServerMessage =
        serde_json::from_str(&response.response).expect("decode canonical server response");
    let ServerMessage::Response(envelope) = message else {
        panic!("expected canonical response envelope");
    };
    assert_eq!(envelope.id, "desktop-spike-init-1");
    let ResponseResult::Success { data: Some(data) } = envelope.result else {
        panic!("expected successful initialize result");
    };
    let initialized: InitializeResult =
        serde_json::from_value(data).expect("decode canonical initialize result");
    assert_eq!(initialized.protocol_version, "1.0");
    assert_eq!(initialized.server_info.name, "orbcode-desktop-spike-child");
}

#[test]
fn ipc_rejects_multiple_ndjson_records_before_spawning() {
    let missing_child = ProbeChild::new("/path/that/must/not/be-spawned");
    let app = build_test_app(missing_child);
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build mock main WebView");

    let response = tauri::test::get_ipc_response(
        &webview,
        InvokeRequest {
            cmd: "run_initialize_probe".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().expect("parse packaged origin"),
            body: tauri::ipc::InvokeBody::Json(serde_json::json!({ "request": "{}\n{}" })),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    );

    let error = response.expect_err("multiple records must be rejected");
    assert!(error.to_string().contains("exactly one NDJSON record"));
}
