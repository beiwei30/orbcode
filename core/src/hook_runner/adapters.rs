use orbcode_config::HookSource;
use orbcode_session_store::deserialize_block_payload;
use serde_json::{Value, json};

use super::HookCommandContext;
use super::command::{
    HookCommandErrorContext, run_command_hook_capture, run_nonblocking_command_hook,
};
use crate::{
    CoreError,
    hooks::{
        HookAdditionalContextCommandRun, HookCommandRunStatus, PermissionDeniedHookCommandRun,
        PreToolHookCommandResult, StopHookCommandRun, StopHookOutcome,
        UserPromptSubmitHookCommandRun, UserPromptSubmitHookOutcome,
        blocking_command_hook_output_status, command_hook_output_status,
        parse_hook_additional_context, parse_permission_denied_hook_stdout,
        parse_post_tool_failure_hook_stdout, parse_pre_tool_hook_stdout,
        parse_stop_hook_command_output, parse_subagent_start_hook_context,
        parse_user_prompt_submit_hook_command_output,
    },
};

fn source_prefix(source: HookSource) -> String {
    if source.is_local() {
        format!("[{}] ", source.label())
    } else {
        String::new()
    }
}

fn capture_stderr(output: &std::process::Output) -> Option<String> {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        None
    } else {
        Some(stderr)
    }
}

fn failure_error(source: HookSource, event: &str, command: &str, stderr: Option<String>) -> String {
    let prefix = source_prefix(source);
    match stderr {
        Some(stderr) => format!("{prefix}{event} hook `{command}` failed: {stderr}"),
        None => format!("{prefix}{event} hook `{command}` failed"),
    }
}

fn timeout_error(source: HookSource, event: &str, command: &str) -> String {
    format!(
        "{}{event} hook `{command}` timed out",
        source_prefix(source)
    )
}

fn start_error(source: HookSource, event: &str, command: &str, error: &CoreError) -> String {
    format!(
        "{}{event} hook `{command}` failed to start: {error}",
        source_prefix(source)
    )
}

pub(crate) async fn run_post_tool_command_hook(
    context: &HookCommandContext<'_>,
    tool_use_id: &str,
    tool_name: &str,
    tool_input: &str,
    tool_response: &Value,
    command: &str,
    timeout_seconds: Option<f64>,
    source: HookSource,
) -> HookAdditionalContextCommandRun {
    let hook_input = json!({
        "session_id": context.session_id,
        "transcript_path": context.transcript_path(),
        "cwd": context.cwd_display(),
        "hook_event_name": "PostToolUse",
        "tool_name": tool_name,
        "tool_input": deserialize_block_payload(tool_input),
        "tool_response": tool_response,
        "tool_use_id": tool_use_id,
    });
    let output = match run_command_hook_capture(
        &context.cwd,
        &hook_input,
        command,
        timeout_seconds,
        HookCommandErrorContext::Generic,
    )
    .await
    {
        Ok(Some(output)) => output,
        Ok(None) => {
            return HookAdditionalContextCommandRun {
                additional_context: None,
                retry: false,
                status: "timed_out",
                exit_code: None,
                error: Some(timeout_error(source, "PostToolUse", command)),
            };
        }
        Err(error) => {
            return HookAdditionalContextCommandRun {
                additional_context: None,
                retry: false,
                status: "failed",
                exit_code: None,
                error: Some(start_error(source, "PostToolUse", command, &error)),
            };
        }
    };
    let (status, exit_code) = command_hook_output_status(&output);
    let mut error = if status == "failed" {
        Some(failure_error(
            source,
            "PostToolUse",
            command,
            capture_stderr(&output),
        ))
    } else {
        None
    };
    let mut additional_context = match parse_hook_additional_context(&output, "PostToolUse") {
        Ok(context) => context,
        Err(reason) => {
            error = Some(format!("{}{reason}", source_prefix(source)));
            None
        }
    };
    // A PostToolUse hook can block and feed the model a reason via exit code 2
    // (stderr) or a JSON `{"decision":"block","reason":...}` on stdout. This was
    // previously recorded only as a UI progress error and never surfaced to the
    // model (fail-open). Merge the reason into the model-visible context.
    if let Some(block_feedback) = post_tool_block_feedback(&output) {
        additional_context = Some(match additional_context {
            Some(existing) => format!("{existing}\n{block_feedback}"),
            None => block_feedback,
        });
    }
    HookAdditionalContextCommandRun {
        additional_context,
        retry: false,
        status,
        exit_code,
        error,
    }
}

/// Extract blocking feedback a PostToolUse hook wants surfaced to the model:
/// exit code 2 carries it on stderr, and an exit-0 `{"decision":"block"}`
/// carries it in the `reason` field.
fn post_tool_block_feedback(output: &std::process::Output) -> Option<String> {
    if output.status.code() == Some(2) {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Some(if stderr.is_empty() {
            "PostToolUse hook blocked (exit code 2).".to_string()
        } else {
            stderr
        });
    }
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim_start().starts_with('{')
            && let Ok(Value::Object(json)) = serde_json::from_str::<Value>(stdout.trim())
            && json.get("decision").and_then(Value::as_str) == Some("block")
        {
            return Some(
                json.get("reason")
                    .and_then(Value::as_str)
                    .filter(|reason| !reason.trim().is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| "PostToolUse hook blocked.".to_string()),
            );
        }
    }
    None
}

pub(crate) async fn run_post_tool_failure_command_hook(
    context: &HookCommandContext<'_>,
    tool_use_id: &str,
    tool_name: &str,
    tool_input: &str,
    error: &str,
    is_interrupt: bool,
    command: &str,
    timeout_seconds: Option<f64>,
    source: HookSource,
) -> HookAdditionalContextCommandRun {
    let hook_input = json!({
        "session_id": context.session_id,
        "transcript_path": context.transcript_path(),
        "cwd": context.cwd_display(),
        "hook_event_name": "PostToolUseFailure",
        "tool_name": tool_name,
        "tool_input": deserialize_block_payload(tool_input),
        "tool_use_id": tool_use_id,
        "error": error,
        "is_interrupt": is_interrupt,
    });
    let output = match run_command_hook_capture(
        &context.cwd,
        &hook_input,
        command,
        timeout_seconds,
        HookCommandErrorContext::Generic,
    )
    .await
    {
        Ok(Some(output)) => output,
        Ok(None) => {
            return HookAdditionalContextCommandRun {
                additional_context: None,
                retry: false,
                status: "timed_out",
                exit_code: None,
                error: Some(timeout_error(source, "PostToolUseFailure", command)),
            };
        }
        Err(error) => {
            return HookAdditionalContextCommandRun {
                additional_context: None,
                retry: false,
                status: "failed",
                exit_code: None,
                error: Some(start_error(source, "PostToolUseFailure", command, &error)),
            };
        }
    };
    let (status, exit_code) = command_hook_output_status(&output);
    let mut run_error = if status == "failed" {
        Some(failure_error(
            source,
            "PostToolUseFailure",
            command,
            capture_stderr(&output),
        ))
    } else {
        None
    };
    let (additional_context, retry) = match parse_post_tool_failure_hook_stdout(&output) {
        Ok(parsed) => (parsed.additional_context, parsed.retry),
        Err(reason) => {
            run_error = Some(format!("{}{reason}", source_prefix(source)));
            (None, false)
        }
    };
    HookAdditionalContextCommandRun {
        additional_context,
        retry,
        status,
        exit_code,
        error: run_error,
    }
}

pub(crate) async fn run_user_prompt_submit_command_hook(
    context: &HookCommandContext<'_>,
    prompt: &str,
    command: &str,
    timeout_seconds: Option<f64>,
    source: HookSource,
) -> UserPromptSubmitHookCommandRun {
    let hook_input = json!({
        "session_id": context.session_id,
        "transcript_path": context.transcript_path(),
        "cwd": context.cwd_display(),
        "hook_event_name": "UserPromptSubmit",
        "prompt": prompt,
    });
    let output = match run_command_hook_capture(
        &context.cwd,
        &hook_input,
        command,
        timeout_seconds,
        HookCommandErrorContext::Generic,
    )
    .await
    {
        Ok(Some(output)) => output,
        Ok(None) => {
            return UserPromptSubmitHookCommandRun {
                outcome: UserPromptSubmitHookOutcome::default(),
                status: "timed_out",
                exit_code: None,
                error: Some(timeout_error(source, "UserPromptSubmit", command)),
            };
        }
        Err(error) => {
            return UserPromptSubmitHookCommandRun {
                outcome: UserPromptSubmitHookOutcome::default(),
                status: "failed",
                exit_code: None,
                error: Some(start_error(source, "UserPromptSubmit", command, &error)),
            };
        }
    };
    let (status, exit_code) = blocking_command_hook_output_status(&output);
    let mut error = if status == "failed" {
        Some(failure_error(
            source,
            "UserPromptSubmit",
            command,
            capture_stderr(&output),
        ))
    } else {
        None
    };
    let outcome = match parse_user_prompt_submit_hook_command_output(command, &output) {
        Ok(outcome) => outcome,
        Err(reason) => {
            error = Some(format!("{}{reason}", source_prefix(source)));
            UserPromptSubmitHookOutcome::default()
        }
    };
    UserPromptSubmitHookCommandRun {
        outcome,
        status,
        exit_code,
        error,
    }
}

pub(crate) async fn run_permission_denied_command_hook(
    context: &HookCommandContext<'_>,
    tool_use_id: &str,
    tool_name: &str,
    tool_input: &str,
    reason: &str,
    command: &str,
    timeout_seconds: Option<f64>,
    source: HookSource,
) -> PermissionDeniedHookCommandRun {
    let hook_input = json!({
        "session_id": context.session_id,
        "transcript_path": context.transcript_path(),
        "cwd": context.cwd_display(),
        "hook_event_name": "PermissionDenied",
        "tool_name": tool_name,
        "tool_input": deserialize_block_payload(tool_input),
        "tool_use_id": tool_use_id,
        "reason": reason,
    });
    let output = match run_command_hook_capture(
        &context.cwd,
        &hook_input,
        command,
        timeout_seconds,
        HookCommandErrorContext::Generic,
    )
    .await
    {
        Ok(Some(output)) => output,
        Ok(None) => {
            return PermissionDeniedHookCommandRun {
                retry: false,
                status: "timed_out",
                exit_code: None,
                error: Some(timeout_error(source, "PermissionDenied", command)),
            };
        }
        Err(error) => {
            return PermissionDeniedHookCommandRun {
                retry: false,
                status: "failed",
                exit_code: None,
                error: Some(start_error(source, "PermissionDenied", command, &error)),
            };
        }
    };
    let (mut status, exit_code) = command_hook_output_status(&output);
    let mut error = None;
    let retry = if status == "completed" {
        match parse_permission_denied_hook_stdout(&String::from_utf8_lossy(&output.stdout)) {
            Ok(retry) => retry,
            Err(reason) => {
                status = "failed";
                error = Some(format!("{}{reason}", source_prefix(source)));
                false
            }
        }
    } else {
        let stderr = capture_stderr(&output);
        error = Some(failure_error(source, "PermissionDenied", command, stderr));
        false
    };
    PermissionDeniedHookCommandRun {
        retry,
        status,
        exit_code,
        error,
    }
}

pub(crate) async fn run_stop_command_hook(
    context: &HookCommandContext<'_>,
    last_assistant_message: &str,
    stop_hook_active: bool,
    command: &str,
    timeout_seconds: Option<f64>,
    source: HookSource,
) -> StopHookCommandRun {
    let hook_input = json!({
        "session_id": context.session_id,
        "transcript_path": context.transcript_path(),
        "cwd": context.cwd_display(),
        "hook_event_name": "Stop",
        "stop_hook_active": stop_hook_active,
        "last_assistant_message": last_assistant_message,
    });
    let output = match run_command_hook_capture(
        &context.cwd,
        &hook_input,
        command,
        timeout_seconds,
        HookCommandErrorContext::Generic,
    )
    .await
    {
        Ok(Some(output)) => output,
        Ok(None) => {
            return StopHookCommandRun {
                outcome: StopHookOutcome::default(),
                status: "timed_out",
                exit_code: None,
                error: Some(timeout_error(source, "Stop", command)),
            };
        }
        Err(error) => {
            return StopHookCommandRun {
                outcome: StopHookOutcome::default(),
                status: "failed",
                exit_code: None,
                error: Some(start_error(source, "Stop", command, &error)),
            };
        }
    };
    let (status, exit_code) = blocking_command_hook_output_status(&output);
    let mut error = if status == "failed" {
        Some(failure_error(
            source,
            "Stop",
            command,
            capture_stderr(&output),
        ))
    } else {
        None
    };
    let outcome = match parse_stop_hook_command_output(command, &output) {
        Ok(outcome) => outcome,
        Err(reason) => {
            error = Some(format!("{}{reason}", source_prefix(source)));
            StopHookOutcome::default()
        }
    };
    StopHookCommandRun {
        outcome,
        status,
        exit_code,
        error,
    }
}

pub(crate) async fn run_stop_failure_command_hook(
    context: &HookCommandContext<'_>,
    error: &str,
    error_details: &str,
    last_assistant_message: Option<&str>,
    command: &str,
    timeout_seconds: Option<f64>,
    source: HookSource,
) -> HookCommandRunStatus {
    let hook_input = json!({
        "session_id": context.session_id,
        "transcript_path": context.transcript_path(),
        "cwd": context.cwd_display(),
        "hook_event_name": "StopFailure",
        "error": error,
        "error_details": error_details,
        "last_assistant_message": last_assistant_message,
    });
    let mut status =
        run_nonblocking_command_hook(&context.cwd, &hook_input, command, timeout_seconds).await;
    if status.error.is_none() {
        status.error = match status.status {
            "timed_out" => Some(timeout_error(source, "StopFailure", command)),
            "failed" => Some(failure_error(source, "StopFailure", command, None)),
            _ => None,
        };
    }
    status
}

pub(crate) async fn run_subagent_start_command_hook(
    context: &HookCommandContext<'_>,
    agent_id: &str,
    agent_type: &str,
    command: &str,
    timeout_seconds: Option<f64>,
    source: HookSource,
) -> HookAdditionalContextCommandRun {
    let hook_input = json!({
        "session_id": context.session_id,
        "transcript_path": context.transcript_path(),
        "cwd": context.cwd_display(),
        "hook_event_name": "SubagentStart",
        "agent_id": agent_id,
        "agent_type": agent_type,
    });
    let output = match run_command_hook_capture(
        &context.cwd,
        &hook_input,
        command,
        timeout_seconds,
        HookCommandErrorContext::Generic,
    )
    .await
    {
        Ok(Some(output)) => output,
        Ok(None) => {
            return HookAdditionalContextCommandRun {
                additional_context: None,
                retry: false,
                status: "timed_out",
                exit_code: None,
                error: Some(timeout_error(source, "SubagentStart", command)),
            };
        }
        Err(error) => {
            return HookAdditionalContextCommandRun {
                additional_context: None,
                retry: false,
                status: "failed",
                exit_code: None,
                error: Some(start_error(source, "SubagentStart", command, &error)),
            };
        }
    };
    let (status, exit_code) = command_hook_output_status(&output);
    let mut error = if status == "failed" {
        Some(failure_error(
            source,
            "SubagentStart",
            command,
            capture_stderr(&output),
        ))
    } else {
        None
    };
    let additional_context = match parse_subagent_start_hook_context(&output) {
        Ok(context) => context,
        Err(reason) => {
            error = Some(format!("{}{reason}", source_prefix(source)));
            None
        }
    };
    HookAdditionalContextCommandRun {
        additional_context,
        retry: false,
        status,
        exit_code,
        error,
    }
}

pub(crate) async fn run_subagent_stop_command_hook(
    context: &HookCommandContext<'_>,
    agent_id: &str,
    child_session_id: &str,
    agent_type: &str,
    last_assistant_message: &str,
    stop_hook_active: bool,
    command: &str,
    timeout_seconds: Option<f64>,
    source: HookSource,
) -> StopHookCommandRun {
    let hook_input = json!({
        "session_id": context.session_id,
        "transcript_path": context.transcript_path(),
        "cwd": context.cwd_display(),
        "hook_event_name": "SubagentStop",
        "stop_hook_active": stop_hook_active,
        "agent_id": agent_id,
        "agent_transcript_path": context.agent_transcript_path(child_session_id),
        "agent_type": agent_type,
        "last_assistant_message": last_assistant_message,
    });
    let output = match run_command_hook_capture(
        &context.cwd,
        &hook_input,
        command,
        timeout_seconds,
        HookCommandErrorContext::Generic,
    )
    .await
    {
        Ok(Some(output)) => output,
        Ok(None) => {
            return StopHookCommandRun {
                outcome: StopHookOutcome::default(),
                status: "timed_out",
                exit_code: None,
                error: Some(timeout_error(source, "SubagentStop", command)),
            };
        }
        Err(error) => {
            return StopHookCommandRun {
                outcome: StopHookOutcome::default(),
                status: "failed",
                exit_code: None,
                error: Some(start_error(source, "SubagentStop", command, &error)),
            };
        }
    };
    let (status, exit_code) = blocking_command_hook_output_status(&output);
    let mut error = if status == "failed" {
        Some(failure_error(
            source,
            "SubagentStop",
            command,
            capture_stderr(&output),
        ))
    } else {
        None
    };
    let outcome = match parse_stop_hook_command_output(command, &output) {
        Ok(outcome) => outcome,
        Err(reason) => {
            error = Some(format!("{}{reason}", source_prefix(source)));
            StopHookOutcome::default()
        }
    };
    StopHookCommandRun {
        outcome,
        status,
        exit_code,
        error,
    }
}

pub(crate) async fn run_pre_tool_command_hook(
    context: &HookCommandContext<'_>,
    tool_use_id: &str,
    tool_name: &str,
    tool_input: &str,
    command: &str,
    timeout_seconds: Option<f64>,
    source: HookSource,
) -> Result<Option<PreToolHookCommandResult>, CoreError> {
    let hook_input = json!({
        "session_id": context.session_id,
        "transcript_path": context.transcript_path(),
        "cwd": context.cwd_display(),
        "hook_event_name": "PreToolUse",
        "tool_name": tool_name,
        "tool_input": deserialize_block_payload(tool_input),
        "tool_use_id": tool_use_id,
    });
    let prefix = source_prefix(source);
    let output = match run_command_hook_capture(
        &context.cwd,
        &hook_input,
        command,
        timeout_seconds,
        HookCommandErrorContext::Event("PreToolUse"),
    )
    .await?
    {
        Some(output) => output,
        None => {
            return Ok(Some(PreToolHookCommandResult::deny(format!(
                "{prefix}PreToolUse hook timed out: {command}"
            ))));
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.code() == Some(2) {
        return Ok(Some(PreToolHookCommandResult::deny(if stderr.is_empty() {
            format!("{prefix}PreToolUse hook blocked tool: {command}")
        } else {
            format!("{prefix}{stderr}")
        })));
    }
    if !output.status.success() {
        return Ok(Some(PreToolHookCommandResult::deny(if stderr.is_empty() {
            format!(
                "{prefix}PreToolUse hook failed with {}: {command}",
                output.status
            )
        } else {
            format!(
                "{prefix}PreToolUse hook failed with {}: {stderr}",
                output.status
            )
        })));
    }
    if stdout.is_empty() {
        return Ok(None);
    }
    match parse_pre_tool_hook_stdout(&stdout) {
        Ok(result) => Ok(result),
        Err(reason) => Ok(Some(PreToolHookCommandResult::deny(format!(
            "{prefix}{reason}"
        )))),
    }
}

#[cfg(test)]
mod tests {
    use super::post_tool_block_feedback;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};

    fn output(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn post_tool_block_feedback_surfaces_exit2_and_json_block() {
        // Exit 2 → stderr is the model-visible block reason.
        assert_eq!(
            post_tool_block_feedback(&output(2, "", "do not do that")),
            Some("do not do that".to_string())
        );
        // Exit 0 with a JSON block decision → reason is surfaced.
        assert_eq!(
            post_tool_block_feedback(&output(
                0,
                r#"{"decision":"block","reason":"blocked by policy"}"#,
                ""
            )),
            Some("blocked by policy".to_string())
        );
        // A successful, non-blocking hook produces no feedback.
        assert_eq!(post_tool_block_feedback(&output(0, "{}", "")), None);
    }
}
