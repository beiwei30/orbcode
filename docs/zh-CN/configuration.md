# 配置

[English](../configuration.md) · [简体中文](configuration.md)

Orb Code 理解 Claude Code 兼容设置，同时提供自己的 `ORBCODE_*` 控制。请选择最小合适
作用域：secret 用进程环境变量，单个 checkout 用 local settings，可共享策略用 project
settings，个人默认值用 user settings。

## Home 目录

解析顺序：

1. 非空 `ORBCODE_HOME`。
2. 非空 `CLAUDE_CONFIG_DIR`。
3. 已经存在的 `~/.orbcode` 目录。
4. `~/.claude`。

默认使用 `~/.claude`，让两套 CLI 共享兼容设置、提示历史、凭据、MCP 配置和 JSONL
transcripts。Orb Code 永远不会自动创建 `~/.orbcode`；创建它表示明确选择全新的独立状态，
不会自动复制数据。空 `~/.orbcode` 遮住已有 `~/.claude` 时，`orbcode doctor` 会告警。

## 设置层

持久化设置从低到高合并：

1. User：`<home>/settings.json`。
2. Project：`<project>/.claude/settings.json`。
3. Local：`<project>/.claude/settings.local.json`。
4. Managed：企业 `managed-settings.json` 和排序后的 drop-ins。

后层标量覆盖前层；环境变量、权限规则、额外目录和 hooks 等集合按各自类型化规则合并。
Managed policy 可锁定顶层 key，此时 TUI/app-server 修改会明确失败。

`--settings <FILE_OR_JSON>` 提供单次 overlay，接受文件或以 `{` 开头的 inline JSON。
它支持模型、环境、权限、额外目录和预算控制，并不是可写的通用第五层。

## 常用设置

```json
{
  "model": "sonnet",
  "theme": "auto",
  "editorMode": "normal",
  "alwaysThinkingEnabled": false,
  "outputStyle": "default",
  "autoMemoryEnabled": true,
  "env": { "PROVIDER_TYPE": "anthropic" },
  "permissions": {
    "allow": ["Read", "Grep", "Bash(cargo check:*)"],
    "ask": ["Bash(git push:*)"],
    "deny": ["Read(./secrets/**)"],
    "additionalDirectories": ["../shared"]
  },
  "maxBudgetUsd": 2.0,
  "maxBudgetUsdStrictUnknownPricing": true,
  "statusline": {
    "command": "git branch --show-current",
    "refreshInterval": 30
  }
}
```

主题值：`auto`、`dark`、`light`、`dark-daltonized`、`light-daltonized`、
`dark-ansi`、`light-ansi`。`editorMode` 接受 `normal`/`emacs` 或 `vim`。
statusline 刷新间隔有效范围是 1–3600 秒，无效值回落到 30 秒。

`maxBudgetUsd` 对可计价 API 调用实施美元上限。订阅调用和未知/自定义模型不一定能换算为
API 美元；设置 `maxBudgetUsdStrictUnknownPricing` 可在无法计价时拒绝，而不是告警后继续。

权限和 sandbox 字段见[权限与沙箱](permissions.md)，hooks/plugins/styles 见[扩展](extensions.md)，
`mcpServers` 见 [MCP](mcp.md)。

## Provider 与模型

provider 优先级从显式 `--provider` 开始，其次是进程环境与 settings `env` 中的
`PROVIDER_TYPE`，兼容后备项优先级更低；默认 Anthropic。

没有 `--model` 参数。请用 `/model`、settings 的 `model` 或 provider 模型环境变量。
`opus`、`sonnet`、`haiku` 是针对当前 provider 解析的 family alias。ChatGPT 订阅使用
Responses endpoint，没有显式模型时默认 `gpt-5.6-sol`。

```json
{
  "model": "gpt-4o",
  "env": { "PROVIDER_TYPE": "openai" }
}
```

不要把 API key 提交到 settings。进程 API key 的优先级也高于已保存的 ChatGPT 订阅凭据。

## 环境变量

兼容 alias 的解析顺序：canonical 进程变量、alias 进程变量、canonical settings `env`、
alias settings `env`。

| Canonical | 兼容 alias | 用途 |
| --- | --- | --- |
| `PROVIDER_TYPE` | `ORBCODE_PROVIDER`、`CLAUDE_CODE_USE_OPENAI=true` | 选择 `anthropic` 或 `openai`。 |
| `ORBCODE_ANTHROPIC_API_KEY` | `ANTHROPIC_API_KEY` | Anthropic API key。 |
| `ORBCODE_ANTHROPIC_AUTH_TOKEN` | `ANTHROPIC_AUTH_TOKEN` | Anthropic bearer token。 |
| `ORBCODE_OAUTH_TOKEN` | `CLAUDE_CODE_OAUTH_TOKEN` | Anthropic 兼容 OAuth token。 |
| `ORBCODE_OPENAI_API_KEY` | `OPENAI_API_KEY` | OpenAI 兼容 API key。 |
| `ORBCODE_ANTHROPIC_BASE_URL` | `ANTHROPIC_BASE_URL` | Anthropic endpoint 覆盖。 |
| `ORBCODE_OPENAI_BASE_URL` | `OPENAI_BASE_URL` | OpenAI 兼容 endpoint；ChatGPT 订阅忽略。 |
| `ORBCODE_ANTHROPIC_MODEL` | `ANTHROPIC_MODEL` | Anthropic 模型覆盖。 |
| `ORBCODE_OPENAI_MODEL` | `OPENAI_MODEL` | OpenAI 模型覆盖。 |
| `ORBCODE_MAX_OUTPUT_TOKENS` | `CLAUDE_CODE_MAX_OUTPUT_TOKENS` | 输出 token 上限。 |
| `ORBCODE_MAX_CONTEXT_TOKENS` | `CLAUDE_CODE_MAX_CONTEXT_TOKENS` | 上下文窗口上限。 |
| `ORBCODE_AUTO_COMPACT_WINDOW` | `CLAUDE_CODE_AUTO_COMPACT_WINDOW` | 自动压缩阈值。 |
| `ORBCODE_API_TIMEOUT_MS` | `API_TIMEOUT_MS` | provider HTTP 超时。 |
| `ORBCODE_API_MAX_RETRIES` | `API_MAX_RETRIES` | provider 重试预算。 |
| `ORBCODE_WEB_ALLOWED_DOMAINS` | `CLAUDE_CODE_WEB_ALLOWED_DOMAINS` | Web allowlist。 |
| `ORBCODE_WEB_BLOCKED_DOMAINS` | `CLAUDE_CODE_WEB_BLOCKED_DOMAINS` | Web denylist。 |

常用 Orb Code 专用变量还包括 `ORBCODE_HOME`、`ORBCODE_FALLBACK_PROVIDER`、
`ORBCODE_MAX_RETRIES`、`ORBCODE_ALLOW_TOOLS`、`ORBCODE_ALLOW_NETWORK`、
`ORBCODE_ALLOWED_TOOLS`、`ORBCODE_DISALLOWED_TOOLS`、`ORBCODE_SANDBOX_MODE`、
`ORBCODE_SANDBOX_NETWORK` 和 `ORBCODE_TRUSTED_PROJECT`。

完整 alias 表（含 family model、自定义 headers/body/metadata、超时、重试延迟和工具超时）
与测试一起保存在 `config/src/env_compat.rs`。诊断变量见[故障排查](troubleshooting.md#诊断开关)。

## 代理

provider 请求和 ChatGPT 登录/刷新按以下顺序选择代理：

1. 合并 settings `env` 中的小写 `https_proxy` / `http_proxy`。
2. 进程 `HTTPS_PROXY`、`HTTP_PROXY`、`ALL_PROXY`（含小写形式）。
3. `ORBCODE_PROXY`、`CLAUDE_CODE_PROXY`、`ANTHROPIC_PROXY_URL`。
4. macOS System Configuration 的静态 HTTP/HTTPS 代理与例外。
5. 直连。

HTTPS 在必要时回退到 HTTP 代理；loopback 始终直连。显式代理遵循 `no_proxy`/`NO_PROXY`，
macOS 例外支持通配符和 IP CIDR。不会执行 PAC JavaScript；只有 PAC 时请显式配置代理。

```json
{
  "env": {
    "https_proxy": "http://127.0.0.1:7890",
    "http_proxy": "http://127.0.0.1:7890",
    "no_proxy": "localhost,127.0.0.1,::1"
  }
}
```

## 项目与 home 文件

| 路径 | 用途 |
| --- | --- |
| `CLAUDE.md`、`.claude/CLAUDE.md` | 项目与目录指令。 |
| `.claude/rules/*.md`、`<home>/rules/*.md` | 附加指令规则。 |
| `.claude/settings.json` | 可共享项目设置。 |
| `.claude/settings.local.json` | 本地项目设置，通常 gitignore。 |
| `.claude/agents/*.md`、`<home>/agents/*.md` | Agent 定义。 |
| `.claude/skills/<name>/SKILL.md`、`<home>/skills/...` | Skills。 |
| `.claude/commands/*.md`、`<home>/commands/*.md` | 自定义 slash commands。 |
| `.claude/output-styles/*.md`、`<home>/output-styles/*.md` | 输出风格。 |
| `.mcp.json` | 项目 MCP servers。 |
| `<home>/keybindings.json` | TUI 快捷键覆盖。 |
| `<home>/plugins/installed_plugins.json` | 已安装 plugin index。 |
| `<home>/auth.json` | Orb Code provider 凭据。 |
| `<home>/projects/<slug>/*.jsonl` | 会话 transcripts。 |

提交可执行 hooks 或 plugin 配置前请阅读[扩展](extensions.md)。不受信任项目不会执行
project/local hooks。

## Managed policy

企业 managed settings 可限制模型、映射模型 ID、允许/拒绝 MCP servers、要求 managed-only
hooks/permissions/MCP、禁用 bypass、要求 plugin-only customization、强制登录方式，并限制
HTTP hook URL 与导出的环境变量。deny 优先，managed keys 对 TUI/app-server 修改接口只读。

相关 keys：`availableModels`、`modelOverrides`、`allowedMcpServers`、`deniedMcpServers`、
`allowManagedHooksOnly`、`allowManagedPermissionRulesOnly`、`allowManagedMcpServersOnly`、
`strictPluginOnlyCustomization`、`forceLoginMethod`、
`permissions.disableBypassPermissionsMode`、`allowedHttpHookUrls`、`httpHookAllowedEnvVars`。

类型所有权和原始 JSON 兼容边界见[设置架构](settings-architecture.md)。
