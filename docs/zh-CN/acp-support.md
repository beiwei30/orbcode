# ACP 支持矩阵

[English](../acp-support.md) · [简体中文](acp-support.md)

最后验证：2026-08-06

`orbcode acp` 是 stdio 上实验性的 Agent Client Protocol（ACP）v1 adapter。canonical runtime
API 仍是 `AppClient` 和 `app-server-protocol`；ACP schema types/translations 位于
`cli/src/acp_sdk/`。

## 锁定的协议依赖

| Dependency | Version | Feature policy |
| --- | --- | --- |
| `agent-client-protocol` | 0.14.0 | 只使用 default features；其默认集合为空。 |
| `agent-client-protocol-schema` | 0.13.6 | 由 SDK 间接解析。 |

SDK 可选 features：`unstable_auth_methods`、`unstable_boolean_config`、
`unstable_elicitation`、`unstable_end_turn_token_usage`、`unstable_mcp_over_acp`、
`unstable_protocol_v2`、`unstable_session_fork`。Orb Code 没有启用或编译它们。未来变更必须
单独做产品决策、增加 handlers/process tests，并先更新本表再宣称支持。

## Client → agent 方法

| ACP v1 方法 | 状态 | Orb Code 行为 |
| --- | --- | --- |
| `initialize` | 已实现 | 协商 v1 并返回下方 capability truth table。 |
| `session/new` | 已实现 | 用绝对 `cwd`、额外目录、session-scoped MCP overlays 创建。 |
| `session/prompt` | 已实现 | 验证 content、提交 turn、stream `session/update`；有 active goal 时同一 ACP lifecycle 观察连续普通 goal-turn subscriptions。 |
| `session/cancel` | 已实现 | 取消 active prompt 并解决 pending server requests。 |
| `session/set_mode` | 已实现 | 设置 reviewed、session-scoped permission mode。 |
| `session/set_config_option` | 已实现 | 设置 session model/thought level 并返回刷新 options。 |
| `session/list` | 已实现 | 只列 ACP 启动 cwd scope 内安全 sessions。 |
| `session/load` | 已实现 | 验证并 replay 安全 transcript history 后接受新 prompt。 |
| `session/resume` | 已实现 | 不 replay，直接 reattach 并接受后续 prompt。 |
| `session/delete` | 已实现 | 只删除 scope 内 inactive、ACP-visible session。 |
| `session/close` | 已实现 | 取消 active work，移除 pending requests/MCP overlays；EOF 同样清理。 |
| `authenticate` | 刻意不支持 | `authMethods` 为空，不通过 ACP 暴露 provider credentials。 |
| `logout` | 刻意不支持 | 不声明 logout capability。 |

`session/fork` 不在已编译 stable surface，仍位于未启用的 `unstable_session_fork` 后。

## Agent → client 方法

| ACP v1 方法 | 状态 | Orb Code 行为 |
| --- | --- | --- |
| `session/update` | 发出 | 把 app-server events 投射为 assistant、plan/thought、tool lifecycle、usage、replay、completion。 |
| `session/request_permission` | 发出 | 用于受保护 tool call、MCP trust、option-based `AskUserQuestion`。 |
| `fs/read_text_file` / `fs/write_text_file` | 不发出 | 使用 canonical file tools，不委托 client I/O。 |
| `terminal/create`、`output`、`release`、`wait_for_exit`、`kill` | 不发出 | 使用 canonical command tool，不委托 client terminal。 |

## Initialize capability truth table

| Wire field | Value |
| --- | --- |
| `loadSession` | `true` |
| `mcpCapabilities.http` | `true`（Streamable HTTP） |
| `mcpCapabilities.sse` | `false` |
| `mcpCapabilities.acp` | omitted |
| `promptCapabilities.embeddedContext` | `true` |
| `promptCapabilities.image` / `audio` | `false` / `false` |
| `sessionCapabilities.additionalDirectories` | present |
| `sessionCapabilities.list` / `delete` / `resume` / `close` | present |
| `sessionCapabilities.fork` | omitted |
| `elicitation`、`providers`、`nes`、`positionEncoding` | omitted |
| `auth.logout` | omitted |
| `authMethods` | `[]` |

unit tests 和 raw-process tests 同时固定声明字段与关键 omission。

persistent goals 不增加 ACP wire capability。adapter 显式让内部 `AppClient` opt in 实验
app-server capability，并只使用 typed `get_goal`/`continue_goal`。load/resume 因此看到与 TUI
相同的 transcript-backed goal。cancel/close/stdin EOF/disconnect 取消当前普通 turn；active goal
checkpoint 为 paused，绝不会脱离 ACP client 继续运行。

## Session controls

mode IDs 只有 `default` 和 `plan`，不声明 `bypass_permissions` 或 `auto`。Plan 使用现有 core
限制，不向模型暴露 mutation/network tools。

`model` 与 `thought_level` options 来自 canonical provider config 与 model capabilities。
mode/model/effort 按 session 隔离，只影响该 session 下一 turn。active turn 中的修改被拒绝，
旧值不变。managed lock、unknown session/option 返回类型化 ACP invalid-params error。

## Prompt content

- Text blocks 保持字节顺序，包括相邻 blocks。
- Resource link 保留 name、URI、description、media type 作为 attributed context，不 fetch URI。
- Embedded text resource 保留 URI/media type 和 attribution，每 prompt 总 payload 上限 1 MiB。
- Blob、image、audio、unknown blocks 在 turn 提交前以 `InvalidParams` 拒绝，不转成合成 user prose。

image/audio 支持需要单独 attachment 设计，覆盖 durable transcript、provider mapping、per-model
capability、limits、privacy；在此之前 flags 必须为 false。

## AskUser 决策

带显式 options 的 `AskUserQuestion` 使用 stable `session/request_permission`，保持 exactly-once
cleanup。free-text 刻意禁用，因为 ACP elicitation 是未编译 unstable feature；Orb Code 会确定性
取消，而不是伪造单 option permission request。

## MCP transports

ACP session setup 接受 stdio 与 Streamable HTTP（`http`/`https`）作为内存 session overlays。
新 server 初始 untrusted，通过 ACP permission request 决定 trust。overlay 不写用户 MCP registry，
close/EOF 时移除。legacy SSE 和 MCP-over-ACP 被拒绝且不声明。

真实编辑器配置与证据见 [ACP with Zed](acp-zed-smoke.md)。
