use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;

use anyhow::Result;
use tokio::process::Command;

use crate::commands::dispatch::SlashCommandOutcome;
use crate::commands::{CommandContext, SlashCommand};

pub(crate) static BRANCH: BranchCommand = BranchCommand;

pub(crate) struct BranchCommand;

const BRANCH_SUGGEST_PROMPT: &str = "\
Based on our conversation so far, suggest a short, descriptive git branch name \
for the work we're doing. Use kebab-case (lowercase with hyphens). \
Reply with ONLY the branch name, nothing else.";

impl SlashCommand for BranchCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            let name = ctx.args.trim();
            if name.is_empty() {
                ctx.state.push_local_slash_command_output(
                    ctx.line,
                    "Asking the model to suggest a branch name...",
                    None,
                );
                return Ok(SlashCommandOutcome::PromptToSubmit(
                    BRANCH_SUGGEST_PROMPT.to_string(),
                ));
            }

            let cwd = ctx.state.cwd.clone();
            let output = Command::new("git")
                .args(["checkout", "-b", name])
                .current_dir(&cwd)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await?;

            if output.status.success() {
                let message = format!("Created and switched to branch {name}");
                ctx.state
                    .push_local_slash_command_output(ctx.line, message.clone(), None);
                ctx.state.set_status_line(message);
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let detail = stderr.trim();
                let message = format!("Failed to create branch: {detail}");
                ctx.state
                    .push_local_slash_command_output(ctx.line, message.clone(), None);
                ctx.state.set_status_line(message);
            }
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_suggest_prompt_is_nonempty() {
        assert!(!BRANCH_SUGGEST_PROMPT.is_empty());
    }
}
