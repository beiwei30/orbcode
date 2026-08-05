use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

macro_rules! impl_list_result {
    ($name:ident, $item:ty) => {
        impl $name {
            pub fn into_inner(self) -> Vec<$item> {
                self.0
            }
        }

        impl std::ops::Deref for $name {
            type Target = [$item];

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl IntoIterator for $name {
            type Item = $item;
            type IntoIter = std::vec::IntoIter<$item>;

            fn into_iter(self) -> Self::IntoIter {
                self.0.into_iter()
            }
        }
    };
}

/// Transport selected for an MCP server.
///
/// This is a wire DTO. Runtime transport behavior remains owned by
/// `orbcode-mcp` and is converted at the app-server boundary.
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
            Self::WebSocket => "web_socket",
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

/// Authentication input accepted by MCP mutation methods.
///
/// Header values are intentionally possible here because this is a write
/// contract. Read methods use [`McpAuthOverview`] instead.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpAuth {
    None,
    BearerEnv { env_var: String },
    Header { name: String, value: String },
}

impl McpAuth {
    pub fn parse(value: Option<&str>) -> Result<Self, McpAuthParseError> {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(Self::None);
        };

        if value.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }

        if let Some(env_var) = value.strip_prefix("bearer-env:") {
            if env_var.trim().is_empty() {
                return Err(McpAuthParseError::Invalid(
                    "bearer-env auth requires an environment variable name".into(),
                ));
            }
            return Ok(Self::BearerEnv {
                env_var: env_var.trim().to_string(),
            });
        }

        if let Some(header_spec) = value.strip_prefix("header:") {
            let Some((name, raw_value)) = header_spec.split_once('=') else {
                return Err(McpAuthParseError::Invalid(
                    "header auth must use header:Name=Value".into(),
                ));
            };
            if name.trim().is_empty() || raw_value.trim().is_empty() {
                return Err(McpAuthParseError::Invalid(
                    "header auth requires both header name and value".into(),
                ));
            }
            return Ok(Self::Header {
                name: name.trim().to_string(),
                value: raw_value.trim().to_string(),
            });
        }

        Err(McpAuthParseError::Invalid(format!(
            "unsupported auth mode: {value}"
        )))
    }
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum McpAuthParseError {
    #[error("invalid MCP authentication configuration: {0}")]
    Invalid(String),
}

/// Safe authentication summary returned by read-only MCP methods.
///
/// The `value` field is retained for protocol-1.0 shape compatibility but the
/// app-server always fills it with a redaction marker.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpAuthOverview {
    None,
    BearerEnv { env_var: String },
    Header { name: String, value: String },
}

impl McpAuthOverview {
    pub fn summary(&self) -> String {
        match self {
            Self::None => "none".to_string(),
            Self::BearerEnv { env_var } => format!("bearer-env:{env_var}"),
            Self::Header { name, .. } => format!("header:{name}=<redacted>"),
        }
    }
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

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
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

/// Secret-bearing input accepted by MCP upsert and session bootstrap methods.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpServerInput {
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
    #[serde(default)]
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

/// Redacted MCP configuration/status returned by read-only protocol methods.
///
/// Collection shapes and field names intentionally match protocol 1.0. Values
/// that may contain credentials are replaced at the app-server boundary.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpServerOverview {
    pub id: String,
    pub transport: McpTransport,
    pub endpoint: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub enabled: bool,
    pub status: McpServerStatus,
    pub error: Option<String>,
    pub summary: String,
    pub auth: McpAuthOverview,
    pub trust: McpServerTrust,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_type_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<McpServerSource>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct McpListServersResult(pub Vec<McpServerOverview>);

impl McpListServersResult {
    pub fn into_inner(self) -> Vec<McpServerOverview> {
        self.0
    }
}

impl std::ops::Deref for McpListServersResult {
    type Target = [McpServerOverview];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpServerIdParams {
    pub server_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpSessionServerParams {
    pub server_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpServerTrustResult {
    pub trust: McpServerTrust,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpSetTrustParams {
    pub server_id: String,
    pub trust: McpServerTrust,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpToolSpec {
    pub name: String,
    pub summary: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct McpListToolsResult(pub Vec<McpToolSpec>);

impl_list_result!(McpListToolsResult, McpToolSpec);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct McpResourceSummary {
    pub uri: String,
    pub name: String,
    pub mime_type: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpAnnotations>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(transparent)]
pub struct McpListResourcesResult(pub Vec<McpResourceSummary>);

impl_list_result!(McpListResourcesResult, McpResourceSummary);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpReadResourceParams {
    pub server_id: String,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct McpResourceContent {
    pub uri: String,
    pub mime_type: String,
    pub contents: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    #[serde(default)]
    pub is_binary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpAnnotations>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct McpPrompt {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub skill: bool,
    /// Extension-defined prompt metadata retained without interpretation.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpPromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(transparent)]
pub struct McpListPromptsResult(pub Vec<McpPrompt>);

impl_list_result!(McpListPromptsResult, McpPrompt);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct McpGetPromptParams {
    pub server_id: String,
    pub name: String,
    /// MCP prompt arguments are extension-defined.
    #[serde(default)]
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpInvokeToolParams {
    pub server_id: String,
    pub tool_name: String,
    #[serde(default = "default_empty_object")]
    pub input: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpToolResult {
    pub server_id: String,
    pub tool_name: String,
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpDiagnosticStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpDiagnosticCheck {
    pub name: String,
    pub status: McpDiagnosticStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct McpDiagnoseResult(pub Vec<McpDiagnosticCheck>);

impl_list_result!(McpDiagnoseResult, McpDiagnosticCheck);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpRemoveServerResult {
    pub removed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpCapability {
    pub transport: McpTransport,
    pub enabled: bool,
    pub note: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct McpCapabilitiesResult(pub Vec<McpCapability>);

impl_list_result!(McpCapabilitiesResult, McpCapability);

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpOAuthOverviewParams {
    pub server_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpLogoutOAuthTokenResult {
    pub logged_out: bool,
}

/// Secret-free OAuth status returned by `mcp/oauth_overview`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpOAuthOverview {
    pub store_path: PathBuf,
    pub entries: Vec<McpOAuthStatusEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct McpOAuthStatusEntry {
    pub server_id: String,
    pub source_summary: String,
    pub usable: bool,
    pub expired: bool,
    pub has_refresh_token: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_token_endpoint: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

/// Result of `prompts/get`: the rendered prompt messages.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct McpPromptResult {
    pub description: String,
    pub messages: Vec<McpPromptMessage>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct McpPromptMessage {
    pub role: String,
    pub content: McpContent,
}

/// A protocol-owned MCP content block.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
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

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct McpAnnotations {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audience: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_empty_object() -> String {
    "{}".to_string()
}
