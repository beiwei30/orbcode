# Troubleshooting

[English](troubleshooting.md) · [简体中文](zh-CN/troubleshooting.md)

Start with `orbcode doctor`. It checks build metadata, workspace/home, provider
chain, model capabilities, auth, network/tool permissions, sandbox, required
binaries, session storage, background jobs, MCP, and extension loading without
making network calls by default.

## Credentials or provider failures

```bash
orbcode auth status
orbcode providers
orbcode doctor
```

- Verify the active provider. `PROVIDER_TYPE` in settings can select OpenAI even
  when you expected Anthropic.
- An OpenAI API key wins over a stored ChatGPT login. Temporarily unset both
  `OPENAI_API_KEY` and `ORBCODE_OPENAI_API_KEY` to test the subscription path.
- ChatGPT credentials live in `<home>/auth.json`, not `~/.codex/auth.json`.
- Gemini/Grok always fail because their adapters are not implemented.
- Custom base URLs are honored for API-key adapters but intentionally ignored
  for the fixed ChatGPT subscription backend.
- Set `ORBCODE_DOCTOR_PROBE=1` only when you want doctor to make a tiny live
  provider request.

For a 401, re-run the appropriate login or replace the environment key. For
timeouts, inspect proxy selection and `ORBCODE_API_TIMEOUT_MS`; do not solve a
credential error by increasing retries.

## Missing settings or sessions

Check the home printed by `orbcode doctor`. Resolution is `ORBCODE_HOME`, then
`CLAUDE_CONFIG_DIR`, existing `~/.orbcode`, then `~/.claude`.

The common surprise is an empty `~/.orbcode` directory shadowing populated
`~/.claude` state. Nothing is migrated automatically. Remove/rename the empty
directory or copy state deliberately while Orb Code is stopped.

Use `orbcode sessions --json` to distinguish an empty session list from a TUI
filter. `--continue` is workspace-specific. A transcript from another project
will not be selected as the latest session for the current workspace.

## Tool denied

```bash
orbcode tools
orbcode --permission-mode default -p "..."
```

Check, in order:

1. `allow_tools` master switch and the active preset.
2. A matching deny rule (deny always wins).
3. A matching ask rule or outside-workspace boundary.
4. Network permission for web tools.
5. OS sandbox errors for Bash.
6. For MCP, both the `mcp__...` rule and server trust.

`Approve for me` still reviews boundary requests; it is not Full Access. In
headless execution without an interactive permission responder, use explicit
narrow rules rather than depending on a prompt.

## Sandbox fails to start

Run `orbcode doctor` and inspect the sandbox row.

- macOS needs the system `sandbox-exec` command.
- Linux needs `bwrap` on `PATH`; missing Bubblewrap is a hard error when the
  sandbox is requested.
- `danger-full-access` means no OS sandbox, even if permission rules are narrow.
- `excludedCommands` run outside the sandbox when allowed; they are not a deny
  list.
- Set `--sandbox-network false` to separate network policy from filesystem
  policy while diagnosing.

## MCP server unavailable

```bash
orbcode mcp servers
orbcode mcp diagnose <SERVER>
orbcode mcp auth status
orbcode mcp tools <SERVER>
```

Configuration expansion fails when `${VAR}` has no value/default. A stdio
server also needs a valid command/cwd/environment. Remote servers may require a
fresh OAuth token. After connectivity works, trust the server and add a
matching tool permission. `diagnose` bypasses the call gate only for diagnosis.

Set `ORBCODE_DOCTOR_MCP_PROBE=1` to include live MCP reachability in doctor.

## Background or child session looks orphaned

Use `orbcode ps`, `orbcode logs <JOB_ID>`, and `orbcode attach <JOB_ID>` first.
If parent transcripts were deleted outside Orb Code, preview cleanup:

```bash
orbcode doctor cleanup-orphans --dry-run
orbcode doctor cleanup-orphans --dry-run --stale-running-days 7
```

Only run with `--yes` after reviewing the exact candidates. This removes orphan
metadata/artifacts, not healthy parent transcripts.

## TUI display problems

Try `/theme`, `/keybindings`, and `/vim` to rule out configuration. Preserve a
terminal trace for reproducible scrollback/resize bugs and include terminal,
tmux, OS, and window-size details in the issue. Do not attach a trace until you
have checked it for prompt/tool content.

## Diagnostic switches

These are unstable diagnostics, not compatibility promises:

| Variable | Effect |
| --- | --- |
| `ORBCODE_DOCTOR_PROBE=1` | Make a small live provider probe. |
| `ORBCODE_DOCTOR_MCP_PROBE=1` | Probe configured MCP server reachability. |
| `ORBCODE_TUI_TERMINAL_TRACE=<path>` | Append escaped terminal writes/resizes/viewport data as JSONL; `1` chooses a temp path. |
| `ORBCODE_TUI_RENDER_METRICS=1` | Emit per-frame render metrics; optional path via `ORBCODE_TUI_RENDER_METRICS_PATH`. |
| `ORBCODE_TUI_RESIZE_SETTLE_MS=<ms>` | Override resize debounce (default 150 ms). |
| `ORBCODE_DEBUG_PROVIDER_ROUNDS=1` | Append provider-round diagnostics to the transcript. |
| `ORBCODE_DEBUG_AUTO_CONTINUE=1` | Append auto-continuation decisions and provider-round diagnostics. |
| `ORBCODE_FORCE_RG_FALLBACK=1` | Force the built-in Grep engine. |
| `ORBCODE_TRUSTED_PROJECT=0` | Disable project-origin hooks by marking the project untrusted. |

Render metrics are unstable diagnostics. Their `output.draw_command_count` is
the total number of logical terminal commands queued for that frame: cursor,
style, print, and clear commands plus the line-wrap disable/enable pair. An
unchanged incremental frame therefore has a fixed count of five (two line-wrap
commands and three style resets), while `output.bytes` records encoded bytes.

## Reporting a useful issue

Include `orbcode --version`, OS/target, the redacted `doctor` rows, exact command
and exit code, active home choice, and the smallest reproducer. Never post API
keys, OAuth tokens, full auth files, private transcripts, MCP headers, or
unreviewed terminal traces. Report vulnerabilities through
[SECURITY.md](../SECURITY.md), not a public issue.
