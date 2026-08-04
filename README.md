# Orb Code

A native Rust re-implementation of the Claude Code CLI, shipped as a single
binary named `orbcode`.

[![ci](https://github.com/beiwei30/orbcode/actions/workflows/ci.yml/badge.svg)](https://github.com/beiwei30/orbcode/actions/workflows/ci.yml)
![status: alpha](https://img.shields.io/badge/status-alpha-orange)
![rust: edition 2024](https://img.shields.io/badge/rust-edition%202024-informational)
![license: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)

Compatibility with the TypeScript CLI is a primary design goal: the on-disk
transcript format, settings layering, `.mcp.json`/`CLAUDE.md` discovery, CLI
flags and the `stream-json` wire format are all built to match, and golden
fixtures in `compat-fixtures` enforce it. By default Orb Code reads and writes
the same `~/.claude` directory as the TypeScript CLI, so the two can be used
interchangeably against the same state with nothing to migrate.

> **Project status: alpha (`0.0.1`).** There is no published crate or npm
> package, no stable release channel, and no compatibility promise across
> commits. The [feature maturity](#features-and-maturity) tables below say
> exactly which surfaces are dependable and which are still moving.

> **Not affiliated with Anthropic.** Orb Code is an independent, unofficial
> project — not endorsed by, sponsored by, or connected to Anthropic in any
> way. "Anthropic", "Claude" and "Claude Code" are trademarks of Anthropic PBC
> and appear here only to identify the CLI this software reimplements and the
> API it talks to. See [License](#license).

## Contents

- [Install](#install)
- [Quick start](#quick-start)
- [Features and maturity](#features-and-maturity)
- [CLI reference](#cli-reference)
  - [Global flags](#global-flags)
  - [Subcommands](#subcommands)
  - [Recipes](#recipes)
  - [Exit codes](#exit-codes)
- [Configuration](#configuration)
  - [Home directory](#home-directory)
  - [Settings layering](#settings-layering)
  - [`settings.json` recipes](#settingsjson-recipes)
  - [Environment variables](#environment-variables)
  - [Outbound proxies](#outbound-proxies)
  - [Project files](#project-files)
- [MCP servers](#mcp-servers)
- [Compatibility with the TypeScript CLI](#compatibility-with-the-typescript-cli)
- [Development](#development)
  - [Running from source](#running-from-source)
  - [Debugging and tracing](#debugging-and-tracing)
  - [Crate layout](#crate-layout)
- [Documentation](#documentation)
- [License](#license)

## Install

There is no package registry release yet. Build from source with a
rustup-managed stable toolchain (Rust 2024 edition, i.e. 1.85 or newer):

```bash
git clone <repo-url> && cd <repo-dir>

# Build a release binary at target/release/orbcode
cargo build --release -p orbcode

# ...or install it onto your PATH
cargo install --path cli
```

CI also publishes packaged archives (`orbcode-<version>-<target>.tar.gz` /
`.zip`, plus a `.sha256`) for `x86_64-unknown-linux-gnu`,
`aarch64-apple-darwin` and `x86_64-pc-windows-msvc` on tag pushes. To produce
the same artifact locally:

```bash
scripts/package-release.sh --out-dir dist
```

External runtime dependencies: `git` and `ripgrep` (`rg`) for the shell and
`grep` tools; `sandbox-exec` (macOS, preinstalled) or `bwrap` (Linux) only if
you enable sandboxing. Run `orbcode doctor` to see what is present.

## Quick start

```bash
export ANTHROPIC_API_KEY="sk-ant-..."      # or CLAUDE_CODE_OAUTH_TOKEN

orbcode                                     # interactive TUI
orbcode -p "explain this repo"              # one-shot, non-interactive
orbcode -p --output-format json "..."       # machine-readable result
orbcode --continue                          # resume the latest session in the TUI
orbcode doctor                              # environment health check
```

OpenAI can also use a ChatGPT/Codex subscription without an OpenAI API key:

```bash
orbcode auth login --provider openai --method chatgpt
# On a headless host:
orbcode auth login --provider openai --method chatgpt --device-code

orbcode auth status
env -u OPENAI_API_KEY -u ORBCODE_OPENAI_API_KEY \
  orbcode --provider openai prompt "reply OK"
```

This is a separate, experimental auth path backed by the ChatGPT Codex
Responses endpoint. Credentials are stored in `<home>/auth.json`; Orb Code does
not read or modify `~/.codex/auth.json`. An explicit OpenAI API key takes
precedence over the saved ChatGPT login. Use
`orbcode auth logout --provider openai` to remove stored OpenAI credentials.

Tool execution is **off by default**: with no permission configuration, the
model can talk but cannot run `Bash` or edit files. Turn it on per invocation
with a permission preset or an explicit flag:

```bash
orbcode --permission-mode acceptEdits -p "fix the failing test"
orbcode --allow-tools true --allowed-tools "Bash(cargo test:*),Read,Edit" -p "..."
```

In the TUI, `/permissions` and `/allow-all` do the same interactively. Settings
`permissions.allow` rules also unlock individual tools — see
[Configuration](#configuration).

From a source checkout, replace `orbcode` with `cargo run -p orbcode --` or
`scripts/run.sh` (which resolves the workspace root from its own path and so
works from any directory). See [Running from source](#running-from-source) for
the throwaway-home and tracing patterns.

## Features and maturity

| Level | Meaning |
| --- | --- |
| **Stable** | Implemented, test-covered, and the intended supported path. |
| **Beta** | Usable day to day; rough edges and behavior changes still expected. |
| **Experimental** | Shape may change without notice; hidden or unpolished surface. |
| **Deferred** | Deliberately not implemented. Kept out of user- and model-visible surfaces on purpose. |

### Interfaces

| Feature | Maturity | Notes |
| --- | --- | --- |
| Interactive TUI | Beta | ratatui chat UI, 46 slash commands, model/permission/sandbox/session/rewind/memory pickers, diff view, transcript pager, themes, vim keybindings. |
| Headless prompt (`-p/--print`, `prompt`) | Stable | `--output-format text\|json\|stream-json`. |
| `stream-json` duplex (`--input-format stream-json`) | Beta | NDJSON in and out. Control requests handled: `interrupt`, `set_permission_mode`, `get_session_state`; anything else gets a structured "unsupported" response. |
| Background jobs | Stable | `prompt --bg` plus `ps`, `logs`, `attach`, `kill`. |
| Direct tool invocation (`orbcode tool`) | Beta | Runs one tool outside a turn; useful for debugging. |
| ACP adapter (`orbcode acp`) | Experimental | Agent Client Protocol v1 over stdio; manually smoke-tested against Zed. |
| App-server protocol (`orbcode serve` / `orbcode remote`) | Experimental | Hidden subcommands. Protocol `1.0` over stdio, Unix socket or WebSocket; one client per `serve`. Stdio is implicitly trusted; socket and WebSocket require an auth token (auto-generated if `--auth-token` is omitted, reported in the startup connection-info line), and WebSocket also validates `Origin`. |
| Remote-control bridge, voice, computer use | Deferred | Reported as deferred by `orbcode advanced`. |

### Model providers

| Feature | Maturity | Notes |
| --- | --- | --- |
| Anthropic | Stable | Default provider. Streaming, thinking + interleaved thinking, server-side token counting, API key or OAuth token. |
| OpenAI-compatible API key | Beta | Chat Completions streaming with `effort`; works against compatible endpoints via `OPENAI_BASE_URL`. No server-side token counting. |
| ChatGPT/Codex subscription | Experimental | Browser PKCE or device-code login, token refresh, and Responses streaming with reasoning and function calls. Uses the fixed ChatGPT Codex backend; `OPENAI_BASE_URL` is intentionally ignored. Subscription tokens are counted but not assigned API-dollar prices. |
| Gemini, Grok | Not implemented | Accepted as `--provider` values, but every request fails with a `unsupported_provider` error and a suggestion. Do not select them. |
| Retry, fallback, rate-limit handling | Stable | `--max-retries`, `--fallback-provider`, normalized provider error categories, retry-after handling. |
| Model resolution | Stable | Settings, `ANTHROPIC_MODEL`-style env vars, and family aliases (`opus`/`sonnet`/`haiku`). Note there is **no `--model` flag**; use `/model`, settings, or env. |

### Built-in tools

25 tools ship in `ToolRegistry::foundation()`; `orbcode tools` prints the live
list with its permission requirements.

| Group | Maturity | Provider-facing names |
| --- | --- | --- |
| Files and shell | Stable | `Read`, `Edit`, `Write`, `Glob`, `Grep`, `Bash`, `NotebookEdit` |
| Web | Stable | `WebFetch` (curl), `WebSearch` (DuckDuckGo HTML) |
| Planning and tasks | Beta | `EnterPlanMode`, `ExitPlanMode`, `VerifyPlanExecution`, `TodoWrite`, `TaskCreate`, `TaskGet`, `TaskList`, `TaskUpdate`, `TaskOutput`, `TaskStop` |
| Subagents | Beta | `Agent` (local, synchronous) |
| Skills and discovery | Beta | `Skill`, `ToolSearch` |
| Code intelligence | Experimental | `LSP` — heuristic queries over the workspace, not a real language-server client. |
| Workflows | Experimental | `Workflow` — starts a generated dynamic workflow as a durable background task. |
| Interactive questions | Experimental | `AskUserQuestion` — hidden from the provider; renders a payload for a later approval step. |
| Deferred | Deferred | `PowerShell`, `Cron*`, `Monitor`, `Sleep`, `Browser`, `RemoteTrigger`, `Teams`, `Vault`, `ReviewArtifact`, `SyntheticOutput`, `Marketplace`, `PushNotification`, `ScheduleWakeup`, `EnterWorktree`, `ExitWorktree`. A registry invariant test keeps these out of both the catalog and provider requests until they are closed-loop. |

### Permissions and sandboxing

| Feature | Maturity | Notes |
| --- | --- | --- |
| Allow/deny rules | Stable | Rule syntax matches the TypeScript CLI (`Bash(git commit:*)`, `Read`, `mcp__server__tool`). Deny wins. |
| Structured bash rules | Stable | Commands are parsed with tree-sitter-bash, so rules understand pipes, subshells and operators instead of matching strings. |
| Permission modes | Stable | `default`, `acceptEdits`, `bypassPermissions`, `dontAsk`, `plan`, `auto`. |
| Managed (enterprise) policy | Stable | Can lock settings keys, forbid `bypassPermissions`, and prune disallowed MCP servers at load time. |
| MCP trust gate | Stable | An allow rule cannot bypass an untrusted server, and trust cannot bypass a missing allow rule. |
| macOS seatbelt sandbox | Beta | `--sandbox-mode workspace-write\|read-only` via `sandbox-exec`. |
| Linux bubblewrap sandbox | Beta | Requires `bwrap` on `PATH`; a missing binary is a hard error, not a silent downgrade. |
| Windows sandbox runner | Experimental | Argument builder is tested; real host validation is opt-in (`ORBCODE_RUN_WINDOWS_SANDBOX_HOST_TESTS=1`). |
| Default sandbox | — | `danger-full-access`: `Bash` runs **without** OS sandboxing unless you pass `--sandbox-mode`. |

### Sessions, context and persistence

| Feature | Maturity | Notes |
| --- | --- | --- |
| JSONL transcripts | Stable | Byte-compatible with the TypeScript CLI and pinned by `compat-fixtures` golden tests. Writes are flushed so on-disk order matches await order. |
| `--resume` / `--continue` / `fork` / `rename` | Stable | Sessions written by either CLI can be resumed by the other. |
| Large tool results | Stable | Persisted out-of-line with preview messages. |
| Prompt history, child sessions, live-session registry | Stable | Shared with the TypeScript CLI's layout. |
| Context estimation and auto-compaction | Beta | Configurable via `ORBCODE_MAX_CONTEXT_TOKENS` / `ORBCODE_AUTO_COMPACT_WINDOW`; `/compact` and `/context` in the TUI. |
| Rewind / checkpoint | Beta | `/rewind` picker over transcript checkpoints. |

### Configuration and extensions

| Feature | Maturity | Notes |
| --- | --- | --- |
| Settings layering | Stable | User → Project → Local → Managed, with `--settings` as an inline overlay. |
| `CLAUDE.md` memory | Stable | Managed, user, project and directory-scoped files, plus `<home>/rules` and per-directory `.claude/CLAUDE.md`. |
| Subagents | Beta | `<home>/agents/*.md` and `.claude/agents/*.md`; `/agents`. |
| Skills | Beta | `<home>/skills/<name>/SKILL.md` and `.claude/skills/...`; `/skills` and the `Skill` tool. |
| Output styles | Beta | `<home>/output-styles/*.md` and `.claude/output-styles/*.md`; `/output-style`. |
| Plugins | Experimental | `<home>/plugins/installed_plugins.json` (v1 and v2), contributing skills, agents and hooks. Plugin tools are surfaced as `plugin__<plugin>__<tool>`. |
| Hooks | Beta | `command` hooks for `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `UserPromptSubmit`, `Stop`, `SubagentStop`, with `decision`/`permissionDecision`/`updatedInput`/`additionalContext` handling. Other TypeScript hook events (`SessionStart`, `SessionEnd`, `PreCompact`, `Notification`) are not implemented. `/hooks` lists what was discovered. |
| Keybindings | Beta | `<home>/keybindings.json` merged over the built-in keymap; `/keybindings`. |

### MCP

| Feature | Maturity | Notes |
| --- | --- | --- |
| stdio transport | Stable | Local server processes. |
| Streamable HTTP | Stable | Canonical remote transport, with session management and both JSON and SSE response modes. Legacy `http`, `https` and `sse` configs are accepted as aliases. |
| WebSocket | Beta | `ws://` and `wss://` over real JSON-RPC. |
| Tools, resources, prompts | Stable | `mcp tools`, `mcp resources`, `mcp read`, `mcp prompts`, `mcp prompt`, `mcp call`. Model-facing names are `mcp__<server>__<tool>`. |
| OAuth | Beta | Token store plus device-code, browser and RFC 7591 dynamic client registration flows (`mcp auth ...`). |
| Trust management | Stable | `mcp trust` / `mcp distrust` / `mcp untrust`. |
| Config discovery and hot reload | Beta | `.mcp.json`, settings `mcpServers`, and `--mcp-config`, re-read on change. |

## CLI reference

`orbcode --help` and `orbcode help <subcommand>` are authoritative; this section
mirrors them.

```
orbcode [OPTIONS] [PROMPT] [COMMAND]
```

With no subcommand, `orbcode` launches the TUI — unless `-p/--print` is given,
in which case the positional `PROMPT` runs headlessly.

### Global flags

All of these are `global = true`, so they may appear before or after a
subcommand.

#### Session selection

| Flag | Values | Description |
| --- | --- | --- |
| `-c`, `--continue` | — | Resume the most recent session for this workspace. |
| `-r`, `--resume [<SESSION_ID>]` | session id, optional | Resume a specific session; with no value, the latest one. `--resume=<id>` and `--resume <id>` both work. Careful: a bare `-r` followed by a non-flag token consumes it as a session id (`-r tui` looks up a session named `tui`). |
| `--session-id <SESSION_ID>` | id | Use this session id for the invocation, creating it if missing. |

#### Headless / print mode

| Flag | Values | Description |
| --- | --- | --- |
| `-p`, `--print` | — | Non-interactive single-turn mode. TypeScript-compatible. |
| `--output-format <FORMAT>` | `text` (default), `json`, `stream-json` | Result shape in print/headless mode. |
| `--input-format <FORMAT>` | `text` (default), `stream-json` | Read NDJSON user messages and control requests from stdin. |
| `--verbose` | — | Verbose progress output. **Required** with `--output-format stream-json` in print mode. |
| `--append-system-prompt <TEXT>` | text | Extra text appended to the system prompt for this invocation. |

#### Permissions and tools

| Flag | Values | Description |
| --- | --- | --- |
| `--permission-mode <MODE>` | `default`, `acceptEdits`, `bypassPermissions`, `dontAsk`, `plan`, `auto` | Permission preset. `acceptEdits`/`bypassPermissions`/`dontAsk`/`auto` imply `--allow-tools true`; `plan` implies `false`. `bypassPermissions` is downgraded to `default` when managed policy forbids it. Kebab-case aliases (`accept-edits`) are accepted. |
| `--allow-tools <true\|false>` | bool | Master switch for tool execution and local file mutation. Default `false` (also settable via `ORBCODE_ALLOW_TOOLS`). |
| `--allowed-tools <TOOLS>` | rule list, repeatable | Additional allow rules, e.g. `--allowed-tools "Bash(cargo test:*),Read,Edit"`. Comma- or space-separated, parentheses respected. Alias: `--allowedTools`. |
| `--disallowed-tools <TOOLS>` | rule list, repeatable | Additional deny rules. Deny always beats allow. Alias: `--disallowedTools`. |
| `--allow-network <true\|false>` | bool | Whether network-backed tools (`WebFetch`, `WebSearch`) are available. Default `true`. |
| `--add-dir <DIR>` | path, repeatable | Additional workspace root the tools may read and write. |

#### Sandboxing

| Flag | Values | Description |
| --- | --- | --- |
| `--sandbox-mode <MODE>` | `danger-full-access` (default), `workspace-write`, `read-only` | OS-level sandbox for `Bash`. Enforced through macOS seatbelt, Linux bubblewrap, or the Windows sandbox runner. |
| `--sandbox-network <true\|false>` | bool | Whether the sandbox may reach the network. Defaults to the `--allow-network` value. |

#### Providers

| Flag | Values | Description |
| --- | --- | --- |
| `--provider <PROVIDER>` | `anthropic` (default), `openai`, `gemini`, `grok` | Primary provider. `gemini` and `grok` are not implemented and will fail every request. |
| `--fallback-provider <PROVIDER>` | same set | Provider to fall back to when the primary fails with a retryable-then-exhausted error. |
| `--max-retries <N>` | integer | Provider retry budget (default `2`). |

#### Configuration

| Flag | Values | Description |
| --- | --- | --- |
| `--settings <FILE_OR_JSON>` | path or inline JSON | Settings overlay applied on top of the layered settings. Inline values must start with `{`; invalid JSON is a hard error. |
| `--mcp-config <CONFIG>` | path or inline JSON, repeatable | Additional MCP server definitions. |
| `-h`, `--help` / `-V`, `--version` | — | Help; version with commit sha, target, profile, build time and compiled providers. |

### Subcommands

| Subcommand | Description |
| --- | --- |
| *(none)* / `tui` | Interactive TUI with a local core. |
| `prompt <PROMPT> [--session <ID>] [--bg]` | One headless turn. `--bg` queues it as a background job. |
| `resume <SESSION_ID>` | Open an existing session in the TUI. |
| `fork <SESSION_ID> [--title T] [--note N] [--prompt P] [--tui]` | Fork a session into a new one. |
| `sessions [--json]` | List persisted sessions. |
| `rename <SESSION_ID> <NEW_TITLE>` | Override a session's generated title. |
| `ps` / `logs <JOB_ID> [--follow]` / `attach <JOB_ID>` / `kill <JOB_ID>` | Background job management. |
| `providers` | Provider catalog plus the active chain and permission summary. |
| `context` | Current context snapshot (memory files, directories, git state). |
| `tools` | Tool registry with permission and network requirements. |
| `tool <TOOL_NAME> [INPUT] [--session ID]` | Invoke one tool directly with a JSON input. |
| `auth status` / `auth login` / `auth logout` | Provider auth metadata. OpenAI subscription login: `login --provider openai --method chatgpt [--device-code]`; token/env inputs remain available for `api-key` and `o-auth-device`. |
| `mcp <...>` | MCP registry: `servers`, `capabilities`, `add`, `remove`, `diagnose`, `tools`, `call`, `resources`, `read`, `prompts`, `prompt`, `trust`, `distrust`, `untrust`, `auth {status,login,device-login,browser-login,logout}`. |
| `doctor [cleanup-orphans]` | Health checks. `cleanup-orphans --dry-run \| --yes [--stale-running-days N]` prunes orphaned child-session metadata. |
| `advanced` | Which advanced capability slices are implemented vs deferred. |
| `acp` | ACP adapter over stdio (for editors such as Zed). |
| `serve` / `remote` | Hidden, experimental protocol server and remote TUI client. |

Two `doctor` probes are opt-in because they cost a network round trip:
`ORBCODE_DOCTOR_PROBE=1` fires a ~1-token provider request, and
`ORBCODE_DOCTOR_MCP_PROBE=1` checks MCP reachability.

### Recipes

Machine-readable single turn:

```bash
orbcode -p --output-format json "list the workspace crates"
```

Streaming NDJSON events (note the required `--verbose`):

```bash
orbcode -p --verbose --output-format stream-json "refactor the retry helper"
```

Duplex session driven by another program — send `user` messages and control
requests on stdin, read events on stdout:

```bash
printf '%s\n' \
  '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}' \
  '{"type":"control_request","request_id":"1","request":{"subtype":"set_permission_mode","mode":"acceptEdits"}}' \
  | orbcode -p --verbose --input-format stream-json --output-format stream-json
```

Sandboxed edits scoped to two directories:

```bash
orbcode --permission-mode acceptEdits \
        --sandbox-mode workspace-write --sandbox-network false \
        --add-dir ../shared-lib \
        -p "port the retry helper to the shared crate"
```

Tight rule set instead of a broad preset:

```bash
orbcode --allow-tools true \
        --allowed-tools "Read,Grep,Bash(cargo check:*)" \
        --disallowed-tools "Bash(git push:*),Write" \
        -p "why does the build fail?"
```

Ad-hoc settings and MCP config without touching any file:

```bash
orbcode --settings '{"model":"sonnet"}' \
        --mcp-config ./ci/mcp.json \
        -p "summarize open incidents"
```

Background job, then follow it (the `--bg` invocation prints the job id it
queued):

```bash
orbcode prompt --bg "run the full test suite and summarize failures"
orbcode ps
orbcode logs <JOB_ID> --follow
```

### Exit codes

The TypeScript CLI collapses every headless outcome to `0` or `1` and carries
the detail in `result.subtype`. Orb Code keeps those `subtype` strings
byte-compatible **and** maps each outcome to a distinct exit code, so scripts can
branch on `$?` without parsing stdout:

| Code | Meaning | `result.subtype` |
| --- | --- | --- |
| `0` | Turn completed normally | `success` |
| `1` | Model / provider / tool error | `error_during_execution` |
| `2` | Bad flag combination or missing prompt (pre-flight) | *(no result emitted)* |
| `3` | Credentials rejected | `error_during_execution` |
| `4` | A tool call was denied by permission policy | `error_during_execution` |
| `5` | Turn interrupted | `error_during_execution` |
| `6` | Turn ceiling hit — reserved, not yet emitted | `error_max_turns` |
| `7` | Budget ceiling hit — reserved, not yet emitted | `error_max_budget_usd` |

Codes `5`–`7` are pinned by tests but not reachable from a headless subprocess
yet: `-p/--print` installs no SIGINT handler, so Ctrl-C exits with the default
signal disposition (`130`).

## Configuration

### Home directory

The home directory is resolved in this order:

1. `ORBCODE_HOME`
2. `CLAUDE_CONFIG_DIR`
3. `~/.orbcode` — **only if that directory already exists**
4. `~/.claude` (default)

Defaulting to `~/.claude` is deliberate: settings, credentials, prompt history
and transcripts are all in the TypeScript CLI's formats, so out of the box both
CLIs share one state directory and there is nothing to migrate.

To keep Orb Code's state separate, create the directory yourself:

```bash
mkdir ~/.orbcode
```

From then on Orb Code uses it and ignores `~/.claude`. Orb Code never creates
`~/.orbcode` for you, so this cannot happen by accident on an upgrade — and it
copies nothing, so a fresh `~/.orbcode` starts empty (no sessions, no
credentials). Remove the directory to go back to the shared home. `orbcode
doctor` warns when an empty `~/.orbcode` is shadowing a populated `~/.claude`.

### Settings layering

Lowest to highest precedence:

1. **User** — `<home>/settings.json`
2. **Project** — `<project>/.claude/settings.json`
3. **Local** — `<project>/.claude/settings.local.json` (gitignore this)
4. **Managed** — enterprise policy, which can *lock* keys

`--settings` applies on top as a per-invocation overlay. Mutations to
managed-locked keys are rejected rather than silently ignored.

### `settings.json` recipes

For a user-wide configuration, edit `<home>/settings.json` (normally
`~/.orbcode/settings.json` after opting into an Orb Code home). Project and
local settings use the same schema. Do not commit API keys in project settings;
prefer the process environment, a secret manager, or the user-only settings
file with owner-only permissions.

#### ChatGPT/Codex subscription

After signing in once, the recommended configuration needs only the provider:

```json
{
  "env": {
    "PROVIDER_TYPE": "openai"
  }
}
```

```bash
orbcode auth login --provider openai --method chatgpt
```

No API key belongs in `settings.json`; OAuth credentials live in
`<home>/auth.json`. When `model` is omitted, this subscription path currently
defaults to `gpt-5.6-sol`. To deliberately pin it instead:

```json
{
  "model": "gpt-5.6-sol",
  "env": {
    "PROVIDER_TYPE": "openai"
  }
}
```

#### OpenAI API key

Keep the key outside a versioned settings file:

```bash
export OPENAI_API_KEY="your-key"
```

Then select OpenAI and, optionally, an OpenAI-compatible model/base URL:

```json
{
  "model": "gpt-4o",
  "env": {
    "PROVIDER_TYPE": "openai"
  }
}
```

`ORBCODE_OPENAI_API_KEY` is also accepted. An explicit API key takes precedence
over a stored ChatGPT subscription login.

#### Anthropic

Anthropic is the default, so `PROVIDER_TYPE` may be omitted. An explicit setup
looks like this:

```bash
export ANTHROPIC_API_KEY="your-key"
```

```json
{
  "model": "sonnet",
  "env": {
    "PROVIDER_TYPE": "anthropic"
  }
}
```

`ORBCODE_ANTHROPIC_API_KEY` is also accepted. Model values may be family aliases
such as `sonnet`, `opus`, or `haiku`, or a concrete provider model ID.

#### Provider and proxy in one file

An explicit proxy can be combined with either provider configuration:

```json
{
  "env": {
    "PROVIDER_TYPE": "openai",
    "http_proxy": "http://127.0.0.1:7890",
    "https_proxy": "http://127.0.0.1:7890",
    "no_proxy": "localhost,127.0.0.1,::1"
  }
}
```

On macOS these proxy fields are optional: when no higher-priority proxy is
configured, Orb Code discovers the static HTTP/HTTPS system proxy. See
[Outbound proxies](#outbound-proxies) for the complete precedence and PAC
limitation.

`PROVIDER_TYPE` accepts `openai` and `anthropic`; when omitted or invalid, the
default is `anthropic`. The `--provider` CLI option has highest precedence,
followed by `PROVIDER_TYPE` (process environment before settings). The older
process-only `ORBCODE_PROVIDER` and boolean `CLAUDE_CODE_USE_OPENAI=true` forms
remain lower-priority compatibility fallbacks.

### Environment variables

Provider selection uses the neutral `PROVIDER_TYPE` key; other Orb Code-specific
variables use the `ORBCODE_` prefix. Every variable that also exists in the
TypeScript CLI is accepted under **both** names, canonical first. The full table
lives in `config/src/env_compat.rs`; the ones you are most likely to set:

| Canonical | Also accepted | Purpose |
| --- | --- | --- |
| `PROVIDER_TYPE` | `ORBCODE_PROVIDER`; `CLAUDE_CODE_USE_OPENAI=true` | Default provider: `anthropic` or `openai`; defaults to `anthropic`. |
| `ORBCODE_ANTHROPIC_API_KEY` | `ANTHROPIC_API_KEY` | Anthropic API key. |
| `ORBCODE_ANTHROPIC_AUTH_TOKEN` | `ANTHROPIC_AUTH_TOKEN` | Anthropic bearer token. |
| `ORBCODE_OAUTH_TOKEN` | `CLAUDE_CODE_OAUTH_TOKEN` | OAuth token. |
| `ORBCODE_OPENAI_API_KEY` | `OPENAI_API_KEY` | OpenAI-compatible API key. |
| `ORBCODE_ANTHROPIC_BASE_URL` / `ORBCODE_OPENAI_BASE_URL` | `ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL` | Endpoint override (proxies, gateways, local servers). |
| `ORBCODE_ANTHROPIC_MODEL` / `ORBCODE_OPENAI_MODEL` | `ANTHROPIC_MODEL` / `OPENAI_MODEL` | Model override — there is no `--model` flag. |
| `ORBCODE_MAX_OUTPUT_TOKENS` | `CLAUDE_CODE_MAX_OUTPUT_TOKENS` | Output token cap. |
| `ORBCODE_MAX_CONTEXT_TOKENS` | `CLAUDE_CODE_MAX_CONTEXT_TOKENS` | Context window cap. |
| `ORBCODE_AUTO_COMPACT_WINDOW` | `CLAUDE_CODE_AUTO_COMPACT_WINDOW` | Auto-compaction threshold. |
| `ORBCODE_API_TIMEOUT_MS` / `ORBCODE_API_MAX_RETRIES` | `API_TIMEOUT_MS` / `API_MAX_RETRIES` | HTTP timeout and retry budget. |
| `ORBCODE_WEB_ALLOWED_DOMAINS` / `ORBCODE_WEB_BLOCKED_DOMAINS` | `CLAUDE_CODE_WEB_*` | Web tool domain filters. |

Orb-Code-only switches without a TypeScript counterpart include
`ORBCODE_HOME`, `ORBCODE_ALLOW_TOOLS`, `ORBCODE_ALLOW_NETWORK`,
`ORBCODE_SANDBOX_NETWORK`, `ORBCODE_ALLOWED_TOOLS`,
`ORBCODE_DISALLOWED_TOOLS` and `ORBCODE_TRUSTED_PROJECT`.
`CLAUDE_CONFIG_DIR` keeps its name outright. Diagnostic and test-only variables
(TUI traces, doctor probes, golden regeneration) are listed under
[Debugging and tracing](#debugging-and-tracing).

For ChatGPT subscription auth, no API-key environment variable is required.
If `ORBCODE_OPENAI_API_KEY` or `OPENAI_API_KEY` is set, that API-key path wins.
Without an explicit OpenAI model, the subscription path defaults to
`gpt-5.6-sol`; an explicit model setting is sent as-is and can still be rejected
by the account's plan or model availability.

### Outbound proxies

The recommended explicit proxy configuration is the `env` block in the active
`settings.json`:

```json
{
  "env": {
    "http_proxy": "http://127.0.0.1:7890",
    "https_proxy": "http://127.0.0.1:7890",
    "no_proxy": "localhost,127.0.0.1,::1"
  }
}
```

Provider requests and ChatGPT login/token refresh use the same destination-aware
selection order:

1. Merged `settings.json` `env.https_proxy` / `env.http_proxy` (lowercase only).
2. Process `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY` variables, including their
   lowercase forms.
3. Legacy process-only `ORBCODE_PROXY`, `CLAUDE_CODE_PROXY`, or
   `ANTHROPIC_PROXY_URL` compatibility variables.
4. Static HTTP/HTTPS proxy and exception rules reported by macOS System
   Configuration.
5. Direct connection.

HTTPS destinations fall back to `http_proxy` when no HTTPS-specific proxy is
set. Loopback destinations always connect directly; explicit proxies honor the
matching `no_proxy`/`NO_PROXY`, and macOS proxy exceptions include wildcard and
IP CIDR entries. PAC JavaScript is not evaluated; when macOS reports an active
PAC configuration Orb Code connects directly unless a higher-priority explicit
or process proxy is configured.

### Project files

| Path | Purpose |
| --- | --- |
| `CLAUDE.md`, `.claude/CLAUDE.md` | Project instructions, loaded per directory. |
| `.claude/rules/*.md` | Additional rule files (also `<home>/rules`, and a managed rules dir). |
| `.claude/settings.json`, `.claude/settings.local.json` | Project and local settings. |
| `.claude/agents/*.md` | Subagent definitions. |
| `.claude/skills/<name>/SKILL.md` | Skills. |
| `.claude/output-styles/*.md` | Output styles. |
| `.mcp.json` | Project MCP servers. |
| `<home>/keybindings.json` | Keymap overrides. |
| `<home>/plugins/installed_plugins.json` | Installed plugins. |

## MCP servers

`.mcp.json` uses the same format as the TypeScript CLI:

```json
{
  "mcpServers": {
    "my-db": {
      "command": "node",
      "args": ["./tools/db-server.js"],
      "env": { "DATABASE_URL": "postgres://localhost/mydb" }
    },
    "remote": {
      "type": "streamable_http",
      "url": "https://mcp.example.com/mcp",
      "headers": { "Authorization": "Bearer ${MY_TOKEN}" }
    }
  }
}
```

String fields support `${VAR}` and `${VAR:-default}` expansion; a missing
variable with no default is a load-time diagnostic rather than a silent empty
value.

```bash
orbcode mcp servers            # registry with status, transport, trust, auth
orbcode mcp capabilities       # which transports are enabled
orbcode mcp diagnose my-db     # probe one server, bypassing the call gate
orbcode mcp trust my-db        # trust is required before any call succeeds
orbcode mcp tools my-db
orbcode mcp call my-db query '{"sql":"select 1"}'
```

A call needs **both** a matching allow rule (`mcp__my-db__query`) and a trusted
server. Neither substitutes for the other.

## Compatibility with the TypeScript CLI

### Shared without conversion

- **Sessions** — JSONL transcripts under `<home>/projects/<slug>/`; `--resume`
  works across both binaries.
- **Settings** — same schema and same layering.
- **MCP** — `.mcp.json` and settings `mcpServers` are read as-is.
- **Memory** — `CLAUDE.md` discovery and precedence match.
- **Flags** — `--resume`, `--continue`, `-p/--print`, `--output-format`,
  `--input-format`, `--permission-mode`, `--allowed-tools`,
  `--disallowed-tools`, `--add-dir`, `--mcp-config`, `--settings`,
  `--append-system-prompt`, `--session-id`, `--verbose`.
- **Wire format** — `stream-json` events, including `init`, `compact_boundary`
  and result subtypes.

### Differences to know about

- **No Node.js.** A single binary; faster startup, lower memory.
- **No `--model` flag.** Use `/model`, settings, or `ANTHROPIC_MODEL` /
  `OPENAI_MODEL`.
- **Some TypeScript flags are absent**: `--debug`,
  `--dangerously-skip-permissions` (use `--permission-mode
  bypassPermissions`), `--fork-session` (use the `fork` subcommand),
  `--strict-mcp-config`, `--include-partial-messages`, `--max-turns`,
  `--agents`, `--system-prompt` (only `--append-system-prompt`).
- **Hook coverage is partial** — see the hooks row in
  [Configuration and extensions](#configuration-and-extensions).
- **The `stream-json` control union is a subset** — unsupported subtypes get a
  structured error instead of being silently dropped.
- **Feature lag.** New TypeScript-only features can take a while to land; the
  maturity tables above are the current honest picture.

## Development

```bash
scripts/check.sh                # canonical: fmt → clippy → check → test
scripts/check.sh --quick        # fmt → clippy → check (no tests)
scripts/check.sh --release      # same pipeline, release profile
scripts/check.sh --pty-e2e      # only the #[ignore]d PTY e2e tests, serially

cargo check --workspace
cargo test --workspace --no-fail-fast
cargo fmt --all --check         # rustfmt is the formatting authority
cargo clippy --workspace

cargo test -p orbcode-core retry            # one test by name
cargo test -p orbcode --test stream_json_e2e  # one integration file
```

Around 3,700 unit and integration tests currently run under
`cargo test --workspace`. A handful of PTY-level TUI tests are `#[ignore]`d
because they are load-sensitive; CI runs them in a dedicated serial job.

Other useful scripts: `scripts/audit-brand.sh` (naming guard — fails both on a
leaked pre-rename identifier and on a compatibility alias going missing),
`scripts/audit-public-surface.sh`, `scripts/smoke-release.sh`,
`scripts/ci-cross-platform-smoke.sh`.

`scripts/tui-native-scrollback-tmux-smoke.sh` is the manual counterpart to the
above: it drives the real TUI inside a live tmux pane to check the surfaces no
headless test reaches — native scrollback, mouse drag and wheel, resize during
streaming, the transcript pager. It needs a local `tmux` and a real terminal, so
it is deliberately outside CI; run it by hand after touching the TUI's terminal
handling. `--help` lists the individual smoke modes.

### Running from source

Anything after `--` is passed to the binary; env vars go in front of `cargo`, so
one command can pin both the state directory and the diagnostics:

```bash
# Interactive TUI against a throwaway home, with a terminal-write trace
ORBCODE_TUI_TERMINAL_TRACE=/tmp/orbcode-trace.jsonl \
ORBCODE_HOME="$HOME/.orbcode-dev" \
  cargo run -p orbcode

# Same thing headless, so the trace covers exactly one turn
ORBCODE_HOME="$(mktemp -d)" \
  cargo run -p orbcode -- -p --output-format stream-json "hello"

# Release build, arguments after `--`
cargo run --release -p orbcode -- --continue
```

Pointing `ORBCODE_HOME` at a scratch directory is the normal way to develop:
it keeps experiments out of the `~/.claude` state you use day to day, and a
fresh directory starts empty, so it also reproduces the first-run path
(no credentials, no sessions, no settings). Use `--settings` for a
per-invocation settings overlay without touching the home at all.

### Debugging and tracing

All of these are diagnostics, off unless set, and none are part of the
compatibility surface — they can change at any time.

| Variable | Effect |
| --- | --- |
| `ORBCODE_TUI_TERMINAL_TRACE=<path>` | Append a JSONL trace of terminal writes, resizes and viewport geometry — each record carries the escaped ANSI bytes (capped at 32 KiB per write). `1` writes to `$TMPDIR/orbcode-tui-terminal-trace-<pid>.jsonl`. The primary tool for scrollback and viewport bugs. |
| `ORBCODE_TUI_RENDER_METRICS=1` | Per-frame render metrics as JSONL: redraw reasons, event counts, terminal vs viewport size, line counts. Path from `ORBCODE_TUI_RENDER_METRICS_PATH`, default `$TMPDIR/orbcode-tui-render-metrics-<pid>.jsonl`. Only the literal `1` enables it. |
| `ORBCODE_TUI_RESIZE_SETTLE_MS=<ms>` | Resize debounce before the full history rebuild (default `150`). Raise it for terminals with a slow drag cadence. |
| `ORBCODE_DEBUG_PROVIDER_ROUNDS=1` | Append per-provider-round diagnostics to the transcript as `system` messages. |
| `ORBCODE_DEBUG_AUTO_CONTINUE=1` | Append `[debug:auto-continue]` decision records (stop reason, message shape, attempt count) to the transcript; also implies the provider-round diagnostics. |
| `ORBCODE_DOCTOR_PROBE=1` / `ORBCODE_DOCTOR_MCP_PROBE=1` | Let `orbcode doctor` actually reach the network: a live provider request and MCP reachability checks. Without them those two checks report `warn` and stay offline. |
| `ORBCODE_FORCE_RG_FALLBACK=1` | Force the `grep` tool's built-in engine instead of `rg`, to test the no-ripgrep path. |
| `ORBCODE_TRUSTED_PROJECT=0` | Treat the working directory as untrusted (default is trusted), which disables project-supplied hooks. |

Test-harness switches, for when a suite skips instead of running:
`ORBCODE_REQUIRE_NODE=1` turns "Node not found, skipping" into a failure for the
TypeScript-reference e2e tests; `ORBCODE_RUN_MACOS_SANDBOX_HOST_TESTS=1` (and the
`LINUX` / `WINDOWS` variants) opt into sandbox tests that need real host
facilities; `ORBCODE_UPDATE_STREAM_JSON_GOLDENS=1 cargo test -p orbcode --test
compat_stream_json` rewrites the `stream-json` goldens after an intentional
wire-format change — inspect the diff before committing it.

### Crate layout

Dependency DAG, lowest layer first:

```
protocol         shared serde types: StreamEvent, TranscriptMessage, SessionRecord, ...
config           settings layering, auth, permission rules, home resolution, memory, plugins
model-provider   streaming HTTP providers, retry, rate limits, token counting
session-store    JSONL transcripts, prompt history, child sessions, live-session registry
mcp              MCP client: transports, OAuth, registry, trust, hot reload
tools            tool adapters + ToolRegistry::foundation()
core             orchestration: SessionManager, turn loop, agent loop, permissions, hooks, compaction
app-server       in-process facade over core + tools + mcp + config
app-server-protocol / -client / -transport   canonical multi-client protocol boundary
tui              ratatui interface
cli              the `orbcode` binary
compat-fixtures  dev-only golden TypeScript-vs-Rust fixture corpus
```

Cargo packages use the `orbcode-*` prefix, except the binary crate, which is
just `orbcode`. Put a new cross-layer type in the lowest crate that needs it —
usually `protocol`.

Before opening a pull request, read [CONTRIBUTING.md](CONTRIBUTING.md) — it is
short, and it lists the changes that look like cleanup but are actually breaking
(renaming a TypeScript-CLI compatibility name, editing a golden fixture).
[AGENTS.md](AGENTS.md) is the long form of the same expectations; architecture
notes live in [CLAUDE.md](CLAUDE.md).

## Documentation

| Document | Contents |
| --- | --- |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to build, the one command CI runs, and the four things that will fail review. |
| [AGENTS.md](AGENTS.md) | Full contributor and agent guide: commit style, PR expectations, testing conventions. |
| [CLAUDE.md](CLAUDE.md) | Architecture: crate layering, the request/turn flow, configuration and permission internals. |
| [SECURITY.md](SECURITY.md) | How to report a vulnerability privately, and which boundaries are in scope. |
| [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) | Contributor Covenant 2.1. |

Module-level doc comments carry the detail for individual subsystems — start at
each crate's `lib.rs`, which re-exports and documents its public surface.

## License

[Apache-2.0](LICENSE).

### Trademarks and affiliation

Orb Code is an independent, unofficial reimplementation. It is not affiliated
with, authorized by, endorsed by or sponsored by Anthropic, and Anthropic bears
no responsibility for it. Questions and bug reports about this project belong in
this repository's issues, never in Anthropic's support channels; questions about
Anthropic's own CLI belong with Anthropic, not here.

"Anthropic", "Claude" and "Claude Code" are trademarks of Anthropic PBC. This
project uses those names only nominatively: to say which CLI's behaviour,
on-disk formats and wire protocol it reimplements, and which models and API it
speaks to. No trademark rights or license are claimed, and Apache-2.0 grants
none — see section 6 of [LICENSE](LICENSE).

The distinction is built into the code, not just asserted here. The binary is
`orbcode` and every environment variable this project introduces is `ORBCODE_*`,
so nothing it produces presents itself as Anthropic's own tool. The
compatibility names it must keep honouring — `~/.claude`, `CLAUDE_CONFIG_DIR`,
`ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN` — are interoperability
requirements rather than branding, and `scripts/audit-brand.sh` enforces that
line in both directions.
