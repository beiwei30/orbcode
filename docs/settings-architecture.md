# Typed Settings Ownership and Raw JSON Boundaries

[English](settings-architecture.md) · [简体中文](zh-CN/settings-architecture.md)

This document records the post-boundary settings architecture. Stable client
settings are typed; raw JSON remains only where compatibility or an extension
owner requires an open schema.

## Ownership and data flow

```text
settings files ──> config parsing/layers ──> persisted + source/lock metadata
                                              │
runtime controls ──> RuntimeSessionState/core ├─> effective state
                                              │
provider catalog/resolver ────────────────────┘
                                              │
                                              v
                                  AppServer protocol mapping
                                              │
                                              v
                                  typed AppClient projections
                                              │
                              TUI / ACP / headless / SDK clients
```

- `config` owns persisted parsing, the `defaults → user → project → local →
  managed → CLI → environment → session` attribution order, defaults, bounds,
  managed locks, and lossless read-modify-write mutation.
- `RuntimeSessionState` and core own runtime choices. Model state uses distinct
  `Inherit`, `Default`, and `Model(id)` variants. `Inherit` resumes layered
  env/persisted selection; `Default` deliberately bypasses it.
- Theme and editor mode are currently process-wide runtime preferences. The
  shared `RuntimeSessionState` owns their overrides, so changing either affects
  all clients/sessions in that process. They are not session controls.
- App-server maps config/core types into protocol DTOs once. AppClient and edge
  adapters do not reopen settings files or infer field meaning from JSON.
- Statusline execution is TUI-owned. Config validates the command/interval and
  app-server only transports them.
- Permission rule strings remain interpreted by config's structured parser.
  The typed projection preserves source groups and deny/ask/allow precedence;
  MCP trust remains a separate required gate.

## Typed setting families

| Family | Persisted owner | Runtime/effective owner | Client projection |
| --- | --- | --- | --- |
| Statusline | `ClaudeSettings::statusline_config` | TUI execution state | `BootstrapState::statusline` |
| Model | `PersistedModelSetting` | `RuntimeModelOverride`, `EffectiveModelSelection` | `SessionControlState::model_selection` |
| Permission rules | `SettingsLayers::permission_rules` | core permission runtime and policy | `PermissionOverview::effective_rules` |
| Theme/editor | typed `ClaudeSettings` values | process-wide `ClientPreferences` | bootstrap plus typed settings methods |

The compatibility fields still present on `PermissionOverview` are protocol
1.0 read compatibility only. New consumers use the source-preserving
projection. The legacy `settings/set_model` route remains for existing clients;
new session controls use `session/set_model`, while its `None` value means the
provider default rather than clearing to `Inherit`.

## Compatibility behavior matrix

| Input/state | Statusline | Model | Permission/theme/editor mutation |
| --- | --- | --- | --- |
| Missing | no command, 30-second interval | provider/env fallback | typed default/current value |
| Explicit `null` | same resolved statusline default | no persisted model | parser-specific existing behavior |
| Wrong JSON type | settings load error for typed fields | settings load error | rejected before mutation |
| Partial higher layer | nested sibling from lower layer survives | highest attributed model wins | source groups stay ordered |
| Managed lock | read remains available | source/lock reported; write rejected | disk and runtime remain unchanged |
| Unknown keys | preserved | preserved | read-modify-write preserves them |

Statusline intervals are accepted only from 1 through 3600 seconds; missing,
null, zero, or out-of-range values resolve to the 30-second compatibility
default.

## Raw JSON boundary inventory

The entries below classify all production `Value`/`json!` sites in the scoped
crates. Test modules and fixtures use JSON as assertions and are classified as
test fixtures rather than production boundaries.

| Files | Classification | Owner and disposition |
| --- | --- | --- |
| `config/src/layers.rs`, `policy.rs`, `settings_resolution.rs` | raw persistence | config owns source attribution, policy diagnostics, aliases, and unknown keys |
| `config/src/claude_home.rs` | raw persistence/mutation | config owns compatibility parsing and atomic read-modify-write; typed intent is validated before entry |
| `config/src/plugins.rs` | plugin-defined data/dynamic schema | plugin manifests own MCP blocks and tool input schemas |
| `config/src/hooks.rs` | dynamic hook payload | hook protocol owns event input/output JSON |
| `config/src/keybindings.rs`, `output_styles.rs` | dedicated loader | their typed loaders own precedence while retaining source documents where needed |
| `config/src/auth.rs`, `openai_oauth.rs` | external auth envelope | provider OAuth/token response formats are external schemas |
| `config/src/config.rs` | provider pass-through | `extra_body` and analytics metadata are provider-owned opaque JSON |
| `app-server/src/protocol_handler.rs`, `message_processor.rs`, `protocol_handler/*.rs` | protocol envelope | the dispatcher deserializes raw request envelopes immediately into named DTOs |
| `app-server/src/mcp_api.rs`, `tools_api.rs`, `workflow_api.rs` | dynamic schema | MCP/tool/workflow extension owners define arguments, results, and schemas |
| `app-server/src/background_api.rs`, `background/events.rs`, `sessions.rs` | event/transcript envelope | background and transcript compatibility formats are open event payloads |
| `app-server/src/bootstrap.rs`, `doctor/environment.rs` | external diagnostic input | environment/plugin diagnostic values are open, not stable settings projections |
| `app-server-client/src/transport.rs`, `in_process.rs`, `ndjson_transport.rs`, `websocket_transport.rs` | protocol envelope | transports carry raw envelopes; AppClient methods deserialize named results |
| `app-server-client/src/lib.rs` | protocol envelope/dynamic input | generic transport plumbing plus MCP/workflow arguments; stable public results remain typed |
| `core/src/agent_tool.rs`, `tool_flow.rs`, `tool_progress.rs`, `permission_state*.rs` | tool payload | tool implementations and structured permission parsing own open tool input |
| `core/src/hooks.rs`, `hook_runner/*.rs` | hook payload | hook contracts own JSON event inputs/outputs |
| `core/src/compaction.rs`, `context_estimation.rs`, `config_provider.rs`, `system_prompt.rs` | transcript/provider payload | transcript and provider request compatibility owns these shapes |
| `core/src/session_manager/*.rs` | transcript/tool/workflow envelope | session orchestration carries open tool blocks and extension events without treating them as settings |
| `tui/src/state.rs`, `chat/stream_events.rs`, `embedded_progress.rs`, `history_cell/local_note.rs` | render-only event payload | TUI stores typed events with opaque hook/tool progress metadata |
| `tui/src/overlays/*.rs`, `render/*.rs`, `tool_cell/*.rs` | tool/hook rendering | UI inspects tool input and extension metadata, not settings projections |
| `tui/src/custom_terminal.rs`, `terminal_trace.rs`, `render_metrics.rs`, `tui_runtime/terminal_session.rs` | diagnostics | terminal traces and render metrics intentionally emit open diagnostic JSON |
| `tui/src/commands/dispatch.rs` | command envelope | slash command output carries extension-defined data |
| `cli/src/control.rs`, `stream_json.rs`, `headless.rs` | wire envelope | stream-json/control compatibility requires exact open JSON records; settings state comes from typed AppClient DTOs |
| `cli/src/acp_sdk/*.rs` | external protocol envelope | ACP content blocks and annotations are owned by the ACP schema |
| `cli/src/args.rs`, `commands/mod.rs`, `main.rs` | CLI/external envelope | CLI settings JSON input and machine-readable connection output are protocol input/output, not settings reads |

Any new raw site must fit one of these named owners and carry a local ownership
comment when the reason is not obvious. A new public AppClient method returning
`Value`, a client-side settings-file read, or stable settings string parsing is
rejected by `cli/tests/app_client_boundary_audit.rs`.
