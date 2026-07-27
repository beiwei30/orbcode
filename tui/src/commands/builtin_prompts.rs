use crate::slash_commands::BuiltinPromptSlashCommand;

/// The prompt submitted by `/init`. Mirrors the TypeScript CLI's built-in
/// `init` command so the model produces a `CLAUDE.md` with the same structure
/// and guardrails.
///
/// The "Claude Code" wording below is deliberately NOT rebranded: the artefact
/// is a file literally named `CLAUDE.md`, which both this CLI and the TypeScript
/// CLI read. Telling the model to address the file to "Orb Code" would make the
/// generated file misleading to anyone using it from the TypeScript side.
const INIT_PROMPT: &str = r#"Please analyze this codebase and create a CLAUDE.md file, which will be given to future instances of Claude Code to operate in this repository.

What to add:
1. Commands that will be commonly used, such as how to build, lint, and run tests. Include the necessary commands to develop in this codebase, such as how to run a single test.
2. High-level code architecture and structure so that future instances can be productive more quickly. Focus on the "big picture" architecture that requires reading multiple files to understand.

Usage notes:
- If there's already a CLAUDE.md, suggest improvements to it.
- When you make the initial CLAUDE.md, do not repeat yourself and do not include obvious instructions like "Provide helpful error messages to users", "Write unit tests for all new utilities", "Never include sensitive information (API keys, tokens) in code or commits".
- Avoid listing every component or file structure that can be easily discovered.
- Don't include generic development practices.
- If there are Cursor rules (in .cursor/rules/ or .cursorrules) or Copilot rules (in .github/copilot-instructions.md), make sure to include the important parts.
- If there is a README.md, make sure to include the important parts.
- Do not make up information such as "Common Development Tasks", "Tips for Development", "Support and Documentation" unless this is expressly included in other files that you read.
- Be sure to prefix the file with the following text:

```
# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.
```"#;

pub(crate) fn builtin_prompt_body(command: BuiltinPromptSlashCommand) -> &'static str {
    match command {
        BuiltinPromptSlashCommand::Init => INIT_PROMPT,
        BuiltinPromptSlashCommand::Review => {
            unreachable!("handled by extracted ReviewCommand")
        }
    }
}
