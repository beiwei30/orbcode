use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::{
    ToolContext, ToolError, ToolOutcome, ToolRegistry, WorkspacePlanSnapshot,
    payload::{parse_payload, string_field},
    process::run_command_output,
    task_tools::sanitize_task_list_id,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct PlanModeState {
    pub(crate) workspace_id: String,
    pub(crate) plan_file: String,
    pub(crate) in_plan_mode: bool,
    pub(crate) entered_at: String,
    pub(crate) exited_at: Option<String>,
}

impl ToolRegistry {
    pub(crate) async fn enter_plan_mode(
        &self,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let plan_path = workspace_plan_file_path(context);
        let mut state = load_plan_mode_state(context)
            .await?
            .unwrap_or_else(|| PlanModeState {
                workspace_id: workspace_plan_workspace_id(context),
                plan_file: plan_path.display().to_string(),
                in_plan_mode: false,
                entered_at: Utc::now().to_rfc3339(),
                exited_at: None,
            });

        ensure_plan_mode_paths(context).await?;
        if !tokio::fs::try_exists(&plan_path).await? {
            tokio::fs::write(
                &plan_path,
                concat!(
                    "# Plan\n\n",
                    "## Problem\n\n",
                    "Describe the problem to solve.\n\n",
                    "## Approach\n\n",
                    "Describe the intended implementation approach.\n\n",
                    "## Steps\n\n",
                    "1. Inspect relevant code paths.\n",
                    "2. Make the required changes.\n",
                    "3. Validate the result.\n"
                ),
            )
            .await?;
        }
        state.in_plan_mode = true;
        state.entered_at = Utc::now().to_rfc3339();
        state.exited_at = None;
        state.plan_file = plan_path.display().to_string();
        save_plan_mode_state(context, &state).await?;

        Ok(ToolOutcome {
            name: "enter-plan-mode".to_string(),
            summary: "Entered plan mode.".to_string(),
            output: format!(
                "Entered plan mode for this workspace.\n\nPlan file: {}\n\nIn plan mode:\n1. Explore the codebase and gather context.\n2. Write or update the plan file above.\n3. Do not edit non-plan files until you call ExitPlanMode.",
                plan_path.display()
            ),
            metadata: None,
            changed_paths: vec![plan_path, workspace_plan_state_path(context)],
        })
    }

    pub(crate) async fn exit_plan_mode(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let state = load_plan_mode_state(context).await?;
        if !state.as_ref().is_some_and(|s| s.in_plan_mode) {
            return Err(ToolError::InvalidInput(
                "You are not in plan mode. This tool is only for exiting plan mode after writing a plan. If your plan was already approved, continue with implementation.".into(),
            ));
        }
        let payload = parse_payload(input)?;
        let plan_path = workspace_plan_file_path(context);
        ensure_plan_mode_paths(context).await?;
        let plan_contents = if let Some(plan) = string_field(&payload, "plan") {
            tokio::fs::write(&plan_path, &plan).await?;
            plan
        } else if tokio::fs::try_exists(&plan_path).await? {
            tokio::fs::read_to_string(&plan_path).await?
        } else {
            String::new()
        };

        let mut state = load_plan_mode_state(context)
            .await?
            .unwrap_or_else(|| PlanModeState {
                workspace_id: workspace_plan_workspace_id(context),
                plan_file: plan_path.display().to_string(),
                in_plan_mode: false,
                entered_at: Utc::now().to_rfc3339(),
                exited_at: None,
            });
        state.in_plan_mode = false;
        state.plan_file = plan_path.display().to_string();
        state.exited_at = Some(Utc::now().to_rfc3339());
        save_plan_mode_state(context, &state).await?;

        let output = if plan_contents.trim().is_empty() {
            format!(
                "Exited plan mode.\n\nPlan file: {}\n\nNo plan content was found. You may proceed, but writing a concrete plan first is recommended.",
                plan_path.display()
            )
        } else {
            format!(
                "Exited plan mode.\n\nPlan file: {}\n\nPlan:\n{}\n\nYou may now proceed with implementation. Use the approved plan above as the execution checklist.",
                plan_path.display(),
                plan_contents.trim_end()
            )
        };

        Ok(ToolOutcome {
            name: "exit-plan-mode".to_string(),
            summary: "Exited plan mode.".to_string(),
            output,
            metadata: None,
            changed_paths: vec![plan_path, workspace_plan_state_path(context)],
        })
    }

    pub(crate) async fn verify_plan_execution(
        &self,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        ensure_plan_mode_paths(context).await?;
        let plan_path = workspace_plan_file_path(context);
        let state = load_plan_mode_state(context).await?;
        let plan_contents = if tokio::fs::try_exists(&plan_path).await? {
            tokio::fs::read_to_string(&plan_path).await?
        } else {
            String::new()
        };
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&context.cwd)
            .arg("status")
            .arg("--short");
        let git_status = match run_command_output(&mut command, context).await {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if stdout.is_empty() {
                    "working tree clean".to_string()
                } else {
                    stdout
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if stderr.is_empty() {
                    "git status unavailable".to_string()
                } else {
                    format!("git status unavailable: {stderr}")
                }
            }
            Err(error) => format!("git status unavailable: {error}"),
        };

        Ok(ToolOutcome {
            name: "verify-plan-execution".to_string(),
            summary: "Captured a plan execution verification snapshot.".to_string(),
            output: format!(
                "Verification snapshot\n\nPlan file: {}\nPlan present: {}\nPlan mode active: {}\n\nCurrent plan:\n{}\n\nGit status:\n{}\n\nAutomated plan verification is not yet implemented in Orb Code. Use this snapshot to confirm the implementation matches the plan and run the necessary targeted tests.",
                plan_path.display(),
                if plan_contents.trim().is_empty() {
                    "no"
                } else {
                    "yes"
                },
                state.is_some_and(|value| value.in_plan_mode),
                if plan_contents.trim().is_empty() {
                    "(empty plan)".to_string()
                } else {
                    plan_contents.trim_end().to_string()
                },
                git_status
            ),
            metadata: None,
            changed_paths: Vec::new(),
        })
    }
}

fn plans_dir(context: &ToolContext) -> PathBuf {
    if let Some(ref override_dir) = context.plans_directory_override {
        return override_dir.clone();
    }
    context.home_dir.join("plans")
}

fn workspace_plan_workspace_id(context: &ToolContext) -> String {
    sanitize_task_list_id(&context.cwd.display().to_string())
}

pub(crate) fn workspace_plan_file_path(context: &ToolContext) -> PathBuf {
    plans_dir(context).join(format!("{}.md", workspace_plan_workspace_id(context)))
}

fn workspace_plan_state_path(context: &ToolContext) -> PathBuf {
    plans_dir(context).join(format!("{}.json", workspace_plan_workspace_id(context)))
}

async fn ensure_plan_mode_paths(context: &ToolContext) -> Result<(), ToolError> {
    tokio::fs::create_dir_all(plans_dir(context)).await?;
    Ok(())
}

pub(crate) async fn load_plan_mode_state(
    context: &ToolContext,
) -> Result<Option<PlanModeState>, ToolError> {
    let path = workspace_plan_state_path(context);
    if !tokio::fs::try_exists(&path).await? {
        return Ok(None);
    }
    let contents = tokio::fs::read_to_string(path).await?;
    Ok(Some(serde_json::from_str(&contents)?))
}

pub async fn workspace_plan_snapshot(
    context: &ToolContext,
) -> Result<WorkspacePlanSnapshot, ToolError> {
    let plan_file = workspace_plan_file_path(context);
    let state_file = workspace_plan_state_path(context);
    let state = load_plan_mode_state(context).await?;
    let plan = if tokio::fs::try_exists(&plan_file).await? {
        tokio::fs::read_to_string(&plan_file).await?
    } else {
        String::new()
    };
    Ok(WorkspacePlanSnapshot {
        plan_file,
        state_file,
        in_plan_mode: state.is_some_and(|value| value.in_plan_mode),
        plan,
    })
}

async fn save_plan_mode_state(
    context: &ToolContext,
    state: &PlanModeState,
) -> Result<(), ToolError> {
    ensure_plan_mode_paths(context).await?;
    tokio::fs::write(
        workspace_plan_state_path(context),
        serde_json::to_string_pretty(state)?,
    )
    .await?;
    Ok(())
}
