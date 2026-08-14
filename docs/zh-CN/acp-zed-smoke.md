# 在 Zed 中使用 ACP

[English](../acp-zed-smoke.md) · [简体中文](acp-zed-smoke.md)

最后验证：2026-08-05

本指南配置 Zed 以 custom external agent 启动 `orbcode acp`，并记录真实编辑器验收。准确协议
surface 见 [ACP 支持矩阵](acp-support.md)。

## 已验证基线

| 组件 | 版本 |
| --- | --- |
| orbcode | 0.0.1，基于 commit `2a7d48b` 加 ACP productization changes |
| ACP Rust SDK / schema | 0.14.0 / 0.13.6 |
| Zed | 1.13.2，build 20260802.163511 |
| Platform | macOS 26.6，build 25G72，Apple silicon |

后续 Zed 保留 ACP v1 兼容时应可工作，但修改 README 的 verified version 前必须重跑清单。

## 前置条件

1. 安装 Zed，并在仓库根目录构建：`cargo build -p orbcode`。
2. 配置受支持 provider，不要把 token 放在仓库、Zed settings、截图或日志中；推荐由安全 launcher
   注入环境变量。
3. 创建一次性 home 并记录准确路径：

   ```bash
   ORBCODE_ZED_SMOKE_HOME="$(mktemp -d)"
   printf '%s\n' "$ORBCODE_ZED_SMOKE_HOME"
   ```

   把输出路径用于下方 `ORBCODE_HOME`。测试后只删除这个准确目录，不要指向日常 Claude/Orb Code
   home。

仓库内 deterministic tests 使用 `mock-provider` 开发 feature 和 `mock://` URL。它只用于测试，
不能用于生产 build 或 Zed 配置。

## Zed 配置

在 Zed `settings.json` 的 `agent_servers` 添加：

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

在 Zed 打开仓库、Agent Panel，选择 `orbcode` external agent 并新建 thread。Zed 把项目 cwd
传为 ACP session `cwd`，Orb Code 要求绝对路径。list/load/resume/delete 只暴露启动目录内 sessions。

Zed 的 [External Agents guide](https://zed.dev/docs/ai/external-agents) 说明 custom ACP agents/logs；
[MCP guide](https://zed.dev/docs/ai/mcp) 说明如何向 external agents 转发 context servers。

## 可重复清单

使用干净临时 home，并记录 Zed、Orb Code、OS 版本。

### Lifecycle 与 streaming

- 新建 thread，发送 `Reply exactly ZED_ACP_SMOKE_OK.`，确认增量文本和正常结束。
- 发起长请求，从 Zed cancel，再在同 thread 发 prompt；确认第一轮停止、第二轮运行。
- active turn 中关闭 thread，确认没有残留 `orbcode acp` child；退出 Zed 也覆盖 EOF cleanup。
- 重启 Zed 并 reopen/import，确认历史按序 replay，且新 prompt 可用。
- 在 history 中验证当前项目 session 的 list/load/resume/delete，其他项目不可见。

### Modes 与配置

- mode selector 只能有 Default、Plan，不得出现 bypass/auto。
- Plan 中请求文件/命令修改，模型本轮不应收到 mutation/network tools。
- idle 时改 Model/Thought level，确认只属于当前 thread；第二 thread 保持独立值。
- active turn 中修改 control 应显示 typed error，cancel/complete 后再试。

### Permissions 与 AskUser

- 触发受保护 command，分别验证 Allow once 和 Reject once；拒绝时 tool 不执行。
- 触发有显式 choices 的 `AskUserQuestion`，选择后确保只传递一次 answer。
- 无 options 的 AskUser 应取消而不显示 elicitation form，这是禁用 `unstable_elicitation` 的预期行为。

### MCP

- 在 Zed 配置一次性 stdio MCP，新 thread 首次调用前应出现 trust request。
- 对一次性 Streamable HTTP 重复，验证 trust allow/deny；close 后不能写入日常 registry。
- SSE 与 MCP-over-ACP 应以 typed error 失败。

### Prompt content

- 发送 text、embedded text resource、resource link，确认顺序与来源 attribution；link 不得自动 fetch。
- image、audio、blob、unknown content 应在 model turn 开始前失败。

## 已记录验收

2026-08-05 使用干净临时 `ORBCODE_HOME` 与无真实 secret 的 deterministic test provider。测试后
移除临时 agent/keybindings/home，退出 Zed 后无 `orbcode acp` child。

| 检查 | 证据 |
| --- | --- |
| External-agent startup、ACP initialize/new | Zed 中 PASS |
| Prompt、stream response、completion | Zed 中 PASS |
| Restart 与 history restoration | Zed 中 PASS |
| Model selector、thought level | Zed 中 PASS；重启后显示 Sonnet/Low |
| Protected command Reject once | Zed 中 PASS；command card 未执行即完成 |
| Tool title | Zed 中 PASS；显示 `Run Command` 与 `bash(echo zed-acp-tool)` |
| Close/EOF child cleanup | Zed + process inspection PASS |
| Allow once、cancel、mode、AskUser、stdio/HTTP MCP、list/resume/delete | raw-process 与 official SDK-client E2E 覆盖，保留在 RC 手动清单 |
| Unsupported content、zero turn submission | raw-process E2E 覆盖 |

扩展行不宣称本次由 UI 观察到：Zed Agent Panel canvas 未向自动 desktop harness 暴露 composer
accessibility element；其协议行为由 readiness plan 中 focused matrix 固定。

## 故障排查

- 在 Zed 运行 `dev: open acp logs`，只检查脱敏输出。ACP stdout 用于 JSON-RPC，诊断必须在 stderr。
- agent 不出现时检查 binary path 为绝对路径，并在同环境运行 `orbcode doctor`。
- session setup 失败时检查 `cwd` 和 additional directories 都是可访问绝对路径。
- 缺少 model/thought controls 时，当前 provider/model 可能未声明对应 canonical options。
- active turn 中配置失败时先 cancel 或等待完成；这是刻意拒绝的 mutation。
- MCP 不 ready 时检查 transport、command/URL、trust；只支持 stdio 与 Streamable HTTP。
- 分享日志前删除 tokens、HTTP headers、本地路径、prompt、transcripts、MCP payloads。
