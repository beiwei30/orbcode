# Orb Code

[English](README.md) · [简体中文](README.zh-CN.md)

Claude Code CLI 的原生 Rust 重新实现，以单个 `orbcode` 二进制文件发布。

[![CI](https://github.com/beiwei30/orbcode/actions/workflows/ci.yml/badge.svg)](https://github.com/beiwei30/orbcode/actions/workflows/ci.yml)
![状态：alpha](https://img.shields.io/badge/status-alpha-orange)
![Rust 2024](https://img.shields.io/badge/rust-2024-informational)
[![Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

Orb Code 以关键位置的字节级兼容为目标：JSONL 会话记录、设置分层、
`CLAUDE.md` 与 `.mcp.json` 发现、常用 CLI 参数以及 stream-JSON 事件。
默认使用相同的 `~/.claude` 目录，因此兼容的会话和配置无需迁移。

> **Alpha（`0.0.1`）。** 当前尚未发布 crate/npm 包，也没有稳定发布渠道；
> 不同提交之间的接口可能变化。依赖实验功能前请查看如实记录的
> [功能状态矩阵](docs/zh-CN/feature-status.md)。

## 为什么选择 Orb Code？

- 单个原生二进制文件，启动时无需 Node.js 运行时。
- 同时提供交互式 TUI、文本/JSON 输出和双向 stream-JSON 自动化接口。
- 支持 Anthropic、OpenAI 兼容 API，以及实验性的 ChatGPT/Codex 订阅登录。
- 提供工作区感知工具、结构化 Bash 权限、操作系统沙箱、hooks、agents、
  skills、plugins、后台任务和 MCP。
- 分层 Rust workspace，并为贡献者提供兼容性 fixtures 和聚焦的集成测试。

## 安装

Orb Code 当前需要从源码构建，要求 Rust 1.85 或更高版本：

```bash
git clone https://github.com/beiwei30/orbcode.git
cd orbcode
cargo install --path cli
```

运行时需要 `git`，并推荐安装 `ripgrep`。操作系统沙箱还需要 macOS 的
`sandbox-exec` 或 Linux 的 `bwrap`。运行 `orbcode doctor` 可检查本机环境。

## 一分钟上手

Anthropic 是默认 provider：

```bash
export ANTHROPIC_API_KEY="sk-ant-..."

orbcode                                      # 交互式 TUI
orbcode -p "解释这个仓库"                    # 单次无头执行
orbcode --continue                           # 恢复当前工作区最近会话
orbcode doctor
```

也可以不使用 OpenAI API key，直接登录 ChatGPT/Codex 订阅：

```bash
orbcode auth login --provider openai --method chatgpt
# 无图形界面的主机请增加 --device-code

orbcode --provider openai -p "只回复 OK"
```

进入 TUI 后，先尝试 `/help`、`/permissions` 和 `/doctor`。`Shift+Tab` 会在
“请求批准”“自动批准”“完全访问”和“计划”之间切换。

不安装、直接从源码运行：

```bash
cargo run -p orbcode -- -p "总结这个 workspace"
# 或：scripts/run.sh -p "总结这个 workspace"
```

认证方式、状态目录选择和首次运行说明见[快速开始](docs/zh-CN/getting-started.md)。

## 文档

| 我想要…… | 阅读 |
| --- | --- |
| 安装并运行第一个提示 | [快速开始](docs/zh-CN/getting-started.md) |
| 学习 TUI、会话、工具和后台任务 | [用户指南](docs/zh-CN/user-guide.md) |
| 配置 provider、模型、设置和代理 | [配置](docs/zh-CN/configuration.md) |
| 安全地控制工具访问 | [权限与沙箱](docs/zh-CN/permissions.md) |
| 添加 agents、skills、hooks 或 plugins | [扩展](docs/zh-CN/extensions.md) |
| 连接 MCP servers | [MCP](docs/zh-CN/mcp.md) |
| 编写 CLI 脚本或消费 NDJSON | [CLI 参考](docs/zh-CN/cli-reference.md) · [Stream-JSON](docs/zh-CN/stream-json.md) |
| 集成编辑器或轻量客户端 | [ACP 与 app-server 集成](docs/zh-CN/integrations.md) |
| 核对支持状态或排查问题 | [功能状态](docs/zh-CN/feature-status.md) · [故障排查](docs/zh-CN/troubleshooting.md) |

完整英文手册位于 [`docs/`](docs/README.md)，中文镜像位于
[`docs/zh-CN/`](docs/zh-CN/README.md)。

## 参与贡献

欢迎贡献，尤其是有针对性的兼容性 fixtures、缺失的 provider/tool 行为、
TUI 改进、跨平台沙箱验证，以及由代码或测试支持的文档修正。

```bash
scripts/check.sh          # fmt、clippy、check、tests
scripts/check.sh --quick  # 迭代时跳过 tests
```

提交 PR 前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。架构与 crate 所有权见
[CLAUDE.md](CLAUDE.md)，仓库约定见 [AGENTS.md](AGENTS.md)。安全漏洞请按
[SECURITY.md](SECURITY.md) 私下报告。

## 许可证与从属关系

[Apache-2.0](LICENSE)。

Orb Code 是独立、非官方的重新实现，与 Anthropic 没有附属、授权、背书或
赞助关系。“Anthropic”“Claude”和“Claude Code”是 Anthropic PBC 的商标，
这里只用于标识兼容的 CLI、格式与 API。项目问题应提交到本仓库，而不是
Anthropic 客服。
