//! Wire-shape fixture tests that pin the exact JSON structure of protocol
//! messages. These tests guard against accidental breaking changes to the
//! serialisation format that downstream consumers (TUI, CLI, future IPC
//! clients) depend on.

use orbcode_app_server_protocol::{
    ClientCapabilities, ClientInfo, ClientMessage, ClientPreferences, ClientRequestEnvelope,
    EditorModeSetting, EffectivePermissionRules, ErrorCode, InitializeParams, InitializeResult,
    PermissionRuleEffect, PermissionRuleKind, PermissionRuleParams, PermissionRuleTargetScope,
    ProtocolError, ResponseResult, RuntimeModelOverride, ServerCapabilities, ServerInfo,
    ServerMessage, ServerNotificationEnvelope, ServerRequestEnvelope, ServerRequestResponse,
    ServerResponseEnvelope, SessionGoalContinueResult, SessionGoalNotStartedReason,
    SessionGoalSetParams, SessionPermissionRuleParams, SetSessionModelParams, StatuslineConfig,
    StreamEventNotification, ThemeSetting,
};
use orbcode_protocol::StreamEvent;
use serde_json::{Value, json};

#[derive(serde::Serialize, serde::Deserialize)]
struct TypedSettingsWireFixture {
    theme: ThemeSetting,
    editor_mode: EditorModeSetting,
    #[serde(flatten)]
    statusline: StatuslineConfig,
}

#[test]
fn typed_settings_wire_spelling_and_flattening_are_pinned() {
    let value = serde_json::to_value(TypedSettingsWireFixture {
        theme: ThemeSetting::DarkDaltonized,
        editor_mode: EditorModeSetting::Vim,
        statusline: StatuslineConfig {
            command: Some("echo ready".to_string()),
            refresh_interval_secs: 15,
        },
    })
    .expect("serialize typed settings");
    assert_eq!(
        value,
        json!({
            "theme": "dark-daltonized",
            "editor_mode": "vim",
            "statusline_command": "echo ready",
            "statusline_refresh_interval_secs": 15
        })
    );
    let decoded: TypedSettingsWireFixture =
        serde_json::from_value(value).expect("deserialize typed settings");
    assert_eq!(decoded.theme, ThemeSetting::DarkDaltonized);
    assert_eq!(decoded.editor_mode, EditorModeSetting::Vim);

    // Bootstrap keeps its pre-typed PascalCase compatibility spelling even
    // though settings methods use canonical lower/kebab-case enum values.
    assert_eq!(
        serde_json::to_value(ClientPreferences {
            theme: ThemeSetting::DarkDaltonized,
            editor_mode: EditorModeSetting::Vim,
        })
        .expect("serialize bootstrap preferences"),
        json!({"theme": "DarkDaltonized", "editor_mode": "Vim"})
    );
}

#[test]
fn model_override_and_permission_mutation_intents_are_typed() {
    assert_eq!(
        serde_json::to_value(RuntimeModelOverride::Model("sonnet".to_string()))
            .expect("serialize model override"),
        json!({"kind": "model", "model": "sonnet"})
    );
    let selected = SetSessionModelParams::select("session-1", Some("sonnet".to_string()));
    assert_eq!(
        serde_json::to_value(&selected).expect("serialize selected model"),
        json!({"session_id": "session-1", "model": "sonnet"})
    );
    assert_eq!(
        selected.selection(),
        Ok(RuntimeModelOverride::Model("sonnet".to_string()))
    );
    let provider_default = SetSessionModelParams::select("session-1", None);
    assert_eq!(
        serde_json::to_value(&provider_default).expect("serialize provider default"),
        json!({"session_id": "session-1", "model": null})
    );
    assert_eq!(
        provider_default.selection(),
        Ok(RuntimeModelOverride::Default)
    );
    let inherited = SetSessionModelParams::inherit("session-1");
    assert_eq!(
        serde_json::to_value(&inherited).expect("serialize inherit"),
        json!({"session_id": "session-1", "model": null, "inherit": true})
    );
    assert_eq!(inherited.selection(), Ok(RuntimeModelOverride::Inherit));

    let contradictory: SetSessionModelParams = serde_json::from_value(json!({
        "session_id": "session-1",
        "model": "sonnet",
        "inherit": true,
    }))
    .expect("deserialize contradictory session model intent");
    assert_eq!(
        contradictory.selection(),
        Err("`inherit` cannot be true when `model` is non-null")
    );

    let settings = PermissionRuleParams {
        kind: PermissionRuleKind::Allow,
        rule: "Read(src/**)".to_string(),
    };
    assert_eq!(settings.target_scope(), PermissionRuleTargetScope::Settings);
    assert_eq!(
        serde_json::to_value(&settings).expect("serialize settings rule"),
        json!({"kind": "allow", "rule": "Read(src/**)"})
    );
    let session = SessionPermissionRuleParams {
        session_id: "session-1".to_string(),
        kind: PermissionRuleKind::Deny,
        rule: "Bash(rm:*)".to_string(),
    };
    assert_eq!(session.target_scope(), PermissionRuleTargetScope::Session);
    assert_eq!(
        serde_json::to_value(&session).expect("serialize session rule"),
        json!({"session_id": "session-1", "kind": "deny", "rule": "Bash(rm:*)"})
    );

    let effective = EffectivePermissionRules {
        precedence: vec![
            PermissionRuleEffect::Deny,
            PermissionRuleEffect::Ask,
            PermissionRuleEffect::Allow,
        ],
        ..EffectivePermissionRules::default()
    };
    assert_eq!(
        serde_json::to_value(effective).expect("serialize permission projection")["precedence"],
        json!(["deny", "ask", "allow"])
    );
}

// ---------------------------------------------------------------------------
// 1. ClientMessage::Request wire shape
// ---------------------------------------------------------------------------

#[test]
fn client_request_wire_shape() {
    let msg = ClientMessage::Request(ClientRequestEnvelope {
        id: "req-1".to_string(),
        method: "session/list".to_string(),
        params: None,
    });
    let value = serde_json::to_value(&msg).unwrap();
    assert_eq!(value["type"], "request");
    assert_eq!(value["id"], "req-1");
    assert_eq!(value["method"], "session/list");
    // params should be absent when None (skip_serializing_if)
    assert!(
        value.get("params").is_none() || value["params"].is_null(),
        "params should be absent or null when None"
    );
}

#[test]
fn client_request_with_params_wire_shape() {
    let msg = ClientMessage::Request(ClientRequestEnvelope {
        id: "req-2".to_string(),
        method: "turn/submit".to_string(),
        params: Some(json!({"prompt": "hello"})),
    });
    let value = serde_json::to_value(&msg).unwrap();
    assert_eq!(value["type"], "request");
    assert_eq!(value["id"], "req-2");
    assert_eq!(value["method"], "turn/submit");
    assert_eq!(value["params"]["prompt"], "hello");
}

// ---------------------------------------------------------------------------
// 2. ClientMessage::Response wire shape
// ---------------------------------------------------------------------------

#[test]
fn client_response_wire_shape() {
    let msg = ClientMessage::Response(ServerRequestResponse {
        id: "srv-req-1".to_string(),
        result: ResponseResult::Success {
            data: Some(json!(true)),
        },
    });
    let value = serde_json::to_value(&msg).unwrap();
    assert_eq!(value["type"], "response");
    assert_eq!(value["id"], "srv-req-1");
    assert_eq!(value["result"]["status"], "success");
    assert_eq!(value["result"]["data"], json!(true));
}

// ---------------------------------------------------------------------------
// 3. ServerMessage::Response wire shape -- success
// ---------------------------------------------------------------------------

#[test]
fn server_response_success_wire_shape() {
    let msg = ServerMessage::Response(ServerResponseEnvelope {
        id: "req-1".to_string(),
        result: ResponseResult::Success {
            data: Some(json!({"sessions": []})),
        },
    });
    let value = serde_json::to_value(&msg).unwrap();
    assert_eq!(value["type"], "response");
    assert_eq!(value["id"], "req-1");
    assert_eq!(value["result"]["status"], "success");
    assert_eq!(value["result"]["data"]["sessions"], json!([]));
}

#[test]
fn server_response_success_no_data_wire_shape() {
    let msg = ServerMessage::Response(ServerResponseEnvelope {
        id: "req-2".to_string(),
        result: ResponseResult::Success { data: None },
    });
    let value = serde_json::to_value(&msg).unwrap();
    assert_eq!(value["type"], "response");
    assert_eq!(value["result"]["status"], "success");
    // data should be absent when None
    assert!(
        value["result"].get("data").is_none() || value["result"]["data"].is_null(),
        "data should be absent or null when None"
    );
}

// ---------------------------------------------------------------------------
// 4. ServerMessage::Response wire shape -- error
// ---------------------------------------------------------------------------

#[test]
fn server_response_error_wire_shape() {
    let msg = ServerMessage::Response(ServerResponseEnvelope {
        id: "req-3".to_string(),
        result: ResponseResult::Error(ProtocolError {
            code: ErrorCode::MethodNotFound,
            message: "unknown method: foo/bar".to_string(),
            data: None,
        }),
    });
    let value = serde_json::to_value(&msg).unwrap();
    assert_eq!(value["type"], "response");
    assert_eq!(value["id"], "req-3");
    assert_eq!(value["result"]["status"], "error");
    assert_eq!(value["result"]["code"], "method_not_found");
    assert_eq!(value["result"]["message"], "unknown method: foo/bar");
}

#[test]
fn server_response_error_with_data_wire_shape() {
    let msg = ServerMessage::Response(ServerResponseEnvelope {
        id: "req-4".to_string(),
        result: ResponseResult::Error(ProtocolError {
            code: ErrorCode::InvalidParams,
            message: "missing field".to_string(),
            data: Some(json!({"field": "session_id"})),
        }),
    });
    let value = serde_json::to_value(&msg).unwrap();
    assert_eq!(value["result"]["status"], "error");
    assert_eq!(value["result"]["code"], "invalid_params");
    assert_eq!(value["result"]["data"]["field"], "session_id");
}

// ---------------------------------------------------------------------------
// 5. ServerMessage::Notification wire shape
// ---------------------------------------------------------------------------

#[test]
fn server_notification_wire_shape() {
    let msg = ServerMessage::Notification(ServerNotificationEnvelope {
        method: "stream/event".to_string(),
        params: json!({"event": "session_started"}),
    });
    let value = serde_json::to_value(&msg).unwrap();
    assert_eq!(value["type"], "notification");
    assert_eq!(value["method"], "stream/event");
    assert_eq!(value["params"]["event"], "session_started");
}

// ---------------------------------------------------------------------------
// 6. ServerMessage::Request wire shape
// ---------------------------------------------------------------------------

#[test]
fn server_request_wire_shape() {
    let msg = ServerMessage::Request(ServerRequestEnvelope {
        id: "srv-req-42".to_string(),
        method: "permission/request".to_string(),
        params: json!({"tool_name": "bash", "command": "ls -la"}),
    });
    let value = serde_json::to_value(&msg).unwrap();
    assert_eq!(value["type"], "request");
    assert_eq!(value["id"], "srv-req-42");
    assert_eq!(value["method"], "permission/request");
    assert_eq!(value["params"]["tool_name"], "bash");
}

// ---------------------------------------------------------------------------
// 7. ErrorCode snake_case serialisation for all variants
// ---------------------------------------------------------------------------

#[test]
fn error_code_all_variants_snake_case() {
    let expected = vec![
        (ErrorCode::ParseError, "parse_error"),
        (ErrorCode::InvalidRequest, "invalid_request"),
        (ErrorCode::MethodNotFound, "method_not_found"),
        (ErrorCode::InvalidParams, "invalid_params"),
        (ErrorCode::InternalError, "internal_error"),
        (ErrorCode::SessionNotFound, "session_not_found"),
        (ErrorCode::ActiveTurn, "active_turn"),
        (ErrorCode::NoActiveTurn, "no_active_turn"),
        (ErrorCode::PermissionDenied, "permission_denied"),
        (ErrorCode::ProviderFailed, "provider_failed"),
        (ErrorCode::ConfigError, "config_error"),
        (ErrorCode::ToolError, "tool_error"),
        (ErrorCode::McpError, "mcp_error"),
    ];
    for (code, expected_str) in expected {
        let value = serde_json::to_value(code).unwrap();
        assert_eq!(
            value,
            json!(expected_str),
            "ErrorCode::{code:?} should serialize to \"{expected_str}\""
        );
    }
}

// ---------------------------------------------------------------------------
// 8. ProtocolError envelope shape
// ---------------------------------------------------------------------------

#[test]
fn protocol_error_wire_shape_without_data() {
    let err = ProtocolError {
        code: ErrorCode::InternalError,
        message: "something broke".to_string(),
        data: None,
    };
    let value = serde_json::to_value(&err).unwrap();
    assert_eq!(value["code"], "internal_error");
    assert_eq!(value["message"], "something broke");
    // data should be absent when None
    assert!(
        value.get("data").is_none() || value["data"].is_null(),
        "data should be absent when None"
    );
}

#[test]
fn protocol_error_wire_shape_with_data() {
    let err = ProtocolError {
        code: ErrorCode::InvalidParams,
        message: "bad input".to_string(),
        data: Some(json!({"details": ["missing field x"]})),
    };
    let value = serde_json::to_value(&err).unwrap();
    assert_eq!(value["code"], "invalid_params");
    assert_eq!(value["message"], "bad input");
    assert_eq!(value["data"]["details"][0], "missing field x");
}

// ---------------------------------------------------------------------------
// 9. InitializeParams wire shape
// ---------------------------------------------------------------------------

#[test]
fn initialize_params_wire_shape() {
    let params = InitializeParams {
        protocol_version: "1.0".to_string(),
        client_info: ClientInfo {
            name: "test-client".to_string(),
            version: "0.1.0".to_string(),
        },
        capabilities: ClientCapabilities {
            streaming: true,
            ..Default::default()
        },
    };
    let value = serde_json::to_value(&params).unwrap();
    assert_eq!(value["protocol_version"], "1.0");
    assert_eq!(value["client_info"]["name"], "test-client");
    assert_eq!(value["client_info"]["version"], "0.1.0");
    assert_eq!(value["capabilities"]["streaming"], true);
}

#[test]
fn initialize_params_default_capabilities_wire_shape() {
    let params = InitializeParams {
        protocol_version: "1.0".to_string(),
        client_info: ClientInfo {
            name: "c".to_string(),
            version: "0.1".to_string(),
        },
        capabilities: ClientCapabilities::default(),
    };
    let value = serde_json::to_value(&params).unwrap();
    assert_eq!(value["capabilities"]["streaming"], false);
    assert_eq!(value["capabilities"]["experimental_methods"], false);
    assert_eq!(value["capabilities"]["persistent_goals"], false);
}

#[test]
fn initialize_params_experimental_opt_in_wire_shape() {
    let raw = json!({
        "protocol_version": "1.0",
        "client_info": { "name": "ext", "version": "0.1" },
        "capabilities": { "streaming": true, "experimental_methods": true }
    });
    let params: InitializeParams = serde_json::from_value(raw).unwrap();
    assert!(params.capabilities.experimental_methods);
    let serialized = serde_json::to_value(&params).unwrap();
    assert_eq!(serialized["capabilities"]["experimental_methods"], true);
}

#[test]
fn initialize_params_persistent_goal_opt_in_wire_shape() {
    let raw = json!({
        "protocol_version": "1.0",
        "client_info": { "name": "goal-client", "version": "0.1" },
        "capabilities": {
            "streaming": true,
            "experimental_methods": true,
            "persistent_goals": true
        }
    });
    let params: InitializeParams = serde_json::from_value(raw).unwrap();
    assert!(params.capabilities.persistent_goals);
}

#[test]
fn session_goal_set_distinguishes_missing_and_null_budget() {
    let keep: SessionGoalSetParams = serde_json::from_value(json!({
        "session_id": "session-1",
        "expected_revision": 3
    }))
    .unwrap();
    assert_eq!(keep.token_budget, None);

    let clear: SessionGoalSetParams = serde_json::from_value(json!({
        "session_id": "session-1",
        "expected_revision": 3,
        "token_budget": null
    }))
    .unwrap();
    assert_eq!(clear.token_budget, Some(None));
    assert_eq!(
        serde_json::to_value(clear).unwrap()["token_budget"],
        Value::Null
    );
}

#[test]
fn session_goal_continue_not_started_is_tagged() {
    let result = SessionGoalContinueResult::NotStarted {
        reason: SessionGoalNotStartedReason::StaleRevision,
        goal: None,
    };
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({
            "outcome": "not_started",
            "reason": "stale_revision",
            "goal": null
        })
    );
}

// ---------------------------------------------------------------------------
// 10. InitializeResult wire shape
// ---------------------------------------------------------------------------

#[test]
fn initialize_result_wire_shape() {
    let result = InitializeResult {
        protocol_version: "1.0".to_string(),
        server_info: ServerInfo {
            name: "orbcode".to_string(),
            version: "0.2.0".to_string(),
        },
        capabilities: ServerCapabilities {
            streaming: true,
            stable_methods: vec![
                "initialize".to_string(),
                "session/list".to_string(),
                "turn/submit".to_string(),
            ],
            experimental_methods: vec!["background/create".to_string()],
            server_notification_methods: vec!["stream/event".to_string()],
            server_request_methods: vec!["permission/request".to_string()],
        },
    };
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["protocol_version"], "1.0");
    assert_eq!(value["server_info"]["name"], "orbcode");
    assert_eq!(value["server_info"]["version"], "0.2.0");
    assert_eq!(value["capabilities"]["streaming"], true);
    let stable = value["capabilities"]["stable_methods"]
        .as_array()
        .expect("stable_methods should be array");
    assert_eq!(stable.len(), 3);
    assert_eq!(stable[0], "initialize");
    assert_eq!(stable[1], "session/list");
    assert_eq!(stable[2], "turn/submit");
    let experimental = value["capabilities"]["experimental_methods"]
        .as_array()
        .expect("experimental_methods should be array");
    assert_eq!(experimental.len(), 1);
    assert_eq!(experimental[0], "background/create");
    let notif_methods = value["capabilities"]["server_notification_methods"]
        .as_array()
        .expect("server_notification_methods should be array");
    assert_eq!(notif_methods.len(), 1);
    assert_eq!(notif_methods[0], "stream/event");
    let srv_req_methods = value["capabilities"]["server_request_methods"]
        .as_array()
        .expect("server_request_methods should be array");
    assert_eq!(srv_req_methods.len(), 1);
    assert_eq!(srv_req_methods[0], "permission/request");
}

// ---------------------------------------------------------------------------
// 11. StreamEventNotification wire shape
// ---------------------------------------------------------------------------

#[test]
fn stream_event_notification_wire_shape() {
    let notif = StreamEventNotification {
        subscription_id: "sub-42".to_string(),
        event: StreamEvent::Error {
            session_id: Some("sess-1".to_string()),
            provider: None,
            category: None,
            message: "test error".to_string(),
            suggestion: None,
        },
    };
    let value = serde_json::to_value(&notif).unwrap();
    // Top level should have "subscription_id" and "event" keys
    assert_eq!(value["subscription_id"], "sub-42");
    assert!(value.get("event").is_some(), "should have event key");
    // The event should be internally tagged with "event" discriminator
    assert_eq!(value["event"]["event"], "error");
    assert_eq!(value["event"]["message"], "test error");
    assert_eq!(value["event"]["session_id"], "sess-1");
}

// ---------------------------------------------------------------------------
// 12. ResponseResult tagged union shape
// ---------------------------------------------------------------------------

#[test]
fn response_result_success_tagged_shape() {
    let r = ResponseResult::Success {
        data: Some(json!(42)),
    };
    let value = serde_json::to_value(&r).unwrap();
    assert_eq!(value["status"], "success");
    assert_eq!(value["data"], 42);
}

#[test]
fn response_result_error_tagged_shape() {
    let r = ResponseResult::Error(ProtocolError {
        code: ErrorCode::SessionNotFound,
        message: "not found".to_string(),
        data: None,
    });
    let value = serde_json::to_value(&r).unwrap();
    assert_eq!(value["status"], "error");
    assert_eq!(value["code"], "session_not_found");
    assert_eq!(value["message"], "not found");
}

// ---------------------------------------------------------------------------
// 13. Deserialization from canonical JSON
// ---------------------------------------------------------------------------

#[test]
fn client_message_deserializes_from_canonical_json() {
    let json_str = r#"{"type":"request","id":"r1","method":"session/list"}"#;
    let msg: ClientMessage = serde_json::from_str(json_str).unwrap();
    match msg {
        ClientMessage::Request(env) => {
            assert_eq!(env.id, "r1");
            assert_eq!(env.method, "session/list");
            assert!(env.params.is_none());
        }
        _ => panic!("expected Request variant"),
    }
}

#[test]
fn server_message_response_deserializes_from_canonical_json() {
    let json_str =
        r#"{"type":"response","id":"r1","result":{"status":"success","data":{"ok":true}}}"#;
    let msg: ServerMessage = serde_json::from_str(json_str).unwrap();
    match msg {
        ServerMessage::Response(env) => {
            assert_eq!(env.id, "r1");
            match env.result {
                ResponseResult::Success { data } => {
                    assert_eq!(data, Some(json!({"ok": true})));
                }
                _ => panic!("expected Success"),
            }
        }
        _ => panic!("expected Response"),
    }
}

#[test]
fn server_message_error_deserializes_from_canonical_json() {
    let json_str = r#"{"type":"response","id":"r2","result":{"status":"error","code":"method_not_found","message":"nope"}}"#;
    let msg: ServerMessage = serde_json::from_str(json_str).unwrap();
    match msg {
        ServerMessage::Response(env) => {
            assert_eq!(env.id, "r2");
            match env.result {
                ResponseResult::Error(err) => {
                    assert_eq!(err.code, ErrorCode::MethodNotFound);
                    assert_eq!(err.message, "nope");
                }
                _ => panic!("expected Error"),
            }
        }
        _ => panic!("expected Response"),
    }
}

// ===========================================================================
// DTO robustness tests
// ===========================================================================

// ---------------------------------------------------------------------------
// 14. Unknown fields are ignored during deserialization
// ---------------------------------------------------------------------------

#[test]
fn unknown_fields_ignored_in_request() {
    // ClientRequestEnvelope should silently ignore extra fields
    let json_str = r#"{
        "id": "req-extra",
        "method": "session/list",
        "params": null,
        "extra_field": "should be ignored",
        "another_unknown": 42
    }"#;
    let env: ClientRequestEnvelope = serde_json::from_str(json_str).unwrap();
    assert_eq!(env.id, "req-extra");
    assert_eq!(env.method, "session/list");
}

// ---------------------------------------------------------------------------
// 15. Missing optional params defaults to None
// ---------------------------------------------------------------------------

#[test]
fn missing_optional_params_defaults_to_null() {
    // When the "params" field is entirely absent, it should default to None
    let json_str = r#"{"id": "req-no-params", "method": "session/list"}"#;
    let env: ClientRequestEnvelope = serde_json::from_str(json_str).unwrap();
    assert_eq!(env.id, "req-no-params");
    assert_eq!(env.method, "session/list");
    assert!(
        env.params.is_none(),
        "params should be None when absent from JSON"
    );
}

// ---------------------------------------------------------------------------
// 16. ErrorCode round-trip for all variants
// ---------------------------------------------------------------------------

#[test]
fn error_code_round_trip_all_variants() {
    let variants_and_strings = [
        (ErrorCode::ParseError, "parse_error"),
        (ErrorCode::InvalidRequest, "invalid_request"),
        (ErrorCode::MethodNotFound, "method_not_found"),
        (ErrorCode::InvalidParams, "invalid_params"),
        (ErrorCode::InternalError, "internal_error"),
        (ErrorCode::SessionNotFound, "session_not_found"),
        (ErrorCode::ActiveTurn, "active_turn"),
        (ErrorCode::NoActiveTurn, "no_active_turn"),
        (ErrorCode::PermissionDenied, "permission_denied"),
        (ErrorCode::ProviderFailed, "provider_failed"),
        (ErrorCode::ConfigError, "config_error"),
        (ErrorCode::ToolError, "tool_error"),
        (ErrorCode::McpError, "mcp_error"),
    ];

    for (code, expected_str) in &variants_and_strings {
        // Serialize to string
        let serialized = serde_json::to_value(code).unwrap();
        assert_eq!(
            serialized,
            json!(expected_str),
            "ErrorCode::{code:?} should serialize to \"{expected_str}\""
        );

        // Deserialize back from the string
        let deserialized: ErrorCode =
            serde_json::from_value(json!(expected_str)).unwrap_or_else(|e| {
                panic!("failed to deserialize \"{expected_str}\" back to ErrorCode: {e}")
            });
        assert_eq!(
            *code, deserialized,
            "ErrorCode round-trip failed for \"{expected_str}\""
        );
    }
}

// ---------------------------------------------------------------------------
// 17. Method constants are stable (snapshot test)
// ---------------------------------------------------------------------------

#[test]
fn method_constants_are_stable() {
    use orbcode_app_server_protocol::method;

    let actual = method::all_methods();

    // Pin the exact list of all methods. If a method is added, removed, or
    // renamed, this test will fail, forcing an intentional update.
    let expected: Vec<&str> = vec![
        // Stable client request methods
        // Lifecycle
        "initialize",
        // Session
        "session/bootstrap",
        "session/list",
        "session/rename",
        "session/fork",
        "session/clear",
        "session/rewind",
        "session/record_message",
        "session/compact",
        "session/compact_decision",
        "session/find_by_title",
        // Turn
        "turn/submit",
        "turn/steer",
        "turn/cancel",
        "turn/interrupt",
        // Permission
        "permission/respond",
        "permission/overview",
        "permission/mode",
        "permission/set_mode",
        "permission/add_rule",
        "permission/remove_rule",
        "permission/add_session_rule",
        "permission/remove_session_rule",
        "permission/add_directory",
        "permission/validate_directory",
        // Settings
        "settings/model_name",
        "settings/model_options",
        "settings/set_model",
        "settings/set_thinking_budget",
        "settings/providers",
        "settings/theme",
        "settings/set_theme",
        "settings/effort",
        "settings/set_effort",
        "settings/output_style",
        "settings/set_output_style",
        "settings/sandbox",
        "settings/update_sandbox",
        "settings/keybindings",
        "settings/load_keybindings",
        "settings/editor_mode",
        "settings/set_editor_mode",
        "settings/output_style_options",
        "settings/active_output_style",
        "settings/is_locked",
        "settings/set_auto_memory",
        "settings/ensure_memory_file",
        "settings/add_sandbox_excluded",
        "settings/allow_all",
        "settings/set_allow_all",
        // Context / Usage
        "context/preview",
        "context/overview",
        "usage/overview",
        "usage/cost",
        "usage/stats",
        // MCP
        "mcp/list_servers",
        "mcp/status",
        "mcp/server_trust",
        "mcp/set_trust",
        "mcp/list_tools",
        "mcp/list_resources",
        "mcp/read_resource",
        "mcp/list_prompts",
        "mcp/get_prompt",
        "mcp/invoke_tool",
        "mcp/diagnose",
        "mcp/upsert_server",
        "mcp/remove_server",
        "mcp/capabilities",
        "mcp/slash_suggestions",
        "mcp/oauth_overview",
        "mcp/logout_oauth_token",
        // Tools
        "tools/list",
        "tools/invoke",
        "tools/skills",
        "tools/agents",
        "tools/plan",
        "tools/task_list",
        "tools/enter_plan",
        "tools/agents_with_warnings",
        "tools/seed_read_state",
        // Auth
        "auth/overview",
        "auth/login",
        "auth/logout",
        // Diagnostics
        "diagnostics/status",
        "diagnostics/memory",
        "diagnostics/doctor",
        "diagnostics/hooks",
        "diagnostics/diff",
        "diagnostics/advanced",
        "diagnostics/cleanup_child_sessions",
        "diagnostics/last_request",
        "diagnostics/pre_user_instructions",
        // Experimental client request methods
        // ACP session setup helpers
        "session/acp_load_preflight",
        "session/acp_load_setup",
        "session/acp_resume_setup",
        "session/acp_delete",
        "session/acp_close",
        "session/control_state",
        "session/set_permission_mode",
        "session/set_model",
        "session/set_effort",
        "session/goal/get",
        "session/goal/set",
        "session/goal/clear",
        "session/goal/continue",
        // Background
        "background/create",
        "background/list",
        "background/detail",
        "background/cancel",
        "background/log",
        "background/events",
        "background/list_summary",
        "background/subscribe",
        "background/cancel_async",
        // Workflows
        "workflow/list",
        "workflow/start",
        "workflow/start_dynamic",
        "workflow/resume",
        // Notifications (server -> client)
        "stream/event",
        // Server-initiated requests
        "permission/request",
        "mcp_trust/request",
        "ask_user/request",
    ];

    assert_eq!(
        actual, expected,
        "method list mismatch -- if this is intentional, update the expected list in this test"
    );
}

// ---------------------------------------------------------------------------
// 18. Method strings are pinned (prevents accidental renames)
// ---------------------------------------------------------------------------

#[test]
fn method_strings_pinned() {
    use orbcode_app_server_protocol::method;

    // Pin individual method constants to their exact string values.
    // If a constant is renamed this test will catch it at compile time.
    // If a constant's *value* changes, the assertion catches it at runtime.
    assert_eq!(method::INITIALIZE, "initialize");
    assert_eq!(method::SESSION_BOOTSTRAP, "session/bootstrap");
    assert_eq!(method::SESSION_LIST, "session/list");
    assert_eq!(method::SESSION_RENAME, "session/rename");
    assert_eq!(method::SESSION_FORK, "session/fork");
    assert_eq!(method::SESSION_CLEAR, "session/clear");
    assert_eq!(method::SESSION_REWIND, "session/rewind");
    assert_eq!(method::SESSION_CONTROL_STATE, "session/control_state");
    assert_eq!(
        method::SESSION_SET_PERMISSION_MODE,
        "session/set_permission_mode"
    );
    assert_eq!(method::SESSION_SET_MODEL, "session/set_model");
    assert_eq!(method::SESSION_SET_EFFORT, "session/set_effort");
    assert_eq!(method::SESSION_GOAL_GET, "session/goal/get");
    assert_eq!(method::SESSION_GOAL_SET, "session/goal/set");
    assert_eq!(method::SESSION_GOAL_CLEAR, "session/goal/clear");
    assert_eq!(method::SESSION_GOAL_CONTINUE, "session/goal/continue");
    assert_eq!(method::TURN_SUBMIT, "turn/submit");
    assert_eq!(method::TURN_STEER, "turn/steer");
    assert_eq!(method::TURN_CANCEL, "turn/cancel");
    assert_eq!(method::TURN_INTERRUPT, "turn/interrupt");
    assert_eq!(method::PERMISSION_RESPOND, "permission/respond");
    assert_eq!(method::PERMISSION_OVERVIEW, "permission/overview");
    assert_eq!(method::PERMISSION_MODE, "permission/mode");
    assert_eq!(method::PERMISSION_SET_MODE, "permission/set_mode");
    assert_eq!(method::PERMISSION_ADD_RULE, "permission/add_rule");
    assert_eq!(method::PERMISSION_REMOVE_RULE, "permission/remove_rule");
    assert_eq!(method::PERMISSION_ADD_DIRECTORY, "permission/add_directory");
    assert_eq!(
        method::PERMISSION_VALIDATE_DIRECTORY,
        "permission/validate_directory"
    );
    assert_eq!(method::SETTINGS_MODEL_NAME, "settings/model_name");
    assert_eq!(method::SETTINGS_MODEL_OPTIONS, "settings/model_options");
    assert_eq!(method::SETTINGS_SET_MODEL, "settings/set_model");
    assert_eq!(
        method::SETTINGS_SET_THINKING_BUDGET,
        "settings/set_thinking_budget"
    );
    assert_eq!(method::SETTINGS_PROVIDERS, "settings/providers");
    assert_eq!(method::SETTINGS_THEME, "settings/theme");
    assert_eq!(method::SETTINGS_SET_THEME, "settings/set_theme");
    assert_eq!(method::SETTINGS_EFFORT, "settings/effort");
    assert_eq!(method::SETTINGS_SET_EFFORT, "settings/set_effort");
    assert_eq!(method::SETTINGS_OUTPUT_STYLE, "settings/output_style");
    assert_eq!(
        method::SETTINGS_SET_OUTPUT_STYLE,
        "settings/set_output_style"
    );
    assert_eq!(method::SETTINGS_SANDBOX, "settings/sandbox");
    assert_eq!(method::SETTINGS_UPDATE_SANDBOX, "settings/update_sandbox");
    assert_eq!(method::SETTINGS_KEYBINDINGS, "settings/keybindings");
    assert_eq!(method::CONTEXT_PREVIEW, "context/preview");
    assert_eq!(method::CONTEXT_OVERVIEW, "context/overview");
    assert_eq!(method::USAGE_OVERVIEW, "usage/overview");
    assert_eq!(method::USAGE_COST, "usage/cost");
    assert_eq!(method::USAGE_STATS, "usage/stats");
    assert_eq!(method::MCP_LIST_SERVERS, "mcp/list_servers");
    assert_eq!(method::MCP_STATUS, "mcp/status");
    assert_eq!(method::MCP_SERVER_TRUST, "mcp/server_trust");
    assert_eq!(method::MCP_SET_TRUST, "mcp/set_trust");
    assert_eq!(method::MCP_LIST_TOOLS, "mcp/list_tools");
    assert_eq!(method::MCP_LIST_RESOURCES, "mcp/list_resources");
    assert_eq!(method::MCP_READ_RESOURCE, "mcp/read_resource");
    assert_eq!(method::MCP_LIST_PROMPTS, "mcp/list_prompts");
    assert_eq!(method::MCP_GET_PROMPT, "mcp/get_prompt");
    assert_eq!(method::MCP_INVOKE_TOOL, "mcp/invoke_tool");
    assert_eq!(method::MCP_DIAGNOSE, "mcp/diagnose");
    assert_eq!(method::MCP_UPSERT_SERVER, "mcp/upsert_server");
    assert_eq!(method::MCP_REMOVE_SERVER, "mcp/remove_server");
    assert_eq!(method::MCP_CAPABILITIES, "mcp/capabilities");
    assert_eq!(method::MCP_SLASH_SUGGESTIONS, "mcp/slash_suggestions");
    assert_eq!(method::TOOLS_LIST, "tools/list");
    assert_eq!(method::TOOLS_INVOKE, "tools/invoke");
    assert_eq!(method::TOOLS_SKILLS, "tools/skills");
    assert_eq!(method::TOOLS_AGENTS, "tools/agents");
    assert_eq!(method::TOOLS_PLAN, "tools/plan");
    assert_eq!(method::TOOLS_TASK_LIST, "tools/task_list");
    assert_eq!(method::TOOLS_SEED_READ_STATE, "tools/seed_read_state");
    assert_eq!(method::BACKGROUND_CREATE, "background/create");
    assert_eq!(method::BACKGROUND_LIST, "background/list");
    assert_eq!(method::BACKGROUND_DETAIL, "background/detail");
    assert_eq!(method::BACKGROUND_CANCEL, "background/cancel");
    assert_eq!(method::BACKGROUND_LOG, "background/log");
    assert_eq!(method::BACKGROUND_SUBSCRIBE, "background/subscribe");
    assert_eq!(method::BACKGROUND_CANCEL_ASYNC, "background/cancel_async");
    assert_eq!(method::AUTH_OVERVIEW, "auth/overview");
    assert_eq!(method::AUTH_LOGIN, "auth/login");
    assert_eq!(method::AUTH_LOGOUT, "auth/logout");
    assert_eq!(method::DIAGNOSTICS_STATUS, "diagnostics/status");
    assert_eq!(method::DIAGNOSTICS_MEMORY, "diagnostics/memory");
    assert_eq!(method::DIAGNOSTICS_DOCTOR, "diagnostics/doctor");
    assert_eq!(method::DIAGNOSTICS_HOOKS, "diagnostics/hooks");
    assert_eq!(method::DIAGNOSTICS_DIFF, "diagnostics/diff");
    assert_eq!(method::DIAGNOSTICS_ADVANCED, "diagnostics/advanced");
    assert_eq!(
        method::DIAGNOSTICS_CLEANUP_CHILD_SESSIONS,
        "diagnostics/cleanup_child_sessions"
    );
    assert_eq!(method::SESSION_RECORD_MESSAGE, "session/record_message");
    assert_eq!(method::BACKGROUND_EVENTS, "background/events");
    assert_eq!(method::NOTIFICATION_STREAM_EVENT, "stream/event");
    assert_eq!(method::SERVER_REQUEST_PERMISSION, "permission/request");
    assert_eq!(method::SERVER_REQUEST_MCP_TRUST, "mcp_trust/request");
    assert_eq!(method::SERVER_REQUEST_ASK_USER, "ask_user/request");
}

// ---------------------------------------------------------------------------
// 19. Server-request DTO wire shapes
// ---------------------------------------------------------------------------

#[test]
fn permission_decision_approve_wire_shape() {
    use orbcode_app_server_protocol::PermissionDecisionWire;
    let json = serde_json::to_value(PermissionDecisionWire::Approve).unwrap();
    assert_eq!(json, serde_json::json!({"decision": "approve"}));
}

#[test]
fn permission_decision_deny_wire_shape() {
    use orbcode_app_server_protocol::PermissionDecisionWire;
    let json = serde_json::to_value(PermissionDecisionWire::Deny).unwrap();
    assert_eq!(json, serde_json::json!({"decision": "deny"}));
}

#[test]
fn permission_decision_approve_always_wire_shape() {
    use orbcode_app_server_protocol::PermissionDecisionWire;
    let json = serde_json::to_value(PermissionDecisionWire::ApproveAlways {
        rules: vec!["Bash(npm test)".into()],
    })
    .unwrap();
    assert_eq!(
        json,
        serde_json::json!({"decision": "approve_always", "rules": ["Bash(npm test)"]})
    );
}

#[test]
fn permission_response_params_wire_shape() {
    use orbcode_app_server_protocol::{PermissionDecisionWire, PermissionResponseParams};
    let params = PermissionResponseParams {
        request_id: "perm-42".into(),
        decision: PermissionDecisionWire::Approve,
    };
    let json = serde_json::to_value(&params).unwrap();
    assert_eq!(json["request_id"], "perm-42");
    assert_eq!(json["decision"]["decision"], "approve");
}

#[test]
fn mcp_trust_decision_trust_wire_shape() {
    use orbcode_app_server_protocol::McpTrustDecisionWire;
    let json = serde_json::to_value(McpTrustDecisionWire::Trust).unwrap();
    assert_eq!(json, serde_json::json!("trust"));
}

#[test]
fn mcp_trust_decision_deny_wire_shape() {
    use orbcode_app_server_protocol::McpTrustDecisionWire;
    let json = serde_json::to_value(McpTrustDecisionWire::Deny).unwrap();
    assert_eq!(json, serde_json::json!("deny"));
}

#[test]
fn mcp_trust_response_params_wire_shape() {
    use orbcode_app_server_protocol::{McpTrustDecisionWire, McpTrustResponseParams};
    let params = McpTrustResponseParams {
        request_id: "mcp-trust-7".into(),
        decision: McpTrustDecisionWire::Trust,
    };
    let json = serde_json::to_value(&params).unwrap();
    assert_eq!(json["request_id"], "mcp-trust-7");
    assert_eq!(json["decision"], "trust");
}

// ---------------------------------------------------------------------------
// 20. Forward compatibility: extra fields in params are ignored
// ---------------------------------------------------------------------------

#[test]
fn initialize_params_extra_fields_ignored() {
    let raw = serde_json::json!({
        "protocol_version": "1.0",
        "client_info": { "name": "test", "version": "0.1" },
        "future_field": true,
        "another": [1, 2, 3]
    });
    let params: InitializeParams = serde_json::from_value(raw).unwrap();
    assert_eq!(params.protocol_version, "1.0");
    assert_eq!(params.client_info.name, "test");
}

#[test]
fn permission_response_params_extra_fields_ignored() {
    let raw = serde_json::json!({
        "request_id": "perm-1",
        "decision": { "decision": "approve" },
        "extra": "ignored"
    });
    let params: orbcode_app_server_protocol::PermissionResponseParams =
        serde_json::from_value(raw).unwrap();
    assert_eq!(params.request_id, "perm-1");
}

#[test]
fn mcp_trust_response_params_extra_fields_ignored() {
    let raw = serde_json::json!({
        "request_id": "mcp-1",
        "decision": "trust",
        "extra": 42
    });
    let params: orbcode_app_server_protocol::McpTrustResponseParams =
        serde_json::from_value(raw).unwrap();
    assert_eq!(params.request_id, "mcp-1");
}

#[test]
fn client_request_envelope_extra_fields_ignored() {
    let raw = serde_json::json!({
        "id": "req-1",
        "method": "session/list",
        "future_header": "v2"
    });
    let env: ClientRequestEnvelope = serde_json::from_value(raw).unwrap();
    assert_eq!(env.id, "req-1");
    assert_eq!(env.method, "session/list");
    assert!(env.params.is_none());
}
