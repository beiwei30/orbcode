//! Privileged lifecycle host for the Orbcode desktop thin client.
//!
//! The host owns immutable assets, one child process, and canonical envelope
//! relay. It intentionally contains no AppServer or agent-core business calls.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicBool, Ordering},
};

use orbcode_app_server_client::{
    ChildExitDiagnostics, ChildExitReason, ChildStdioHandle, ChildStdioTransport,
    ChildStdioTransportConfig, ChildTermination, ClientMessage, ClientTransport, ServerMessage,
    SshOption, SshRemoteConfig, spawn_ssh_transport,
};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, Runtime};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const PROTOCOL_EVENT: &str = "orbcode://protocol";
const CONNECTION_EXIT_EVENT: &str = "orbcode://connection-exit";
const DEVELOPMENT_BINARY_ENV: &str = "ORBCODE_DESKTOP_DEV_BINARY";
const DESKTOP_SSH_DEFAULT_OPTIONS: [&str; 3] = [
    "ConnectTimeout=20",
    "ServerAliveInterval=15",
    "ServerAliveCountMax=3",
];

#[derive(Clone, Debug, Default)]
pub struct DesktopHostPolicy {
    bundled_binary_for_test: Option<PathBuf>,
    ssh_program_for_test: Option<PathBuf>,
}

impl DesktopHostPolicy {
    /// Construct deterministic launch policy for the isolated desktop harness.
    /// Production startup always uses [`Default`].
    #[doc(hidden)]
    pub fn test_harness(
        bundled_binary: impl Into<PathBuf>,
        ssh_program: impl Into<PathBuf>,
    ) -> Self {
        Self {
            bundled_binary_for_test: Some(bundled_binary.into()),
            ssh_program_for_test: Some(ssh_program.into()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    Local,
    Ssh,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BinarySource {
    Bundled,
    DevelopmentOverride,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConnectionStatus {
    pub active: bool,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ConnectionKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_source: Option<BinarySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_pid: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct LocalConnectionInput {
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SshConnectionInput {
    pub target: String,
    pub remote_cwd: Option<String>,
    pub remote_orbcode_path: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProtocolReply {
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<ServerMessage>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostExitDiagnostics {
    pub pid: u32,
    pub reason: ChildExitReason,
    pub termination: ChildTermination,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

impl From<ChildExitDiagnostics> for HostExitDiagnostics {
    fn from(value: ChildExitDiagnostics) -> Self {
        Self {
            pid: value.pid,
            reason: value.reason,
            termination: value.termination,
            success: value.success,
            exit_code: value.exit_code,
            signal: value.signal,
        }
    }
}

#[derive(Clone, Serialize)]
struct HostProtocolEvent {
    generation: u64,
    message: ServerMessage,
}

#[derive(Clone, Serialize)]
struct HostConnectionExitEvent {
    generation: u64,
    diagnostics: HostExitDiagnostics,
}

trait ProtocolEventSink: Send + Sync {
    fn emit(&self, event: HostProtocolEvent) -> Result<(), String>;
    fn emit_connection_exit(&self, event: HostConnectionExitEvent) -> Result<(), String>;
}

struct TauriEventSink<R: Runtime>(tauri::AppHandle<R>);

impl<R: Runtime> ProtocolEventSink for TauriEventSink<R> {
    fn emit(&self, event: HostProtocolEvent) -> Result<(), String> {
        self.0
            .emit(PROTOCOL_EVENT, event)
            .map_err(|_| "desktop protocol event delivery failed".to_string())
    }

    fn emit_connection_exit(&self, event: HostConnectionExitEvent) -> Result<(), String> {
        self.0
            .emit(CONNECTION_EXIT_EVENT, event)
            .map_err(|_| "desktop connection-exit event delivery failed".to_string())
    }
}

struct RuntimeContext {
    bundled_binary: Result<PathBuf, String>,
    event_sink: Arc<dyn ProtocolEventSink>,
}

struct ActiveConnection {
    generation: u64,
    kind: ConnectionKind,
    binary_source: Option<BinarySource>,
    transport: Arc<ChildStdioTransport>,
    child: ChildStdioHandle,
    pumps: Vec<JoinHandle<()>>,
}

#[derive(Default)]
struct HostInner {
    generation: u64,
    active: Option<ActiveConnection>,
}

struct HostShared {
    policy: DesktopHostPolicy,
    runtime: StdMutex<Option<RuntimeContext>>,
    operation: Mutex<()>,
    inner: Mutex<HostInner>,
    has_active: AtomicBool,
    cleanup_started: AtomicBool,
}

#[derive(Clone)]
pub struct DesktopHostState {
    shared: Arc<HostShared>,
}

impl DesktopHostState {
    fn new(policy: DesktopHostPolicy) -> Self {
        Self {
            shared: Arc::new(HostShared {
                policy,
                runtime: StdMutex::new(None),
                operation: Mutex::new(()),
                inner: Mutex::new(HostInner::default()),
                has_active: AtomicBool::new(false),
                cleanup_started: AtomicBool::new(false),
            }),
        }
    }

    fn configure_runtime<R: Runtime>(&self, app: tauri::AppHandle<R>) -> Result<(), String> {
        let bundled_binary = match &self.shared.policy.bundled_binary_for_test {
            Some(path) => Ok(path.clone()),
            None => app
                .path()
                .resource_dir()
                .map(|directory| directory.join("bin").join("orbcode"))
                .map_err(|_| "resolve desktop application resource directory".to_string()),
        };
        let mut runtime = self
            .shared
            .runtime
            .lock()
            .map_err(|_| "desktop runtime state lock was poisoned".to_string())?;
        if runtime.is_some() {
            return Err("desktop runtime was configured more than once".to_string());
        }
        *runtime = Some(RuntimeContext {
            bundled_binary,
            event_sink: Arc::new(TauriEventSink(app)),
        });
        Ok(())
    }

    fn has_active(&self) -> bool {
        self.shared.has_active.load(Ordering::SeqCst)
    }

    fn begin_cleanup(&self) -> bool {
        !self.shared.cleanup_started.swap(true, Ordering::SeqCst)
    }

    async fn connect_local(&self, input: LocalConnectionInput) -> Result<ConnectionStatus, String> {
        let (binary, source) = self.resolve_local_binary()?;
        let cwd = resolve_working_directory(input.cwd)?;
        let _operation = self.shared.operation.lock().await;
        let generation = self.prepare_replacement().await?;

        let mut command = Command::new(binary);
        command.arg("serve").arg("--stdio").current_dir(cwd);
        let spawned = ChildStdioTransport::spawn(command, desktop_child_config()).await;
        let (transport, child) = match spawned {
            Ok(parts) => parts,
            Err(error) => return Err(format!("launch bundled orbcode child: {error}")),
        };
        self.install_connection(
            generation,
            ConnectionKind::Local,
            Some(source),
            transport,
            child,
        )
        .await
    }

    async fn connect_ssh(&self, input: SshConnectionInput) -> Result<ConnectionStatus, String> {
        let mut config = SshRemoteConfig::new(input.target);
        config.remote_cwd = input.remote_cwd;
        if let Some(path) = input.remote_orbcode_path {
            config.remote_orbcode_path = path;
        }
        config.options = desktop_ssh_options(input.options)?;
        config.child = desktop_child_config();
        if let Some(program) = &self.shared.policy.ssh_program_for_test {
            config = config.with_ssh_program(program);
        }

        let _operation = self.shared.operation.lock().await;
        let generation = self.prepare_replacement().await?;
        let (transport, child) = spawn_ssh_transport(config)
            .await
            .map_err(|error| error.to_string())?;
        self.install_connection(generation, ConnectionKind::Ssh, None, transport, child)
            .await
    }

    async fn prepare_replacement(&self) -> Result<u64, String> {
        let (old, generation) = {
            let mut inner = self.shared.inner.lock().await;
            let generation = inner
                .generation
                .checked_add(1)
                .ok_or_else(|| "desktop connection generation exhausted".to_string())?;
            inner.generation = generation;
            (inner.active.take(), generation)
        };
        self.shared.has_active.store(false, Ordering::SeqCst);
        if let Some(active) = old {
            stop_active(active).await?;
        }
        Ok(generation)
    }

    async fn install_connection(
        &self,
        generation: u64,
        kind: ConnectionKind,
        binary_source: Option<BinarySource>,
        transport: ChildStdioTransport,
        child: ChildStdioHandle,
    ) -> Result<ConnectionStatus, String> {
        let notification_rx = transport
            .take_notification_receiver()
            .await
            .ok_or_else(|| "child notification stream was already claimed".to_string())?;
        let server_request_rx = transport
            .take_server_request_receiver()
            .await
            .ok_or_else(|| "child server-request stream was already claimed".to_string())?;
        let sink = self.event_sink()?;
        let pumps = vec![
            spawn_notification_pump(generation, Arc::clone(&sink), notification_rx),
            spawn_server_request_pump(generation, Arc::clone(&sink), server_request_rx),
            spawn_connection_exit_monitor(generation, sink, child.clone()),
        ];
        let child_pid = child.pid();
        let active = ActiveConnection {
            generation,
            kind: kind.clone(),
            binary_source: binary_source.clone(),
            transport: Arc::new(transport),
            child,
            pumps,
        };
        self.shared.inner.lock().await.active = Some(active);
        self.shared.has_active.store(true, Ordering::SeqCst);
        self.shared.cleanup_started.store(false, Ordering::SeqCst);
        Ok(ConnectionStatus {
            active: true,
            generation,
            kind: Some(kind),
            binary_source,
            child_pid: Some(child_pid),
        })
    }

    async fn protocol_send(
        &self,
        generation: u64,
        message: ClientMessage,
    ) -> Result<ProtocolReply, String> {
        let transport = {
            let inner = self.shared.inner.lock().await;
            let active = inner
                .active
                .as_ref()
                .ok_or_else(|| "desktop protocol connection is not active".to_string())?;
            if active.generation != generation {
                return Err("stale desktop connection generation".to_string());
            }
            Arc::clone(&active.transport)
        };

        let response = match message {
            ClientMessage::Request(request) => {
                let original_id = request.id;
                let mut response = transport
                    .request_raw(&request.method, request.params)
                    .await
                    .map_err(|error| error.to_string())?;
                response.id = original_id;
                Some(ServerMessage::Response(response))
            }
            ClientMessage::Response(response) => {
                transport
                    .respond_raw(response.id, response.result)
                    .await
                    .map_err(|error| error.to_string())?;
                None
            }
            _ => return Err("unsupported client protocol message".to_string()),
        };

        Ok(ProtocolReply {
            generation,
            message: response,
        })
    }

    async fn disconnect(&self, generation: u64) -> Result<Option<HostExitDiagnostics>, String> {
        let _operation = self.shared.operation.lock().await;
        let active = {
            let mut inner = self.shared.inner.lock().await;
            if inner.generation != generation {
                return Err("stale desktop connection generation".to_string());
            }
            inner.active.take()
        };
        self.shared.has_active.store(false, Ordering::SeqCst);
        match active {
            Some(active) => stop_active(active).await.map(Some),
            None => Ok(None),
        }
    }

    async fn status(&self) -> ConnectionStatus {
        let inner = self.shared.inner.lock().await;
        match &inner.active {
            Some(active) => ConnectionStatus {
                active: true,
                generation: active.generation,
                kind: Some(active.kind.clone()),
                binary_source: active.binary_source.clone(),
                child_pid: Some(active.child.pid()),
            },
            None => ConnectionStatus {
                active: false,
                generation: inner.generation,
                kind: None,
                binary_source: None,
                child_pid: None,
            },
        }
    }

    async fn shutdown_all(&self) -> Result<Option<HostExitDiagnostics>, String> {
        let _operation = self.shared.operation.lock().await;
        let active = self.shared.inner.lock().await.active.take();
        self.shared.has_active.store(false, Ordering::SeqCst);
        match active {
            Some(active) => stop_active(active).await.map(Some),
            None => Ok(None),
        }
    }

    fn resolve_local_binary(&self) -> Result<(PathBuf, BinarySource), String> {
        let bundled = {
            let runtime = self
                .shared
                .runtime
                .lock()
                .map_err(|_| "desktop runtime state lock was poisoned".to_string())?;
            runtime
                .as_ref()
                .ok_or_else(|| "desktop runtime is not configured".to_string())?
                .bundled_binary
                .clone()
        };
        let development_override = if cfg!(debug_assertions) {
            std::env::var_os(DEVELOPMENT_BINARY_ENV)
        } else {
            None
        };
        select_local_binary(bundled, development_override)
    }

    fn event_sink(&self) -> Result<Arc<dyn ProtocolEventSink>, String> {
        self.shared
            .runtime
            .lock()
            .map_err(|_| "desktop runtime state lock was poisoned".to_string())?
            .as_ref()
            .map(|runtime| Arc::clone(&runtime.event_sink))
            .ok_or_else(|| "desktop runtime is not configured".to_string())
    }
}

fn select_local_binary(
    bundled: Result<PathBuf, String>,
    development_override: Option<std::ffi::OsString>,
) -> Result<(PathBuf, BinarySource), String> {
    if let Ok(path) = bundled
        && let Ok(executable) = validate_executable(&path)
    {
        return Ok((executable, BinarySource::Bundled));
    }

    if let Some(path) = development_override {
        return validate_executable(Path::new(&path))
            .map(|path| (path, BinarySource::DevelopmentOverride))
            .map_err(|_| {
                format!("{DEVELOPMENT_BINARY_ENV} must name an absolute executable file")
            });
    }

    Err("bundled orbcode executable is unavailable; install a complete desktop bundle".to_string())
}

#[tauri::command]
async fn connect_local(
    state: tauri::State<'_, DesktopHostState>,
    input: LocalConnectionInput,
) -> Result<ConnectionStatus, String> {
    state.connect_local(input).await
}

#[tauri::command]
async fn connect_ssh(
    state: tauri::State<'_, DesktopHostState>,
    input: SshConnectionInput,
) -> Result<ConnectionStatus, String> {
    state.connect_ssh(input).await
}

#[tauri::command]
async fn protocol_send(
    state: tauri::State<'_, DesktopHostState>,
    generation: u64,
    message: ClientMessage,
) -> Result<ProtocolReply, String> {
    state.protocol_send(generation, message).await
}

#[tauri::command]
async fn disconnect(
    state: tauri::State<'_, DesktopHostState>,
    generation: u64,
) -> Result<Option<HostExitDiagnostics>, String> {
    state.disconnect(generation).await
}

#[tauri::command]
async fn connection_status(
    state: tauri::State<'_, DesktopHostState>,
) -> Result<ConnectionStatus, String> {
    Ok(state.status().await)
}

pub fn configure_builder<R: Runtime>(
    builder: tauri::Builder<R>,
    policy: DesktopHostPolicy,
) -> tauri::Builder<R> {
    let state = DesktopHostState::new(policy);
    let setup_state = state.clone();
    builder
        .manage(state)
        .setup(move |app| {
            setup_state
                .configure_runtime(app.handle().clone())
                .map_err(Into::into)
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<DesktopHostState>();
                if state.has_active() {
                    api.prevent_close();
                    if state.begin_cleanup() {
                        let window = window.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = shutdown_children(window.app_handle()).await;
                            let _ = window.destroy();
                        });
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            connect_local,
            connect_ssh,
            protocol_send,
            disconnect,
            connection_status
        ])
}

pub fn navigation_guard<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("packaged-navigation-only")
        .on_navigation(|_webview, url| {
            url.scheme() == "tauri"
                || (matches!(url.scheme(), "http" | "https")
                    && url.host_str() == Some("tauri.localhost"))
        })
        .build()
}

pub async fn shutdown_children<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Option<HostExitDiagnostics>, String> {
    app.state::<DesktopHostState>().shutdown_all().await
}

pub fn run() {
    let app = configure_builder(tauri::Builder::default(), DesktopHostPolicy::default())
        .plugin(navigation_guard())
        .build(tauri::generate_context!())
        .expect("desktop host startup failed");
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
            let state = app.state::<DesktopHostState>();
            if state.has_active() {
                api.prevent_exit();
                if state.begin_cleanup() {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = shutdown_children(&app).await;
                        app.exit(code.unwrap_or(0));
                    });
                }
            }
        }
    });
}

async fn stop_active(active: ActiveConnection) -> Result<HostExitDiagnostics, String> {
    for pump in active.pumps {
        pump.abort();
    }
    active
        .child
        .shutdown()
        .await
        .map(HostExitDiagnostics::from)
        .map_err(|error| error.to_string())
}

fn spawn_notification_pump(
    generation: u64,
    sink: Arc<dyn ProtocolEventSink>,
    mut receiver: tokio::sync::mpsc::Receiver<
        orbcode_app_server_client::ServerNotificationEnvelope,
    >,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(notification) = receiver.recv().await {
            if sink
                .emit(HostProtocolEvent {
                    generation,
                    message: ServerMessage::Notification(notification),
                })
                .is_err()
            {
                break;
            }
        }
    })
}

fn spawn_server_request_pump(
    generation: u64,
    sink: Arc<dyn ProtocolEventSink>,
    mut receiver: tokio::sync::mpsc::Receiver<orbcode_app_server_client::ServerRequestEnvelope>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(request) = receiver.recv().await {
            if sink
                .emit(HostProtocolEvent {
                    generation,
                    message: ServerMessage::Request(request),
                })
                .is_err()
            {
                break;
            }
        }
    })
}

fn spawn_connection_exit_monitor(
    generation: u64,
    sink: Arc<dyn ProtocolEventSink>,
    child: ChildStdioHandle,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Ok(diagnostics) = child.wait_for_exit().await {
            let _ = sink.emit_connection_exit(HostConnectionExitEvent {
                generation,
                diagnostics: diagnostics.into(),
            });
        }
    })
}

fn resolve_working_directory(cwd: Option<String>) -> Result<PathBuf, String> {
    let path = match cwd {
        Some(cwd) => PathBuf::from(cwd),
        None => {
            std::env::current_dir().map_err(|_| "resolve current working directory".to_string())?
        }
    };
    if !path.is_absolute() {
        return Err("local working directory must be an absolute path".to_string());
    }
    let path = path
        .canonicalize()
        .map_err(|_| "local working directory does not exist".to_string())?;
    if !path.is_dir() {
        return Err("local working directory must name a directory".to_string());
    }
    Ok(path)
}

fn validate_executable(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("executable path must be absolute".to_string());
    }
    let path = path
        .canonicalize()
        .map_err(|_| "executable does not exist".to_string())?;
    let metadata = path
        .metadata()
        .map_err(|_| "read executable metadata".to_string())?;
    if !metadata.is_file() {
        return Err("executable path must name a file".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("executable file has no execute permission".to_string());
        }
    }
    Ok(path)
}

fn desktop_child_config() -> ChildStdioTransportConfig {
    let mut config = ChildStdioTransportConfig::default();
    for (key, value) in std::env::vars() {
        if is_sensitive_environment_key(&key) {
            config = config.with_redacted_value(value);
        }
    }
    config
}

fn desktop_ssh_options(raw: Vec<String>) -> Result<Vec<SshOption>, String> {
    let mut options = raw
        .iter()
        .map(|option| SshOption::from_str(option).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    if options.iter().any(|option| {
        matches!(
            option.key(),
            "ConnectTimeout" | "ServerAliveInterval" | "ServerAliveCountMax"
        ) && option.value() == "0"
    }) {
        return Err("desktop SSH timeout and keepalive values must be greater than zero".into());
    }
    for default in DESKTOP_SSH_DEFAULT_OPTIONS {
        let candidate = SshOption::from_str(default).expect("desktop SSH default is valid");
        if !options.iter().any(|option| option.key() == candidate.key()) {
            options.push(candidate);
        }
    }
    Ok(options)
}

fn is_sensitive_environment_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.contains("TOKEN")
        || upper.contains("SECRET")
        || upper.contains("PASSWORD")
        || upper.ends_with("_KEY")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_environment_keys_are_recognized_without_values() {
        assert!(is_sensitive_environment_key("ANTHROPIC_API_KEY"));
        assert!(is_sensitive_environment_key("SSH_AUTH_TOKEN"));
        assert!(is_sensitive_environment_key("client_secret"));
        assert!(!is_sensitive_environment_key("PATH"));
    }

    #[test]
    fn local_working_directory_must_be_absolute() {
        let error = resolve_working_directory(Some("relative".into())).unwrap_err();
        assert_eq!(error, "local working directory must be an absolute path");
    }

    #[test]
    fn local_binary_selection_is_bundle_first_and_marks_development_fallback() {
        let executable = std::env::current_exe().expect("current test executable");
        let selected = select_local_binary(
            Ok(executable.clone()),
            Some("/path/that/must/not/exist".into()),
        )
        .expect("select bundled executable first");
        assert_eq!(selected.1, BinarySource::Bundled);

        let selected = select_local_binary(
            Err("bundle unavailable".into()),
            Some(executable.into_os_string()),
        )
        .expect("select explicit development executable");
        assert_eq!(selected.1, BinarySource::DevelopmentOverride);
    }

    #[test]
    fn desktop_ssh_defaults_bound_connect_and_dead_peer_detection() {
        let defaults = desktop_ssh_options(Vec::new()).expect("desktop SSH defaults");
        assert_eq!(
            defaults.iter().map(ToString::to_string).collect::<Vec<_>>(),
            DESKTOP_SSH_DEFAULT_OPTIONS
        );

        let overridden = desktop_ssh_options(vec![
            "ServerAliveInterval=30".into(),
            "ServerAliveCountMax=2".into(),
        ])
        .expect("reviewed override");
        assert!(
            overridden
                .iter()
                .any(|option| option.to_string() == "ServerAliveInterval=30")
        );
        assert_eq!(
            overridden
                .iter()
                .filter(|option| option.key() == "ServerAliveInterval")
                .count(),
            1
        );
        assert!(
            overridden
                .iter()
                .any(|option| option.to_string() == "ConnectTimeout=20")
        );

        let disabled = desktop_ssh_options(vec!["ServerAliveInterval=0".into()]);
        assert!(
            matches!(disabled, Err(message) if message.contains("greater than zero")),
            "desktop renderer must not disable dead-peer detection"
        );
    }
}
