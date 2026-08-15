# Orb Code 文档

[English](../README.md) · [简体中文](README.md)

本手册按“你要完成什么”组织。参数以可执行文件的 `orbcode --help` 为准，
这些页面负责解释各部分如何配合。

## 从这里开始

- [快速开始](getting-started.md) — 安装、认证，并运行第一个交互式或无头提示。
- [用户指南](user-guide.md) — TUI、工具、会话、后台任务、上下文、计划和持久目标。
- [故障排查](troubleshooting.md) — 诊断认证、沙箱、MCP、状态目录和会话问题。

## 配置与扩展

- [配置](configuration.md) — home 解析、设置层、支持的设置、环境变量、项目文件和代理。
- [权限与沙箱](permissions.md) — 预设、规则、项目边界、系统沙箱和 MCP 的第二道信任门。
- [扩展](extensions.md) — 指令、agents、skills、commands、输出风格、hooks、快捷键和 plugins。
- [MCP](mcp.md) — 配置 servers，并使用 tools、resources、prompts、OAuth、信任和热重载。

## 自动化与集成

- [CLI 参考](cli-reference.md) — 全局选项、所有命令族、示例和退出码。
- [无头模式与 stream-JSON](stream-json.md) — 输出模式、双向 NDJSON、控制请求、权限回调和事件兼容性。
- [集成](integrations.md) — ACP、Zed 和实验性的 app-server 协议。

## 项目状态与兼容性

- [功能状态](feature-status.md) — 哪些能力稳定、实验中、延后或明确不支持。
- [Claude Code 兼容性](compatibility.md) — 可直接共享的状态与已知差异。
- [交互式提问](interactive-questions.md) — 能力门控的 `AskUserQuestion` 契约。
- [持久目标](persistent-goals.md) — 目标状态、监督、恢复和客户端支持。

## 贡献者与设计文档

- [参与贡献](../../CONTRIBUTING.md) — 开发环境和验证流程。
- [架构](../../CLAUDE.md) — crate 边界和请求流。
- [仓库指南](../../AGENTS.md) — 详细贡献约定。
- [设置架构](settings-architecture.md) — 类型化所有权和原始 JSON 边界。
- [ACP 支持矩阵](acp-support.md)和 [Zed 冒烟指南](acp-zed-smoke.md)。
- [`../plans/`](../plans/) — 设计记录和延后产品计划，不代表面向用户的承诺。

文档必须描述当前实现，而不是未来设想。当行为与文字冲突时，请检查命令的
`--help`、所属 crate 的公开模块文档和聚焦测试，然后修正文档。
