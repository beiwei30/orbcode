# 无头模式与 stream-JSON

[English](../stream-json.md) · [简体中文](stream-json.md)

Orb Code 同时支持简单的单次输出和双向 NDJSON 控制通道。事件 schema 面向 Claude Code SDK
兼容，支持的 control union 是明确、有限的。

## 输出模式

```bash
orbcode -p "解释这个仓库"                         # text
orbcode -p --output-format json "解释这个仓库"    # 单个 JSON result
orbcode -p --verbose --output-format stream-json "解释"  # NDJSON events
```

- `text` 输出最终 assistant 文本。
- `json` 输出一个包含 session、usage、cost、subtype 和错误信息的 result object。
- `stream-json` 输出初始化、assistant/content/tool/progress、压缩边界和最终 result；必须加
  `--verbose`。

每行一个 JSON object。stdout 应只用于协议，应用日志请写到其他位置。

## 双向输入

同时设置 input/output format，可让进程持续接收 user frames 和 controls：

```bash
printf '%s\n' \
  '{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}' \
  '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}' \
  | orbcode -p --verbose \
      --input-format stream-json --output-format stream-json
```

user content 可以是字符串或兼容 content-block array。对于 malformed line 和 unsupported
request，只要存在 request ID 就返回结构化、关联的错误，不会静默丢弃。

## Control envelopes

Host → CLI：

```json
{
  "type": "control_request",
  "request_id": "model-1",
  "request": { "subtype": "set_model", "model": "sonnet" }
}
```

响应：

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

错误使用 `subtype: "error"` 和 `error` 字符串。response 与 assistant/terminal events 保持
输入顺序关系。

## 支持的 controls

| Subtype | 方向 | 效果 |
| --- | --- | --- |
| `initialize` | Host → CLI | 幂等的 session、model、tools、MCP、capability snapshot。 |
| `interrupt` | Host → CLI | 中断 active turn，idle 时也安全。 |
| `set_permission_mode` | Host → CLI | 修改下一个权限决策 mode。 |
| `get_session_state` | Host → CLI | 权威的类型化 session state。 |
| `get_context_usage` | Host → CLI | 当前 context/token 明细。 |
| `mcp_status` | Host → CLI | 不含 secret 的 MCP status。 |
| `set_model` | Host → CLI | 设置/清除下一次 provider request 的 model。 |
| `set_max_thinking_tokens` | Host → CLI | 设置/清除经过验证的 Anthropic thinking-token override。 |
| `seed_read_state` | Host → CLI | 验证并植入 file identity，供 stale-write protection 使用。 |
| `cancel_async_message` | Host → CLI | signal 当前拥有的一项 prompt job、local agent、workflow 或 shell task。 |
| `can_use_tool` | CLI → Host | 请求 host 解决已有 tool permission request。 |

`rewind_files` 可识别但返回错误，因为 transcript rewind 不是文件恢复。未知未来 subtype 也会
返回关联的 unsupported error。`cancel_async_message` 返回 `signalled`、`already_terminal` 或
`not_found`，不能取消其他 session 拥有的任意工作。

## 工具权限回调

需要批准时，CLI 发送 server-originated `can_use_tool`，包含 tool name、input、tool-use ID 和
边界上下文。host 用关联 response 回复：

- `allow`，可附带 `updatedInput` 与 `toolUseID`；或
- `deny`，带 message 和可选 `interrupt`。

每个请求只解决一次。EOF 会拒绝待处理批准，避免 turn 永久阻塞。独立的 `ask_user` server
request 和能力协商见[交互式提问](interactive-questions.md)。

## 兼容事件

stream 包含关键的 `system` `init` 和最终 `result.subtype`，压缩时发出 `compact_boundary`。
session/tool progress 来自 TUI 和 app-server 共用的类型化 `StreamEvent`，减少 adapter-only 行为。

TypeScript CLI 大多只使用 0/1 进程状态；Orb Code 保留 result 字符串，同时提供
[CLI 参考](cli-reference.md#退出码)中的细化退出码。

若程序只拥有一个 CLI 进程，请使用此通道；需要持久多 session 控制时参见实验性的
[app-server 协议](integrations.md#app-server-协议)。
