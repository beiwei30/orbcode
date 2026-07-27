use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::McpError;
use crate::types::{McpServerConfig, McpServerTrust, McpToolSpec};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StoredMcpResource {
    pub(crate) uri: String,
    pub(crate) name: String,
    pub(crate) mime_type: String,
    pub(crate) description: String,
    pub(crate) contents: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StoredMcpServer {
    pub(crate) config: McpServerConfig,
    pub(crate) resources: Vec<StoredMcpResource>,
    pub(crate) tools: Vec<McpToolSpec>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct McpStore {
    pub(crate) servers: Vec<StoredMcpServer>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct McpTrustStore {
    #[serde(default)]
    pub(crate) servers: BTreeMap<String, McpServerTrust>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct McpOAuthTokenStore {
    #[serde(default)]
    pub(crate) servers: BTreeMap<String, StoredMcpOAuthToken>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StoredMcpOAuthToken {
    pub(crate) access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires_at: Option<i64>,
    #[serde(default)]
    pub(crate) scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) updated_at: Option<i64>,
    /// The SSRF/TLS enforcement decision captured when this token was obtained
    /// (public MCP endpoint → enforce). It is FROZEN here so a later change to
    /// the server's endpoint (e.g. public → local via `upsert_server`) cannot
    /// downgrade a refresh of this token to `enforce=false` and re-open a DNS
    /// path to an internal address. Old tokens without the field default to
    /// enforcing (fail safe).
    #[serde(default = "default_enforce_ssrf")]
    pub(crate) enforce_ssrf: bool,
}

fn default_enforce_ssrf() -> bool {
    true
}

pub(crate) fn seed_server(config: McpServerConfig) -> StoredMcpServer {
    let info_resource = StoredMcpResource {
        uri: format!("mcp://{}/info", config.id),
        name: format!("{} info", config.id),
        mime_type: "text/plain".to_string(),
        description: "Seeded description for the configured MCP server.".to_string(),
        contents: format!(
            "id={}\ntransport={}\nendpoint={}\nauth={}\nsummary={}",
            config.id,
            config.transport,
            config.endpoint,
            config.auth.summary(),
            config.summary
        ),
    };
    let transport_resource = StoredMcpResource {
        uri: format!("mcp://{}/transport", config.id),
        name: format!("{} transport", config.id),
        mime_type: "text/plain".to_string(),
        description: "Seeded transport and auth metadata.".to_string(),
        contents: format!(
            "transport={}\nenabled={}\nauth={}",
            config.transport,
            config.enabled,
            config.auth.summary()
        ),
    };

    StoredMcpServer {
        config,
        resources: vec![info_resource, transport_resource],
        tools: vec![
            McpToolSpec {
                name: "echo".to_string(),
                summary: "Echo the provided input with server metadata.".to_string(),
            },
            McpToolSpec {
                name: "inspect".to_string(),
                summary: "Describe the persisted server configuration.".to_string(),
            },
        ],
    }
}

pub(crate) fn apply_persisted_trust(store: &mut McpStore, trust: &McpTrustStore) {
    for server in &mut store.servers {
        if let Some(persisted) = trust.servers.get(&server.config.id).copied() {
            server.config.trust = persisted;
        }
    }
}

pub(crate) async fn save_store(storage_dir: &Path, store: &McpStore) -> Result<(), McpError> {
    let path = storage_dir.join("servers.json");
    let tmp_path = storage_dir.join("servers.json.tmp");
    let contents = serde_json::to_string_pretty(store)?;
    tokio::fs::write(&tmp_path, contents).await?;
    tokio::fs::rename(&tmp_path, &path).await?;
    Ok(())
}

pub(crate) async fn save_trust(storage_dir: &Path, trust: &McpTrustStore) -> Result<(), McpError> {
    let path = storage_dir.join("trust.json");
    let tmp_path = storage_dir.join("trust.json.tmp");
    let contents = serde_json::to_string_pretty(trust)?;
    tokio::fs::write(&tmp_path, contents).await?;
    tokio::fs::rename(&tmp_path, &path).await?;
    Ok(())
}

pub(crate) async fn save_oauth_tokens(
    storage_dir: &Path,
    tokens: &McpOAuthTokenStore,
) -> Result<(), McpError> {
    let path = storage_dir.join("oauth_tokens.json");
    let tmp_path = storage_dir.join("oauth_tokens.json.tmp");
    let contents = serde_json::to_string_pretty(tokens)?;
    tokio::fs::write(&tmp_path, contents).await?;
    #[cfg(unix)]
    tokio::fs::set_permissions(
        &tmp_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .await?;
    tokio::fs::rename(&tmp_path, &path).await?;
    Ok(())
}
