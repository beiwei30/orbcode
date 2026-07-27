use anyhow::Result;
use orbcode_app_server_client::AppClient;
use orbcode_protocol::EffortLevel;

use crate::overlays::{EffortChoice, EffortCycleDirection};

pub(crate) async fn set_effort_override_message(
    app_server: &AppClient,
    session_id: &str,
    effort: Option<EffortLevel>,
) -> Result<String> {
    match effort {
        Some(effort) => {
            app_server
                .set_effort_override(session_id, Some(effort.as_str()))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(format!(
                "Set effort level to {effort}: {}",
                effort.description()
            ))
        }
        None => {
            app_server
                .set_effort_override(session_id, None)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok("Effort level set to auto.".to_string())
        }
    }
}

pub(crate) fn next_effort_choice(effort: Option<EffortLevel>) -> EffortChoice {
    EffortChoice::from_effort(effort).cycle(EffortCycleDirection::Right)
}

pub(crate) async fn run_effort_slash_command(
    app_server: &AppClient,
    session_id: &str,
    args: &str,
) -> Result<String> {
    let arg = args.trim().to_ascii_lowercase();
    if arg.is_empty() || arg == "current" || arg == "status" {
        let value = app_server
            .effort_level()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let level = value["effort"].as_str().and_then(EffortLevel::parse);
        return Ok(match level {
            Some(level) => format!("Current effort level: {level} ({})", level.description()),
            None => "Effort level: auto (currently high)".to_string(),
        });
    }
    if arg == "auto" || arg == "unset" {
        return set_effort_override_message(app_server, session_id, None).await;
    }
    let effort = EffortLevel::parse(&arg)
        .ok_or_else(|| anyhow::anyhow!("usage: /effort [low|medium|high|max|auto]"))?;
    set_effort_override_message(app_server, session_id, Some(effort)).await
}
