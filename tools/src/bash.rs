use std::ffi::OsString;
use std::fmt::Write as _;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::process::Stdio;

use orbcode_protocol::{
    FileChangeSummary, OutputTruncation, PermissionSummary, SandboxSummary, ToolResultMetadata,
};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::{Duration, Instant, sleep, timeout};

use crate::local_shell_task::{CreateLocalShellTask, LocalShellTaskRecord};
use crate::output::{TruncatedToolOutput, truncate_tool_output_with_metadata};
use crate::payload::{
    bool_field_keys, field_or_raw_keys, parse_payload, string_field_keys, usize_field_keys,
};
use crate::permissions::{ensure_not_cancelled, require_tools};
use crate::process::{configure_child_process, terminate_child_process};
use crate::progress::{bash_progress_payload, emit_tool_progress, read_child_stream};
use crate::{SandboxMode, ToolContext, ToolError, ToolOutcome, ToolRegistry};

/// Signal name recorded as the kill intent when a Bash run is cancelled.
/// `terminate_child_process` sends SIGTERM to the process group before
/// escalating, so that is the intent we persist for the registry record.
const BASH_KILL_SIGNAL: &str = "SIGTERM";
const BASH_KILL_REASON: &str = "interrupted by user";

const MAX_BASH_OUTPUT_CHARS: usize = 30_000;
const DEFAULT_BASH_TIMEOUT_MS: u64 = 120_000;
const MAX_BASH_TIMEOUT_MS: u64 = 600_000;
#[cfg(target_os = "macos")]
const MACOS_SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
#[cfg(target_os = "linux")]
pub(crate) const LINUX_BUBBLEWRAP: &str = "bwrap";
#[cfg(target_os = "windows")]
const WINDOWS_SANDBOX_RUNNER_ENV: &str = "ORBCODE_WINDOWS_SANDBOX_RUNNER";
#[cfg(target_os = "windows")]
const WINDOWS_SANDBOX_RUNNER: &str = "orbcode-windows-sandbox-runner";
const BASH_ESCALATED_SANDBOX_PERMISSION: &str = "require_escalated";

impl ToolRegistry {
    pub(crate) async fn run_bash(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        ensure_not_cancelled(context)?;
        let payload = parse_payload(input)?;
        let command = field_or_raw_keys(&payload, &["command", "cmd", "script"], input)?;
        let timeout_ms = bash_timeout_ms(&payload)?;
        let requests_sandbox_escalation = bash_payload_requests_sandbox_escalation(&payload);
        let pre_snapshot = capture_workspace_snapshot(&context.cwd).await;
        let started_at = Instant::now();

        // Register the run in the durable local-shell registry up front so the
        // task lifecycle (queued -> running -> terminal) and byte-addressable
        // output survive process exit and can be resumed by a fresh registry.
        let registry = context.local_shell_registry();
        let task = registry
            .create(CreateLocalShellTask {
                session_id: context.local_shell_session_id(),
                command: command.clone(),
                cwd: context.cwd.clone(),
                label: None,
            })
            .await?;
        let task_id = task.task_id;

        let probe_command =
            crate::bash_cwd::wrap_command_with_cwd_probe(&command, &shell_program());
        let mut process =
            bash_process(&probe_command, context, requests_sandbox_escalation).await?;
        configure_child_process(&mut process);
        let mut child = process.spawn()?;
        let pid = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::ExecutionFailed("bash stdout pipe was unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::ExecutionFailed("bash stderr pipe was unavailable".into()))?;
        registry.mark_running(&task_id, pid).await?;
        emit_tool_progress(
            context,
            bash_progress_payload("Running bash command", None, None, None),
        )
        .await?;

        // Readers only append to the registry; the terminal transitions below
        // run after both readers are joined, so the durable record is never
        // mutated concurrently with the per-task append lock.
        let stdout_task = tokio::spawn(read_child_stream(
            stdout,
            registry.clone(),
            task_id.clone(),
            true,
            context.progress.clone(),
            "stdout",
        ));
        let stderr_task = tokio::spawn(read_child_stream(
            stderr,
            registry.clone(),
            task_id.clone(),
            false,
            context.progress.clone(),
            "stderr",
        ));

        let (status, timed_out, interrupted) = tokio::select! {
            status = child.wait() => (Some(status?), false, false),
            _ = context.cancellation.cancelled() => {
                // Esc / TaskStop: record the process-local cancel intent, then
                // perform a real kill of the child (and its group).
                let _ = registry.request_cancel(&task_id).await;
                let status = terminate_child_process(&mut child).await;
                emit_tool_progress(
                    context,
                    bash_progress_payload("Bash command interrupted", None, None, None),
                )
                .await?;
                (status, false, true)
            }
            _ = sleep(Duration::from_millis(timeout_ms)) => {
                let status = terminate_child_process(&mut child).await;
                emit_tool_progress(
                    context,
                    bash_progress_payload("Bash command timed out", None, None, Some(143)),
                )
                .await?;
                (status, true, false)
            }
        };
        let raw_stdout = String::from_utf8_lossy(
            &stdout_task
                .await
                .map_err(ToolError::execution_failed_source)??,
        )
        .trim_end()
        .to_string();
        let stderr = String::from_utf8_lossy(
            &stderr_task
                .await
                .map_err(ToolError::execution_failed_source)??,
        )
        .trim_end()
        .to_string();

        let cwd_extraction =
            crate::bash_cwd::extract_cwd_from_output(&raw_stdout, &context.cwd).await;
        let stdout = cwd_extraction.stdout;
        let detected_cwd = cwd_extraction.new_cwd;
        if let Some(ref new_cwd) = detected_cwd {
            context.notify_cwd_change(new_cwd);
        }

        let exit_code = status.as_ref().and_then(ExitStatus::code);
        let signal = status.as_ref().and_then(exit_status_signal);
        let duration_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let post_snapshot = capture_workspace_snapshot(&context.cwd).await;
        let workspace_impact = workspace_impact_value(&pre_snapshot, &post_snapshot);
        let visible_output = if stdout.is_empty() {
            stderr.clone()
        } else {
            stdout.clone()
        };
        let truncated_output = truncate_tool_output_with_metadata(
            visible_output,
            MAX_BASH_OUTPUT_CHARS,
            "Bash output truncated for transcript safety. Re-run with a narrower command if you need the omitted portion.",
        );

        // Drive the registry to its terminal state. Readers are joined above, so
        // these are the only transitions racing nothing.
        let task_record = drive_terminal_transition(
            &registry,
            &task_id,
            TerminalOutcome {
                interrupted,
                timed_out,
                success: status.as_ref().is_some_and(ExitStatus::success),
                exit_code,
                signal,
                timeout_ms,
                stderr: &stderr,
            },
        )
        .await?;
        let record_path = registry.record_path_for(&task_id).display().to_string();

        let metadata = bash_result_metadata(BashResultMetadataInput {
            command: &command,
            context,
            timeout_ms,
            duration_ms,
            exit_code,
            signal,
            interrupted,
            timed_out,
            stdout: &stdout,
            stderr: &stderr,
            output: &truncated_output,
            requests_sandbox_escalation,
            workspace_impact: workspace_impact.as_ref(),
            task: &task_record,
            record_path: &record_path,
            new_cwd: detected_cwd.as_ref(),
        });

        if interrupted {
            return Err(ToolError::InterruptedWithMetadata { metadata });
        }

        if timed_out {
            let message = bash_timeout_message(&command, timeout_ms, &stdout, &stderr);
            return Err(ToolError::ExecutionFailedWithMetadata { message, metadata });
        }

        if !status.as_ref().is_some_and(ExitStatus::success) {
            emit_tool_progress(
                context,
                bash_progress_payload("Bash command failed", None, None, exit_code),
            )
            .await?;
            return Err(ToolError::ExecutionFailedWithMetadata {
                message: bash_failure_message(
                    &command,
                    status.as_ref(),
                    &stderr,
                    context,
                    requests_sandbox_escalation,
                ),
                metadata,
            });
        }
        emit_tool_progress(
            context,
            bash_progress_payload("Bash command completed", None, None, exit_code),
        )
        .await?;

        Ok(ToolOutcome {
            name: "bash".to_string(),
            summary: format!("Executed `{command}`."),
            output: truncated_output.output,
            metadata: Some(metadata),
            changed_paths: Vec::new(),
        })
    }
}

struct BashResultMetadataInput<'a> {
    command: &'a str,
    context: &'a ToolContext,
    timeout_ms: u64,
    duration_ms: u64,
    exit_code: Option<i32>,
    signal: Option<i32>,
    interrupted: bool,
    timed_out: bool,
    stdout: &'a str,
    stderr: &'a str,
    output: &'a TruncatedToolOutput,
    requests_sandbox_escalation: bool,
    workspace_impact: Option<&'a Value>,
    task: &'a LocalShellTaskRecord,
    record_path: &'a str,
    new_cwd: Option<&'a PathBuf>,
}

/// Outcome of a Bash run as observed by `run_bash`, used to drive the durable
/// registry to its matching terminal state once both output readers are joined.
struct TerminalOutcome<'a> {
    interrupted: bool,
    timed_out: bool,
    success: bool,
    exit_code: Option<i32>,
    signal: Option<i32>,
    timeout_ms: u64,
    stderr: &'a str,
}

/// Transition the registry record for `task_id` from `Running` to the terminal
/// state implied by `outcome`, and return the persisted record so the caller
/// can fold registry truth (task id, status, log/record paths) into the tool
/// result metadata. Interrupts pass through `Interrupting` first so the kill
/// signal/reason are recorded before the task is marked `Interrupted`.
async fn drive_terminal_transition(
    registry: &crate::local_shell_task::LocalShellTaskRegistry,
    task_id: &str,
    outcome: TerminalOutcome<'_>,
) -> Result<LocalShellTaskRecord, ToolError> {
    if outcome.interrupted {
        registry
            .mark_interrupting(task_id, BASH_KILL_SIGNAL, Some(BASH_KILL_REASON))
            .await?;
        return registry
            .mark_interrupted(
                task_id,
                outcome.exit_code,
                outcome.signal,
                Some(BASH_KILL_REASON.to_string()),
            )
            .await;
    }
    if outcome.timed_out {
        let reason = format!("timed out after {}ms", outcome.timeout_ms);
        return registry
            .mark_failed(task_id, outcome.exit_code, outcome.signal, Some(reason))
            .await;
    }
    if outcome.success {
        return registry
            .mark_succeeded(task_id, outcome.exit_code.unwrap_or(0))
            .await;
    }
    let reason = bash_terminal_failure_reason(outcome.exit_code, outcome.signal, outcome.stderr);
    registry
        .mark_failed(task_id, outcome.exit_code, outcome.signal, reason)
        .await
}

fn bash_terminal_failure_reason(
    exit_code: Option<i32>,
    signal: Option<i32>,
    stderr: &str,
) -> Option<String> {
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return Some(stderr.to_string());
    }
    if let Some(signal) = signal {
        return Some(format!("terminated by signal {signal}"));
    }
    exit_code.map(|code| format!("exited with code {code}"))
}

fn bash_timeout_ms(payload: &Value) -> Result<u64, ToolError> {
    let timeout = usize_field_keys(payload, &["timeout", "timeoutMs", "timeout_ms"])
        .map_or_else(default_bash_timeout_ms, |value| value as u64);
    if timeout == 0 {
        return Err(ToolError::InvalidInput(
            "bash timeout must be positive".into(),
        ));
    }
    let max_timeout = max_bash_timeout_ms();
    if timeout > max_timeout {
        eprintln!(
            "bash timeout {timeout}ms exceeds maximum {max_timeout}ms; clamping to {max_timeout}ms"
        );
        return Ok(max_timeout);
    }
    Ok(timeout)
}

fn default_bash_timeout_ms() -> u64 {
    positive_env_u64("BASH_DEFAULT_TIMEOUT_MS").unwrap_or(DEFAULT_BASH_TIMEOUT_MS)
}

fn max_bash_timeout_ms() -> u64 {
    positive_env_u64("BASH_MAX_TIMEOUT_MS")
        .unwrap_or(MAX_BASH_TIMEOUT_MS)
        .max(default_bash_timeout_ms())
}

fn positive_env_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
}

fn bash_result_metadata(input: BashResultMetadataInput<'_>) -> Value {
    let mut bash = json!({
        "command": input.command,
        "cwd": input.context.cwd.display().to_string(),
        "sandboxMode": input.context.sandbox_mode,
        "sandboxEscalated": input.requests_sandbox_escalation,
        "networkAllowed": input.context.sandbox_allow_network,
        "timeoutMs": input.timeout_ms,
        "durationMs": input.duration_ms,
        "exitCode": input.exit_code,
        "signal": input.signal,
        "interrupted": input.interrupted,
        "timedOut": input.timed_out,
        "stdoutChars": input.stdout.chars().count(),
        "stderrChars": input.stderr.chars().count(),
        "outputChars": input.output.original_chars,
        "outputTruncated": input.output.truncated,
        "omittedChars": input.output.omitted_chars,
        "taskId": input.task.task_id,
        "taskStatus": input.task.status.as_str(),
        "recordPath": input.record_path,
        "logPath": input.task.log_path,
        "outputBytes": input.task.output_bytes,
    });
    if let Some(map) = bash.as_object_mut() {
        if let Some(reason) = input
            .task
            .current_attempt
            .as_ref()
            .and_then(|attempt| attempt.kill_reason.clone())
        {
            map.insert("killReason".to_string(), Value::String(reason));
        }
        if let Some(signal) = input.task.kill_signal_sent.clone() {
            map.insert("killSignal".to_string(), Value::String(signal));
        }
        if let Some(impact) = input.workspace_impact {
            map.insert("workspaceImpact".to_string(), impact.clone());
        }
        if let Some(new_cwd) = input.new_cwd {
            map.insert(
                "newCwd".to_string(),
                Value::String(new_cwd.display().to_string()),
            );
        }
    }

    let unified = ToolResultMetadata {
        duration_ms: Some(input.duration_ms),
        truncation: Some(OutputTruncation {
            truncated: input.output.truncated,
            original_chars: Some(input.output.original_chars as u64),
            omitted_chars: Some(input.output.omitted_chars as u64),
        }),
        permissions: Some(PermissionSummary {
            tools_allowed: Some(input.context.allow_tools),
            network_allowed: Some(input.context.allow_network),
        }),
        sandbox: Some(SandboxSummary {
            mode: Some(input.context.sandbox_mode),
            network_allowed: Some(input.context.sandbox_allow_network),
            escalated: Some(input.requests_sandbox_escalation),
        }),
        file_changes: input.workspace_impact.map(|impact| FileChangeSummary {
            paths: Vec::new(),
            operation: Some("bash".to_string()),
            git: impact.get("git").cloned(),
        }),
        ..Default::default()
    };
    let mut metadata = unified.to_value();
    if let Some(map) = metadata.as_object_mut() {
        map.insert("bash".to_string(), bash);
    }
    metadata
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceSnapshot {
    git: Option<GitSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitSnapshot {
    branch: Option<String>,
    head: Option<String>,
    dirty_files: usize,
}

const WORKSPACE_SNAPSHOT_TIMEOUT_MS: u64 = 1_500;

pub(crate) async fn capture_workspace_snapshot(cwd: &Path) -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        git: capture_git_snapshot(cwd).await,
    }
}

async fn capture_git_snapshot(cwd: &Path) -> Option<GitSnapshot> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(cwd)
        .arg("status")
        .arg("--porcelain=v2")
        .arg("--branch")
        .arg("--untracked-files=all")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let run = async {
        let child = command.spawn().ok()?;
        let output = child.wait_with_output().await.ok()?;
        if !output.status.success() {
            return None;
        }
        Some(parse_git_status_porcelain(&String::from_utf8_lossy(
            &output.stdout,
        )))
    };

    timeout(Duration::from_millis(WORKSPACE_SNAPSHOT_TIMEOUT_MS), run)
        .await
        .unwrap_or_default()
}

fn parse_git_status_porcelain(text: &str) -> GitSnapshot {
    let mut branch = None;
    let mut head = None;
    let mut dirty_files = 0usize;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            let value = rest.trim();
            if !value.is_empty() && value != "(detached)" {
                branch = Some(value.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("# branch.oid ") {
            let value = rest.trim();
            if !value.is_empty() && value != "(initial)" {
                head = Some(value.to_string());
            }
        } else if line.starts_with('1')
            || line.starts_with('2')
            || line.starts_with('u')
            || line.starts_with('?')
            || line.starts_with('!')
        {
            dirty_files += 1;
        }
    }
    GitSnapshot {
        branch,
        head,
        dirty_files,
    }
}

fn workspace_impact_value(pre: &WorkspaceSnapshot, post: &WorkspaceSnapshot) -> Option<Value> {
    let git_impact = git_impact_value(pre.git.as_ref(), post.git.as_ref());
    git_impact.as_ref()?;
    let mut impact = serde_json::Map::new();
    if let Some(git) = git_impact {
        impact.insert("git".to_string(), git);
    }
    Some(Value::Object(impact))
}

fn git_impact_value(pre: Option<&GitSnapshot>, post: Option<&GitSnapshot>) -> Option<Value> {
    if pre.is_none() && post.is_none() {
        return None;
    }
    let became_repo = pre.is_none() && post.is_some();
    let lost_repo = pre.is_some() && post.is_none();
    let branch_changed = pre.and_then(|s| s.branch.clone()) != post.and_then(|s| s.branch.clone());
    let head_changed = pre.and_then(|s| s.head.clone()) != post.and_then(|s| s.head.clone());
    let pre_dirty = pre.map_or(0, |s| s.dirty_files);
    let post_dirty = post.map_or(0, |s| s.dirty_files);
    let working_tree_changed = pre_dirty != post_dirty || became_repo || lost_repo;

    if !branch_changed && !head_changed && !working_tree_changed && !became_repo && !lost_repo {
        return None;
    }

    let dirty_delta = post_dirty as i64 - pre_dirty as i64;
    let mut impact = serde_json::Map::new();
    impact.insert(
        "preBranch".to_string(),
        pre.and_then(|s| s.branch.clone())
            .map_or(Value::Null, Value::String),
    );
    impact.insert(
        "postBranch".to_string(),
        post.and_then(|s| s.branch.clone())
            .map_or(Value::Null, Value::String),
    );
    impact.insert("branchChanged".to_string(), Value::Bool(branch_changed));
    impact.insert(
        "preHead".to_string(),
        pre.and_then(|s| s.head.clone())
            .map_or(Value::Null, Value::String),
    );
    impact.insert(
        "postHead".to_string(),
        post.and_then(|s| s.head.clone())
            .map_or(Value::Null, Value::String),
    );
    impact.insert("headChanged".to_string(), Value::Bool(head_changed));
    impact.insert("preDirtyFiles".to_string(), Value::from(pre_dirty));
    impact.insert("postDirtyFiles".to_string(), Value::from(post_dirty));
    impact.insert("dirtyDelta".to_string(), Value::from(dirty_delta));
    impact.insert(
        "workingTreeChanged".to_string(),
        Value::Bool(working_tree_changed),
    );
    if became_repo {
        impact.insert("repoInitialized".to_string(), Value::Bool(true));
    }
    if lost_repo {
        impact.insert("repoRemoved".to_string(), Value::Bool(true));
    }
    Some(Value::Object(impact))
}

fn exit_status_signal(status: &ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        status.signal()
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

fn bash_timeout_message(command: &str, timeout_ms: u64, stdout: &str, stderr: &str) -> String {
    let output = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    if output.is_empty() {
        format!("command `{command}` timed out after {timeout_ms}ms")
    } else {
        format!("command `{command}` timed out after {timeout_ms}ms: {output}")
    }
}

fn bash_failure_message(
    command: &str,
    status: Option<&ExitStatus>,
    stderr: &str,
    context: &ToolContext,
    requests_sandbox_escalation: bool,
) -> String {
    let status = status.map_or_else(
        || "unknown status".to_string(),
        std::string::ToString::to_string,
    );
    let mut message = format!("command `{command}` exited with {status}: {stderr}");
    if bash_failure_likely_sandbox_denial(stderr, context, requests_sandbox_escalation) {
        message.push_str("\n\n");
        message.push_str(
            "The configured sandbox likely blocked this Bash command. Retry affordance: call Bash again with the same command and set `sandbox_permissions` to `require_escalated`. Orb Code will ask the user before running the retry outside the filesystem and network sandbox.",
        );
    }
    message
}

pub(crate) fn bash_failure_likely_sandbox_denial(
    stderr: &str,
    context: &ToolContext,
    requests_sandbox_escalation: bool,
) -> bool {
    if requests_sandbox_escalation || !context.sandbox_mode.is_restrictive() {
        return false;
    }
    if !context.sandbox_allow_network && stderr.trim().is_empty() {
        return true;
    }
    let stderr = stderr.to_ascii_lowercase();
    [
        "operation not permitted",
        "permission denied",
        "read-only file system",
        "not permitted",
        "sandbox",
        "denied",
    ]
    .iter()
    .any(|needle| stderr.contains(needle))
}

async fn bash_process(
    command: &str,
    context: &ToolContext,
    requests_sandbox_escalation: bool,
) -> Result<Command, ToolError> {
    match bash_effective_sandbox_mode(context.sandbox_mode, requests_sandbox_escalation) {
        SandboxMode::DangerFullAccess => {
            let mut process = shell_command(command);
            process.current_dir(&context.cwd);
            Ok(process)
        }
        SandboxMode::WorkspaceWrite | SandboxMode::ReadOnly => {
            sandboxed_bash_process(command, context).await
        }
        _ => sandboxed_bash_process(command, context).await,
    }
}

fn bash_effective_sandbox_mode(
    configured_mode: SandboxMode,
    requests_sandbox_escalation: bool,
) -> SandboxMode {
    if requests_sandbox_escalation && configured_mode.is_restrictive() {
        SandboxMode::DangerFullAccess
    } else {
        configured_mode
    }
}

pub fn bash_input_requests_sandbox_escalation(input: &str) -> bool {
    serde_json::from_str::<Value>(input)
        .ok()
        .is_some_and(|payload| bash_payload_requests_sandbox_escalation(&payload))
}

fn bash_payload_requests_sandbox_escalation(payload: &Value) -> bool {
    string_field_keys(payload, &["sandbox_permissions", "sandboxPermissions"])
        .is_some_and(|value| value.trim() == BASH_ESCALATED_SANDBOX_PERMISSION)
        || bool_field_keys(payload, &["dangerouslyDisableSandbox"]).unwrap_or(false)
}

#[cfg(target_os = "macos")]
async fn sandboxed_bash_process(
    command: &str,
    context: &ToolContext,
) -> Result<Command, ToolError> {
    if !tokio::fs::try_exists(MACOS_SANDBOX_EXEC)
        .await
        .unwrap_or(false)
    {
        return Err(ToolError::ExecutionFailed(format!(
            "sandbox mode `{}` requires `{MACOS_SANDBOX_EXEC}`, but it was not found",
            context.sandbox_mode.as_str()
        )));
    }

    let (profile, params) = macos_seatbelt_profile(
        context.sandbox_mode,
        &context.cwd,
        &context.additional_directories,
        context.sandbox_allow_network,
    )
    .await?;
    let mut process = Command::new(MACOS_SANDBOX_EXEC);
    process.arg("-p").arg(profile);
    for (name, path) in params {
        process.arg(format!("-D{name}={}", path.display()));
    }
    process
        .arg("--")
        .arg(shell_program())
        .arg("-lc")
        .arg(command)
        .current_dir(&context.cwd)
        .env("ORBCODE_SANDBOX", "seatbelt");
    if !context.sandbox_allow_network {
        process.env("ORBCODE_SANDBOX_NETWORK_DISABLED", "1");
    }
    Ok(process)
}

#[cfg(target_os = "linux")]
async fn sandboxed_bash_process(
    command: &str,
    context: &ToolContext,
) -> Result<Command, ToolError> {
    let Some(binary) = executable_in_path(LINUX_BUBBLEWRAP).await else {
        return Err(ToolError::ExecutionFailed(format!(
            "sandbox mode `{}` requires Linux bubblewrap (`{LINUX_BUBBLEWRAP}`), but it was not found in PATH",
            context.sandbox_mode.as_str()
        )));
    };

    let mut process = Command::new(binary);
    process
        .args(
            linux_bubblewrap_args(
                command,
                context.sandbox_mode,
                &context.cwd,
                &context.additional_directories,
                context.sandbox_allow_network,
            )
            .await?,
        )
        .current_dir(&context.cwd);
    Ok(process)
}

#[cfg(target_os = "windows")]
async fn sandboxed_bash_process(
    command: &str,
    context: &ToolContext,
) -> Result<Command, ToolError> {
    let Some(runner) = windows_sandbox_runner_path().await else {
        return Err(ToolError::ExecutionFailed(format!(
            "sandbox mode `{}` requires `{WINDOWS_SANDBOX_RUNNER}` on PATH or {WINDOWS_SANDBOX_RUNNER_ENV} pointing to a Windows sandbox runner",
            context.sandbox_mode.as_str()
        )));
    };

    let mut process = Command::new(runner);
    process
        .args(
            windows_sandbox_runner_args(
                command,
                context.sandbox_mode,
                &context.cwd,
                &context.additional_directories,
                context.sandbox_allow_network,
            )
            .await?,
        )
        .current_dir(&context.cwd)
        .env("ORBCODE_SANDBOX", "windows-runner");
    if !context.sandbox_allow_network {
        process.env("ORBCODE_SANDBOX_NETWORK_DISABLED", "1");
    }
    Ok(process)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
async fn sandboxed_bash_process(
    _command: &str,
    context: &ToolContext,
) -> Result<Command, ToolError> {
    Err(ToolError::ExecutionFailed(format!(
        "sandbox mode `{}` is configured, but Orb Code only enforces Bash sandboxing on macOS seatbelt, Linux bubblewrap, and the Windows sandbox runner backend right now",
        context.sandbox_mode.as_str()
    )))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) async fn linux_bubblewrap_args(
    command: &str,
    sandbox_mode: SandboxMode,
    cwd: &Path,
    additional_directories: &[PathBuf],
    allow_network: bool,
) -> Result<Vec<OsString>, ToolError> {
    let cwd = canonical_sandbox_path(cwd).await?;
    let additional_directories = canonical_additional_sandbox_paths(additional_directories).await?;
    let mut args = os_strings([
        "--die-with-parent",
        "--unshare-all",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
    ]);
    if allow_network {
        args.push("--share-net".into());
    }
    if matches!(sandbox_mode, SandboxMode::WorkspaceWrite) {
        args.push("--bind".into());
        args.push(cwd.as_os_str().to_owned());
        args.push(cwd.as_os_str().to_owned());
        for dir in &additional_directories {
            args.push("--bind".into());
            args.push(dir.as_os_str().to_owned());
            args.push(dir.as_os_str().to_owned());
        }
    }
    args.extend(os_strings(["--chdir"]));
    args.push(cwd.as_os_str().to_owned());
    args.extend(os_strings(["--setenv", "ORBCODE_SANDBOX", "bubblewrap"]));
    if !allow_network {
        args.extend(os_strings([
            "--setenv",
            "ORBCODE_SANDBOX_NETWORK_DISABLED",
            "1",
        ]));
    }
    args.push("--".into());
    let (shell, shell_args) = shell_invocation(command);
    args.push(shell.into());
    args.extend(shell_args);
    Ok(args)
}

#[cfg(target_os = "windows")]
pub(crate) async fn windows_sandbox_runner_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(WINDOWS_SANDBOX_RUNNER_ENV).map(PathBuf::from) {
        if is_executable_file(&path).await {
            return Some(path);
        }
    }
    executable_in_path(WINDOWS_SANDBOX_RUNNER).await
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) async fn windows_sandbox_runner_args(
    command: &str,
    sandbox_mode: SandboxMode,
    cwd: &Path,
    additional_directories: &[PathBuf],
    allow_network: bool,
) -> Result<Vec<OsString>, ToolError> {
    let cwd = canonical_sandbox_path(cwd).await?;
    let additional_directories = canonical_additional_sandbox_paths(additional_directories).await?;
    let mut args = os_strings(["--mode", sandbox_mode.as_str(), "--cwd"]);
    args.push(cwd.as_os_str().to_owned());
    for dir in &additional_directories {
        args.push("--add-dir".into());
        args.push(dir.as_os_str().to_owned());
    }
    args.extend(os_strings([
        "--network",
        if allow_network { "allow" } else { "deny" },
        "--",
    ]));
    let (shell, shell_args) = shell_invocation(command);
    args.push(shell.into());
    args.extend(shell_args);
    Ok(args)
}

#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
fn os_strings<const N: usize>(values: [&str; N]) -> Vec<OsString> {
    values.into_iter().map(OsString::from).collect()
}

#[cfg(target_os = "macos")]
pub(crate) async fn macos_seatbelt_profile(
    sandbox_mode: SandboxMode,
    cwd: &Path,
    additional_directories: &[PathBuf],
    allow_network: bool,
) -> Result<(String, Vec<(String, PathBuf)>), ToolError> {
    let mut profile = String::from(
        r#"(version 1)
(deny default)
(allow process*)
(allow signal (target same-sandbox))
(allow file-read*)
(allow file-write-data
  (require-all
    (literal "/dev/null")
    (vnode-type CHARACTER-DEVICE)))
(allow sysctl-read)
(allow mach-lookup)
(allow ipc-posix-sem)
(allow ipc-posix-shm*)
(allow user-preference-read)
"#,
    );
    let mut params = Vec::new();
    if allow_network {
        profile.push_str("(allow network*)\n");
    }
    if matches!(sandbox_mode, SandboxMode::WorkspaceWrite) {
        profile.push_str(r#"(allow file-write* (subpath (param "ORBCODE_WORKSPACE_WRITE_ROOT")))"#);
        profile.push('\n');
        params.push((
            "ORBCODE_WORKSPACE_WRITE_ROOT".to_string(),
            canonical_sandbox_path(cwd).await?,
        ));
        for (index, dir) in canonical_additional_sandbox_paths(additional_directories)
            .await?
            .into_iter()
            .enumerate()
        {
            let name = format!("ORBCODE_WORKSPACE_WRITE_DIR_{index}");
            write!(profile, r#"(allow file-write* (subpath (param "{name}")))"#)
                .expect("writing to String cannot fail");
            profile.push('\n');
            params.push((name, dir));
        }
    }
    Ok((profile, params))
}

async fn canonical_additional_sandbox_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>, ToolError> {
    let mut canonical = Vec::new();
    for path in paths {
        let path = canonical_sandbox_path(path).await?;
        if !canonical.iter().any(|existing| existing == &path) {
            canonical.push(path);
        }
    }
    Ok(canonical)
}

async fn canonical_sandbox_path(path: &Path) -> Result<PathBuf, ToolError> {
    tokio::fs::canonicalize(path).await.map_err(|error| {
        ToolError::ExecutionFailed(format!(
            "failed to canonicalize sandbox path `{}`: {error}",
            path.display()
        ))
    })
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) async fn executable_in_path(binary: &str) -> Option<PathBuf> {
    let candidate = Path::new(binary);
    if candidate.components().count() > 1 && is_executable_file(candidate).await {
        return Some(candidate.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let path = dir.join(binary);
        if is_executable_file(&path).await {
            return Some(path);
        }
    }
    None
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
async fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        return tokio::fs::metadata(path)
            .await
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }

    #[cfg(not(unix))]
    {
        tokio::fs::metadata(path)
            .await
            .map(|metadata| metadata.is_file())
            .unwrap_or(false)
    }
}

#[cfg(all(test, not(any(target_os = "linux", target_os = "windows"))))]
#[allow(dead_code)]
async fn is_executable_file(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
}

#[cfg(all(test, not(any(target_os = "linux", target_os = "windows"))))]
#[allow(dead_code)]
pub(crate) async fn executable_in_path(binary: &str) -> Option<PathBuf> {
    let candidate = Path::new(binary);
    if candidate.components().count() > 1 && is_executable_file(candidate).await {
        return Some(candidate.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let path = dir.join(binary);
        if is_executable_file(&path).await {
            return Some(path);
        }
    }
    None
}

fn shell_command(command: &str) -> Command {
    let (shell, args) = shell_invocation(command);
    let mut process = Command::new(shell);
    process.args(args);
    process
}

fn shell_invocation(command: &str) -> (String, Vec<OsString>) {
    (shell_program(), shell_command_args(command))
}

#[cfg(not(target_os = "windows"))]
fn shell_command_args(command: &str) -> Vec<OsString> {
    vec!["-lc".into(), command.into()]
}

#[cfg(target_os = "windows")]
fn shell_command_args(command: &str) -> Vec<OsString> {
    vec![
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-Command".into(),
        command.into(),
    ]
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn shell_program() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "zsh".to_string())
}

#[cfg(target_os = "windows")]
pub(crate) fn shell_program() -> String {
    std::env::var("POWERSHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "powershell.exe".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_defaults_to_120s() {
        let payload = json!({});
        let ms = bash_timeout_ms(&payload).unwrap();
        assert_eq!(ms, DEFAULT_BASH_TIMEOUT_MS);
        assert_eq!(ms, 120_000);
    }

    #[test]
    fn timeout_zero_is_rejected() {
        let payload = json!({"timeout": 0});
        let err = bash_timeout_ms(&payload).unwrap_err();
        assert!(
            err.to_string().contains("positive"),
            "expected 'positive' in error: {err}"
        );
    }

    #[test]
    fn timeout_within_max_accepted() {
        let payload = json!({"timeout": 300_000});
        let ms = bash_timeout_ms(&payload).unwrap();
        assert_eq!(ms, 300_000);
    }

    #[test]
    fn timeout_exceeding_max_is_clamped() {
        let payload = json!({"timeout": 999_999});
        let ms = bash_timeout_ms(&payload).unwrap();
        assert_eq!(ms, MAX_BASH_TIMEOUT_MS);
        assert_eq!(ms, 600_000);
    }

    #[test]
    fn timeout_at_max_boundary_accepted() {
        let payload = json!({"timeout": 600_000});
        let ms = bash_timeout_ms(&payload).unwrap();
        assert_eq!(ms, 600_000);
    }
}
