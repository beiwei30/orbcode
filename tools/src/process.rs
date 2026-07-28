use std::io;
use std::process::{ExitStatus, Output, Stdio};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::{ToolContext, ToolError, permissions::ensure_not_cancelled};

pub(crate) async fn run_command_output(
    command: &mut Command,
    context: &ToolContext,
) -> Result<Output, ToolError> {
    ensure_not_cancelled(context)?;
    configure_child_process(command);
    let mut child = command.spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::ExecutionFailed("child stdout pipe was unavailable".into()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::ExecutionFailed("child stderr pipe was unavailable".into()))?;

    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await?;
        Ok::<_, io::Error>(bytes)
    });
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await?;
        Ok::<_, io::Error>(bytes)
    });

    let status = tokio::select! {
        status = child.wait() => status?,
        _ = context.cancellation.cancelled() => {
            terminate_child_process(&mut child).await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(ToolError::Interrupted);
        }
    };

    let stdout = stdout_task
        .await
        .map_err(ToolError::execution_failed_source)??;
    let stderr = stderr_task
        .await
        .map_err(ToolError::execution_failed_source)??;

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

pub(crate) fn configure_child_process(command: &mut Command) {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_child_process_group(command);
}

#[cfg(unix)]
fn configure_child_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_child_process_group(_command: &mut Command) {}

pub(crate) async fn terminate_child_process(
    child: &mut tokio::process::Child,
) -> Option<ExitStatus> {
    if let Some(pid) = child.id() {
        terminate_process_group(pid).await;
    }
    let _ = child.kill().await;
    child.wait().await.ok()
}

#[cfg(unix)]
async fn terminate_process_group(pid: u32) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg("--")
        .arg(format!("-{pid}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

#[cfg(not(unix))]
async fn terminate_process_group(_pid: u32) {}
