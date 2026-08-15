# 故障排查

[English](../troubleshooting.md) · [简体中文](troubleshooting.md)

先运行 `orbcode doctor`。它默认不联网，检查 build metadata、workspace/home、provider chain、
model capabilities、auth、network/tool permissions、sandbox、外部命令、session storage、
background jobs、MCP 和 extension loading。

## 凭据或 provider 失败

```bash
orbcode auth status
orbcode providers
orbcode doctor
```

- 检查 active provider；settings 中 `PROVIDER_TYPE` 可能选择了 OpenAI。
- OpenAI API key 优先于已保存 ChatGPT 登录。测试订阅路径时临时 unset
  `OPENAI_API_KEY`、`ORBCODE_OPENAI_API_KEY`。
- ChatGPT 凭据在 `<home>/auth.json`，不在 `~/.codex/auth.json`。
- Gemini/Grok adapter 未实现，必然失败。
- API-key adapter 接受自定义 base URL；固定 ChatGPT subscription backend 刻意忽略它。
- 只有需要实际发出极小 provider probe 时才设置 `ORBCODE_DOCTOR_PROBE=1`。

401 时重新登录或更换 key。timeout 时检查代理和 `ORBCODE_API_TIMEOUT_MS`，不要用提高重试次数
解决凭据错误。

## 找不到设置或会话

查看 `orbcode doctor` 输出的 home。顺序是 `ORBCODE_HOME`、`CLAUDE_CONFIG_DIR`、已有
`~/.orbcode`、`~/.claude`。最常见情况是空 `~/.orbcode` 遮住有数据的 `~/.claude`。
不会自动迁移；可删除/改名空目录，或在 Orb Code 停止时有意识地复制状态。

用 `orbcode sessions --json` 区分真正空列表和 TUI filter。`--continue` 按 workspace 选择，
其他项目 transcript 不会成为当前 workspace 的 latest session。

## 工具被拒绝

按顺序检查：

1. `allow_tools` 总开关和 active preset。
2. 匹配 deny rule（始终优先）。
3. 匹配 ask rule 或 outside-workspace boundary。
4. Web tools 的 network permission。
5. Bash 的 OS sandbox 错误。
6. MCP 同时需要 `mcp__...` rule 和 server trust。

“自动批准”仍会审查边界，不等于 Full Access。无交互 permission responder 的 headless 模式应使用
明确、狭窄的规则，不要依赖弹窗。

## 沙箱启动失败

- macOS 需要系统 `sandbox-exec`。
- Linux 需要 PATH 中的 `bwrap`；请求沙箱但缺失时 hard error。
- `danger-full-access` 表示没有 OS sandbox，即使权限规则很窄。
- `excludedCommands` 是允许时在沙箱外运行，不是 deny list。
- 排查时用 `--sandbox-network false` 将网络与文件系统策略分离。

## MCP server 不可用

```bash
orbcode mcp servers
orbcode mcp diagnose <SERVER>
orbcode mcp auth status
orbcode mcp tools <SERVER>
```

`${VAR}` 无值/默认值会导致配置展开失败。stdio 还需要有效 command/cwd/env；remote 可能需要刷新
OAuth token。连接成功后还要 trust server 并添加匹配权限。`diagnose` 只为诊断绕过 call gate。
设置 `ORBCODE_DOCTOR_MCP_PROBE=1` 才让 doctor 探测 MCP reachability。

## 后台或 child session 孤立

先用 `orbcode ps`、`orbcode logs <JOB_ID>`、`orbcode attach <JOB_ID>`。若 parent transcript
被外部删除，预览清理：

```bash
orbcode doctor cleanup-orphans --dry-run
orbcode doctor cleanup-orphans --dry-run --stale-running-days 7
```

审查候选后才使用 `--yes`。该命令删除 orphan metadata/artifacts，不删除健康 parent transcripts。

## TUI 显示问题

用 `/theme`、`/keybindings`、`/vim` 排除配置。为 scrollback/resize bug 保存 terminal trace，
issue 中注明 terminal、tmux、OS、窗口尺寸。附加前检查 trace 是否含 prompt/tool 私密内容。

## 诊断开关

这些是非稳定诊断接口：

| 变量 | 效果 |
| --- | --- |
| `ORBCODE_DOCTOR_PROBE=1` | 运行极小 live provider probe。 |
| `ORBCODE_DOCTOR_MCP_PROBE=1` | 探测 MCP server reachability。 |
| `ORBCODE_TUI_TERMINAL_TRACE=<path>` | 以 JSONL 记录 terminal writes/resizes/viewport；`1` 自动选择临时路径。 |
| `ORBCODE_TUI_RENDER_METRICS=1` | 输出每帧 metrics；路径可由 `ORBCODE_TUI_RENDER_METRICS_PATH` 指定。 |
| `ORBCODE_TUI_RESIZE_SETTLE_MS=<ms>` | 覆盖 resize debounce（默认 150 ms）。 |
| `ORBCODE_DEBUG_PROVIDER_ROUNDS=1` | 把 provider-round diagnostics 写入 transcript。 |
| `ORBCODE_DEBUG_AUTO_CONTINUE=1` | 写入 auto-continuation decisions，并启用 provider diagnostics。 |
| `ORBCODE_FORCE_RG_FALLBACK=1` | 强制内置 Grep engine。 |
| `ORBCODE_TRUSTED_PROJECT=0` | 标记项目不受信任，禁用 project-origin hooks。 |

## 报告有效 issue

请提供 `orbcode --version`、OS/target、脱敏 doctor rows、准确命令与退出码、active home、最小
复现。禁止发布 API keys、OAuth tokens、完整 auth files、私密 transcripts、MCP headers 或未经
审查的 terminal traces。安全漏洞按 [SECURITY.md](../../SECURITY.md) 私下报告。
