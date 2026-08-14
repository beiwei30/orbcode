# Headless and stream-JSON

[English](stream-json.md) · [简体中文](zh-CN/stream-json.md)

Orb Code supports simple one-shot output and a duplex NDJSON control channel.
The event schema is designed for Claude Code SDK compatibility; the supported
control union is deliberately explicit.

## Output modes

```bash
orbcode -p "explain this repository"                         # text
orbcode -p --output-format json "explain this repository"    # one JSON result
orbcode -p --verbose --output-format stream-json "explain"   # NDJSON events
```

- `text` prints the final assistant text.
- `json` emits one result object with session, usage, cost, subtype, and error
  information.
- `stream-json` emits initialization, assistant/content/tool/progress events,
  compaction boundaries, and a terminal result. `--verbose` is required.

Events are written one JSON object per line. Keep stdout exclusively for the
protocol; send application logging elsewhere.

## Duplex input

Set both formats to keep one process open for user frames and controls:

```bash
printf '%s\n' \
  '{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}' \
  '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}' \
  | orbcode -p --verbose \
      --input-format stream-json --output-format stream-json
```

User content may be a string or compatible content-block array. Malformed
lines and unsupported requests yield structured correlated errors where a
request ID is available; they do not silently disappear.

## Control envelopes

Host-to-CLI requests use:

```json
{
  "type": "control_request",
  "request_id": "model-1",
  "request": { "subtype": "set_model", "model": "sonnet" }
}
```

The response is:

```json
{
  "type": "control_response",
  "response": {
    "subtype": "success",
    "request_id": "model-1",
    "response": { "model": "sonnet" }
  }
}
```

Errors use `subtype: "error"` and an `error` string. Responses retain input
ordering relative to assistant and terminal events.

## Supported controls

| Subtype | Direction | Effect |
| --- | --- | --- |
| `initialize` | Host → CLI | Idempotent session, model, tools, MCP, and capability snapshot. |
| `interrupt` | Host → CLI | Interrupt the active turn; safe when idle. |
| `set_permission_mode` | Host → CLI | Change the next permission decision mode. |
| `get_session_state` | Host → CLI | Typed authoritative session state. |
| `get_context_usage` | Host → CLI | Current context/token breakdown. |
| `mcp_status` | Host → CLI | Secret-free MCP status. |
| `set_model` | Host → CLI | Set or clear the model used by the next provider request. |
| `set_max_thinking_tokens` | Host → CLI | Set/clear a validated Anthropic thinking-token override. |
| `seed_read_state` | Host → CLI | Validate and seed file identity for stale-write protection. |
| `cancel_async_message` | Host → CLI | Signal one owned prompt job, local agent, workflow, or shell task. |
| `can_use_tool` | CLI → host | Ask the host to resolve an existing tool permission request. |

`rewind_files` is recognized but returns an error because transcript rewind is
not file restoration. Unknown future subtypes also return a correlated
unsupported error.

`cancel_async_message` reports `signalled`, `already_terminal`, or `not_found`.
It cannot cancel arbitrary work owned by another session.

## Tool permission callback

When a tool requires approval, the CLI sends a server-originated
`can_use_tool` control request containing tool name, input, tool-use ID, and
boundary context. The host replies with a `server_response`/correlated response
whose behavior is:

- `allow`, optionally with `updatedInput` and `toolUseID`; or
- `deny`, with a message and optional `interrupt`.

Each request resolves once. EOF denies pending approvals so a turn cannot stay
blocked forever. See [Interactive questions](interactive-questions.md) for the
separate `ask_user` server request and capability negotiation.

## Compatibility events

The stream includes the compatibility-critical `system` `init` event and
terminal `result` subtype strings. Compaction emits `compact_boundary`. Session
and tool progress is produced from the same typed `StreamEvent` contract used
by the TUI and app-server, reducing adapter-only behavior.

The TypeScript CLI collapses process failure mostly to 0/1; Orb Code preserves
result strings but provides the richer process exit mapping documented in the
[CLI reference](cli-reference.md#exit-codes).

For consumers that need persistent multi-session control rather than an owned
stdio process, use the experimental [app-server protocol](integrations.md#app-server-protocol).
