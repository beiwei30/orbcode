# 用户指南

[English](../user-guide.md) · [简体中文](user-guide.md)

Orb Code 通过交互式 TUI、单次提示、后台任务、ACP 和 app-server 协议暴露同一套
核心能力。本页介绍常规本地工作流。

## 交互式 TUI

运行 `orbcode` 或 `orbcode tui`。位置参数中的提示会作为第一轮输入。

| 按键 | 操作 |
| --- | --- |
| `Enter` | 提交输入。 |
| `Shift+Enter`、`Alt+Enter`、`Ctrl+Enter` | 插入换行。 |
| `Tab` | 补全 slash command，或插入空格。 |
| `Shift+Tab` | 循环切换请求批准、自动批准、完全访问和计划。 |
| `Ctrl+R` | 打开会话历史。 |
| `Ctrl+O` | 切换详细 tool/transcript 显示。 |
| `PageUp` / `PageDown`、`Home` / `End` | 浏览时间线。 |
| `Esc` | 取消当前 turn、关闭浮层或退出 Vim 插入模式。 |
| `Ctrl+C` | 取消；空闲时再次按下退出。 |

在支持的终端中，鼠标选择和滚轮使用原生 scrollback。运行 `/help` 查看实时完整按键，
`/keybindings` 查看已配置覆盖。

### Slash commands

| 用途 | 命令 |
| --- | --- |
| 帮助与诊断 | `/help` (`/?`)、`/doctor`、`/config`、`/status`、`/context` (`/ctx`)、`/stats`、`/usage`、`/cost`、`/trace`、`/diff`、`/release-notes` |
| 项目 prompts | `/init`、`/review` |
| 会话 | `/sessions`、`/resume` (`/session`)、`/rename`、`/fork`、`/branch`、`/clear` (`/new`, `/reset`)、`/rewind` (`/checkpoint`)、`/compact` |
| 模型与行为 | `/model`、`/effort`、`/permissions`、`/sandbox`、`/plan`、`/goal`、`/output-style`、`/memory`、`/instructions` |
| 工具与扩展 | `/tools`、`/tool`、`/mcp`、`/agents`、`/skills`、`/hooks`、`/files`、`/add-dir`、`/jobs` (`/background`) |
| 外观与输入 | `/theme`、`/vim`、`/keybindings`、`/copy` |
| 账户与退出 | `/login`、`/logout`、`/exit` (`/quit`) |

命令面板还会加入用户/项目 commands、已启用 plugin commands、skills、受信任 MCP
prompts 和 workflows，因此可用内容会随工作区和配置变化。

## 无头提示

下面两种写法都执行一轮：

```bash
orbcode -p "解释失败原因"
orbcode prompt "解释失败原因"
```

`text` 输出最终文本，`json` 输出一个结果对象，`stream-json` 输出 NDJSON 事件。
stream 输出必须增加 `--verbose`。双向协议见[无头模式与 stream-JSON](stream-json.md)。
无头提示是单轮的；持久目标不会让无人值守的 print 进程无限续跑。

## 工具

`orbcode tools` 输出当前 registry，包括权限和网络要求。foundation registry 包含：

- 文件与 shell：`Read`、`Edit`、`Write`、`Glob`、`Grep`、`Bash`、`NotebookEdit`。
- Web：`WebFetch`、`WebSearch`。
- 计划与任务：进入/退出/验证计划、todos、task 列表、输出和取消。
- Agents、skills、工具发现、动态 workflows 和启发式 LSP 查询。
- 只有客户端声明完整交互能力时才出现的 `AskUserQuestion`。

`orbcode tool <TOOL_NAME> [JSON_INPUT]` 可直接调试单个工具，但仍经过权限策略。
MCP 和 plugin tools 会动态加入；请以 `orbcode tools` 为准，不要依赖固定数量。
计划验证只记录快照和计划状态，不能代替真正运行构建或测试。

## 会话

transcript 以兼容格式的 JSONL 持久化到 `<home>/projects/<workspace-slug>/`，写入按顺序
flush。大型 tool result 可能单独存储，transcript 中只保留预览。

```bash
orbcode sessions
orbcode sessions --json          # 每行一个 JSON 对象
orbcode --continue
orbcode --resume <SESSION_ID>
orbcode resume <SESSION_ID>
orbcode rename <SESSION_ID> "新标题"
orbcode fork <SESSION_ID> --title "另一种方案"
orbcode fork <SESSION_ID> --prompt "尝试另一条路径" --tui
```

`--session-id <ID>` 选择或创建特定会话；`--resume` 只恢复已有状态，自动化中不要混用。
裸 `-r` 会把后面的非选项 token 当作 session ID，生成命令时推荐 `--resume=<ID>`。
TUI 的 `/rewind` 只回退对话 checkpoint，不恢复工作区文件。

## 后台任务

```bash
orbcode prompt --bg "运行完整测试并总结失败"
orbcode ps
orbcode logs <JOB_ID>
orbcode logs <JOB_ID> --follow
orbcode attach <JOB_ID>
orbcode kill <JOB_ID>
```

任务会持久化状态与日志，状态包括 queued、running、completed、failed、cancelled、orphaned。
TUI 的 `/jobs` 打开同一界面。`doctor cleanup-orphans --dry-run` 只预览孤立的 child-session
元数据，真正删除必须使用 `--yes`。

## 上下文与压缩

`orbcode context` 显示会话初始指令、工作区 roots 和 git 上下文；`/context` 显示当前 token
占用。接近窗口上限时可自动把旧上下文压缩成摘要，并在兼容 stream 中发出
`compact_boundary`。使用 `/compact` 手动触发。主要变量是
`ORBCODE_MAX_CONTEXT_TOKENS` 和 `ORBCODE_AUTO_COMPACT_WINDOW`。

## 计划、任务、agents 与 workflows

- Plan mode 隐藏执行工具，让模型编写计划；`/plan` 控制模式，状态位于 `<home>/plans/`。
- Todo 是轻量 turn 指引；task tools 提供 pending、in-progress、completed 的持久单元。
- `Agent` 同步运行已配置的本地 subagent；定义可来自内置、用户、项目或 plugins。
- `Workflow` 以持久后台任务启动生成的动态工作流，目前是实验功能。

编写方式见[扩展](extensions.md)。

## 持久目标

每个会话可附加一个实验性的持久目标。`/goal` 可创建、查看、清除或继续。只有具备能力的
交互客户端才监督每次续跑；模型不能静默替换活动目标或扩大 token budget。

目标工具 `get_goal`、`create_goal`、`update_goal` 只在有能力门控的目标 turn 中出现，
模型只能按运行时策略完成目标或报告 blocked。重启会从 transcript 恢复状态。完整状态机和
客户端矩阵见[持久目标](persistent-goals.md)。
