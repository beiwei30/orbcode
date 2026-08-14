# CLI 参考

[English](../cli-reference.md) · [简体中文](cli-reference.md)

当前二进制的 `orbcode --help` 和 `orbcode help <command>` 是参数权威来源。全局选项可放在
subcommand 前后。

```text
orbcode [OPTIONS] [PROMPT] [COMMAND]
```

没有 command 时启动 TUI；带 `-p/--print` 时位置 prompt 以无头模式运行。

## 全局选项

| 领域 | 选项 |
| --- | --- |
| 会话 | `-c/--continue`、`-r/--resume [ID]`、`--session-id ID` |
| 无头 | `-p/--print`、`--output-format text|json|stream-json`、`--input-format text|stream-json`、`--verbose`、`--append-system-prompt TEXT` |
| Provider | `--provider`、`--fallback-provider`、`--max-retries` |
| 权限 | `--permission-mode default|bypassPermissions|plan|auto`、`--allow-tools BOOL`、`--allow-network BOOL`、`--allowed-tools RULES`、`--disallowed-tools RULES`、`--add-dir DIR` |
| 沙箱 | `--sandbox-mode danger-full-access|workspace-write|read-only`、`--sandbox-network BOOL` |
| 配置 | `--settings FILE_OR_JSON`、可重复的 `--mcp-config FILE_OR_JSON` |

provider enum 也接受 `gemini`、`grok`，但 adapter 未实现。没有 `--model`，请用 `/model`、
settings 或模型环境变量。print mode 的 `stream-json` 需要 `--verbose`。`--resume` 无值时选择
最近会话；后续位置 token 可能被当成 ID，自动化推荐 `--resume=<ID>`。

## 命令

### 会话与 turns

| 命令 | 用途 |
| --- | --- |
| `tui` | 启动本地交互 UI。 |
| `prompt <PROMPT> [--session ID] [--bg]` | 运行一轮或排队后台任务。 |
| `resume <SESSION_ID> [PROMPT]` | 打开已有会话。 |
| `fork <SESSION_ID> [--title T] [--note N] [--prompt P] [--tui]` | 从 transcript 创建新会话。 |
| `sessions [--json]` | 列出会话；JSON mode 是 NDJSON。 |
| `rename <SESSION_ID> <NEW_TITLE>` | 覆盖生成标题。 |

### 后台工作

| 命令 | 用途 |
| --- | --- |
| `ps` | 列出持久后台提示任务。 |
| `logs <JOB_ID> [--follow]` | 输出/跟随日志。 |
| `attach <JOB_ID>` | 用 TUI 附加任务/会话。 |
| `kill <JOB_ID>` | 取消后台任务。 |

### 检查与直接工具调用

| 命令 | 用途 |
| --- | --- |
| `providers` | 活动 provider chain、模型、capabilities、权限摘要。 |
| `context` | 预览指令、roots、git 上下文。 |
| `tools` | 输出实时 tool registry。 |
| `tool <TOOL_NAME> [JSON_INPUT] [--session ID]` | 经过普通权限调用单个工具。 |
| `doctor` | offline-first 环境检查。 |
| `doctor cleanup-orphans --dry-run|--yes [--stale-running-days N]` | 预览/删除孤立 child-session artifacts。 |
| `advanced` | 输出 active/deferred advanced capabilities。 |

### 认证

```bash
orbcode auth status
orbcode auth login --provider <PROVIDER> \
  --method api-key|o-auth-device|chatgpt [--token TOKEN] [--env-var NAME]
orbcode auth login --provider openai --method chatgpt [--device-code]
orbcode auth logout [--provider PROVIDER]
```

logout 不带 `--provider` 时删除所有持久 provider auth metadata。

### MCP

```text
mcp capabilities
mcp servers
mcp diagnose SERVER
mcp add SERVER --transport TRANSPORT --endpoint ENDPOINT [--summary S] [--auth SPEC] [--enabled]
mcp remove SERVER
mcp tools SERVER
mcp call SERVER TOOL [JSON_INPUT] [--session ID]
mcp resources SERVER
mcp read SERVER URI
mcp prompts SERVER
mcp prompt SERVER PROMPT [JSON_ARGUMENTS]
mcp trust|distrust|untrust SERVER
mcp auth status|login|device-login|browser-login|logout ...
```

配置、OAuth 和双 gate 策略见 [MCP](mcp.md)。

### 集成

| 命令 | 状态 | 用途 |
| --- | --- | --- |
| `acp` | Experimental | stdio 上的 ACP v1 adapter。 |
| `serve --stdio` | Experimental，hidden | 被父进程管理的 app-server stdio。 |
| `serve --socket PATH [--auth-token TOKEN]` | Experimental，hidden | Unix socket listener。 |
| `serve --websocket ADDR [--auth-token TOKEN] [--allowed-origin ORIGIN]...` | Experimental，hidden | WebSocket listener。 |
| `remote ENDPOINT --token TOKEN` | Experimental | 完全由现有 socket/WebSocket server 驱动的 TUI。 |

socket/WebSocket 允许依次重连，但同一时间只有一个 active client，并要求 token。未提供 token
时会生成并写入 startup JSON。stdio 绑定父进程并隐式 trusted。

## 示例

```bash
orbcode -p --output-format json "总结仓库"
orbcode -p --verbose --output-format stream-json "运行聚焦测试"
orbcode --settings '{"model":"sonnet"}' --mcp-config ./ci/mcp.json \
  -p "总结 incidents"
orbcode --allow-tools true \
  --allowed-tools "Read,Grep,Bash(cargo test:*)" \
  --sandbox-mode workspace-write --sandbox-network false \
  -p "诊断测试失败"
```

## 退出码

| Code | 含义 | Result subtype |
| --- | --- | --- |
| `0` | 成功 | `success` |
| `1` | Provider、model 或 tool 执行错误 | `error_during_execution` |
| `2` | 执行前参数无效或缺少 prompt | 无 result event |
| `3` | 凭据被拒绝 | `error_during_execution` |
| `4` | 工具被权限策略拒绝 | `error_during_execution` |
| `5` | Turn 被中断 | `error_during_execution` |
| `6` | Turn 上限 | `error_max_turns`（reserved） |
| `7` | 美元预算上限 | `error_max_budget_usd`（exit mapping 保留） |

5–7 已由测试固定，但并非都能从普通 print 子进程到达。print mode 当前使用进程默认 SIGINT，
终端 Ctrl-C 通常退出为 130。
