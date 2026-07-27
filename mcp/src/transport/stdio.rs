use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::McpError;
use crate::wire::{StdioInitializeResult, StdioListToolsResult, StdioToolCallResult};

pub(crate) const STDIO_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const STDIO_STARTUP_TIMEOUT: Duration = STDIO_REQUEST_TIMEOUT;
const STDIO_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const STDERR_CAPTURE_LIMIT: usize = 8 * 1024;

pub struct StdioMcpClient {
    child: Option<Child>,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: BufReader<tokio::process::ChildStdout>,
    stderr: Arc<Mutex<String>>,
    stderr_task: Option<JoinHandle<()>>,
    next_id: u64,
    request_timeout: Duration,
}

impl fmt::Debug for StdioMcpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StdioMcpClient")
            .field("has_child", &self.child.is_some())
            .field("has_stdin", &self.stdin.is_some())
            .field("has_stderr_task", &self.stderr_task.is_some())
            .field("next_id", &self.next_id)
            .field("request_timeout", &self.request_timeout)
            .finish_non_exhaustive()
    }
}

impl StdioMcpClient {
    pub async fn spawn(
        command: impl AsRef<OsStr>,
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
        request_timeout: Duration,
    ) -> Result<Self, McpError> {
        let mut command = Command::new(command);
        command.args(args);
        Self::spawn_command(command, request_timeout).await
    }

    pub async fn spawn_configured(
        command: impl AsRef<OsStr>,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: Option<&str>,
        request_timeout: Duration,
    ) -> Result<Self, McpError> {
        let mut command = Command::new(command);
        command.args(args);
        command.envs(env);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        Self::spawn_command(command, request_timeout).await
    }

    #[allow(clippy::unused_async)] // Kept async to match the public stdio startup path.
    async fn spawn_command(
        mut command: Command,
        request_timeout: Duration,
    ) -> Result<Self, McpError> {
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Protocol("stdio server stdin was not piped".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Protocol("stdio server stdout was not piped".to_string()))?;
        let stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| McpError::Protocol("stdio server stderr was not piped".to_string()))?;
        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_task = Some(spawn_stderr_capture(stderr_pipe, stderr.clone()));

        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            stderr,
            stderr_task,
            next_id: 1,
            request_timeout,
        })
    }

    pub async fn initialize(&mut self) -> Result<StdioInitializeResult, McpError> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "orbcode",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
        )
        .await
    }

    pub async fn list_tools(&mut self) -> Result<StdioListToolsResult, McpError> {
        self.request("tools/list", json!({})).await
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<StdioToolCallResult, McpError> {
        self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
            }),
        )
        .await
    }

    pub async fn request<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<T, McpError> {
        let id = self.next_id;
        self.next_id += 1;

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let payload = serde_json::to_string(&request)?;

        let mut line = String::new();
        let response = timeout(self.request_timeout, async {
            let stdin = self.stdin.as_mut().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "stdio server stdin is closed",
                )
            })?;
            stdin.write_all(payload.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
            // Read until the reply for *this* id arrives, discarding a stale
            // response left buffered by an earlier request that timed out. Its
            // id is below the current one; without skipping it, it would be
            // mis-read as this request's reply, fail the id check, and leave the
            // stream permanently one response behind.
            loop {
                line.clear();
                let bytes_read = self.stdout.read_line(&mut line).await?;
                if bytes_read == 0 {
                    return Ok::<usize, std::io::Error>(0);
                }
                let is_stale = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|value| value.get("id").and_then(Value::as_u64))
                    .is_some_and(|resp_id| resp_id < id);
                if !is_stale {
                    return Ok(bytes_read);
                }
            }
        })
        .await;
        let bytes_read = match response {
            Ok(Ok(bytes_read)) => bytes_read,
            Ok(Err(error)) => {
                return Err(self
                    .protocol_error_with_stderr(format!("stdio request {method} failed: {error}"))
                    .await);
            }
            Err(_) => {
                if let Some(status) = self.child_status()? {
                    return Err(McpError::Protocol(
                        self.message_with_stderr(format!(
                            "stdio server exited before responding to {method}: {status}"
                        ))
                        .await,
                    ));
                }
                return Err(McpError::Timeout(method.to_string()));
            }
        };

        if bytes_read == 0 {
            let base = if let Some(status) = self.child_status()? {
                format!("stdio server exited before responding to {method}: {status}")
            } else {
                format!("stdio server closed stdout before responding to {method}")
            };
            return Err(self.protocol_error_with_stderr(base).await);
        }

        let response: Value = serde_json::from_str(&line)?;
        if response.get("jsonrpc") != Some(&Value::String("2.0".to_string())) {
            return Err(McpError::Protocol(format!(
                "invalid JSON-RPC version in {method} response"
            )));
        }
        if response.get("id") != Some(&json!(id)) {
            return Err(McpError::Protocol(format!(
                "mismatched JSON-RPC id in {method} response"
            )));
        }
        if let Some(error) = response.get("error") {
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown JSON-RPC error")
                .to_string();
            return Err(McpError::JsonRpc { code, message });
        }

        let result = response
            .get("result")
            .cloned()
            .ok_or_else(|| McpError::Protocol(format!("missing result in {method} response")))?;
        Ok(serde_json::from_value(result)?)
    }

    pub async fn shutdown(&mut self) -> Result<(), McpError> {
        self.stdin.take();
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };

        let wait = timeout(STDIO_SHUTDOWN_TIMEOUT, child.wait()).await;
        match wait {
            Ok(result) => {
                result?;
            }
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }

        if let Some(stderr_task) = self.stderr_task.take() {
            let _ = timeout(Duration::from_millis(200), stderr_task).await;
        }
        Ok(())
    }

    async fn protocol_error_with_stderr(&self, base: String) -> McpError {
        McpError::Protocol(self.message_with_stderr(base).await)
    }

    pub(crate) async fn message_with_stderr(&self, base: String) -> String {
        let stderr = self.stderr_snapshot().await;
        if stderr.is_empty() {
            base
        } else {
            format!("{base}; stderr: {stderr}")
        }
    }

    pub async fn stderr_snapshot(&self) -> String {
        self.stderr.lock().await.trim().to_string()
    }

    fn child_status(&mut self) -> Result<Option<std::process::ExitStatus>, McpError> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        Ok(child.try_wait()?)
    }
}

impl Drop for StdioMcpClient {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.start_kill();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = child.wait().await;
            });
        }
    }
}

fn spawn_stderr_capture(
    mut stderr_pipe: tokio::process::ChildStderr,
    stderr: Arc<Mutex<String>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = [0_u8; 1024];
        loop {
            let bytes_read = match stderr_pipe.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(bytes_read) => bytes_read,
            };
            let chunk = String::from_utf8_lossy(&buffer[..bytes_read]);
            let mut captured = stderr.lock().await;
            captured.push_str(&chunk);
            if captured.len() > STDERR_CAPTURE_LIMIT {
                let mut start = captured.len() - STDERR_CAPTURE_LIMIT;
                while !captured.is_char_boundary(start) {
                    start += 1;
                }
                *captured = captured[start..].to_string();
            }
        }
    })
}
