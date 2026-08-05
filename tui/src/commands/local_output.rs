use std::collections::BTreeMap;

use anyhow::Result;
use orbcode_app_server_client::{
    AppClient, McpAuth, McpOAuthOverview, McpOAuthStatusEntry, McpServerInput, McpServerStatus,
    McpServerTrust, McpTransport, ProviderRequestDebugSnapshot,
};

use crate::commands::utils::split_first_word;
use crate::history_cell::local_note::nonempty_detail;
use crate::render::slash_output::render_last_provider_request_snapshot;
use crate::slash_commands::LocalOutputSlashCommand;
use crate::state::TuiState;

pub(crate) struct LocalOutputCommandResult {
    pub(crate) summary: String,
    pub(crate) detail: Option<String>,
    pub(crate) status: String,
}

impl LocalOutputCommandResult {
    fn new(summary: impl Into<String>, detail: Option<String>) -> Self {
        let summary = summary.into();
        Self {
            status: summary.clone(),
            summary,
            detail,
        }
    }

    fn with_status(
        summary: impl Into<String>,
        detail: Option<String>,
        status: impl Into<String>,
    ) -> Self {
        Self {
            summary: summary.into(),
            detail,
            status: status.into(),
        }
    }
}

pub(crate) async fn run_local_output_slash_command(
    command: LocalOutputSlashCommand,
    args: &str,
    app_server: &AppClient,
) -> Result<LocalOutputCommandResult> {
    match command {
        LocalOutputSlashCommand::LastRequest => {
            Ok(run_last_request_slash_command(app_server).await)
        }
        LocalOutputSlashCommand::Tools => Ok(run_tools_slash_command(app_server).await),
        LocalOutputSlashCommand::McpInspection => {
            run_mcp_inspection_slash_command(args, app_server).await
        }
    }
}

impl TuiState {
    pub(crate) async fn run_local_output_slash_command(
        &mut self,
        command: LocalOutputSlashCommand,
        args: &str,
        line: &str,
        app_server: &AppClient,
    ) -> Result<()> {
        let LocalOutputCommandResult {
            summary,
            detail,
            status,
        } = self::run_local_output_slash_command(command, args, app_server).await?;
        self.push_local_slash_command_output(line, summary, detail);
        self.set_status_line(status);
        Ok(())
    }
}

async fn run_last_request_slash_command(app_server: &AppClient) -> LocalOutputCommandResult {
    let no_capture = || {
        LocalOutputCommandResult::new(
            "No provider request captured yet.",
            Some("Send a prompt first, then run /trace.".to_string()),
        )
    };
    let value = match app_server.last_provider_request_snapshot().await {
        Ok(result) => match result.0 {
            Some(value) => value,
            None => return no_capture(),
        },
        _ => return no_capture(),
    };
    match orbcode_protocol::ProviderId::parse(&value.provider) {
        Some(provider) => {
            let snapshot = ProviderRequestDebugSnapshot {
                provider,
                source: value.source,
                session_id: value.session_id,
                model: value.model,
                base_url: value.base_url,
                captured_at: value.captured_at,
                recent_activity_json: value.recent_activity_json,
                previous_turn_json: value.previous_turn_json,
                body_json: value.body_json,
            };
            let (summary, detail) = render_last_provider_request_snapshot(&snapshot);
            LocalOutputCommandResult::new(summary, Some(detail))
        }
        None => no_capture(),
    }
}

async fn run_tools_slash_command(app_server: &AppClient) -> LocalOutputCommandResult {
    let tools_result = app_server.list_tools().await;
    let rendered = match tools_result {
        Ok(tools) => tools
            .iter()
            .map(|tool| {
                format!(
                    "{}  tools={} network={} {}",
                    tool.name,
                    tool.requires_tools_permission,
                    tool.requires_network_permission,
                    tool.summary
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(_) => String::new(),
    };
    LocalOutputCommandResult::new("Listed tool registry.", nonempty_detail(rendered))
}

async fn run_mcp_inspection_slash_command(
    args: &str,
    app_server: &AppClient,
) -> Result<LocalOutputCommandResult> {
    let (subcommand, rest) =
        split_first_word(args).ok_or_else(|| anyhow::anyhow!("unknown slash command"))?;
    match subcommand {
        "capabilities" => {
            if !rest.trim().is_empty() {
                return Err(anyhow::anyhow!("unknown slash command"));
            }
            let capabilities = app_server
                .mcp_capabilities()
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let rendered = capabilities
                .iter()
                .map(|capability| {
                    format!(
                        "{} enabled={} {}",
                        capability.transport, capability.enabled, capability.note
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(LocalOutputCommandResult::new(
                "Listed MCP transport capabilities.",
                nonempty_detail(rendered),
            ))
        }
        "servers" | "list" | "status" => {
            if !rest.trim().is_empty() {
                return Err(anyhow::anyhow!("usage: /mcp {subcommand}"));
            }
            run_mcp_server_table(subcommand, app_server).await
        }
        "add" => run_mcp_add_slash_command(rest, app_server).await,
        "remove" => run_mcp_remove_slash_command(rest, app_server).await,
        "trust" => {
            run_mcp_trust_slash_command(rest, app_server, McpServerTrust::Trusted, "Trusted").await
        }
        "distrust" => {
            run_mcp_trust_slash_command(rest, app_server, McpServerTrust::Denied, "Denied").await
        }
        "untrust" => {
            run_mcp_trust_slash_command(
                rest,
                app_server,
                McpServerTrust::Unknown,
                "Cleared trust for",
            )
            .await
        }
        "resources" => {
            let server_id = rest.trim();
            if server_id.is_empty() {
                return Err(anyhow::anyhow!("usage: /mcp resources <server>"));
            }
            let resources = app_server
                .list_mcp_resources(server_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let rendered = resources
                .iter()
                .map(|resource| {
                    format!(
                        "{}  {}  {}",
                        resource.uri, resource.mime_type, resource.description
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let status = format!("Listed MCP resources from `{server_id}`.");
            Ok(LocalOutputCommandResult::with_status(
                status.clone(),
                nonempty_detail(rendered),
                status,
            ))
        }
        "tools" => {
            let server_id = rest.trim();
            if server_id.is_empty() {
                return Err(anyhow::anyhow!("usage: /mcp tools <server>"));
            }
            let tools = app_server
                .list_mcp_tools(server_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let rendered = tools
                .iter()
                .map(|tool| format!("{}  {}", tool.name, tool.summary))
                .collect::<Vec<_>>()
                .join("\n");
            let status = format!("Listed MCP tools from `{server_id}`.");
            Ok(LocalOutputCommandResult::with_status(
                status.clone(),
                nonempty_detail(rendered),
                status,
            ))
        }
        "auth" => run_mcp_auth_slash_command(rest, app_server).await,
        _ => Err(anyhow::anyhow!("unknown slash command")),
    }
}

/// Render the configured MCP servers (including the trust column) so the TUI
/// `/mcp list` and `/mcp status` views match `orbcode mcp servers`.
async fn run_mcp_server_table(
    subcommand: &str,
    app_server: &AppClient,
) -> Result<LocalOutputCommandResult> {
    let servers = app_server
        .list_mcp_servers()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if servers.is_empty() {
        return Ok(LocalOutputCommandResult::new(
            "No MCP servers configured.",
            Some(
                "Add one with `/mcp add <id> <transport> <endpoint>`, ~/.claude/settings.json, or .mcp.json."
                    .to_string(),
            ),
        ));
    }
    let rendered = servers
        .iter()
        .map(|server| {
            let error = server
                .error
                .as_ref()
                .map(|error| format!(" error={error}"))
                .unwrap_or_default();
            format!(
                "{} {} status={} trust={} enabled={} auth={} {}{}",
                server.id,
                server.transport,
                server.status.as_str(),
                server.trust.as_str(),
                server.enabled,
                server.auth.summary(),
                server.endpoint,
                error
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let status = if subcommand == "status" {
        "Showed MCP server status."
    } else {
        "Listed MCP servers."
    };
    Ok(LocalOutputCommandResult::new(
        status,
        nonempty_detail(rendered),
    ))
}

async fn run_mcp_add_slash_command(
    rest: &str,
    app_server: &AppClient,
) -> Result<LocalOutputCommandResult> {
    let usage = "usage: /mcp add <id> <transport> <endpoint> [summary]";
    let (id, after_id) = split_first_word(rest).ok_or_else(|| anyhow::anyhow!(usage))?;
    let (transport_token, after_transport) =
        split_first_word(after_id).ok_or_else(|| anyhow::anyhow!(usage))?;
    let (endpoint, summary_rest) =
        split_first_word(after_transport).ok_or_else(|| anyhow::anyhow!(usage))?;
    let transport = parse_mcp_transport(transport_token)?;
    let summary = {
        let trimmed = summary_rest.trim();
        if trimmed.is_empty() {
            format!("{id} ({transport})")
        } else {
            trimmed.to_string()
        }
    };
    let config = McpServerInput {
        id: id.to_string(),
        transport,
        endpoint: endpoint.to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        headers: BTreeMap::new(),
        enabled: true,
        status: McpServerStatus::Ready,
        error: None,
        summary,
        auth: McpAuth::None,
        // Register untrusted: the Unknown-until-trusted model requires the user
        // to explicitly approve a server before its tools can be invoked.
        // Registering as `Trusted` here would let a freshly-added server run
        // tools with no confirmation.
        trust: McpServerTrust::Unknown,
        transport_type_hint: None,
        source: None,
    };
    app_server
        .upsert_mcp_server(config)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    // `upsert_server` auto-promotes a programmatic add to `Trusted`. That would
    // let a freshly added server run tools with no confirmation, bypassing the
    // Unknown-until-trusted model, so reset it to `Unknown`: the user must
    // explicitly `/mcp trust <id>` before its tools become invokable.
    app_server
        .set_mcp_server_trust(id, McpServerTrust::Unknown)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let status = format!("Added MCP server `{id}` (untrusted; run `/mcp trust {id}` to enable).");
    Ok(LocalOutputCommandResult::new(status, None))
}

async fn run_mcp_remove_slash_command(
    rest: &str,
    app_server: &AppClient,
) -> Result<LocalOutputCommandResult> {
    let server_id = rest.trim();
    if server_id.is_empty() {
        return Err(anyhow::anyhow!("usage: /mcp remove <server>"));
    }
    let result = app_server
        .remove_mcp_server(server_id)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let removed = result.removed;
    let status = if removed {
        format!("Removed MCP server `{server_id}`.")
    } else {
        format!("No MCP server `{server_id}` to remove.")
    };
    Ok(LocalOutputCommandResult::new(status, None))
}

async fn run_mcp_trust_slash_command(
    rest: &str,
    app_server: &AppClient,
    trust: McpServerTrust,
    verb: &str,
) -> Result<LocalOutputCommandResult> {
    let server_id = rest.trim();
    if server_id.is_empty() {
        return Err(anyhow::anyhow!("usage: /mcp trust <server>"));
    }
    app_server
        .set_mcp_server_trust(server_id, trust)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let status = format!("{verb} MCP server `{server_id}`.");
    Ok(LocalOutputCommandResult::new(status, None))
}

fn parse_mcp_transport(token: &str) -> Result<McpTransport> {
    match token.to_ascii_lowercase().as_str() {
        "stdio" => Ok(McpTransport::Stdio),
        "streamable_http" => Ok(McpTransport::StreamableHttp),
        "http" => Ok(McpTransport::StreamableHttp),
        "https" => Ok(McpTransport::StreamableHttp),
        "ws" | "wss" | "websocket" => Ok(McpTransport::WebSocket),
        other => Err(anyhow::anyhow!(
            "unknown MCP transport `{other}`; expected stdio|streamable_http|http|https|websocket"
        )),
    }
}

async fn run_mcp_auth_slash_command(
    rest: &str,
    app_server: &AppClient,
) -> Result<LocalOutputCommandResult> {
    let (subcommand, args) = match split_first_word(rest) {
        Some((sub, args)) => (sub, args),
        None => ("status", ""),
    };
    match subcommand {
        "status" => {
            let server_id = {
                let trimmed = args.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            };
            let overview = app_server
                .mcp_oauth_overview(server_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let rendered = render_mcp_oauth_overview(&overview);
            Ok(LocalOutputCommandResult::new(
                "MCP OAuth token status.",
                nonempty_detail(rendered),
            ))
        }
        "login" => {
            let server_id = args.trim();
            if server_id.is_empty() {
                return Err(anyhow::anyhow!("usage: /mcp auth login <server>"));
            }
            let overview = app_server
                .mcp_oauth_overview(Some(server_id))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if overview.entries.is_empty() {
                return Ok(LocalOutputCommandResult::new(
                    format!("No MCP server `{server_id}` found."),
                    Some(
                        "Use `orbcode mcp auth browser-login` or `orbcode mcp auth device-login` from the CLI for interactive OAuth flows."
                            .to_string(),
                    ),
                ));
            }
            let entry = &overview.entries[0];
            if entry.usable {
                return Ok(LocalOutputCommandResult::new(
                    format!("MCP server `{server_id}` already has a valid token."),
                    Some(render_mcp_oauth_entry(entry)),
                ));
            }
            Ok(LocalOutputCommandResult::new(
                format!("MCP server `{server_id}` requires authentication."),
                Some(
                    "Run `orbcode mcp auth browser-login` or `orbcode mcp auth device-login` from the CLI to complete the OAuth flow interactively."
                        .to_string(),
                ),
            ))
        }
        "logout" => {
            let server_id = args.trim();
            if server_id.is_empty() {
                return Err(anyhow::anyhow!("usage: /mcp auth logout <server>"));
            }
            let result = app_server
                .logout_mcp_oauth_token(server_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let status = if result.logged_out {
                format!("Logged out MCP server `{server_id}`.")
            } else {
                format!("No OAuth token for MCP server `{server_id}`.")
            };
            Ok(LocalOutputCommandResult::new(status, None))
        }
        _ => Err(anyhow::anyhow!(
            "usage: /mcp auth [status|login|logout] [server]"
        )),
    }
}

pub(crate) fn render_mcp_oauth_overview(overview: &McpOAuthOverview) -> String {
    if overview.entries.is_empty() {
        return "No MCP OAuth tokens stored.".to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("Token store: {}", overview.store_path.display()));
    lines.push(String::new());
    for entry in &overview.entries {
        lines.push(render_mcp_oauth_entry(entry));
    }
    lines.join("\n")
}

pub(crate) fn render_mcp_oauth_entry(entry: &McpOAuthStatusEntry) -> String {
    let status_indicator = if entry.usable {
        "ready"
    } else if entry.expired {
        "expired"
    } else {
        "blocked"
    };
    let refresh = if entry.has_refresh_token {
        " refresh=yes"
    } else {
        " refresh=no"
    };
    let scopes = if entry.scopes.is_empty() {
        String::new()
    } else {
        format!(" scopes={}", entry.scopes.join(","))
    };
    let expiry = entry
        .expires_at
        .map(|ts| format!(" expires_at={ts}"))
        .unwrap_or_default();
    format!(
        "{} status={}{}{refresh}{scopes}",
        entry.server_id, status_indicator, expiry
    )
}
