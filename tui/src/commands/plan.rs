use anyhow::Result;
use orbcode_app_server_client::{AppClient, PlanOverview};

use crate::render::slash_output::render_plan_overview;

pub(crate) struct PlanCommandResult {
    pub(crate) command: String,
    pub(crate) output: String,
    pub(crate) status: String,
    pub(crate) submit_prompt: Option<String>,
}

pub(crate) async fn run_plan_slash_command(
    client: &AppClient,
    args: String,
) -> Result<PlanCommandResult> {
    let value = client
        .plan_overview()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let overview: PlanOverview =
        serde_json::from_value(value).map_err(|e| anyhow::anyhow!("plan overview parse: {e}"))?;
    let args = args.trim();
    let command = if args.is_empty() {
        "/plan".to_string()
    } else {
        format!("/plan {args}")
    };
    if !overview.in_plan_mode {
        let enter_value = client
            .enter_plan_mode()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut output = enter_value["output"].as_str().unwrap_or("").to_string();
        let mut submit_prompt = None;
        if !args.is_empty() && args != "open" {
            output.push_str("\n\nPlan focus:\n");
            output.push_str(args);
            output.push_str("\n\nStarting a planning turn with this prompt.");
            submit_prompt = Some(args.to_string());
        }
        return Ok(PlanCommandResult {
            command,
            output,
            status: "Plan mode enabled.".to_string(),
            submit_prompt,
        });
    }

    if args == "open" {
        return Ok(PlanCommandResult {
            command,
            output: format!(
                "Current Plan\n{}\n\nOpening the plan file in $EDITOR is not implemented in the Rust TUI yet.",
                overview.plan_file.display()
            ),
            status: "Plan file path shown.".to_string(),
            submit_prompt: None,
        });
    }

    Ok(PlanCommandResult {
        command,
        output: render_plan_overview(&overview),
        status: "Plan loaded.".to_string(),
        submit_prompt: None,
    })
}
