use std::sync::Arc;

use agent_client_protocol::Result;
use agent_client_protocol::schema::{
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption, SessionMode,
    SessionModeState, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    SetSessionModeRequest, SetSessionModeResponse,
};
use orbcode_app_server_client::{ClientError, SessionControlState};
use orbcode_app_server_protocol::{ErrorCode, PermissionMode};
use orbcode_protocol::EffortLevel;

use super::{AcpSdkState, internal_error, invalid_params};

pub(super) const MODEL_CONFIG_ID: &str = "model";
pub(super) const THOUGHT_LEVEL_CONFIG_ID: &str = "thought_level";
const DEFAULT_CONFIG_VALUE: &str = "default";

pub(super) fn acp_mode_state(state: &SessionControlState) -> SessionModeState {
    SessionModeState::new(
        acp_mode_id(state.permission_mode),
        vec![
            SessionMode::new("default", "Default")
                .description("Ask before protected tool or network operations."),
            SessionMode::new("plan", "Plan")
                .description("Plan without exposing mutation or network tools to the model."),
        ],
    )
}

pub(super) fn acp_config_options(state: &SessionControlState) -> Vec<SessionConfigOption> {
    let current_model = state
        .model_options
        .iter()
        .find(|option| option.current)
        .and_then(|option| option.value.clone())
        .unwrap_or_else(|| DEFAULT_CONFIG_VALUE.to_string());
    let model_options = state
        .model_options
        .iter()
        .map(|option| {
            SessionConfigSelectOption::new(
                option
                    .value
                    .clone()
                    .unwrap_or_else(|| DEFAULT_CONFIG_VALUE.to_string()),
                option.label.clone(),
            )
            .description(option.description.clone())
        })
        .collect::<Vec<_>>();
    let mut options = vec![
        SessionConfigOption::select(MODEL_CONFIG_ID, "Model", current_model, model_options)
            .category(SessionConfigOptionCategory::Model),
    ];

    if !state.effort_options.is_empty() {
        let mut effort_options = vec![
            SessionConfigSelectOption::new(DEFAULT_CONFIG_VALUE, "Default")
                .description("Use the model's default thought level."),
        ];
        effort_options.extend(state.effort_options.iter().map(|effort| {
            SessionConfigSelectOption::new(effort.as_str(), effort_label(*effort))
                .description(effort.description())
        }));
        options.push(
            SessionConfigOption::select(
                THOUGHT_LEVEL_CONFIG_ID,
                "Thought level",
                state
                    .effort_level
                    .map(EffortLevel::as_str)
                    .unwrap_or(DEFAULT_CONFIG_VALUE),
                effort_options,
            )
            .category(SessionConfigOptionCategory::ThoughtLevel),
        );
    }

    options
}

pub(super) async fn control_state(
    state: &AcpSdkState,
    session_id: &str,
) -> std::result::Result<SessionControlState, agent_client_protocol::Error> {
    state
        .client
        .session_control_state(session_id)
        .await
        .map_err(|error| control_error("read session controls", error, session_id))
}

pub(super) async fn handle_set_mode(
    state: Arc<AcpSdkState>,
    request: SetSessionModeRequest,
    responder: agent_client_protocol::Responder<SetSessionModeResponse>,
) -> Result<()> {
    let session_id = request.session_id.to_string();
    let mode_id = request.mode_id.to_string();
    if let Err(error) = ensure_adapter_control_mutable(&state.sessions, &session_id).await {
        return responder.respond_with_error(error);
    }
    let Some(mode) = permission_mode_from_acp(&mode_id) else {
        return responder.respond_with_error(invalid_params(format!(
            "unsupported session mode id: {mode_id}"
        )));
    };
    match state
        .client
        .set_session_permission_mode(&session_id, mode)
        .await
    {
        Ok(_) => responder.respond(SetSessionModeResponse::new()),
        Err(error) => {
            responder.respond_with_error(control_error("set session mode", error, &session_id))
        }
    }
}

pub(super) async fn handle_set_config_option(
    state: Arc<AcpSdkState>,
    request: SetSessionConfigOptionRequest,
    responder: agent_client_protocol::Responder<SetSessionConfigOptionResponse>,
) -> Result<()> {
    let session_id = request.session_id.to_string();
    let config_id = request.config_id.to_string();
    let value = request.value.to_string();
    if let Err(error) = ensure_adapter_control_mutable(&state.sessions, &session_id).await {
        return responder.respond_with_error(error);
    }
    let result = match config_id.as_str() {
        MODEL_CONFIG_ID => {
            let model = (value != DEFAULT_CONFIG_VALUE).then_some(value);
            state.client.set_session_model(&session_id, model).await
        }
        THOUGHT_LEVEL_CONFIG_ID => {
            let effort = if value == DEFAULT_CONFIG_VALUE {
                None
            } else {
                let Some(effort) = EffortLevel::parse(&value) else {
                    return responder.respond_with_error(invalid_params(format!(
                        "unsupported thought level value: {value}"
                    )));
                };
                Some(effort)
            };
            state.client.set_session_effort(&session_id, effort).await
        }
        _ => {
            return responder.respond_with_error(invalid_params(format!(
                "unknown session config option id: {config_id}"
            )));
        }
    };

    match result {
        Ok(control_state) => responder.respond(SetSessionConfigOptionResponse::new(
            acp_config_options(&control_state),
        )),
        Err(error) => responder.respond_with_error(control_error(
            "set session config option",
            error,
            &session_id,
        )),
    }
}

async fn ensure_adapter_control_mutable(
    sessions: &tokio::sync::Mutex<std::collections::HashMap<String, super::AcpSessionState>>,
    session_id: &str,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let sessions = sessions.lock().await;
    let Some(session) = sessions.get(session_id) else {
        return Err(invalid_params(format!(
            "unknown ACP session_id: {session_id}"
        )));
    };
    if session.active_prompt_generation.is_some() {
        return Err(invalid_params(format!(
            "session {session_id} has an active prompt; change controls after it completes or is cancelled"
        )));
    }
    Ok(())
}

fn acp_mode_id(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "default",
        PermissionMode::Plan => "plan",
        PermissionMode::BypassPermissions | PermissionMode::Auto => "default",
    }
}

fn permission_mode_from_acp(mode_id: &str) -> Option<PermissionMode> {
    match mode_id {
        "default" => Some(PermissionMode::Default),
        "plan" => Some(PermissionMode::Plan),
        _ => None,
    }
}

fn effort_label(effort: EffortLevel) -> &'static str {
    match effort {
        EffortLevel::Low => "Low",
        EffortLevel::Medium => "Medium",
        EffortLevel::High => "High",
        EffortLevel::Max => "Max",
        _ => "Custom",
    }
}

fn control_error(
    operation: &str,
    error: ClientError,
    session_id: &str,
) -> agent_client_protocol::Error {
    match error {
        ClientError::Protocol(protocol_error)
            if matches!(
                protocol_error.code,
                ErrorCode::SessionNotFound
                    | ErrorCode::ActiveTurn
                    | ErrorCode::ConfigError
                    | ErrorCode::InvalidParams
                    | ErrorCode::PermissionDenied
            ) =>
        {
            invalid_params(format!(
                "{operation} failed for session {session_id}: {}",
                protocol_error.message
            ))
        }
        other => internal_error(format!("{operation} failed: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use orbcode_app_server_protocol::{
        EffectiveModelSelection, ModelSelectionSource, PermissionMode, PersistedModelSetting,
        ProviderModelSelection, RuntimeModelOverride, SessionModelOption,
    };
    use tokio::sync::Mutex;

    use super::*;

    fn state() -> SessionControlState {
        SessionControlState {
            session_id: "session".to_string(),
            permission_mode: PermissionMode::Plan,
            active_permission_preset: None,
            permission_presets: Vec::new(),
            model_selection: EffectiveModelSelection {
                persisted: PersistedModelSetting {
                    value: None,
                    source: None,
                    locked: false,
                },
                runtime_override: RuntimeModelOverride::Model("sonnet".to_string()),
                requested_model: Some("sonnet".to_string()),
                source: ModelSelectionSource::Runtime,
                provider: orbcode_protocol::ProviderId::Anthropic,
                resolution: ProviderModelSelection {
                    requested_setting: Some("sonnet".to_string()),
                    family: Some("sonnet".to_string()),
                    model: "claude-sonnet-4-6".to_string(),
                    request_model: "claude-sonnet-4-6".to_string(),
                    display_label: "Sonnet".to_string(),
                    display_name: "Sonnet".to_string(),
                    capabilities: Vec::new(),
                },
            },
            model_options: vec![
                SessionModelOption {
                    value: None,
                    label: "Default".to_string(),
                    description: "Configured default".to_string(),
                    current: false,
                },
                SessionModelOption {
                    value: Some("sonnet".to_string()),
                    label: "Sonnet".to_string(),
                    description: "Balanced".to_string(),
                    current: true,
                },
            ],
            effort_level: Some(EffortLevel::High),
            effort_options: vec![EffortLevel::Low, EffortLevel::High],
        }
    }

    #[test]
    fn projects_stable_modes_without_bypass() {
        let modes = acp_mode_state(&state());
        assert_eq!(modes.current_mode_id.to_string(), "plan");
        let ids = modes
            .available_modes
            .iter()
            .map(|mode| mode.id.to_string())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["default", "plan"]);
        assert!(!ids.contains(&"bypass_permissions".to_string()));
    }

    #[test]
    fn removed_mode_ids_are_rejected() {
        assert_eq!(permission_mode_from_acp("accept_edits"), None);
        assert_eq!(permission_mode_from_acp("dont_ask"), None);
    }

    #[test]
    fn projects_model_and_thought_options() {
        let options = serde_json::to_value(acp_config_options(&state())).expect("serialize");
        assert_eq!(options[0]["id"], MODEL_CONFIG_ID);
        assert_eq!(options[0]["currentValue"], "sonnet");
        assert_eq!(options[0]["category"], "model");
        assert_eq!(options[1]["id"], THOUGHT_LEVEL_CONFIG_ID);
        assert_eq!(options[1]["currentValue"], "high");
        assert_eq!(options[1]["category"], "thought_level");
    }

    #[tokio::test]
    async fn mutable_control_check_releases_sessions_lock_before_returning() {
        let sessions = Mutex::new(HashMap::from([(
            "session".to_string(),
            super::super::AcpSessionState::default(),
        )]));

        ensure_adapter_control_mutable(&sessions, "session")
            .await
            .expect("session controls are mutable");

        let _sessions = sessions
            .try_lock()
            .expect("mutable check must release the sessions lock before AppClient awaits");
    }
}
