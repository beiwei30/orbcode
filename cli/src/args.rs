use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use orbcode_app_server::{AppConfigOverrides, PermissionMode, ProviderId, parse_tool_rule_list};
use orbcode_app_server_client::{AuthMethod, McpTransport};
use orbcode_protocol::SandboxMode;

use crate::build_info;

#[derive(Debug, Parser)]
#[command(
    name = "orbcode",
    about = "Terminal coding assistant",
    version = build_info::VERSION_LINE,
    long_version = build_info::LONG_VERSION_LINE,
)]
pub(crate) struct Cli {
    #[arg(long, value_enum, global = true)]
    pub provider: Option<CliProvider>,
    #[arg(long = "fallback-provider", value_enum, global = true)]
    pub fallback_provider: Option<CliProvider>,
    #[arg(long, global = true)]
    pub max_retries: Option<usize>,
    #[arg(long = "sandbox-mode", value_enum, global = true)]
    pub sandbox_mode: Option<CliSandboxMode>,
    #[arg(long = "sandbox-network", global = true)]
    pub sandbox_allow_network: Option<bool>,
    #[arg(long, global = true)]
    pub allow_network: Option<bool>,
    #[arg(long, global = true)]
    pub allow_tools: Option<bool>,
    #[arg(
        long = "allowed-tools",
        alias = "allowedTools",
        value_name = "TOOLS",
        global = true
    )]
    pub allowed_tools: Vec<String>,
    #[arg(
        long = "disallowed-tools",
        alias = "disallowedTools",
        value_name = "TOOLS",
        global = true
    )]
    pub disallowed_tools: Vec<String>,
    #[arg(long = "add-dir", value_name = "DIR", global = true)]
    pub add_dirs: Vec<PathBuf>,
    #[arg(long = "mcp-config", value_name = "CONFIG", global = true)]
    pub mcp_config: Vec<String>,
    #[arg(short = 'c', long = "continue", global = true)]
    pub continue_latest: bool,
    /// Resume a session by id; without a value, resumes the latest available.
    /// Matches the TypeScript CLI: `--resume <id>` and `--resume=<id>` both
    /// work. Be aware that bare `-r` followed by a non-flag token will treat
    /// that token as a session id (e.g. `-r tui` looks up session "tui").
    #[arg(
        short = 'r',
        long = "resume",
        value_name = "SESSION_ID",
        num_args = 0..=1,
        default_missing_value = "",
        global = true
    )]
    pub resume: Option<String>,
    /// Use a specific session id for this invocation (creates it if missing).
    #[arg(long = "session-id", value_name = "SESSION_ID", global = true)]
    pub session_id: Option<String>,
    /// Run in non-interactive print mode (TypeScript-compatible `-p/--print`).
    #[arg(short = 'p', long = "print", global = true, default_value_t = false)]
    pub print: bool,
    /// Output format for headless / print mode.
    #[arg(
        long = "output-format",
        value_enum,
        value_name = "FORMAT",
        global = true
    )]
    pub output_format: Option<CliOutputFormat>,
    /// Input format for headless / print mode (text or NDJSON stream).
    #[arg(
        long = "input-format",
        value_enum,
        value_name = "FORMAT",
        global = true
    )]
    pub input_format: Option<CliInputFormat>,
    /// Inline settings JSON or path to a settings JSON file.
    #[arg(long = "settings", value_name = "FILE_OR_JSON", global = true)]
    pub settings: Option<String>,
    /// Extra text appended to the system prompt for this invocation.
    #[arg(long = "append-system-prompt", value_name = "TEXT", global = true)]
    pub append_system_prompt: Option<String>,
    /// Permission preset (TypeScript-compatible).
    #[arg(
        long = "permission-mode",
        value_enum,
        value_name = "MODE",
        global = true
    )]
    pub permission_mode: Option<CliPermissionMode>,
    /// Enable verbose progress output (required for `--output-format stream-json` in print mode).
    #[arg(long = "verbose", global = true, default_value_t = false)]
    pub verbose: bool,
    /// Positional prompt for `-p/--print` mode.
    #[arg(value_name = "PROMPT", global = true)]
    pub prompt: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliOutputFormat {
    Text,
    Json,
    #[value(name = "stream-json")]
    StreamJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliInputFormat {
    Text,
    #[value(name = "stream-json")]
    StreamJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliPermissionMode {
    Default,
    #[value(name = "acceptEdits", alias = "accept-edits")]
    AcceptEdits,
    #[value(name = "bypassPermissions", alias = "bypass-permissions")]
    BypassPermissions,
    #[value(name = "dontAsk", alias = "dont-ask")]
    DontAsk,
    Plan,
    Auto,
}

impl From<CliPermissionMode> for PermissionMode {
    fn from(value: CliPermissionMode) -> Self {
        match value {
            CliPermissionMode::Default => PermissionMode::Default,
            CliPermissionMode::AcceptEdits => PermissionMode::AcceptEdits,
            CliPermissionMode::BypassPermissions => PermissionMode::BypassPermissions,
            CliPermissionMode::DontAsk => PermissionMode::DontAsk,
            CliPermissionMode::Plan => PermissionMode::Plan,
            CliPermissionMode::Auto => PermissionMode::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum CliProvider {
    Anthropic,
    #[value(name = "openai")]
    OpenAi,
    Gemini,
    Grok,
}

impl CliProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
        }
    }
}

impl From<CliProvider> for ProviderId {
    fn from(value: CliProvider) -> Self {
        match value {
            CliProvider::Anthropic => ProviderId::Anthropic,
            CliProvider::OpenAi => ProviderId::OpenAi,
            CliProvider::Gemini => ProviderId::Gemini,
            CliProvider::Grok => ProviderId::Grok,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum CliSandboxMode {
    #[value(name = "danger-full-access")]
    DangerFullAccess,
    #[value(name = "workspace-write")]
    WorkspaceWrite,
    #[value(name = "read-only")]
    ReadOnly,
}

impl CliSandboxMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DangerFullAccess => "danger-full-access",
            Self::WorkspaceWrite => "workspace-write",
            Self::ReadOnly => "read-only",
        }
    }
}

impl From<CliSandboxMode> for SandboxMode {
    fn from(value: CliSandboxMode) -> Self {
        match value {
            CliSandboxMode::DangerFullAccess => SandboxMode::DangerFullAccess,
            CliSandboxMode::WorkspaceWrite => SandboxMode::WorkspaceWrite,
            CliSandboxMode::ReadOnly => SandboxMode::ReadOnly,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum CliMcpTransport {
    Stdio,
    Http,
    Https,
    StreamableHttp,
    Websocket,
}

impl From<CliMcpTransport> for McpTransport {
    fn from(value: CliMcpTransport) -> Self {
        match value {
            CliMcpTransport::Stdio => McpTransport::Stdio,
            CliMcpTransport::Http => McpTransport::StreamableHttp,
            CliMcpTransport::Https => McpTransport::StreamableHttp,
            CliMcpTransport::StreamableHttp => McpTransport::StreamableHttp,
            CliMcpTransport::Websocket => McpTransport::WebSocket,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum CliAuthMethod {
    ApiKey,
    OAuthDevice,
    #[value(name = "chatgpt", alias = "chat-gpt")]
    ChatGpt,
}

impl From<CliAuthMethod> for AuthMethod {
    fn from(value: CliAuthMethod) -> Self {
        match value {
            CliAuthMethod::ApiKey => AuthMethod::ApiKey,
            CliAuthMethod::OAuthDevice => AuthMethod::OAuthDevice,
            CliAuthMethod::ChatGpt => AuthMethod::ChatGpt,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct GlobalOptions {
    pub provider: Option<CliProvider>,
    pub fallback_provider: Option<CliProvider>,
    pub max_retries: Option<usize>,
    pub sandbox_mode: Option<CliSandboxMode>,
    pub sandbox_allow_network: Option<bool>,
    pub allow_network: Option<bool>,
    pub allow_tools: Option<bool>,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub add_dirs: Vec<PathBuf>,
    pub mcp_config: Vec<String>,
    pub permission_mode: Option<CliPermissionMode>,
    pub append_system_prompt: Option<String>,
    pub settings_source: Option<String>,
}

impl GlobalOptions {
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            provider: cli.provider,
            fallback_provider: cli.fallback_provider,
            max_retries: cli.max_retries,
            sandbox_mode: cli.sandbox_mode,
            sandbox_allow_network: cli.sandbox_allow_network,
            allow_network: cli.allow_network,
            allow_tools: cli.allow_tools,
            allowed_tools: parse_tool_rule_list(&cli.allowed_tools),
            disallowed_tools: parse_tool_rule_list(&cli.disallowed_tools),
            add_dirs: cli.add_dirs.clone(),
            mcp_config: cli.mcp_config.clone(),
            permission_mode: cli.permission_mode,
            append_system_prompt: cli.append_system_prompt.clone(),
            settings_source: cli.settings.clone(),
        }
    }

    pub fn as_overrides(&self) -> Result<AppConfigOverrides> {
        let settings_json = match self.settings_source.as_deref() {
            Some(value) => Some(load_inline_settings_source(value)?),
            None => None,
        };
        Ok(AppConfigOverrides {
            default_provider: self.provider.map(Into::into),
            fallback_provider: self.fallback_provider.map(Into::into),
            max_retries: self.max_retries,
            sandbox_mode: self.sandbox_mode.map(Into::into),
            sandbox_allow_network: self.sandbox_allow_network,
            allow_network: self.allow_network,
            allow_tools: self.allow_tools,
            allowed_tools: self.allowed_tools.clone(),
            disallowed_tools: self.disallowed_tools.clone(),
            add_dirs: self.add_dirs.clone(),
            mcp_config_inputs: self.mcp_config.clone(),
            append_system_prompt: self.append_system_prompt.clone(),
            permission_mode: self.permission_mode.map(Into::into),
            settings_json,
            ..AppConfigOverrides::default()
        })
    }

    pub fn as_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(provider) = self.provider {
            args.push("--provider".to_string());
            args.push(provider.as_str().to_string());
        }
        if let Some(provider) = self.fallback_provider {
            args.push("--fallback-provider".to_string());
            args.push(provider.as_str().to_string());
        }
        if let Some(max_retries) = self.max_retries {
            args.push("--max-retries".to_string());
            args.push(max_retries.to_string());
        }
        if let Some(sandbox_mode) = self.sandbox_mode {
            args.push("--sandbox-mode".to_string());
            args.push(sandbox_mode.as_str().to_string());
        }
        if let Some(sandbox_allow_network) = self.sandbox_allow_network {
            args.push("--sandbox-network".to_string());
            args.push(sandbox_allow_network.to_string());
        }
        if let Some(allow_network) = self.allow_network {
            args.push("--allow-network".to_string());
            args.push(allow_network.to_string());
        }
        if let Some(allow_tools) = self.allow_tools {
            args.push("--allow-tools".to_string());
            args.push(allow_tools.to_string());
        }
        for rule in &self.allowed_tools {
            args.push("--allowed-tools".to_string());
            args.push(rule.clone());
        }
        for rule in &self.disallowed_tools {
            args.push("--disallowed-tools".to_string());
            args.push(rule.clone());
        }
        for dir in &self.add_dirs {
            args.push("--add-dir".to_string());
            args.push(dir.display().to_string());
        }
        for config in &self.mcp_config {
            args.push("--mcp-config".to_string());
            args.push(config.clone());
        }
        if let Some(mode) = self.permission_mode {
            args.push("--permission-mode".to_string());
            args.push(mode.to_possible_value().unwrap().get_name().to_string());
        }
        if let Some(text) = &self.append_system_prompt {
            args.push("--append-system-prompt".to_string());
            args.push(text.clone());
        }
        if let Some(source) = &self.settings_source {
            args.push("--settings".to_string());
            args.push(source.clone());
        }
        args
    }
}

pub(crate) fn load_inline_settings_source(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("--settings value is empty");
    }
    if trimmed.starts_with('{') {
        serde_json::from_str::<serde_json::Value>(trimmed)
            .map_err(|error| anyhow::anyhow!("--settings inline JSON is invalid: {error}"))?;
        return Ok(trimmed.to_string());
    }
    let path = PathBuf::from(trimmed);
    let contents = std::fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("failed to read --settings file {trimmed}: {error}"))?;
    serde_json::from_str::<serde_json::Value>(&contents)
        .map_err(|error| anyhow::anyhow!("--settings file {trimmed} is not valid JSON: {error}"))?;
    Ok(contents)
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Run the interactive TUI (embedded mode — starts local core).
    Tui,
    /// Run the TUI in remote mode — connects to a running orbcode serve instance.
    /// Does not start a local core; all operations go through the protocol.
    Remote {
        /// Socket path or WebSocket address to connect to.
        #[arg(value_name = "ENDPOINT")]
        endpoint: String,
        /// Auth token for the connection.
        #[arg(long)]
        token: String,
    },
    /// Resume an existing persisted session in the TUI.
    Resume {
        #[arg(value_name = "SESSION_ID")]
        session_id: String,
    },
    /// Fork an existing session into a new persisted session.
    Fork {
        #[arg(value_name = "SESSION_ID")]
        session_id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, value_name = "PROMPT")]
        prompt: Option<String>,
        #[arg(long, default_value_t = false)]
        tui: bool,
    },
    /// Submit a single prompt in headless mode.
    Prompt {
        #[arg(value_name = "PROMPT")]
        prompt: String,
        #[arg(long, value_name = "SESSION_ID")]
        session: Option<String>,
        #[arg(long, default_value_t = false)]
        bg: bool,
    },
    /// List persisted sessions.
    Sessions {
        /// Emit one JSON object per line instead of the human-readable layout.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Rename a persisted session (overrides the auto-generated title).
    Rename {
        #[arg(value_name = "SESSION_ID")]
        session_id: String,
        #[arg(value_name = "NEW_TITLE")]
        title: String,
    },
    /// List persisted background prompt jobs.
    Ps,
    /// Print one background job log.
    Logs {
        #[arg(value_name = "JOB_ID")]
        job_id: String,
        #[arg(long, default_value_t = false)]
        follow: bool,
    },
    /// Follow one background job log until it finishes.
    Attach {
        #[arg(value_name = "JOB_ID")]
        job_id: String,
    },
    /// Cancel one background job.
    Kill {
        #[arg(value_name = "JOB_ID")]
        job_id: String,
    },
    /// Show the provider catalog and active provider chain.
    Providers,
    /// Render the current context snapshot.
    Context,
    /// Show the tool registry.
    Tools,
    /// Invoke a tool directly.
    Tool {
        #[arg(value_name = "TOOL_NAME")]
        name: String,
        #[arg(value_name = "INPUT")]
        input: Option<String>,
        #[arg(long, value_name = "SESSION_ID")]
        session: Option<String>,
    },
    /// Inspect or configure provider auth metadata.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Inspect or configure the MCP registry.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Run local health checks for the Rust shell.
    Doctor {
        #[command(subcommand)]
        command: Option<DoctorCommand>,
    },
    /// Show which advanced capability slices are implemented or deferred.
    Advanced,
    /// Run the ACP (Agent Client Protocol) adapter over stdio.
    Acp,
    #[command(hide = true)]
    BgWorker {
        #[arg(long)]
        job_id: String,
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        prompt: String,
    },
    /// Start a one-shot protocol transport server (experimental).
    ///
    /// Accepts a single client connection then exits. Stdio is implicitly
    /// trusted; socket and WebSocket require an auth token (auto-generated
    /// if not provided, printed in the startup JSON).
    #[command(hide = true)]
    Serve {
        /// Use stdin/stdout as the transport (implicitly trusted, no auth).
        #[arg(long, default_value_t = false)]
        stdio: bool,
        /// Listen on a Unix domain socket at the given path.
        #[arg(long, value_name = "PATH")]
        socket: Option<PathBuf>,
        /// Listen for WebSocket connections on the given address (e.g. 127.0.0.1:8080).
        #[arg(long, value_name = "ADDR")]
        websocket: Option<String>,
        /// Auth token for socket/WebSocket transports. If not provided,
        /// a random token is generated and printed in the startup JSON.
        #[arg(long, value_name = "TOKEN")]
        auth_token: Option<String>,
        /// Allowed Origin header values for WebSocket connections.
        /// Can be specified multiple times. When set, connections with
        /// a non-matching Origin are rejected with HTTP 403.
        #[arg(long = "allowed-origin", value_name = "ORIGIN")]
        allowed_origins: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum DoctorCommand {
    /// Preview or remove orphan child-session metadata whose parent transcript is gone.
    CleanupOrphans {
        /// Only preview what would be removed.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Actually remove eligible orphan child-session artifacts.
        #[arg(long, default_value_t = false)]
        yes: bool,
        /// Also treat Running orphan child sessions as eligible when their last activity is older than this many days.
        #[arg(long, value_name = "DAYS")]
        stale_running_days: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum AuthCommand {
    /// Show persisted and environment-backed auth modeling.
    Status,
    /// Persist auth metadata for one provider.
    Login {
        #[arg(long, value_enum)]
        provider: CliProvider,
        #[arg(long, value_enum, default_value_t = CliAuthMethod::ApiKey)]
        method: CliAuthMethod,
        #[arg(long)]
        token: Option<String>,
        #[arg(long = "env-var")]
        env_var: Option<String>,
        /// Use the headless ChatGPT device-code flow instead of a browser callback.
        #[arg(long, default_value_t = false)]
        device_code: bool,
    },
    /// Remove persisted auth metadata for one provider or for all providers.
    Logout {
        #[arg(long, value_enum)]
        provider: Option<CliProvider>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum McpCommand {
    Capabilities,
    Servers,
    /// Probe one MCP server configuration without using the runtime call permission gate.
    Diagnose {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
    },
    /// Inspect or manage stored OAuth tokens for MCP servers.
    Auth {
        #[command(subcommand)]
        command: CliMcpAuthCommand,
    },
    Add {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
        #[arg(long, value_enum)]
        transport: CliMcpTransport,
        #[arg(long, value_name = "ENDPOINT")]
        endpoint: String,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long, value_name = "AUTH_SPEC")]
        auth: Option<String>,
        #[arg(long, default_value_t = true)]
        enabled: bool,
    },
    Remove {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
    },
    Resources {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
    },
    Read {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
        #[arg(value_name = "URI")]
        uri: String,
    },
    /// List the prompts advertised by an MCP server.
    Prompts {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
    },
    /// Render an MCP prompt by name, with optional JSON arguments.
    Prompt {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
        #[arg(value_name = "PROMPT_NAME")]
        prompt_name: String,
        #[arg(value_name = "ARGUMENTS")]
        arguments: Option<String>,
    },
    Tools {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
    },
    Call {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
        #[arg(value_name = "TOOL_NAME")]
        tool_name: String,
        #[arg(value_name = "INPUT")]
        input: Option<String>,
        #[arg(long, value_name = "SESSION_ID")]
        session: Option<String>,
    },
    /// Mark an MCP server as trusted so the model and CLI can invoke it.
    Trust {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
    },
    /// Mark an MCP server as denied so all calls fail until reset.
    Distrust {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
    },
    /// Reset trust state so the next call re-prompts.
    Untrust {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum CliMcpAuthCommand {
    /// Show stored MCP OAuth tokens.
    Status {
        #[arg(value_name = "SERVER_ID")]
        server_id: Option<String>,
    },
    /// Store an OAuth access token for one MCP server.
    Login {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
        #[arg(long = "access-token")]
        access_token: String,
        #[arg(long = "refresh-token")]
        refresh_token: Option<String>,
        #[arg(long = "token-endpoint")]
        token_endpoint: Option<String>,
        #[arg(long = "client-id")]
        client_id: Option<String>,
        #[arg(long = "expires-at", value_name = "UNIX_SECONDS")]
        expires_at: Option<i64>,
        #[arg(long = "scope")]
        scopes: Vec<String>,
    },
    /// Complete an OAuth device-code login for one MCP server.
    DeviceLogin {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
        #[arg(long = "device-authorization-endpoint")]
        device_authorization_endpoint: Option<String>,
        #[arg(long = "token-endpoint")]
        token_endpoint: Option<String>,
        #[arg(long = "client-id")]
        client_id: String,
        #[arg(long = "scope")]
        scopes: Vec<String>,
    },
    /// Complete an OAuth browser login for one MCP server.
    BrowserLogin {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
        #[arg(long = "authorization-endpoint")]
        authorization_endpoint: Option<String>,
        #[arg(long = "token-endpoint")]
        token_endpoint: Option<String>,
        /// Pre-registered OAuth client id. Omit to attempt RFC 7591 dynamic
        /// client registration against the registration endpoint.
        #[arg(long = "client-id")]
        client_id: Option<String>,
        /// RFC 7591 dynamic client registration endpoint (defaults to the one
        /// advertised by OAuth discovery).
        #[arg(long = "registration-endpoint")]
        registration_endpoint: Option<String>,
        #[arg(long = "scope")]
        scopes: Vec<String>,
        #[arg(long = "redirect-port")]
        redirect_port: Option<u16>,
    },
    /// Remove a stored OAuth token for one MCP server.
    Logout {
        #[arg(value_name = "SERVER_ID")]
        server_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_print_flag_with_positional_prompt() {
        let parsed = Cli::try_parse_from(["orbcode", "-p", "hello world"]).expect("parse");
        assert!(parsed.print);
        assert_eq!(parsed.prompt.as_deref(), Some("hello world"));
        assert!(parsed.command.is_none());
    }

    #[test]
    fn cli_parses_output_and_input_format() {
        let parsed = Cli::try_parse_from([
            "orbcode",
            "--print",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--verbose",
            "ignored",
        ])
        .expect("parse");
        assert_eq!(parsed.output_format, Some(CliOutputFormat::StreamJson));
        assert_eq!(parsed.input_format, Some(CliInputFormat::StreamJson));
        assert!(parsed.verbose);
    }

    #[test]
    fn cli_parses_permission_mode_with_camel_alias() {
        let parsed =
            Cli::try_parse_from(["orbcode", "--permission-mode", "bypassPermissions", "tui"])
                .expect("parse");
        assert_eq!(
            parsed.permission_mode,
            Some(CliPermissionMode::BypassPermissions)
        );
    }

    #[test]
    fn cli_parses_resume_without_value() {
        let parsed = Cli::try_parse_from(["orbcode", "-r"]).expect("parse");
        assert_eq!(parsed.resume.as_deref(), Some(""));
    }

    #[test]
    fn cli_parses_resume_with_equals_form() {
        let parsed = Cli::try_parse_from(["orbcode", "--resume=abc-123"]).expect("parse");
        assert_eq!(parsed.resume.as_deref(), Some("abc-123"));
    }

    #[test]
    fn cli_parses_resume_with_space_form() {
        let parsed = Cli::try_parse_from(["orbcode", "--resume", "abc-123"]).expect("parse");
        assert_eq!(parsed.resume.as_deref(), Some("abc-123"));
        let parsed = Cli::try_parse_from(["orbcode", "-r", "abc-123"]).expect("parse");
        assert_eq!(parsed.resume.as_deref(), Some("abc-123"));
    }

    #[test]
    fn cli_resume_greedily_consumes_non_flag_token() {
        let parsed = Cli::try_parse_from(["orbcode", "-r", "tui"]).expect("parse");
        assert_eq!(parsed.resume.as_deref(), Some("tui"));
        assert!(parsed.command.is_none());
    }

    #[test]
    fn cli_rejects_positional_prompt_without_print() {
        let parsed = Cli::try_parse_from(["orbcode", "hello"]).expect("parse");
        assert_eq!(parsed.prompt.as_deref(), Some("hello"));
        assert!(parsed.command.is_none());
        assert!(!parsed.print);
    }

    #[test]
    fn cli_parses_session_id_and_append_system_prompt() {
        let parsed = Cli::try_parse_from([
            "orbcode",
            "--session-id",
            "abc",
            "--append-system-prompt",
            "extra context",
            "tui",
        ])
        .expect("parse");
        assert_eq!(parsed.session_id.as_deref(), Some("abc"));
        assert_eq!(
            parsed.append_system_prompt.as_deref(),
            Some("extra context")
        );
    }

    #[test]
    fn load_inline_settings_source_accepts_inline_json() {
        let payload = "{\"model\":\"sonnet\"}";
        let loaded = load_inline_settings_source(payload).expect("inline JSON");
        let parsed: serde_json::Value = serde_json::from_str(&loaded).unwrap();
        assert_eq!(parsed["model"], "sonnet");
    }

    #[test]
    fn load_inline_settings_source_rejects_invalid_json() {
        assert!(load_inline_settings_source("{not-json").is_err());
    }

    #[test]
    fn permission_mode_conversion_round_trip() {
        for mode in [
            CliPermissionMode::Default,
            CliPermissionMode::AcceptEdits,
            CliPermissionMode::BypassPermissions,
            CliPermissionMode::DontAsk,
            CliPermissionMode::Plan,
            CliPermissionMode::Auto,
        ] {
            let mapped: PermissionMode = mode.into();
            assert!(!mapped.as_str().is_empty());
        }
    }
}
