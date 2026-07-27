use crate::error::McpError;
use crate::oauth::diagnose_oauth_discovery;
use crate::registry::{McpRegistry, canonicalize_auth_server};
use crate::transport::spawn_stdio_client;
use crate::transport::{http_client, websocket_client};
use crate::types::{McpDiagnosticCheck, McpDiagnosticStatus, McpServerConfig, McpTransport};

impl McpRegistry {
    pub async fn diagnose_server(
        &self,
        server_id: &str,
    ) -> Result<Vec<McpDiagnosticCheck>, McpError> {
        let server = self.server_snapshot(server_id).await?;
        let mut checks = Vec::new();
        let config = server.config;
        checks.push(mcp_check(
            "config",
            if config.enabled {
                McpDiagnosticStatus::Pass
            } else {
                McpDiagnosticStatus::Warn
            },
            if config.enabled {
                format!(
                    "transport={} endpoint={} trust={}",
                    config.transport, config.endpoint, config.trust
                )
            } else {
                format!("server `{}` is disabled", config.id)
            },
        ));
        checks.push(mcp_check(
            "trust",
            if config.trust.is_trusted() {
                McpDiagnosticStatus::Pass
            } else {
                McpDiagnosticStatus::Warn
            },
            if config.trust.is_trusted() {
                "server is trusted".to_string()
            } else {
                format!(
                    "server trust is {}; runtime calls remain blocked until trusted",
                    config.trust
                )
            },
        ));
        if !config.enabled {
            return Ok(checks);
        }

        let oauth_access_token = if matches!(
            config.transport,
            McpTransport::Http
                | McpTransport::Https
                | McpTransport::StreamableHttp
                | McpTransport::WebSocket
        ) {
            self.mcp_oauth_access_token(&config.id).await?
        } else {
            None
        };
        let probe = match config.transport {
            McpTransport::Stdio => diagnose_stdio_server(&config).await,
            McpTransport::Http | McpTransport::Https | McpTransport::StreamableHttp => {
                diagnose_http_server(&config, oauth_access_token.as_deref()).await
            }
            McpTransport::WebSocket => {
                diagnose_websocket_server(&config, oauth_access_token.as_deref()).await
            }
        };
        checks.push(match probe {
            Ok(detail) => mcp_check("probe", McpDiagnosticStatus::Pass, detail),
            Err(error) => mcp_check(
                "probe",
                McpDiagnosticStatus::Fail,
                canonicalize_auth_server(error, &config.id).to_string(),
            ),
        });

        if matches!(
            config.transport,
            McpTransport::Http
                | McpTransport::Https
                | McpTransport::StreamableHttp
                | McpTransport::WebSocket
        ) {
            let oauth_check =
                diagnose_oauth_discovery(&config, oauth_access_token.as_deref()).await;
            checks.push(oauth_check);
        }
        Ok(checks)
    }
}

async fn diagnose_stdio_server(config: &McpServerConfig) -> Result<String, McpError> {
    let mut client = spawn_stdio_client(config).await?;
    let result = client.list_tools().await;
    let _ = client.shutdown().await;
    let tools = result?.tools;
    Ok(format!(
        "initialize and tools/list succeeded; {} tool(s) discovered",
        tools.len()
    ))
}

async fn diagnose_http_server(
    config: &McpServerConfig,
    oauth_access_token: Option<&str>,
) -> Result<String, McpError> {
    let mut client = http_client(config, oauth_access_token)?;
    client.initialize().await?;
    let tools = client.list_tools().await?.tools;
    let _ = client.shutdown().await;
    Ok(format!(
        "initialize and tools/list succeeded over {}; {} tool(s) discovered",
        config.transport,
        tools.len()
    ))
}

async fn diagnose_websocket_server(
    config: &McpServerConfig,
    oauth_access_token: Option<&str>,
) -> Result<String, McpError> {
    let mut client = websocket_client(config, oauth_access_token).await?;
    client.initialize().await?;
    let tools = client.list_tools().await?.tools;
    Ok(format!(
        "initialize and tools/list succeeded over {}; {} tool(s) discovered",
        config.transport,
        tools.len()
    ))
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
