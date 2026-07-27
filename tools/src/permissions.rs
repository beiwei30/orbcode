use orbcode_mcp::McpError;
use serde_json::json;

use crate::{ToolContext, ToolError};

/// Convert an [`McpError`] into a [`ToolError`]. Auth and trust failures are the
/// ones a user can actually fix, so they carry structured `mcp` metadata (server,
/// kind, reason/status, and the exact CLI command to resolve it) rather than being
/// flattened into opaque model text. Other transport/protocol failures keep the
/// lightweight `ToolError::Mcp` string form.
pub(crate) fn map_mcp_error(error: McpError) -> ToolError {
    let message = error.to_string();
    match error {
        McpError::AuthRequired { server, reason } => ToolError::ExecutionFailedWithMetadata {
            message,
            metadata: json!({
                "mcp": {
                    "server": server,
                    "errorKind": "auth_required",
                    "reason": reason,
                    "fix": format!("orbcode mcp auth login {server}"),
                }
            }),
        },
        McpError::ServerUntrusted { server, status } => ToolError::ExecutionFailedWithMetadata {
            message,
            metadata: json!({
                "mcp": {
                    "server": server,
                    "errorKind": "server_untrusted",
                    "status": status,
                    "fix": format!("orbcode mcp trust {server}"),
                }
            }),
        },
        _ => ToolError::Mcp(message),
    }
}

pub(crate) fn require_tools(context: &ToolContext) -> Result<(), ToolError> {
    ensure_not_cancelled(context)?;
    if context.allow_tools {
        Ok(())
    } else {
        Err(ToolError::PermissionDenied)
    }
}

pub(crate) fn require_network(context: &ToolContext) -> Result<(), ToolError> {
    ensure_not_cancelled(context)?;
    if context.allow_network {
        Ok(())
    } else {
        Err(ToolError::NetworkDenied)
    }
}

pub(crate) fn ensure_not_cancelled(context: &ToolContext) -> Result<(), ToolError> {
    if context.cancellation.is_cancelled() {
        Err(ToolError::Interrupted)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn map_mcp_error_surfaces_auth_required_as_structured_metadata() {
        let error = map_mcp_error(McpError::AuthRequired {
            server: "docs".to_string(),
            reason: "remote server returned HTTP 401".to_string(),
        });
        let metadata = error
            .metadata()
            .expect("auth-required errors must carry structured metadata");
        let mcp = &metadata["mcp"];
        assert_eq!(mcp["server"], Value::from("docs"));
        assert_eq!(mcp["errorKind"], Value::from("auth_required"));
        assert_eq!(
            mcp["reason"],
            Value::from("remote server returned HTTP 401")
        );
        assert_eq!(mcp["fix"], Value::from("orbcode mcp auth login docs"));

        // The display message stays actionable (server + sign-in guidance), so even a
        // consumer that ignores metadata never sees opaque text.
        let rendered = error.to_string();
        assert!(rendered.contains("docs"), "{rendered}");
        assert!(
            rendered.contains("orbcode mcp auth login docs"),
            "{rendered}"
        );
    }

    #[test]
    fn map_mcp_error_surfaces_server_untrusted_status() {
        let error = map_mcp_error(McpError::ServerUntrusted {
            server: "docs".to_string(),
            status: "unknown",
        });
        let metadata = error
            .metadata()
            .expect("untrusted errors must carry structured metadata");
        let mcp = &metadata["mcp"];
        assert_eq!(mcp["server"], Value::from("docs"));
        assert_eq!(mcp["errorKind"], Value::from("server_untrusted"));
        assert_eq!(mcp["status"], Value::from("unknown"));
        assert_eq!(mcp["fix"], Value::from("orbcode mcp trust docs"));
    }

    #[test]
    fn map_mcp_error_keeps_other_failures_as_plain_text() {
        let error = map_mcp_error(McpError::Protocol("boom".to_string()));
        assert!(matches!(error, ToolError::Mcp(_)));
        assert!(error.metadata().is_none());
    }
}
