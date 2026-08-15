# 快速开始

[English](../getting-started.md) · [简体中文](getting-started.md)

Orb Code 是 alpha 阶段的 Claude Code CLI 原生 Rust 重新实现。它以一个
`orbcode` 二进制文件发布，默认使用相同的 `~/.claude` 设置和会话格式。

## 环境要求

- 通过 rustup 管理、支持 Rust 2024 的稳定工具链（1.85 或更高）。
- 用于获取仓库上下文的 `git`。
- 推荐安装 `ripgrep`（`rg`）以获得最快的 `Grep`；Orb Code 有较慢的内置后备实现。
- 只有启用系统沙箱时才需要 macOS 的 `sandbox-exec`（系统自带）或 Linux 的 `bwrap`。

安装后运行 `orbcode doctor`，可检查当前 home、provider、凭据、工具权限、
沙箱 runner、会话、MCP servers 和外部命令。默认不会执行网络探测。

## 从源码安装

当前尚未发布 crate 或 npm 包。

```bash
git clone https://github.com/beiwei30/orbcode.git
cd orbcode

# 安装到 PATH。
cargo install --path cli

# 或在 target/release/orbcode 生成 release 二进制。
cargo build --release -p orbcode
```

tag 构建会为 Linux x86-64、Apple Silicon macOS 和 Windows x86-64 生成压缩包。
在稳定发布渠道建立之前，请把二进制和新增的磁盘格式视为 alpha 软件。本地可运行
`scripts/package-release.sh --out-dir dist` 生成相同格式的产物。

在源码 checkout 中，也可用 `cargo run -p orbcode -- <参数>` 或
`scripts/run.sh <参数>`，无需安装。

## 认证

### Anthropic

Anthropic 是默认 provider，可使用 API key 或兼容的 OAuth token：

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
# 或：export CLAUDE_CODE_OAUTH_TOKEN="..."

orbcode -p "只回复 OK"
```

对应的 Orb Code 前缀变量是 `ORBCODE_ANTHROPIC_API_KEY`、
`ORBCODE_ANTHROPIC_AUTH_TOKEN` 和 `ORBCODE_OAUTH_TOKEN`。

### OpenAI API

```bash
export OPENAI_API_KEY="sk-..."
orbcode --provider openai -p "只回复 OK"
```

使用兼容 Chat Completions 服务时设置 `OPENAI_BASE_URL`，用 `OPENAI_MODEL`
选择模型。由于兼容服务存在差异，且没有服务端 token 计数，该路径目前是 beta。

### ChatGPT/Codex 订阅

Orb Code 提供独立、实验性的 ChatGPT 订阅登录路径：

```bash
# 浏览器 PKCE 回调。
orbcode auth login --provider openai --method chatgpt

# 无图形界面主机的 device-code 流程。
orbcode auth login --provider openai --method chatgpt --device-code

orbcode auth status
env -u OPENAI_API_KEY -u ORBCODE_OPENAI_API_KEY \
  orbcode --provider openai -p "只回复 OK"
```

凭据存放在 `<home>/auth.json`。Orb Code 不读取或修改 `~/.codex/auth.json`。
显式 OpenAI API key 的优先级高于已保存的订阅登录。使用
`orbcode auth logout --provider openai` 删除该凭据。

Gemini 和 Grok 只因兼容工作而存在于 provider 枚举中，并没有可用 adapter；
选择它们会返回 `unsupported_provider`。

## 首次运行

在要处理的仓库目录启动交互式终端 UI：

```bash
cd my-project
orbcode
```

输入提示后按 Enter。用 `/help` 查看命令和快捷键，`/permissions` 检查权限预设，
`/doctor` 查看运行状况。`Shift+Tab` 在请求批准、自动批准、完全访问和计划之间切换。

常用无头形式：

```bash
orbcode -p "解释这个仓库"
orbcode -p --output-format json "列出 workspace crates"
orbcode --continue                    # 在 TUI 中恢复当前工作区最近会话
orbcode sessions                      # 列出持久化会话
```

默认交互预设允许常见的工作区内操作，越界前会询问。原始无头配置更保守，除非选择
预设或增加规则。授予无人值守的广泛权限前请阅读[权限与沙箱](permissions.md)。

## 隔离 Orb Code 状态

Orb Code 默认使用 `~/.claude`，以共享兼容设置和会话。要选择独立 home，请在启动前创建：

```bash
mkdir ~/.orbcode
```

数据不会自动复制，因此新的 `~/.orbcode` 没有凭据、设置或会话。删除它会回到共享默认值，
也可用 `ORBCODE_HOME` 指定目录。详见 [home 目录解析](configuration.md#home-目录)。

## 下一步

- 学习 [TUI、会话和后台任务](user-guide.md)。
- 在[配置](configuration.md)中设置持久默认值。
- 用[权限与沙箱](permissions.md)限制工具。
- 通过 [MCP](mcp.md) 连接外部工具。
- 结合 [CLI 参考](cli-reference.md)和 [stream-JSON 指南](stream-json.md)编写脚本。
