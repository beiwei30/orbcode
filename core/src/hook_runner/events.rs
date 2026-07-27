use std::time::Instant;

use orbcode_config::{HookMatcher, HookSource};
use serde_json::Value;

use super::HookCommandContext;
use super::adapters::{
    run_permission_denied_command_hook, run_post_tool_command_hook,
    run_post_tool_failure_command_hook, run_pre_tool_command_hook, run_stop_command_hook,
    run_stop_failure_command_hook, run_subagent_start_command_hook, run_subagent_stop_command_hook,
    run_user_prompt_submit_command_hook,
};
use super::command::{HookCommandProgress, command_hook_parts, progress};
use crate::{
    CoreError,
    hooks::{
        HookPermissionDecision, PreToolHookOutcome, StopHookOutcome, UserPromptSubmitHookOutcome,
        lifecycle_hook_matches, pre_tool_hook_command_status, tool_hook_matches_checked,
    },
    permissions::PermissionRule,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct HookAdditionalContextEventRun {
    pub(crate) additional_contexts: Vec<String>,
    pub(crate) retry: bool,
    pub(crate) progress: Vec<HookCommandProgress>,
}

#[derive(Debug)]
pub(crate) struct PreToolHookEventRun {
    pub(crate) outcome: Result<PreToolHookOutcome, CoreError>,
    pub(crate) progress: Vec<HookCommandProgress>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PermissionDeniedHookEventRun {
    pub(crate) retry: bool,
    pub(crate) progress: Vec<HookCommandProgress>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StopHookEventRun {
    pub(crate) outcome: StopHookOutcome,
    pub(crate) progress: Vec<HookCommandProgress>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StopFailureHookEventRun {
    pub(crate) progress: Vec<HookCommandProgress>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct UserPromptSubmitHookEventRun {
    pub(crate) outcome: UserPromptSubmitHookOutcome,
    pub(crate) progress: Vec<HookCommandProgress>,
}

fn matcher_source(sources: Option<&[HookSource]>, index: usize) -> HookSource {
    sources
        .and_then(|slice| slice.get(index))
        .copied()
        .unwrap_or_default()
}

pub(crate) async fn run_post_tool_hook_commands(
    context: &HookCommandContext<'_>,
    matchers: Option<&[HookMatcher]>,
    sources: Option<&[HookSource]>,
    tool_use_id: &str,
    tool_name: &str,
    tool_input: &str,
    tool_response: &Value,
) -> HookAdditionalContextEventRun {
    let Some(matchers) = matchers else {
        return HookAdditionalContextEventRun::default();
    };
    let mut run = HookAdditionalContextEventRun::default();
    for (idx, matcher) in matchers.iter().enumerate() {
        // Fail closed on an invalid matcher: unlike PreToolUse (which denies the
        // tool), a post-tool / permission-denied hook cannot grant permission, so
        // the safe resolution is to NOT run its side-effecting command for a
        // matcher whose scope is unknowable. A blanket "matches" would run the
        // command for every tool (a hook-scope error).
        if !matches!(
            tool_hook_matches_checked(matcher, tool_name, tool_input),
            Ok(true)
        ) {
            continue;
        }
        let source = matcher_source(sources, idx);
        for hook in &matcher.hooks {
            let Some((command, condition, timeout)) = command_hook_parts(hook) else {
                continue;
            };
            if let Some(condition) = condition
                && !PermissionRule::parse(condition).matches_tool_call(tool_name, tool_input)
            {
                continue;
            }
            let started = Instant::now();
            let result = run_post_tool_command_hook(
                context,
                tool_use_id,
                tool_name,
                tool_input,
                tool_response,
                command,
                timeout,
                source,
            )
            .await;
            run.progress.push(progress(
                "PostToolUse",
                command,
                result.status,
                result.exit_code,
                result.error.as_deref(),
                started,
            ));
            if let Some(additional_context) = result.additional_context {
                run.additional_contexts.push(additional_context);
            }
        }
    }
    run
}

pub(crate) async fn run_post_tool_failure_hook_commands(
    context: &HookCommandContext<'_>,
    matchers: Option<&[HookMatcher]>,
    sources: Option<&[HookSource]>,
    tool_use_id: &str,
    tool_name: &str,
    tool_input: &str,
    error: &str,
    is_interrupt: bool,
) -> HookAdditionalContextEventRun {
    let Some(matchers) = matchers else {
        return HookAdditionalContextEventRun::default();
    };
    let mut run = HookAdditionalContextEventRun::default();
    for (idx, matcher) in matchers.iter().enumerate() {
        // Fail closed on an invalid matcher: unlike PreToolUse (which denies the
        // tool), a post-tool / permission-denied hook cannot grant permission, so
        // the safe resolution is to NOT run its side-effecting command for a
        // matcher whose scope is unknowable. A blanket "matches" would run the
        // command for every tool (a hook-scope error).
        if !matches!(
            tool_hook_matches_checked(matcher, tool_name, tool_input),
            Ok(true)
        ) {
            continue;
        }
        let source = matcher_source(sources, idx);
        for hook in &matcher.hooks {
            let Some((command, condition, timeout)) = command_hook_parts(hook) else {
                continue;
            };
            if let Some(condition) = condition
                && !PermissionRule::parse(condition).matches_tool_call(tool_name, tool_input)
            {
                continue;
            }
            let started = Instant::now();
            let result = run_post_tool_failure_command_hook(
                context,
                tool_use_id,
                tool_name,
                tool_input,
                error,
                is_interrupt,
                command,
                timeout,
                source,
            )
            .await;
            run.progress.push(progress(
                "PostToolUseFailure",
                command,
                result.status,
                result.exit_code,
                result.error.as_deref(),
                started,
            ));
            if let Some(additional_context) = result.additional_context {
                run.additional_contexts.push(additional_context);
            }
            if result.retry {
                run.retry = true;
            }
        }
    }
    run
}

pub(crate) async fn run_user_prompt_submit_hook_commands(
    context: &HookCommandContext<'_>,
    matchers: Option<&[HookMatcher]>,
    sources: Option<&[HookSource]>,
    prompt: &str,
) -> UserPromptSubmitHookEventRun {
    let Some(matchers) = matchers else {
        return UserPromptSubmitHookEventRun::default();
    };
    let mut run = UserPromptSubmitHookEventRun::default();
    for (idx, matcher) in matchers.iter().enumerate() {
        if !lifecycle_hook_matches(matcher, None) {
            continue;
        }
        let source = matcher_source(sources, idx);
        for hook in &matcher.hooks {
            let Some((command, _, timeout)) = command_hook_parts(hook) else {
                continue;
            };
            let started = Instant::now();
            let result =
                run_user_prompt_submit_command_hook(context, prompt, command, timeout, source)
                    .await;
            run.progress.push(progress(
                "UserPromptSubmit",
                command,
                result.status,
                result.exit_code,
                result.error.as_deref(),
                started,
            ));
            run.outcome.merge(result.outcome);
        }
    }
    run
}

pub(crate) async fn run_permission_denied_hook_commands(
    context: &HookCommandContext<'_>,
    matchers: Option<&[HookMatcher]>,
    sources: Option<&[HookSource]>,
    tool_use_id: &str,
    tool_name: &str,
    tool_input: &str,
    reason: &str,
) -> PermissionDeniedHookEventRun {
    let Some(matchers) = matchers else {
        return PermissionDeniedHookEventRun::default();
    };
    let mut run = PermissionDeniedHookEventRun::default();
    for (idx, matcher) in matchers.iter().enumerate() {
        // Fail closed on an invalid matcher: unlike PreToolUse (which denies the
        // tool), a post-tool / permission-denied hook cannot grant permission, so
        // the safe resolution is to NOT run its side-effecting command for a
        // matcher whose scope is unknowable. A blanket "matches" would run the
        // command for every tool (a hook-scope error).
        if !matches!(
            tool_hook_matches_checked(matcher, tool_name, tool_input),
            Ok(true)
        ) {
            continue;
        }
        let source = matcher_source(sources, idx);
        for hook in &matcher.hooks {
            let Some((command, condition, timeout)) = command_hook_parts(hook) else {
                continue;
            };
            if let Some(condition) = condition
                && !PermissionRule::parse(condition).matches_tool_call(tool_name, tool_input)
            {
                continue;
            }
            let started = Instant::now();
            let result = run_permission_denied_command_hook(
                context,
                tool_use_id,
                tool_name,
                tool_input,
                reason,
                command,
                timeout,
                source,
            )
            .await;
            run.progress.push(progress(
                "PermissionDenied",
                command,
                result.status,
                result.exit_code,
                result.error.as_deref(),
                started,
            ));
            if result.retry {
                run.retry = true;
            }
        }
    }
    run
}

pub(crate) async fn run_stop_hook_commands(
    context: &HookCommandContext<'_>,
    matchers: Option<&[HookMatcher]>,
    sources: Option<&[HookSource]>,
    last_assistant_message: &str,
    stop_hook_active: bool,
) -> StopHookEventRun {
    let Some(matchers) = matchers else {
        return StopHookEventRun::default();
    };
    let mut run = StopHookEventRun::default();
    for (idx, matcher) in matchers.iter().enumerate() {
        if !lifecycle_hook_matches(matcher, None) {
            continue;
        }
        let source = matcher_source(sources, idx);
        for hook in &matcher.hooks {
            let Some((command, _, timeout)) = command_hook_parts(hook) else {
                continue;
            };
            let started = Instant::now();
            let result = run_stop_command_hook(
                context,
                last_assistant_message,
                stop_hook_active,
                command,
                timeout,
                source,
            )
            .await;
            run.progress.push(progress(
                "Stop",
                command,
                result.status,
                result.exit_code,
                result.error.as_deref(),
                started,
            ));
            run.outcome.merge(result.outcome);
        }
    }
    run
}

pub(crate) async fn run_stop_failure_hook_commands(
    context: &HookCommandContext<'_>,
    matchers: Option<&[HookMatcher]>,
    sources: Option<&[HookSource]>,
    error: &str,
    error_details: &str,
    last_assistant_message: Option<&str>,
) -> StopFailureHookEventRun {
    let Some(matchers) = matchers else {
        return StopFailureHookEventRun::default();
    };
    let mut run = StopFailureHookEventRun::default();
    for (idx, matcher) in matchers.iter().enumerate() {
        if !lifecycle_hook_matches(matcher, Some(error)) {
            continue;
        }
        let source = matcher_source(sources, idx);
        for hook in &matcher.hooks {
            let Some((command, _, timeout)) = command_hook_parts(hook) else {
                continue;
            };
            let started = Instant::now();
            let result = run_stop_failure_command_hook(
                context,
                error,
                error_details,
                last_assistant_message,
                command,
                timeout,
                source,
            )
            .await;
            run.progress.push(progress(
                "StopFailure",
                command,
                result.status,
                result.exit_code,
                result.error.as_deref(),
                started,
            ));
        }
    }
    run
}

pub(crate) async fn run_subagent_start_hook_commands(
    context: &HookCommandContext<'_>,
    matchers: Option<&[HookMatcher]>,
    sources: Option<&[HookSource]>,
    agent_id: &str,
    agent_type: &str,
) -> HookAdditionalContextEventRun {
    let Some(matchers) = matchers else {
        return HookAdditionalContextEventRun::default();
    };
    let mut run = HookAdditionalContextEventRun::default();
    for (idx, matcher) in matchers.iter().enumerate() {
        if !lifecycle_hook_matches(matcher, Some(agent_type)) {
            continue;
        }
        let source = matcher_source(sources, idx);
        for hook in &matcher.hooks {
            let Some((command, _, timeout)) = command_hook_parts(hook) else {
                continue;
            };
            let started = Instant::now();
            let result = run_subagent_start_command_hook(
                context, agent_id, agent_type, command, timeout, source,
            )
            .await;
            run.progress.push(progress(
                "SubagentStart",
                command,
                result.status,
                result.exit_code,
                result.error.as_deref(),
                started,
            ));
            if let Some(additional_context) = result.additional_context {
                run.additional_contexts.push(additional_context);
            }
        }
    }
    run
}

pub(crate) async fn run_subagent_stop_hook_commands(
    context: &HookCommandContext<'_>,
    matchers: Option<&[HookMatcher]>,
    sources: Option<&[HookSource]>,
    agent_id: &str,
    child_session_id: &str,
    agent_type: &str,
    last_assistant_message: &str,
    stop_hook_active: bool,
) -> StopHookEventRun {
    let Some(matchers) = matchers else {
        return StopHookEventRun::default();
    };
    let mut run = StopHookEventRun::default();
    for (idx, matcher) in matchers.iter().enumerate() {
        // Match against the agent type (as SubagentStart does). Passing `None`
        // meant a matcher-scoped SubagentStop hook (e.g. `matcher: "Explore"`)
        // never fired — only unscoped (`*`/empty) matchers did.
        if !lifecycle_hook_matches(matcher, Some(agent_type)) {
            continue;
        }
        let source = matcher_source(sources, idx);
        for hook in &matcher.hooks {
            let Some((command, _, timeout)) = command_hook_parts(hook) else {
                continue;
            };
            let started = Instant::now();
            let result = run_subagent_stop_command_hook(
                context,
                agent_id,
                child_session_id,
                agent_type,
                last_assistant_message,
                stop_hook_active,
                command,
                timeout,
                source,
            )
            .await;
            run.progress.push(progress(
                "SubagentStop",
                command,
                result.status,
                result.exit_code,
                result.error.as_deref(),
                started,
            ));
            run.outcome.merge(result.outcome);
        }
    }
    run
}

pub(crate) async fn run_pre_tool_hook_commands(
    context: &HookCommandContext<'_>,
    matchers: Option<&[HookMatcher]>,
    sources: Option<&[HookSource]>,
    tool_use_id: &str,
    tool_name: &str,
    tool_input: &str,
) -> PreToolHookEventRun {
    let mut outcome = PreToolHookOutcome {
        tool_input: tool_input.to_string(),
        decision: None,
        reason: None,
        additional_contexts: Vec::new(),
    };
    let Some(matchers) = matchers else {
        return PreToolHookEventRun {
            outcome: Ok(outcome),
            progress: Vec::new(),
        };
    };
    let mut progress_records = Vec::new();
    for (idx, matcher) in matchers.iter().enumerate() {
        match tool_hook_matches_checked(matcher, tool_name, &outcome.tool_input) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(reason) => {
                // An unparseable matcher must fail CLOSED: we cannot tell whether
                // its hidden hook was an `allow` (a spurious match would authorize
                // the tool) or a `block` (a spurious skip would drop the guard), so
                // deny the tool and surface the config error instead of guessing.
                outcome.decision = Some(HookPermissionDecision::Deny);
                outcome.reason = Some(format!("invalid PreToolUse hook matcher: {reason}"));
                return PreToolHookEventRun {
                    outcome: Ok(outcome),
                    progress: progress_records,
                };
            }
        }
        let source = matcher_source(sources, idx);
        for hook in &matcher.hooks {
            let Some((command, condition, timeout)) = command_hook_parts(hook) else {
                continue;
            };
            if let Some(condition) = condition
                && !PermissionRule::parse(condition)
                    .matches_tool_call(tool_name, &outcome.tool_input)
            {
                continue;
            }
            let started = Instant::now();
            let result = run_pre_tool_command_hook(
                context,
                tool_use_id,
                tool_name,
                &outcome.tool_input,
                command,
                timeout,
                source,
            )
            .await;
            let (status, exit_code) = match &result {
                Ok(Some(result)) => (pre_tool_hook_command_status(result), None),
                Ok(None) => ("completed", None),
                Err(_) => ("failed", None),
            };
            progress_records.push(progress(
                "PreToolUse",
                command,
                status,
                exit_code,
                None,
                started,
            ));
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    return PreToolHookEventRun {
                        outcome: Err(error),
                        progress: progress_records,
                    };
                }
            };
            let Some(result) = result else {
                continue;
            };
            if let Some(updated_input) = result.updated_input
                && !matches!(result.decision, Some(HookPermissionDecision::Deny))
            {
                outcome.tool_input = updated_input;
            }
            if let Some(reason) = result.reason {
                outcome.reason = Some(reason);
            }
            if let Some(additional_context) = result.additional_context {
                outcome.additional_contexts.push(additional_context);
            }
            match result.decision {
                Some(HookPermissionDecision::Deny) => {
                    outcome.decision = Some(HookPermissionDecision::Deny);
                    return PreToolHookEventRun {
                        outcome: Ok(outcome),
                        progress: progress_records,
                    };
                }
                Some(HookPermissionDecision::Ask) => {
                    if !matches!(outcome.decision, Some(HookPermissionDecision::Deny)) {
                        outcome.decision = Some(HookPermissionDecision::Ask);
                    }
                }
                Some(HookPermissionDecision::Allow) if outcome.decision.is_none() => {
                    outcome.decision = Some(HookPermissionDecision::Allow);
                }
                _ => {}
            }
        }
    }
    PreToolHookEventRun {
        outcome: Ok(outcome),
        progress: progress_records,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_config::HookCommand;
    use orbcode_session_store::SessionStore;

    fn matcher_with_allow_hook(pattern: &str) -> HookMatcher {
        HookMatcher {
            matcher: Some(pattern.to_string()),
            hooks: vec![HookCommand::Command {
                // A hook that would emit an `allow` decision if it ever ran.
                command: "printf '{\"decision\":\"approve\"}'".to_string(),
                r#if: None,
                timeout: None,
            }],
        }
    }

    #[tokio::test]
    async fn invalid_pre_tool_matcher_denies_before_running_any_hook() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            "test-model".to_string(),
        );
        let context = HookCommandContext::new("session", &store, dir.path());
        // `(` never closes — invalid in JavaScript too. Even though the hook
        // would emit `allow`, the tool must be DENIED (fail closed), not
        // authorized, and no command should have run (no progress records).
        let matchers = [matcher_with_allow_hook("(")];
        let run = run_pre_tool_hook_commands(
            &context,
            Some(&matchers),
            None,
            "tool-use-id",
            "Bash",
            "{}",
        )
        .await;
        let outcome = run.outcome.expect("outcome");
        assert_eq!(outcome.decision, Some(HookPermissionDecision::Deny));
        assert!(
            outcome
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("invalid PreToolUse hook matcher")),
            "{:?}",
            outcome.reason
        );
        assert!(
            run.progress.is_empty(),
            "no hook command should run for an invalid matcher"
        );
    }

    #[tokio::test]
    async fn invalid_matcher_does_not_run_post_tool_hook_for_any_tool() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            "test-model".to_string(),
        );
        let context = HookCommandContext::new("session", &store, dir.path());
        // A PostToolUse hook cannot grant permission, but a side-effecting
        // command must still NOT run for an invalid matcher — otherwise `(`
        // would fire it for every tool (a hook-scope error). Fail closed: skip.
        let matchers = [matcher_with_allow_hook("(")];
        let run = run_post_tool_hook_commands(
            &context,
            Some(&matchers),
            None,
            "tool-use-id",
            "Bash",
            "{}",
            &serde_json::json!({}),
        )
        .await;
        assert!(
            run.progress.is_empty(),
            "an invalid matcher must not run a PostToolUse command"
        );
        assert!(run.additional_contexts.is_empty());
    }
}
