use agent_client_protocol::schema::{
    AgentCapabilities, Implementation, InitializeRequest, InitializeResponse, McpCapabilities,
    SessionAdditionalDirectoriesCapabilities, SessionCapabilities, SessionCloseCapabilities,
    SessionDeleteCapabilities, SessionListCapabilities, SessionResumeCapabilities,
};

pub(super) fn initialize_response(initialize: InitializeRequest) -> InitializeResponse {
    InitializeResponse::new(initialize.protocol_version)
        .agent_capabilities(
            AgentCapabilities::new()
                .load_session(true)
                .mcp_capabilities(McpCapabilities::new().http(true))
                .session_capabilities(
                    SessionCapabilities::new()
                        .additional_directories(SessionAdditionalDirectoriesCapabilities::new())
                        .list(SessionListCapabilities::new())
                        .delete(SessionDeleteCapabilities::new())
                        .resume(SessionResumeCapabilities::new())
                        .close(SessionCloseCapabilities::new()),
                ),
        )
        .auth_methods(Vec::new())
        .agent_info(Implementation::new("orbcode", env!("CARGO_PKG_VERSION")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::ProtocolVersion;
    use serde_json::json;

    #[test]
    fn initialize_response_uses_acp_v1_camel_case() {
        let response = initialize_response(InitializeRequest::new(ProtocolVersion::V1));
        let json = serde_json::to_value(response).expect("serialize");

        assert_eq!(json["protocolVersion"], json!(1));
        assert!(json.get("protocol_version").is_none());
        assert!(json.get("agentCapabilities").is_some());
        assert_eq!(json["agentInfo"]["name"], "orbcode");
        assert_eq!(json["authMethods"], json!([]));
        assert_eq!(json["agentCapabilities"]["mcpCapabilities"]["http"], true);
        assert!(json["agentCapabilities"]["auth"].is_object());
        assert!(json["agentCapabilities"]["sessionCapabilities"]["close"].is_object());
        assert!(json["agentCapabilities"]["sessionCapabilities"]["list"].is_object());
        assert!(
            json["agentCapabilities"]["sessionCapabilities"]["additionalDirectories"].is_object()
        );
    }

    #[test]
    fn initialize_response_advertises_only_implemented_capabilities() {
        let response = initialize_response(InitializeRequest::new(ProtocolVersion::V1));
        let capabilities = response.agent_capabilities;

        assert!(
            capabilities.load_session,
            "session/load is implemented through AppClient replay preflight"
        );
        assert!(capabilities.mcp_capabilities.http);
        assert!(
            !capabilities.mcp_capabilities.sse,
            "SSE MCP transport is rejected by session/new"
        );
        assert!(
            capabilities
                .session_capabilities
                .additional_directories
                .is_some()
        );
        assert!(capabilities.session_capabilities.close.is_some());
        assert!(
            capabilities.session_capabilities.list.is_some(),
            "session/list is implemented through AppClient::list_sessions"
        );
        assert!(
            capabilities.session_capabilities.resume.is_some(),
            "session/resume is implemented through AppClient resume setup"
        );
        assert!(
            capabilities.session_capabilities.delete.is_some(),
            "session/delete is implemented through AppClient scoped delete"
        );
        assert!(!capabilities.prompt_capabilities.image);
        assert!(!capabilities.prompt_capabilities.audio);
        assert!(!capabilities.prompt_capabilities.embedded_context);
    }

    #[test]
    fn initialize_response_omits_unimplemented_optional_capabilities_on_wire() {
        let response = initialize_response(InitializeRequest::new(ProtocolVersion::V1));
        let json = serde_json::to_value(response).expect("serialize");
        let agent_capabilities = &json["agentCapabilities"];
        let session_capabilities = &agent_capabilities["sessionCapabilities"];

        assert_eq!(agent_capabilities["loadSession"], json!(true));
        assert_eq!(json["authMethods"], json!([]));
        assert!(agent_capabilities["auth"].is_object());
        assert!(agent_capabilities["auth"].get("logout").is_none());
        assert!(session_capabilities["list"].is_object());
        assert!(session_capabilities["close"].is_object());
        assert!(session_capabilities["additionalDirectories"].is_object());
        assert!(session_capabilities["resume"].is_object());
        assert!(session_capabilities["delete"].is_object());
        assert!(session_capabilities.get("fork").is_none());
        assert!(agent_capabilities.get("elicitation").is_none());
        assert!(agent_capabilities.get("providers").is_none());
        assert!(agent_capabilities.get("nes").is_none());
        assert!(agent_capabilities.get("positionEncoding").is_none());
        assert_eq!(agent_capabilities["mcpCapabilities"]["http"], json!(true));
        assert_eq!(agent_capabilities["mcpCapabilities"]["sse"], json!(false));
        assert!(agent_capabilities["mcpCapabilities"].get("acp").is_none());
    }
}
