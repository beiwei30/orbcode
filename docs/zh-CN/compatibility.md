# Claude Code 兼容性

[English](../compatibility.md) · [简体中文](compatibility.md)

与 TypeScript Claude Code CLI 的字节级兼容是主要设计目标，并由捕获 fixtures 和 Rust/TypeScript
对比约束。Orb Code 是独立非官方项目；兼容名称只标识所实现的格式或 API，不表示从属关系。

## 无需转换即可共享

- `<home>/projects/<slug>/` 下 JSONL transcripts，包括跨 CLI resume、tool blocks、
  compaction boundaries、child-session metadata。
- Settings schema 与 User → Project → Local → Managed layering。
- `.mcp.json`、settings `mcpServers` 和兼容 MCP trust lists。
- `CLAUDE.md`、`.claude/CLAUDE.md` 与 rules discovery。
- Prompt history 和兼容凭据/token sources。
- resume/continue、print/output/input formats、permission mode/rules、additional directories、
  MCP/settings overlay、session ID、append system prompt 等常用参数。
- 由兼容 fixtures 覆盖的 stream-JSON init、content/progress、control correlation、result subtype。

默认 home 选择 `~/.claude` 就是为了直接共享。若不希望共享，请创建 `~/.orbcode` 或设置
`ORBCODE_HOME`。

## 有意差异

- Orb Code 是原生 Rust binary，运行不需要 Node.js。
- 没有 `--model`，使用 `/model`、settings 或 model env。
- `--dangerously-skip-permissions` 对应 `--permission-mode bypassPermissions`，并可能被 managed
  policy 禁用。
- fork 使用 `fork` command，而不是 `--fork-session`。
- 缺少 `--debug`、`--strict-mcp-config`、`--include-partial-messages`、`--max-turns`、`--agents`、
  `--system-prompt`；支持 `--append-system-prompt`。
- headless 进程退出码更细，但最终 `result.subtype` 兼容。
- stream-JSON 对未知/不支持操作返回关联错误，`rewind_files` 不会伪装成 transcript rewind。
- Hook 覆盖不完整，见[扩展](extensions.md#hooks)。
- provider adapter 当前只有 Anthropic 和 OpenAI 路径。

## 兼容纪律

captured fixtures、render goldens、public-surface audits 会让格式漂移可见。`ANTHROPIC_*`、
`OPENAI_*`、`CLAUDE_CODE_*`、`CLAUDE_CONFIG_DIR`、`~/.claude` 是兼容 alias，不是旧品牌，
不能随意改名。`scripts/audit-brand.sh` 同时检查旧项目标识不泄漏、必要兼容名称不消失。

两套 CLI 行为不一致时，请报告准确版本/commit、命令、active home 和脱敏最小 fixture，禁止附
真实 transcripts 或 tokens。上游新增功能可能滞后，应如实登记到[功能状态](feature-status.md)，
不能在未实现时写成支持。
