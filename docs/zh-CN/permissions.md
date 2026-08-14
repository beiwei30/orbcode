# 权限与沙箱

[English](../permissions.md) · [简体中文](permissions.md)

权限决定工具能否运行；沙箱在操作系统层限制已获准的 `Bash` 进程。二者互补：allow
不会自动创建沙箱，沙箱也不会授予工具权限。

## 交互预设

TUI 通过 `/permissions` 或 `Shift+Tab` 提供三套完整策略和 Plan mode。

| 预设 | CLI mode | 越界处理 | 沙箱 |
| --- | --- | --- | --- |
| 请求批准 | `default` | 网络、外部副作用、沙箱升级或允许 roots 之外访问时询问用户。 | Workspace write，网络关闭。 |
| 自动批准 | `auto` | 使用运行时自动越界审查。 | Workspace write，网络关闭。 |
| 完全访问 | `bypassPermissions` | 不询问，工具可跨越边界。 | `danger-full-access`，网络开启。 |
| 计划 | `plan` | 隐藏执行工具。 | 不允许模型执行。 |

兼容 alias `acceptEdits` 和 `dontAsk` 会解析为 `default` 与 `bypassPermissions`。
Managed policy 可禁用完全访问。显式 CLI/环境限制保留来源，因此隐式默认预设不会静默放宽它们。

## 权限规则

规则位于 `permissions.allow`、`permissions.ask`、`permissions.deny`，或来自
`--allowed-tools` / `--disallowed-tools`。优先级恒为 `deny` > `ask` > `allow`。

```json
{
  "permissions": {
    "allow": ["Read", "Grep", "Bash(cargo check:*)", "mcp__issues__search"],
    "ask": ["Bash(git push:*)"],
    "deny": ["Read(./secrets/**)", "Bash(rm:*)"],
    "additionalDirectories": ["../shared-lib"]
  }
}
```

规则可命名整个工具（`Read`），也可用兼容的括号形式约束参数。CLI 列表可用逗号或空格
分隔，解析时会保留括号内容。

`Bash` 规则基于 tree-sitter Bash 语法树评估，能识别管道、subshell、命令替换、操作符和
复合命令，不能仅靠改空格绕过 deny。请保持规则狭窄，并用
`orbcode tool Bash '{"command":"..."}'` 测试意外情况。

交互中记住的批准会成为 session rule。Managed permission rules 仍然权威，也可配置为唯一来源。

## 工作区边界

当前目录与所有 `--add-dir` / `permissions.additionalDirectories` 组成允许 roots。
Ask/Auto 预设下，path-aware tools 可在 roots 内工作，外部路径需要越界审查。symlink 与规范化
检查会阻止简单路径穿越。

`ORBCODE_TRUSTED_PROJECT=0` 把目录标记为不受信任，并禁用项目 hooks；这并不表示任意工具
执行变得安全。

## 总开关

- `--allow-tools` / `ORBCODE_ALLOW_TOOLS`：控制模型发起的本地工具和修改。
- `--allow-network` / `ORBCODE_ALLOW_NETWORK`：控制 `WebFetch`、`WebSearch` 等网络工具。
- `ORBCODE_PROVIDER_NETWORK`：独立控制 provider API 流量。

这样可允许模型 API，同时禁止工具出站网络。

## 操作系统沙箱

```bash
orbcode --sandbox-mode workspace-write --sandbox-network false \
  --add-dir ../shared-lib -p "运行聚焦测试"
```

| Mode | 行为 |
| --- | --- |
| `danger-full-access` | 没有 OS 沙箱，是底层配置默认值和 Full Access 预设。 |
| `workspace-write` | 在允许 roots 内读写，把边界和网络策略投射到平台 runner。 |
| `read-only` | 平台 runner 阻止文件写入。 |

macOS 使用 `sandbox-exec`；Linux 使用 Bubblewrap，请求沙箱但缺少 `bwrap` 时 fail closed。
Windows runner 参数已有测试，真实主机验证仍是实验性的。用 `orbcode doctor` 检查可用 runner。

持久化 `sandbox` 对象控制 TUI 本地偏好：

```json
{
  "sandbox": {
    "enabled": true,
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    "excludedCommands": ["docker:*"],
    "filesystem": {
      "allowWrite": ["./tmp"],
      "denyWrite": ["./secrets"],
      "denyRead": ["./private"],
      "allowRead": ["./private/public.md"]
    },
    "network": {
      "allowedDomains": ["example.com"],
      "allowUnixSockets": ["/tmp/service.sock"],
      "allowAllUnixSockets": false,
      "allowLocalBinding": false,
      "httpProxyPort": 8080,
      "socksProxyPort": 1080
    }
  }
}
```

`allowUnsandboxedCommands: false` 表示严格模式：模型命令必须进入沙箱，除非匹配
`excludedCommands`。excluded 表示刻意在沙箱外运行，不是拒绝，因此仍需配合权限规则。

## MCP 是第二道信任门

每个 MCP call 同时需要匹配权限（如 `mcp__issues__search`）和 trusted server。
信任不能代替权限，allow 也不能绕过 unknown/denied trust。使用 `orbcode mcp trust`、
`distrust`、`untrust`。撤销信任会关闭活动 stdio client。

## 安全的无人值守使用

```bash
orbcode --allow-tools true \
  --allowed-tools "Read,Grep,Bash(cargo check:*),Bash(cargo test:*)" \
  --disallowed-tools "Bash(git push:*),Bash(rm:*)" \
  --sandbox-mode workspace-write --sandbox-network false \
  -p "诊断失败测试"
```

CI 中使用一次性的 `ORBCODE_HOME`，不要把 token 放入仓库，并把启用的 hooks/plugins/MCP
servers 当作可执行代码审查。
