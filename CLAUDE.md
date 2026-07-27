# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A Rust 2024 Cargo workspace that re-implements the Claude Code CLI as a native
binary (`orbcode`). A primary goal is **byte-level compatibility with the
TypeScript CLI**: on-disk transcript format, settings layering, CLI flags, and
stream-json wire output are all expected to match. The `compat-fixtures` crate
holds golden TS-vs-Rust fixtures that enforce this.

`AGENTS.md` holds the human-facing contributor guide (commit style, PR
expectations); this file focuses on architecture and the commands needed to be
productive.

## Commands

Run these from the workspace root (the directory containing this file):

- `cargo check --workspace` — type-check every crate.
- `cargo test --workspace` — run all unit + integration tests.
- `cargo fmt --all --check` — verify rustfmt (CI-enforced; rustfmt is the
  formatting authority, do not hand-format).
- `cargo clippy --workspace` — lints (`too_many_arguments` is allowed
  workspace-wide; see root `Cargo.toml`).
- `cargo run -p orbcode -- <args>` — run the `orbcode` binary. With no
  subcommand it launches the TUI. Useful subcommands: `prompt "..."`,
  `sessions`, `providers`, `context`, `tools`, `doctor`, `mcp servers`.
- `scripts/check.sh` — canonical local verification (fmt → clippy → check →
  test). Supports `--quick` (skip tests) and `--release`. Works from the
  workspace root.
- `scripts/run.sh <args>` wraps the CLI. It resolves the workspace root from its
  own path, so it works from any directory.

### Running a single test

```
cargo test -p <crate> <test_name>          # e.g. cargo test -p orbcode-core retry
cargo test -p orbcode --test stream_json_e2e   # one integration test file
```

Crate packages use the `orbcode-*` prefix (e.g. `orbcode-core`, `orbcode-app-server`),
except the binary crate, which is just `orbcode`.

### The `mock-provider` feature

`orbcode-model-provider` has a `mock-provider` feature that compiles a URL-driven
mock provider activated by `mock://...` base URLs. It is **off in release** and
pulled in only through dev-dependencies (in `core` and `cli`) so retry,
fallback, and error-path test suites can drive deterministic provider behavior.
Cargo feature unification keeps it scoped to test builds. When writing tests
that need to simulate provider errors/retries, drive them via a `mock://` URL
rather than ad-hoc prompt markers.

## Architecture

### Crate layering (dependency DAG, lowest first)

```
protocol            shared serde types: StreamEvent, TranscriptMessage,
                    SessionRecord, TurnContext, TokenUsage, ProviderId, ... (no internal deps)
config              settings layering, auth, permission rules, claude_home,
                    model resolution, memory, plugins, managed policy
model-provider      streaming HTTP providers (Anthropic/OpenAI/...), retry,
                    rate-limit, token counting, stream accumulation, mock
session-store       persisted JSONL transcripts, prompt history, child
                    sessions, live-session registry, codec/normalization
mcp                 MCP client: transports, OAuth, server registry/trust,
                    config hot-reload (~17 modules under transport/, registry/)
tools               tool adapters + ToolRegistry::foundation() (bash, file-*,
                    glob, grep, web-*, task-*, plan, skill, lsp, Agent, ...)
core                orchestration: SessionManager, turn loop, agent loop,
                    permissions, hooks, compaction, context estimation, retry
app-server          AppServer in-process facade over core+tools+mcp+config
tui                 ratatui interface (depends on app-server)
cli                 orbcode binary entrypoint (depends on app-server + tui)
compat-fixtures     dev-only golden TS/Rust fixture corpus + normalizers
```

When adding a type used across layers, put it in the lowest crate that needs it
(usually `protocol`). Crates re-export their public surface through `lib.rs`;
follow the existing `pub use` grouping there.

### The request/turn flow

1. `cli/src/main.rs` parses args (clap), builds `AppConfigOverrides`, and
   constructs an `AppServer` (`app-server/src/lib.rs`). Many TS-compatible flags
   are global (`--resume`, `--continue`, `-p/--print`, `--output-format`,
   `--permission-mode`, `--allowed-tools`, `--mcp-config`, ...).
2. `AppServer` is the single facade the TUI and CLI talk to. It owns a
   `SessionManager` (core), `AuthManager` (config), `BackgroundManager`,
   `ToolRegistry` (tools), and `McpRegistry` (mcp). `bootstrap()` resumes or
   starts a session and returns a `BootstrapState`.
3. `AppServer::submit_turn()` delegates to `SessionManager` and returns an
   `mpsc::UnboundedReceiver<StreamEvent>`. **`StreamEvent` (in `protocol`) is
   the central streaming contract** — assistant deltas, tool lifecycle,
   permission requests, compaction, cancellation, and turn completion all flow
   through it. Both the TUI (`tui/src/chat/stream_events.rs`) and the headless
   CLI loops (`run_headless_prompt`, `run_print_mode`, `run_background_worker`)
   consume the same event stream.
4. The core turn machinery lives in `core/src/session_manager/` (split into
   `session_*` modules by concern) and `core/src/agent_loop/` (no-tool turns vs.
   tool rounds) plus `core/src/turn_loop.rs` (active-turn registry +
   cancellation flags). One active turn per session is enforced
   (`CoreError::ActiveTurn`).

### Configuration & policy

- Home dir resolves as `ORBCODE_HOME` > `CLAUDE_CONFIG_DIR` > existing
  `~/.orbcode` > `~/.claude` (`config/src/claude_home.rs`). `~/.orbcode` is
  **opt-in and never created by us** — `default_home_dir` only probes for it — so
  by default the home directory and everything in it stays shared with the
  TypeScript CLI. Creating `~/.orbcode` is how a user asks for a separate store;
  there is no migration, the two directories are independent.
- Env keys are canonically `ORBCODE_*`. `config/src/env_compat.rs` maps each one
  to its TypeScript-era alias (`ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN`,
  ...) and resolves canonical-first across both the process env and the settings
  `env` block. **Those aliases are compatibility contracts — do not rename
  them.** The project's own pre-rename prefix, by contrast, is a hard break and is
  accepted nowhere; `scripts/audit-brand.sh` enforces both directions (fails on a
  leaked old name, and on a compatibility name going missing). That script's
  header lists every exempt path and why it is exempt.
- Settings layer precedence (lowest → highest):
  **User → Project → Local → Managed** (`config/src/layers.rs`). `Managed` is
  enterprise policy and can *lock* keys; `AppServer::ensure_setting_mutable`
  rejects mutations to managed-locked keys, and managed policy can prune
  disallowed MCP servers at load time.

### Permissions

The permission system is substantial. `config/src/permission_rules/` parses
allow/deny rules, including a bash AST (`bash_ast.rs`, `bash_allow.rs`,
`bash_deny.rs`, tree-sitter-bash) so command rules understand shell structure
rather than matching strings. Deny precedence is resolved in the parsing layer.
Runtime decisions and edit/rule state live in `core/src/permission_state/` and
`core/src/permissions/`. Tools declare `requires_tools_permission` /
`requires_network_permission` in the registry; MCP calls additionally gate on
per-server *trust* (`Trusted`/`Unknown`/`Denied`) — an allow rule cannot bypass
an untrusted server and trust alone cannot bypass a missing allow rule.

### Tools

`ToolRegistry::foundation()` in `tools/src/catalog.rs` is the authoritative list
of built-in tools and their permission/network requirements. Tool names have a
canonical internal form and a separate provider-facing name/description/schema
(see `provider_facing_tool_name`, `tool_input_schema`). MCP tools are surfaced
through `mcp_provider_tool_name` (`mcp__<server>__<tool>`).

### Persistence

`session-store` writes transcripts as JSONL compatible with the TypeScript CLI.
Transcript writes are flushed so on-disk order matches `await` order. Large tool
results are persisted out-of-line with preview messages (`tool_results.rs`).
Compatibility is verified by `session-store/tests/compat_transcripts.rs` against
`compat-fixtures`.

## Conventions

- Tests: keep unit tests in the affected file's `#[cfg(test)] mod tests`; use a
  crate-level `tests/` directory only for public integration flows (see
  `cli/tests/`, `mcp/tests/`, `model-provider/tests/`). Use `#[tokio::test]` for
  async paths.
- Prefer small modules with explicit ownership over broad utility layers; match
  the error-handling pattern of nearby code (`thiserror` enums per crate,
  converted up the stack — e.g. `ConfigError`/`SessionStoreError` → `CoreError`).
- Design rationale lives next to the code it explains: crate-level docs in
  `lib.rs`, subsystem docs in module headers. When a decision is non-obvious,
  record it there rather than in a separate document that can drift.
