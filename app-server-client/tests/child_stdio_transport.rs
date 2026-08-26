use std::time::Duration;

use orbcode_app_server_client::{
    AppClient, ChildExitDiagnostics, ChildExitReason, ChildStdioHandle, ChildStdioTransport,
    ChildStdioTransportConfig, ChildTermination, ClientError, ClientTransport, ResponseResult,
};
use serde_json::json;
use tokio::process::Command;
use tokio::time::{Instant, timeout};

// Fixture messages and process diagnostics prove progress. This duration is
// only the outer deadlock watchdog for a complete asynchronous operation.
const DEADLOCK_WATCHDOG: Duration = Duration::from_secs(20);
// A ready child may still need an OS scheduling turn to consume stdin EOF.
const LIFECYCLE_GRACE_TIMEOUT: Duration = Duration::from_secs(10);
// Only the explicit escalation test intentionally compresses these phases.
const ESCALATION_PHASE_TIMEOUT: Duration = Duration::from_millis(100);

fn fixture_command(mode: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_orbcode-child-stdio-fixture"));
    command.arg(mode);
    command
}

fn lifecycle_config() -> ChildStdioTransportConfig {
    let mut config = ChildStdioTransportConfig::default();
    config.graceful_shutdown_timeout = LIFECYCLE_GRACE_TIMEOUT;
    config
}

fn held_fault_config() -> ChildStdioTransportConfig {
    // This fixture deliberately cannot exit on EOF, so production defaults
    // bound its expected terminate-or-kill path after the fault is published.
    ChildStdioTransportConfig::default()
}

fn escalation_config() -> ChildStdioTransportConfig {
    let mut config = ChildStdioTransportConfig::default();
    config.graceful_shutdown_timeout = ESCALATION_PHASE_TIMEOUT;
    config.terminate_timeout = ESCALATION_PHASE_TIMEOUT;
    config
}

async fn shutdown_with_watchdog(handle: &ChildStdioHandle) -> ChildExitDiagnostics {
    timeout(DEADLOCK_WATCHDOG, handle.shutdown())
        .await
        .expect("shutdown should not hang")
        .expect("child diagnostics")
}

fn assert_graceful_shutdown(diagnostics: &ChildExitDiagnostics) {
    assert_eq!(diagnostics.reason, ChildExitReason::ShutdownRequested);
    assert_eq!(
        diagnostics.termination,
        ChildTermination::Graceful,
        "unexpected shutdown diagnostics: {diagnostics:?}"
    );
    assert_eq!(diagnostics.exit_code, Some(0));
    assert_eq!(diagnostics.signal, None);
    assert!(
        diagnostics.success,
        "unexpected shutdown diagnostics: {diagnostics:?}"
    );
}

fn assert_graceful_fault(diagnostics: &ChildExitDiagnostics, reason: ChildExitReason) {
    assert_eq!(diagnostics.reason, reason);
    assert_eq!(
        diagnostics.termination,
        ChildTermination::Graceful,
        "unexpected fault diagnostics: {diagnostics:?}"
    );
    assert_eq!(diagnostics.exit_code, Some(0));
    assert_eq!(diagnostics.signal, None);
    assert!(
        diagnostics.success,
        "unexpected fault diagnostics: {diagnostics:?}"
    );
}

#[tokio::test]
async fn canonical_initialize_works_through_app_client() {
    let (transport, handle) =
        ChildStdioTransport::spawn(fixture_command("normal"), lifecycle_config())
            .await
            .expect("spawn fixture");

    let client = timeout(
        DEADLOCK_WATCHDOG,
        AppClient::from_transport(Box::new(transport)),
    )
    .await
    .expect("initialize should not hang")
    .expect("canonical initialize should succeed");

    drop(client);
    let diagnostics = timeout(DEADLOCK_WATCHDOG, handle.wait_for_exit())
        .await
        .expect("drop should stop child")
        .expect("child diagnostics");
    assert_graceful_shutdown(&diagnostics);
}

#[tokio::test]
async fn responses_notifications_and_server_requests_keep_protocol_semantics() {
    let (transport, handle) =
        ChildStdioTransport::spawn(fixture_command("normal"), lifecycle_config())
            .await
            .expect("spawn fixture");
    let mut notifications = transport
        .take_notification_receiver()
        .await
        .expect("notification receiver");
    let mut server_requests = transport
        .take_server_request_receiver()
        .await
        .expect("server request receiver");

    let (first, second) = tokio::join!(
        transport.request("fixture/echo", Some(json!({"request": 1}))),
        transport.request("fixture/echo", Some(json!({"request": 2}))),
    );
    assert_eq!(first.expect("first response"), json!({"request": 1}));
    assert_eq!(second.expect("second response"), json!({"request": 2}));

    let ordered = timeout(
        DEADLOCK_WATCHDOG,
        transport.request("fixture/ordered", Some(json!({}))),
    )
    .await
    .expect("ordered request should not hang")
    .expect("ordered response");
    let notification_1 = notifications.recv().await.expect("first notification");
    let notification_2 = notifications.recv().await.expect("second notification");
    assert_eq!(notification_1.params, json!({"order": 1}));
    assert_eq!(notification_2.params, json!({"order": 2}));
    assert_eq!(ordered, json!({"order": 3}));

    let answer_server_request = async {
        let request = server_requests.recv().await.expect("server request");
        assert_eq!(request.id, "fixture-server-request-1");
        assert_eq!(request.method, "fixture/question");
        transport
            .respond_to_server_request(
                request.id,
                ResponseResult::Success {
                    data: Some(json!({"answer": true})),
                },
            )
            .await
            .expect("respond to server request");
    };
    let (response, ()) = tokio::join!(
        transport.request("fixture/server-request", None),
        answer_server_request,
    );
    assert_eq!(
        response.expect("response after server request"),
        json!({
            "client_result": {
                "status": "success",
                "data": {"answer": true}
            }
        })
    );

    let diagnostics = shutdown_with_watchdog(&handle).await;
    assert_graceful_shutdown(&diagnostics);
}

#[tokio::test]
async fn durable_notification_survives_saturated_best_effort_queue() {
    let (transport, handle) =
        ChildStdioTransport::spawn(fixture_command("normal"), lifecycle_config())
            .await
            .expect("spawn fixture");
    let mut notifications = transport
        .take_notification_receiver()
        .await
        .expect("notification receiver");

    let response = timeout(
        DEADLOCK_WATCHDOG,
        transport.request("fixture/notification-backpressure", None),
    )
    .await
    .expect("fixture response should arrive before the durable notification")
    .expect("fixture response");
    assert_eq!(response, json!({"notifications_sent": 300}));

    let (best_effort_count, terminal) = timeout(DEADLOCK_WATCHDOG, async {
        let mut best_effort_count = 0;
        loop {
            let notification = notifications.recv().await.expect("notification");
            match notification.params["event"]["event"].as_str() {
                Some("assistant_delta") => best_effort_count += 1,
                Some("turn_finished") => break (best_effort_count, notification),
                event => panic!("unexpected stream event: {event:?}"),
            }
        }
    })
    .await
    .expect("durable notification must not be dropped");

    assert!(
        best_effort_count < 300,
        "transient notifications should drop under bounded pressure"
    );
    assert_eq!(terminal.params["subscription_id"], "fixture-backpressure");
    assert_eq!(terminal.params["event"]["event"], "turn_finished");

    let diagnostics = shutdown_with_watchdog(&handle).await;
    assert_graceful_shutdown(&diagnostics);
}

#[tokio::test]
async fn malformed_stdout_cancels_pending_request_and_reports_reason() {
    let (transport, handle) =
        ChildStdioTransport::spawn(fixture_command("malformed"), lifecycle_config())
            .await
            .expect("spawn fixture");

    let result = timeout(DEADLOCK_WATCHDOG, transport.request("fixture/echo", None))
        .await
        .expect("malformed output should not hang");
    assert!(matches!(result, Err(ClientError::Cancelled)));

    let diagnostics = timeout(DEADLOCK_WATCHDOG, handle.wait_for_exit())
        .await
        .expect("child should be reaped")
        .expect("child diagnostics");
    assert_graceful_fault(&diagnostics, ChildExitReason::MalformedStdout);
}

#[tokio::test]
async fn oversized_stdout_is_bounded_and_cancels_pending_request() {
    let mut config = lifecycle_config();
    config.max_payload_bytes = 256;
    let (transport, handle) = ChildStdioTransport::spawn(fixture_command("oversized"), config)
        .await
        .expect("spawn fixture");

    let result = timeout(DEADLOCK_WATCHDOG, transport.request("fixture/echo", None))
        .await
        .expect("oversized output should not hang");
    assert!(matches!(result, Err(ClientError::Cancelled)));

    let diagnostics = timeout(DEADLOCK_WATCHDOG, handle.wait_for_exit())
        .await
        .expect("child should be reaped")
        .expect("child diagnostics");
    assert_graceful_fault(&diagnostics, ChildExitReason::OversizedStdout);
}

#[tokio::test]
async fn early_exit_does_not_leave_requests_pending() {
    let (transport, handle) =
        ChildStdioTransport::spawn(fixture_command("early-exit"), lifecycle_config())
            .await
            .expect("process can spawn before it exits");

    let result = timeout(DEADLOCK_WATCHDOG, transport.request("fixture/echo", None))
        .await
        .expect("request should be released after early exit");
    assert!(matches!(
        result,
        Err(ClientError::Cancelled | ClientError::Transport(_))
    ));

    let diagnostics = timeout(DEADLOCK_WATCHDOG, handle.wait_for_exit())
        .await
        .expect("child should be reaped")
        .expect("child diagnostics");
    assert_eq!(diagnostics.exit_code, Some(23));
    assert_eq!(diagnostics.signal, None);
    assert!(!diagnostics.success);
}

#[cfg(unix)]
#[tokio::test]
async fn broken_stdin_cancels_pending_request_and_reports_reason() {
    let (transport, handle) =
        ChildStdioTransport::spawn(fixture_command("broken-stdin"), held_fault_config())
            .await
            .expect("spawn fixture");
    let mut notifications = transport
        .take_notification_receiver()
        .await
        .expect("notification receiver");
    let armed = timeout(
        DEADLOCK_WATCHDOG,
        transport.request("fixture/arm-broken-stdin", None),
    )
    .await
    .expect("fixture arm request should not hang")
    .expect("fixture arm response");
    assert_eq!(armed, json!({"armed": true}));
    let ready = timeout(DEADLOCK_WATCHDOG, notifications.recv())
        .await
        .expect("fixture ready timeout")
        .expect("fixture ready notification");
    assert_eq!(ready.method, "fixture/ready");

    assert!(
        handle.diagnostics().is_none(),
        "the held fixture must still be alive after readiness"
    );
    let result = timeout(DEADLOCK_WATCHDOG, transport.request("fixture/echo", None))
        .await
        .expect("broken stdin should not hang");
    assert!(matches!(
        result,
        Err(ClientError::Cancelled | ClientError::Transport(_))
    ));

    let diagnostics = timeout(DEADLOCK_WATCHDOG, handle.wait_for_exit())
        .await
        .expect("child should be reaped")
        .expect("child diagnostics");
    assert_eq!(diagnostics.reason, ChildExitReason::StdinIo);
    assert!(matches!(
        diagnostics.termination,
        ChildTermination::Terminated | ChildTermination::Killed
    ));
    assert_eq!(diagnostics.exit_code, None);
    assert!(matches!(
        diagnostics.signal,
        Some(libc::SIGTERM) | Some(libc::SIGKILL)
    ));
    assert!(!diagnostics.success);
}

#[tokio::test]
async fn shutdown_escalates_after_eof_and_remains_bounded() {
    let (transport, handle) =
        ChildStdioTransport::spawn(fixture_command("slow-shutdown"), escalation_config())
            .await
            .expect("spawn fixture");
    let response = transport
        .request("fixture/echo", Some(json!({"ready": true})))
        .await
        .expect("fixture is ready");
    assert_eq!(response, json!({"ready": true}));

    let started = Instant::now();
    let diagnostics = timeout(DEADLOCK_WATCHDOG, handle.shutdown())
        .await
        .expect("shutdown must remain bounded")
        .expect("child diagnostics");
    assert!(started.elapsed() < DEADLOCK_WATCHDOG);
    assert_eq!(diagnostics.reason, ChildExitReason::ShutdownRequested);
    assert!(matches!(
        diagnostics.termination,
        ChildTermination::Terminated | ChildTermination::Killed
    ));
    assert!(!diagnostics.success);

    let repeated = handle.shutdown().await.expect("shutdown is idempotent");
    assert_eq!(repeated, diagnostics);
    drop(transport);
}

#[tokio::test]
async fn stderr_tail_is_bounded_and_redacted_before_exposure() {
    let mut config = lifecycle_config();
    config.stderr_tail_bytes = 512;
    let config = config.with_redacted_value("fixture-prompt-secret");
    let (transport, handle) = ChildStdioTransport::spawn(fixture_command("stderr"), config)
        .await
        .expect("spawn fixture");
    assert_eq!(
        transport
            .request("fixture/echo", Some(json!({"ready": true})))
            .await
            .expect("fixture response"),
        json!({"ready": true})
    );

    let diagnostics = shutdown_with_watchdog(&handle).await;
    assert_graceful_shutdown(&diagnostics);
    assert!(diagnostics.stderr_tail.len() <= 512);
    assert!(diagnostics.stderr_tail.contains("[REDACTED]"));
    for secret in [
        "fixture-api-key",
        "fixture-bearer",
        "fixture-prompt-secret",
        "fixture-query-secret",
    ] {
        assert!(
            !diagnostics.stderr_tail.contains(secret),
            "stderr leaked {secret}: {}",
            diagnostics.stderr_tail
        );
    }
}

#[tokio::test]
async fn outbound_payload_limit_is_enforced_before_write() {
    let mut config = lifecycle_config();
    config.max_payload_bytes = 128;
    let (transport, handle) = ChildStdioTransport::spawn(fixture_command("normal"), config)
        .await
        .expect("spawn fixture");

    let ready = timeout(
        DEADLOCK_WATCHDOG,
        transport.request("fixture/echo", Some(json!({"ready": true}))),
    )
    .await
    .expect("fixture readiness should not hang")
    .expect("fixture readiness response");
    assert_eq!(ready, json!({"ready": true}));

    let result = transport
        .request("fixture/echo", Some(json!({"large": "x".repeat(1024)})))
        .await;
    assert!(matches!(result, Err(ClientError::Transport(message)) if message.contains("exceeds")));
    let diagnostics = shutdown_with_watchdog(&handle).await;
    assert_graceful_shutdown(&diagnostics);
}

#[tokio::test]
async fn invalid_zero_payload_limit_does_not_launch_child() {
    let mut config = lifecycle_config();
    config.max_payload_bytes = 0;
    let result = ChildStdioTransport::spawn(fixture_command("normal"), config).await;
    assert!(
        matches!(result, Err(ClientError::Transport(message)) if message.contains("greater than zero"))
    );
}
