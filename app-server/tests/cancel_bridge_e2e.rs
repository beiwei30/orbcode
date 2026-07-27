//! E2e test: `submit_background_turn` bridges the `BackgroundManager` cancel
//! token to the turn's cancellation, so setting the token causes the in-flight
//! turn — and any tools it is executing — to observe cancellation through
//! `ToolContext.cancellation`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use orbcode_app_server::{AppConfigOverrides, AppServer};
use orbcode_protocol::StreamEvent;
use tokio::time::{Duration, timeout};

fn test_path(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "orbcode-cancel-bridge-{label}-{}-{unique}",
        std::process::id()
    ))
}

fn mock_hang_overrides() -> HashMap<String, String> {
    let mut env = orbcode_app_server::sealed_provider_env_overrides();
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=hang".to_string(),
    );
    env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());
    env
}

#[tokio::test]
async fn cancel_token_bridges_to_turn_cancellation() {
    let home = test_path("bridge-home");
    let cwd = test_path("bridge-cwd");
    tokio::fs::create_dir_all(&home).await.expect("home");
    tokio::fs::create_dir_all(&cwd).await.expect("cwd");

    let app = AppServer::new(
        cwd,
        AppConfigOverrides {
            home_dir: Some(home),
            env_overrides: mock_hang_overrides(),
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("app server");

    let bootstrap = app.bootstrap(None).await.expect("bootstrap");
    let session_id = bootstrap.session.session_id.clone();

    let record = app
        .create_background_job(&session_id, "hang test")
        .await
        .expect("create job");
    app.mark_background_running(&record.job_id, None)
        .await
        .expect("mark running");

    let mut rx = app
        .submit_background_turn(&session_id, "trigger hang scenario", &record.job_id)
        .await
        .expect("submit background turn");

    // Wait for the turn to start streaming (the mock emits MessageStart +
    // ContentBlockStart then blocks), then signal cancel via the
    // BackgroundManager's token — exactly as cancel_background_job would.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let token = app
        .background_cancel_token(&record.job_id)
        .expect("cancel token should exist");
    token.store(true, Ordering::SeqCst);

    // The cancel bridge (spawned by submit_background_turn) polls the token
    // and calls cancel_turn, which makes the mock provider unblock and the
    // turn loop emit TurnCancelled.
    let saw_cancelled = timeout(Duration::from_secs(10), async {
        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::TurnCancelled { .. }) {
                return true;
            }
        }
        false
    })
    .await;

    assert!(
        saw_cancelled.unwrap_or(false),
        "expected TurnCancelled after BackgroundManager cancel token was signalled"
    );
}
