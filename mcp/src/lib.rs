pub mod cancel;
mod config_load;
pub mod error;
mod oauth;
mod registry;
mod store;
pub mod transport;
pub mod types;
pub mod wire;

pub use cancel::McpCancellationToken;
pub use config_load::scoped_plugin_server_id;
pub use error::McpError;
pub use registry::McpRegistry;
pub use transport::stdio::StdioMcpClient;
pub use types::{
    ErasedTrustApprovalHandler, McpAuth, McpCapability, McpConfigReloadResult, McpContent,
    McpDiagnosticCheck, McpDiagnosticStatus, McpLoadOptions, McpOAuthBrowserLoginInput,
    McpOAuthBrowserLoginSession, McpOAuthDeviceLoginInput, McpOAuthDeviceLoginSession,
    McpOAuthOverview, McpOAuthStatusEntry, McpOAuthTokenInput, McpPluginConfigSource,
    McpPluginConfigSourceKind, McpPluginSource, McpPrompt, McpPromptArgument, McpPromptMessage,
    McpPromptResult, McpResourceContent, McpResourceSummary, McpResourceTemplate, McpServerConfig,
    McpServerSource, McpServerStatus, McpServerTrust, McpToolDescriptor, McpToolResult,
    McpToolSpec, McpTransport, SharedTrustApprovalHandler, TrustApprovalHandler,
    TrustApprovalRequest, TrustApprovalResponse,
};
pub use wire::{
    StdioContentBlock, StdioInitializeResult, StdioListToolsResult, StdioServerInfo,
    StdioToolCallResult, StdioToolSpec,
};

#[cfg(test)]
mod tests;
