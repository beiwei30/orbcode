# 类型化设置所有权与原始 JSON 边界

[English](../settings-architecture.md) · [简体中文](settings-architecture.md)

本文记录边界重构后的 settings 架构：稳定 client settings 使用类型；只有兼容性或 extension
owner 需要开放 schema 时才保留 raw JSON。

## 所有权与数据流

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

- `config` 负责持久化解析、`defaults → user → project → local → managed → CLI → environment →
  session` 来源顺序、默认值、边界、managed locks、无损 read-modify-write。
- `RuntimeSessionState` 与 core 负责 runtime choices。模型状态区分 `Inherit`、`Default`、
  `Model(id)`；`Inherit` 恢复分层 env/persisted 选择，`Default` 刻意绕过它。
- theme/editor mode 当前是 process-wide runtime preferences，由共享 `RuntimeSessionState` 拥有，
  修改会影响进程内所有 clients/sessions，并非 session controls。
- app-server 只把 config/core 类型映射一次到 protocol DTO。AppClient/edge adapters 不重读
  settings file，也不从 JSON 推测字段含义。
- statusline 执行属于 TUI；config 验证 command/interval，app-server 只传输。
- permission rule 字符串由 config 的 structured parser 解释。typed projection 保留 source groups
  和 deny/ask/allow precedence；MCP trust 仍是独立必需 gate。

## 类型化 setting families

| Family | Persisted owner | Runtime/effective owner | Client projection |
| --- | --- | --- | --- |
| Statusline | `ClaudeSettings::statusline_config` | TUI execution state | `BootstrapState::statusline` |
| Model | `PersistedModelSetting` | `RuntimeModelOverride`、`EffectiveModelSelection` | `SessionControlState::model_selection` |
| Permission rules | `SettingsLayers::permission_rules` | core permission runtime/policy | `PermissionOverview::effective_rules` |
| Theme/editor | typed `ClaudeSettings` | process-wide `ClientPreferences` | bootstrap + typed settings methods |

`PermissionOverview` 上残留 compatibility fields 仅用于 protocol 1.0 read compatibility；新 consumer
使用保留 source 的 projection。旧 `settings/set_model` 仍服务既有 client；新 session controls 用
`session/set_model`。其 `None` 表示 provider default，而不是清到 `Inherit`。

## 兼容行为矩阵

| Input/state | Statusline | Model | Permission/theme/editor mutation |
| --- | --- | --- | --- |
| Missing | 无 command，30 秒 interval | provider/env fallback | typed default/current |
| 显式 `null` | 同 resolved default | 无 persisted model | parser-specific existing behavior |
| 错 JSON type | typed field load error | load error | mutation 前拒绝 |
| 高层部分值 | 低层 nested sibling 保留 | 最高来源 model 胜出 | source groups 保持顺序 |
| Managed lock | 可读 | 报告 source/lock，拒绝写 | disk/runtime 不变 |
| Unknown keys | 保留 | 保留 | read-modify-write 保留 |

statusline interval 只接受 1–3600 秒；missing、null、0、越界值解析为 30 秒兼容默认。

## Raw JSON 边界清单

下表对 scoped crates 的 production `Value`/`json!` sites 分类。tests/fixtures 中的 JSON 是断言，
不属于 production boundary。

| Files | 分类 | Owner 与处理 |
| --- | --- | --- |
| `config/src/layers.rs`、`policy.rs`、`settings_resolution.rs` | raw persistence | config 负责 source attribution、policy diagnostics、aliases、unknown keys |
| `config/src/claude_home.rs` | raw persistence/mutation | config 负责兼容解析与 atomic read-modify-write；进入前验证 typed intent |
| `config/src/plugins.rs` | plugin dynamic schema | plugin manifests 拥有 MCP blocks 与 tool input schemas |
| `config/src/hooks.rs` | dynamic hook payload | hook protocol 拥有 event input/output JSON |
| `config/src/keybindings.rs`、`output_styles.rs` | dedicated loader | typed loader 负责 precedence，按需保留 source document |
| `config/src/auth.rs`、`openai_oauth.rs` | external auth envelope | provider OAuth/token response 是外部 schema |
| `config/src/config.rs` | provider pass-through | provider 拥有 `extra_body` 与 analytics metadata |
| `app-server/src/protocol_handler.rs`、`message_processor.rs`、`protocol_handler/*.rs` | protocol envelope | dispatcher 立即把 raw request 反序列化成 named DTO |
| `app-server/src/mcp_api.rs`、`tools_api.rs`、`workflow_api.rs` | dynamic schema | MCP/tool/workflow owner 定义 arguments/results/schemas |
| `app-server/src/background_api.rs`、`background/events.rs`、`sessions.rs` | event/transcript envelope | background/transcript 兼容格式为开放 event payload |
| `app-server/src/bootstrap.rs`、`doctor/environment.rs` | external diagnostic input | environment/plugin diagnostics 开放，不是 stable settings projection |
| `app-server-client/src/transport.rs`、`in_process.rs`、`ndjson_transport.rs`、`websocket_transport.rs` | protocol envelope | transport 携带 raw envelope；AppClient 反序列化 named result |
| `app-server-client/src/lib.rs` | protocol/dynamic input | generic transport + MCP/workflow args；stable public results typed |
| `core/src/agent_tool.rs`、`tool_flow.rs`、`tool_progress.rs`、`permission_state*.rs` | tool payload | tool implementation/structured permission parser 拥有 open input |
| `core/src/hooks.rs`、`hook_runner/*.rs` | hook payload | hook contract 拥有 JSON events |
| `core/src/compaction.rs`、`context_estimation.rs`、`config_provider.rs`、`system_prompt.rs` | transcript/provider payload | transcript/provider compatibility 拥有 shape |
| `core/src/session_manager/*.rs` | transcript/tool/workflow envelope | orchestration 携带 open tool blocks/extension events，但不视为 settings |
| `tui/src/state.rs`、`chat/stream_events.rs`、`embedded_progress.rs`、`history_cell/local_note.rs` | render event payload | TUI 保存 typed events + opaque hook/tool metadata |
| `tui/src/overlays/*.rs`、`render/*.rs`、`tool_cell/*.rs` | tool/hook rendering | UI 检查 tool input/extension metadata，不检查 settings projection |
| `tui/src/custom_terminal.rs`、`terminal_trace.rs`、`render_metrics.rs`、`tui_runtime/terminal_session.rs` | diagnostics | terminal trace/render metrics 刻意输出开放 JSON |
| `tui/src/commands/dispatch.rs` | command envelope | slash output 携带 extension-defined data |
| `cli/src/control.rs`、`stream_json.rs`、`headless.rs` | wire envelope | stream-json/control 需要准确开放 records；settings 来自 typed AppClient DTO |
| `cli/src/acp_sdk/*.rs` | external protocol envelope | ACP schema 拥有 content blocks/annotations |
| `cli/src/args.rs`、`commands/mod.rs`、`main.rs` | CLI/external envelope | CLI settings JSON input 与 connection output，不是 settings read |

任何新 raw site 必须属于上述 owner；原因不明显时写本地 ownership comment。新增返回 `Value`
的 public AppClient method、client-side settings file read、stable settings string parsing 会被
`cli/tests/app_client_boundary_audit.rs` 拒绝。
