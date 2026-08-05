use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_client_protocol::schema::{
    CloseSessionRequest, CloseSessionResponse, ContentBlock, DeleteSessionRequest,
    DeleteSessionResponse, EmbeddedResourceResource, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, ResumeSessionRequest, ResumeSessionResponse, SessionId, SessionInfo,
};
use agent_client_protocol::{Client, ConnectionTo, Result};
use orbcode_app_server_client::ClientError;
use orbcode_app_server_protocol::{AcpDeleteSessionParams, BootstrapParams, ProtocolError};
use orbcode_protocol::{SessionRecord, SessionStatus, SessionSummary};

use super::mcp_setup::acp_mcp_servers_to_configs;
use super::replay::replay_updates_for_session;
use super::server_requests::deny_pending_server_requests;
use super::tool_updates::send_session_update;
use super::{
    AcpSdkState, AcpSessionState, finish_prompt_generation, internal_error, invalid_params,
    start_server_request_pump,
};

pub(super) async fn handle_list_sessions(
    state: Arc<AcpSdkState>,
    request: ListSessionsRequest,
    responder: agent_client_protocol::Responder<ListSessionsResponse>,
) -> Result<()> {
    let cwd_filter = match requested_session_list_cwd(&request, &state.launch_cwd) {
        Ok(cwd) => cwd,
        Err(err) => return responder.respond_with_error(err),
    };
    if request.cursor.is_some() {
        return responder.respond_with_error(invalid_params(
            "session/list cursor is not supported because Orb Code returns a single page",
        ));
    }

    let summaries = state
        .client
        .list_sessions()
        .await
        .map_err(|e| internal_error(format!("session/list failed: {e}")))?;
    let sessions = summaries
        .iter()
        .filter_map(|summary| acp_session_info_from_summary(summary, Some(cwd_filter.as_path())))
        .collect();

    responder.respond(ListSessionsResponse::new(sessions))
}

fn requested_session_list_cwd(request: &ListSessionsRequest, launch_cwd: &Path) -> Result<PathBuf> {
    let Some(requested) = request.cwd.as_ref() else {
        return Ok(launch_cwd.to_path_buf());
    };
    if !requested.is_absolute() {
        return Err(invalid_params("session/list cwd must be absolute"));
    }

    let requested = requested.canonicalize().map_err(|e| {
        invalid_params(format!(
            "session/list cwd {} cannot be canonicalized: {e}",
            requested.display()
        ))
    })?;
    if requested != launch_cwd {
        return Err(invalid_params(format!(
            "session/list cwd must match the orbcode acp launch cwd: requested {}, launch {}",
            requested.display(),
            launch_cwd.display()
        )));
    }

    Ok(requested)
}

fn acp_session_info_from_summary(
    summary: &SessionSummary,
    cwd_filter: Option<&Path>,
) -> Option<SessionInfo> {
    if !matches!(summary.status, SessionStatus::Available) {
        return None;
    }

    let cwd = std::path::PathBuf::from(summary.cwd.as_deref()?);
    if !cwd.is_absolute() || cwd_filter.is_some_and(|filter| filter != cwd.as_path()) {
        return None;
    }

    Some(
        SessionInfo::new(summary.session_id.clone(), cwd)
            .title(summary.display_title().map(str::to_string))
            .updated_at(Some(summary.updated_at.to_rfc3339())),
    )
}

pub(super) async fn handle_new_session(
    state: Arc<AcpSdkState>,
    request: NewSessionRequest,
    responder: agent_client_protocol::Responder<NewSessionResponse>,
) -> Result<()> {
    if !request.cwd.is_absolute() {
        return responder.respond_with_error(invalid_params("session/new cwd must be absolute"));
    }

    let requested_cwd = request.cwd.canonicalize().map_err(|e| {
        invalid_params(format!(
            "session/new cwd {} cannot be canonicalized: {e}",
            request.cwd.display()
        ))
    })?;

    if let Some(directory) = request
        .additional_directories
        .iter()
        .find(|directory| !directory.is_absolute())
    {
        return responder.respond_with_error(invalid_params(format!(
            "session/new additionalDirectories entry must be absolute: {}",
            directory.display()
        )));
    }

    let session_mcp_servers = acp_mcp_servers_to_configs("session/new", &request.mcp_servers)?;
    let bootstrap = state
        .client
        .bootstrap_with_params(BootstrapParams {
            cwd: Some(requested_cwd),
            additional_directories: request.additional_directories,
            session_mcp_servers,
            ..BootstrapParams::default()
        })
        .await
        .map_err(|e| internal_error(format!("session bootstrap failed: {e}")))?;

    let session_id = bootstrap.session.session_id;
    state
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), AcpSessionState::default());
    responder.respond(NewSessionResponse::new(SessionId::new(session_id)))
}

pub(super) async fn handle_load_session(
    state: Arc<AcpSdkState>,
    request: LoadSessionRequest,
    responder: agent_client_protocol::Responder<LoadSessionResponse>,
    connection: ConnectionTo<Client>,
) -> Result<()> {
    let session_id = request.session_id.to_string();
    let requested_cwd = match validate_session_cwd("session/load", &request.cwd, &state.launch_cwd)
    {
        Ok(cwd) => cwd,
        Err(err) => return responder.respond_with_error(err),
    };

    if let Some(directory) = request
        .additional_directories
        .iter()
        .find(|directory| !directory.is_absolute())
    {
        return responder.respond_with_error(invalid_params(format!(
            "session/load additionalDirectories entry must be absolute: {}",
            directory.display()
        )));
    }

    let session_mcp_servers = match acp_mcp_servers_to_configs("session/load", &request.mcp_servers)
    {
        Ok(configs) => configs,
        Err(err) => return responder.respond_with_error(err),
    };

    {
        let sessions = state.sessions.lock().await;
        if sessions
            .get(&session_id)
            .and_then(|session| session.active_prompt_generation)
            .is_some()
        {
            return responder.respond_with_error(invalid_params(format!(
                "session {session_id} already has an active prompt"
            )));
        }
    }

    let preflight = match state.client.acp_load_replay_preflight(&session_id).await {
        Ok(preflight) => preflight,
        Err(err) => {
            return responder.respond_with_error(session_preflight_error(
                "session/load",
                err,
                &session_id,
            ));
        }
    };
    if !preflight.replay_allowed {
        return responder.respond_with_error(invalid_params(format!(
            "session/load cannot replay session {session_id}: {}",
            preflight.blockers.join("; ")
        )));
    }
    if let Err(err) =
        validate_persisted_session_cwd("session/load", &preflight.session, &requested_cwd)
    {
        return responder.respond_with_error(err);
    }

    let updates = replay_updates_for_session(&preflight.session);

    if let Err(err) = state
        .client
        .acp_load_setup(BootstrapParams {
            session_id: Some(session_id.clone()),
            cwd: Some(requested_cwd),
            additional_directories: request.additional_directories,
            session_mcp_servers,
            read_only: false,
        })
        .await
    {
        return responder.respond_with_error(internal_error(format!(
            "session/load bootstrap failed: {err}"
        )));
    }

    state
        .sessions
        .lock()
        .await
        .entry(session_id.clone())
        .or_default();

    for update in updates {
        send_session_update(&connection, &session_id, update)?;
    }

    responder.respond(LoadSessionResponse::default())
}

pub(super) async fn handle_resume_session(
    state: Arc<AcpSdkState>,
    request: ResumeSessionRequest,
    responder: agent_client_protocol::Responder<ResumeSessionResponse>,
) -> Result<()> {
    let session_id = request.session_id.to_string();
    let requested_cwd =
        match validate_session_cwd("session/resume", &request.cwd, &state.launch_cwd) {
            Ok(cwd) => cwd,
            Err(err) => return responder.respond_with_error(err),
        };

    if let Some(directory) = request
        .additional_directories
        .iter()
        .find(|directory| !directory.is_absolute())
    {
        return responder.respond_with_error(invalid_params(format!(
            "session/resume additionalDirectories entry must be absolute: {}",
            directory.display()
        )));
    }

    let session_mcp_servers =
        match acp_mcp_servers_to_configs("session/resume", &request.mcp_servers) {
            Ok(configs) => configs,
            Err(err) => return responder.respond_with_error(err),
        };

    {
        let sessions = state.sessions.lock().await;
        if sessions
            .get(&session_id)
            .and_then(|session| session.active_prompt_generation)
            .is_some()
        {
            return responder.respond_with_error(invalid_params(format!(
                "session {session_id} already has an active prompt"
            )));
        }
    }

    let preflight = match state.client.acp_load_replay_preflight(&session_id).await {
        Ok(preflight) => preflight,
        Err(err) => {
            return responder.respond_with_error(session_preflight_error(
                "session/resume",
                err,
                &session_id,
            ));
        }
    };
    if let Err(err) =
        validate_persisted_session_cwd("session/resume", &preflight.session, &requested_cwd)
    {
        return responder.respond_with_error(err);
    }

    if let Err(err) = state
        .client
        .acp_resume_setup(BootstrapParams {
            session_id: Some(session_id.clone()),
            cwd: Some(requested_cwd),
            additional_directories: request.additional_directories,
            session_mcp_servers,
            read_only: false,
        })
        .await
    {
        return responder.respond_with_error(internal_error(format!(
            "session/resume bootstrap failed: {err}"
        )));
    }

    state
        .sessions
        .lock()
        .await
        .entry(session_id.clone())
        .or_default();

    responder.respond(ResumeSessionResponse::new())
}

pub(super) async fn handle_delete_session(
    state: Arc<AcpSdkState>,
    request: DeleteSessionRequest,
    responder: agent_client_protocol::Responder<DeleteSessionResponse>,
) -> Result<()> {
    let session_id = request.session_id.to_string();
    {
        let sessions = state.sessions.lock().await;
        if sessions
            .get(&session_id)
            .and_then(|session| session.active_prompt_generation)
            .is_some()
        {
            return responder.respond_with_error(invalid_params(format!(
                "session/delete cannot delete active session {session_id}"
            )));
        }
    }

    if let Err(err) = state
        .client
        .acp_delete_session(AcpDeleteSessionParams {
            session_id: session_id.clone(),
            cwd: state.launch_cwd.clone(),
        })
        .await
    {
        return responder.respond_with_error(delete_session_error(err, &session_id));
    }

    state.sessions.lock().await.remove(&session_id);
    deny_pending_server_requests(&state, &session_id).await;
    responder.respond(DeleteSessionResponse::new())
}

fn validate_session_cwd(method_name: &str, requested: &Path, launch_cwd: &Path) -> Result<PathBuf> {
    if !requested.is_absolute() {
        return Err(invalid_params(format!(
            "{method_name} cwd must be absolute"
        )));
    }
    let requested = requested.canonicalize().map_err(|e| {
        invalid_params(format!(
            "{method_name} cwd {} cannot be canonicalized: {e}",
            requested.display()
        ))
    })?;
    if requested != launch_cwd {
        return Err(invalid_params(format!(
            "{method_name} cwd must match the orbcode acp launch cwd: requested {}, launch {}",
            requested.display(),
            launch_cwd.display()
        )));
    }
    Ok(requested)
}

fn validate_persisted_session_cwd(
    method_name: &str,
    session: &SessionRecord,
    requested_cwd: &Path,
) -> Result<()> {
    let Some(persisted) = session.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) else {
        return Err(invalid_params(format!(
            "{method_name} session {} has no persisted cwd",
            session.session_id
        )));
    };
    let persisted = PathBuf::from(persisted);
    if !persisted.is_absolute() {
        return Err(invalid_params(format!(
            "{method_name} session {} has relative persisted cwd: {}",
            session.session_id,
            persisted.display()
        )));
    }
    let persisted = persisted.canonicalize().map_err(|e| {
        invalid_params(format!(
            "{method_name} persisted cwd {} cannot be canonicalized: {e}",
            persisted.display()
        ))
    })?;
    if persisted != requested_cwd {
        return Err(invalid_params(format!(
            "{method_name} cwd mismatch for session {}: requested {}, persisted {}",
            session.session_id,
            requested_cwd.display(),
            persisted.display()
        )));
    }
    Ok(())
}

fn session_preflight_error(
    method_name: &str,
    err: ClientError,
    session_id: &str,
) -> agent_client_protocol::Error {
    match err {
        ClientError::Protocol(ProtocolError {
            code: orbcode_app_server_protocol::ErrorCode::SessionNotFound,
            ..
        }) => invalid_params(format!("{method_name} session not found: {session_id}")),
        other => internal_error(format!("{method_name} preflight failed: {other}")),
    }
}

fn delete_session_error(err: ClientError, session_id: &str) -> agent_client_protocol::Error {
    match err {
        ClientError::Protocol(ProtocolError {
            code:
                orbcode_app_server_protocol::ErrorCode::SessionNotFound
                | orbcode_app_server_protocol::ErrorCode::ConfigError
                | orbcode_app_server_protocol::ErrorCode::ActiveTurn,
            message,
            ..
        }) => invalid_params(format!(
            "session/delete failed for session {session_id}: {message}"
        )),
        other => internal_error(format!("session/delete failed: {other}")),
    }
}

pub(super) async fn handle_prompt(
    state: Arc<AcpSdkState>,
    request: PromptRequest,
    responder: agent_client_protocol::Responder<PromptResponse>,
    connection: ConnectionTo<Client>,
) -> Result<()> {
    let session_id = request.session_id.to_string();
    let prompt_generation = {
        let mut sessions = state.sessions.lock().await;
        let Some(session) = sessions.get_mut(&session_id) else {
            return responder.respond_with_error(invalid_params(format!(
                "unknown ACP session_id: {session_id}"
            )));
        };
        if session.active_prompt_generation.is_some() {
            return responder.respond_with_error(invalid_params(format!(
                "session {session_id} already has an active prompt"
            )));
        }
        let generation = session.next_prompt_generation;
        session.next_prompt_generation = session.next_prompt_generation.saturating_add(1);
        session.active_prompt_generation = Some(generation);
        generation
    };

    let prompt = prompt_blocks_to_text(&request.prompt);
    if prompt.trim().is_empty() {
        finish_prompt_generation(&state, &session_id, prompt_generation).await;
        return responder.respond_with_error(invalid_params(
            "session/prompt requires at least one supported content block",
        ));
    }

    start_server_request_pump(&state, connection.clone()).await;

    let task_state = Arc::clone(&state);
    let task_session_id = session_id.clone();
    let task_connection = connection.clone();
    if let Err(err) = connection.spawn(async move {
        run_prompt_task(
            task_state,
            task_session_id,
            prompt_generation,
            prompt,
            responder,
            task_connection,
        )
        .await
    }) {
        finish_prompt_generation(&state, &session_id, prompt_generation).await;
        return Err(err);
    }

    Ok(())
}

pub(super) async fn run_prompt_task(
    state: Arc<AcpSdkState>,
    session_id: String,
    prompt_generation: u64,
    prompt: String,
    responder: agent_client_protocol::Responder<PromptResponse>,
    connection: ConnectionTo<Client>,
) -> Result<()> {
    let mut event_rx = match state.client.submit_turn_stream(&session_id, prompt).await {
        Ok(rx) => rx,
        Err(err) => {
            finish_prompt_generation(&state, &session_id, prompt_generation).await;
            return responder
                .respond_with_error(internal_error(format!("turn submit failed: {err}")));
        }
    };

    let response = super::prompt_response_loop(&session_id, &mut event_rx, connection).await;
    finish_prompt_generation(&state, &session_id, prompt_generation).await;

    match response {
        Ok(stop_reason) => responder.respond(PromptResponse::new(stop_reason)),
        Err(err) => responder.respond_with_error(err),
    }
}

pub(super) async fn handle_close_session(
    state: Arc<AcpSdkState>,
    request: CloseSessionRequest,
    responder: agent_client_protocol::Responder<CloseSessionResponse>,
) -> Result<()> {
    let session_id = request.session_id.to_string();
    if state.sessions.lock().await.remove(&session_id).is_none() {
        return responder.respond_with_error(invalid_params(format!(
            "unknown ACP session_id: {session_id}"
        )));
    }

    if let Err(err) = state.client.cancel_turn(&session_id).await {
        tracing::warn!(%session_id, %err, "ACP session/close cancel failed");
    }
    deny_pending_server_requests(&state, &session_id).await;
    if let Err(err) = state.client.acp_close_session(&session_id).await {
        tracing::warn!(%session_id, %err, "ACP session/close cleanup failed");
    }

    responder.respond(CloseSessionResponse::new())
}

pub(super) fn prompt_blocks_to_text(blocks: &[ContentBlock]) -> String {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(text) => parts.push(text.text.clone()),
            ContentBlock::ResourceLink(link) => {
                parts.push(format!("Resource: {} ({})", link.name, link.uri));
            }
            ContentBlock::Resource(resource) => match &resource.resource {
                EmbeddedResourceResource::TextResourceContents(text) => {
                    parts.push(format!("Resource {}:\n{}", text.uri, text.text));
                }
                EmbeddedResourceResource::BlobResourceContents(blob) => {
                    parts.push(format!("Binary resource: {}", blob.uri));
                }
                _ => parts.push("Unsupported embedded resource".to_string()),
            },
            ContentBlock::Image(image) => {
                let label = image.uri.as_deref().unwrap_or("<inline image>");
                parts.push(format!("Unsupported image content: {label}"));
            }
            ContentBlock::Audio(audio) => {
                parts.push(format!("Unsupported audio content: {}", audio.mime_type));
            }
            _ => parts.push("Unsupported ACP content block".to_string()),
        }
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_summary_mapping_requires_available_absolute_cwd() {
        let now = chrono::Utc::now();
        let mut summary = SessionSummary {
            session_id: "session-1".to_string(),
            title: Some("hello".to_string()),
            custom_title: None,
            message_count: 1,
            created_at: now,
            updated_at: now,
            cwd: Some("/tmp/project".to_string()),
            git_branch: None,
            model: None,
            provider: None,
            transcript_path: None,
            status: SessionStatus::Available,
            total_input_tokens: 0,
            total_output_tokens: 0,
            duration_secs: None,
        };

        let info = acp_session_info_from_summary(&summary, None).expect("mapped");
        assert_eq!(info.session_id.to_string(), "session-1");
        assert_eq!(info.cwd, std::path::PathBuf::from("/tmp/project"));
        assert_eq!(info.title.as_deref(), Some("hello"));
        let updated_at = now.to_rfc3339();
        assert_eq!(info.updated_at.as_deref(), Some(updated_at.as_str()));

        assert!(
            acp_session_info_from_summary(&summary, Some(std::path::Path::new("/tmp/other")))
                .is_none()
        );

        summary.cwd = Some("relative/project".to_string());
        assert!(acp_session_info_from_summary(&summary, None).is_none());

        summary.cwd = Some("/tmp/project".to_string());
        summary.status = SessionStatus::Corrupt {
            reason: "bad json".to_string(),
        };
        assert!(acp_session_info_from_summary(&summary, None).is_none());
    }

    #[test]
    fn prompt_blocks_to_text_supports_text_and_resource_link() {
        let text = prompt_blocks_to_text(&[
            ContentBlock::from("hello"),
            ContentBlock::ResourceLink(agent_client_protocol::schema::ResourceLink::new(
                "README",
                "file:///README.md",
            )),
        ]);

        assert!(text.contains("hello"));
        assert!(text.contains("Resource: README"));
        assert!(text.contains("file:///README.md"));
    }
}
