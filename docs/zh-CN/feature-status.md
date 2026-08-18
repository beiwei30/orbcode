# 功能状态

[English](../feature-status.md) · [简体中文](feature-status.md)

Orb Code 是 alpha 软件。这里的 “Stable” 只表示当前 alpha 内已实现、有聚焦测试且是推荐路径，
不构成跨版本兼容承诺。

| 级别 | 含义 |
| --- | --- |
| Stable | 当前推荐路径，并有聚焦测试。 |
| Beta | 日常可用，但行为或 UX 仍可能变化。 |
| Experimental | 形态可能变化，不保证协议/版本兼容。 |
| Deferred | 刻意不出现在用户或模型可见 surface。 |

## 接口

| Surface | 状态 | 说明 |
| --- | --- | --- |
| 交互式 TUI | Beta | Chat、tools、permission/model/session pickers、rewind、diff、transcript pager、themes、Vim、动态 slash commands。 |
| 无头 text/JSON | Stable | 通过 `-p` 或 `prompt` 执行一轮。 |
| 双向 stream-JSON | Beta | 类型化 NDJSON controls 和 permission/question callbacks。 |
| 后台 prompt jobs | Stable | 排队、列表、日志/跟随、attach、cancel、恢复 metadata。 |
| 持久目标 | Experimental | transcript-backed、client-supervised 多轮续跑。 |
| 直接 tool 调用 | Beta | 经过权限调试一个 registered tool。 |
| ACP | Experimental | stdio 上 ACP v1，能力受限映射有明确文档。 |
| App-server/remote | Experimental | stdio、Unix socket、WebSocket 上 protocol 1.0。 |

## Providers

| 路径 | 状态 | 说明 |
| --- | --- | --- |
| Anthropic | Stable | Streaming Messages、thinking/interleaved thinking、token counting、API key/bearer/OAuth。 |
| OpenAI-compatible API key | Beta | streaming Chat Completions、effort、endpoint override，无服务端 token count。 |
| ChatGPT/Codex subscription | Experimental | browser/device login、refresh、Responses reasoning/function calls、固定订阅 backend。 |
| Gemini、Grok | 未实现 | 枚举接受，但返回 `unsupported_provider`。 |
| Retry/fallback/rate limit | Stable | 错误规范化、retry-after、retry budget、eligible exhaustion 后 fallback。 |

## 工具与编排

| 组 | 状态 | 说明 |
| --- | --- | --- |
| Read/Edit/Write/Glob/Grep/Bash/NotebookEdit | Stable | 工作区边界、stale-write、结构化 Bash permissions。 |
| WebFetch/WebSearch | Stable | domain/network policy，curl 与 DuckDuckGo HTML 路径。 |
| Plans、todos、tasks | Beta | 持久 plan、task state/log/cancel；verify 只是快照。 |
| Local Agent | Beta | 配置的同步 subagent 和 child-session tracking。 |
| Skills/ToolSearch | Beta | bundled/user/project/plugin 与 trusted MCP prompt discovery。 |
| AskUserQuestion | Experimental | 只对满足 interaction capability 的 client 可见。 |
| LSP | Experimental | 启发式 workspace 查询，不是真正 language-server client。 |
| Workflow | Experimental | 生成的持久 dynamic workflow。 |
| Goal tools | Experimental | 仅 supervised persistent-goal turns。 |

以下工具在闭环实现前被 registry/provider schema 明确排除：PowerShell、Cron variants、Monitor、
Sleep、Browser、RemoteTrigger、Teams、Vault、ReviewArtifact、SyntheticOutput、Marketplace、
PushNotification、ScheduleWakeup、EnterWorktree、ExitWorktree。实时清单以 `orbcode tools` 为准，
不要依赖固定数量。

## 配置与安全

| Surface | 状态 | 说明 |
| --- | --- | --- |
| Settings layering/home compatibility | Stable | User → Project → Local → Managed；可选择独立 home。 |
| Permission rules/Bash parsing | Stable | deny > ask > allow；复合命令结构检查。 |
| Managed policy | Stable | 模型、权限、hooks、MCP、auth、plugins 的锁与限制。 |
| macOS/Linux sandbox | Beta | Seatbelt/Bubblewrap；缺 runner fail closed。 |
| Windows sandbox | Experimental | builder/tests 已有，host validation 仍 opt-in。 |
| Instructions/memory | Stable | 兼容 CLAUDE.md/rules discovery。 |
| Agents、skills、styles、keybindings | Beta | user/project/plugin discovery 与 precedence。 |
| Hooks | Beta | 七个已实现事件，相对 TypeScript CLI 仍不完整。 |
| Plugins | Experimental | installed index 与 commands/agents/skills/hooks/styles/MCP/tools，无 marketplace UI。 |

## MCP 与持久化

| Surface | 状态 | 说明 |
| --- | --- | --- |
| MCP stdio/Streamable HTTP | Stable | 当前 HTTP path 提出 `2024-11-05`，支持 POST JSON/有限 SSE response；standalone GET 与 modern subscription Deferred。 |
| MCP WebSocket | Beta | ws/wss 上 JSON-RPC。 |
| MCP OAuth | Beta | token import/refresh、device/browser PKCE、dynamic registration。 |
| MCP trust + permissions | Stable | 独立 gates，不能互相绕过。 |
| MCP hot reload | Beta | 根据配置变化 add/remove/restart。 |
| JSONL transcripts/session operations | Stable | 兼容 layout、ordered flush、resume/fork/rename。 |
| Context estimation/compaction | Beta | 手动/自动压缩与 boundary events。 |
| Transcript rewind | Beta | 只回退对话，不恢复文件。 |

`orbcode advanced` 是 advanced slices 的运行时权威：当前 background sessions active，
remote-control bridge、voice、computer use deferred。
