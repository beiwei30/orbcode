use std::time::Duration;

use orbcode_config::{HookCommand, HookMatcher};
use orbcode_protocol::{MessageRole, TranscriptMessage};
use serde_json::{Value, json};

use crate::permissions::PermissionRule;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookPermissionDecision {
    Allow,
    Deny,
    Ask,
}

#[derive(Clone, Debug)]
pub(crate) struct PreToolHookOutcome {
    pub(crate) tool_input: String,
    pub(crate) decision: Option<HookPermissionDecision>,
    pub(crate) reason: Option<String>,
    pub(crate) additional_contexts: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreToolPhaseOutcome {
    pub(crate) tool_input: String,
    pub(crate) decision: Option<HookPermissionDecision>,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreToolHookCommandResult {
    pub(crate) decision: Option<HookPermissionDecision>,
    pub(crate) reason: Option<String>,
    pub(crate) updated_input: Option<String>,
    pub(crate) additional_context: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct HookAdditionalContextCommandRun {
    pub(crate) additional_context: Option<String>,
    pub(crate) retry: bool,
    pub(crate) status: &'static str,
    pub(crate) exit_code: Option<i32>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PermissionDeniedHookCommandRun {
    pub(crate) retry: bool,
    pub(crate) status: &'static str,
    pub(crate) exit_code: Option<i32>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct HookCommandRunStatus {
    pub(crate) status: &'static str,
    pub(crate) exit_code: Option<i32>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct StopHookCommandRun {
    pub(crate) outcome: StopHookOutcome,
    pub(crate) status: &'static str,
    pub(crate) exit_code: Option<i32>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct UserPromptSubmitHookCommandRun {
    pub(crate) outcome: UserPromptSubmitHookOutcome,
    pub(crate) status: &'static str,
    pub(crate) exit_code: Option<i32>,
    pub(crate) error: Option<String>,
}

impl PreToolHookCommandResult {
    pub(crate) fn deny(reason: String) -> Self {
        Self {
            decision: Some(HookPermissionDecision::Deny),
            reason: Some(reason),
            updated_input: None,
            additional_context: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StopHookOutcome {
    pub(crate) blocking_errors: Vec<String>,
    pub(crate) prevent_continuation: bool,
    pub(crate) stop_reason: Option<String>,
}

impl StopHookOutcome {
    pub(crate) fn merge(&mut self, other: Self) {
        self.blocking_errors.extend(other.blocking_errors);
        if other.prevent_continuation {
            self.prevent_continuation = true;
            if other.stop_reason.is_some() {
                self.stop_reason = other.stop_reason;
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct UserPromptSubmitHookOutcome {
    pub(crate) additional_contexts: Vec<String>,
    pub(crate) blocking_errors: Vec<String>,
}

impl UserPromptSubmitHookOutcome {
    pub(crate) fn merge(&mut self, other: Self) {
        self.additional_contexts.extend(other.additional_contexts);
        self.blocking_errors.extend(other.blocking_errors);
    }
}

/// Whether a hook matcher applies to a tool call, for callers that cannot
/// propagate an error (probes that decide whether to *engage* the hook
/// pipeline, and post-tool events).
///
/// An unparseable matcher is reported as MATCHING here so the pipeline still
/// engages — the authoritative decision then happens in
/// [`tool_hook_matches_checked`], which fails closed. Reporting "no match"
/// instead would let a broken PreToolUse guard be skipped via a fast-path
/// auto-approve before the decision layer ever runs.
pub(crate) fn tool_hook_matches(matcher: &HookMatcher, tool_name: &str, tool_input: &str) -> bool {
    tool_hook_matches_checked(matcher, tool_name, tool_input).unwrap_or(true)
}

/// Whether a hook matcher applies, distinguishing an *invalid* matcher (one that
/// is neither a permission-rule shape nor a regex valid in JavaScript, or whose
/// evaluation errors) from a clean match/no-match.
///
/// The PreToolUse decision path uses this and fails CLOSED on `Err`: a single
/// boolean fallback cannot be safe for both an `allow` hook (a spurious match
/// silently authorizes the tool) and a `block` hook (a spurious no-match
/// silently skips the guard), and the matcher's hidden decision is unknowable at
/// match time. Denying the tool with the config error surfaced is the only
/// resolution that is safe regardless of the hook's intent.
pub(crate) fn tool_hook_matches_checked(
    matcher: &HookMatcher,
    tool_name: &str,
    tool_input: &str,
) -> Result<bool, String> {
    let Some(pattern) = matcher.matcher.as_deref() else {
        return Ok(true);
    };
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == "*" {
        return Ok(true);
    }
    tool_hook_matcher_part_matches(pattern, tool_name, tool_input)
}

pub(crate) fn matching_tool_hook_has_command(
    matcher: &HookMatcher,
    tool_name: &str,
    tool_input: &str,
) -> bool {
    if !tool_hook_matches(matcher, tool_name, tool_input) {
        return false;
    }
    matcher.hooks.iter().any(|hook| match hook {
        HookCommand::Command { r#if, .. } => match r#if {
            Some(condition) => {
                PermissionRule::parse(condition).matches_tool_call(tool_name, tool_input)
            }
            None => true,
        },
        HookCommand::Unsupported => false,
    })
}

fn tool_hook_matcher_part_matches(
    pattern: &str,
    tool_name: &str,
    tool_input: &str,
) -> Result<bool, String> {
    let pattern = pattern.trim();
    // Orb Code extension: a `Tool(inputPattern)` matcher (`Bash(rm:*)`,
    // `Edit(src/**)`) also constrains on the tool input, not just the name.
    if looks_like_permission_rule(pattern) {
        return Ok(PermissionRule::parse(pattern).matches_tool_call(tool_name, tool_input));
    }
    // TS parity: the matcher is a JavaScript regexp, tested (unanchored, like
    // `RegExp.test`) against the tool name. This covers `mcp__.*`, `.*`,
    // `^(Edit|Write)$`, `Notebook.*`, and plain `Edit|Write` alternations.
    if let Ok(re) = regex::Regex::new(pattern) {
        return Ok(re.is_match(tool_name));
    }
    // The Rust `regex` crate rejects some valid-JavaScript syntax (lookaround,
    // backreferences), e.g. `^(?!Read$).*` ("any tool but Read"). Evaluate those
    // with `fancy-regex`, which supports them, so the matcher matches EXACTLY the
    // JS semantics — Bash/Write match, Read does NOT. A runtime evaluation error
    // (e.g. backtracking limit) is reported as an error so the decision layer can
    // fail closed rather than silently drop a `block` hook.
    match fancy_regex::Regex::new(pattern) {
        Ok(re) => re.is_match(tool_name).map_err(|error| {
            format!("hook matcher `{pattern}` failed to evaluate against `{tool_name}`: {error}")
        }),
        // Truly malformed (invalid in JavaScript too): unknowable intent, so
        // report an error and let the caller fail closed. `pattern` is non-empty
        // here (empty/`*` returned earlier).
        Err(error) => Err(format!(
            "hook matcher `{pattern}` is not a valid regular expression: {error}"
        )),
    }
}

/// Returns true when `pattern` has the Orb Code `Tool(inputPattern)` shape — a
/// non-empty tool-name-like prefix followed by a parenthesised body — rather
/// than a bare regex. `(Edit|Write)` (empty prefix) and `^(Edit|Write)$` (not
/// closed by `)`) are regexes, not rules.
fn looks_like_permission_rule(pattern: &str) -> bool {
    let Some(open) = pattern.find('(') else {
        return false;
    };
    if !pattern.ends_with(')') {
        return false;
    }
    let name = &pattern[..open];
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub(crate) fn lifecycle_hook_matches(matcher: &HookMatcher, match_query: Option<&str>) -> bool {
    let Some(pattern) = matcher.matcher.as_deref() else {
        return true;
    };
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    let Some(match_query) = match_query else {
        return false;
    };
    if pattern.contains('|') {
        return pattern
            .split('|')
            .any(|part| part.trim().eq_ignore_ascii_case(match_query));
    }
    pattern.eq_ignore_ascii_case(match_query)
}

pub(crate) fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

pub(crate) fn parse_stop_hook_command_output(
    command: &str,
    output: &std::process::Output,
) -> Result<StopHookOutcome, String> {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.code() == Some(2) {
        return Ok(StopHookOutcome {
            blocking_errors: vec![if stderr.is_empty() {
                format!("[{command}]: No stderr output")
            } else {
                format!("[{command}]: {stderr}")
            }],
            ..StopHookOutcome::default()
        });
    }
    if !output.status.success() || stdout.is_empty() || !stdout.trim_start().starts_with('{') {
        return Ok(StopHookOutcome::default());
    }
    let json = serde_json::from_str::<Value>(&stdout)
        .map_err(|error| format!("Stop hook returned invalid JSON: {error}"))?;
    if !json.is_object() {
        return Err("Stop hook output must be a JSON object".to_string());
    }

    let mut outcome = StopHookOutcome::default();
    match json.get("continue") {
        Some(Value::Bool(value)) => {
            if !*value {
                outcome.prevent_continuation = true;
                outcome.stop_reason = match json.get("stopReason") {
                    Some(Value::String(reason)) => Some(reason.clone()),
                    Some(_) => return Err("Stop hook stopReason must be a string".to_string()),
                    None => Some("Stop hook prevented continuation".to_string()),
                };
            }
        }
        Some(_) => return Err("Stop hook continue must be a boolean".to_string()),
        None => {}
    }
    match json.get("decision") {
        Some(Value::String(value)) if value == "block" => {
            outcome.blocking_errors.push(match json.get("reason") {
                Some(Value::String(reason)) => reason.clone(),
                Some(_) => return Err("Stop hook reason must be a string".to_string()),
                None => "Blocked by hook".to_string(),
            });
        }
        Some(Value::String(_)) | None => {}
        Some(_) => return Err("Stop hook decision must be a string".to_string()),
    }
    Ok(outcome)
}

pub(crate) fn parse_user_prompt_submit_hook_command_output(
    command: &str,
    output: &std::process::Output,
) -> Result<UserPromptSubmitHookOutcome, String> {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.code() == Some(2) {
        return Ok(UserPromptSubmitHookOutcome {
            blocking_errors: vec![if stderr.is_empty() {
                format!("[{command}]: No stderr output")
            } else {
                format!("[{command}]: {stderr}")
            }],
            ..UserPromptSubmitHookOutcome::default()
        });
    }

    let mut outcome = UserPromptSubmitHookOutcome::default();
    // A UserPromptSubmit hook can also block via a JSON `{"decision":"block"}`
    // on stdout with exit 0 (not only via exit 2). Without parsing this, such a
    // block was silently ignored and the prompt was sent to the model anyway
    // (fail-open).
    let stdout = String::from_utf8_lossy(&output.stdout);
    if output.status.success()
        && stdout.trim_start().starts_with('{')
        && let Ok(Value::Object(json)) = serde_json::from_str::<Value>(stdout.trim())
        && json.get("decision").and_then(Value::as_str) == Some("block")
    {
        let reason = json
            .get("reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "Blocked by hook".to_string());
        outcome.blocking_errors.push(reason);
    }
    if let Some(additional_context) = parse_hook_additional_context(output, "UserPromptSubmit")? {
        outcome.additional_contexts.push(additional_context);
    }
    Ok(outcome)
}

pub(crate) fn stop_hook_feedback(blocking_error: &str) -> String {
    format!("Stop hook feedback:\n{blocking_error}")
}

pub(crate) fn user_prompt_submit_hook_blocking_message(blocking_errors: &[String]) -> String {
    format!(
        "UserPromptSubmit operation blocked by hook:\n{}",
        blocking_errors.join("\n")
    )
}

pub(crate) fn hook_additional_context(hook_event: &str, additional_context: &str) -> String {
    format!("{hook_event} hook context:\n{additional_context}")
}

pub(crate) fn model_visible_context_message(content: String) -> TranscriptMessage {
    TranscriptMessage::new(MessageRole::User, content).with_synthetic(true)
}

pub(crate) fn parse_subagent_start_hook_context(
    output: &std::process::Output,
) -> Result<Option<String>, String> {
    parse_hook_additional_context(output, "SubagentStart")
}

pub(crate) fn parse_hook_additional_context(
    output: &std::process::Output,
    expected_event: &str,
) -> Result<Option<String>, String> {
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() || !stdout.trim_start().starts_with('{') {
        return Ok(None);
    }
    let json = serde_json::from_str::<Value>(&stdout)
        .map_err(|error| format!("{expected_event} hook returned invalid JSON: {error}"))?;
    let Some(specific) = json.get("hookSpecificOutput") else {
        return Ok(None);
    };
    let specific = specific
        .as_object()
        .ok_or_else(|| format!("{expected_event} hookSpecificOutput must be an object"))?;
    let event_name = specific.get("hookEventName").and_then(Value::as_str);
    if event_name != Some(expected_event) {
        if event_name.is_some() {
            return Err(format!(
                "{expected_event} hookSpecificOutput.hookEventName must be `{expected_event}`"
            ));
        }
        return Ok(None);
    }
    match specific.get("additionalContext") {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!(
            "{expected_event} hookSpecificOutput.additionalContext must be a string"
        )),
        None => Ok(None),
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PostToolFailureHookOutput {
    pub(crate) additional_context: Option<String>,
    pub(crate) retry: bool,
}

pub(crate) fn parse_post_tool_failure_hook_stdout(
    output: &std::process::Output,
) -> Result<PostToolFailureHookOutput, String> {
    if !output.status.success() {
        return Ok(PostToolFailureHookOutput::default());
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() || !stdout.trim_start().starts_with('{') {
        return Ok(PostToolFailureHookOutput::default());
    }
    let json = serde_json::from_str::<Value>(&stdout)
        .map_err(|error| format!("PostToolUseFailure hook returned invalid JSON: {error}"))?;
    let Some(specific) = json.get("hookSpecificOutput") else {
        return Ok(PostToolFailureHookOutput::default());
    };
    let specific = specific
        .as_object()
        .ok_or_else(|| "PostToolUseFailure hookSpecificOutput must be an object".to_string())?;
    let event_name = specific.get("hookEventName").and_then(Value::as_str);
    if event_name != Some("PostToolUseFailure") {
        if event_name.is_some() {
            return Err(
                "PostToolUseFailure hookSpecificOutput.hookEventName must be `PostToolUseFailure`"
                    .to_string(),
            );
        }
        return Ok(PostToolFailureHookOutput::default());
    }
    let additional_context = match specific.get("additionalContext") {
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => {
            return Err(
                "PostToolUseFailure hookSpecificOutput.additionalContext must be a string"
                    .to_string(),
            );
        }
        None => None,
    };
    let retry = match specific.get("retry") {
        Some(Value::Bool(value)) => *value,
        Some(_) => {
            return Err(
                "PostToolUseFailure hookSpecificOutput.retry must be a boolean".to_string(),
            );
        }
        None => false,
    };
    Ok(PostToolFailureHookOutput {
        additional_context,
        retry,
    })
}

pub(crate) fn parse_permission_denied_hook_stdout(stdout: &str) -> Result<bool, String> {
    let stdout = stdout.trim();
    if stdout.is_empty() {
        return Ok(false);
    }
    let json = serde_json::from_str::<Value>(stdout)
        .map_err(|error| format!("PermissionDenied hook returned invalid JSON: {error}"))?;
    if !json.is_object() {
        return Err("PermissionDenied hook output must be a JSON object".to_string());
    }
    let Some(specific) = json.get("hookSpecificOutput") else {
        return Ok(false);
    };
    let specific = specific
        .as_object()
        .ok_or_else(|| "PermissionDenied hookSpecificOutput must be an object".to_string())?;
    let event_name = specific
        .get("hookEventName")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "PermissionDenied hookSpecificOutput.hookEventName must be `PermissionDenied`"
                .to_string()
        })?;
    if event_name != "PermissionDenied" {
        return Err(
            "PermissionDenied hookSpecificOutput.hookEventName must be `PermissionDenied`"
                .to_string(),
        );
    }
    match specific.get("retry") {
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err("PermissionDenied hookSpecificOutput.retry must be a boolean".to_string()),
        None => Ok(false),
    }
}

pub(crate) fn subagent_start_hook_context(additional_context: &str) -> String {
    format!("SubagentStart hook context:\n{additional_context}")
}

pub(crate) fn subagent_stop_hook_feedback(blocking_error: &str) -> String {
    format!("SubagentStop hook feedback:\n{blocking_error}")
}

pub(crate) fn parse_pre_tool_hook_stdout(
    stdout: &str,
) -> Result<Option<PreToolHookCommandResult>, String> {
    let json = serde_json::from_str::<Value>(stdout)
        .map_err(|error| format!("PreToolUse hook returned invalid JSON: {error}"))?;
    if !json.is_object() {
        return Err("PreToolUse hook output must be a JSON object".to_string());
    }
    let mut result = PreToolHookCommandResult {
        decision: parse_top_level_hook_decision(&json)?,
        reason: parse_optional_string_field(&json, "reason")?,
        updated_input: None,
        additional_context: None,
    };

    if let Some(specific) = json.get("hookSpecificOutput") {
        let specific = specific
            .as_object()
            .ok_or_else(|| "PreToolUse hookSpecificOutput must be an object".to_string())?;
        let event_name = specific
            .get("hookEventName")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                "PreToolUse hookSpecificOutput.hookEventName must be `PreToolUse`".to_string()
            })?;
        if event_name != "PreToolUse" {
            return Err(
                "PreToolUse hookSpecificOutput.hookEventName must be `PreToolUse`".to_string(),
            );
        }
        if let Some(decision) = parse_hook_permission_decision(specific.get("permissionDecision"))?
        {
            result.decision = Some(decision);
        }
        if let Some(reason) =
            parse_optional_object_string_field(specific, "permissionDecisionReason")?
        {
            result.reason = Some(reason);
        }
        if let Some(updated_input) = specific.get("updatedInput") {
            if !updated_input.is_object() {
                return Err(
                    "PreToolUse hookSpecificOutput.updatedInput must be an object".to_string(),
                );
            }
            result.updated_input = Some(updated_input.to_string());
        }
        if let Some(additional_context) =
            parse_optional_object_string_field(specific, "additionalContext")?
        {
            result.additional_context = Some(additional_context);
        }
    }

    Ok(Some(result))
}

fn parse_top_level_hook_decision(json: &Value) -> Result<Option<HookPermissionDecision>, String> {
    match json.get("decision") {
        Some(Value::String(value)) => match value.as_str() {
            "approve" => Ok(Some(HookPermissionDecision::Allow)),
            "block" => Ok(Some(HookPermissionDecision::Deny)),
            _ => Err("PreToolUse hook decision must be `approve` or `block`".to_string()),
        },
        Some(_) => Err("PreToolUse hook decision must be a string".to_string()),
        None => Ok(None),
    }
}

fn parse_hook_permission_decision(
    value: Option<&Value>,
) -> Result<Option<HookPermissionDecision>, String> {
    match value {
        Some(Value::String(value)) => match value.as_str() {
            "allow" => Ok(Some(HookPermissionDecision::Allow)),
            "deny" => Ok(Some(HookPermissionDecision::Deny)),
            "ask" => Ok(Some(HookPermissionDecision::Ask)),
            _ => Err(
                "PreToolUse hook permissionDecision must be `allow`, `deny`, or `ask`".to_string(),
            ),
        },
        Some(_) => Err("PreToolUse hook permissionDecision must be a string".to_string()),
        None => Ok(None),
    }
}

fn parse_optional_string_field(json: &Value, field: &str) -> Result<Option<String>, String> {
    match json.get(field) {
        Some(Value::String(value)) => Ok(Some(value.to_string())),
        Some(_) => Err(format!("PreToolUse hook {field} must be a string")),
        None => Ok(None),
    }
}

fn parse_optional_object_string_field(
    json: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<String>, String> {
    match json.get(field) {
        Some(Value::String(value)) => Ok(Some(value.to_string())),
        Some(_) => Err(format!(
            "PreToolUse hookSpecificOutput.{field} must be a string"
        )),
        None => Ok(None),
    }
}

pub(crate) fn hook_shell_program() -> String {
    // Always run hooks under a POSIX shell (matching the TypeScript CLI's
    // `sh -c`). Using `$SHELL` would break when the user's login shell is
    // non-POSIX (e.g. fish/csh), which cannot parse the POSIX hook command.
    "sh".to_string()
}

pub(crate) fn command_hook_output_status(
    output: &std::process::Output,
) -> (&'static str, Option<i32>) {
    if output.status.success() {
        ("completed", output.status.code())
    } else {
        ("failed", output.status.code())
    }
}

pub(crate) fn blocking_command_hook_output_status(
    output: &std::process::Output,
) -> (&'static str, Option<i32>) {
    if output.status.code() == Some(2) {
        ("blocked", output.status.code())
    } else {
        command_hook_output_status(output)
    }
}

pub(crate) fn pre_tool_hook_command_status(result: &PreToolHookCommandResult) -> &'static str {
    if !matches!(result.decision, Some(HookPermissionDecision::Deny)) {
        return "completed";
    }
    let reason = result.reason.as_deref().unwrap_or_default();
    if reason.starts_with("PreToolUse hook timed out") {
        "timed_out"
    } else if reason.starts_with("PreToolUse hook failed") {
        "failed"
    } else {
        "blocked"
    }
}

pub(crate) fn hook_progress_record(
    hook_event_name: &str,
    command: &str,
    status: &'static str,
    exit_code: Option<i32>,
    error: Option<&str>,
    duration: Duration,
) -> Value {
    let mut data = json!({
        "type": "hook_progress",
        "hookEventName": hook_event_name,
        "command": command,
        "status": hook_progress_status_label(hook_event_name, status, duration),
        "result": status,
        "durationMs": duration.as_millis() as u64,
    });
    if let Some(exit_code) = exit_code
        && let Some(object) = data.as_object_mut()
    {
        object.insert("exitCode".to_string(), Value::from(exit_code));
    }
    if let Some(error) = error.and_then(non_empty_trimmed)
        && let Some(object) = data.as_object_mut()
    {
        object.insert("error".to_string(), Value::from(error));
    }
    json!({ "data": data })
}

fn hook_progress_status_label(
    hook_event_name: &str,
    status: &'static str,
    duration: Duration,
) -> String {
    let verb = match status {
        "blocked" => "blocked",
        "failed" => "failed",
        "timed_out" => "timed out",
        _ => "completed",
    };
    format!(
        "{hook_event_name} hook {verb} in {} ms",
        duration.as_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(pattern: &str) -> HookMatcher {
        HookMatcher {
            matcher: Some(pattern.to_string()),
            hooks: Vec::new(),
        }
    }

    #[test]
    fn tool_hook_matcher_supports_regex_patterns() {
        // Regex matchers that the old literal matcher silently dropped.
        assert!(tool_hook_matches(
            &matcher("mcp__.*"),
            "mcp__server__tool",
            "{}"
        ));
        assert!(tool_hook_matches(&matcher(".*"), "Bash", "{}"));
        assert!(tool_hook_matches(&matcher("^(Edit|Write)$"), "Edit", "{}"));
        assert!(tool_hook_matches(&matcher("^(Edit|Write)$"), "Write", "{}"));
        assert!(!tool_hook_matches(&matcher("^(Edit|Write)$"), "Read", "{}"));
        assert!(tool_hook_matches(&matcher("Edit|Write"), "Write", "{}"));
        assert!(tool_hook_matches(
            &matcher("Notebook.*"),
            "NotebookEdit",
            "{}"
        ));
        assert!(!tool_hook_matches(&matcher("^Edit$"), "Read", "{}"));
    }

    #[test]
    // This test deliberately constructs a pattern the Rust `regex` crate
    // rejects (lookaround), to assert the fallback engine handles it; silence
    // clippy's compile-time regex validation for that one intentional literal.
    #[allow(clippy::invalid_regex)]
    fn tool_hook_matcher_evaluates_js_lookaround_correctly() {
        // A negative-lookahead matcher ("any tool but Read") is valid JS regex
        // that the Rust `regex` crate cannot compile; `fancy-regex` evaluates it
        // with correct JS semantics. It must match Bash/Write and — crucially —
        // must NOT match Read, or an `allow` hook would wrongly authorize the
        // tool the matcher was written to exclude.
        assert!(regex::Regex::new("^(?!Read$).*").is_err());
        assert!(tool_hook_matches(&matcher("^(?!Read$).*"), "Bash", "{}"));
        assert!(tool_hook_matches(&matcher("^(?!Read$).*"), "Write", "{}"));
        assert!(
            !tool_hook_matches(&matcher("^(?!Read$).*"), "Read", "{}"),
            "negative lookahead must exclude Read (allow-hook over-match)"
        );
        // Positive lookahead is likewise honored.
        assert!(tool_hook_matches(&matcher("^Read(?=$)"), "Read", "{}"));
        assert!(!tool_hook_matches(&matcher("^Read(?=$)"), "ReadFile", "{}"));
    }

    #[test]
    fn invalid_tool_hook_matcher_fails_closed() {
        // A pattern that is invalid in JavaScript too (`(` never closes) has an
        // unknowable intent: the *checked* API reports an error so the PreToolUse
        // decision layer can DENY the tool rather than either over-authorize an
        // `allow` hook or silently skip a `block` hook.
        let error = tool_hook_matches_checked(&matcher("("), "Bash", "{}")
            .expect_err("an unparseable matcher must be reported as an error");
        assert!(error.contains("valid regular expression"), "{error}");
        // The best-effort boolean API reports MATCHING so the hook pipeline still
        // engages (and then fails closed in the decision layer) rather than being
        // skipped by a fast-path auto-approve.
        assert!(tool_hook_matches(&matcher("("), "Bash", "{}"));
        // A well-formed matcher still returns a clean Ok(bool).
        assert_eq!(
            tool_hook_matches_checked(&matcher("^Edit$"), "Edit", "{}"),
            Ok(true)
        );
        assert_eq!(
            tool_hook_matches_checked(&matcher("^Edit$"), "Read", "{}"),
            Ok(false)
        );
    }

    #[test]
    fn tool_hook_matcher_preserves_permission_rule_extension() {
        // `Tool(inputPattern)` still matches on input.
        assert!(tool_hook_matches(
            &matcher("Bash(rm:*)"),
            "Bash",
            &serde_json::json!({ "command": "rm -rf /tmp/x" }).to_string()
        ));
        assert!(!tool_hook_matches(
            &matcher("Bash(rm:*)"),
            "Bash",
            &serde_json::json!({ "command": "ls" }).to_string()
        ));
    }

    #[test]
    fn pre_tool_hook_stdout_validation_rejects_bad_schema() {
        let error = parse_pre_tool_hook_stdout(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"yes"}}"#,
        )
        .expect_err("invalid permission decision should be rejected");
        assert!(error.contains("permissionDecision"));

        let error = parse_pre_tool_hook_stdout(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":"echo hi"}}"#,
        )
        .expect_err("non-object updatedInput should be rejected");
        assert!(error.contains("updatedInput"));

        let error = parse_pre_tool_hook_stdout(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":42}}"#,
        )
        .expect_err("non-string additionalContext should be rejected");
        assert!(error.contains("additionalContext"));
    }

    #[test]
    fn user_prompt_submit_block_via_json_decision_populates_blocking_error() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::Output;

        // Exit 0 with `{"decision":"block"}` must block (previously only exit 2
        // was honored, so this JSON block was silently ignored — fail-open).
        let output = Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: br#"{"decision":"block","reason":"prompt not allowed"}"#.to_vec(),
            stderr: Vec::new(),
        };
        let outcome =
            parse_user_prompt_submit_hook_command_output("hook", &output).expect("valid output");
        assert_eq!(
            outcome.blocking_errors,
            vec!["prompt not allowed".to_string()]
        );

        // A non-block decision does not block.
        let output = Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: br#"{"decision":"approve"}"#.to_vec(),
            stderr: Vec::new(),
        };
        let outcome =
            parse_user_prompt_submit_hook_command_output("hook", &output).expect("valid output");
        assert!(outcome.blocking_errors.is_empty());
    }

    #[test]
    fn post_tool_failure_hook_stdout_parses_retry_and_context() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::Output;

        let success_output = |stdout: &str| Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        };

        let result = parse_post_tool_failure_hook_stdout(&success_output(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUseFailure","retry":true,"additionalContext":"try again"}}"#,
        ))
        .expect("valid output");
        assert!(result.retry);
        assert_eq!(result.additional_context.as_deref(), Some("try again"));

        let result = parse_post_tool_failure_hook_stdout(&success_output(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUseFailure","retry":false}}"#,
        ))
        .expect("valid output");
        assert!(!result.retry);
        assert_eq!(result.additional_context, None);

        let result = parse_post_tool_failure_hook_stdout(&success_output("{}"))
            .expect("empty object is no-op");
        assert!(!result.retry);
        assert_eq!(result.additional_context, None);

        let error = parse_post_tool_failure_hook_stdout(&success_output(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUseFailure","retry":"yes"}}"#,
        ))
        .expect_err("non-boolean retry should be rejected");
        assert!(error.contains("retry"));

        let error = parse_post_tool_failure_hook_stdout(&success_output(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","retry":true}}"#,
        ))
        .expect_err("wrong event name should be rejected");
        assert!(error.contains("hookEventName"));
    }

    #[test]
    fn permission_denied_hook_stdout_validation_rejects_bad_schema() {
        assert!(
            parse_permission_denied_hook_stdout(
                r#"{"hookSpecificOutput":{"hookEventName":"PermissionDenied","retry":true}}"#,
            )
            .expect("valid retry output")
        );
        assert!(!parse_permission_denied_hook_stdout(r"{}").expect("empty object is no-op"));

        let error = parse_permission_denied_hook_stdout("not json")
            .expect_err("invalid JSON should be rejected");
        assert!(error.contains("invalid JSON"));

        let error = parse_permission_denied_hook_stdout(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","retry":true}}"#,
        )
        .expect_err("wrong event name should be rejected");
        assert!(error.contains("hookEventName"));

        let error = parse_permission_denied_hook_stdout(
            r#"{"hookSpecificOutput":{"hookEventName":"PermissionDenied","retry":"yes"}}"#,
        )
        .expect_err("non-boolean retry should be rejected");
        assert!(error.contains("retry"));
    }
}
