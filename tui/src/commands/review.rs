use std::future::Future;
use std::pin::Pin;

use anyhow::Result;

use crate::commands::dispatch::SlashCommandOutcome;
use crate::commands::{CommandContext, SlashCommand};

pub(crate) static REVIEW: ReviewCommand = ReviewCommand;

pub(crate) struct ReviewCommand;

const REVIEW_PROMPT: &str = "\
Review the current diff for correctness bugs. Focus on logic errors, potential \
regressions, security issues, and missing edge-case handling. Provide findings \
as a concise list with file, line, and severity.";

const REVIEW_COMMENT_PROMPT: &str = "\
Review the current diff for correctness bugs. Focus on logic errors, potential \
regressions, security issues, and missing edge-case handling. Post your findings \
as inline PR comments using the appropriate tool. Each comment should reference \
the specific file and line.";

impl SlashCommand for ReviewCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            let args = ctx.args.trim();
            let comment_mode = parse_review_args(args)?;

            let prompt = if comment_mode {
                REVIEW_COMMENT_PROMPT
            } else {
                REVIEW_PROMPT
            };

            let feedback = if comment_mode {
                "Reviewing diff (inline comment mode)..."
            } else {
                "Reviewing diff..."
            };
            ctx.state
                .push_local_slash_command_output(ctx.line, feedback, None);

            Ok(SlashCommandOutcome::PromptToSubmit(prompt.to_string()))
        })
    }
}

fn parse_review_args(args: &str) -> Result<bool> {
    if args.is_empty() {
        return Ok(false);
    }
    let mut comment = false;
    for token in args.split_whitespace() {
        match token {
            "--comment" => comment = true,
            other => {
                return Err(anyhow::anyhow!(
                    "unknown /review argument `{other}`; expected --comment"
                ));
            }
        }
    }
    Ok(comment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_args() {
        assert!(!parse_review_args("").unwrap());
    }

    #[test]
    fn parse_comment_flag() {
        assert!(parse_review_args("--comment").unwrap());
    }

    #[test]
    fn parse_unknown_arg_errors() {
        assert!(parse_review_args("--unknown").is_err());
    }

    #[test]
    fn review_prompts_are_nonempty() {
        assert!(!REVIEW_PROMPT.is_empty());
        assert!(!REVIEW_COMMENT_PROMPT.is_empty());
    }
}
