# ACP support matrix

Last verified: 2026-08-05

`orbcode acp` is an experimental Agent Client Protocol (ACP) v1 adapter over
stdio. The canonical runtime API remains `AppClient` and
`app-server-protocol`; ACP schema types and translations stay in
`cli/src/acp_sdk/`.

## Locked protocol dependencies

| Dependency | Version | Feature policy |
| --- | --- | --- |
| `agent-client-protocol` | 0.14.0 | Default features only; its default feature set is empty. |
| `agent-client-protocol-schema` | 0.13.6 | Resolved transitively by the SDK. |

The locked SDK offers these optional features: `unstable_auth_methods`,
`unstable_boolean_config`, `unstable_elicitation`,
`unstable_end_turn_token_usage`, `unstable_mcp_over_acp`,
`unstable_protocol_v2`, and `unstable_session_fork`. None is enabled or
compiled by orbcode. A future change must make a separate product decision,
add handlers and process tests, and update this table before advertising one.

## Client-to-agent methods

| ACP v1 method | Status | Orbcode behavior |
| --- | --- | --- |
| `initialize` | Implemented | Negotiates v1 and returns the capability truth table below. |
| `session/new` | Implemented | Creates a session with absolute `cwd`, additional directories, and session-scoped MCP overlays. |
| `session/prompt` | Implemented | Validates content, submits one canonical turn, and streams `session/update`. |
| `session/cancel` | Implemented | Cancels the active prompt and resolves its pending server requests. |
| `session/set_mode` | Implemented | Sets a reviewed, session-scoped permission mode. |
| `session/set_config_option` | Implemented | Sets the session model or thought level and returns refreshed options. |
| `session/list` | Implemented | Lists safe sessions scoped to the ACP launch working directory. |
| `session/load` | Implemented | Validates and replays safe transcript history before accepting new prompts. |
| `session/resume` | Implemented | Reattaches without replay and accepts subsequent prompts. |
| `session/delete` | Implemented | Deletes only inactive, ACP-visible sessions in scope. |
| `session/close` | Implemented | Cancels active work and removes pending requests and MCP overlays. EOF performs the same cleanup. |
| `authenticate` | Intentionally unsupported | `authMethods` is empty; provider credentials are not exposed through ACP. |
| `logout` | Intentionally unsupported | The logout capability is omitted. |

`session/fork` is not part of the compiled stable surface. It remains
intentionally unsupported behind the SDK's `unstable_session_fork` feature.

## Agent-to-client methods

| ACP v1 method | Status | Orbcode behavior |
| --- | --- | --- |
| `session/update` | Emitted | Assistant chunks, plan/thought chunks, tool lifecycle, usage, replay, and completion are projected from app-server events. |
| `session/request_permission` | Emitted | Used for protected tool calls, MCP trust, and option-based `AskUserQuestion`. |
| `fs/read_text_file` / `fs/write_text_file` | Not emitted | Orbcode runs its canonical file tools instead of delegating file I/O to the client. |
| `terminal/create`, `terminal/output`, `terminal/release`, `terminal/wait_for_exit`, `terminal/kill` | Not emitted | Orbcode runs its canonical command tool instead of delegating a client terminal. |

## Initialize capability truth table

| Wire field | Value |
| --- | --- |
| `loadSession` | `true` |
| `mcpCapabilities.http` | `true` (Streamable HTTP) |
| `mcpCapabilities.sse` | `false` |
| `mcpCapabilities.acp` | omitted |
| `promptCapabilities.embeddedContext` | `true` |
| `promptCapabilities.image` / `audio` | `false` / `false` |
| `sessionCapabilities.additionalDirectories` | present |
| `sessionCapabilities.list` / `delete` / `resume` / `close` | present |
| `sessionCapabilities.fork` | omitted |
| `elicitation`, `providers`, `nes`, `positionEncoding` | omitted |
| `auth.logout` | omitted |
| `authMethods` | `[]` |

The initialize unit tests and raw-process tests pin both advertised fields and
important omissions.

## Session controls

Mode IDs are `default`, `accept_edits`, `plan`, and `dont_ask`. The adapter
never advertises `bypass_permissions` or `auto`. Plan mode uses the existing
core restrictions and does not expose mutation or network tools to the model.

The `model` and `thought_level` select options come from canonical provider
configuration and model capabilities. Mode, model, and effort are isolated per
session; a change affects the next turn in that session only. A change while a
turn is active is rejected, leaving the old value intact. Managed locks and
unknown sessions/options are returned as typed ACP invalid-params errors.

## Prompt content

- Text blocks remain byte-for-byte ordered, including adjacent text blocks.
- Resource links preserve name, URI, description, and media type as attributed
  context. Orbcode does not fetch the URI.
- Embedded text resources preserve their URI and media type with explicit
  attribution. Their aggregate payload limit is 1 MiB per prompt.
- Blob resources, images, audio, and unknown blocks are rejected with
  `InvalidParams` before a turn is submitted. They are never converted to
  synthetic user prose.

Image or audio support requires a separate attachment design covering durable
transcripts, provider request mapping, per-model capabilities, limits, and
privacy. Until that gate lands, the capability flags must remain false.

## AskUser decision

`AskUserQuestion` with explicit options uses stable
`session/request_permission` and keeps exactly-once request cleanup semantics.
Free-text AskUser remains deliberately disabled because ACP elicitation is an
unstable, uncompiled SDK feature. Orbcode deterministically cancels that tool
request and does not create a fake one-option permission request.

## MCP transports

ACP session setup accepts stdio and Streamable HTTP (`http`/`https`) servers as
in-memory session overlays. New servers start untrusted and use ACP permission
requests for the trust decision. The overlays never persist to the user's MCP
registry and are removed at close or EOF. Legacy SSE and MCP-over-ACP are
rejected and unadvertised.

For setup and real-editor evidence, see [ACP with Zed](acp-zed-smoke.md).
