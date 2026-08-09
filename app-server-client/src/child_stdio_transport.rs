use std::collections::HashMap;
use std::fmt;
use std::io;
use std::process::{ExitStatus, Stdio};
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use orbcode_app_server_protocol::{
    ClientMessage, ClientRequestEnvelope, ResponseResult, ServerMessage,
    ServerNotificationEnvelope, ServerRequestEnvelope, ServerRequestResponse,
    ServerResponseEnvelope,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::ClientError;
use crate::transport::ClientTransport;

const STDERR_REDACTION_CONTEXT_BYTES: usize = 4096;

/// Child-stdio framing and lifecycle policy.
///
/// Command construction remains the launcher's responsibility. The transport
/// adds the three piped stdio handles and `kill_on_drop` immediately before it
/// spawns the already-constructed command.
#[derive(Clone)]
pub struct ChildStdioTransportConfig {
    /// Maximum encoded bytes in one inbound or outbound NDJSON record.
    pub max_payload_bytes: usize,
    /// Maximum UTF-8 bytes exposed in the sanitized stderr diagnostic tail.
    pub stderr_tail_bytes: usize,
    /// Time allowed for the child to exit after its stdin receives EOF.
    pub graceful_shutdown_timeout: Duration,
    /// Time allowed after a terminate signal before a hard kill.
    pub terminate_timeout: Duration,
    redacted_values: Vec<String>,
}

impl Default for ChildStdioTransportConfig {
    fn default() -> Self {
        Self {
            max_payload_bytes: 10 * 1024 * 1024,
            stderr_tail_bytes: 16 * 1024,
            graceful_shutdown_timeout: Duration::from_secs(2),
            terminate_timeout: Duration::from_secs(2),
            redacted_values: Vec::new(),
        }
    }
}

impl fmt::Debug for ChildStdioTransportConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChildStdioTransportConfig")
            .field("max_payload_bytes", &self.max_payload_bytes)
            .field("stderr_tail_bytes", &self.stderr_tail_bytes)
            .field("graceful_shutdown_timeout", &self.graceful_shutdown_timeout)
            .field("terminate_timeout", &self.terminate_timeout)
            .field("redacted_value_count", &self.redacted_values.len())
            .finish()
    }
}

impl ChildStdioTransportConfig {
    /// Add an exact value that must be removed from the exposed stderr tail.
    /// Empty values are ignored. Debug output reports only the value count.
    pub fn with_redacted_value(mut self, value: impl Into<String>) -> Self {
        let value = value.into();
        if !value.is_empty() {
            self.redacted_values.push(value);
        }
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildExitReason {
    Exited,
    ShutdownRequested,
    StdoutEof,
    MalformedStdout,
    OversizedStdout,
    StdoutIo,
    StdinIo,
    ServerRequestBackpressure,
    WaitFailed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildTermination {
    Natural,
    Graceful,
    Terminated,
    Killed,
    WaitFailed,
}

/// Sanitized process evidence exposed to supervising hosts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChildExitDiagnostics {
    pub pid: u32,
    pub reason: ChildExitReason,
    pub termination: ChildTermination,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stderr_tail: String,
}

#[derive(Clone, Debug)]
enum ProcessState {
    Running,
    Exited(ChildExitDiagnostics),
}

#[derive(Default)]
struct StderrCapture {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Cloneable lifecycle/diagnostic handle retained by the process-owning host.
#[derive(Clone)]
pub struct ChildStdioHandle {
    pid: u32,
    shutdown_tx: mpsc::UnboundedSender<()>,
    state_rx: watch::Receiver<ProcessState>,
    closed: Arc<AtomicBool>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>>,
}

impl ChildStdioHandle {
    /// Operating-system process identifier for lifecycle diagnostics.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Return completed diagnostics without waiting, or `None` while running.
    pub fn diagnostics(&self) -> Option<ChildExitDiagnostics> {
        match &*self.state_rx.borrow() {
            ProcessState::Running => None,
            ProcessState::Exited(diagnostics) => Some(diagnostics.clone()),
        }
    }

    /// Wait until the child has exited and its bounded stderr tail is ready.
    pub async fn wait_for_exit(&self) -> Result<ChildExitDiagnostics, ClientError> {
        let mut state_rx = self.state_rx.clone();
        loop {
            if let ProcessState::Exited(diagnostics) = &*state_rx.borrow() {
                return Ok(diagnostics.clone());
            }
            state_rx.changed().await.map_err(|_| {
                ClientError::Transport("child supervisor exited without diagnostics".into())
            })?;
        }
    }

    /// Close stdin, wait, send terminate, then hard-kill and reap if needed.
    /// Repeated and concurrent calls return the same final diagnostics.
    pub async fn shutdown(&self) -> Result<ChildExitDiagnostics, ClientError> {
        if let Some(diagnostics) = self.diagnostics() {
            return Ok(diagnostics);
        }
        self.closed.store(true, Ordering::SeqCst);
        self.pending.lock().await.clear();
        let _ = self.shutdown_tx.send(());
        self.wait_for_exit().await
    }
}

/// Canonical NDJSON client transport over a supervised child process.
pub struct ChildStdioTransport {
    writer_tx: mpsc::Sender<WriterCommand>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>>,
    closed: Arc<AtomicBool>,
    notification_rx: Mutex<Option<mpsc::Receiver<ServerNotificationEnvelope>>>,
    server_request_rx: Mutex<Option<mpsc::Receiver<ServerRequestEnvelope>>>,
    shutdown_tx: mpsc::UnboundedSender<()>,
    max_payload_bytes: usize,
}

impl ChildStdioTransport {
    /// Spawn an already-constructed command and take exclusive ownership of
    /// its stdin/stdout/stderr and lifecycle.
    pub async fn spawn(
        mut command: Command,
        config: ChildStdioTransportConfig,
    ) -> Result<(Self, ChildStdioHandle), ClientError> {
        if config.max_payload_bytes == 0 {
            return Err(ClientError::Transport(
                "child stdio max_payload_bytes must be greater than zero".into(),
            ));
        }

        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| ClientError::Transport(format!("child launch: {error}")))?;
        let pid = child
            .id()
            .ok_or_else(|| ClientError::Transport("spawned child has no process id".into()))?;

        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => return cleanup_incomplete_child(child, "stdin").await,
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => return cleanup_incomplete_child(child, "stdout").await,
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => return cleanup_incomplete_child(child, "stderr").await,
        };

        let pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let (notification_tx, notification_rx) = mpsc::channel(256);
        let (server_request_tx, server_request_rx) = mpsc::channel(64);
        let (writer_tx, writer_rx) = mpsc::channel(256);
        let (fault_tx, fault_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();
        let (state_tx, state_rx) = watch::channel(ProcessState::Running);
        let stderr_tail = Arc::new(StdMutex::new(StderrCapture::default()));
        let stderr_capture_bytes = config
            .stderr_tail_bytes
            .saturating_add(STDERR_REDACTION_CONTEXT_BYTES);
        let max_payload_bytes = config.max_payload_bytes;

        let reader_handle = tokio::spawn(read_stdout(
            stdout,
            config.max_payload_bytes,
            Arc::clone(&pending),
            Arc::clone(&closed),
            notification_tx,
            server_request_tx,
            fault_tx.clone(),
        ));
        let writer_handle = tokio::spawn(write_stdin(
            stdin,
            writer_rx,
            Arc::clone(&pending),
            Arc::clone(&closed),
            fault_tx,
        ));
        let stderr_handle = tokio::spawn(read_stderr(
            stderr,
            Arc::clone(&stderr_tail),
            stderr_capture_bytes,
        ));
        tokio::spawn(supervise_child(
            child,
            pid,
            config,
            Arc::clone(&pending),
            Arc::clone(&closed),
            writer_tx.clone(),
            writer_handle,
            reader_handle,
            stderr_handle,
            stderr_tail,
            shutdown_rx,
            fault_rx,
            state_tx,
        ));

        let handle = ChildStdioHandle {
            pid,
            shutdown_tx: shutdown_tx.clone(),
            state_rx,
            closed: Arc::clone(&closed),
            pending: Arc::clone(&pending),
        };
        let transport = Self {
            writer_tx,
            pending,
            closed,
            notification_rx: Mutex::new(Some(notification_rx)),
            server_request_rx: Mutex::new(Some(server_request_rx)),
            shutdown_tx,
            max_payload_bytes,
        };
        Ok((transport, handle))
    }

    async fn send_message(&self, message: ClientMessage) -> Result<(), ClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(ClientError::Transport(
                "child stdio connection closed".into(),
            ));
        }
        let line = serde_json::to_vec(&message).map_err(ClientError::Serialization)?;
        if line.len() > self.max_payload_bytes {
            return Err(ClientError::Transport(format!(
                "child stdio message exceeds {} byte limit",
                self.max_payload_bytes
            )));
        }
        self.writer_tx
            .send(WriterCommand::Message(line))
            .await
            .map_err(|_| ClientError::Transport("child stdio writer closed".into()))
    }

    /// Send one canonical client request while preserving the complete server
    /// response envelope. Privileged relays use this to keep the caller's
    /// success/error wire semantics without reimplementing stdio framing.
    pub async fn request_raw(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<ServerResponseEnvelope, ClientError> {
        let id = uuid::Uuid::new_v4().to_string();
        let (response_tx, response_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if self.closed.load(Ordering::SeqCst) {
                return Err(ClientError::Transport(
                    "child stdio connection closed".into(),
                ));
            }
            pending.insert(id.clone(), response_tx);
        }

        let message = ClientMessage::Request(ClientRequestEnvelope {
            id: id.clone(),
            method: method.to_string(),
            params,
        });
        if let Err(error) = self.send_message(message).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }

        response_rx.await.map_err(|_| ClientError::Cancelled)
    }

    /// Send a response to one server-initiated request.
    pub async fn respond_raw(&self, id: String, result: ResponseResult) -> Result<(), ClientError> {
        self.send_message(ClientMessage::Response(ServerRequestResponse {
            id,
            result,
        }))
        .await
    }
}

impl Drop for ChildStdioTransport {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.shutdown_tx.send(());
    }
}

#[async_trait::async_trait]
impl ClientTransport for ChildStdioTransport {
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, ClientError> {
        let response = self.request_raw(method, params).await?;
        match response.result {
            ResponseResult::Success { data } => Ok(data.unwrap_or(Value::Null)),
            ResponseResult::Error(error) => Err(ClientError::Protocol(error)),
            _ => Err(ClientError::Transport(
                "unsupported response result".to_string(),
            )),
        }
    }

    async fn respond_to_server_request(
        &self,
        id: String,
        result: ResponseResult,
    ) -> Result<(), ClientError> {
        self.respond_raw(id, result).await
    }

    async fn take_notification_receiver(
        &self,
    ) -> Option<mpsc::Receiver<ServerNotificationEnvelope>> {
        self.notification_rx.lock().await.take()
    }

    async fn take_server_request_receiver(&self) -> Option<mpsc::Receiver<ServerRequestEnvelope>> {
        self.server_request_rx.lock().await.take()
    }
}

enum WriterCommand {
    Message(Vec<u8>),
    Close(oneshot::Sender<()>),
}

#[derive(Clone, Debug)]
enum ChildFault {
    StdoutEof,
    MalformedStdout,
    OversizedStdout,
    StdoutIo,
    StdinIo,
    ServerRequestBackpressure,
}

impl ChildFault {
    fn into_reason(self) -> ChildExitReason {
        match self {
            Self::StdoutEof => ChildExitReason::StdoutEof,
            Self::MalformedStdout => ChildExitReason::MalformedStdout,
            Self::OversizedStdout => ChildExitReason::OversizedStdout,
            Self::StdoutIo => ChildExitReason::StdoutIo,
            Self::StdinIo => ChildExitReason::StdinIo,
            Self::ServerRequestBackpressure => ChildExitReason::ServerRequestBackpressure,
        }
    }
}

enum CappedLine {
    Eof,
    Line(Vec<u8>),
    Oversized,
}

async fn read_stdout(
    stdout: ChildStdout,
    max_payload_bytes: usize,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>>,
    closed: Arc<AtomicBool>,
    notification_tx: mpsc::Sender<ServerNotificationEnvelope>,
    server_request_tx: mpsc::Sender<ServerRequestEnvelope>,
    fault_tx: mpsc::UnboundedSender<ChildFault>,
) {
    let mut reader = BufReader::new(stdout);
    let fault = loop {
        let line = match read_capped_line(&mut reader, max_payload_bytes).await {
            Ok(line) => line,
            Err(_) => break ChildFault::StdoutIo,
        };
        let bytes = match line {
            CappedLine::Eof => break ChildFault::StdoutEof,
            CappedLine::Oversized => break ChildFault::OversizedStdout,
            CappedLine::Line(bytes) => bytes,
        };
        let Ok(line) = std::str::from_utf8(&bytes) else {
            break ChildFault::MalformedStdout;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<ServerMessage>(trimmed) else {
            break ChildFault::MalformedStdout;
        };
        match message {
            ServerMessage::Response(response) => {
                if let Some(sender) = pending.lock().await.remove(&response.id) {
                    let _ = sender.send(response);
                }
            }
            ServerMessage::Notification(notification) => {
                // Notifications are best-effort, as in the socket/WebSocket
                // transports. `try_send` keeps a slow renderer from blocking
                // later request responses on the same stdout stream.
                let _ = notification_tx.try_send(notification);
            }
            ServerMessage::Request(request) => {
                if server_request_tx.try_send(request).is_err() {
                    break ChildFault::ServerRequestBackpressure;
                }
            }
            _ => {}
        }
    };
    close_transport(&closed, &pending).await;
    let _ = fault_tx.send(fault);
}

async fn write_stdin(
    mut stdin: ChildStdin,
    mut writer_rx: mpsc::Receiver<WriterCommand>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>>,
    closed: Arc<AtomicBool>,
    fault_tx: mpsc::UnboundedSender<ChildFault>,
) {
    while let Some(command) = writer_rx.recv().await {
        match command {
            WriterCommand::Message(mut line) => {
                line.push(b'\n');
                if stdin.write_all(&line).await.is_err() || stdin.flush().await.is_err() {
                    close_transport(&closed, &pending).await;
                    let _ = fault_tx.send(ChildFault::StdinIo);
                    return;
                }
            }
            WriterCommand::Close(acknowledge) => {
                let _ = stdin.shutdown().await;
                let _ = acknowledge.send(());
                return;
            }
        }
    }
    let _ = stdin.shutdown().await;
}

async fn read_stderr(
    mut stderr: ChildStderr,
    tail: Arc<StdMutex<StderrCapture>>,
    max_capture_bytes: usize,
) {
    let mut buffer = [0_u8; 2048];
    loop {
        let read = match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let mut tail = tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        tail.bytes.extend_from_slice(&buffer[..read]);
        if tail.bytes.len() > max_capture_bytes {
            let remove = tail.bytes.len() - max_capture_bytes;
            tail.bytes.drain(..remove);
            tail.truncated = true;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn supervise_child(
    mut child: Child,
    pid: u32,
    config: ChildStdioTransportConfig,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>>,
    closed: Arc<AtomicBool>,
    writer_tx: mpsc::Sender<WriterCommand>,
    writer_handle: JoinHandle<()>,
    reader_handle: JoinHandle<()>,
    stderr_handle: JoinHandle<()>,
    stderr_tail: Arc<StdMutex<StderrCapture>>,
    mut shutdown_rx: mpsc::UnboundedReceiver<()>,
    mut fault_rx: mpsc::UnboundedReceiver<ChildFault>,
    state_tx: watch::Sender<ProcessState>,
) {
    enum Trigger {
        Exited(io::Result<ExitStatus>),
        Shutdown,
        Fault(ChildExitReason),
    }

    let trigger = tokio::select! {
        status = child.wait() => Trigger::Exited(status),
        _ = shutdown_rx.recv() => Trigger::Shutdown,
        Some(fault) = fault_rx.recv() => Trigger::Fault(fault.into_reason()),
    };

    let (mut reason, termination, status) = match trigger {
        Trigger::Exited(status) => (ChildExitReason::Exited, ChildTermination::Natural, status),
        Trigger::Shutdown => {
            let (status, termination) = stop_child(&mut child, &writer_tx, &config).await;
            (ChildExitReason::ShutdownRequested, termination, status)
        }
        Trigger::Fault(reason) => {
            let (status, termination) = stop_child(&mut child, &writer_tx, &config).await;
            (reason, termination, status)
        }
    };

    close_transport(&closed, &pending).await;
    request_writer_close(&writer_tx, config.graceful_shutdown_timeout).await;
    let _ = writer_handle.await;
    let _ = reader_handle.await;
    let _ = stderr_handle.await;

    let stderr_tail = {
        let capture = stderr_tail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        redact_stderr_tail(
            &capture.bytes,
            capture.truncated,
            &config.redacted_values,
            config.stderr_tail_bytes,
        )
    };
    let (success, exit_code, signal) = match status {
        Ok(status) => (status.success(), status.code(), exit_signal(&status)),
        Err(_) => {
            reason = ChildExitReason::WaitFailed;
            (false, None, None)
        }
    };
    let termination = if reason == ChildExitReason::WaitFailed {
        ChildTermination::WaitFailed
    } else {
        termination
    };
    state_tx.send_replace(ProcessState::Exited(ChildExitDiagnostics {
        pid,
        reason,
        termination,
        success,
        exit_code,
        signal,
        stderr_tail,
    }));
}

async fn stop_child(
    child: &mut Child,
    writer_tx: &mpsc::Sender<WriterCommand>,
    config: &ChildStdioTransportConfig,
) -> (io::Result<ExitStatus>, ChildTermination) {
    request_writer_close(writer_tx, config.graceful_shutdown_timeout).await;
    if let Ok(status) = timeout(config.graceful_shutdown_timeout, child.wait()).await {
        return (status, ChildTermination::Graceful);
    }

    if send_terminate(child).is_ok()
        && let Ok(status) = timeout(config.terminate_timeout, child.wait()).await
    {
        return (status, ChildTermination::Terminated);
    }

    let _ = child.start_kill();
    (child.wait().await, ChildTermination::Killed)
}

async fn request_writer_close(writer_tx: &mpsc::Sender<WriterCommand>, wait: Duration) {
    let (acknowledge, acknowledged) = oneshot::channel();
    let close = async {
        writer_tx
            .send(WriterCommand::Close(acknowledge))
            .await
            .map_err(|_| ())?;
        acknowledged.await.map_err(|_| ())
    };
    let _ = timeout(wait, close).await;
}

async fn close_transport(
    closed: &AtomicBool,
    pending: &Mutex<HashMap<String, oneshot::Sender<ServerResponseEnvelope>>>,
) {
    closed.store(true, Ordering::SeqCst);
    pending.lock().await.clear();
}

async fn cleanup_incomplete_child<T>(mut child: Child, pipe: &str) -> Result<T, ClientError> {
    let _ = child.kill().await;
    Err(ClientError::Transport(format!(
        "spawned child did not expose piped {pipe}"
    )))
}

async fn read_capped_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> io::Result<CappedLine> {
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if oversized {
                Ok(CappedLine::Oversized)
            } else if line.is_empty() {
                Ok(CappedLine::Eof)
            } else {
                Ok(CappedLine::Line(line))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |position| position + 1);
        let content = newline.map_or(available, |position| &available[..position]);
        if !oversized {
            if line.len().saturating_add(content.len()) > max_bytes {
                oversized = true;
            } else {
                line.extend_from_slice(content);
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return if oversized {
                Ok(CappedLine::Oversized)
            } else {
                Ok(CappedLine::Line(line))
            };
        }
    }
}

#[cfg(unix)]
fn send_terminate(child: &mut Child) -> io::Result<()> {
    let Some(pid) = child.id() else {
        return Ok(());
    };
    // SAFETY: `pid` comes from the live child owned by this supervisor and the
    // signal has no pointer or memory-safety contract.
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if result == 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

#[cfg(not(unix))]
fn send_terminate(_child: &mut Child) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "terminate signal is unavailable on this platform",
    ))
}

#[cfg(unix)]
fn exit_signal(status: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn exit_signal(_status: &ExitStatus) -> Option<i32> {
    None
}

fn redact_stderr_tail(
    raw: &[u8],
    truncated: bool,
    exact_values: &[String],
    max_bytes: usize,
) -> String {
    let safe_raw = if truncated {
        let Some(first_newline) = raw.iter().position(|byte| *byte == b'\n') else {
            return truncate_utf8_tail(
                "[stderr line omitted after bounded capture]".into(),
                max_bytes,
            );
        };
        &raw[first_newline + 1..]
    } else {
        raw
    };
    let mut text = String::from_utf8_lossy(safe_raw).into_owned();
    for value in exact_values {
        if !value.is_empty() {
            text = text.replace(value, "[REDACTED]");
        }
    }
    let redacted = text
        .split_inclusive('\n')
        .map(redact_common_line)
        .collect::<String>();
    truncate_utf8_tail(redacted, max_bytes)
}

fn redact_common_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let lowercase = trimmed.to_ascii_lowercase();
    for header in [
        "authorization:",
        "proxy-authorization:",
        "cookie:",
        "set-cookie:",
    ] {
        if lowercase.starts_with(header) {
            let prefix_len = line.len() - trimmed.len() + header.len();
            let newline = if line.ends_with('\n') { "\n" } else { "" };
            return format!("{} [REDACTED]{newline}", &line[..prefix_len]);
        }
    }

    let mut redacted = line.to_string();
    let mut ranges = Vec::new();
    for (index, byte) in line.bytes().enumerate() {
        if byte != b'=' {
            continue;
        }
        let key_start = line[..index]
            .rfind(|character: char| {
                character.is_whitespace() || character == ',' || character == ';'
            })
            .map_or(0, |position| position + 1);
        let key = line[key_start..index].to_ascii_lowercase();
        if !is_sensitive_key(&key) {
            continue;
        }
        let value_start = index + 1;
        let value_end = line[value_start..]
            .find(|character: char| {
                character.is_whitespace() || character == ',' || character == ';'
            })
            .map_or(line.len(), |position| value_start + position);
        ranges.push((value_start, value_end));
    }
    for (start, end) in ranges.into_iter().rev() {
        redacted.replace_range(start..end, "[REDACTED]");
    }

    redact_bearer_tokens(&mut redacted);
    redact_url_queries(&mut redacted);
    redacted
}

fn is_sensitive_key(key: &str) -> bool {
    key.contains("token")
        || key.contains("secret")
        || key.contains("password")
        || key.contains("passwd")
        || key.ends_with("key")
        || key.contains("api_key")
}

fn redact_bearer_tokens(text: &mut String) {
    let mut offset = 0;
    while offset < text.len() {
        let lowercase = text[offset..].to_ascii_lowercase();
        let Some(position) = lowercase.find("bearer ") else {
            break;
        };
        let start = offset + position + "bearer ".len();
        let end = text[start..]
            .find(char::is_whitespace)
            .map_or(text.len(), |position| start + position);
        text.replace_range(start..end, "[REDACTED]");
        offset = start + "[REDACTED]".len();
    }
}

fn redact_url_queries(text: &mut String) {
    let mut cursor = 0;
    while let Some(scheme) = text[cursor..].find("://") {
        let url_start = cursor + scheme;
        let url_end = text[url_start..]
            .find(char::is_whitespace)
            .map_or(text.len(), |position| url_start + position);
        let Some(query) = text[url_start..url_end].find('?') else {
            cursor = url_end;
            continue;
        };
        let query_start = url_start + query + 1;
        text.replace_range(query_start..url_end, "[REDACTED]");
        cursor = query_start + "[REDACTED]".len();
    }
}

fn truncate_utf8_tail(mut text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text.drain(..start);
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_redaction_handles_exact_values_headers_assignments_bearers_and_urls() {
        let raw = b"token=token-one Authorization: Bearer token-two\n\
Authorization: Basic token-three\n\
url=https://example.invalid/path?token=token-four\n\
exact-value\n";
        let redacted = redact_stderr_tail(raw, false, &["exact-value".into()], 1024);

        for secret in [
            "token-one",
            "token-two",
            "token-three",
            "token-four",
            "exact-value",
        ] {
            assert!(!redacted.contains(secret), "leaked {secret}: {redacted}");
        }
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn truncated_stderr_discards_unknown_partial_line() {
        let redacted =
            redact_stderr_tail(b"partial-secret-suffix\nsafe diagnostic\n", true, &[], 1024);
        assert_eq!(redacted, "safe diagnostic\n");

        let single_line = redact_stderr_tail(b"partial-secret-suffix", true, &[], 1024);
        assert_eq!(single_line, "[stderr line omitted after bounded capture]");
    }

    #[tokio::test]
    async fn capped_line_reader_rejects_oversized_input_without_buffering_the_remainder() {
        let input = b"123456789\nnext\n";
        let mut reader = BufReader::new(&input[..]);
        assert!(matches!(
            read_capped_line(&mut reader, 4).await.unwrap(),
            CappedLine::Oversized
        ));
        match read_capped_line(&mut reader, 4).await.unwrap() {
            CappedLine::Line(line) => assert_eq!(line, b"next"),
            _ => panic!("expected the next bounded line"),
        }
    }
}
