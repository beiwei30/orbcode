# ACP with Zed

Last verified: 2026-08-05

This guide configures Zed to launch `orbcode acp` as a custom external agent
and records the current real-editor sign-off. The exact supported protocol
surface is in [ACP support matrix](acp-support.md).

## Verified baseline

| Component | Version |
| --- | --- |
| orbcode | 0.0.1, repository state based on commit `2a7d48b` plus the ACP productization changes |
| ACP Rust SDK / schema | 0.14.0 / 0.13.6 |
| Zed | 1.13.2, build 20260802.163511 |
| Platform | macOS 26.6, build 25G72, Apple silicon |

Later Zed releases should work when they retain ACP v1 compatibility, but rerun
this checklist before changing the verified version in the README.

## Prerequisites

1. Install Zed and build orbcode from the repository root:

       cargo build -p orbcode

2. Configure one supported provider without putting a token in the repository,
   Zed settings, screenshots, or captured logs. Environment variables supplied
   by a secure launcher are preferred.
3. Create a disposable home for the smoke and print its exact path:

       ORBCODE_ZED_SMOKE_HOME="$(mktemp -d)"
       printf '%s\n' "$ORBCODE_ZED_SMOKE_HOME"

   Use the printed path as `ORBCODE_HOME` in the Zed configuration below.
   Remove that exact directory after the run, and do not point the setting at a
   normal Claude or orbcode home.

For a deterministic repository-only run, the ACP process tests use the
`mock-provider` development feature and `mock://` URLs. That mock is test-only:
do not enable it in a production build or ship a Zed configuration containing
the test key.

## Zed configuration

Add a custom server under `agent_servers` in Zed's `settings.json`. Replace the
paths and provider environment with values for your machine. The `ORBCODE_HOME`
value must be the disposable path printed in prerequisite step 3:

```jsonc
{
  "agent_servers": {
    "orbcode": {
      "type": "custom",
      "command": "/absolute/path/to/orbcode/target/debug/orbcode",
      "args": ["acp"],
      "env": {
        "ORBCODE_HOME": "/absolute/path/to/a/disposable/home"
      }
    }
  }
}
```

Open the repository folder in Zed, open the Agent Panel, choose `orbcode` as an
external agent, and start a new thread. Zed passes the project working directory
as the ACP session `cwd`; orbcode requires it to be absolute. Sessions returned
by list/load/resume/delete are restricted to this launch directory.

Zed documents custom ACP agents and ACP logs in its
[External Agents guide](https://zed.dev/docs/ai/external-agents). It forwards
configured context servers to external agents as described in its
[MCP guide](https://zed.dev/docs/ai/mcp).

## Repeatable checklist

Use a clean disposable home and record the Zed, orbcode, and OS versions.

### Lifecycle and streaming

- Start a new external-agent thread and send `Reply exactly ZED_ACP_SMOKE_OK.`
  Confirm that text appears incrementally and the turn finishes.
- Start a long request, cancel it from Zed, then send another prompt in the same
  thread. Confirm the first turn stops and the second runs.
- Close the thread while a turn is active, then confirm no `orbcode acp` child
  remains. Quitting Zed exercises the EOF cleanup path as well.
- Restart Zed and reopen/import the thread. Confirm prior user and assistant
  messages replay in order and a new prompt works.
- In Zed's thread history, verify list, load, resume, and delete for a session
  in the current project. Confirm a session from another project is not exposed.

### Modes and configuration

- Verify the mode selector offers only Default and Plan. It must not offer
  bypass-permissions or auto.
- Select Plan and request a file or command mutation. The model must not receive
  mutation/network tools for that turn.
- Change Model and Thought level while idle and confirm the selection remains
  local to the thread. Open a second thread and confirm it retains its own
  values.
- Attempt to change a control during an active turn. Zed should show a typed
  error; retry after cancellation or completion.

### Permissions and AskUser

- Trigger a protected command. Choose Allow once and confirm it runs; trigger a
  second request and choose Reject once, confirming the tool ends without
  running.
- Trigger an `AskUserQuestion` with explicit choices. Select one choice and
  confirm exactly one answer reaches the turn.
- An option-free AskUser request is expected to cancel without showing an
  elicitation form. This is deliberate while `unstable_elicitation` is disabled.

### MCP

- Configure one disposable stdio MCP server in Zed, start a new orbcode thread,
  and invoke one tool. Confirm a trust request appears before the first call.
- Repeat with a disposable Streamable HTTP server. Test both trust approval and
  denial, then close the thread and confirm neither server was written to the
  normal orbcode MCP registry.
- SSE and MCP-over-ACP are expected to fail setup with a typed error.

### Prompt content

- Send text plus an embedded text resource and a resource link. Confirm their
  order and source attribution. The link must not be fetched merely because it
  was supplied.
- Image, audio, blob resources, and unknown content are expected to fail before
  a model turn begins.

## Recorded sign-off

The 2026-08-05 run used a clean temporary `ORBCODE_HOME` and a deterministic
test provider containing no real provider secret. The temporary Zed agent and
key bindings were removed after the run, the temporary home was deleted, and
quitting Zed left no `orbcode acp` child process.

| Check | Evidence |
| --- | --- |
| External-agent startup and ACP initialize/new | PASS in Zed |
| Prompt submission, streamed response, completion | PASS in Zed |
| Restart and thread/history restoration | PASS in Zed |
| Model selector and thought-level change | PASS in Zed; Sonnet and Low were selected and displayed after restart |
| Protected command permission — Reject once | PASS in Zed; the command card completed without execution |
| Tool title | PASS in Zed; the card showed `Run Command` and `bash(echo zed-acp-tool)` |
| Close/EOF child-process cleanup | PASS in Zed plus process inspection |
| Allow once, cancellation, mode switching, AskUser, stdio/HTTP MCP, list/resume/delete | Covered by raw-process and official SDK-client E2E; retained in the manual checklist for a release-candidate sweep |
| Unsupported content and zero turn submission | Covered by raw-process E2E |

The extended rows are not claimed as UI-observed in this run: Zed's Agent Panel
canvas did not expose its composer as an accessibility element to the automated
desktop harness. Their protocol behavior is nevertheless pinned by the focused
matrix in the readiness plan.

## Troubleshooting

- In Zed, run `dev: open acp logs` and inspect only redacted output. ACP uses
  stdout for JSON-RPC, so diagnostic output must stay on stderr.
- If the agent does not appear, verify the binary path is absolute and run
  `orbcode doctor` from the same environment.
- If session setup fails, verify `cwd` and every additional directory are
  absolute and accessible.
- If model or thought-level controls are absent, the selected provider/model
  may not advertise corresponding canonical options.
- If a mode/config change fails during a turn, cancel or wait for completion;
  active-turn mutation is intentionally rejected.
- If an MCP server never becomes ready, inspect its transport, command/URL, and
  trust decision. Only stdio and Streamable HTTP are supported.
- Before sharing logs, remove tokens, HTTP headers, local paths, prompt text,
  transcripts, and MCP payloads.
