# Repository Guidelines

This file is the human-facing contributor and agent guide. `CLAUDE.md` is the
source of truth for detailed architecture notes and should be checked when
touching unfamiliar layers.

## Working Expectations

- State assumptions before coding. If the task has multiple plausible meanings,
  surface the tradeoff instead of choosing silently.
- Keep changes minimal and directly tied to the request. Do not add speculative
  features, broad abstractions, or unrelated cleanup.
- Match existing style and ownership boundaries. Remove only unused code that
  your change created.
- Define verification up front for multi-step work, then loop until the focused
  checks pass or a concrete blocker is identified.

## Project Structure & Module Organization

This is a Rust 2024 Cargo workspace that re-implements the Claude Code CLI as a
native binary (`orbcode`). Byte-level compatibility with the TypeScript CLI is a
primary goal, including transcript format, settings layering, CLI flags, and
stream-json wire output.

Top-level crates follow a layered dependency model:

- `protocol`: shared serde types and streaming contracts.
- `config`: settings layering, auth, permissions, managed policy, and home
  resolution.
- `model-provider`: streaming HTTP providers, retry behavior, token counting,
  and test-only mock provider support.
- `session-store`: persisted JSONL transcripts, prompt history, and live-session
  registry.
- `mcp`: MCP transports, OAuth, server registry, trust, and config hot reload.
- `tools`: built-in tool adapters and `ToolRegistry::foundation()`.
- `core`: turn orchestration, agent loop, permissions, hooks, compaction, and
  cancellation.
- `app-server`: in-process facade used by the TUI and CLI.
- `tui`: ratatui interface.
- `cli`: `orbcode` binary entrypoint.
- `compat-fixtures`: golden TypeScript-vs-Rust fixture corpus.

Each crate documents its public surface in `lib.rs`; subsystem detail lives in
module-level doc comments rather than in separate design documents.

## Build, Test, and Development Commands

Run commands from the workspace root, the directory containing this file.

- `cargo check --workspace`: type-check every crate.
- `cargo test --workspace`: run unit and integration tests.
- `cargo fmt --all --check`: verify rustfmt output.
- `cargo clippy --workspace`: run lints.
- `scripts/check.sh`: canonical local verification flow, running fmt, clippy,
  check, and tests. Use `--quick` to skip tests or `--release` for release mode.
- `cargo run -p orbcode -- <args>`: run the `orbcode` binary. With no subcommand
  it launches the TUI. Useful subcommands include `prompt "..."`, `sessions`,
  `providers`, `context`, `tools`, `doctor`, and `mcp servers`.

`scripts/run.sh <args>` wraps the CLI. It resolves the workspace root from its
own path, so it works from any directory.

## Testing Guidelines

Add focused tests for behavioral changes. Keep unit tests in the affected file's
`#[cfg(test)] mod tests` when possible, and use crate-level `tests/` only for
public integration flows such as `cli/tests/`, `mcp/tests/`, and
`model-provider/tests/`. Use `#[tokio::test]` for async paths.

To run a single test:

```sh
cargo test -p <crate> <test_name>
cargo test -p orbcode --test stream_json_e2e
```

When testing provider errors, retries, or fallback behavior, use the
`mock-provider` feature through existing dev-dependency paths and drive behavior
with `mock://` URLs rather than ad-hoc prompt markers.

## Coding Style & Naming Conventions

Use rustfmt as the formatting authority. Follow standard Rust naming:
`snake_case` for functions, modules, and files; `PascalCase` for types and
traits; and `SCREAMING_SNAKE_CASE` for constants. Crate packages use the
`orbcode-*` prefix, except the binary crate itself, which is just `orbcode`.

Prefer small modules with explicit ownership over broad utility layers. Put
shared types in the lowest crate that needs them, usually `protocol`, and
re-export public APIs through each crate's `lib.rs` using the existing grouping.
Match nearby error handling patterns, such as `thiserror` enums per crate that
convert upward.

## Configuration & Permission Notes

- Home resolution is `ORBCODE_HOME` > `CLAUDE_CONFIG_DIR` > existing `~/.orbcode`
  > `~/.claude`. `~/.orbcode` is opt-in and never created automatically, so the
  default stays shared with the TypeScript CLI.
- Our env keys are `ORBCODE_*`. The `CLAUDE_CODE_*` / `ANTHROPIC_*` / `OPENAI_*`
  aliases in `config/src/env_compat.rs` are TypeScript-CLI compatibility, not old
  branding — keep them. The pre-rename prefix is a hard break and is accepted
  nowhere. Run `scripts/audit-brand.sh`: it fails on a leaked old name and also on
  a compatibility name disappearing. The exempt paths — captured fixtures, render
  goldens, and one pre-rename transcript regression test — are listed with their
  reasons at the top of that script.
- Settings precedence is User, Project, Local, then Managed. Managed policy can
  lock keys and prune disallowed MCP servers.
- Permission rules include structured bash parsing; do not replace them with
  string matching.
- MCP tool calls require both matching allow rules and trusted server state.
  Trust alone cannot bypass missing allow rules, and allow rules cannot bypass
  an untrusted server.

## Commit & Pull Request Guidelines

Use short, imperative commit subjects, specific and under about 72 characters.
PRs should describe the user-visible change, list verification commands, link
related issues or plan items, and include terminal screenshots only for
meaningful TUI behavior changes.

## Security

Do not commit provider tokens, local transcript data, generated secrets, or
runtime state. When testing auth flows, prefer environment variables or
disposable values and keep state outside the repository.
