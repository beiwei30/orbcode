# 模型上下文协议（MCP）

[English](../mcp.md) · [简体中文](mcp.md)

Orb Code 可从 Claude Code 兼容文件、enabled plugins 和单次 overlay 加载 MCP servers，支持
tools、resources、prompts。调用同时受权限和 server trust 保护。

## 配置 server

项目 `.mcp.json` 使用标准 `mcpServers`：

```json
{
  "mcpServers": {
    "local-files": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
      "env": { "LOG_LEVEL": "warn" }
    },
    "issues": {
      "type": "streamable_http",
      "url": "https://mcp.example.com/mcp",
      "headers": { "X-Tenant": "${TENANT_ID}" }
    },
    "events": {
      "type": "websocket",
      "url": "wss://mcp.example.com/events"
    }
  }
}
```

字符串支持 `${VAR}` 和 `${VAR:-default}`。缺少变量且无默认值会成为加载诊断，不会静默替换
为空串。发现源依次包括 user `settings.json`、祖先 `.mcp.json`、project/local settings、重复
`--mcp-config` 和 enabled plugin MCP definitions。settings 可禁用 servers；managed policy 可
allow、deny 或要求 managed-only servers。

CLI 添加持久 registry entry：

```bash
orbcode mcp add issues \
  --transport streamable-http \
  --endpoint https://mcp.example.com/mcp \
  --auth bearer-env:MCP_TOKEN \
  --summary "Issue tracker" --enabled

orbcode mcp remove issues
```

`mcp add` 的 auth spec：`none`、`bearer-env:VARIABLE`、`header:Name=Value`。优先用环境变量
bearer 或 OAuth，不要把 literal secret 留在 shell history/settings。

## Transports

| Transport | 状态 | 说明 |
| --- | --- | --- |
| `stdio` | Stable | 启动本地 command，可带 args、env、cwd。 |
| `streamable_http` | Stable | 标准远程 transport，支持 JSON/SSE response 和 MCP session。 |
| `http`、`https`、旧 `sse` | 兼容 alias | 作为远程 HTTP family 加载。 |
| `websocket` | Beta | 在 `ws://` / `wss://` 上运行真实 JSON-RPC。 |

`orbcode mcp capabilities` 输出实时 transport inventory。

## 检查与使用

```bash
orbcode mcp servers
orbcode mcp diagnose issues
orbcode mcp tools issues
orbcode mcp resources issues
orbcode mcp read issues 'issue://123'
orbcode mcp prompts issues
orbcode mcp prompt issues triage '{"severity":"high"}'
orbcode mcp call issues search '{"query":"is:open"}'
```

`diagnose` 为区分连接/认证和策略失败，会绕过 runtime call permission gate 进行探测，但不会授予
后续调用。模型看到的工具名是 `mcp__<server>__<tool>`。trusted prompts 也会出现在 TUI slash
suggestions 和 skill catalog；resource/prompt content 保留 MCP content type。

## Trust 与权限

新 server 初始为 unknown：

```bash
orbcode mcp trust issues
orbcode mcp distrust issues
orbcode mcp untrust issues
```

只有 server trusted 且普通规则引擎允许 `mcp__...` 时调用才成功。信任不能授予工具权限，
权限也不能启动 unknown/denied server。session-owned MCP 保留 session-local trust；持久定义把
trust 写入兼容 settings 和 registry store。

## OAuth

OAuth token 与 server definition 分开存储，status/control 输出会脱敏。

```bash
orbcode mcp auth status

orbcode mcp auth login issues --access-token "$TOKEN" \
  --refresh-token "$REFRESH" --token-endpoint https://auth.example.com/token

orbcode mcp auth device-login issues --client-id orbcode-cli \
  --scope mcp.read --scope mcp.write

# 省略 --client-id 会尝试 RFC 7591 dynamic registration。
orbcode mcp auth browser-login issues --scope mcp.read

orbcode mcp auth logout issues
```

server advertised metadata 可补齐省略的 authorization/token/registration endpoints。浏览器登录
使用 PKCE 与 loopback callback。公网 endpoints 接受 TLS/SSRF 检查；明确的本地 MCP 配置保留
本地流程处理。

## 热重载与故障

配置文件变化会新增、删除或重启 server。解析/变量展开错误会带 source/path 报告，而不是部分
应用歧义配置。撤销信任会关闭活动 stdio client。

排查顺序：

1. `orbcode mcp servers` 查看 load/status/trust/auth。
2. `orbcode mcp diagnose <server>` 查看 transport/handshake。
3. `orbcode mcp auth status` 查看 token expiry/refresh。
4. 检查对应 `mcp__server__tool` 权限。
5. 只有需要 doctor 实际连接时才设置 `ORBCODE_DOCTOR_MCP_PROBE=1`。
