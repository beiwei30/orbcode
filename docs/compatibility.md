# Claude Code compatibility

[English](compatibility.md) · [简体中文](zh-CN/compatibility.md)

Byte-level compatibility with the TypeScript Claude Code CLI is a primary
design goal, enforced with captured fixtures and Rust/TypeScript comparisons.
Orb Code remains independent and unofficial; compatibility names identify the
format or API being implemented, not affiliation.

## Shared without conversion

- JSONL transcripts under `<home>/projects/<slug>/`, including cross-CLI
  resume, tool blocks, compaction boundaries, and child-session metadata.
- Settings schema and User → Project → Local → Managed layering.
- `.mcp.json`, settings `mcpServers`, and compatible MCP trust lists.
- `CLAUDE.md`, `.claude/CLAUDE.md`, and rules discovery.
- Prompt history and compatible credential/token sources.
- Common flags such as resume/continue, print/output/input formats, permission
  mode/rules, additional directories, MCP/settings overlays, session ID, and
  appended system prompt.
- Stream-JSON initialization, content/progress, control correlation, and result
  subtype shapes covered by compatibility fixtures.

The default home is `~/.claude` specifically so these formats can be shared.
Create `~/.orbcode` or set `ORBCODE_HOME` when you do not want that.

## Intentional differences

- Orb Code is a native Rust binary and does not require Node.js to run.
- There is no `--model` flag; use `/model`, settings, or a model environment
  variable.
- `--dangerously-skip-permissions` is represented by
  `--permission-mode bypassPermissions` and can be disabled by managed policy.
- Session forking is a `fork` command, not `--fork-session`.
- Flags such as `--debug`, `--strict-mcp-config`,
  `--include-partial-messages`, `--max-turns`, `--agents`, and
  `--system-prompt` are absent. `--append-system-prompt` is supported.
- Headless process exit codes are more specific while terminal `result.subtype`
  strings remain compatible.
- The stream-JSON control union rejects unknown and unsupported operations with
  correlated errors. `rewind_files` is not emulated by transcript rewind.
- Hook coverage is partial; see [Extensions](extensions.md#hooks).
- Provider adapters currently ship only for Anthropic and OpenAI paths.

## Compatibility discipline

Captured fixtures, render goldens, and public-surface audits make format drift
visible. Compatibility aliases such as `ANTHROPIC_*`, `OPENAI_*`,
`CLAUDE_CODE_*`, `CLAUDE_CONFIG_DIR`, and `~/.claude` are not old branding and
must not be renamed casually. `scripts/audit-brand.sh` checks both that the
pre-rename project identity does not leak and that required compatibility names
do not disappear.

When the two CLIs disagree, report the exact version/commit, command, active
home, and a redacted minimal fixture. Do not attach real transcripts or tokens.
New upstream TypeScript-only features may lag and should be listed honestly in
[Feature status](feature-status.md) rather than documented as implemented.
