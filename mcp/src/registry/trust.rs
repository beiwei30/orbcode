use crate::error::McpError;
use crate::registry::{
    McpRegistry, server_visible_to_session, session_owner_for_server, trust_to_setting,
};
use crate::store::save_trust;
use crate::types::{McpServerTrust, SharedTrustApprovalHandler};

impl McpRegistry {
    pub async fn server_trust(&self, server_id: &str) -> McpServerTrust {
        self.server_trust_visible_to(server_id, None)
            .await
            .unwrap_or_default()
    }

    pub async fn server_trust_for_session(
        &self,
        session_id: &str,
        server_id: &str,
    ) -> Option<McpServerTrust> {
        self.server_trust_visible_to(server_id, Some(session_id))
            .await
    }

    async fn server_trust_visible_to(
        &self,
        server_id: &str,
        session_id: Option<&str>,
    ) -> Option<McpServerTrust> {
        let state = self.state.lock().await;
        state
            .store
            .servers
            .iter()
            .find(|server| {
                server.config.id == server_id
                    && server_visible_to_session(&state, &server.config.id, session_id)
            })
            .map(|server| server.config.trust)
    }

    pub async fn set_server_trust(
        &self,
        server_id: &str,
        trust: McpServerTrust,
    ) -> Result<(), McpError> {
        self.set_server_trust_visible_to(server_id, trust, None)
            .await
    }

    pub async fn set_server_trust_for_session(
        &self,
        session_id: &str,
        server_id: &str,
        trust: McpServerTrust,
    ) -> Result<(), McpError> {
        self.set_server_trust_visible_to(server_id, trust, Some(session_id))
            .await
    }

    async fn set_server_trust_visible_to(
        &self,
        server_id: &str,
        trust: McpServerTrust,
        session_id: Option<&str>,
    ) -> Result<(), McpError> {
        let (home_dir, storage_dir, trust_store, persist) = {
            let mut state = self.state.lock().await;
            if !server_visible_to_session(&state, server_id, session_id) {
                return Err(McpError::UnknownServer(server_id.to_string()));
            }
            let persist = session_owner_for_server(&state, server_id).is_none();
            let server = state
                .store
                .servers
                .iter_mut()
                .find(|server| server.config.id == server_id)
                .ok_or_else(|| McpError::UnknownServer(server_id.to_string()))?;
            server.config.trust = trust;
            if persist {
                state.trust.servers.insert(server_id.to_string(), trust);
            }
            (
                state.home_dir.clone(),
                state.storage_dir.clone(),
                state.trust.clone(),
                persist,
            )
        };

        if !trust.is_trusted() {
            self.shutdown_stdio_client(server_id).await;
        }
        if !persist {
            return Ok(());
        }
        // Persist to the legacy store and to the settings layer (the same layer
        // other permissions use) so the decision survives a fresh registry even
        // if `trust.json` is removed.
        save_trust(&storage_dir, &trust_store).await?;
        orbcode_config::set_mcp_server_trust_setting(&home_dir, server_id, trust_to_setting(trust))
            .await
            .map_err(|error| McpError::Io(std::io::Error::other(error)))?;
        Ok(())
    }

    pub async fn set_trust_approval_handler(&self, handler: SharedTrustApprovalHandler) {
        let mut slot = self.trust_approval_handler.lock().await;
        *slot = Some(handler);
    }
}
