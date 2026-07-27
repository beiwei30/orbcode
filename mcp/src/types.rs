use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::TcpListener;

use crate::error::McpError;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    Stdio,
    Http,
    Https,
    StreamableHttp,
    WebSocket,
}

impl McpTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Https => "https",
            Self::StreamableHttp => "streamable_http",
            Self::WebSocket => "websocket",
        }
    }

    pub fn is_http_family(self) -> bool {
        matches!(self, Self::Http | Self::Https | Self::StreamableHttp)
    }
}

impl fmt::Display for McpTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str((*self).as_str())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpAuth {
    None,
    BearerEnv { env_var: String },
    Header { name: String, value: String },
}

impl McpAuth {
    pub fn parse(value: Option<&str>) -> Result<Self, McpError> {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(Self::None);
        };

        if value.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }

        if let Some(env_var) = value.strip_prefix("bearer-env:") {
            if env_var.trim().is_empty() {
                return Err(McpError::InvalidConfig(
                    "bearer-env auth requires an environment variable name".into(),
                ));
            }
            return Ok(Self::BearerEnv {
                env_var: env_var.trim().to_string(),
            });
        }

        if let Some(header_spec) = value.strip_prefix("header:") {
            let Some((name, raw_value)) = header_spec.split_once('=') else {
                return Err(McpError::InvalidConfig(
                    "header auth must use header:Name=Value".into(),
                ));
            };
            if name.trim().is_empty() || raw_value.trim().is_empty() {
                return Err(McpError::InvalidConfig(
                    "header auth requires both header name and value".into(),
                ));
            }
            return Ok(Self::Header {
                name: name.trim().to_string(),
                value: raw_value.trim().to_string(),
            });
        }

        Err(McpError::InvalidConfig(format!(
            "unsupported auth mode: {value}"
        )))
    }

    pub fn summary(&self) -> String {
        match self {
            Self::None => "none".to_string(),
            Self::BearerEnv { env_var } => format!("bearer-env:{env_var}"),
            Self::Header { name, .. } => format!("header:{name}=<redacted>"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpCapability {
    pub transport: McpTransport,
    pub enabled: bool,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpOAuthTokenInput {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_endpoint: Option<String>,
    pub client_id: Option<String>,
    pub expires_at: Option<i64>,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpOAuthDeviceLoginInput {
    pub device_authorization_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    pub client_id: String,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpOAuthDeviceLoginSession {
    pub server_id: String,
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_at: i64,
    pub interval_secs: u64,
    pub token_endpoint: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    /// Whether SSRF/TLS enforcement applies to this flow, decided at login start
    /// from the MCP endpoint (public → enforce). Carried through to the token
    /// poll so a deliberately-local MCP whose authorization server uses a private
    /// DNS name (e.g. `http://keycloak:8080`) is not wrongly rejected later.
    pub enforce_ssrf: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpOAuthBrowserLoginInput {
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    /// Pre-registered OAuth client id. When `None`, browser login attempts RFC
    /// 7591 dynamic client registration against `registration_endpoint` (or the
    /// one advertised by OAuth discovery).
    pub client_id: Option<String>,
    /// RFC 7591 dynamic client registration endpoint override. When `None`, the
    /// endpoint advertised by OAuth discovery is used (if any).
    pub registration_endpoint: Option<String>,
    pub scopes: Vec<String>,
    pub redirect_port: Option<u16>,
}

pub struct McpOAuthBrowserLoginSession {
    pub server_id: String,
    pub authorization_url: String,
    pub redirect_uri: String,
    pub expires_at: i64,
    pub(crate) token_endpoint: String,
    pub(crate) client_id: String,
    pub(crate) client_secret: Option<String>,
    pub(crate) scopes: Vec<String>,
    pub(crate) state: String,
    /// Whether SSRF/TLS enforcement applies to this flow, decided at login start
    /// from the MCP endpoint and carried through to the token exchange (see
    /// [`McpOAuthDeviceLoginSession::enforce_ssrf`]).
    pub(crate) enforce_ssrf: bool,
    /// PKCE (RFC 7636) code verifier, sent in the token exchange to prove this
    /// client initiated the flow — an intercepted authorization code cannot be
    /// redeemed without it.
    pub(crate) code_verifier: String,
    pub(crate) listener: TcpListener,
}

impl fmt::Debug for McpOAuthBrowserLoginSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpOAuthBrowserLoginSession")
            .field("server_id", &self.server_id)
            .field("authorization_url", &self.authorization_url)
            .field("redirect_uri", &self.redirect_uri)
            .field("expires_at", &self.expires_at)
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field("has_client_secret", &self.client_secret.is_some())
            .field("scopes", &self.scopes)
            .field("state", &"<redacted>")
            .field("listener", &"<tcp listener>")
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpOAuthStatusEntry {
    pub server_id: String,
    pub source_summary: String,
    pub usable: bool,
    pub expired: bool,
    pub has_refresh_token: bool,
    pub has_token_endpoint: bool,
    pub expires_at: Option<i64>,
    pub scopes: Vec<String>,
    pub updated_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpOAuthOverview {
    pub store_path: PathBuf,
    pub entries: Vec<McpOAuthStatusEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpServerConfig {
    pub id: String,
    pub transport: McpTransport,
    pub endpoint: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub enabled: bool,
    #[serde(default = "default_mcp_server_status")]
    pub status: McpServerStatus,
    #[serde(default)]
    pub error: Option<String>,
    pub summary: String,
    pub auth: McpAuth,
    #[serde(default)]
    pub trust: McpServerTrust,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_type_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<McpServerSource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpServerSource {
    Plugin(McpPluginSource),
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpPluginSource {
    pub plugin_id: String,
    pub plugin_name: String,
    pub server_name: String,
    pub source: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpServerTrust {
    #[default]
    Unknown,
    Trusted,
    Denied,
}

impl McpServerTrust {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Trusted => "trusted",
            Self::Denied => "denied",
        }
    }

    pub fn is_trusted(self) -> bool {
        matches!(self, Self::Trusted)
    }
}

impl fmt::Display for McpServerTrust {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str((*self).as_str())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum McpServerStatus {
    Disabled,
    Starting,
    #[default]
    Ready,
    Failed,
    Unauthorized,
    Restarting,
    Stopped,
}

impl McpServerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Unauthorized => "unauthorized",
            Self::Restarting => "restarting",
            Self::Stopped => "stopped",
        }
    }
}

fn default_mcp_server_status() -> McpServerStatus {
    McpServerStatus::Ready
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpResourceSummary {
    pub uri: String,
    pub name: String,
    pub mime_type: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpAnnotations>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpResourceContent {
    pub uri: String,
    pub mime_type: String,
    /// Decoded text payload. Empty when the resource is binary (`is_binary`).
    pub contents: String,
    /// Base64-encoded binary payload, present only when `is_binary` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    /// Marks blob (base64 binary) resources so callers keep them off the text path.
    #[serde(default)]
    pub is_binary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpAnnotations>,
}

/// Resource template advertised by `resources/templates/list`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpResourceTemplate {
    pub uri_template: String,
    pub name: String,
    pub mime_type: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpAnnotations>,
}

/// MCP annotations (e.g. `audience`, `priority`) preserved from list/read results.
/// Unrecognized keys are retained in `extra` so nothing is dropped on round-trip.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct McpAnnotations {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audience: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Prompt advertised by `prompts/list`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpPrompt {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub skill: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpPromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}

/// Result of `prompts/get`: the rendered prompt messages.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpPromptResult {
    pub description: String,
    pub messages: Vec<McpPromptMessage>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpPromptMessage {
    pub role: String,
    pub content: McpContent,
}

/// A single content block from a prompt message or resource read. Binary payloads
/// (image/audio `data` or resource `blob`) are kept in `binary` and flagged with
/// `is_binary` so text consumers never see base64 noise.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpContent {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    #[serde(default)]
    pub is_binary: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpAnnotations>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolSpec {
    pub name: String,
    pub summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolResult {
    pub server_id: String,
    pub tool_name: String,
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolDescriptor {
    pub server_id: String,
    pub tool_name: String,
    pub description: String,
    pub input_schema: Value,
    pub source: Option<McpServerSource>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpDiagnosticStatus {
    Pass,
    Warn,
    Fail,
}

impl fmt::Display for McpDiagnosticStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpDiagnosticCheck {
    pub name: String,
    pub status: McpDiagnosticStatus,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Config reload result
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpConfigReloadResult {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub restarted: Vec<String>,
}

// ---------------------------------------------------------------------------
// Trust approval flow types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustApprovalRequest {
    pub request_id: String,
    pub server_id: String,
    pub tool_name: String,
    pub server_source: Option<McpServerSource>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrustApprovalResponse {
    Trusted,
    Denied,
}

pub trait TrustApprovalHandler: Send + Sync {
    fn request_trust_approval(
        &self,
        request: TrustApprovalRequest,
    ) -> impl std::future::Future<Output = Option<TrustApprovalResponse>> + Send;
}

pub type SharedTrustApprovalHandler = Arc<dyn ErasedTrustApprovalHandler>;

pub trait ErasedTrustApprovalHandler: Send + Sync {
    fn request_trust_approval(
        &self,
        request: TrustApprovalRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Option<TrustApprovalResponse>> + Send + '_>,
    >;
}

impl<T: TrustApprovalHandler> ErasedTrustApprovalHandler for T {
    fn request_trust_approval(
        &self,
        request: TrustApprovalRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Option<TrustApprovalResponse>> + Send + '_>,
    > {
        Box::pin(TrustApprovalHandler::request_trust_approval(self, request))
    }
}

// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct McpLoadOptions {
    pub config_inputs: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub plugin_sources: Vec<McpPluginConfigSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpPluginConfigSource {
    pub plugin_id: String,
    pub plugin_name: String,
    pub label: String,
    pub kind: McpPluginConfigSourceKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpPluginConfigSourceKind {
    File(PathBuf),
    Inline(Value),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_type_serde_round_trip() {
        let variants = [
            (McpTransport::Stdio, "\"stdio\""),
            (McpTransport::Http, "\"http\""),
            (McpTransport::Https, "\"https\""),
            (McpTransport::StreamableHttp, "\"streamable_http\""),
            (McpTransport::WebSocket, "\"web_socket\""),
        ];
        for (variant, expected_json) in variants {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_json, "serialize {variant:?}");
            let deserialized: McpTransport = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, variant, "deserialize {expected_json}");
        }
    }

    #[test]
    fn transport_type_legacy_http_https_deserializes() {
        let http: McpTransport = serde_json::from_str("\"http\"").unwrap();
        assert_eq!(http, McpTransport::Http);
        let https: McpTransport = serde_json::from_str("\"https\"").unwrap();
        assert_eq!(https, McpTransport::Https);
    }

    #[test]
    fn transport_type_streamable_http_deserializes() {
        let t: McpTransport = serde_json::from_str("\"streamable_http\"").unwrap();
        assert_eq!(t, McpTransport::StreamableHttp);
    }

    #[test]
    fn transport_type_is_http_family() {
        assert!(McpTransport::Http.is_http_family());
        assert!(McpTransport::Https.is_http_family());
        assert!(McpTransport::StreamableHttp.is_http_family());
        assert!(!McpTransport::Stdio.is_http_family());
        assert!(!McpTransport::WebSocket.is_http_family());
    }

    #[test]
    fn transport_type_display() {
        assert_eq!(McpTransport::Stdio.to_string(), "stdio");
        assert_eq!(McpTransport::Http.to_string(), "http");
        assert_eq!(McpTransport::Https.to_string(), "https");
        assert_eq!(McpTransport::StreamableHttp.to_string(), "streamable_http");
        assert_eq!(McpTransport::WebSocket.to_string(), "websocket");
    }

    #[test]
    fn transport_type_sse_not_a_variant() {
        let result = serde_json::from_str::<McpTransport>("\"sse\"");
        assert!(result.is_err());
    }
}
