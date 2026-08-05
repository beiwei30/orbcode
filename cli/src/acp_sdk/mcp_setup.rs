use std::collections::HashSet;

use agent_client_protocol::Result;
use agent_client_protocol::schema::McpServer;
use orbcode_app_server_protocol::{
    McpAuth, McpServerInput, McpServerStatus, McpServerTrust, McpTransport,
};

use super::invalid_params;

pub(super) fn acp_mcp_servers_to_configs(
    method_name: &str,
    servers: &[McpServer],
) -> Result<Vec<McpServerInput>> {
    let mut configs = Vec::with_capacity(servers.len());
    let mut used_names = HashSet::new();
    for server in servers {
        match server {
            McpServer::Stdio(stdio) => {
                let id = unique_acp_server_name(&stdio.name, &mut used_names);
                configs.push(McpServerInput {
                    id,
                    transport: McpTransport::Stdio,
                    endpoint: stdio.command.display().to_string(),
                    args: stdio.args.clone(),
                    env: stdio
                        .env
                        .iter()
                        .map(|entry| (entry.name.clone(), entry.value.clone()))
                        .collect(),
                    cwd: None,
                    headers: Default::default(),
                    enabled: true,
                    status: McpServerStatus::Ready,
                    error: None,
                    summary: stdio.name.clone(),
                    auth: McpAuth::None,
                    trust: McpServerTrust::Unknown,
                    transport_type_hint: None,
                    source: None,
                });
            }
            McpServer::Http(http) => {
                let id = unique_acp_server_name(&http.name, &mut used_names);
                if !http.url.starts_with("https://") && !http.url.starts_with("http://") {
                    return Err(invalid_params(format!(
                        "{method_name} mcpServers HTTP URL must start with http:// or https://: {}",
                        http.url
                    )));
                }
                configs.push(McpServerInput {
                    id,
                    transport: McpTransport::StreamableHttp,
                    endpoint: http.url.clone(),
                    args: Vec::new(),
                    env: Default::default(),
                    cwd: None,
                    headers: http
                        .headers
                        .iter()
                        .map(|entry| (entry.name.clone(), entry.value.clone()))
                        .collect(),
                    enabled: true,
                    status: McpServerStatus::Ready,
                    error: None,
                    summary: http.name.clone(),
                    auth: McpAuth::None,
                    trust: McpServerTrust::Unknown,
                    transport_type_hint: None,
                    source: None,
                });
            }
            McpServer::Sse(sse) => {
                return Err(invalid_params(format!(
                    "{method_name} mcpServers SSE transport is not supported: {}",
                    sse.name
                )));
            }
            _ => {
                return Err(invalid_params(format!(
                    "{method_name} mcpServers transport is not supported"
                )));
            }
        }
    }
    Ok(configs)
}

pub(super) fn unique_acp_server_name(name: &str, used_names: &mut HashSet<String>) -> String {
    let base = sanitize_acp_server_name(name);
    if used_names.insert(base.clone()) {
        return base;
    }
    for index in 2.. {
        let candidate = format!("{base}-{index}");
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("unbounded integer suffix should find a unique ACP MCP server name")
}

fn sanitize_acp_server_name(name: &str) -> String {
    let mut sanitized = String::new();
    let mut previous_dash = false;
    for ch in name.trim().chars() {
        let replacement = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            Some(ch.to_ascii_lowercase())
        } else if ch.is_whitespace() || ch == '.' || ch == '/' || ch == ':' {
            Some('-')
        } else {
            None
        };
        let Some(ch) = replacement else {
            continue;
        };
        if ch == '-' {
            if previous_dash {
                continue;
            }
            previous_dash = true;
        } else {
            previous_dash = false;
        }
        sanitized.push(ch);
    }
    let sanitized = sanitized.trim_matches('-').to_string();
    if sanitized.is_empty() {
        "mcp".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::McpServer;

    #[test]
    fn acp_mcp_servers_convert_stdio_and_http() {
        let stdio =
            agent_client_protocol::schema::McpServerStdio::new("Docs Server", "/usr/bin/docs-mcp")
                .args(vec!["--stdio".to_string()])
                .env(vec![agent_client_protocol::schema::EnvVariable::new(
                    "DOCS_TOKEN",
                    "secret",
                )]);
        let http = agent_client_protocol::schema::McpServerHttp::new(
            "Docs Server",
            "https://docs.test/mcp",
        )
        .headers(vec![agent_client_protocol::schema::HttpHeader::new(
            "Authorization",
            "Bearer test",
        )]);

        let configs = acp_mcp_servers_to_configs(
            "session/new",
            &[McpServer::Stdio(stdio), McpServer::Http(http)],
        )
        .expect("convert");

        assert_eq!(configs[0].id, "docs-server");
        assert_eq!(configs[0].transport, McpTransport::Stdio);
        assert_eq!(configs[0].endpoint, "/usr/bin/docs-mcp");
        assert_eq!(configs[0].args, vec!["--stdio"]);
        assert_eq!(configs[0].env["DOCS_TOKEN"], "secret");
        assert_eq!(configs[0].trust, McpServerTrust::Unknown);
        assert_eq!(configs[1].id, "docs-server-2");
        assert_eq!(configs[1].transport, McpTransport::StreamableHttp);
        assert_eq!(configs[1].headers["Authorization"], "Bearer test");
    }

    #[test]
    fn acp_mcp_servers_reject_sse() {
        let sse =
            agent_client_protocol::schema::McpServerSse::new("events", "https://docs.test/events");
        let err = acp_mcp_servers_to_configs("session/new", &[McpServer::Sse(sse)])
            .expect_err("SSE is unsupported");

        let rendered = serde_json::to_string(&err).expect("serialize error");
        assert!(rendered.contains("SSE transport is not supported"));
    }

    #[test]
    fn sanitize_acp_server_name_dedupes_empty_and_punctuation() {
        let mut used = HashSet::new();
        assert_eq!(
            unique_acp_server_name(" ../Docs Server!! ", &mut used),
            "docs-server"
        );
        assert_eq!(
            unique_acp_server_name("docs server", &mut used),
            "docs-server-2"
        );
        assert_eq!(unique_acp_server_name("💥", &mut used), "mcp");
    }
}
