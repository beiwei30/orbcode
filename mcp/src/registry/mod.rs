pub(crate) mod diagnostics;
pub(crate) mod oauth;
pub(crate) mod resources;
pub(crate) mod rpc;
pub(crate) mod tools;
pub(crate) mod trust;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::config_load::{
    default_capabilities, diff_server_configs, load_configured_servers, merge_configured_servers,
};
use crate::error::McpError;
use crate::store::{
    McpOAuthTokenStore, McpStore, McpTrustStore, StoredMcpServer, apply_persisted_trust,
    save_oauth_tokens, save_store, seed_server,
};
use crate::transport::stdio::StdioMcpClient;
use crate::transport::streamable_http::StreamableHttpMcpClient;
use crate::types::{
    McpCapability, McpConfigReloadResult, McpLoadOptions, McpServerConfig, McpServerStatus,
    McpServerTrust, SharedTrustApprovalHandler,
};

#[derive(Clone, Debug)]
pub(crate) struct RestartBackoff {
    pub(crate) attempt: u32,
    pub(crate) next_allowed_at: Instant,
}

impl RestartBackoff {
    const BASE_SECS: f64 = 1.0;
    const MAX_SECS: f64 = 30.0;
    const JITTER_FRACTION: f64 = 0.2;

    pub(crate) fn new() -> Self {
        Self {
            attempt: 0,
            next_allowed_at: Instant::now(),
        }
    }

    pub(crate) fn next_delay(&self) -> std::time::Duration {
        let raw = (Self::BASE_SECS * 2.0_f64.powi(self.attempt as i32)).min(Self::MAX_SECS);
        let jitter_range = raw * Self::JITTER_FRACTION;
        let jitter =
            (f64::from(rand_u32()) / f64::from(u32::MAX)) * 2.0 * jitter_range - jitter_range;
        let delay = (raw + jitter).max(0.1);
        std::time::Duration::from_secs_f64(delay)
    }

    pub(crate) fn is_allowed(&self) -> bool {
        Instant::now() >= self.next_allowed_at
    }

    pub(crate) fn record_attempt(&mut self) {
        let delay = self.next_delay();
        self.attempt = self.attempt.saturating_add(1);
        self.next_allowed_at = Instant::now() + delay;
    }

    pub(crate) fn reset(&mut self) {
        self.attempt = 0;
        self.next_allowed_at = Instant::now();
    }
}

fn rand_u32() -> u32 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish() as u32
}

#[derive(Clone)]
pub struct McpRegistry {
    pub(crate) state: Arc<Mutex<RegistryState>>,
    pub(crate) stdio_clients: Arc<Mutex<BTreeMap<String, SharedStdioClient>>>,
    pub(crate) streamable_http_clients: Arc<Mutex<BTreeMap<String, SharedStreamableHttpClient>>>,
    pub(crate) restart_backoffs: Arc<Mutex<BTreeMap<String, RestartBackoff>>>,
    pub(crate) streamable_http_start_locks: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
    pub(crate) refresh_locks: Arc<Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    pub(crate) trust_approval_handler: Arc<Mutex<Option<SharedTrustApprovalHandler>>>,
}

impl fmt::Debug for McpRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpRegistry")
            .field("state", &"<registry state>")
            .field("stdio_clients", &"<stdio client map>")
            .field("restart_backoffs", &"<restart backoff map>")
            .field("refresh_locks", &"<refresh lock map>")
            .field("has_trust_approval_handler", &"<handler state>")
            .finish()
    }
}

pub(crate) struct StdioClientSlot {
    serialize: tokio::sync::Semaphore,
    client: std::sync::Mutex<Option<StdioMcpClient>>,
}

fn lock_client_slot<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // The mutex only protects a replaceable client slot. Recovering the inner
    // Option after a task panic is safe: an interrupted take remains `None` and
    // is reported by the existing typed "client unavailable" path.
    match mutex.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl StdioClientSlot {
    pub(crate) fn new(client: StdioMcpClient) -> Self {
        Self {
            serialize: tokio::sync::Semaphore::new(1),
            client: std::sync::Mutex::new(Some(client)),
        }
    }

    pub(crate) async fn acquire(
        &self,
    ) -> Result<(tokio::sync::SemaphorePermit<'_>, StdioMcpClient), McpError> {
        let permit = self
            .serialize
            .acquire()
            .await
            .map_err(|_| McpError::Protocol("stdio client closed".into()))?;
        let client = lock_client_slot(&self.client)
            .take()
            .ok_or_else(|| McpError::Protocol("stdio client unavailable".into()))?;
        Ok((permit, client))
    }

    pub(crate) fn return_client(&self, client: StdioMcpClient) {
        *lock_client_slot(&self.client) = Some(client);
    }
}

pub(crate) type SharedStdioClient = Arc<StdioClientSlot>;

pub(crate) struct StreamableHttpClientSlot {
    serialize: tokio::sync::Semaphore,
    client: std::sync::Mutex<Option<StreamableHttpMcpClient>>,
}

impl StreamableHttpClientSlot {
    pub(crate) fn new(client: StreamableHttpMcpClient) -> Self {
        Self {
            serialize: tokio::sync::Semaphore::new(1),
            client: std::sync::Mutex::new(Some(client)),
        }
    }

    pub(crate) async fn acquire(&self) -> Result<StreamableHttpClientLease<'_>, McpError> {
        let permit = self
            .serialize
            .acquire()
            .await
            .map_err(|_| McpError::Protocol("streamable HTTP client closed".into()))?;
        let client = lock_client_slot(&self.client)
            .take()
            .ok_or_else(|| McpError::Protocol("streamable HTTP client unavailable".into()))?;
        Ok(StreamableHttpClientLease {
            _permit: permit,
            slot: self,
            client: Some(client),
        })
    }

    fn return_client(&self, client: StreamableHttpMcpClient) {
        *lock_client_slot(&self.client) = Some(client);
    }
}

pub(crate) type SharedStreamableHttpClient = Arc<StreamableHttpClientSlot>;

pub(crate) struct StreamableHttpClientLease<'a> {
    _permit: tokio::sync::SemaphorePermit<'a>,
    slot: &'a StreamableHttpClientSlot,
    client: Option<StreamableHttpMcpClient>,
}

impl StreamableHttpClientLease<'_> {
    pub(crate) fn client_mut(&mut self) -> &mut StreamableHttpMcpClient {
        self.client
            .as_mut()
            .expect("streamable HTTP lease always holds a client until drop")
    }
}

impl Drop for StreamableHttpClientLease<'_> {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            self.slot.return_client(client);
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RegistryState {
    pub(crate) home_dir: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) storage_dir: PathBuf,
    pub(crate) store: McpStore,
    pub(crate) trust: McpTrustStore,
    pub(crate) oauth_tokens: McpOAuthTokenStore,
    pub(crate) capabilities: Vec<McpCapability>,
    pub(crate) session_servers: BTreeMap<String, Vec<String>>,
}

impl McpRegistry {
    pub async fn load(
        home_dir: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
    ) -> Result<Self, McpError> {
        Self::load_with_options(home_dir, cwd, McpLoadOptions::default()).await
    }

    pub async fn load_with_options(
        home_dir: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
        options: McpLoadOptions,
    ) -> Result<Self, McpError> {
        let home_dir = home_dir.into();
        let cwd = cwd.into();
        let storage_dir = home_dir.join("mcp");
        tokio::fs::create_dir_all(&storage_dir).await?;

        let store_path = storage_dir.join("servers.json");
        let mut store = if tokio::fs::try_exists(&store_path).await? {
            let contents = tokio::fs::read_to_string(&store_path).await?;
            serde_json::from_str(&contents)?
        } else {
            McpStore::default()
        };
        let trust_path = storage_dir.join("trust.json");
        let trust = if tokio::fs::try_exists(&trust_path).await? {
            let contents = tokio::fs::read_to_string(&trust_path).await?;
            serde_json::from_str(&contents).unwrap_or_default()
        } else {
            McpTrustStore::default()
        };
        let oauth_tokens_path = storage_dir.join("oauth_tokens.json");
        let oauth_tokens = if tokio::fs::try_exists(&oauth_tokens_path).await? {
            let contents = tokio::fs::read_to_string(&oauth_tokens_path).await?;
            serde_json::from_str(&contents).unwrap_or_default()
        } else {
            McpOAuthTokenStore::default()
        };
        let loaded_configs = load_configured_servers(&home_dir, &cwd, &options).await?;
        merge_configured_servers(&mut store, loaded_configs);
        // Legacy `mcp/trust.json` is applied first; settings-layer decisions then
        // override it so the settings layer is authoritative across resume.
        apply_persisted_trust(&mut store, &trust);
        let settings_trust = orbcode_config::load_mcp_trust_overrides(&home_dir, &cwd).await;
        apply_settings_trust(&mut store, &settings_trust);

        Ok(Self {
            state: Arc::new(Mutex::new(RegistryState {
                home_dir,
                cwd,
                storage_dir,
                store,
                trust,
                oauth_tokens,
                capabilities: default_capabilities(),
                session_servers: BTreeMap::new(),
            })),
            stdio_clients: Arc::new(Mutex::new(BTreeMap::new())),
            streamable_http_clients: Arc::new(Mutex::new(BTreeMap::new())),
            restart_backoffs: Arc::new(Mutex::new(BTreeMap::new())),
            streamable_http_start_locks: Arc::new(Mutex::new(BTreeMap::new())),
            refresh_locks: Arc::new(Mutex::new(BTreeMap::new())),
            trust_approval_handler: Arc::new(Mutex::new(None)),
        })
    }

    pub async fn capabilities(&self) -> Vec<McpCapability> {
        self.state.lock().await.capabilities.clone()
    }

    pub async fn list_servers(&self) -> Vec<McpServerConfig> {
        self.list_servers_visible_to(None).await
    }

    pub async fn list_servers_for_session(&self, session_id: &str) -> Vec<McpServerConfig> {
        self.list_servers_visible_to(Some(session_id)).await
    }

    async fn list_servers_visible_to(&self, session_id: Option<&str>) -> Vec<McpServerConfig> {
        let state = self.state.lock().await;
        let mut servers: Vec<McpServerConfig> = state
            .store
            .servers
            .iter()
            .filter(|server| server_visible_to_session(&state, &server.config.id, session_id))
            .map(|server| normalized_server_config(&server.config))
            .collect();
        servers.sort_by(|left, right| left.id.cmp(&right.id));
        servers
    }

    pub async fn upsert_server(&self, mut config: McpServerConfig) -> Result<(), McpError> {
        let config_id = config.id.clone();

        // Programmatic upserts (CLI add, app-server APIs) default to Trusted unless the caller
        // explicitly marked the entry Denied. Loading from `.mcp.json` flows through a
        // different path that preserves Unknown.
        if matches!(config.trust, McpServerTrust::Unknown) {
            config.trust = McpServerTrust::Trusted;
        }

        let (storage_dir, store, shutdown_existing) = {
            let mut state = self.state.lock().await;
            if session_owner_for_server(&state, &config_id).is_some() {
                return Err(McpError::UnknownServer(config_id));
            }
            let shutdown_existing = state
                .store
                .servers
                .iter()
                .any(|server| server.config.id == config_id);
            match state
                .store
                .servers
                .iter_mut()
                .find(|server| server.config.id == config.id)
            {
                Some(server) => server.config = config.clone(),
                None => state.store.servers.push(seed_server(config)),
            }
            (
                state.storage_dir.clone(),
                persistent_store_snapshot(&state),
                shutdown_existing,
            )
        };

        if shutdown_existing {
            self.shutdown_stdio_client(&config_id).await;
            self.shutdown_streamable_http_client(&config_id).await;
        }
        save_store(&storage_dir, &store).await
    }

    pub async fn upsert_session_servers(
        &self,
        session_id: &str,
        mut configs: Vec<McpServerConfig>,
    ) -> Vec<McpServerConfig> {
        if configs.is_empty() {
            return Vec::new();
        }

        let replaced_ids = {
            let mut state = self.state.lock().await;
            let previous_ids = state
                .session_servers
                .get(session_id)
                .cloned()
                .unwrap_or_default();
            state
                .store
                .servers
                .retain(|server| !previous_ids.contains(&server.config.id));

            let mut accepted = Vec::with_capacity(configs.len());
            let mut ids = Vec::with_capacity(configs.len());
            for mut config in configs.drain(..) {
                config.id = unique_session_server_id(&state.store, &accepted, &config.id);
                config.trust = McpServerTrust::Unknown;
                ids.push(config.id.clone());
                state.store.servers.push(seed_server(config.clone()));
                accepted.push(config);
            }

            state.session_servers.insert(session_id.to_string(), ids);
            (previous_ids, accepted)
        };

        for id in &replaced_ids.0 {
            self.shutdown_stdio_client(id).await;
            self.shutdown_streamable_http_client(id).await;
        }

        replaced_ids.1
    }

    pub async fn remove_session_servers(&self, session_id: &str) -> Vec<String> {
        let ids = {
            let mut state = self.state.lock().await;
            let ids = state.session_servers.remove(session_id).unwrap_or_default();
            state
                .store
                .servers
                .retain(|server| !ids.contains(&server.config.id));
            ids
        };
        for id in &ids {
            self.shutdown_stdio_client(id).await;
            self.shutdown_streamable_http_client(id).await;
        }
        ids
    }

    pub async fn remove_server(&self, server_id: &str) -> Result<bool, McpError> {
        let (storage_dir, store, oauth_tokens, removed) = {
            let mut state = self.state.lock().await;
            if session_owner_for_server(&state, server_id).is_some() {
                return Ok(false);
            }
            let previous_len = state.store.servers.len();
            state
                .store
                .servers
                .retain(|server| server.config.id != server_id);
            state.oauth_tokens.servers.remove(server_id);
            (
                state.storage_dir.clone(),
                persistent_store_snapshot(&state),
                state.oauth_tokens.clone(),
                previous_len != state.store.servers.len(),
            )
        };

        if removed {
            self.shutdown_stdio_client(server_id).await;
            self.shutdown_streamable_http_client(server_id).await;
            save_store(&storage_dir, &store).await?;
            save_oauth_tokens(&storage_dir, &oauth_tokens).await?;
        }

        Ok(removed)
    }

    /// Drop servers rejected by managed policy from the in-memory registry
    /// without mutating the persisted store. The `allowed` predicate receives
    /// each server id; rejected servers have their stdio clients shut down.
    /// Returns the ids that were pruned, sorted.
    pub async fn retain_policy_allowed<F>(&self, allowed: F) -> Vec<String>
    where
        F: Fn(&str) -> bool,
    {
        let mut pruned = {
            let mut state = self.state.lock().await;
            let mut pruned = Vec::new();
            let mut kept = Vec::with_capacity(state.store.servers.len());
            for server in std::mem::take(&mut state.store.servers) {
                if allowed(&server.config.id) {
                    kept.push(server);
                } else {
                    pruned.push(server.config.id.clone());
                }
            }
            state.store.servers = kept;
            pruned
        };
        for id in &pruned {
            self.shutdown_stdio_client(id).await;
            self.shutdown_streamable_http_client(id).await;
        }
        pruned.sort();
        pruned
    }

    pub async fn reload_config(
        &self,
        options: McpLoadOptions,
    ) -> Result<McpConfigReloadResult, McpError> {
        let (home_dir, cwd) = {
            let state = self.state.lock().await;
            (state.home_dir.clone(), state.cwd.clone())
        };

        let new_configs = load_configured_servers(&home_dir, &cwd, &options).await?;
        let settings_trust = orbcode_config::load_mcp_trust_overrides(&home_dir, &cwd).await;

        let (result, new_configs) = {
            let state = self.state.lock().await;
            let session_owned_ids = state
                .session_servers
                .values()
                .flatten()
                .cloned()
                .collect::<std::collections::HashSet<_>>();
            let new_configs = new_configs
                .into_iter()
                .filter(|config| !session_owned_ids.contains(&config.id))
                .collect::<Vec<_>>();
            let store = persistent_store_snapshot(&state);
            (
                diff_server_configs(&store.servers, &new_configs),
                new_configs,
            )
        };

        if result.added.is_empty() && result.removed.is_empty() && result.restarted.is_empty() {
            return Ok(result);
        }

        for id in &result.removed {
            self.shutdown_stdio_client(id).await;
            self.shutdown_streamable_http_client(id).await;
        }
        for id in &result.restarted {
            self.shutdown_stdio_client(id).await;
            self.shutdown_streamable_http_client(id).await;
        }

        {
            let mut state = self.state.lock().await;
            let mut store = persistent_store_snapshot(&state);
            store
                .servers
                .retain(|server| !result.removed.contains(&server.config.id));
            merge_configured_servers(&mut store, new_configs);
            let trust = state.trust.clone();
            apply_persisted_trust(&mut store, &trust);
            apply_settings_trust(&mut store, &settings_trust);
            let persistent_store = store.clone();
            store.servers.extend(
                state
                    .store
                    .servers
                    .iter()
                    .filter(|server| session_owner_for_server(&state, &server.config.id).is_some())
                    .cloned(),
            );
            state.store = store;
            save_store(&state.storage_dir, &persistent_store).await?;
        }

        {
            let mut backoffs = self.restart_backoffs.lock().await;
            for id in result.restarted.iter().chain(result.added.iter()) {
                backoffs.remove(id);
            }
        }

        Ok(result)
    }

    pub(crate) async fn server_snapshot(
        &self,
        server_id: &str,
    ) -> Result<StoredMcpServer, McpError> {
        self.server_snapshot_visible_to(server_id, None).await
    }

    pub(crate) async fn server_snapshot_for_session(
        &self,
        session_id: &str,
        server_id: &str,
    ) -> Result<StoredMcpServer, McpError> {
        self.server_snapshot_visible_to(server_id, Some(session_id))
            .await
    }

    async fn server_snapshot_visible_to(
        &self,
        server_id: &str,
        session_id: Option<&str>,
    ) -> Result<StoredMcpServer, McpError> {
        let state = self.state.lock().await;
        state
            .store
            .servers
            .iter()
            .find(|server| {
                server.config.id == server_id
                    && server_visible_to_session(&state, &server.config.id, session_id)
            })
            .cloned()
            .ok_or_else(|| McpError::UnknownServer(server_id.to_string()))
    }

    pub(crate) async fn set_server_status(
        &self,
        server_id: &str,
        status: McpServerStatus,
        error: Option<String>,
    ) {
        let mut state = self.state.lock().await;
        if let Some(server) = state
            .store
            .servers
            .iter_mut()
            .find(|server| server.config.id == server_id)
        {
            server.config.status = status;
            server.config.error = error;
        }
    }

    async fn ensure_stored_server(&self, server_id: &str) -> Result<(), McpError> {
        self.stored_server_config(server_id).await.map(|_| ())
    }

    async fn stored_server_config(&self, server_id: &str) -> Result<McpServerConfig, McpError> {
        let state = self.state.lock().await;
        state
            .store
            .servers
            .iter()
            .find(|server| {
                server.config.id == server_id
                    && server_visible_to_session(&state, &server.config.id, None)
            })
            .map(|server| server.config.clone())
            .ok_or_else(|| McpError::UnknownServer(server_id.to_string()))
    }
}

pub(crate) fn server_visible_to_session(
    state: &RegistryState,
    server_id: &str,
    session_id: Option<&str>,
) -> bool {
    match session_owner_for_server(state, server_id) {
        Some(owner) => session_id == Some(owner),
        None => true,
    }
}

pub(crate) fn session_owner_for_server<'a>(
    state: &'a RegistryState,
    server_id: &str,
) -> Option<&'a str> {
    state
        .session_servers
        .iter()
        .find(|(_, ids)| ids.iter().any(|id| id == server_id))
        .map(|(session_id, _)| session_id.as_str())
}

fn persistent_store_snapshot(state: &RegistryState) -> McpStore {
    McpStore {
        servers: state
            .store
            .servers
            .iter()
            .filter(|server| session_owner_for_server(state, &server.config.id).is_none())
            .cloned()
            .collect(),
    }
}

pub(crate) fn normalized_server_config(config: &McpServerConfig) -> McpServerConfig {
    let mut config = config.clone();
    if !config.enabled {
        config.status = McpServerStatus::Disabled;
        config.error = None;
    }
    config
}

fn unique_session_server_id(
    store: &McpStore,
    accepted: &[McpServerConfig],
    requested: &str,
) -> String {
    let base = if requested.trim().is_empty() {
        "mcp".to_string()
    } else {
        requested.to_string()
    };
    if !server_id_exists(store, accepted, &base) {
        return base;
    }
    for index in 2.. {
        let candidate = format!("{base}-{index}");
        if !server_id_exists(store, accepted, &candidate) {
            return candidate;
        }
    }
    unreachable!("unbounded integer suffix should find a unique MCP server id")
}

fn server_id_exists(store: &McpStore, accepted: &[McpServerConfig], id: &str) -> bool {
    store.servers.iter().any(|server| server.config.id == id)
        || accepted.iter().any(|server| server.id == id)
}

pub(crate) fn is_real_stdio_server(config: &McpServerConfig) -> bool {
    config.enabled && config.transport == crate::types::McpTransport::Stdio
}

pub(crate) fn is_real_http_server(config: &McpServerConfig) -> bool {
    config.enabled && config.transport.is_http_family()
}

pub(crate) fn is_real_websocket_server(config: &McpServerConfig) -> bool {
    config.enabled
        && config.transport == crate::types::McpTransport::WebSocket
        && (config.endpoint.starts_with("ws://") || config.endpoint.starts_with("wss://"))
}

/// Rewrite a transport-produced `AuthRequired` so its `server` field names the
/// configured server id rather than the raw endpoint URL.
pub(crate) fn canonicalize_auth_server(error: McpError, server_id: &str) -> McpError {
    match error {
        McpError::AuthRequired { server, reason } if server != server_id => {
            McpError::AuthRequired {
                server: server_id.to_string(),
                reason,
            }
        }
        other => other,
    }
}

pub(crate) fn ensure_server_trusted(
    server_id: &str,
    server: &StoredMcpServer,
) -> Result<(), McpError> {
    // Disabled servers report their own dedicated error downstream — running the trust
    // check here would mask that with a less actionable "not trusted" message.
    if !server.config.enabled {
        return Ok(());
    }
    match server.config.trust {
        McpServerTrust::Trusted => Ok(()),
        other => Err(McpError::ServerUntrusted {
            server: server_id.to_string(),
            status: other.as_str(),
        }),
    }
}

/// Map an in-memory trust state to the persisted settings-layer representation.
fn trust_to_setting(trust: McpServerTrust) -> orbcode_config::McpServerTrustSetting {
    match trust {
        McpServerTrust::Trusted => orbcode_config::McpServerTrustSetting::Trusted,
        McpServerTrust::Denied => orbcode_config::McpServerTrustSetting::Denied,
        McpServerTrust::Unknown => orbcode_config::McpServerTrustSetting::Cleared,
    }
}

fn apply_settings_trust(
    store: &mut McpStore,
    overrides: &BTreeMap<String, orbcode_config::McpServerTrustSetting>,
) {
    for server in &mut store.servers {
        match overrides.get(&server.config.id) {
            Some(orbcode_config::McpServerTrustSetting::Trusted) => {
                server.config.trust = McpServerTrust::Trusted;
            }
            Some(orbcode_config::McpServerTrustSetting::Denied) => {
                server.config.trust = McpServerTrust::Denied;
            }
            Some(orbcode_config::McpServerTrustSetting::Cleared) | None => {}
        }
    }
}

#[cfg(test)]
mod client_slot_tests {
    use super::lock_client_slot;

    #[test]
    fn client_slot_lock_recovers_after_poisoning() {
        let slot = std::sync::Mutex::new(Some(1_u8));
        let panic = std::panic::catch_unwind(|| {
            let _guard = slot.lock().expect("lock before poisoning");
            panic!("poison client slot lock");
        });
        assert!(panic.is_err());

        assert_eq!(lock_client_slot(&slot).take(), Some(1));
    }
}
