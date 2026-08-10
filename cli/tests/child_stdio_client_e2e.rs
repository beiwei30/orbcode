//! Proves the reusable child-stdio client can own the real local serve process.

use std::time::Duration;

use orbcode_app_server_client::{
    AppClient, ChildExitReason, ChildStdioTransport, ChildStdioTransportConfig, ChildTermination,
};
use tokio::process::Command;
use tokio::time::timeout;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

#[tokio::test]
async fn child_stdio_client_initializes_lists_sessions_and_reaps_local_serve() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    std::fs::write(
        home.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
    )
    .expect("write settings");

    // The host constructs the command and owns policy such as cwd/environment;
    // the reusable transport only adds and supervises the stdio pipes.
    let mut command = Command::new(ORBCODE_BIN);
    command
        .arg("serve")
        .arg("--stdio")
        .current_dir(cwd.path())
        .env_clear()
        .env("ORBCODE_HOME", home.path())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("RUST_LOG", "warn");

    let (transport, handle) =
        ChildStdioTransport::spawn(command, ChildStdioTransportConfig::default())
            .await
            .expect("spawn orbcode serve --stdio");
    let client = timeout(
        Duration::from_secs(10),
        AppClient::from_transport(Box::new(transport)),
    )
    .await
    .expect("initialize timeout")
    .expect("initialize through child transport");

    let sessions = timeout(Duration::from_secs(10), client.list_sessions())
        .await
        .expect("session/list timeout")
        .expect("session/list through child transport");
    assert!(sessions.0.is_empty());

    let diagnostics = timeout(Duration::from_secs(10), handle.shutdown())
        .await
        .expect("shutdown timeout")
        .expect("child diagnostics");
    assert_eq!(diagnostics.reason, ChildExitReason::ShutdownRequested);
    assert_eq!(diagnostics.termination, ChildTermination::Graceful);
    assert!(diagnostics.success);
    drop(client);
}
