# User guide

[English](user-guide.md) · [简体中文](zh-CN/user-guide.md)

Orb Code exposes the same core through an interactive TUI, one-shot prompts,
background jobs, ACP, and the app-server protocol. This page covers the normal
local workflows.

## Interactive TUI

Run `orbcode` or `orbcode tui`. A positional prompt seeds the first turn.

Important keys:

| Key | Action |
| --- | --- |
| `Enter` | Submit the input. |
| `Shift+Enter`, `Alt+Enter`, `Ctrl+Enter` | Insert a newline. |
| `Tab` | Complete a slash command, or insert spaces. |
| `Shift+Tab` | Cycle Ask for approval, Approve for me, Full Access, and Plan. |
| `Ctrl+R` | Open session history. |
| `Ctrl+O` | Toggle detailed tool/transcript display. |
| `PageUp` / `PageDown`, `Home` / `End` | Navigate the timeline. |
| `Esc` | Cancel the active turn, close an overlay, or leave Vim insert mode. |
| `Ctrl+C` | Cancel; press again when idle to exit. |

Mouse selection and wheel scrolling use the terminal's native scrollback where
available. Run `/help` for the complete live key list and `/keybindings` for
the configured overrides.

### Slash commands

The built-in commands are grouped here by purpose. Aliases appear in
parentheses.

| Purpose | Commands |
| --- | --- |
| Help and diagnostics | `/help` (`/?`), `/doctor`, `/config`, `/status`, `/context` (`/ctx`), `/stats`, `/usage`, `/cost`, `/trace`, `/diff`, `/release-notes` |
| Project prompts | `/init`, `/review` |
| Sessions | `/sessions`, `/resume` (`/session`), `/rename`, `/fork`, `/branch`, `/clear` (`/new`, `/reset`), `/rewind` (`/checkpoint`), `/compact` |
| Models and behavior | `/model`, `/effort`, `/permissions`, `/sandbox`, `/plan`, `/goal`, `/output-style`, `/memory`, `/instructions` |
| Tools and extensions | `/tools`, `/tool`, `/mcp`, `/agents`, `/skills`, `/hooks`, `/files`, `/add-dir` (`/add-directory`), `/jobs` (`/background`) |
| Appearance and input | `/theme`, `/vim`, `/keybindings`, `/copy` |
| Account and exit | `/login`, `/logout`, `/exit` (`/quit`) |

The command palette also includes project/user commands, enabled plugin
commands, skills, trusted MCP prompts, and workflows. Their availability is
therefore workspace- and configuration-dependent.

## Headless prompts

Use either spelling for one turn:

```bash
orbcode -p "explain the failure"
orbcode prompt "explain the failure"
```

`text` prints the final assistant text, `json` prints one result object, and
`stream-json` emits NDJSON events. Stream output requires `--verbose`. See
[Headless and stream-JSON](stream-json.md) for the duplex protocol.

Headless prompts are single-turn. Persistent goals do not make an unattended
print process continue indefinitely.

## Tools

`orbcode tools` prints the current registry, including permission and network
requirements. The foundation registry includes:

- Files and shell: `Read`, `Edit`, `Write`, `Glob`, `Grep`, `Bash`, and
  `NotebookEdit`.
- Web: `WebFetch` and `WebSearch`.
- Planning and tasks: plan entry/exit/verification, todos, task list operations,
  task output, and task cancellation.
- Agents, skills, tool discovery, dynamic workflows, and heuristic LSP queries.
- `AskUserQuestion` only when the active client advertises the full interaction
  capability.

`orbcode tool <TOOL_NAME> [JSON_INPUT]` invokes one tool directly for debugging.
It still passes through the normal permission policy. MCP and plugin tools join
the registry dynamically; use `orbcode tools` rather than relying on a fixed
count.

Plan verification records a snapshot and plan state; it is not a substitute for
running the relevant build or tests.

## Sessions

Transcripts are persisted as JSONL under `<home>/projects/<workspace-slug>/` in
the Claude Code-compatible format. Writes are flushed in order. Large tool
results may be stored out of line with a preview in the transcript.

```bash
orbcode sessions                 # human-readable list
orbcode sessions --json          # one JSON object per line
orbcode --continue               # latest session for this workspace
orbcode --resume <SESSION_ID>
orbcode resume <SESSION_ID>
orbcode rename <SESSION_ID> "new title"
orbcode fork <SESSION_ID> --title "alternative"
orbcode fork <SESSION_ID> --prompt "try another approach" --tui
```

`--session-id <ID>` selects or creates a specific session. `--resume` resumes
existing state; do not use the two interchangeably in automation. A bare `-r`
can consume the following non-option token as the session ID, so prefer
`--resume=<ID>` in generated commands.

The TUI can rewind to a transcript checkpoint with `/rewind`. This rewinds
conversation state only; it does not restore workspace files.

## Background jobs

Queue a prompt without keeping the caller attached:

```bash
orbcode prompt --bg "run the full test suite and summarize failures"
orbcode ps
orbcode logs <JOB_ID>
orbcode logs <JOB_ID> --follow
orbcode attach <JOB_ID>
orbcode kill <JOB_ID>
```

Jobs persist their status and log. The runtime distinguishes queued, running,
completed, failed, cancelled, and orphaned work. `/jobs` opens the same surface
inside the TUI. `doctor cleanup-orphans --dry-run` previews stale child-session
metadata; deletion requires `--yes`.

## Context and compaction

`orbcode context` shows the instructions, workspace roots, and git context that
would seed a session. `/context` shows the current token-usage breakdown.

When the configured window is approached, Orb Code can compact older context
into a summary and emits a `compact_boundary` in compatible stream output.
Use `/compact` to request it manually. The main controls are
`ORBCODE_MAX_CONTEXT_TOKENS` and `ORBCODE_AUTO_COMPACT_WINDOW`.

Prompt history, child sessions, and the live-session registry are stored under
the active home. Do not commit them.

## Plans, tasks, agents, and workflows

- Plan mode hides execution tools while the model writes a plan. `/plan`
  controls the mode; the plan tools persist state under `<home>/plans/`.
- Todo state is lightweight turn guidance. Task tools provide durable units with
  pending, in-progress, and completed states.
- `Agent` runs a configured local subagent synchronously. Agent definitions can
  come from built-ins, the user home, the project, or plugins.
- `Workflow` starts a generated dynamic workflow as a durable background task.
  This surface is experimental.

See [Extensions](extensions.md) for authoring agents, skills, commands, and
workflows.

## Persistent goals

One experimental persistent goal can be attached to a session. `/goal` creates,
shows, clears, or resumes it. A capable interactive client supervises each
continuation; the model cannot silently replace an active goal or give itself a
larger token budget.

Goal tools (`get_goal`, `create_goal`, `update_goal`) appear only for an active,
capability-scoped goal turn. Model authority is limited to completing the goal
or reporting it blocked under the runtime's policy. Restarts recover the
transcript-backed state. Read [Persistent goals](persistent-goals.md) for the
state machine and client matrix.
