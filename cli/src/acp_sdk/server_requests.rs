use std::sync::Arc;

use agent_client_protocol::schema::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionId,
};
use agent_client_protocol::{Client, ConnectionTo};
use orbcode_app_server_protocol::{
    AskUserQuestionRequest, AskUserQuestionResponse, McpTrustDecisionWire, PermissionDecisionWire,
    PermissionResponseParams, RequestId, ResponseResult, ServerRequestEnvelope, method,
};
use orbcode_protocol::{McpTrustApprovalRequest, PermissionRequest};
use tokio::sync::{mpsc, oneshot};

use super::tool_updates::tool_call_update;
use super::{
    ACP_ALLOW_ONCE, ACP_ASK_OPTION_PREFIX, ACP_DENY_MCP, ACP_DENY_ONCE, ACP_TRUST_MCP, AcpSdkState,
    PendingServerRequest,
};

pub(super) async fn server_request_pump(
    mut rx: mpsc::Receiver<ServerRequestEnvelope>,
    state: Arc<AcpSdkState>,
    connection: ConnectionTo<Client>,
) {
    while let Some(envelope) = rx.recv().await {
        match envelope.method.as_str() {
            method::SERVER_REQUEST_PERMISSION => {
                // Detached request bridge; pending-request state owns
                // cancellation and fallback response cleanup.
                let _permission_request_handle = tokio::spawn(handle_permission_request(
                    envelope,
                    Arc::clone(&state),
                    connection.clone(),
                ));
            }
            method::SERVER_REQUEST_MCP_TRUST => {
                // Detached request bridge; pending-request state owns
                // cancellation and fallback response cleanup.
                let _mcp_trust_request_handle = tokio::spawn(handle_mcp_trust_request(
                    envelope,
                    Arc::clone(&state),
                    connection.clone(),
                ));
            }
            method::SERVER_REQUEST_ASK_USER => {
                // Detached request bridge; pending-request state owns
                // cancellation and fallback response cleanup.
                let _ask_user_request_handle = tokio::spawn(handle_ask_user_request(
                    envelope,
                    Arc::clone(&state),
                    connection.clone(),
                ));
            }
            other => {
                tracing::warn!(method = other, "unsupported ACP server-request");
            }
        }
    }
}

async fn handle_permission_request(
    envelope: ServerRequestEnvelope,
    state: Arc<AcpSdkState>,
    connection: ConnectionTo<Client>,
) {
    let Ok(request) = serde_json::from_value::<PermissionRequest>(envelope.params.clone()) else {
        tracing::warn!("invalid permission server-request payload for ACP adapter");
        return;
    };

    let (pending_session_id, mut cancel_rx) = remember_pending_server_request(
        &state,
        Some(request.session_id.clone()),
        envelope.id.clone(),
        deny_permission_result(request.request_id.clone()),
    )
    .await;

    let response = tokio::select! {
        response = connection.send_request(permission_request_to_acp(&request)).block_task() => response,
        _ = &mut cancel_rx => return,
    };
    if !take_pending_server_request(&state, pending_session_id.as_deref(), &envelope.id).await {
        return;
    }
    let decision = response.map_or(PermissionDecisionWire::Deny, permission_decision_from_acp);

    let data = serde_json::to_value(PermissionResponseParams {
        request_id: request.request_id,
        decision,
    })
    .ok();
    let _ = state
        .client
        .respond_to_server_request(envelope.id.clone(), ResponseResult::Success { data })
        .await;
}

async fn handle_mcp_trust_request(
    envelope: ServerRequestEnvelope,
    state: Arc<AcpSdkState>,
    connection: ConnectionTo<Client>,
) {
    let Ok(request) = serde_json::from_value::<McpTrustApprovalRequest>(envelope.params.clone())
    else {
        tracing::warn!("invalid MCP trust server-request payload for ACP adapter");
        return;
    };

    let (pending_session_id, mut cancel_rx) = remember_pending_server_request(
        &state,
        Some(request.session_id.clone()),
        envelope.id.clone(),
        deny_mcp_trust_result(),
    )
    .await;

    let response = tokio::select! {
        response = connection.send_request(mcp_trust_request_to_acp(&request)).block_task() => response,
        _ = &mut cancel_rx => return,
    };
    if !take_pending_server_request(&state, pending_session_id.as_deref(), &envelope.id).await {
        return;
    }
    let decision = response.map_or(McpTrustDecisionWire::Deny, mcp_trust_decision_from_acp);

    let data = serde_json::to_value(decision).ok();
    let _ = state
        .client
        .respond_to_server_request(envelope.id.clone(), ResponseResult::Success { data })
        .await;
}

async fn handle_ask_user_request(
    envelope: ServerRequestEnvelope,
    state: Arc<AcpSdkState>,
    connection: ConnectionTo<Client>,
) {
    let Ok(request) = serde_json::from_value::<AskUserQuestionRequest>(envelope.params.clone())
    else {
        tracing::warn!("invalid AskUserQuestion server-request payload for ACP adapter");
        return;
    };

    let Some(session_id) = ask_user_session_id(&state, &request).await else {
        respond_to_ask_user_request(&state, envelope.id, request.request_id, None).await;
        return;
    };

    let (pending_session_id, mut cancel_rx) = remember_pending_server_request(
        &state,
        Some(session_id.clone()),
        envelope.id.clone(),
        cancel_ask_user_result(request.request_id.clone()),
    )
    .await;

    if request.options.is_empty() {
        if take_pending_server_request(&state, pending_session_id.as_deref(), &envelope.id).await {
            respond_to_ask_user_request(&state, envelope.id.clone(), request.request_id, None)
                .await;
        }
        return;
    }

    let response = tokio::select! {
        response = connection.send_request(ask_user_request_to_acp(&session_id, &request)).block_task() => response,
        _ = &mut cancel_rx => return,
    };
    if !take_pending_server_request(&state, pending_session_id.as_deref(), &envelope.id).await {
        return;
    }
    let answer = response
        .ok()
        .and_then(|response| ask_user_answer_from_acp(&request, response));

    respond_to_ask_user_request(&state, envelope.id.clone(), request.request_id, answer).await;
}

async fn ask_user_session_id(
    state: &AcpSdkState,
    request: &AskUserQuestionRequest,
) -> Option<String> {
    if !request.session_id.is_empty() {
        let sessions = state.sessions.lock().await;
        if sessions.contains_key(&request.session_id) {
            return Some(request.session_id.clone());
        }
        tracing::warn!(
            session_id = request.session_id,
            "AskUser server-request referenced an unknown ACP session"
        );
        return None;
    }

    active_ask_user_session_id(state).await
}

async fn active_ask_user_session_id(state: &AcpSdkState) -> Option<String> {
    let sessions = state.sessions.lock().await;
    let mut active = sessions
        .iter()
        .filter(|(_, session)| session.active_prompt_generation.is_some())
        .map(|(session_id, _)| session_id.clone());
    let session_id = active.next()?;
    if active.next().is_none() {
        Some(session_id)
    } else {
        None
    }
}

async fn respond_to_ask_user_request(
    state: &AcpSdkState,
    envelope_id: RequestId,
    request_id: String,
    answer: Option<String>,
) {
    let data = serde_json::to_value(AskUserQuestionResponse { request_id, answer }).ok();
    let _ = state
        .client
        .respond_to_server_request(envelope_id, ResponseResult::Success { data })
        .await;
}

fn permission_request_to_acp(request: &PermissionRequest) -> RequestPermissionRequest {
    RequestPermissionRequest::new(
        SessionId::new(request.session_id.clone()),
        tool_call_update(
            &request.tool_use_id,
            request.summary(),
            &request.tool_name,
            &request.tool_input,
        ),
        vec![
            PermissionOption::new(
                ACP_ALLOW_ONCE,
                "Allow once",
                PermissionOptionKind::AllowOnce,
            ),
            PermissionOption::new(ACP_DENY_ONCE, "Reject", PermissionOptionKind::RejectOnce),
        ],
    )
}

fn mcp_trust_request_to_acp(request: &McpTrustApprovalRequest) -> RequestPermissionRequest {
    RequestPermissionRequest::new(
        SessionId::new(request.session_id.clone()),
        tool_call_update(
            &format!("mcp-trust-{}", request.server_id),
            format!(
                "Trust MCP server {} for tool {}",
                request.server_id, request.tool_name
            ),
            "mcp_trust",
            &serde_json::json!({
                "server_id": request.server_id,
                "tool_name": request.tool_name,
            })
            .to_string(),
        ),
        vec![
            PermissionOption::new(
                ACP_TRUST_MCP,
                "Trust server",
                PermissionOptionKind::AllowOnce,
            ),
            PermissionOption::new(
                ACP_DENY_MCP,
                "Reject server",
                PermissionOptionKind::RejectOnce,
            ),
        ],
    )
}

fn ask_user_request_to_acp(
    session_id: &str,
    request: &AskUserQuestionRequest,
) -> RequestPermissionRequest {
    let raw_input = serde_json::json!({
        "question": &request.question,
        "options": &request.options,
    })
    .to_string();

    RequestPermissionRequest::new(
        SessionId::new(session_id.to_string()),
        tool_call_update(
            &request.request_id,
            request.question.clone(),
            "AskUserQuestion",
            &raw_input,
        ),
        request
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                PermissionOption::new(
                    ask_user_option_id(index),
                    option.clone(),
                    PermissionOptionKind::AllowOnce,
                )
            })
            .collect(),
    )
}

fn permission_decision_from_acp(response: RequestPermissionResponse) -> PermissionDecisionWire {
    match response.outcome {
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome { option_id, .. }) => {
            match option_id.to_string().as_str() {
                ACP_ALLOW_ONCE => PermissionDecisionWire::Approve,
                _ => PermissionDecisionWire::Deny,
            }
        }
        _ => PermissionDecisionWire::Deny,
    }
}

fn mcp_trust_decision_from_acp(response: RequestPermissionResponse) -> McpTrustDecisionWire {
    match response.outcome {
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome { option_id, .. }) => {
            match option_id.to_string().as_str() {
                ACP_TRUST_MCP => McpTrustDecisionWire::Trust,
                _ => McpTrustDecisionWire::Deny,
            }
        }
        _ => McpTrustDecisionWire::Deny,
    }
}

fn ask_user_answer_from_acp(
    request: &AskUserQuestionRequest,
    response: RequestPermissionResponse,
) -> Option<String> {
    match response.outcome {
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome { option_id, .. }) => {
            ask_user_option_index(&option_id.to_string())
                .and_then(|index| request.options.get(index).cloned())
        }
        _ => None,
    }
}

fn ask_user_option_id(index: usize) -> String {
    format!("{ACP_ASK_OPTION_PREFIX}{index}")
}

fn ask_user_option_index(option_id: &str) -> Option<usize> {
    option_id.strip_prefix(ACP_ASK_OPTION_PREFIX)?.parse().ok()
}

pub(super) async fn remember_pending_server_request(
    state: &AcpSdkState,
    session_id: Option<String>,
    id: RequestId,
    result: ResponseResult,
) -> (Option<String>, oneshot::Receiver<()>) {
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let Some(session_id) = session_id else {
        return (None, cancel_rx);
    };
    state
        .pending_server_requests
        .lock()
        .await
        .entry(session_id.clone())
        .or_default()
        .push(PendingServerRequest {
            id,
            result,
            cancel_tx,
        });
    (Some(session_id), cancel_rx)
}

pub(super) async fn take_pending_server_request(
    state: &AcpSdkState,
    session_id: Option<&str>,
    request_id: &str,
) -> bool {
    let Some(session_id) = session_id else {
        return true;
    };
    let mut pending = state.pending_server_requests.lock().await;
    let Some(requests) = pending.get_mut(session_id) else {
        return false;
    };
    let Some(index) = requests.iter().position(|request| request.id == request_id) else {
        return false;
    };
    requests.swap_remove(index);
    if requests.is_empty() {
        pending.remove(session_id);
    }
    true
}

pub(super) async fn deny_pending_server_requests(state: &AcpSdkState, session_id: &str) {
    let pending = state
        .pending_server_requests
        .lock()
        .await
        .remove(session_id)
        .unwrap_or_default();

    for PendingServerRequest {
        id,
        result,
        cancel_tx,
    } in pending
    {
        let _ = cancel_tx.send(());
        let _ = state.client.respond_to_server_request(id, result).await;
    }
}

fn deny_permission_result(request_id: String) -> ResponseResult {
    ResponseResult::Success {
        data: serde_json::to_value(PermissionResponseParams {
            request_id,
            decision: PermissionDecisionWire::Deny,
        })
        .ok(),
    }
}

fn deny_mcp_trust_result() -> ResponseResult {
    ResponseResult::Success {
        data: serde_json::to_value(McpTrustDecisionWire::Deny).ok(),
    }
}

fn cancel_ask_user_result(request_id: String) -> ResponseResult {
    ResponseResult::Success {
        data: serde_json::to_value(AskUserQuestionResponse {
            request_id,
            answer: None,
        })
        .ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::{ToolCallStatus, ToolKind};
    use serde_json::json;

    #[test]
    fn permission_request_maps_to_acp_request_permission_shape() {
        let request = PermissionRequest {
            request_id: "perm-1".into(),
            session_id: "session-1".into(),
            tool_use_id: "toolu-1".into(),
            tool_name: "Bash".into(),
            tool_input: r#"{"command":"echo hi"}"#.into(),
            requires_tools_permission: true,
            requires_network_permission: false,
        };

        let acp = permission_request_to_acp(&request);

        assert_eq!(acp.session_id.to_string(), "session-1");
        assert_eq!(acp.tool_call.tool_call_id.to_string(), "toolu-1");
        assert_eq!(acp.tool_call.fields.kind, Some(ToolKind::Execute));
        assert_eq!(acp.tool_call.fields.status, Some(ToolCallStatus::Pending));
        assert_eq!(
            acp.tool_call.fields.raw_input,
            Some(json!({"command":"echo hi"}))
        );
        assert_eq!(
            acp.options
                .iter()
                .map(|option| option.option_id.to_string())
                .collect::<Vec<_>>(),
            vec![ACP_ALLOW_ONCE, ACP_DENY_ONCE]
        );
    }

    #[test]
    fn mcp_trust_request_maps_real_session_id_to_acp_request_permission_shape() {
        let request = McpTrustApprovalRequest {
            request_id: "trust-1".into(),
            session_id: "session-2".into(),
            server_id: "docs-server".into(),
            tool_name: "echo".into(),
        };

        let acp = mcp_trust_request_to_acp(&request);

        assert_eq!(acp.session_id.to_string(), "session-2");
        assert_eq!(
            acp.tool_call.tool_call_id.to_string(),
            "mcp-trust-docs-server"
        );
        assert_eq!(acp.tool_call.fields.kind, Some(ToolKind::Other));
        assert_eq!(acp.tool_call.fields.status, Some(ToolCallStatus::Pending));
        assert_eq!(
            acp.options
                .iter()
                .map(|option| option.option_id.to_string())
                .collect::<Vec<_>>(),
            vec![ACP_TRUST_MCP, ACP_DENY_MCP]
        );
    }

    #[test]
    fn acp_permission_outcome_maps_to_wire_decision() {
        let allow = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(ACP_ALLOW_ONCE),
        ));
        let deny = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(ACP_DENY_ONCE),
        ));
        let cancelled = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);

        assert_eq!(
            permission_decision_from_acp(allow),
            PermissionDecisionWire::Approve
        );
        assert_eq!(
            permission_decision_from_acp(deny),
            PermissionDecisionWire::Deny
        );
        assert_eq!(
            permission_decision_from_acp(cancelled),
            PermissionDecisionWire::Deny
        );
    }

    #[test]
    fn acp_mcp_trust_outcome_maps_to_wire_decision() {
        let trust = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(ACP_TRUST_MCP),
        ));
        let deny = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(ACP_DENY_MCP),
        ));

        assert_eq!(
            mcp_trust_decision_from_acp(trust),
            McpTrustDecisionWire::Trust
        );
        assert_eq!(
            mcp_trust_decision_from_acp(deny),
            McpTrustDecisionWire::Deny
        );
    }

    #[test]
    fn ask_user_options_map_to_acp_request_permission_shape() {
        let request = AskUserQuestionRequest {
            session_id: "session-ask".into(),
            request_id: "ask-1".into(),
            question: "Pick a color".into(),
            options: vec!["red".into(), "blue".into()],
        };

        let acp = ask_user_request_to_acp("session-ask", &request);

        assert_eq!(acp.session_id.to_string(), "session-ask");
        assert_eq!(acp.tool_call.tool_call_id.to_string(), "ask-1");
        assert_eq!(acp.tool_call.fields.kind, Some(ToolKind::Other));
        assert_eq!(acp.tool_call.fields.status, Some(ToolCallStatus::Pending));
        assert_eq!(acp.tool_call.fields.title, Some("Pick a color".into()));
        assert_eq!(
            acp.tool_call.fields.raw_input,
            Some(json!({
                "question": "Pick a color",
                "options": ["red", "blue"],
            }))
        );
        assert_eq!(
            acp.options
                .iter()
                .map(|option| (option.option_id.to_string(), option.name.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("ask_user_option_0".to_string(), "red".to_string()),
                ("ask_user_option_1".to_string(), "blue".to_string()),
            ]
        );
    }

    #[test]
    fn acp_ask_user_selected_option_maps_to_answer() {
        let request = AskUserQuestionRequest {
            session_id: "session-ask".into(),
            request_id: "ask-2".into(),
            question: "Pick a color".into(),
            options: vec!["red".into(), "blue".into()],
        };
        let response = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new("ask_user_option_1"),
        ));

        assert_eq!(
            ask_user_answer_from_acp(&request, response),
            Some("blue".to_string())
        );
    }

    #[test]
    fn acp_ask_user_cancelled_or_unknown_option_maps_to_none() {
        let request = AskUserQuestionRequest {
            session_id: "session-ask".into(),
            request_id: "ask-3".into(),
            question: "Pick a color".into(),
            options: vec!["red".into(), "blue".into()],
        };
        let cancelled = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
        let unknown = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new("ask_user_option_99"),
        ));

        assert_eq!(ask_user_answer_from_acp(&request, cancelled), None);
        assert_eq!(ask_user_answer_from_acp(&request, unknown), None);
    }
}
