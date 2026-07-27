use std::time::Duration;

use orbcode_config::AppConfig;
use orbcode_mcp::{McpDiagnosticCheck, McpDiagnosticStatus, McpRegistry};

use super::{DoctorCheck, DoctorStatus, is_truthy};

const MCP_REACHABILITY_CHECK: &str = "mcp_reachability";

/// Per-server probe budget. `diagnose_server` already bounds its own network
/// I/O, but a wedged endpoint (accepts the socket, never replies) could still
/// stall the whole report, so each probe is additionally capped here.
const MCP_PROBE_TIMEOUT_SECS: u64 = 8;

fn mcp_probe_enabled(config: &AppConfig) -> bool {
    config
        .resolve_env("ORBCODE_DOCTOR_MCP_PROBE")
        .is_some_and(|value| is_truthy(&value))
}

/// Broadens the per-server `mcp diagnose <id>` reachability check into a single
/// global doctor check. For every enabled, trusted server it read-only reuses
/// `McpRegistry::diagnose_server` (no new mcp API, runtime call permissions
/// untouched) and reports Pass when all reachable, Warn (with server id +
/// reason) for any that fail to connect / handshake / authenticate.
///
/// The probe is opt-in (`ORBCODE_DOCTOR_MCP_PROBE`) so a no-network environment
/// never pays for it, and unreachable servers are a Warn — never a Fail — to
/// match `outcome_summary_classifies_network_unreachable_as_warn`: a configured
/// endpoint being down is an environmental condition, not a broken install.
pub(super) async fn mcp_reachability_check(
    config: &AppConfig,
    registry: &McpRegistry,
) -> DoctorCheck {
    if !mcp_probe_enabled(config) {
        return DoctorCheck {
            name: MCP_REACHABILITY_CHECK.to_string(),
            status: DoctorStatus::Warn,
            detail:
                "skipped; set ORBCODE_DOCTOR_MCP_PROBE=1 to probe configured MCP servers for reachability"
                    .to_string(),
        };
    }

    let targets: Vec<_> = registry
        .list_servers()
        .await
        .into_iter()
        .filter(|server| server.enabled && server.trust.is_trusted())
        .collect();
    if targets.is_empty() {
        return DoctorCheck {
            name: MCP_REACHABILITY_CHECK.to_string(),
            status: DoctorStatus::Pass,
            detail: "no trusted, enabled MCP servers to probe".to_string(),
        };
    }

    let mut unreachable: Vec<String> = Vec::new();
    let mut reachable = 0usize;
    for server in &targets {
        match probe_mcp_server(registry, &server.id).await {
            Ok(()) => reachable += 1,
            Err(reason) => unreachable.push(format!("{}: {reason}", server.id)),
        }
    }

    if unreachable.is_empty() {
        DoctorCheck {
            name: MCP_REACHABILITY_CHECK.to_string(),
            status: DoctorStatus::Pass,
            detail: format!("{reachable} configured MCP server(s) reachable"),
        }
    } else {
        DoctorCheck {
            name: MCP_REACHABILITY_CHECK.to_string(),
            status: DoctorStatus::Warn,
            detail: format!(
                "{} of {} configured MCP server(s) unreachable: {}",
                unreachable.len(),
                targets.len(),
                unreachable.join("; ")
            ),
        }
    }
}

/// Runs `diagnose_server` under a hard timeout. Returns `Ok` when the server is
/// reachable and healthy, otherwise an explanatory reason. Probe errors and
/// timeouts are mapped to reasons too so the caller treats them uniformly.
async fn probe_mcp_server(registry: &McpRegistry, server_id: &str) -> Result<(), String> {
    match tokio::time::timeout(
        Duration::from_secs(MCP_PROBE_TIMEOUT_SECS),
        registry.diagnose_server(server_id),
    )
    .await
    {
        Err(_elapsed) => Err(format!("probe timed out after {MCP_PROBE_TIMEOUT_SECS}s")),
        Ok(Err(error)) => Err(error.to_string()),
        Ok(Ok(checks)) => classify_server_diagnostics(&checks),
    }
}

/// A server is reachable and healthy when none of its diagnostic checks failed.
/// Any Fail check (unreachable endpoint, handshake error, missing bearer) yields
/// a joined reason string naming each failing sub-check.
fn classify_server_diagnostics(checks: &[McpDiagnosticCheck]) -> Result<(), String> {
    let reasons: Vec<String> = checks
        .iter()
        .filter(|check| check.status == McpDiagnosticStatus::Fail)
        .map(|check| format!("{}: {}", check.name, check.detail))
        .collect();
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(reasons.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::tests::config;
    use orbcode_mcp::{McpAuth, McpServerConfig, McpServerStatus, McpServerTrust, McpTransport};

    fn diag(name: &str, status: McpDiagnosticStatus, detail: &str) -> McpDiagnosticCheck {
        McpDiagnosticCheck {
            name: name.to_string(),
            status,
            detail: detail.to_string(),
        }
    }

    #[test]
    fn classify_server_diagnostics_passes_when_no_failures() {
        let checks = vec![
            diag("config", McpDiagnosticStatus::Pass, "ok"),
            diag("trust", McpDiagnosticStatus::Pass, "trusted"),
            diag("probe", McpDiagnosticStatus::Pass, "handshake ok"),
        ];
        assert!(classify_server_diagnostics(&checks).is_ok());
    }

    #[test]
    fn classify_server_diagnostics_reports_each_failure_with_reason() {
        let checks = vec![
            diag("config", McpDiagnosticStatus::Pass, "ok"),
            diag("trust", McpDiagnosticStatus::Pass, "trusted"),
            diag("probe", McpDiagnosticStatus::Fail, "connection refused"),
        ];
        let reason = classify_server_diagnostics(&checks).expect_err("should be unreachable");
        assert!(reason.contains("probe"), "{reason}");
        assert!(reason.contains("connection refused"), "{reason}");
    }

    #[tokio::test]
    async fn mcp_reachability_is_skipped_when_probe_toggle_is_off() {
        let temp = tempfile::tempdir().expect("tempdir");
        let registry = McpRegistry::load(temp.path().join("home"), temp.path().join("cwd"))
            .await
            .expect("load registry");
        let check = mcp_reachability_check(&config(), &registry).await;
        assert_eq!(check.name, "mcp_reachability");
        assert_eq!(check.status, DoctorStatus::Warn);
        assert!(check.detail.contains("ORBCODE_DOCTOR_MCP_PROBE"));
        assert_ne!(check.status, DoctorStatus::Fail);
    }

    #[tokio::test]
    async fn mcp_reachability_passes_when_no_trusted_servers_to_probe() {
        let temp = tempfile::tempdir().expect("tempdir");
        let registry = McpRegistry::load(temp.path().join("home"), temp.path().join("cwd"))
            .await
            .expect("load registry");
        let mut config = config();
        config
            .env_overrides
            .insert("ORBCODE_DOCTOR_MCP_PROBE".to_string(), "1".to_string());

        let check = mcp_reachability_check(&config, &registry).await;
        assert_eq!(check.status, DoctorStatus::Pass);
        assert!(check.detail.contains("no trusted"));
    }

    #[tokio::test]
    async fn mcp_reachability_warns_with_server_id_and_reason_for_unreachable_endpoint() {
        // Bind then immediately drop to claim a definitely-closed local port so
        // the probe fails fast with connection-refused instead of timing out.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind temp port");
        let endpoint = format!("http://{}/mcp", listener.local_addr().expect("local addr"));
        drop(listener);

        let temp = tempfile::tempdir().expect("tempdir");
        let registry = McpRegistry::load(temp.path().join("home"), temp.path().join("cwd"))
            .await
            .expect("load registry");
        registry
            .upsert_server(McpServerConfig {
                id: "dead-remote".to_string(),
                transport: McpTransport::Http,
                endpoint,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                cwd: None,
                headers: std::collections::BTreeMap::new(),
                enabled: true,
                status: McpServerStatus::Ready,
                error: None,
                summary: "Dead remote".to_string(),
                auth: McpAuth::None,
                trust: McpServerTrust::Trusted,
                transport_type_hint: None,
                source: None,
            })
            .await
            .expect("upsert remote");

        let mut config = config();
        config
            .env_overrides
            .insert("ORBCODE_DOCTOR_MCP_PROBE".to_string(), "1".to_string());

        let check = mcp_reachability_check(&config, &registry).await;
        assert_eq!(check.name, "mcp_reachability");
        assert_eq!(
            check.status,
            DoctorStatus::Warn,
            "unreachable endpoint must be Warn, never Fail: {}",
            check.detail
        );
        assert!(
            check.detail.contains("dead-remote"),
            "detail should name the server: {}",
            check.detail
        );
        assert!(
            check.detail.contains("unreachable"),
            "detail should explain the reason: {}",
            check.detail
        );
    }
}
