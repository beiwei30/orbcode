use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::error::McpError;
use crate::store::{McpStore, StoredMcpServer, seed_server};
use crate::types::{
    McpAuth, McpCapability, McpConfigReloadResult, McpLoadOptions, McpPluginConfigSource,
    McpPluginConfigSourceKind, McpPluginSource, McpServerConfig, McpServerSource, McpServerStatus,
    McpServerTrust, McpTransport,
};

#[derive(Debug, Deserialize)]
struct RawMcpFile {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, Value>,
    #[serde(default, rename = "disabledMcpServers")]
    disabled_mcp_servers: Vec<String>,
    #[serde(default, rename = "disabledMcpjsonServers")]
    disabled_mcp_json_servers: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawMcpServer {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default, rename = "transportType")]
    transport_type: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    disabled: Option<bool>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Clone, Debug)]
struct ConfiguredServer {
    config: McpServerConfig,
}

#[derive(Clone, Debug)]
struct PluginLoadSource {
    plugin_id: String,
    plugin_name: String,
    label: String,
}

#[derive(Debug)]
struct McpConfigDiagnostic {
    source: String,
    path: String,
    message: String,
}

impl fmt::Display for McpConfigDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.source, self.path, self.message)
    }
}

pub(crate) async fn load_configured_servers(
    home_dir: &Path,
    cwd: &Path,
    options: &McpLoadOptions,
) -> Result<Vec<McpServerConfig>, McpError> {
    let mut loaded = Vec::new();
    let mut diagnostics = Vec::new();

    load_optional_mcp_file(
        &home_dir.join("settings.json"),
        "user settings",
        false,
        options,
        &mut loaded,
        &mut diagnostics,
    )
    .await?;

    for dir in ancestor_dirs(cwd) {
        load_optional_mcp_file(
            &dir.join(".mcp.json"),
            ".mcp.json",
            true,
            options,
            &mut loaded,
            &mut diagnostics,
        )
        .await?;
    }

    load_optional_mcp_file(
        &cwd.join(".claude/settings.json"),
        "project settings",
        false,
        options,
        &mut loaded,
        &mut diagnostics,
    )
    .await?;

    load_optional_mcp_file(
        &cwd.join(".claude/settings.local.json"),
        "project local settings",
        false,
        options,
        &mut loaded,
        &mut diagnostics,
    )
    .await?;

    for input in &options.config_inputs {
        load_mcp_config_input(input, cwd, options, &mut loaded, &mut diagnostics).await?;
    }

    for source in &options.plugin_sources {
        load_plugin_mcp_config_source(source, options, &mut loaded, &mut diagnostics).await?;
    }

    if diagnostics.is_empty() {
        Ok(loaded.into_iter().map(|c| c.config).collect())
    } else {
        Err(McpError::InvalidConfig(
            diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        ))
    }
}

pub(crate) fn merge_configured_servers(store: &mut McpStore, servers: Vec<McpServerConfig>) {
    for config in servers {
        match store
            .servers
            .iter_mut()
            .find(|server| server.config.id == config.id)
        {
            Some(server) => server.config = config,
            None => store.servers.push(seed_server(config)),
        }
    }
}

pub(crate) fn default_capabilities() -> Vec<McpCapability> {
    vec![
        McpCapability {
            transport: McpTransport::Stdio,
            enabled: true,
            note: "Local stdio server definitions can be persisted and inspected.".into(),
        },
        McpCapability {
            transport: McpTransport::StreamableHttp,
            enabled: true,
            note: "Streamable HTTP is the canonical remote MCP transport with session management, JSON and SSE response modes, and auth headers. Legacy http/https configs are accepted as aliases.".into(),
        },
        McpCapability {
            transport: McpTransport::WebSocket,
            enabled: true,
            note: "ws:// and wss:// WebSocket definitions use real JSON-RPC transport.".into(),
        },
    ]
}

pub(crate) fn server_config_changed(old: &McpServerConfig, new: &McpServerConfig) -> bool {
    old.transport != new.transport
        || old.endpoint != new.endpoint
        || old.args != new.args
        || old.env != new.env
        || old.cwd != new.cwd
        || old.headers != new.headers
        || old.enabled != new.enabled
        || old.auth != new.auth
        || old.transport_type_hint != new.transport_type_hint
        || old.source != new.source
}

pub(crate) fn diff_server_configs(
    current: &[StoredMcpServer],
    new_configs: &[McpServerConfig],
) -> McpConfigReloadResult {
    let current_by_id: BTreeMap<&str, &McpServerConfig> = current
        .iter()
        .map(|s| (s.config.id.as_str(), &s.config))
        .collect();
    let new_by_id: BTreeMap<&str, &McpServerConfig> =
        new_configs.iter().map(|c| (c.id.as_str(), c)).collect();

    let added = new_configs
        .iter()
        .filter(|c| !current_by_id.contains_key(c.id.as_str()))
        .map(|c| c.id.clone())
        .collect();

    let removed = current
        .iter()
        .filter(|s| !new_by_id.contains_key(s.config.id.as_str()))
        .map(|s| s.config.id.clone())
        .collect();

    let restarted = new_configs
        .iter()
        .filter(|new| {
            current_by_id
                .get(new.id.as_str())
                .is_some_and(|old| server_config_changed(old, new))
        })
        .map(|c| c.id.clone())
        .collect();

    McpConfigReloadResult {
        added,
        removed,
        restarted,
    }
}

pub fn scoped_plugin_server_id(plugin_id: &str, server_name: &str) -> String {
    format!(
        "plugin_{}_{}",
        provider_safe_segment(plugin_id),
        provider_safe_segment(server_name)
    )
}

fn provider_safe_segment(value: &str) -> String {
    let mut output = String::new();
    let mut last_was_separator = false;
    for ch in value.chars() {
        let next = if ch.is_ascii_alphanumeric() {
            last_was_separator = false;
            Some(ch.to_ascii_lowercase())
        } else if !last_was_separator {
            last_was_separator = true;
            Some('_')
        } else {
            None
        };
        if let Some(ch) = next {
            output.push(ch);
        }
    }
    let trimmed = output.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed
    }
}

async fn load_plugin_mcp_config_source(
    source: &McpPluginConfigSource,
    options: &McpLoadOptions,
    loaded: &mut Vec<ConfiguredServer>,
    diagnostics: &mut Vec<McpConfigDiagnostic>,
) -> Result<(), McpError> {
    let plugin = PluginLoadSource {
        plugin_id: source.plugin_id.clone(),
        plugin_name: source.plugin_name.clone(),
        label: source.label.clone(),
    };
    match &source.kind {
        McpPluginConfigSourceKind::File(path) => {
            if !tokio::fs::try_exists(path).await? {
                return Ok(());
            }
            let contents = tokio::fs::read_to_string(path).await?;
            load_mcp_config_value(
                parse_config_json(&contents, path, &source.label, diagnostics),
                source.label.clone(),
                false,
                options,
                loaded,
                diagnostics,
                Some(&plugin),
            );
        }
        McpPluginConfigSourceKind::Inline(value) => {
            load_mcp_config_value(
                Some(value.clone()),
                source.label.clone(),
                false,
                options,
                loaded,
                diagnostics,
                Some(&plugin),
            );
        }
    }
    Ok(())
}

async fn load_optional_mcp_file(
    path: &Path,
    label: &str,
    mcp_json_source: bool,
    options: &McpLoadOptions,
    loaded: &mut Vec<ConfiguredServer>,
    diagnostics: &mut Vec<McpConfigDiagnostic>,
) -> Result<(), McpError> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(());
    }
    let contents = tokio::fs::read_to_string(path).await?;
    load_mcp_config_value(
        parse_config_json(&contents, path, label, diagnostics),
        path.display().to_string(),
        mcp_json_source,
        options,
        loaded,
        diagnostics,
        None,
    );
    Ok(())
}

async fn load_mcp_config_input(
    input: &str,
    cwd: &Path,
    options: &McpLoadOptions,
    loaded: &mut Vec<ConfiguredServer>,
    diagnostics: &mut Vec<McpConfigDiagnostic>,
) -> Result<(), McpError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        load_mcp_config_value(
            Some(value),
            "command line".to_string(),
            false,
            options,
            loaded,
            diagnostics,
            None,
        );
        return Ok(());
    }

    let path = resolve_input_path(cwd, trimmed);
    if !tokio::fs::try_exists(&path).await? {
        diagnostics.push(McpConfigDiagnostic {
            source: path.display().to_string(),
            path: "mcpServers".to_string(),
            message: "--mcp-config file not found".to_string(),
        });
        return Ok(());
    }
    let contents = tokio::fs::read_to_string(&path).await?;
    load_mcp_config_value(
        parse_config_json(&contents, &path, "--mcp-config", diagnostics),
        path.display().to_string(),
        false,
        options,
        loaded,
        diagnostics,
        None,
    );
    Ok(())
}

fn parse_config_json(
    contents: &str,
    path: &Path,
    label: &str,
    diagnostics: &mut Vec<McpConfigDiagnostic>,
) -> Option<Value> {
    match serde_json::from_str(contents) {
        Ok(value) => Some(value),
        Err(error) => {
            diagnostics.push(McpConfigDiagnostic {
                source: format!("{label} {}", path.display()),
                path: "mcpServers".to_string(),
                message: format!("invalid JSON: {error}"),
            });
            None
        }
    }
}

fn load_mcp_config_value(
    value: Option<Value>,
    source: String,
    mcp_json_source: bool,
    options: &McpLoadOptions,
    loaded: &mut Vec<ConfiguredServer>,
    diagnostics: &mut Vec<McpConfigDiagnostic>,
    plugin: Option<&PluginLoadSource>,
) {
    let Some(value) = value else {
        return;
    };
    let raw = match serde_json::from_value::<RawMcpFile>(value) {
        Ok(raw) => raw,
        Err(error) => {
            diagnostics.push(McpConfigDiagnostic {
                source,
                path: "mcpServers".to_string(),
                message: format!("does not adhere to MCP server configuration schema: {error}"),
            });
            return;
        }
    };
    if raw.mcp_servers.is_empty() {
        return;
    }

    let disabled_servers = if mcp_json_source {
        raw.disabled_mcp_json_servers
    } else {
        raw.disabled_mcp_servers
    };

    for (name, value) in raw.mcp_servers {
        match parse_mcp_server(
            name.clone(),
            value,
            &source,
            mcp_json_source,
            &disabled_servers,
            options,
            plugin,
        ) {
            Ok(config) => loaded.push(ConfiguredServer { config }),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
}

fn parse_mcp_server(
    name: String,
    value: Value,
    source: &str,
    mcp_json_source: bool,
    disabled_servers: &[String],
    options: &McpLoadOptions,
    plugin: Option<&PluginLoadSource>,
) -> Result<McpServerConfig, McpConfigDiagnostic> {
    // Project `.mcp.json` servers require an explicit trust decision; other config layers
    // are user-owned (user settings, project settings, CLI) so we treat them as trusted on
    // first load. Persisted trust overrides this default at the registry level.
    let default_trust = if plugin.is_some() {
        McpServerTrust::Trusted
    } else if mcp_json_source {
        McpServerTrust::Unknown
    } else {
        McpServerTrust::Trusted
    };
    if name.trim().is_empty() {
        return Err(config_error(
            source,
            "mcpServers",
            "server name cannot be empty",
        ));
    }

    let raw = serde_json::from_value::<RawMcpServer>(value).map_err(|error| {
        config_error(
            source,
            &format!("mcpServers.{name}"),
            &format!("does not adhere to MCP server configuration schema: {error}"),
        )
    })?;
    let source_metadata = plugin.map(|plugin| {
        McpServerSource::Plugin(McpPluginSource {
            plugin_id: plugin.plugin_id.clone(),
            plugin_name: plugin.plugin_name.clone(),
            server_name: name.clone(),
            source: plugin.label.clone(),
        })
    });
    let id = plugin.map_or_else(
        || name.clone(),
        |plugin| scoped_plugin_server_id(&plugin.plugin_id, &name),
    );
    let kind = raw.kind.as_deref().unwrap_or("stdio");
    let explicitly_disabled = raw.disabled.unwrap_or(false)
        || raw.enabled == Some(false)
        || disabled_servers.iter().any(|server| server == &name);
    let summary = raw
        .summary
        .or(raw.description)
        .unwrap_or_else(|| format!("MCP server from {source}."));

    match kind {
        "stdio" => {
            let command = raw
                .command
                .filter(|command| !command.trim().is_empty())
                .ok_or_else(|| {
                    config_error(
                        source,
                        &format!("mcpServers.{name}.command"),
                        "stdio MCP servers require a non-empty command",
                    )
                })?;
            Ok(McpServerConfig {
                id,
                transport: McpTransport::Stdio,
                endpoint: expand_env_string(&command, source, "command", options)?,
                args: expand_env_vec(raw.args, source, "args", options)?,
                env: expand_env_map(raw.env, source, "env", options)?,
                cwd: expand_env_option(raw.cwd, source, "cwd", options)?,
                headers: BTreeMap::new(),
                enabled: !explicitly_disabled,
                status: if explicitly_disabled {
                    McpServerStatus::Disabled
                } else {
                    McpServerStatus::Stopped
                },
                error: None,
                summary,
                auth: McpAuth::None,
                trust: default_trust,
                transport_type_hint: None,
                source: source_metadata,
            })
        }
        "http" | "streamable_http" | "sse" | "ws" | "websocket" => {
            let url = raw
                .url
                .filter(|url| !url.trim().is_empty())
                .ok_or_else(|| {
                    config_error(
                        source,
                        &format!("mcpServers.{name}.url"),
                        "remote MCP servers require a non-empty URL",
                    )
                })?;
            let endpoint = expand_env_string(&url, source, "url", options)?;
            let transport_type_hint = raw.transport_type.as_deref();
            let effective_kind = transport_type_hint.unwrap_or(kind);
            let transport = remote_transport(effective_kind, &endpoint).ok_or_else(|| {
                config_error(
                    source,
                    &format!("mcpServers.{name}.url"),
                    "remote MCP server URL must start with http://, https://, ws://, or wss://",
                )
            })?;
            let headers = expand_env_map(raw.headers, source, "headers", options)?;
            let auth = header_auth_summary(&headers);
            Ok(McpServerConfig {
                id,
                transport,
                endpoint,
                args: Vec::new(),
                env: BTreeMap::new(),
                cwd: None,
                headers,
                enabled: !explicitly_disabled,
                status: if explicitly_disabled {
                    McpServerStatus::Disabled
                } else {
                    McpServerStatus::Ready
                },
                error: None,
                summary,
                auth,
                trust: default_trust,
                transport_type_hint: raw.transport_type.clone(),
                source: source_metadata,
            })
        }
        unsupported => Err(config_error(
            source,
            &format!("mcpServers.{name}.type"),
            &format!("unsupported MCP server type `{unsupported}`"),
        )),
    }
}

fn remote_transport(kind: &str, endpoint: &str) -> Option<McpTransport> {
    match kind {
        "ws" | "websocket" => Some(McpTransport::WebSocket),
        "streamable_http" => {
            if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
                Some(McpTransport::StreamableHttp)
            } else {
                None
            }
        }
        "sse" => {
            if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
                Some(McpTransport::StreamableHttp)
            } else {
                None
            }
        }
        _ if endpoint.starts_with("http://") || endpoint.starts_with("https://") => {
            Some(McpTransport::StreamableHttp)
        }
        _ if endpoint.starts_with("ws://") || endpoint.starts_with("wss://") => {
            Some(McpTransport::WebSocket)
        }
        _ => None,
    }
}

fn header_auth_summary(headers: &BTreeMap<String, String>) -> McpAuth {
    headers
        .iter()
        .next()
        .map_or(McpAuth::None, |(name, value)| McpAuth::Header {
            name: name.clone(),
            value: value.clone(),
        })
}

fn expand_env_option(
    value: Option<String>,
    source: &str,
    field: &str,
    options: &McpLoadOptions,
) -> Result<Option<String>, McpConfigDiagnostic> {
    value
        .map(|value| expand_env_string(&value, source, field, options))
        .transpose()
}

fn expand_env_vec(
    values: Vec<String>,
    source: &str,
    field: &str,
    options: &McpLoadOptions,
) -> Result<Vec<String>, McpConfigDiagnostic> {
    values
        .iter()
        .map(|value| expand_env_string(value, source, field, options))
        .collect()
}

fn expand_env_map(
    values: BTreeMap<String, String>,
    source: &str,
    field: &str,
    options: &McpLoadOptions,
) -> Result<BTreeMap<String, String>, McpConfigDiagnostic> {
    values
        .iter()
        .map(|(key, value)| {
            Ok((
                key.clone(),
                expand_env_string(value, source, field, options)?,
            ))
        })
        .collect()
}

fn expand_env_string(
    value: &str,
    source: &str,
    field: &str,
    options: &McpLoadOptions,
) -> Result<String, McpConfigDiagnostic> {
    let mut expanded = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        expanded.push_str(&rest[..start]);
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find('}') else {
            expanded.push_str(&rest[start..]);
            return Ok(expanded);
        };
        let expression = &after_start[..end];
        let (name, default) = expression
            .split_once(":-")
            .map_or((expression, None), |(name, default)| (name, Some(default)));
        let replacement = std::env::var(name)
            .ok()
            .or_else(|| options.env.get(name).cloned())
            .or_else(|| default.map(str::to_string))
            .ok_or_else(|| {
                config_error(
                    source,
                    field,
                    &format!("missing environment variable `{name}`"),
                )
            })?;
        expanded.push_str(&replacement);
        rest = &after_start[end + 1..];
    }
    expanded.push_str(rest);
    Ok(expanded)
}

fn config_error(source: &str, path: &str, message: &str) -> McpConfigDiagnostic {
    McpConfigDiagnostic {
        source: source.to_string(),
        path: path.to_string(),
        message: message.to_string(),
    }
}

fn ancestor_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for ancestor in cwd.ancestors() {
        dirs.push(ancestor.to_path_buf());
    }
    dirs.reverse();
    dirs
}

fn resolve_input_path(cwd: &Path, input: &str) -> PathBuf {
    let path = PathBuf::from(input);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}
