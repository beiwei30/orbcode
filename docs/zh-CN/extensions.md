# 扩展与自定义

[English](../extensions.md) · [简体中文](extensions.md)

Orb Code 可发现兼容的指令、commands、agents、skills、输出风格、hooks、快捷键和已安装
plugins。项目提供的可执行自定义需要 trusted project，且可能受 managed policy 限制。

## 指令与 memory

指令上下文来自 managed、user、project 和 directory-scoped sources：

- 当前目录及祖先中的 `CLAUDE.md`、`.claude/CLAUDE.md`。
- 用户指令 `<home>/CLAUDE.md`。
- `<home>/rules/*.md` 和 `.claude/rules/*.md`。
- 工具进入嵌套路径时的目录级 `.claude/CLAUDE.md`。

用 `orbcode context`、`/instructions`、`/memory` 检查加载内容。
`autoMemoryEnabled` 控制 auto-memory，TUI 负责创建和更新 memory file。

## 自定义 slash commands

`<home>/commands/` 与 `.claude/commands/` 下的 Markdown 会成为 slash commands。文件名是
命令名，正文是提示模板。已启用 plugin 也可贡献 commands。避免与内置命令同名。

## Agents

定义位于 `<home>/agents/*.md` 和 `.claude/agents/*.md`，使用 YAML frontmatter + Markdown。
可指定 description、model、tools、permission mode、skills、memory、background behavior 和
hooks；正文作为 agent prompt。

优先级为 project > user > built-in；plugin agents 使用 namespace。agent 定义的 `Stop`
hook 会映射为 `SubagentStop`，与 child loop 事件一致。用 `/agents` 检查解析结果。
`Agent` 工具同步运行本地 subagent；持久异步工作由 background tasks/workflows 表示。

## Skills

skill 是包含 `SKILL.md` 的目录：

```text
.claude/skills/release-check/
├── SKILL.md
├── scripts/
└── references/
```

frontmatter 提供名称和描述，正文是操作指令。`Skill` 工具按需加载，`/skills` 显示发现结果
和 warnings。解析优先级为 project、plugin、user、bundled、MCP prompt skills。只有 trusted
MCP server 的 prompts 才会作为 skills，plugin/MCP 名称会被隔离，不能静默替换其他扩展。

## 输出风格

`<home>/output-styles/*.md` 和 `.claude/output-styles/*.md` 定义 prompt styles。内置
`default`、`Explanatory`、`Learning`。project 优先于 user 和 plugin；plugin 值有 namespace。
通过 `/output-style` 或 `outputStyle` 选择，managed policy 可锁定。

## Hooks

command hooks 配置在 settings `hooks` 中，也可来自 agents、skills 或 enabled plugins。

| 事件 | 运行时机 |
| --- | --- |
| `UserPromptSubmit` | 接受用户输入后、provider turn 前。 |
| `PreToolUse` | 已获许可的工具 dispatch 前。 |
| `PostToolUse` | 工具成功后。 |
| `PostToolUseFailure` | 工具失败后，可附加上下文并建议重试。 |
| `Stop` | main agent 即将停止时。 |
| `StopFailure` | stop-hook 处理失败时。 |
| `SubagentStop` | child agent 即将停止时。 |

运行时支持 allow/deny/ask、`updatedInput`、`additionalContext`、stop feedback 和失败重试指导。
`PreToolUse` 改写输入后仍会重新检查 deny，hook 不能借此绕过配置或已记住的拒绝。

不受信任项目不会运行 project/local settings hooks。Managed policy 可要求 managed-only
hooks、限制 HTTP hook URL 和可接收的环境变量。`SessionStart`、`SessionEnd`、`PreCompact`、
`Notification` 尚未实现。用 `/hooks` 检查有效集合。hooks 以你的用户权限执行，请审查命令，
不要在配置中提交 secrets。

## 快捷键、主题与 statusline

`<home>/keybindings.json` 覆盖内置 keymap。可配置动作仅限安全 TUI 行为，如 transcript/todo/job
切换、历史搜索、清空输入、行首/行尾。`/keybindings` 显示加载结果与 warnings。

`/theme` 可选 auto/dark/light、daltonized 和 ANSI palettes；`/vim` 切换 normal/Vim 输入。
`statusline.command` 由客户端执行，默认每 30 秒刷新。

## Plugins

实验性 plugin loader 读取 `<home>/plugins/installed_plugins.json` 的 v1/v2 格式，再应用
settings `enabledPlugins`。plugin 可贡献：

- commands、agents、skills、hooks、output styles；
- `.mcp.json` servers；
- manifest 声明的 tools。

manifest 位于 `.claude-plugin/plugin.json` 或顶层 `plugin.json`。plugin tool 名形如
`plugin__<plugin>__<tool>`，其他名称也会按需 namespace，防止覆盖内置或其他 plugin。

启用 plugin 等于信任其代码、hooks、MCP processes 和 tool adapters。Orb Code 当前只消费已安装
index，不提供 marketplace 安装 UI。Managed `strictPluginOnlyCustomization` 可要求指定自定义
surface 只能来自 plugins。
