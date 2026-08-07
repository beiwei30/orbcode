//! OpenSSH launcher for the canonical child-stdio transport.
//!
//! The launcher never invokes a local shell and never owns SSH credentials.
//! OpenSSH continues to resolve its normal config, agent, known-hosts, jump
//! hosts, and terminal-backed authentication. SSH servers execute remote
//! commands through the account's login shell, so every dynamic remote path is
//! encoded as one POSIX-shell word before it enters the SSH command string.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

#[cfg(test)]
use crate::InteractiveQuestionsCapability;
use crate::{
    AppClient, ChildExitDiagnostics, ChildStdioHandle, ChildStdioTransport,
    ChildStdioTransportConfig, interactive_client_capabilities,
};

/// One reviewed OpenSSH `-o Key=Value` setting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshOption {
    key: &'static str,
    value: String,
}

impl SshOption {
    pub fn key(&self) -> &'static str {
        self.key
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    fn as_argument(&self) -> String {
        format!("{}={}", self.key, self.value)
    }
}

impl fmt::Display for SshOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}={}", self.key, self.value)
    }
}

impl FromStr for SshOption {
    type Err = SshOptionParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (raw_key, value) = raw
            .split_once('=')
            .ok_or(SshOptionParseError::MissingEquals)?;
        let key = reviewed_option_key(raw_key)
            .ok_or_else(|| SshOptionParseError::UnsupportedKey(raw_key.to_string()))?;
        validate_option_value(key, value)?;
        Ok(Self {
            key,
            value: value.to_string(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SshOptionParseError {
    #[error("SSH option must use KEY=VALUE")]
    MissingEquals,
    #[error("SSH option {0:?} is not in the reviewed allowlist")]
    UnsupportedKey(String),
    #[error("SSH option {key} has invalid value: {message}")]
    InvalidValue { key: String, message: String },
}

/// Launch policy shared by the TUI and desktop host.
#[derive(Clone, Debug)]
pub struct SshRemoteConfig {
    pub target: String,
    pub remote_cwd: Option<String>,
    pub remote_orbcode_path: String,
    pub options: Vec<SshOption>,
    pub initialize_timeout: Duration,
    pub child: ChildStdioTransportConfig,
    ssh_program: PathBuf,
}

impl SshRemoteConfig {
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            remote_cwd: None,
            remote_orbcode_path: "orbcode".into(),
            options: Vec::new(),
            initialize_timeout: Duration::from_secs(20),
            child: ChildStdioTransportConfig::default(),
            ssh_program: default_ssh_program(),
        }
    }

    /// Override the OpenSSH executable. Release desktop hosts should retain
    /// the absolute system default; this hook also supports deterministic test
    /// harnesses.
    pub fn with_ssh_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.ssh_program = program.into();
        self
    }
}

/// A connected canonical client and the handle that owns its SSH child.
pub struct SshRemoteConnection {
    client: AppClient,
    child: ChildStdioHandle,
}

impl SshRemoteConnection {
    pub async fn connect(config: SshRemoteConfig) -> Result<Self, SshRemoteError> {
        let initialize_timeout = config.initialize_timeout;
        let (transport, child) = spawn_ssh_transport(config).await?;

        let initialize = timeout(
            initialize_timeout,
            AppClient::from_transport_with_capabilities(
                Box::new(transport),
                interactive_client_capabilities(),
            ),
        )
        .await;
        match initialize {
            Ok(Ok(client)) => Ok(Self { client, child }),
            Ok(Err(error)) => {
                let diagnostics = child.shutdown().await.ok();
                Err(classify_initialize_failure(error.to_string(), diagnostics))
            }
            Err(_) => {
                let diagnostics = child.shutdown().await.ok();
                Err(SshRemoteError::InitializeTimeout {
                    timeout: initialize_timeout,
                    diagnostics,
                })
            }
        }
    }

    pub fn client(&self) -> &AppClient {
        &self.client
    }

    pub fn child(&self) -> &ChildStdioHandle {
        &self.child
    }

    pub fn into_parts(self) -> (AppClient, ChildStdioHandle) {
        (self.client, self.child)
    }
}

/// Spawn the reviewed OpenSSH command without consuming protocol initialize.
/// Desktop relays use this when the generated TypeScript client owns the
/// canonical initialize envelope and request IDs.
pub async fn spawn_ssh_transport(
    config: SshRemoteConfig,
) -> Result<(ChildStdioTransport, ChildStdioHandle), SshRemoteError> {
    validate_remote_config(&config)?;
    let command = build_ssh_command(&config);
    ChildStdioTransport::spawn(command, config.child)
        .await
        .map_err(|error| SshRemoteError::Launch(error.to_string()))
}

#[derive(Debug, thiserror::Error)]
pub enum SshRemoteError {
    #[error("invalid SSH remote configuration: {0}")]
    InvalidConfig(String),
    #[error("failed to launch system OpenSSH: {0}")]
    Launch(String),
    #[error("SSH host-key verification failed: {detail}")]
    HostKey {
        detail: String,
        diagnostics: Option<ChildExitDiagnostics>,
    },
    #[error("SSH authentication failed: {detail}")]
    Authentication {
        detail: String,
        diagnostics: Option<ChildExitDiagnostics>,
    },
    #[error("SSH connection failed: {detail}")]
    Connection {
        detail: String,
        diagnostics: Option<ChildExitDiagnostics>,
    },
    #[error("remote orbcode executable was not found: {detail}")]
    RemoteBinaryNotFound {
        detail: String,
        diagnostics: Option<ChildExitDiagnostics>,
    },
    #[error("remote protocol initialize failed: {message}")]
    ProtocolInitialize {
        message: String,
        diagnostics: Option<ChildExitDiagnostics>,
    },
    #[error("remote protocol initialize timed out after {timeout:?}")]
    InitializeTimeout {
        timeout: Duration,
        diagnostics: Option<ChildExitDiagnostics>,
    },
    #[error("OpenSSH exited before protocol initialize: {detail}")]
    SshExited {
        detail: String,
        diagnostics: Option<ChildExitDiagnostics>,
    },
}

impl SshRemoteError {
    pub fn diagnostics(&self) -> Option<&ChildExitDiagnostics> {
        match self {
            Self::HostKey { diagnostics, .. }
            | Self::Authentication { diagnostics, .. }
            | Self::Connection { diagnostics, .. }
            | Self::RemoteBinaryNotFound { diagnostics, .. }
            | Self::ProtocolInitialize { diagnostics, .. }
            | Self::InitializeTimeout { diagnostics, .. }
            | Self::SshExited { diagnostics, .. } => diagnostics.as_ref(),
            Self::InvalidConfig(_) | Self::Launch(_) => None,
        }
    }
}

fn default_ssh_program() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/usr/bin/ssh")
    }
    #[cfg(not(unix))]
    {
        PathBuf::from("ssh")
    }
}

fn reviewed_option_key(raw: &str) -> Option<&'static str> {
    match raw.to_ascii_lowercase().as_str() {
        "port" => Some("Port"),
        "user" => Some("User"),
        "hostname" => Some("HostName"),
        "identityfile" => Some("IdentityFile"),
        "identitiesonly" => Some("IdentitiesOnly"),
        "proxyjump" => Some("ProxyJump"),
        "connecttimeout" => Some("ConnectTimeout"),
        "serveraliveinterval" => Some("ServerAliveInterval"),
        "serveralivecountmax" => Some("ServerAliveCountMax"),
        _ => None,
    }
}

fn validate_option_value(key: &str, value: &str) -> Result<(), SshOptionParseError> {
    let invalid = |message: &str| SshOptionParseError::InvalidValue {
        key: key.to_string(),
        message: message.to_string(),
    };
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(invalid(
            "must be non-empty and contain no control characters",
        ));
    }
    match key {
        "Port" => {
            value
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| invalid("expected an integer from 1 to 65535"))?;
        }
        "ConnectTimeout" | "ServerAliveInterval" | "ServerAliveCountMax" => {
            value
                .parse::<u32>()
                .ok()
                .filter(|number| *number <= 86_400)
                .ok_or_else(|| invalid("expected an integer from 0 to 86400"))?;
        }
        "IdentitiesOnly" if !matches!(value.to_ascii_lowercase().as_str(), "yes" | "no") => {
            return Err(invalid("expected yes or no"));
        }
        _ => {}
    }
    Ok(())
}

fn validate_remote_config(config: &SshRemoteConfig) -> Result<(), SshRemoteError> {
    if config.target.is_empty()
        || config.target.starts_with('-')
        || config.target.chars().any(char::is_control)
    {
        return Err(SshRemoteError::InvalidConfig(
            "target must be non-empty, must not start with '-', and must contain no control characters"
                .into(),
        ));
    }
    if config.initialize_timeout.is_zero() {
        return Err(SshRemoteError::InvalidConfig(
            "initialize timeout must be greater than zero".into(),
        ));
    }
    if let Some(cwd) = &config.remote_cwd
        && (!cwd.starts_with('/') || cwd.chars().any(char::is_control))
    {
        return Err(SshRemoteError::InvalidConfig(
            "remote cwd must be an absolute path with no control characters".into(),
        ));
    }
    let binary = &config.remote_orbcode_path;
    let safe_bare_binary = !binary.is_empty()
        && !binary.starts_with('-')
        && binary.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        });
    let safe_absolute_binary = binary.starts_with('/') && !binary.chars().any(char::is_control);
    if !safe_bare_binary && !safe_absolute_binary {
        return Err(SshRemoteError::InvalidConfig(
            "remote orbcode path must be an absolute path or a simple command name".into(),
        ));
    }
    Ok(())
}

fn build_ssh_command(config: &SshRemoteConfig) -> Command {
    let mut command = Command::new(&config.ssh_program);
    for option in &config.options {
        command.arg("-o").arg(option.as_argument());
    }
    command.arg("--").arg(&config.target);
    command.arg(build_remote_command(config));
    command
}

fn build_remote_command(config: &SshRemoteConfig) -> String {
    let executable = shell_word(&config.remote_orbcode_path);
    let serve = shell_word("serve");
    let stdio = shell_word("--stdio");
    match &config.remote_cwd {
        Some(cwd) => format!(
            "cd {} && exec {executable} {serve} {stdio}",
            shell_word(cwd)
        ),
        None => format!("exec {executable} {serve} {stdio}"),
    }
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn classify_initialize_failure(
    message: String,
    diagnostics: Option<ChildExitDiagnostics>,
) -> SshRemoteError {
    let detail = diagnostic_detail(diagnostics.as_ref());
    let lowercase = detail.to_ascii_lowercase();
    if lowercase.contains("host key verification failed")
        || lowercase.contains("remote host identification has changed")
    {
        return SshRemoteError::HostKey {
            detail,
            diagnostics,
        };
    }
    if lowercase.contains("permission denied")
        || lowercase.contains("authentication failed")
        || lowercase.contains("no supported authentication methods")
    {
        return SshRemoteError::Authentication {
            detail,
            diagnostics,
        };
    }
    if diagnostics.as_ref().and_then(|value| value.exit_code) == Some(127)
        || lowercase.contains("command not found")
        || lowercase.contains("orbcode: not found")
    {
        return SshRemoteError::RemoteBinaryNotFound {
            detail,
            diagnostics,
        };
    }
    if lowercase.contains("could not resolve hostname")
        || lowercase.contains("connection timed out")
        || lowercase.contains("connection refused")
        || lowercase.contains("no route to host")
    {
        return SshRemoteError::Connection {
            detail,
            diagnostics,
        };
    }
    if diagnostics
        .as_ref()
        .is_some_and(|value| value.exit_code == Some(255))
    {
        return SshRemoteError::SshExited {
            detail,
            diagnostics,
        };
    }
    SshRemoteError::ProtocolInitialize {
        message,
        diagnostics,
    }
}

fn diagnostic_detail(diagnostics: Option<&ChildExitDiagnostics>) -> String {
    let Some(diagnostics) = diagnostics else {
        return "no child diagnostics were available".into();
    };
    let stderr = diagnostics.stderr_tail.trim();
    if !stderr.is_empty() {
        return stderr.to_string();
    }
    match (diagnostics.exit_code, diagnostics.signal) {
        (Some(code), _) => format!("OpenSSH exited with code {code}"),
        (_, Some(signal)) => format!("OpenSSH exited from signal {signal}"),
        _ => "OpenSSH exited without stderr or an exit code".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewed_options_are_canonicalized_and_dangerous_keys_rejected() {
        assert_eq!(
            "port=2222".parse::<SshOption>().unwrap().to_string(),
            "Port=2222"
        );
        assert_eq!(
            "identitiesonly=YES"
                .parse::<SshOption>()
                .unwrap()
                .to_string(),
            "IdentitiesOnly=YES"
        );
        for raw in [
            "ProxyCommand=touch /tmp/pwn",
            "LocalCommand=touch /tmp/pwn",
            "StrictHostKeyChecking=no",
            "RemoteCommand=sh",
        ] {
            assert!(matches!(
                raw.parse::<SshOption>(),
                Err(SshOptionParseError::UnsupportedKey(_))
            ));
        }
    }

    #[test]
    fn remote_command_posix_quotes_dynamic_paths() {
        let mut config = SshRemoteConfig::new("host");
        config.remote_cwd = Some("/srv/it's; touch /tmp/pwn".into());
        config.remote_orbcode_path = "/opt/orb code/orb'code".into();
        assert_eq!(
            build_remote_command(&config),
            "cd '/srv/it'\"'\"'s; touch /tmp/pwn' && exec '/opt/orb code/orb'\"'\"'code' 'serve' '--stdio'"
        );
    }

    #[test]
    fn interactive_capability_remains_full_for_ssh_tui() {
        assert_eq!(
            interactive_client_capabilities().interactive_questions,
            Some(InteractiveQuestionsCapability::full())
        );
    }
}
