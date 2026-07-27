use crate::error::McpError;
use crate::oauth::{
    complete_mcp_oauth_browser_login, is_mcp_oauth_token_expired, mcp_endpoint_is_internal,
    mcp_oauth_status_entry, poll_mcp_oauth_device_token, refresh_mcp_oauth_token,
    start_mcp_oauth_browser_login, start_mcp_oauth_device_login, unix_timestamp_now,
};
use crate::registry::McpRegistry;
use crate::store::{StoredMcpOAuthToken, save_oauth_tokens};
use crate::types::{
    McpOAuthBrowserLoginInput, McpOAuthBrowserLoginSession, McpOAuthDeviceLoginInput,
    McpOAuthDeviceLoginSession, McpOAuthOverview, McpOAuthStatusEntry, McpOAuthTokenInput,
};

impl McpRegistry {
    pub async fn mcp_oauth_overview(
        &self,
        server_id: Option<&str>,
    ) -> Result<McpOAuthOverview, McpError> {
        let state = self.state.lock().await;
        let mut entries = state
            .oauth_tokens
            .servers
            .iter()
            .filter(|(id, _)| match server_id {
                Some(requested) => requested == id.as_str(),
                None => true,
            })
            .map(|(id, token)| mcp_oauth_status_entry(id, token))
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.server_id.cmp(&right.server_id));
        Ok(McpOAuthOverview {
            store_path: state.storage_dir.join("oauth_tokens.json"),
            entries,
        })
    }

    /// Public token-store entry point (e.g. manual `mcp oauth store`). There is
    /// no login-start decision to preserve here, so SSRF enforcement is derived
    /// from the server's current endpoint.
    pub async fn store_mcp_oauth_token(
        &self,
        server_id: &str,
        input: McpOAuthTokenInput,
    ) -> Result<McpOAuthStatusEntry, McpError> {
        self.store_mcp_oauth_token_inner(server_id, input, None)
            .await
    }

    /// Store a token, optionally FREEZING the SSRF enforcement decision made at
    /// login start (`enforce_ssrf_override`). The OAuth login-completion paths
    /// pass the session's original decision so a server endpoint edited from
    /// public→local mid-login cannot downgrade the stored (and later refreshed)
    /// token. `None` derives enforcement from the current server config.
    pub(crate) async fn store_mcp_oauth_token_inner(
        &self,
        server_id: &str,
        input: McpOAuthTokenInput,
        enforce_ssrf_override: Option<bool>,
    ) -> Result<McpOAuthStatusEntry, McpError> {
        let access_token = input.access_token.trim().to_string();
        if access_token.is_empty() {
            return Err(McpError::InvalidConfig(
                "MCP OAuth access token cannot be empty".into(),
            ));
        }

        let refresh_token = input
            .refresh_token
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let token_endpoint = input
            .token_endpoint
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let client_id = input
            .client_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let scopes = input
            .scopes
            .into_iter()
            .map(|scope| scope.trim().to_string())
            .filter(|scope| !scope.is_empty())
            .collect::<Vec<_>>();

        let (storage_dir, oauth_tokens, entry) = {
            let mut state = self.state.lock().await;
            let Some(server) = state
                .store
                .servers
                .iter()
                .find(|server| server.config.id == server_id)
            else {
                return Err(McpError::UnknownServer(server_id.to_string()));
            };
            // Freeze the enforcement decision: prefer the login-start decision
            // carried on the session (`enforce_ssrf_override`), so a server
            // endpoint edited from public→local between login start and token
            // store cannot silently downgrade it. Fall back to the current
            // endpoint for the manual store path. Refresh reads this stored value
            // rather than re-deriving from the (possibly-changed) live config.
            let enforce_ssrf = enforce_ssrf_override
                .unwrap_or_else(|| !mcp_endpoint_is_internal(&server.config.endpoint));
            let token = StoredMcpOAuthToken {
                access_token,
                refresh_token,
                token_endpoint,
                client_id,
                expires_at: input.expires_at,
                scopes,
                updated_at: Some(unix_timestamp_now()),
                enforce_ssrf,
            };
            let entry = mcp_oauth_status_entry(server_id, &token);
            state
                .oauth_tokens
                .servers
                .insert(server_id.to_string(), token);
            (state.storage_dir.clone(), state.oauth_tokens.clone(), entry)
        };

        save_oauth_tokens(&storage_dir, &oauth_tokens).await?;
        Ok(entry)
    }

    pub async fn start_mcp_oauth_device_login(
        &self,
        server_id: &str,
        input: McpOAuthDeviceLoginInput,
    ) -> Result<McpOAuthDeviceLoginSession, McpError> {
        let config = self.stored_server_config(server_id).await?;
        start_mcp_oauth_device_login(&config, input).await
    }

    pub async fn complete_mcp_oauth_device_login(
        &self,
        session: McpOAuthDeviceLoginSession,
    ) -> Result<McpOAuthStatusEntry, McpError> {
        self.ensure_stored_server(&session.server_id).await?;
        // Preserve the enforcement decision made when this login started, not the
        // server's (possibly since-edited) current endpoint.
        let enforce_ssrf = session.enforce_ssrf;
        let server_id = session.server_id.clone();
        let input = poll_mcp_oauth_device_token(&session).await?;
        self.store_mcp_oauth_token_inner(&server_id, input, Some(enforce_ssrf))
            .await
    }

    pub async fn start_mcp_oauth_browser_login(
        &self,
        server_id: &str,
        input: McpOAuthBrowserLoginInput,
    ) -> Result<McpOAuthBrowserLoginSession, McpError> {
        let config = self.stored_server_config(server_id).await?;
        start_mcp_oauth_browser_login(&config, input).await
    }

    pub async fn complete_mcp_oauth_browser_login(
        &self,
        session: McpOAuthBrowserLoginSession,
    ) -> Result<McpOAuthStatusEntry, McpError> {
        self.ensure_stored_server(&session.server_id).await?;
        // Preserve the login-start enforcement decision (see device login above).
        let enforce_ssrf = session.enforce_ssrf;
        let input = complete_mcp_oauth_browser_login(session).await?;
        self.store_mcp_oauth_token_inner(&input.0, input.1, Some(enforce_ssrf))
            .await
    }

    pub async fn logout_mcp_oauth_token(&self, server_id: &str) -> Result<bool, McpError> {
        let (storage_dir, oauth_tokens, removed) = {
            let mut state = self.state.lock().await;
            if !state
                .store
                .servers
                .iter()
                .any(|server| server.config.id == server_id)
            {
                return Err(McpError::UnknownServer(server_id.to_string()));
            }
            let removed = state.oauth_tokens.servers.remove(server_id).is_some();
            (
                state.storage_dir.clone(),
                state.oauth_tokens.clone(),
                removed,
            )
        };

        if removed {
            save_oauth_tokens(&storage_dir, &oauth_tokens).await?;
        }
        Ok(removed)
    }

    pub(crate) async fn mcp_oauth_access_token(
        &self,
        server_id: &str,
    ) -> Result<Option<String>, McpError> {
        {
            let state = self.state.lock().await;
            let Some(token) = state.oauth_tokens.servers.get(server_id) else {
                return Ok(None);
            };
            if !is_mcp_oauth_token_expired(token) {
                return Ok(Some(token.access_token.clone()));
            }
        }

        let lock = {
            let mut locks = self.refresh_locks.lock().await;
            locks
                .entry(server_id.to_string())
                .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        let (storage_dir, token) = {
            let state = self.state.lock().await;
            let Some(token) = state.oauth_tokens.servers.get(server_id) else {
                return Ok(None);
            };
            if !is_mcp_oauth_token_expired(token) {
                return Ok(Some(token.access_token.clone()));
            }
            (state.storage_dir.clone(), token.clone())
        };

        // Enforcement is read from the token's frozen `enforce_ssrf` (see
        // `refresh_mcp_oauth_token`), NOT re-derived from the live server config —
        // a public→local endpoint edit must not downgrade an existing public
        // token's refresh.
        let original_scopes = token.scopes.clone();
        let refreshed = refresh_mcp_oauth_token(server_id, &token).await?;

        if !original_scopes.is_empty() && !refreshed.scopes.is_empty() {
            let original_set: std::collections::BTreeSet<&str> =
                original_scopes.iter().map(String::as_str).collect();
            let new_set: std::collections::BTreeSet<&str> =
                refreshed.scopes.iter().map(String::as_str).collect();
            if new_set.is_subset(&original_set) && new_set != original_set {
                eprintln!(
                    "warning: OAuth scope downgrade for server `{server_id}`: \
                     requested [{}] but received [{}]",
                    original_scopes.join(", "),
                    refreshed.scopes.join(", "),
                );
            }
        }

        let oauth_tokens = {
            let mut state = self.state.lock().await;
            state
                .oauth_tokens
                .servers
                .insert(server_id.to_string(), refreshed.clone());
            state.oauth_tokens.clone()
        };
        save_oauth_tokens(&storage_dir, &oauth_tokens).await?;
        Ok(Some(refreshed.access_token))
    }
}
