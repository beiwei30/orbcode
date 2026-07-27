use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::McpError;
use crate::types::{
    McpAnnotations, McpContent, McpPrompt, McpPromptArgument, McpPromptMessage, McpPromptResult,
    McpResourceContent, McpResourceSummary, McpResourceTemplate,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct StdioInitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: Value,
    #[serde(rename = "serverInfo")]
    pub server_info: StdioServerInfo,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct StdioServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct StdioListToolsResult {
    pub tools: Vec<StdioToolSpec>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct StdioToolSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct StdioToolCallResult {
    pub content: Vec<StdioContentBlock>,
    #[serde(default, rename = "isError")]
    pub is_error: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct StdioContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct ListResourcesResult {
    #[serde(default)]
    pub(crate) resources: Vec<RawResource>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct RawResource {
    uri: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "mimeType")]
    mime_type: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    annotations: Option<McpAnnotations>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct ListResourceTemplatesResult {
    #[serde(default, rename = "resourceTemplates")]
    pub(crate) resource_templates: Vec<RawResourceTemplate>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct RawResourceTemplate {
    #[serde(rename = "uriTemplate")]
    uri_template: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "mimeType")]
    mime_type: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    annotations: Option<McpAnnotations>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct ReadResourceResult {
    #[serde(default)]
    pub(crate) contents: Vec<RawResourceContents>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct RawResourceContents {
    uri: String,
    #[serde(default, rename = "mimeType")]
    mime_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    blob: Option<String>,
    #[serde(default)]
    annotations: Option<McpAnnotations>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct ListPromptsResult {
    #[serde(default)]
    pub(crate) prompts: Vec<RawPrompt>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct RawPrompt {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    skill: bool,
    #[serde(default, rename = "_meta")]
    meta: Value,
    #[serde(default)]
    arguments: Vec<RawPromptArgument>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct RawPromptArgument {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    required: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct GetPromptResult {
    #[serde(default)]
    description: String,
    #[serde(default)]
    messages: Vec<RawPromptMessage>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct RawPromptMessage {
    role: String,
    content: RawContentBlock,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct RawContentBlock {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    blob: Option<String>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default, rename = "mimeType")]
    mime_type: String,
    /// Embedded resource payload for `type:"resource"` content blocks; the
    /// actual text/blob is nested here rather than at the top level.
    #[serde(default)]
    resource: Option<RawResourceContents>,
    #[serde(default)]
    annotations: Option<McpAnnotations>,
}

impl From<RawResource> for McpResourceSummary {
    fn from(raw: RawResource) -> Self {
        Self {
            uri: raw.uri,
            name: raw.name,
            mime_type: raw.mime_type,
            description: raw.description,
            annotations: raw.annotations,
        }
    }
}

impl From<RawResourceTemplate> for McpResourceTemplate {
    fn from(raw: RawResourceTemplate) -> Self {
        Self {
            uri_template: raw.uri_template,
            name: raw.name,
            mime_type: raw.mime_type,
            description: raw.description,
            annotations: raw.annotations,
        }
    }
}

impl From<RawResourceContents> for McpResourceContent {
    fn from(raw: RawResourceContents) -> Self {
        let is_binary = raw.blob.is_some();
        Self {
            uri: raw.uri,
            mime_type: raw.mime_type,
            contents: raw.text.unwrap_or_default(),
            blob: raw.blob,
            is_binary,
            annotations: raw.annotations,
        }
    }
}

impl From<RawContentBlock> for McpContent {
    fn from(raw: RawContentBlock) -> Self {
        // `type:"resource"` blocks nest their payload under `resource`; flatten it so
        // the text/blob and mime type are read from the same place as direct content.
        let embedded = raw.resource;
        let text = raw
            .text
            .or_else(|| embedded.as_ref().and_then(|r| r.text.clone()));
        // Resource blobs use `blob`; image/audio content uses `data`. Either form is
        // base64 binary that must be flagged and kept out of the text path.
        let binary = raw
            .blob
            .or(raw.data)
            .or_else(|| embedded.as_ref().and_then(|r| r.blob.clone()));
        let is_binary = binary.is_some();
        let mime_type = if raw.mime_type.is_empty() {
            embedded
                .as_ref()
                .map(|r| r.mime_type.clone())
                .unwrap_or_default()
        } else {
            raw.mime_type
        };
        let annotations = raw
            .annotations
            .or_else(|| embedded.and_then(|r| r.annotations));
        Self {
            kind: raw.kind,
            text,
            binary,
            is_binary,
            mime_type,
            annotations,
        }
    }
}

impl From<RawPrompt> for McpPrompt {
    fn from(raw: RawPrompt) -> Self {
        let skill = raw.skill
            || raw
                .meta
                .get("skill")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        Self {
            name: raw.name,
            description: raw.description,
            arguments: raw
                .arguments
                .into_iter()
                .map(|argument| McpPromptArgument {
                    name: argument.name,
                    description: argument.description,
                    required: argument.required,
                })
                .collect(),
            skill,
            extra: raw.extra,
        }
    }
}

impl From<RawPromptMessage> for McpPromptMessage {
    fn from(raw: RawPromptMessage) -> Self {
        Self {
            role: raw.role,
            content: raw.content.into(),
        }
    }
}

impl From<GetPromptResult> for McpPromptResult {
    fn from(raw: GetPromptResult) -> Self {
        Self {
            description: raw.description,
            messages: raw
                .messages
                .into_iter()
                .map(McpPromptMessage::from)
                .collect(),
        }
    }
}

pub(crate) fn parse_http_json_rpc_response(text: &str, request_id: u64) -> Result<Value, McpError> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        return Ok(serde_json::from_str(trimmed)?);
    }

    let mut data_buf = String::new();
    let mut matched: Option<Value> = None;
    let target_id = Value::from(request_id);

    for line in trimmed.lines() {
        let line = line.trim_start();
        if line.is_empty() {
            if !data_buf.is_empty() {
                if let Ok(value) = serde_json::from_str::<Value>(data_buf.trim())
                    && value.get("id") == Some(&target_id)
                {
                    matched = Some(value);
                }
                data_buf.clear();
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("data:") {
            data_buf.push_str(value.trim_start());
        }
    }
    if !data_buf.is_empty()
        && let Ok(value) = serde_json::from_str::<Value>(data_buf.trim())
        && value.get("id") == Some(&target_id)
    {
        matched = Some(value);
    }

    matched.ok_or_else(|| {
        McpError::Protocol(
            "remote server SSE response did not contain a matching JSON-RPC response".to_string(),
        )
    })
}

pub(crate) fn parse_json_rpc_result<T: DeserializeOwned>(
    response: Value,
    id: u64,
    method: &str,
) -> Result<T, McpError> {
    if response.get("jsonrpc") != Some(&Value::String("2.0".to_string())) {
        return Err(McpError::Protocol(format!(
            "invalid JSON-RPC version in {method} response"
        )));
    }
    if response.get("id") != Some(&json!(id)) {
        return Err(McpError::Protocol(format!(
            "mismatched JSON-RPC id in {method} response"
        )));
    }
    if let Some(error) = response.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown JSON-RPC error")
            .to_string();
        return Err(McpError::JsonRpc { code, message });
    }

    let result = response
        .get("result")
        .cloned()
        .ok_or_else(|| McpError::Protocol(format!("missing result in {method} response")))?;
    Ok(serde_json::from_value(result)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sse_skips_notifications_and_finds_matching_response() {
        let sse = "\
event: message\n\
data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"token\":1}}\n\
\n\
event: message\n\
data: {\"jsonrpc\":\"2.0\",\"id\":42,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n\
\n";
        let result = parse_http_json_rpc_response(sse, 42).unwrap();
        assert_eq!(result["id"], 42);
        assert_eq!(result["result"]["content"][0]["text"], "ok");
    }

    #[test]
    fn parse_sse_wrong_id_returns_error() {
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{}}\n\n";
        assert!(parse_http_json_rpc_response(sse, 1).is_err());
    }
}
