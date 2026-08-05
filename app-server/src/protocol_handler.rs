mod auth;
mod background;
mod diagnostics;
mod mcp;
pub(crate) mod permissions;
mod sessions;
mod settings;
mod tools;
pub(crate) mod turns;
mod workflow;

use orbcode_app_server_protocol::{
    ClientRequestEnvelope, ErrorCode, InitializeResult, ProtocolError, ResponseResult,
    ServerCapabilities, ServerInfo, ServerResponseEnvelope, method,
};
use orbcode_core::CoreError;

use crate::AppServer;

impl AppServer {
    /// Dispatch a protocol request directly, bypassing `MessageProcessor`.
    ///
    /// **Not a protocol entry point.** The canonical path goes through
    /// [`MessageProcessor`](crate::message_processor::MessageProcessor),
    /// which enforces initialize gating, capability filtering, and turn_submit
    /// handling before delegating here. Direct callers bypass those checks.
    pub(crate) async fn handle_request(
        &self,
        request: ClientRequestEnvelope,
    ) -> ServerResponseEnvelope {
        let result = self.dispatch(&request.method, request.params).await;
        ServerResponseEnvelope {
            id: request.id,
            result,
        }
    }

    async fn dispatch(&self, method: &str, params: Option<serde_json::Value>) -> ResponseResult {
        match method {
            // Lifecycle
            method::INITIALIZE => self.handle_initialize(params),
            // Session
            method::SESSION_BOOTSTRAP => self.handle_session_bootstrap(params).await,
            method::SESSION_LIST => self.handle_session_list(params).await,
            method::SESSION_RENAME => self.handle_session_rename(params).await,
            method::SESSION_FORK => self.handle_session_fork(params).await,
            method::SESSION_CLEAR => self.handle_session_clear(params).await,
            method::SESSION_REWIND => self.handle_session_rewind(params).await,
            method::SESSION_RECORD_MESSAGE => self.handle_session_record_message(params).await,
            method::SESSION_COMPACT => self.handle_session_compact(params).await,
            method::SESSION_COMPACT_DECISION => self.handle_session_compact_decision(params).await,
            method::SESSION_FIND_BY_TITLE => self.handle_session_find_by_title(params).await,
            method::SESSION_ACP_LOAD_PREFLIGHT => {
                self.handle_session_acp_load_preflight(params).await
            }
            method::SESSION_ACP_LOAD_SETUP => self.handle_session_acp_load_setup(params).await,
            method::SESSION_ACP_RESUME_SETUP => self.handle_session_acp_resume_setup(params).await,
            method::SESSION_ACP_DELETE => self.handle_session_acp_delete(params).await,
            method::SESSION_ACP_CLOSE => self.handle_session_acp_close(params).await,
            // Turn
            method::TURN_SUBMIT => self.handle_turn_submit(params).await,
            method::TURN_STEER => self.handle_turn_steer(params).await,
            method::TURN_CANCEL => self.handle_turn_cancel(params).await,
            method::TURN_INTERRUPT => self.handle_turn_interrupt(params).await,
            // Permission
            method::PERMISSION_RESPOND => self.handle_permission_respond(params).await,
            method::PERMISSION_OVERVIEW => self.handle_permission_overview(params).await,
            method::PERMISSION_MODE => self.handle_permission_mode(params),
            method::PERMISSION_SET_MODE => self.handle_permission_set_mode(params),
            method::PERMISSION_ADD_RULE => self.handle_permission_add_rule(params).await,
            method::PERMISSION_REMOVE_RULE => self.handle_permission_remove_rule(params).await,
            method::PERMISSION_ADD_SESSION_RULE => {
                self.handle_permission_add_session_rule(params).await
            }
            method::PERMISSION_REMOVE_SESSION_RULE => {
                self.handle_permission_remove_session_rule(params).await
            }
            method::PERMISSION_ADD_DIRECTORY => self.handle_permission_add_directory(params).await,
            method::PERMISSION_VALIDATE_DIRECTORY => {
                self.handle_permission_validate_directory(params).await
            }
            // Settings
            method::SETTINGS_MODEL_NAME => self.handle_settings_model_name(params),
            method::SETTINGS_MODEL_OPTIONS => self.handle_settings_model_options(params),
            method::SETTINGS_SET_MODEL => self.handle_settings_set_model(params).await,
            method::SETTINGS_PROVIDERS => self.handle_settings_providers(params),
            method::SETTINGS_THEME => self.handle_settings_theme(params),
            method::SETTINGS_SET_THEME => self.handle_settings_set_theme(params).await,
            method::SETTINGS_EFFORT => self.handle_settings_effort(params),
            method::SETTINGS_SET_EFFORT => self.handle_settings_set_effort(params).await,
            method::SETTINGS_OUTPUT_STYLE => self.handle_settings_output_style(params).await,
            method::SETTINGS_SET_OUTPUT_STYLE => {
                self.handle_settings_set_output_style(params).await
            }
            method::SETTINGS_SANDBOX => self.handle_settings_sandbox(params).await,
            method::SETTINGS_UPDATE_SANDBOX => self.handle_settings_update_sandbox(params).await,
            method::SETTINGS_KEYBINDINGS => self.handle_settings_keybindings(params).await,
            method::SETTINGS_LOAD_KEYBINDINGS => self.handle_settings_load_keybindings(params),
            method::SETTINGS_EDITOR_MODE => self.handle_settings_editor_mode(params),
            method::SETTINGS_SET_EDITOR_MODE => self.handle_settings_set_editor_mode(params).await,
            method::SETTINGS_OUTPUT_STYLE_OPTIONS => {
                self.handle_settings_output_style_options(params).await
            }
            method::SETTINGS_ACTIVE_OUTPUT_STYLE => {
                self.handle_settings_active_output_style(params)
            }
            method::SETTINGS_IS_LOCKED => self.handle_settings_is_locked(params),
            method::SETTINGS_SET_AUTO_MEMORY => self.handle_settings_set_auto_memory(params).await,
            method::SETTINGS_ENSURE_MEMORY_FILE => {
                self.handle_settings_ensure_memory_file(params).await
            }
            method::SETTINGS_ADD_SANDBOX_EXCLUDED => {
                self.handle_settings_add_sandbox_excluded(params).await
            }
            method::SETTINGS_ALLOW_ALL => self.handle_settings_allow_all(params),
            method::SETTINGS_SET_ALLOW_ALL => self.handle_settings_set_allow_all(params),
            // Context / Usage
            method::CONTEXT_PREVIEW => self.handle_context_preview(params).await,
            method::CONTEXT_OVERVIEW => self.handle_context_overview(params).await,
            method::USAGE_OVERVIEW => self.handle_usage_overview(params).await,
            method::USAGE_COST => self.handle_usage_cost(params).await,
            method::USAGE_STATS => self.handle_usage_stats(params).await,
            // MCP
            method::MCP_LIST_SERVERS => self.handle_mcp_list_servers(params).await,
            method::MCP_SERVER_TRUST => self.handle_mcp_server_trust(params).await,
            method::MCP_SET_TRUST => self.handle_mcp_set_trust(params).await,
            method::MCP_LIST_TOOLS => self.handle_mcp_list_tools(params).await,
            method::MCP_LIST_RESOURCES => self.handle_mcp_list_resources(params).await,
            method::MCP_READ_RESOURCE => self.handle_mcp_read_resource(params).await,
            method::MCP_LIST_PROMPTS => self.handle_mcp_list_prompts(params).await,
            method::MCP_GET_PROMPT => self.handle_mcp_get_prompt(params).await,
            method::MCP_INVOKE_TOOL => self.handle_mcp_invoke_tool(params).await,
            method::MCP_DIAGNOSE => self.handle_mcp_diagnose(params).await,
            method::MCP_UPSERT_SERVER => self.handle_mcp_upsert_server(params).await,
            method::MCP_REMOVE_SERVER => self.handle_mcp_remove_server(params).await,
            method::MCP_CAPABILITIES => self.handle_mcp_capabilities(params).await,
            method::MCP_SLASH_SUGGESTIONS => self.handle_mcp_slash_suggestions(params).await,
            method::MCP_OAUTH_OVERVIEW => self.handle_mcp_oauth_overview(params).await,
            method::MCP_LOGOUT_OAUTH_TOKEN => self.handle_mcp_logout_oauth_token(params).await,
            // Tools
            method::TOOLS_LIST => self.handle_tools_list(params),
            method::TOOLS_INVOKE => self.handle_tools_invoke(params).await,
            method::TOOLS_SKILLS => self.handle_tools_skills(params).await,
            method::TOOLS_AGENTS => self.handle_tools_agents(params),
            method::TOOLS_PLAN => self.handle_tools_plan(params).await,
            method::TOOLS_TASK_LIST => self.handle_tools_task_list(params).await,
            method::TOOLS_ENTER_PLAN => self.handle_tools_enter_plan(params).await,
            method::TOOLS_AGENTS_WITH_WARNINGS => {
                self.handle_tools_agents_with_warnings(params).await
            }
            // Background
            method::BACKGROUND_CREATE => self.handle_background_create(params).await,
            method::BACKGROUND_LIST => self.handle_background_list(params).await,
            method::BACKGROUND_DETAIL => self.handle_background_detail(params).await,
            method::BACKGROUND_CANCEL => self.handle_background_cancel(params).await,
            method::BACKGROUND_LOG => self.handle_background_log(params).await,
            method::BACKGROUND_EVENTS => self.handle_background_events(params).await,
            method::BACKGROUND_LIST_SUMMARY => self.handle_background_list_summary(params).await,
            // Workflows
            method::WORKFLOW_LIST => self.handle_workflow_list(params).await,
            method::WORKFLOW_START => self.handle_workflow_start(params).await,
            method::WORKFLOW_START_DYNAMIC => self.handle_workflow_start_dynamic(params).await,
            method::WORKFLOW_RESUME => self.handle_workflow_resume(params).await,
            // Auth
            method::AUTH_OVERVIEW => self.handle_auth_overview(params).await,
            method::AUTH_LOGIN => self.handle_auth_login(params).await,
            method::AUTH_LOGOUT => self.handle_auth_logout(params).await,
            // Diagnostics
            method::DIAGNOSTICS_STATUS => self.handle_diagnostics_status(params).await,
            method::DIAGNOSTICS_MEMORY => self.handle_diagnostics_memory(params).await,
            method::DIAGNOSTICS_DOCTOR => self.handle_diagnostics_doctor(params).await,
            method::DIAGNOSTICS_HOOKS => self.handle_diagnostics_hooks(params).await,
            method::DIAGNOSTICS_DIFF => self.handle_diagnostics_diff(params).await,
            method::DIAGNOSTICS_ADVANCED => self.handle_diagnostics_advanced(params),
            method::DIAGNOSTICS_CLEANUP_CHILD_SESSIONS => {
                self.handle_diagnostics_cleanup_child_sessions(params).await
            }
            method::DIAGNOSTICS_LAST_REQUEST => self.handle_diagnostics_last_request(params).await,
            method::DIAGNOSTICS_PRE_USER_INSTRUCTIONS => {
                self.handle_diagnostics_pre_user_instructions(params).await
            }
            _ => ResponseResult::Error(ProtocolError {
                code: ErrorCode::MethodNotFound,
                message: format!("unknown method: {method}"),
                data: None,
            }),
        }
    }

    fn handle_initialize(&self, _params: Option<serde_json::Value>) -> ResponseResult {
        let to_strings = |v: Vec<&str>| v.into_iter().map(String::from).collect();
        success(InitializeResult {
            protocol_version: "1.0".to_string(),
            server_info: ServerInfo {
                name: "orbcode".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            capabilities: ServerCapabilities {
                streaming: true,
                stable_methods: to_strings(method::stable_client_request_methods()),
                experimental_methods: to_strings(method::experimental_client_request_methods()),
                server_notification_methods: to_strings(method::server_notification_methods()),
                server_request_methods: to_strings(method::server_request_methods()),
            },
        })
    }
}

/// Parse `params` into the requested type, returning an `InvalidParams`
/// error variant on failure.
pub(crate) fn parse_params<T: serde::de::DeserializeOwned>(
    params: Option<serde_json::Value>,
) -> Result<T, ProtocolError> {
    let value = params.unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    serde_json::from_value(value).map_err(|e| ProtocolError {
        code: ErrorCode::InvalidParams,
        message: e.to_string(),
        data: None,
    })
}

/// Try to parse params; if parsing fails, return the error as a
/// `ResponseResult::Error` immediately.
macro_rules! try_parse {
    ($params:expr) => {
        match $crate::protocol_handler::parse_params($params) {
            Ok(v) => v,
            Err(e) => return orbcode_app_server_protocol::ResponseResult::Error(e),
        }
    };
}
pub(crate) use try_parse;

/// Convert a [`CoreError`] to a [`ResponseResult::Error`] with an
/// appropriate protocol error code.
pub(crate) fn core_error(err: CoreError) -> ResponseResult {
    let (code, message) = match &err {
        CoreError::SessionNotFound(id) => (
            ErrorCode::SessionNotFound,
            format!("session not found: {id}"),
        ),
        CoreError::ActiveTurn(_) => (ErrorCode::ActiveTurn, err.to_string()),
        CoreError::NoActiveTurn(_) => (ErrorCode::NoActiveTurn, err.to_string()),
        CoreError::PermissionDenied(_) => (ErrorCode::PermissionDenied, err.to_string()),
        CoreError::ProviderFailed(_) | CoreError::RetryExhausted(_) => {
            (ErrorCode::ProviderFailed, err.to_string())
        }
        CoreError::Config(_) => (ErrorCode::ConfigError, err.to_string()),
        CoreError::Tool(_) | CoreError::ToolErr(_) => (ErrorCode::ToolError, err.to_string()),
        CoreError::Mcp(_) => (ErrorCode::McpError, err.to_string()),
        _ => (ErrorCode::InternalError, err.to_string()),
    };
    ResponseResult::Error(ProtocolError {
        code,
        message,
        data: None,
    })
}

/// Wrap a serialisable value in a successful response.
pub(crate) fn success<T: serde::Serialize>(value: T) -> ResponseResult {
    ResponseResult::Success {
        data: Some(serde_json::to_value(value).unwrap_or(serde_json::Value::Null)),
    }
}

/// A successful response with no data payload.
fn success_empty() -> ResponseResult {
    ResponseResult::Success { data: None }
}

/// Build an `InvalidParams` error response.
fn invalid_params(message: impl Into<String>) -> ResponseResult {
    ResponseResult::Error(ProtocolError {
        code: ErrorCode::InvalidParams,
        message: message.into(),
        data: None,
    })
}

#[cfg(test)]
mod e2e_tests;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use orbcode_app_server_protocol::{
        ClientRequestEnvelope, ErrorCode, InitializeResult, ResponseResult,
    };
    use orbcode_config::AppConfigOverrides;
    use serde_json::json;

    use super::super::AppServer;

    fn test_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "orbcode-protocol-handler-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    async fn test_app(label: &str) -> (AppServer, PathBuf, PathBuf) {
        let home = test_path(&format!("{label}-home"));
        let cwd = test_path(&format!("{label}-cwd"));
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        let app = AppServer::new(
            cwd.clone(),
            AppConfigOverrides {
                home_dir: Some(home.clone()),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");
        (app, home, cwd)
    }

    #[tokio::test]
    async fn initialize_returns_valid_result() {
        let (app, _home, _cwd) = test_app("init").await;
        let resp = app
            .handle_request(ClientRequestEnvelope {
                id: "req-1".into(),
                method: "initialize".into(),
                params: Some(json!({
                    "protocol_version": "1.0",
                    "client_info": { "name": "test", "version": "0.1" },
                })),
            })
            .await;
        assert_eq!(resp.id, "req-1");
        match resp.result {
            ResponseResult::Success { data: Some(data) } => {
                let init: InitializeResult =
                    serde_json::from_value(data).expect("deserialize InitializeResult");
                assert_eq!(init.protocol_version, "1.0");
                assert_eq!(init.server_info.name, "orbcode");
                assert!(init.capabilities.streaming);
                assert!(!init.capabilities.stable_methods.is_empty());
            }
            other => panic!("expected Success with data, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let (app, _home, _cwd) = test_app("unknown").await;
        let resp = app
            .handle_request(ClientRequestEnvelope {
                id: "req-2".into(),
                method: "nonexistent/method".into(),
                params: None,
            })
            .await;
        match resp.result {
            ResponseResult::Error(err) => {
                assert_eq!(err.code, ErrorCode::MethodNotFound);
                assert!(err.message.contains("nonexistent/method"));
            }
            other => panic!("expected Error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_required_params_returns_invalid_params() {
        let (app, _home, _cwd) = test_app("invalid-params").await;
        // session/rename requires session_id and new_title
        let resp = app
            .handle_request(ClientRequestEnvelope {
                id: "req-3".into(),
                method: "session/rename".into(),
                params: None,
            })
            .await;
        match resp.result {
            ResponseResult::Error(err) => {
                assert_eq!(err.code, ErrorCode::InvalidParams);
            }
            other => panic!("expected InvalidParams error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn session_list_returns_empty_for_fresh_server() {
        let (app, _home, _cwd) = test_app("list-empty").await;
        let resp = app
            .handle_request(ClientRequestEnvelope {
                id: "req-4".into(),
                method: "session/list".into(),
                params: None,
            })
            .await;
        match resp.result {
            ResponseResult::Success { data: Some(data) } => {
                let sessions: Vec<serde_json::Value> =
                    serde_json::from_value(data).expect("deserialize session list");
                assert!(sessions.is_empty());
            }
            other => panic!("expected empty session list, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn session_bootstrap_creates_session() {
        let (app, _home, _cwd) = test_app("bootstrap").await;
        let resp = app
            .handle_request(ClientRequestEnvelope {
                id: "req-5".into(),
                method: "session/bootstrap".into(),
                params: None,
            })
            .await;
        match resp.result {
            ResponseResult::Success { data: Some(data) } => {
                let obj = data.as_object().expect("expected object");
                assert!(obj.contains_key("session"), "bootstrap must return session");
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    #[test]
    fn core_error_maps_all_variants() {
        use orbcode_core::CoreError;
        use orbcode_core::ProviderFailure;
        use orbcode_mcp::McpError;
        use orbcode_tools::ToolError;

        use super::core_error;

        let cases: Vec<(CoreError, ErrorCode)> = vec![
            (
                CoreError::SessionNotFound("s1".into()),
                ErrorCode::SessionNotFound,
            ),
            (CoreError::ActiveTurn("s1".into()), ErrorCode::ActiveTurn),
            (
                CoreError::NoActiveTurn("s1".into()),
                ErrorCode::NoActiveTurn,
            ),
            (
                CoreError::PermissionDenied("denied".into()),
                ErrorCode::PermissionDenied,
            ),
            (
                CoreError::ProviderFailed(ProviderFailure::from_message("fail")),
                ErrorCode::ProviderFailed,
            ),
            (
                CoreError::RetryExhausted(ProviderFailure::from_message("exhausted")),
                ErrorCode::ProviderFailed,
            ),
            (
                CoreError::Config("bad config".into()),
                ErrorCode::ConfigError,
            ),
            (CoreError::Tool("tool err".into()), ErrorCode::ToolError),
            (
                CoreError::ToolErr(ToolError::NotFound("x".into())),
                ErrorCode::ToolError,
            ),
            (
                CoreError::Mcp(McpError::UnknownServer("x".into())),
                ErrorCode::McpError,
            ),
            (
                CoreError::Io(std::io::Error::other("io")),
                ErrorCode::InternalError,
            ),
            (
                CoreError::Json(serde_json::from_str::<()>("bad").unwrap_err()),
                ErrorCode::InternalError,
            ),
        ];

        for (err, expected_code) in cases {
            let variant_name = format!("{err:?}").split('(').next().unwrap().to_string();
            match core_error(err) {
                ResponseResult::Error(e) => {
                    assert_eq!(
                        e.code, expected_code,
                        "CoreError::{variant_name} should map to {expected_code:?}"
                    );
                }
                other => {
                    panic!("CoreError::{variant_name} should produce Error, got: {other:?}")
                }
            }
        }
    }
}
