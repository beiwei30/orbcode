mod browser;
mod device;
pub(super) mod discovery;
mod ssrf;
pub(super) mod token;

use serde::Deserialize;

use crate::types::{McpDiagnosticCheck, McpDiagnosticStatus};

#[cfg(test)]
pub(crate) use browser::register_oauth_client;
pub(crate) use browser::{complete_mcp_oauth_browser_login, start_mcp_oauth_browser_login};
pub(crate) use device::{poll_mcp_oauth_device_token, start_mcp_oauth_device_login};
pub(crate) use discovery::diagnose_oauth_discovery;
pub(crate) use ssrf::mcp_endpoint_is_internal;
pub(crate) use token::{
    is_mcp_oauth_token_expired, mcp_oauth_status_entry, refresh_mcp_oauth_token,
};

#[derive(Debug, Deserialize)]
pub(crate) struct OAuthClientRegistrationResponse {
    #[serde(default)]
    pub(crate) client_id: String,
    #[serde(default)]
    pub(crate) client_secret: Option<String>,
}

pub(crate) fn unix_timestamp_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

pub(crate) fn mask_secret(secret: &str) -> String {
    let len = secret.chars().count();
    if len <= 8 {
        return "<redacted>".to_string();
    }
    let prefix: String = secret.chars().take(4).collect();
    let suffix: String = secret
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{prefix}...{suffix}")
}

fn mcp_check(
    name: impl Into<String>,
    status: McpDiagnosticStatus,
    detail: impl Into<String>,
) -> McpDiagnosticCheck {
    McpDiagnosticCheck {
        name: name.into(),
        status,
        detail: detail.into(),
    }
}
