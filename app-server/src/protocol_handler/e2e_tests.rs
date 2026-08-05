//! Integration tests exercising the full `handle_request()` dispatch path
//! through a real `AppServer`. Each test constructs typed
//! `ClientRequestEnvelope` messages, sends them through the protocol handler,
//! and asserts on the `ServerResponseEnvelope` that comes back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{AppServer, sealed_provider_env_overrides};
use orbcode_app_server_protocol::{
    ClientRequestEnvelope, ErrorCode, InitializeResult, ResponseResult,
};
use orbcode_config::AppConfigOverrides;
use serde_json::json;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_path(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "orbcode-proto-e2e-{label}-{}-{unique}",
        std::process::id()
    ))
}

fn make_request(method: &str, params: serde_json::Value) -> ClientRequestEnvelope {
    ClientRequestEnvelope {
        id: uuid::Uuid::new_v4().to_string(),
        method: method.to_string(),
        params: Some(params),
    }
}

fn make_request_no_params(method: &str) -> ClientRequestEnvelope {
    ClientRequestEnvelope {
        id: uuid::Uuid::new_v4().to_string(),
        method: method.to_string(),
        params: None,
    }
}

fn mock_overrides() -> HashMap<String, String> {
    let mut env = sealed_provider_env_overrides();
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        "mock://anthropic?scenario=hello".to_string(),
    );
    env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());
    env
}

async fn test_app(label: &str) -> AppServer {
    let home = test_path(&format!("{label}-home"));
    let cwd = test_path(&format!("{label}-cwd"));
    tokio::fs::create_dir_all(&home).await.expect("home");
    tokio::fs::create_dir_all(&cwd).await.expect("cwd");
    AppServer::new(
        cwd,
        AppConfigOverrides {
            home_dir: Some(home),
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("app server")
}

async fn test_app_with_mock(label: &str) -> AppServer {
    let home = test_path(&format!("{label}-home"));
    let cwd = test_path(&format!("{label}-cwd"));
    tokio::fs::create_dir_all(&home).await.expect("home");
    tokio::fs::create_dir_all(&cwd).await.expect("cwd");
    AppServer::new(
        cwd,
        AppConfigOverrides {
            home_dir: Some(home),
            env_overrides: mock_overrides(),
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("app server")
}

// ---------------------------------------------------------------------------
// 1. Initialize
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initialize_returns_protocol_version_server_info_and_capabilities() {
    let app = test_app("init").await;
    let req = make_request(
        "initialize",
        json!({
            "protocol_version": "1.0",
            "client_info": { "name": "test-harness", "version": "0.1" },
        }),
    );
    let resp = app.handle_request(req).await;
    match resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let init: InitializeResult =
                serde_json::from_value(data).expect("deserialize InitializeResult");
            assert_eq!(init.protocol_version, "1.0");
            assert_eq!(init.server_info.name, "orbcode");
            assert!(!init.server_info.version.is_empty());
            assert!(init.capabilities.streaming);
            assert!(
                !init.capabilities.stable_methods.is_empty(),
                "capabilities must advertise at least one stable method"
            );
            assert!(
                !init.capabilities.experimental_methods.is_empty(),
                "capabilities must advertise at least one experimental method"
            );
            // Spot-check stable methods
            assert!(
                init.capabilities
                    .stable_methods
                    .contains(&"initialize".to_string())
            );
            assert!(
                init.capabilities
                    .stable_methods
                    .contains(&"session/bootstrap".to_string())
            );
            assert!(
                init.capabilities
                    .stable_methods
                    .contains(&"turn/submit".to_string())
            );
            // Spot-check experimental methods
            assert!(
                init.capabilities
                    .experimental_methods
                    .contains(&"background/create".to_string())
            );
            assert!(
                !init.capabilities.server_notification_methods.is_empty(),
                "capabilities must advertise notification methods"
            );
            assert!(
                !init.capabilities.server_request_methods.is_empty(),
                "capabilities must advertise server request methods"
            );
        }
        other => panic!("expected Success with data, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 2. Session lifecycle: bootstrap -> list -> rename -> list -> clear -> list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_lifecycle_bootstrap_list_rename_clear() {
    // Pre-seed a transcript so session/list and session/rename can find it.
    let home = test_path("lifecycle-home");
    let cwd = test_path("lifecycle-cwd");
    tokio::fs::create_dir_all(&home).await.expect("home");
    tokio::fs::create_dir_all(&cwd).await.expect("cwd");

    let project_dir = home
        .join("projects")
        .join(orbcode_config::sanitize_path(&cwd.display().to_string()));
    tokio::fs::create_dir_all(&project_dir)
        .await
        .expect("project dir");
    let session_id = "lifecycle-test-session";
    let payload = json!({
        "type": "user",
        "uuid": "uuid-lc",
        "timestamp": "2026-06-01T00:00:00.000Z",
        "message": { "role": "user", "content": "lifecycle test" },
        "cwd": cwd.display().to_string(),
        "sessionId": session_id,
    });
    tokio::fs::write(
        project_dir.join(format!("{session_id}.jsonl")),
        format!("{}\n", serde_json::to_string(&payload).expect("serialize")),
    )
    .await
    .expect("write transcript");

    let app = AppServer::new(
        cwd,
        AppConfigOverrides {
            home_dir: Some(home),
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("app server");

    // Bootstrap the pre-seeded session
    let resp = app
        .handle_request(make_request(
            "session/bootstrap",
            json!({ "session_id": session_id }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let obj = data
                .as_object()
                .expect("bootstrap data should be an object");
            assert!(obj.contains_key("session"), "bootstrap must return session");
            assert_eq!(
                obj["session"]["session_id"].as_str(),
                Some(session_id),
                "should resume the seeded session"
            );
        }
        other => panic!("expected Success, got: {other:?}"),
    }

    // List sessions: should contain the pre-seeded session
    let resp = app
        .handle_request(make_request_no_params("session/list"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let sessions: Vec<serde_json::Value> =
                serde_json::from_value(data.clone()).expect("parse session list");
            assert!(
                sessions
                    .iter()
                    .any(|s| s["session_id"].as_str() == Some(session_id)),
                "session list should contain the session"
            );
        }
        other => panic!("expected session list, got: {other:?}"),
    }

    // Rename the session
    let resp = app
        .handle_request(make_request(
            "session/rename",
            json!({
                "session_id": session_id,
                "new_title": "My Test Session",
            }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { .. } => {}
        other => panic!("expected rename success, got: {other:?}"),
    }

    // List sessions again: renamed session should have the custom title
    let resp = app
        .handle_request(make_request_no_params("session/list"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let sessions: Vec<serde_json::Value> =
                serde_json::from_value(data.clone()).expect("parse session list");
            let our = sessions
                .iter()
                .find(|s| s["session_id"].as_str() == Some(session_id))
                .expect("our session should still be listed");
            assert_eq!(
                our["custom_title"].as_str(),
                Some("My Test Session"),
                "custom title should be updated after rename"
            );
        }
        other => panic!("expected session list, got: {other:?}"),
    }

    // Clear the session: produces a new session
    let resp = app
        .handle_request(make_request(
            "session/clear",
            json!({ "session_id": session_id }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let obj = data.as_object().expect("clear data should be an object");
            assert!(obj.contains_key("session"), "clear must return session");
            let new_id = obj["session"]["session_id"]
                .as_str()
                .expect("new session_id");
            assert_ne!(new_id, session_id, "clear should produce a new session id");
        }
        other => panic!("expected Success from clear, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3. Turn submit: bootstrap -> turn/submit -> subscription_id
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_submit_returns_subscription_id() {
    let app = test_app_with_mock("turn-submit").await;

    // Bootstrap first
    let resp = app
        .handle_request(make_request_no_params("session/bootstrap"))
        .await;
    let session_id = match &resp.result {
        ResponseResult::Success { data: Some(data) } => data["session"]["session_id"]
            .as_str()
            .expect("session_id")
            .to_string(),
        other => panic!("expected bootstrap Success, got: {other:?}"),
    };

    // Submit a turn
    let resp = app
        .handle_request(make_request(
            "turn/submit",
            json!({
                "session_id": session_id,
                "prompt": "hello world",
            }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let sub_id = data["subscription_id"]
                .as_str()
                .expect("subscription_id should be a string");
            assert!(!sub_id.is_empty(), "subscription_id should not be empty");
            // The subscription_id should be a valid UUID
            assert!(
                uuid::Uuid::parse_str(sub_id).is_ok(),
                "subscription_id should be a valid UUID"
            );
        }
        other => panic!("expected turn/submit Success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 4. Unknown method
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let app = test_app("unknown-method").await;
    let resp = app
        .handle_request(make_request_no_params("totally/bogus"))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(err.code, ErrorCode::MethodNotFound);
            assert!(
                err.message.contains("totally/bogus"),
                "error message should name the unknown method"
            );
        }
        other => panic!("expected MethodNotFound error, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 5. Invalid params
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_rename_with_missing_params_returns_invalid_params() {
    let app = test_app("invalid-params-rename").await;
    // session/rename requires session_id and new_title
    let resp = app
        .handle_request(make_request_no_params("session/rename"))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(err.code, ErrorCode::InvalidParams);
        }
        other => panic!("expected InvalidParams error, got: {other:?}"),
    }
}

#[tokio::test]
async fn session_rename_with_wrong_param_types_returns_invalid_params() {
    let app = test_app("invalid-params-types").await;
    // Send session_id as number instead of string
    let resp = app
        .handle_request(make_request(
            "session/rename",
            json!({
                "session_id": 12345,
                "new_title": true,
            }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(err.code, ErrorCode::InvalidParams);
        }
        other => panic!("expected InvalidParams error for wrong types, got: {other:?}"),
    }
}

#[tokio::test]
async fn turn_submit_without_params_returns_invalid_params() {
    let app = test_app("invalid-params-turn").await;
    let resp = app
        .handle_request(make_request_no_params("turn/submit"))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(err.code, ErrorCode::InvalidParams);
        }
        other => panic!("expected InvalidParams for turn/submit without params, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 6. Permission overview
// ---------------------------------------------------------------------------

#[tokio::test]
async fn permission_overview_returns_permission_data() {
    let app = test_app("perm-overview").await;

    // Bootstrap to initialise the session
    let _bs = app
        .handle_request(make_request_no_params("session/bootstrap"))
        .await;

    let resp = app
        .handle_request(make_request_no_params("permission/overview"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let obj = data
                .as_object()
                .expect("permission overview should be object");
            // Check that expected fields exist
            assert!(obj.contains_key("allow_all"), "should have allow_all field");
            assert!(
                obj.contains_key("settings_allowed_rules"),
                "should have settings_allowed_rules field"
            );
            assert!(
                obj.contains_key("settings_denied_rules"),
                "should have settings_denied_rules field"
            );
        }
        other => panic!("expected permission overview success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 7. Settings: model_name
// ---------------------------------------------------------------------------

#[tokio::test]
async fn settings_model_name_returns_non_empty() {
    let app = test_app("settings-model").await;
    let resp = app
        .handle_request(make_request_no_params("settings/model_name"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let model_name = data["model_name"]
                .as_str()
                .expect("model_name should be a string");
            assert!(!model_name.is_empty(), "model_name should not be empty");
        }
        other => panic!("expected settings/model_name success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 8. Diagnostics: status (requires session_id)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn diagnostics_status_returns_overview() {
    let app = test_app("diag-status").await;

    // Bootstrap to get a session_id
    let resp = app
        .handle_request(make_request_no_params("session/bootstrap"))
        .await;
    let session_id = match &resp.result {
        ResponseResult::Success { data: Some(data) } => data["session"]["session_id"]
            .as_str()
            .expect("session_id")
            .to_string(),
        other => panic!("expected bootstrap Success, got: {other:?}"),
    };

    // Request diagnostics/status
    let resp = app
        .handle_request(make_request(
            "diagnostics/status",
            json!({ "session_id": session_id }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let obj = data
                .as_object()
                .expect("diagnostics status should be object");
            assert!(obj.contains_key("session_id"), "should have session_id");
            assert!(obj.contains_key("cwd"), "should have cwd");
            assert!(
                obj.contains_key("model_display_name"),
                "should have model_display_name"
            );
            assert!(
                obj.contains_key("default_provider"),
                "should have default_provider"
            );
            assert!(
                obj.contains_key("available_tool_count"),
                "should have available_tool_count"
            );
        }
        other => panic!("expected diagnostics/status success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 9. Permission mode
// ---------------------------------------------------------------------------

#[tokio::test]
async fn permission_mode_returns_current_mode() {
    let app = test_app("perm-mode").await;
    let resp = app
        .handle_request(make_request_no_params("permission/mode"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let mode = data["mode"].as_str().expect("mode should be a string");
            assert!(!mode.is_empty(), "permission mode should not be empty");
        }
        other => panic!("expected permission/mode success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 10. Context preview
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_preview_returns_cwd() {
    let app = test_app("ctx-preview").await;
    let resp = app
        .handle_request(make_request_no_params("context/preview"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let cwd = data["cwd"].as_str().expect("cwd should be a string");
            assert!(!cwd.is_empty(), "cwd should not be empty");
        }
        other => panic!("expected context/preview success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 11. Tools list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tools_list_returns_foundation_tools() {
    let app = test_app("tools-list").await;
    let resp = app
        .handle_request(make_request_no_params("tools/list"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let tools: Vec<serde_json::Value> =
                serde_json::from_value(data.clone()).expect("parse tool list");
            assert!(
                !tools.is_empty(),
                "tools list should contain foundation tools"
            );
            // Each tool should have a name and summary
            for tool in &tools {
                assert!(
                    tool["name"].as_str().is_some(),
                    "each tool should have a name"
                );
            }
            let ask_user = tools
                .iter()
                .find(|tool| tool["name"] == "ask-user-question")
                .expect("AskUserQuestion diagnostic entry");
            assert_eq!(
                ask_user["unavailable_reason"].as_str(),
                Some(
                    "available to the provider only for turns owned by a client that declares the full interactive_questions capability"
                )
            );
        }
        other => panic!("expected tools/list success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 12. Settings theme
// ---------------------------------------------------------------------------

#[tokio::test]
async fn settings_theme_returns_current_theme() {
    let app = test_app("settings-theme").await;
    let resp = app
        .handle_request(make_request_no_params("settings/theme"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let theme = data["theme"].as_str().expect("theme should be a string");
            assert!(!theme.is_empty(), "theme should not be empty");
        }
        other => panic!("expected settings/theme success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 13. Response envelope id is preserved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn response_id_matches_request_id() {
    let app = test_app("id-match").await;
    let req_id = "custom-request-id-42";
    let req = ClientRequestEnvelope {
        id: req_id.to_string(),
        method: "initialize".to_string(),
        params: Some(json!({
            "protocol_version": "1.0",
            "client_info": { "name": "test", "version": "0.1" },
        })),
    };
    let resp = app.handle_request(req).await;
    assert_eq!(resp.id, req_id, "response id must match request id");
}

// ---------------------------------------------------------------------------
// 14. Diagnostics status without session returns invalid params
// ---------------------------------------------------------------------------

#[tokio::test]
async fn diagnostics_status_without_session_id_returns_invalid_params() {
    let app = test_app("diag-no-sess").await;
    let resp = app
        .handle_request(make_request_no_params("diagnostics/status"))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(err.code, ErrorCode::InvalidParams);
        }
        other => panic!(
            "expected InvalidParams for diagnostics/status without session_id, got: {other:?}"
        ),
    }
}

// ===========================================================================
// Turn lifecycle E2E tests
// ===========================================================================

// ---------------------------------------------------------------------------
// 15. turn/cancel returns cancelled status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_cancel_returns_cancelled_status() {
    let app = test_app_with_mock("turn-cancel").await;

    // Bootstrap
    let resp = app
        .handle_request(make_request_no_params("session/bootstrap"))
        .await;
    let session_id = match &resp.result {
        ResponseResult::Success { data: Some(data) } => data["session"]["session_id"]
            .as_str()
            .expect("session_id")
            .to_string(),
        other => panic!("expected bootstrap Success, got: {other:?}"),
    };

    // Submit a turn so there is an active turn to cancel
    let resp = app
        .handle_request(make_request(
            "turn/submit",
            json!({
                "session_id": session_id,
                "prompt": "say hello",
            }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(_) } => {}
        other => panic!("expected turn/submit Success, got: {other:?}"),
    }

    // Cancel the turn
    let resp = app
        .handle_request(make_request(
            "turn/cancel",
            json!({ "session_id": session_id }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            // The response should contain a "cancelled" boolean field
            assert!(
                data.get("cancelled").is_some(),
                "turn/cancel response must contain 'cancelled' field"
            );
            assert!(
                data["cancelled"].is_boolean(),
                "cancelled field must be a boolean"
            );
        }
        other => panic!("expected turn/cancel Success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 16. turn/interrupt returns interrupted status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_interrupt_returns_interrupted_status() {
    let app = test_app_with_mock("turn-interrupt").await;

    // Bootstrap
    let resp = app
        .handle_request(make_request_no_params("session/bootstrap"))
        .await;
    let session_id = match &resp.result {
        ResponseResult::Success { data: Some(data) } => data["session"]["session_id"]
            .as_str()
            .expect("session_id")
            .to_string(),
        other => panic!("expected bootstrap Success, got: {other:?}"),
    };

    // Submit a turn
    let resp = app
        .handle_request(make_request(
            "turn/submit",
            json!({
                "session_id": session_id,
                "prompt": "say hello",
            }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(_) } => {}
        other => panic!("expected turn/submit Success, got: {other:?}"),
    }

    // Interrupt the turn
    let resp = app
        .handle_request(make_request(
            "turn/interrupt",
            json!({ "session_id": session_id }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            assert!(
                data.get("interrupted").is_some(),
                "turn/interrupt response must contain 'interrupted' field"
            );
            assert!(
                data["interrupted"].is_boolean(),
                "interrupted field must be a boolean"
            );
        }
        other => panic!("expected turn/interrupt Success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 17. turn/steer during active turn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_steer_during_active_turn() {
    let app = test_app_with_mock("turn-steer").await;

    // Bootstrap
    let resp = app
        .handle_request(make_request_no_params("session/bootstrap"))
        .await;
    let session_id = match &resp.result {
        ResponseResult::Success { data: Some(data) } => data["session"]["session_id"]
            .as_str()
            .expect("session_id")
            .to_string(),
        other => panic!("expected bootstrap Success, got: {other:?}"),
    };

    // Submit a turn first
    let resp = app
        .handle_request(make_request(
            "turn/submit",
            json!({
                "session_id": session_id,
                "prompt": "say hello",
            }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(_) } => {}
        other => panic!("expected turn/submit Success, got: {other:?}"),
    }

    // Steer the turn -- even if the turn completes quickly, we verify the
    // dispatch path returns a well-formed response (either success or a
    // structured error like NoActiveTurn).
    let resp = app
        .handle_request(make_request(
            "turn/steer",
            json!({
                "session_id": session_id,
                "prompt": "actually say goodbye",
            }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { .. } => {
            // Steer accepted
        }
        ResponseResult::Error(err) => {
            // If the turn already finished, NoActiveTurn is acceptable
            assert_eq!(
                err.code,
                ErrorCode::NoActiveTurn,
                "steer during inactive turn should return NoActiveTurn, got: {err:?}"
            );
        }
        _ => panic!(
            "expected turn/steer response result, got: {:?}",
            resp.result
        ),
    }
}

// ---------------------------------------------------------------------------
// 18. turn/submit with missing prompt field returns InvalidParams
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_submit_with_missing_prompt_returns_invalid_params() {
    let app = test_app_with_mock("turn-missing-prompt").await;

    // Bootstrap first
    let resp = app
        .handle_request(make_request_no_params("session/bootstrap"))
        .await;
    let session_id = match &resp.result {
        ResponseResult::Success { data: Some(data) } => data["session"]["session_id"]
            .as_str()
            .expect("session_id")
            .to_string(),
        other => panic!("expected bootstrap Success, got: {other:?}"),
    };

    // Submit a turn with session_id but no prompt
    let resp = app
        .handle_request(make_request(
            "turn/submit",
            json!({ "session_id": session_id }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(
                err.code,
                ErrorCode::InvalidParams,
                "turn/submit without prompt should return InvalidParams, got: {err:?}"
            );
        }
        other => panic!("expected InvalidParams error, got: {other:?}"),
    }
}

// ===========================================================================
// Permission dispatch E2E tests
// ===========================================================================

// ---------------------------------------------------------------------------
// 19. permission/add_rule persists in overview
// ---------------------------------------------------------------------------

#[tokio::test]
async fn permission_add_rule_persists() {
    let app = test_app("perm-add-rule").await;

    // Bootstrap to initialise the session
    let _bs = app
        .handle_request(make_request_no_params("session/bootstrap"))
        .await;

    // Add an allow rule
    let resp = app
        .handle_request(make_request(
            "permission/add_rule",
            json!({
                "kind": "allow",
                "rule": "Bash(echo *)",
            }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { .. } => {}
        other => panic!("expected add_rule success, got: {other:?}"),
    }

    // Verify the rule appears in the overview
    let resp = app
        .handle_request(make_request_no_params("permission/overview"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            // The added rule should appear somewhere in the overview data
            let json_str = serde_json::to_string(data).expect("serialize overview");
            assert!(
                json_str.contains("Bash(echo *)"),
                "permission overview should contain the added rule, got: {json_str}"
            );
        }
        other => panic!("expected permission overview success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 20. permission/remove_rule removes rule from overview
// ---------------------------------------------------------------------------

#[tokio::test]
async fn permission_remove_rule() {
    let app = test_app("perm-remove-rule").await;

    // Bootstrap
    let _bs = app
        .handle_request(make_request_no_params("session/bootstrap"))
        .await;

    // Add a rule
    let resp = app
        .handle_request(make_request(
            "permission/add_rule",
            json!({
                "kind": "allow",
                "rule": "Read(*)",
            }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { .. } => {}
        other => panic!("expected add_rule success, got: {other:?}"),
    }

    // Remove the same rule
    let resp = app
        .handle_request(make_request(
            "permission/remove_rule",
            json!({
                "kind": "allow",
                "rule": "Read(*)",
            }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { .. } => {}
        other => panic!("expected remove_rule success, got: {other:?}"),
    }

    // Verify the rule no longer appears in the overview
    let resp = app
        .handle_request(make_request_no_params("permission/overview"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let json_str = serde_json::to_string(data).expect("serialize overview");
            assert!(
                !json_str.contains("Read(*)"),
                "permission overview should NOT contain the removed rule, got: {json_str}"
            );
        }
        other => panic!("expected permission overview success, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 21. permission/set_mode affects allow_all in overview
// ---------------------------------------------------------------------------

#[tokio::test]
async fn permission_set_mode_round_trips() {
    let app = test_app("perm-set-mode").await;

    // Bootstrap to initialise
    let _bs = app
        .handle_request(make_request_no_params("session/bootstrap"))
        .await;

    // Default state: allow_all should be false
    let resp = app
        .handle_request(make_request_no_params("permission/overview"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            assert_eq!(
                data["allow_all"].as_bool(),
                Some(false),
                "allow_all should start as false"
            );
        }
        other => panic!("expected permission overview success, got: {other:?}"),
    }

    // Set mode to "bypassPermissions" which should enable allow_all
    let resp = app
        .handle_request(make_request(
            "permission/set_mode",
            json!({ "mode": "bypassPermissions" }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { .. } => {}
        other => panic!("expected set_mode success, got: {other:?}"),
    }

    // Verify allow_all is now true
    let resp = app
        .handle_request(make_request_no_params("permission/overview"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            assert_eq!(
                data["allow_all"].as_bool(),
                Some(true),
                "allow_all should be true after setting bypassPermissions mode"
            );
        }
        other => {
            panic!("expected permission overview success after bypassPermissions, got: {other:?}")
        }
    }

    // Set back to "default" which should disable allow_all
    let resp = app
        .handle_request(make_request(
            "permission/set_mode",
            json!({ "mode": "default" }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { .. } => {}
        other => panic!("expected set_mode success, got: {other:?}"),
    }

    // Verify allow_all is false again
    let resp = app
        .handle_request(make_request_no_params("permission/overview"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            assert_eq!(
                data["allow_all"].as_bool(),
                Some(false),
                "allow_all should be false after resetting to default mode"
            );
        }
        other => panic!("expected permission overview success after default, got: {other:?}"),
    }

    // Verify set_mode rejects unknown modes
    let resp = app
        .handle_request(make_request(
            "permission/set_mode",
            json!({ "mode": "invalid_mode_xyz" }),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(
                err.code,
                ErrorCode::InvalidParams,
                "set_mode with invalid mode should return InvalidParams"
            );
        }
        other => panic!("expected InvalidParams for unknown mode, got: {other:?}"),
    }
}

// ===========================================================================
// P3-16: MCP protocol dispatch smoke matrix
// ===========================================================================

#[tokio::test]
async fn mcp_list_servers_returns_empty_array() {
    let app = test_app("mcp-list-servers").await;
    let resp = app
        .handle_request(make_request_no_params("mcp/list_servers"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let arr = data.as_array().expect("should be array");
            assert!(arr.is_empty(), "fresh server should have no MCP servers");
        }
        other => panic!("expected Success, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_capabilities_returns_array_with_fields() {
    let app = test_app("mcp-capabilities").await;
    let resp = app
        .handle_request(make_request_no_params("mcp/capabilities"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let arr = data.as_array().expect("capabilities should be an array");
            let transports = arr
                .iter()
                .map(|cap| cap["transport"].as_str().unwrap_or(""))
                .collect::<Vec<_>>();
            assert_eq!(transports, vec!["stdio", "streamable_http", "web_socket"]);
            for cap in arr {
                assert!(
                    cap.is_object(),
                    "each capability entry should be an object, got: {cap:?}"
                );
                assert!(cap["transport"].is_string(), "should have transport");
                assert!(cap.get("enabled").is_some(), "should have enabled");
            }
        }
        other => panic!("expected Success with array, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_slash_suggestions_returns_structured() {
    let app = test_app("mcp-slash").await;
    let resp = app
        .handle_request(make_request_no_params("mcp/slash_suggestions"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            assert!(data["servers"].is_array());
            assert!(data["tools"].is_array());
            assert!(data["resources"].is_array());
        }
        other => panic!("expected Success, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_server_trust_unknown_server() {
    let app = test_app("mcp-trust-unknown").await;
    let resp = app
        .handle_request(make_request(
            "mcp/server_trust",
            json!({"server_id": "nonexistent-server"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            assert!(
                data["trust"].is_string(),
                "trust should be a string even for unknown server"
            );
        }
        other => panic!("expected Success, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_remove_server_nonexistent() {
    let app = test_app("mcp-remove").await;
    let resp = app
        .handle_request(make_request(
            "mcp/remove_server",
            json!({"server_id": "nonexistent"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            assert_eq!(data["removed"].as_bool(), Some(false));
        }
        other => panic!("expected Success with removed=false, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_list_tools_unknown_server_returns_error() {
    let app = test_app("mcp-list-tools-err").await;
    let resp = app
        .handle_request(make_request(
            "mcp/list_tools",
            json!({"server_id": "no-such-server"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert!(
                err.code == ErrorCode::McpError || err.code == ErrorCode::PermissionDenied,
                "expected McpError or PermissionDenied, got: {err:?}"
            );
        }
        other => panic!("expected error, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_list_resources_unknown_server_returns_error() {
    let app = test_app("mcp-list-res-err").await;
    let resp = app
        .handle_request(make_request(
            "mcp/list_resources",
            json!({"server_id": "no-such-server"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert!(
                err.code == ErrorCode::McpError || err.code == ErrorCode::PermissionDenied,
                "expected McpError or PermissionDenied, got: {err:?}"
            );
        }
        other => panic!("expected error, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_list_prompts_unknown_server_returns_error() {
    let app = test_app("mcp-list-prompts-err").await;
    let resp = app
        .handle_request(make_request(
            "mcp/list_prompts",
            json!({"server_id": "no-such-server"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert!(
                err.code == ErrorCode::McpError || err.code == ErrorCode::PermissionDenied,
                "expected McpError or PermissionDenied, got: {err:?}"
            );
        }
        other => panic!("expected error, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_read_resource_with_session_id_can_use_session_scoped_server() {
    let home = test_path("mcp-session-resource-home");
    let cwd = test_path("mcp-session-resource-cwd");
    tokio::fs::create_dir_all(&home).await.expect("home");
    tokio::fs::create_dir_all(&cwd).await.expect("cwd");
    let app = AppServer::new(
        cwd,
        AppConfigOverrides {
            home_dir: Some(home),
            allow_tools: Some(true),
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("app server");
    let resp = app
        .handle_request(make_request(
            "session/bootstrap",
            json!({
                "session_mcp_servers": [{
                    "id": "session-docs",
                    "transport": "web_socket",
                    "endpoint": "modeled://session-resource",
                    "args": [],
                    "env": {},
                    "headers": {},
                    "enabled": true,
                    "status": "ready",
                    "summary": "Session docs",
                    "auth": { "kind": "none" },
                    "trust": "trusted"
                }]
            }),
        ))
        .await;
    assert!(
        matches!(resp.result, ResponseResult::Success { .. }),
        "bootstrap failed: {:?}",
        resp.result
    );
    let session_id = match &resp.result {
        ResponseResult::Success { data: Some(data) } => data["session"]["session_id"]
            .as_str()
            .expect("session id")
            .to_string(),
        other => panic!("expected bootstrap success, got: {other:?}"),
    };
    app.set_mcp_server_trust_for_session(
        &session_id,
        "session-docs",
        orbcode_mcp::McpServerTrust::Trusted,
    )
    .await
    .expect("trust session server");

    let global_resp = app
        .handle_request(make_request(
            "mcp/read_resource",
            json!({
                "server_id": "session-docs",
                "uri": "mcp://session-docs/info"
            }),
        ))
        .await;
    assert!(
        matches!(global_resp.result, ResponseResult::Error(_)),
        "global MCP handler should not see session-scoped server"
    );

    let session_resp = app
        .handle_request(make_request(
            "mcp/read_resource",
            json!({
                "session_id": session_id,
                "server_id": "session-docs",
                "uri": "mcp://session-docs/info"
            }),
        ))
        .await;
    match &session_resp.result {
        ResponseResult::Success { data: Some(data) } => {
            assert_eq!(data["uri"].as_str(), Some("mcp://session-docs/info"));
            assert!(
                data["contents"]
                    .as_str()
                    .is_some_and(|contents| contents.contains("modeled://session-resource"))
            );
        }
        other => panic!("expected session-scoped read resource success, got: {other:?}"),
    }
}

#[tokio::test]
async fn tools_skills_with_session_id_can_use_session_scoped_mcp_skill() {
    let home = test_path("tools-session-skills-home");
    let cwd = test_path("tools-session-skills-cwd");
    let session_cwd = test_path("tools-session-skills-session-cwd");
    tokio::fs::create_dir_all(&home).await.expect("home");
    tokio::fs::create_dir_all(&cwd).await.expect("cwd");
    tokio::fs::create_dir_all(&session_cwd)
        .await
        .expect("session cwd");
    let global_skill_dir = cwd.join(".claude").join("skills").join("global-project");
    tokio::fs::create_dir_all(&global_skill_dir)
        .await
        .expect("global skill dir");
    tokio::fs::write(
        global_skill_dir.join("SKILL.md"),
        "---\ndescription: Global project skill\n---\nGLOBAL_PROJECT_SKILL_BODY",
    )
    .await
    .expect("global skill");
    let session_skill_dir = session_cwd
        .join(".claude")
        .join("skills")
        .join("session-project");
    tokio::fs::create_dir_all(&session_skill_dir)
        .await
        .expect("session skill dir");
    tokio::fs::write(
        session_skill_dir.join("SKILL.md"),
        "---\ndescription: Session project skill\n---\nSESSION_PROJECT_SKILL_BODY",
    )
    .await
    .expect("session skill");
    let app = AppServer::new(
        cwd.clone(),
        AppConfigOverrides {
            home_dir: Some(home),
            allow_tools: Some(true),
            allow_network: Some(true),
            ..AppConfigOverrides::default()
        },
    )
    .await
    .expect("app server");
    let script = r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  if [ -z "$id" ]; then id=1; fi
  case "$line" in
    *'"method":"initialize"'*)
      result='{"protocolVersion":"2024-11-05","capabilities":{"prompts":{}},"serverInfo":{"name":"session-docs","version":"0.1.0"}}'
      ;;
    *'"method":"prompts/list"'*)
      result='{"prompts":[{"name":"guide","description":"Use session docs.","_meta":{"skill":true}},{"name":"plain","description":"Plain prompt."}]}'
      ;;
    *'"method":"prompts/get"'*)
      result='{"description":"Use session docs.","messages":[{"role":"user","content":{"type":"text","text":"SESSION_DOCS_SKILL_BODY"}}]}'
      ;;
    *)
      result='{}'
      ;;
  esac
  printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done"#;
    let resp = app
        .handle_request(make_request(
            "session/bootstrap",
            json!({
                "cwd": session_cwd.clone(),
                "session_mcp_servers": [{
                    "id": "session-docs",
                    "transport": "stdio",
                    "endpoint": "sh",
                    "args": ["-c", script],
                    "env": {},
                    "headers": {},
                    "enabled": true,
                    "status": "ready",
                    "summary": "Session docs",
                    "auth": { "kind": "none" },
                    "trust": "trusted"
                }]
            }),
        ))
        .await;
    let session_id = match &resp.result {
        ResponseResult::Success { data: Some(data) } => data["session"]["session_id"]
            .as_str()
            .expect("session id")
            .to_string(),
        other => panic!("expected bootstrap success, got: {other:?}"),
    };
    app.set_mcp_server_trust_for_session(
        &session_id,
        "session-docs",
        orbcode_mcp::McpServerTrust::Trusted,
    )
    .await
    .expect("trust session server");

    let current_resp = app
        .handle_request(make_request(
            "session/bootstrap",
            json!({ "cwd": cwd.clone() }),
        ))
        .await;
    assert!(
        matches!(current_resp.result, ResponseResult::Success { .. }),
        "current bootstrap failed: {:?}",
        current_resp.result
    );

    let global_resp = app
        .handle_request(make_request_no_params("tools/skills"))
        .await;
    match &global_resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let skills = data.as_array().expect("skills array");
            assert!(
                skills
                    .iter()
                    .any(|skill| skill["name"].as_str() == Some("global-project")),
                "global tools/skills should use app cwd project skills"
            );
            assert!(
                !skills
                    .iter()
                    .any(|skill| skill["name"].as_str() == Some("session-project")),
                "global tools/skills must not see session cwd project skill"
            );
            assert!(
                !skills
                    .iter()
                    .any(|skill| skill["name"].as_str() == Some("session-docs:guide")),
                "global tools/skills must not see session-scoped MCP skill"
            );
        }
        other => panic!("expected global tools/skills success, got: {other:?}"),
    }

    let session_resp = app
        .handle_request(make_request(
            "tools/skills",
            json!({ "session_id": session_id }),
        ))
        .await;
    match &session_resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let skills = data.as_array().expect("skills array");
            assert!(
                skills
                    .iter()
                    .any(|skill| skill["name"].as_str() == Some("session-project")),
                "session tools/skills should use the target session cwd"
            );
            assert!(
                !skills
                    .iter()
                    .any(|skill| skill["name"].as_str() == Some("global-project")),
                "session tools/skills must not mix in the app cwd project skill"
            );
            let guide = skills
                .iter()
                .find(|skill| skill["name"].as_str() == Some("session-docs:guide"))
                .expect("session-scoped MCP skill listed");
            assert!(
                guide["body"]
                    .as_str()
                    .is_some_and(|body| body.contains("SESSION_DOCS_SKILL_BODY")),
                "session skill body should come from prompts/get: {guide:?}"
            );
            assert!(
                !skills
                    .iter()
                    .any(|skill| skill["name"].as_str() == Some("session-docs:plain")),
                "unmarked session MCP prompt must not be exposed as a skill"
            );
        }
        other => panic!("expected session tools/skills success, got: {other:?}"),
    }
}

#[tokio::test]
async fn tools_skills_with_unknown_session_id_returns_error() {
    let app = test_app("tools-session-skills-unknown").await;
    let resp = app
        .handle_request(make_request(
            "tools/skills",
            json!({"session_id": "missing-session"}),
        ))
        .await;

    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(err.code, ErrorCode::SessionNotFound);
        }
        other => panic!("expected session not found error, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_invoke_tool_unknown_server_returns_error() {
    let app = test_app("mcp-invoke-err").await;
    let resp = app
        .handle_request(make_request(
            "mcp/invoke_tool",
            json!({"server_id": "no-such-server", "tool_name": "test", "input": "{}"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert!(
                err.code == ErrorCode::McpError || err.code == ErrorCode::PermissionDenied,
                "expected McpError or PermissionDenied, got: {err:?}"
            );
        }
        other => panic!("expected error, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_diagnose_unknown_server_returns_error() {
    let app = test_app("mcp-diagnose-err").await;
    let resp = app
        .handle_request(make_request(
            "mcp/diagnose",
            json!({"server_id": "no-such-server"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(err.code, ErrorCode::McpError);
        }
        other => panic!("expected McpError, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_set_trust_then_verify_via_server_trust() {
    let app = test_app("mcp-set-trust").await;

    // 1. Upsert a real MCP server so set_trust has a valid target.
    let upsert = app
        .handle_request(make_request(
            "mcp/upsert_server",
            json!({
                "id": "trust-test-server",
                "transport": "stdio",
                "endpoint": "echo",
                "args": ["hello"],
                "enabled": false,
                "summary": "trust round-trip test",
                "auth": {"kind": "none"},
            }),
        ))
        .await;
    // 2. Assert the upsert succeeded.
    match &upsert.result {
        ResponseResult::Success { .. } => {}
        other => panic!("upsert_server should succeed, got: {other:?}"),
    }

    // 3. Set trust — must succeed now that the server exists.
    let set_resp = app
        .handle_request(make_request(
            "mcp/set_trust",
            json!({"server_id": "trust-test-server", "trust": "trusted"}),
        ))
        .await;
    // 4. Assert the set succeeded.
    match &set_resp.result {
        ResponseResult::Success { .. } => {}
        other => panic!("set_trust should succeed after upsert, got: {other:?}"),
    }

    // 5. Read trust back via mcp/server_trust.
    let get_resp = app
        .handle_request(make_request(
            "mcp/server_trust",
            json!({"server_id": "trust-test-server"}),
        ))
        .await;
    // 6. Assert it returns "trusted" (or "Trusted").
    match &get_resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let trust = data["trust"]
                .as_str()
                .expect("trust field should be a string");
            assert!(
                trust.eq_ignore_ascii_case("trusted"),
                "trust should read back as 'trusted', got: {trust:?}"
            );
        }
        other => panic!("server_trust should succeed, got: {other:?}"),
    }

    // 7. Setting trust on a non-existent server must return an error that is
    //    NOT MethodNotFound or InvalidParams (it should be a domain error).
    let bad_resp = app
        .handle_request(make_request(
            "mcp/set_trust",
            json!({"server_id": "no-such-server-xyz", "trust": "trusted"}),
        ))
        .await;
    match &bad_resp.result {
        ResponseResult::Error(err) => {
            assert_ne!(
                err.code,
                ErrorCode::MethodNotFound,
                "non-existent server should not produce MethodNotFound"
            );
            assert_ne!(
                err.code,
                ErrorCode::InvalidParams,
                "non-existent server should not produce InvalidParams"
            );
        }
        other => panic!("set_trust on non-existent server should error, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_read_resource_unknown_server_returns_error() {
    let app = test_app("mcp-read-res-err").await;
    let resp = app
        .handle_request(make_request(
            "mcp/read_resource",
            json!({"server_id": "no-such-server", "uri": "file:///tmp/x"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert!(
                err.code == ErrorCode::McpError || err.code == ErrorCode::PermissionDenied,
                "expected McpError or PermissionDenied, got: {err:?}"
            );
        }
        other => panic!("expected error, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_get_prompt_unknown_server_returns_error() {
    let app = test_app("mcp-get-prompt-err").await;
    let resp = app
        .handle_request(make_request(
            "mcp/get_prompt",
            json!({"server_id": "no-such-server", "name": "test"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert!(
                err.code == ErrorCode::McpError || err.code == ErrorCode::PermissionDenied,
                "expected McpError or PermissionDenied, got: {err:?}"
            );
        }
        other => panic!("expected error, got: {other:?}"),
    }
}

#[tokio::test]
async fn mcp_upsert_server_missing_params_returns_invalid_params() {
    let app = test_app("mcp-upsert-err").await;
    let resp = app
        .handle_request(make_request("mcp/upsert_server", json!({})))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(err.code, ErrorCode::InvalidParams);
        }
        other => panic!("expected InvalidParams, got: {other:?}"),
    }
}

#[tokio::test]
async fn background_create_returns_job_record() {
    let app = test_app("bg-create").await;
    let resp = app
        .handle_request(make_request(
            "background/create",
            json!({"session_id": "any-session", "prompt": "test"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            assert!(
                data["job_id"].is_string(),
                "should return job_id, got: {data:?}"
            );
            assert!(
                data["status"].is_string(),
                "should return status, got: {data:?}"
            );
        }
        other => panic!("expected Success with job record, got: {other:?}"),
    }
}

// ===========================================================================
// P3-17: Background handlers dispatch coverage
// ===========================================================================

#[tokio::test]
async fn background_list_returns_empty_for_fresh_server() {
    let app = test_app("bg-list").await;
    let resp = app
        .handle_request(make_request_no_params("background/list"))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let arr = data.as_array().expect("should be array");
            assert!(
                arr.is_empty(),
                "fresh server should have no background jobs"
            );
        }
        other => panic!("expected empty array, got: {other:?}"),
    }
}

#[tokio::test]
async fn background_detail_unknown_job_returns_error() {
    let app = test_app("bg-detail-err").await;
    let resp = app
        .handle_request(make_request(
            "background/detail",
            json!({"job_id": "nonexistent-job"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(_) => {}
        other => panic!("expected error for unknown job, got: {other:?}"),
    }
}

#[tokio::test]
async fn background_log_unknown_job_returns_error() {
    let app = test_app("bg-log-err").await;
    let resp = app
        .handle_request(make_request(
            "background/log",
            json!({"job_id": "nonexistent-job"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(_) => {}
        other => panic!("expected error for unknown job, got: {other:?}"),
    }
}

#[tokio::test]
async fn background_cancel_unknown_job_returns_error() {
    let app = test_app("bg-cancel-err").await;
    let resp = app
        .handle_request(make_request(
            "background/cancel",
            json!({"job_id": "nonexistent-job"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(_) => {}
        other => panic!("expected error for unknown job, got: {other:?}"),
    }
}

#[tokio::test]
async fn background_events_unknown_job_returns_empty_string() {
    let app = test_app("bg-events-unknown").await;
    let resp = app
        .handle_request(make_request(
            "background/events",
            json!({"job_id": "nonexistent-job"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Success { data: Some(data) } => {
            let events = &data["events"];
            assert!(
                events.is_string() && events.as_str().unwrap().is_empty(),
                "events for unknown job should be empty string, got: {events:?}"
            );
        }
        other => panic!("expected Success with empty events, got: {other:?}"),
    }
}

// ===========================================================================
// P3-18: tools/invoke dispatch
// ===========================================================================

#[tokio::test]
async fn tools_invoke_nonexistent_tool_returns_error() {
    let app = test_app("tools-invoke-err").await;
    let resp = app
        .handle_request(make_request(
            "tools/invoke",
            json!({"name": "NonexistentTool", "input": "{}"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(err.code, ErrorCode::ToolError);
        }
        other => panic!("expected ToolError, got: {other:?}"),
    }
}

#[tokio::test]
async fn tools_invoke_glob_requires_permission() {
    let app = test_app("tools-invoke-glob").await;
    let resp = app
        .handle_request(make_request(
            "tools/invoke",
            json!({"name": "Glob", "input": "{\"pattern\": \"*.nonexistent-xyz\"}"}),
        ))
        .await;
    match &resp.result {
        ResponseResult::Error(err) => {
            assert_eq!(err.code, ErrorCode::ToolError);
            assert!(
                err.message.contains("permission"),
                "error should mention permissions, got: {:?}",
                err.message
            );
        }
        other => panic!("expected ToolError, got: {other:?}"),
    }
}
