use std::{
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

use orbcode_config::HookCommand;
use serde_json::Value;
use tokio::{io::AsyncWriteExt, process::Command};

use crate::{
    CoreError,
    hooks::{HookCommandRunStatus, command_hook_output_status, hook_shell_program},
};

const DEFAULT_HOOK_TIMEOUT_MS: u64 = 60_000;

#[derive(Clone, Debug)]
pub(crate) struct HookCommandProgress {
    pub(crate) event_name: &'static str,
    pub(crate) command: String,
    pub(crate) status: &'static str,
    pub(crate) exit_code: Option<i32>,
    pub(crate) error: Option<String>,
    pub(crate) elapsed: Duration,
}

pub(crate) fn command_hook_parts(hook: &HookCommand) -> Option<(&str, Option<&str>, Option<f64>)> {
    let HookCommand::Command {
        command,
        r#if,
        timeout,
    } = hook
    else {
        return None;
    };
    Some((command.as_str(), r#if.as_deref(), *timeout))
}

pub(crate) fn progress(
    event_name: &'static str,
    command: &str,
    status: &'static str,
    exit_code: Option<i32>,
    error: Option<&str>,
    started: Instant,
) -> HookCommandProgress {
    HookCommandProgress {
        event_name,
        command: command.to_string(),
        status,
        exit_code,
        error: error.map(str::to_string),
        elapsed: started.elapsed(),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum HookCommandErrorContext {
    Generic,
    Event(&'static str),
}

fn hook_timeout(timeout_seconds: Option<f64>) -> Duration {
    let timeout_ms = timeout_seconds
        .filter(|value| value.is_finite() && *value > 0.0)
        .map_or(DEFAULT_HOOK_TIMEOUT_MS, |value| (value * 1000.0) as u64);
    Duration::from_millis(timeout_ms)
}

fn start_error(context: HookCommandErrorContext, error: &std::io::Error) -> CoreError {
    match context {
        HookCommandErrorContext::Generic => {
            CoreError::Tool(format!("failed to start hook: {error}"))
        }
        HookCommandErrorContext::Event(event) => {
            CoreError::Tool(format!("failed to start {event} hook: {error}"))
        }
    }
}

fn run_error(context: HookCommandErrorContext, command: &str, error: &std::io::Error) -> CoreError {
    match context {
        HookCommandErrorContext::Generic => {
            CoreError::Tool(format!("failed to run hook `{command}`: {error}"))
        }
        HookCommandErrorContext::Event(event) => {
            CoreError::Tool(format!("failed to run {event} hook `{command}`: {error}"))
        }
    }
}

pub(crate) async fn run_command_hook_capture(
    cwd: &Path,
    hook_input: &Value,
    command: &str,
    timeout_seconds: Option<f64>,
    error_context: HookCommandErrorContext,
) -> Result<Option<std::process::Output>, CoreError> {
    let mut process = Command::new(hook_shell_program());
    process
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process.kill_on_drop(true);
    let mut child = process
        .spawn()
        .map_err(|error| start_error(error_context, &error))?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = serde_json::to_vec(hook_input)
            .map_err(|error| CoreError::Tool(format!("failed to encode hook input: {error}")))?;
        // Best-effort stdin writer; hook completion is observed through `wait_with_output`.
        let _stdin_writer_handle = tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
        });
    }
    match tokio::time::timeout(hook_timeout(timeout_seconds), child.wait_with_output()).await {
        Ok(result) => result
            .map(Some)
            .map_err(|error| run_error(error_context, command, &error)),
        Err(_) => Ok(None),
    }
}

pub(crate) async fn run_nonblocking_command_hook(
    cwd: &Path,
    hook_input: &Value,
    command: &str,
    timeout_seconds: Option<f64>,
) -> HookCommandRunStatus {
    let mut process = Command::new(hook_shell_program());
    process
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process.kill_on_drop(true);
    let Ok(mut child) = process.spawn() else {
        return HookCommandRunStatus {
            status: "failed",
            exit_code: None,
            error: None,
        };
    };
    if let Some(mut stdin) = child.stdin.take()
        && let Ok(input) = serde_json::to_vec(hook_input)
    {
        // Best-effort stdin writer for validation; process completion is awaited below.
        let _stdin_writer_handle = tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
        });
    }
    match tokio::time::timeout(hook_timeout(timeout_seconds), child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let (status, exit_code) = command_hook_output_status(&output);
            HookCommandRunStatus {
                status,
                exit_code,
                error: None,
            }
        }
        Ok(Err(_)) => HookCommandRunStatus {
            status: "failed",
            exit_code: None,
            error: None,
        },
        Err(_) => HookCommandRunStatus {
            status: "timed_out",
            exit_code: None,
            error: None,
        },
    }
}
