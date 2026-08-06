use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use orbcode_app_server_client::{
    GoalContinuation, SessionGoalNotStartedReason, SessionGoalSetParams,
};
use orbcode_protocol::{SessionGoal, SessionGoalStatus};

use crate::commands::dispatch::SlashCommandOutcome;
use crate::commands::{CommandContext, SlashCommand};

pub(crate) static GOAL: GoalCommand = GoalCommand;

pub(crate) struct GoalCommand;

impl SlashCommand for GoalCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move { run_goal_command(ctx).await })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum GoalAction {
    Show,
    Create {
        objective: String,
        token_budget: Option<Option<u64>>,
    },
    Edit {
        objective: String,
        token_budget: Option<Option<u64>>,
    },
    Pause,
    Resume,
    Clear,
    Budget(Option<u64>),
}

async fn run_goal_command(mut ctx: CommandContext<'_>) -> Result<SlashCommandOutcome> {
    match parse_goal_action(ctx.args)? {
        GoalAction::Show => {
            let goal = ctx
                .app_server
                .get_goal(&ctx.state.session_id)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            render_goal(&mut ctx, goal.as_ref(), "Persistent goal loaded.");
            Ok(SlashCommandOutcome::Handled)
        }
        GoalAction::Create {
            objective,
            token_budget,
        } => {
            let current = ctx
                .app_server
                .get_goal(&ctx.state.session_id)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            if current
                .as_ref()
                .is_some_and(|goal| goal.status != SessionGoalStatus::Complete)
            {
                return Err(anyhow::anyhow!(
                    "an unfinished goal already exists; use /goal edit or /goal clear"
                ));
            }
            let goal = ctx
                .app_server
                .set_goal(SessionGoalSetParams {
                    session_id: ctx.state.session_id.clone(),
                    expected_revision: current.as_ref().map(|goal| goal.revision),
                    replace: current.is_some(),
                    objective: Some(objective),
                    status: Some(SessionGoalStatus::Active),
                    token_budget,
                })
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            render_goal(&mut ctx, Some(&goal), "Persistent goal created.");
            continue_goal(ctx, goal).await
        }
        GoalAction::Edit {
            objective,
            token_budget,
        } => {
            let current = require_goal(&ctx).await?;
            let resume_after_budget_change = current.status == SessionGoalStatus::BudgetLimited
                && budget_allows_resume(token_budget, &current);
            let goal = ctx
                .app_server
                .set_goal(SessionGoalSetParams {
                    session_id: ctx.state.session_id.clone(),
                    expected_revision: Some(current.revision),
                    replace: false,
                    objective: Some(objective),
                    status: resume_after_budget_change.then_some(SessionGoalStatus::Active),
                    token_budget,
                })
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            render_goal(&mut ctx, Some(&goal), "Persistent goal updated.");
            continue_if_active(ctx, goal).await
        }
        GoalAction::Pause => {
            let current = require_goal(&ctx).await?;
            if current.status != SessionGoalStatus::Active {
                return Err(anyhow::anyhow!("only an active goal can be paused"));
            }
            let goal = ctx
                .app_server
                .set_goal(SessionGoalSetParams {
                    session_id: ctx.state.session_id.clone(),
                    expected_revision: Some(current.revision),
                    replace: false,
                    objective: None,
                    status: Some(SessionGoalStatus::Paused),
                    token_budget: None,
                })
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let _ = ctx.app_server.interrupt_turn(&ctx.state.session_id).await;
            render_goal(&mut ctx, Some(&goal), "Persistent goal paused.");
            Ok(SlashCommandOutcome::Handled)
        }
        GoalAction::Resume => {
            let current = require_goal(&ctx).await?;
            let goal = if current.status == SessionGoalStatus::Active {
                current
            } else {
                ctx.app_server
                    .set_goal(SessionGoalSetParams {
                        session_id: ctx.state.session_id.clone(),
                        expected_revision: Some(current.revision),
                        replace: false,
                        objective: None,
                        status: Some(SessionGoalStatus::Active),
                        token_budget: None,
                    })
                    .await
                    .map_err(|error| anyhow::anyhow!("{error}"))?
            };
            render_goal(&mut ctx, Some(&goal), "Persistent goal resumed.");
            continue_goal(ctx, goal).await
        }
        GoalAction::Clear => {
            let _ = ctx.app_server.interrupt_turn(&ctx.state.session_id).await;
            let cleared = ctx
                .app_server
                .clear_goal(&ctx.state.session_id)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let summary = if cleared {
                "Persistent goal cleared."
            } else {
                "No persistent goal to clear."
            };
            ctx.state
                .push_local_slash_command_output(ctx.line, summary, None);
            ctx.state.set_status_line(summary);
            Ok(SlashCommandOutcome::Handled)
        }
        GoalAction::Budget(token_budget) => {
            let current = require_goal(&ctx).await?;
            let budget_update = Some(token_budget);
            let resume = current.status == SessionGoalStatus::BudgetLimited
                && budget_allows_resume(budget_update, &current);
            let goal = ctx
                .app_server
                .set_goal(SessionGoalSetParams {
                    session_id: ctx.state.session_id.clone(),
                    expected_revision: Some(current.revision),
                    replace: false,
                    objective: None,
                    status: resume.then_some(SessionGoalStatus::Active),
                    token_budget: budget_update,
                })
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            render_goal(&mut ctx, Some(&goal), "Persistent goal budget updated.");
            continue_if_active(ctx, goal).await
        }
    }
}

async fn require_goal(ctx: &CommandContext<'_>) -> Result<SessionGoal> {
    ctx.app_server
        .get_goal(&ctx.state.session_id)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .ok_or_else(|| anyhow::anyhow!("no persistent goal exists; use /goal create <objective>"))
}

async fn continue_if_active(
    ctx: CommandContext<'_>,
    goal: SessionGoal,
) -> Result<SlashCommandOutcome> {
    if goal.status == SessionGoalStatus::Active {
        continue_goal(ctx, goal).await
    } else {
        Ok(SlashCommandOutcome::Handled)
    }
}

async fn continue_goal(
    mut ctx: CommandContext<'_>,
    goal: SessionGoal,
) -> Result<SlashCommandOutcome> {
    match ctx
        .app_server
        .continue_goal(&ctx.state.session_id, &goal.goal_id, goal.revision)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?
    {
        GoalContinuation::Started { events, .. } => {
            ctx.state.set_status_line("Persistent goal turn started...");
            Ok(SlashCommandOutcome::GoalTurnStarted(events))
        }
        GoalContinuation::NotStarted { reason, goal } => {
            let summary = format!(
                "Persistent goal did not start: {}.",
                not_started_reason(reason)
            );
            render_goal(&mut ctx, goal.as_ref(), &summary);
            Ok(SlashCommandOutcome::Handled)
        }
    }
}

fn render_goal(ctx: &mut CommandContext<'_>, goal: Option<&SessionGoal>, summary: &str) {
    let detail = goal.map(format_goal);
    ctx.state
        .push_local_slash_command_output(ctx.line, summary, detail);
    ctx.state.set_status_line(summary);
}

fn format_goal(goal: &SessionGoal) -> String {
    let budget = goal.token_budget.map_or_else(
        || format!("{} / unlimited", goal.tokens_used),
        |budget| format!("{} / {budget}", goal.tokens_used),
    );
    let mut lines = vec![
        format!("Objective: {}", goal.objective),
        format!("Status: {}", goal_status(goal.status)),
        format!("Tokens: {budget}"),
        format!("Elapsed: {}s", goal.elapsed_seconds),
        format!("Revision: {}", goal.revision),
    ];
    if let Some(reason) = goal.stop_reason.as_deref() {
        lines.push(format!("Stop reason: {reason}"));
    }
    lines.join("\n")
}

fn goal_status(status: SessionGoalStatus) -> &'static str {
    match status {
        SessionGoalStatus::Active => "active",
        SessionGoalStatus::Paused => "paused",
        SessionGoalStatus::Blocked => "blocked",
        SessionGoalStatus::UsageLimited => "usage limited",
        SessionGoalStatus::BudgetLimited => "budget limited",
        SessionGoalStatus::Complete => "complete",
        _ => "unknown",
    }
}

fn not_started_reason(reason: SessionGoalNotStartedReason) -> &'static str {
    match reason {
        SessionGoalNotStartedReason::Missing => "no goal exists",
        SessionGoalNotStartedReason::StaleRevision => "the goal changed; retry",
        SessionGoalNotStartedReason::Inactive => "the goal is not active",
        SessionGoalNotStartedReason::UsageLimited => "provider usage is limited",
        SessionGoalNotStartedReason::BudgetLimited => "the token budget is exhausted",
        SessionGoalNotStartedReason::PendingUserInput => "user input is pending",
        SessionGoalNotStartedReason::ActiveTurn => "another turn is active",
        SessionGoalNotStartedReason::ClientNotCapable => "this client cannot supervise goals",
    }
}

fn budget_allows_resume(update: Option<Option<u64>>, goal: &SessionGoal) -> bool {
    match update {
        Some(None) => true,
        Some(Some(budget)) => {
            budget > goal.tokens_used
                && goal
                    .token_budget
                    .is_none_or(|old_budget| budget > old_budget)
        }
        None => false,
    }
}

fn parse_goal_action(args: &str) -> Result<GoalAction> {
    let args = args.trim();
    if args.is_empty() || args == "show" {
        return Ok(GoalAction::Show);
    }
    let (command, rest) = split_first(args);
    match command {
        "create" => {
            let (token_budget, objective) = parse_objective_and_budget(rest)?;
            Ok(GoalAction::Create {
                objective,
                token_budget,
            })
        }
        "edit" => {
            let (token_budget, objective) = parse_objective_and_budget(rest)?;
            Ok(GoalAction::Edit {
                objective,
                token_budget,
            })
        }
        "pause" if rest.is_empty() => Ok(GoalAction::Pause),
        "resume" if rest.is_empty() => Ok(GoalAction::Resume),
        "clear" if rest.is_empty() => Ok(GoalAction::Clear),
        "budget" => {
            let value = parse_budget_value(rest)?;
            Ok(GoalAction::Budget(value))
        }
        "show" | "pause" | "resume" | "clear" => Err(goal_usage()),
        _ => {
            let (token_budget, objective) = parse_objective_and_budget(args)?;
            Ok(GoalAction::Create {
                objective,
                token_budget,
            })
        }
    }
}

fn parse_objective_and_budget(args: &str) -> Result<(Option<Option<u64>>, String)> {
    let mut rest = args.trim();
    let mut token_budget = None;
    loop {
        if let Some(after_flag) = rest.strip_prefix("--budget")
            && after_flag.starts_with(char::is_whitespace)
        {
            let (value, remaining) = split_first(after_flag.trim_start());
            token_budget = Some(parse_budget_value(value)?);
            rest = remaining;
            continue;
        }
        if let Some(remaining) = rest.strip_prefix("--no-budget")
            && (remaining.is_empty() || remaining.starts_with(char::is_whitespace))
        {
            token_budget = Some(None);
            rest = remaining.trim_start();
            continue;
        }
        break;
    }
    let objective = rest.trim();
    if objective.is_empty() {
        return Err(goal_usage());
    }
    Ok((token_budget, objective.to_string()))
}

fn parse_budget_value(value: &str) -> Result<Option<u64>> {
    let value = value.trim();
    if value == "none" {
        return Ok(None);
    }
    let budget = value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(goal_usage)?;
    Ok(Some(budget))
}

fn split_first(value: &str) -> (&str, &str) {
    value
        .split_once(char::is_whitespace)
        .map_or((value, ""), |(first, rest)| (first, rest.trim_start()))
}

fn goal_usage() -> anyhow::Error {
    anyhow::anyhow!(
        "usage: /goal [show|create [--budget N] <objective>|edit [--budget N|--no-budget] <objective>|pause|resume|clear|budget N|none]"
    )
}

#[cfg(test)]
mod tests {
    use super::{GoalAction, parse_goal_action};

    #[test]
    fn parses_goal_command_forms() {
        assert_eq!(parse_goal_action("").unwrap(), GoalAction::Show);
        assert_eq!(parse_goal_action("show").unwrap(), GoalAction::Show);
        assert_eq!(
            parse_goal_action("--budget 1200 ship the feature").unwrap(),
            GoalAction::Create {
                objective: "ship the feature".to_string(),
                token_budget: Some(Some(1200)),
            }
        );
        assert_eq!(
            parse_goal_action("create test everything").unwrap(),
            GoalAction::Create {
                objective: "test everything".to_string(),
                token_budget: None,
            }
        );
        assert_eq!(
            parse_goal_action("edit --no-budget refine objective").unwrap(),
            GoalAction::Edit {
                objective: "refine objective".to_string(),
                token_budget: Some(None),
            }
        );
        assert_eq!(
            parse_goal_action("budget none").unwrap(),
            GoalAction::Budget(None)
        );
        assert!(parse_goal_action("budget 0").is_err());
        assert!(parse_goal_action("pause now").is_err());
    }
}
