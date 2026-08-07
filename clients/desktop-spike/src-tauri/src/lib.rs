use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use orbcode_app_server_protocol::{
    ClientMessage, InitializeParams, InitializeResult, ResponseResult, ServerCapabilities,
    ServerInfo, ServerMessage, ServerResponseEnvelope,
};
use serde::{Deserialize, Serialize};

pub const INTERNAL_CHILD_FLAG: &str = "--desktop-spike-protocol-child";
const MAX_ENVELOPE_BYTES: usize = 64 * 1024;
const MAX_STDERR_TAIL_BYTES: usize = 8 * 1024;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct ProbeChild(PathBuf);

impl ProbeChild {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn current_executable() -> Result<Self, String> {
        std::env::current_exe()
            .map(Self)
            .map_err(|error| format!("resolve desktop spike executable: {error}"))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeTermination {
    Graceful,
    KilledAfterTimeout,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProbeResult {
    pub response: String,
    pub child_pid: u32,
    pub exit_code: Option<i32>,
    pub termination: ProbeTermination,
    pub stderr_tail: String,
}

struct ChildGuard {
    child: Child,
    reaped: bool,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self {
            child,
            reaped: false,
        }
    }

    fn mark_reaped(&mut self) {
        self.reaped = true;
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub fn configure_builder<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
    child: ProbeChild,
) -> tauri::Builder<R> {
    builder
        .manage(child)
        .invoke_handler(tauri::generate_handler![run_initialize_probe])
}

#[tauri::command]
fn run_initialize_probe(
    child: tauri::State<'_, ProbeChild>,
    request: String,
) -> Result<ProbeResult, String> {
    relay_once(&child.0, &request)
}

pub fn navigation_guard<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("packaged-navigation-only")
        .on_navigation(|_webview, url| {
            url.scheme() == "tauri"
                || (matches!(url.scheme(), "http" | "https")
                    && url.host_str() == Some("tauri.localhost"))
        })
        .build()
}

pub fn relay_once(child_executable: &Path, request: &str) -> Result<ProbeResult, String> {
    validate_envelope(request)?;

    let child = Command::new(child_executable)
        .arg(INTERNAL_CHILD_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn fixed protocol test child: {error}"))?;
    let mut child = ChildGuard::new(child);
    let child_pid = child.child.id();

    let mut stdin = child
        .child
        .stdin
        .take()
        .ok_or_else(|| "protocol test child stdin was not piped".to_string())?;
    let stdout = child
        .child
        .stdout
        .take()
        .ok_or_else(|| "protocol test child stdout was not piped".to_string())?;
    let stderr = child
        .child
        .stderr
        .take()
        .ok_or_else(|| "protocol test child stderr was not piped".to_string())?;

    let (response_tx, response_rx) = mpsc::sync_channel(1);
    let stdout_thread = thread::spawn(move || {
        let _ = response_tx.send(read_capped_line(stdout, MAX_ENVELOPE_BYTES));
    });
    let stderr_thread = thread::spawn(move || read_bounded_tail(stderr, MAX_STDERR_TAIL_BYTES));

    let response_result = (|| {
        stdin
            .write_all(request.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("write initialize envelope to child: {error}"))?;

        response_rx
            .recv_timeout(RESPONSE_TIMEOUT)
            .map_err(|_| "timed out waiting for protocol test child response".to_string())?
    })();

    // EOF is the graceful shutdown signal for the stdio server. This drop is
    // unconditional so error paths cannot leave the child waiting for input.
    drop(stdin);
    let (status, termination) = wait_or_kill(&mut child.child, SHUTDOWN_TIMEOUT)?;
    child.mark_reaped();

    stdout_thread
        .join()
        .map_err(|_| "protocol test child stdout reader panicked".to_string())?;
    let stderr_tail = stderr_thread
        .join()
        .map_err(|_| "protocol test child stderr reader panicked".to_string())?;

    Ok(ProbeResult {
        response: response_result?,
        child_pid,
        exit_code: status.code(),
        termination,
        stderr_tail,
    })
}

fn validate_envelope(request: &str) -> Result<(), String> {
    if request.is_empty() {
        return Err("protocol envelope must not be empty".to_string());
    }
    if request.len() > MAX_ENVELOPE_BYTES {
        return Err(format!(
            "protocol envelope exceeds {MAX_ENVELOPE_BYTES} byte spike limit"
        ));
    }
    if request.contains(['\r', '\n']) {
        return Err("protocol envelope must contain exactly one NDJSON record".to_string());
    }
    serde_json::from_str::<serde_json::Value>(request)
        .map(|_| ())
        .map_err(|error| format!("protocol envelope is not valid JSON: {error}"))
}

fn read_capped_line(reader: impl Read, max_bytes: usize) -> Result<String, String> {
    let mut bytes = Vec::new();
    let read = BufReader::new(reader)
        .take((max_bytes + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| format!("read protocol test child response: {error}"))?;
    if read == 0 {
        return Err("protocol test child exited before responding".to_string());
    }
    if bytes.len() > max_bytes {
        return Err(format!(
            "protocol test child response exceeds {max_bytes} byte spike limit"
        ));
    }
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    String::from_utf8(bytes)
        .map_err(|error| format!("protocol test child response was not UTF-8: {error}"))
}

fn read_bounded_tail(mut reader: impl Read, max_bytes: usize) -> String {
    let mut tail = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                tail.extend_from_slice(&buffer[..read]);
                if tail.len() > max_bytes {
                    tail.drain(..tail.len() - max_bytes);
                }
            }
        }
    }
    String::from_utf8_lossy(&tail).into_owned()
}

fn wait_or_kill(
    child: &mut Child,
    timeout: Duration,
) -> Result<(ExitStatus, ProbeTermination), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("poll protocol test child: {error}"))?
        {
            return Ok((status, ProbeTermination::Graceful));
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    child
        .kill()
        .map_err(|error| format!("kill unresponsive protocol test child: {error}"))?;
    let status = child
        .wait()
        .map_err(|error| format!("reap killed protocol test child: {error}"))?;
    Ok((status, ProbeTermination::KilledAfterTimeout))
}

pub fn run_protocol_test_child() -> Result<(), String> {
    let request_line = read_capped_line(std::io::stdin().lock(), MAX_ENVELOPE_BYTES)?;
    let request: ClientMessage = serde_json::from_str(&request_line)
        .map_err(|error| format!("decode canonical client envelope: {error}"))?;

    let ClientMessage::Request(request) = request else {
        return Err("expected canonical request envelope".to_string());
    };
    if request.method != "initialize" {
        return Err(format!("expected initialize, received {}", request.method));
    }
    let params: InitializeParams = serde_json::from_value(
        request
            .params
            .ok_or_else(|| "initialize params are required".to_string())?,
    )
    .map_err(|error| format!("decode canonical initialize params: {error}"))?;
    if params.protocol_version != "1.0" {
        return Err(format!(
            "unsupported probe protocol version {}",
            params.protocol_version
        ));
    }

    let initialized = InitializeResult {
        protocol_version: "1.0".to_string(),
        server_info: ServerInfo {
            name: "orbcode-desktop-spike-child".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        capabilities: ServerCapabilities {
            streaming: true,
            stable_methods: vec!["initialize".to_string()],
            experimental_methods: Vec::new(),
            server_notification_methods: Vec::new(),
            server_request_methods: Vec::new(),
        },
    };
    let response = ServerMessage::Response(ServerResponseEnvelope {
        id: request.id,
        result: ResponseResult::Success {
            data: Some(
                serde_json::to_value(initialized)
                    .map_err(|error| format!("encode initialize result: {error}"))?,
            ),
        },
    });
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &response)
        .map_err(|error| format!("encode canonical server envelope: {error}"))?;
    stdout
        .write_all(b"\n")
        .and_then(|_| stdout.flush())
        .map_err(|error| format!("write canonical server envelope: {error}"))?;

    // Remain alive until the host closes stdin. A passing host test therefore
    // proves EOF delivery and successful wait/reap, not merely a one-shot exit.
    let mut drain = Vec::new();
    std::io::stdin()
        .read_to_end(&mut drain)
        .map_err(|error| format!("wait for host stdin EOF: {error}"))?;
    Ok(())
}
