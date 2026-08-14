# 实验性持久目标

[English](../persistent-goals.md) · [简体中文](persistent-goals.md)

持久目标是 session-scoped、transcript-backed capability。每个 session 最多一个当前 goal，旧
snapshots/checkpoints 留在 JSONL transcript。该能力是实验性的：connection 必须在
`initialize` 同时设置 `capabilities.experimental_methods` 与 `capabilities.persistent_goals`，
server 才会公布/接受方法族。

## App-server 方法

四个方法都是实验性的类型化 contracts：

- `session/goal/get`：`SessionIdParams -> SessionGoalGetResult`。
- `session/goal/set`：`SessionGoalSetParams -> SessionGoalSetResult`。修改已有 goal 需要
  `expected_revision`；替换 terminal goal 还需显式 user-facing `replace`。缺少 `token_budget`
  表示不改，JSON `null` 清除，正整数设置。
- `session/goal/clear`：`SessionIdParams -> SessionGoalClearResult`。
- `session/goal/continue`：`SessionGoalContinueParams -> SessionGoalContinueResult`，同时指定
  `goal_id` 与 `expected_revision`。

continuation result 由 `outcome` 标记。`started` 含新的普通 `subscription_id`、`turn_id` 与刷新
goal；`not_started` 含刷新 goal（或 null）及原因：`missing`、`stale_revision`、`inactive`、
`usage_limited`、`budget_limited`、`pending_user_input`、`active_turn`、`client_not_capable`。
每个 subscription 仍以普通 terminal event 结束，goal 不改变 stream terminal 语义。

## Canonical state 与权限

`SessionGoal` 包含 `goal_id`、`revision`、`session_id`、`objective`、`status`、可选
`token_budget`、累计 `tokens_used`/`elapsed_seconds`、timestamps、可选 `stop_reason` 和
`last_goal_turn_id`。

| 起点 | 允许目标与权限 |
| --- | --- |
| none | user set 或 model create 到 `active` |
| `active` | user/system 可 `paused`；model/user 可 `blocked`/`complete`；system 可限流 |
| `paused` | user 可 `active`、`complete` 或 clear |
| `blocked` | user 可 `active`、`complete` 或 clear |
| `usage_limited` | 条件改变后 user 可 `active` 或 clear |
| `budget_limited` | user 明确提高 budget 后才可 `active`，或 clear |
| `complete` | user 显式 create/replace 新 goal，或 clear |

模型工具只有 `get_goal`、`create_goal`、`update_goal`，其中 `update_goal` 只能选择 `complete`
或 `blocked`。pause/resume、budget 改变、usage-limit 恢复、clear、replace 属于 user/system。
重复 blocker 规则是模型指引加可审计 turn history；当前不会声称能语义判断 blocker 文本等价。

显式 token budget 的 goal 成功 `complete` 时，`update_goal` 返回 `final_usage`，含累计
`tokens_used`、`token_budget`、`elapsed_seconds`，并在 tool success 前持久化。

## TUI 命令与调度

- `/goal` / `/goal show`：显示 objective、status、token usage/budget、elapsed、revision、reason。
- `/goal create [--budget N] <objective>`：创建并启动；`/goal [--budget N] <objective>` 等价。
- `/goal edit [--budget N|--no-budget] <objective>`：编辑当前目标。
- `/goal pause`、`/goal resume`、`/goal clear`：控制生命周期。
- `/goal budget N|none`：修改/移除 budget；提高或移除已耗尽 budget 会重新激活。

每个 terminal goal turn 后，TUI 先提交 queued user follow-up；只有没有 follow-up、active turn、
pending server request 时才请求下一 continuation。Ctrl-C/terminal shutdown 中断当前 turn，并把
active goal 持久化为 paused。

## Client 支持

| Client surface | Goal methods/tools | 自动续跑 |
| --- | --- | --- |
| Local TUI | 是 | 是，每次一个普通 subscription |
| socket/WebSocket Remote TUI | 是 | 是，同 local policy |
| ACP adapter | 是，通过类型化 `AppClient` | 是，在一个 ACP prompt lifecycle 内 |
| 显式 goal-capable `AppClient` | 是 | caller 通过 `continue_goal` 拥有 |
| 默认 `AppClient`、`-p`、`prompt`、background jobs | 否 | 否，一个 prompt 仍一个 terminal result |
| Raw app-server connection | 仅启用两项 experimental bits 后 | caller-owned |

interactive-question 与 persistent-goal supervision 是独立 capabilities；启用 duplex stream-json
questions 不会把 headless prompt 静默变成多轮 goal runner。

## Transcript contract

goal metadata 使用以下有序 JSONL discriminants：

- `goal`：完整 snapshot，使用 camel-case transcript keys：`goalId`、`tokenBudget`、
  `tokensUsed`、`elapsedSeconds`、`createdAt`、`updatedAt`、`stopReason`、`lastGoalTurnId`。
- `goal-cleared`：显式 tombstone，晚于所有旧 snapshot。
- `goal-turn-start`：含 `goalId`、`goalRevision`、`turnId`、`timestamp` 的开始 checkpoint。
- `goal-turn-terminal`：同一 identity、`terminalKind`、canonical `usage`、elapsed delta、timestamp。

最后有效 snapshot 或更晚 tombstone 决定当前状态。四类 record 的 unknown fields 都是 forward-
compatible data，完整 rewrite 时必须保留。malformed goal record 作为 inert metadata 保留，不能
覆盖有效 snapshot 或产生 tombstone/boundary。record 与 message boundary 顺序保留，因此 fork/
rewind 得到选定点可见状态。没有 terminal 匹配的 start checkpoint 在加载时恢复为 paused/
interrupted，绝不自动重启。旧 transcript 无这些记录时解析为 no goal。

`session/clear` 新建无 goal session，旧 transcript 仍可 resume；`session/delete` 连 transcript
一起删除 goal。compaction、fork、rewind 保留所选 transcript boundary 的 goal state。

## 重启与失败

每次 mutation/start/terminal accounting 都在返回新状态或转发 terminal event 前 append。append
失败时，旧持久状态仍权威，并释放 reserved turn gate。

| 事件 | 持久结果 |
| --- | --- |
| 正常结束 | 除非 model complete/blocked 或 budget 达到，否则保持 active |
| goal budget 达到 | `budget_limited`；需提高/移除 budget 才恢复 |
| provider rate/account limit | `usage_limited` |
| provider error 或未分类中断 | `paused`，带 stop reason |
| cancel/Ctrl-C/disconnect/close/EOF | `paused`，不留 detached turn |
| start 后、terminal checkpoint 前进程丢失 | load 时恢复为 `paused`/interrupted |
| 后续 malformed goal record | 保留但忽略，最后有效状态不变 |

自动续跑始终由 client 监督。disconnect、EOF、close、cancel 或进程中断会取消 owned turn 并
pause active goal；第一版没有 daemon-owned detached runner。
