# 交互式提问

[English](../interactive-questions.md) · [简体中文](interactive-questions.md)

只有当某一 turn 的 owner client 声明完整 version-1 interaction capability 时，模型才会看到
`AskUserQuestion`。默认关闭，因此未知 client、普通 `--print` text/JSON、background turns 和
部分 capability client 不会意外获得无法闭环的 question schema。ACP 只声明稳定的单问题
option mapping，不会启用 canonical provider schema。

app-server client 在 `initialize` 中 opt in：

```json
{
  "capabilities": {
    "streaming": true,
    "experimental_methods": true,
    "interactive_questions": {
      "single_select": true,
      "multi_select": true,
      "free_text": true,
      "previews": true,
      "annotations": true,
      "special_outcomes": true
    }
  }
}
```

server 随后发送 `ask_user/request`，其中包含 `session_id`、`turn_id`、`tool_use_id`、
`request_id`、可选绝对 `deadline` 和一到四个 canonical `questions`。问题和选项都有稳定 ID。
protocol-1.0 兼容窗口内仍可读取旧的单问题 `question`/`options` 字段。

## 双向 stream-JSON

使用 `--print --input-format stream-json --output-format stream-json --verbose` 启用。stdin
保持打开时会声明完整 capability，并把请求作为有序 `stream_event` 发出；嵌套 event 的
`type` 为 `server_request`、method 为 `ask_user/request`，包含关联 `request_id` 与 canonical
`params`。在 stdin 用相同 ID 和类型化 outcome 回复：

```json
{"type":"server_response","request_id":"ask-1","response":{"outcome":"answered","answers":{"database":{"kind":"selected","option_id":"postgres"}},"annotations":{}}}
```

其他 outcome：`rejected`、`clarify`、`finish_plan_interview`、`cancelled`（带 reason）。无效 answer
返回关联错误并保持 request pending，host 可重试。unknown/stale/duplicate ID 被确定性拒绝。
关闭 stdin 会以 disconnect 取消所有 pending questions；permission auto-approval 不会回答问题。

权威 wire 参考是 `app-server-protocol/tests/generated/` 下 checked-in JSON Schema 与生成的
TypeScript declarations。

## ACP 子集

ACP 把可表示的单选、纯 option 请求映射到 `session/request_permission`。free-text、多问题、
annotation、preview、special-outcome 请求会取消，而不是被错误表达。因为这不是完整 v1
capability，ACP turns 不会在 provider tool definitions 中启用 `AskUserQuestion`；但兼容 legacy
或强制的 option-only request 仍可回答。
