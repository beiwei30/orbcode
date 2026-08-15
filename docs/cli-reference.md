# CLI reference

[English](cli-reference.md) · [简体中文](zh-CN/cli-reference.md)

`orbcode --help` and `orbcode help <command>` are authoritative for the current
binary. Global options may appear before or after subcommands.

```text
orbcode [OPTIONS] [PROMPT] [COMMAND]
```

With no command, Orb Code starts the TUI. With `-p/--print`, the positional
prompt runs headlessly.

## Global options

| Area | Options |
| --- | --- |
| Session | `-c/--continue`, `-r/--resume [ID]`, `--session-id ID` |
| Headless | `-p/--print`, `--output-format text|json|stream-json`, `--input-format text|stream-json`, `--verbose`, `--append-system-prompt TEXT` |
| Provider | `--provider`, `--fallback-provider`, `--max-retries` |
| Permissions | `--permission-mode default|bypassPermissions|plan|auto`, `--allow-tools BOOL`, `--allow-network BOOL`, `--allowed-tools RULES`, `--disallowed-tools RULES`, `--add-dir DIR` |
| Sandbox | `--sandbox-mode danger-full-access|workspace-write|read-only`, `--sandbox-network BOOL` |
| Config | `--settings FILE_OR_JSON`, repeated `--mcp-config FILE_OR_JSON` |

The provider enum also accepts `gemini` and `grok`, but those adapters are not
implemented. There is no `--model`; use `/model`, settings, or provider model
environment variables.

`stream-json` output in print mode requires `--verbose`. `--resume` without a
value selects the latest session; a following positional token can be consumed
as its value, so automation should use `--resume=<ID>`.

## Commands

### Sessions and turns

| Command | Purpose |
| --- | --- |
| `tui` | Start the local interactive UI. |
| `prompt <PROMPT> [--session ID] [--bg]` | Run one headless turn or queue a background job. |
| `resume <SESSION_ID> [PROMPT]` | Open an existing session. |
| `fork <SESSION_ID> [--title T] [--note N] [--prompt P] [--tui]` | Create a new session from an existing transcript. |
| `sessions [--json]` | List persisted sessions. JSON mode is NDJSON. |
| `rename <SESSION_ID> <NEW_TITLE>` | Override the generated title. |

### Background work

| Command | Purpose |
| --- | --- |
| `ps` | List persisted background prompt jobs. |
| `logs <JOB_ID> [--follow]` | Print/follow a job log. |
| `attach <JOB_ID>` | Attach the TUI to a job/session. |
| `kill <JOB_ID>` | Cancel a background job. |

### Inspection and direct tools

| Command | Purpose |
| --- | --- |
| `providers` | Active provider chain, models, capabilities, and permission summary. |
| `context` | Preview instructions, roots, and git context. |
| `tools` | Print the live tool registry. |
| `tool <TOOL_NAME> [JSON_INPUT] [--session ID]` | Invoke one tool through normal permissions. |
| `doctor` | Offline-first environment health checks. |
| `doctor cleanup-orphans --dry-run|--yes [--stale-running-days N]` | Preview or remove orphan child-session artifacts. |
| `advanced` | Print active and deferred advanced capabilities. |

### Authentication

```bash
orbcode auth status
orbcode auth login --provider <PROVIDER> \
  --method api-key|o-auth-device|chatgpt [--token TOKEN] [--env-var NAME]
orbcode auth login --provider openai --method chatgpt [--device-code]
orbcode auth logout [--provider PROVIDER]
```

Without `--provider`, logout removes all persisted provider auth metadata.

### MCP

The command family includes:

```text
mcp capabilities
mcp servers
mcp diagnose SERVER
mcp add SERVER --transport TRANSPORT --endpoint ENDPOINT [--summary S] [--auth SPEC] [--enabled]
mcp remove SERVER
mcp tools SERVER
mcp call SERVER TOOL [JSON_INPUT] [--session ID]
mcp resources SERVER
mcp read SERVER URI
mcp prompts SERVER
mcp prompt SERVER PROMPT [JSON_ARGUMENTS]
mcp trust|distrust|untrust SERVER
mcp auth status|login|device-login|browser-login|logout ...
```

See [MCP](mcp.md) for configuration, OAuth, and the two-gate call policy.

### Integrations

| Command | Status | Purpose |
| --- | --- | --- |
| `acp` | Experimental | Agent Client Protocol v1 adapter over stdio. |
| `serve --stdio` | Experimental, hidden | App-server protocol over a supervised stdio process. |
| `serve --socket PATH [--auth-token TOKEN]` | Experimental, hidden | Unix socket listener. |
| `serve --websocket ADDR [--auth-token TOKEN] [--allowed-origin ORIGIN]...` | Experimental, hidden | WebSocket listener. |
| `remote ENDPOINT --token TOKEN` | Experimental | TUI backed entirely by an existing socket/WebSocket server. |

Socket/WebSocket endpoints allow sequential reconnects but one active client at
a time. Their token is generated and printed in startup JSON when omitted.
Stdio is bound to its parent process and implicitly trusted.

## Recipes

```bash
# Machine-readable result.
orbcode -p --output-format json "summarize the repository"

# Streaming events.
orbcode -p --verbose --output-format stream-json "run the focused test"

# One-off settings and MCP config.
orbcode --settings '{"model":"sonnet"}' --mcp-config ./ci/mcp.json \
  -p "summarize incidents"

# Restricted workspace automation.
orbcode --allow-tools true \
  --allowed-tools "Read,Grep,Bash(cargo test:*)" \
  --sandbox-mode workspace-write --sandbox-network false \
  -p "diagnose the test failure"
```

## Exit codes

Headless result subtypes stay compatible while the process exposes more useful
exit codes:

| Code | Meaning | Result subtype |
| --- | --- | --- |
| `0` | Success | `success` |
| `1` | Provider, model, or tool execution error | `error_during_execution` |
| `2` | Invalid arguments or missing prompt before execution | No result event |
| `3` | Credentials rejected | `error_during_execution` |
| `4` | Tool denied by permission policy | `error_during_execution` |
| `5` | Turn interrupted | `error_during_execution` |
| `6` | Turn ceiling | `error_max_turns` (reserved) |
| `7` | Dollar budget ceiling | `error_max_budget_usd` (reserved by the exit mapping) |

Codes 5–7 are pinned by tests but are not all reachable from a normal print
subprocess. In particular, print mode currently uses the process's default
SIGINT behavior, so terminal Ctrl-C normally exits as 130.
