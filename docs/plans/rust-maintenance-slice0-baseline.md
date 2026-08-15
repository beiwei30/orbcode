# Rust 维护 Slice 0：`b9ed5da` 基线与后续批次

## 技术结论

- 在 `rustc 1.97.1` / Clippy `0.1.97` 下，计划指定的
  `cargo clippy --workspace --lib` 原始命令仍产生 **1,817** 条 warning，
  其中选定 lint 家族 1,500 条、`unwrap_used` 15 条。原始数包含
  `compat-fixtures`；排除 compatibility 后的 production-library 基线为
  **1,799 / 1,484 / 15**。
- 15 个 production-library `unwrap_used` 中没有未经验证的外部输入直接
  触发点：10 个是 `std::sync::Mutex` poison 策略，3 个由同一表达式或锁内
  不变量证明，2 个十六进制转换由 `is_ascii_hexdigit` 先行验证。另有 1 个
  production binary unwrap，同样由 clap enum 不变量证明。
- 文本扫描的 120 个 `src/**/*.rs` `tokio::spawn` 中，逐项排除 test-only 后有 **58 个
  production spawn**。大部分有 channel/EOF、owner drop、显式 abort 或外层
  join；两个最高信号的待办是 `background_api` 正常完成后不退出的取消
  watcher，以及 AskUserQuestion 每次请求都会遗留到 300 秒的 timeout task。
- 初始的 46 个 `map_err(...to_string())` 文本候选中，32 个确实把原 error
  字符串化，14 个只是 timeout/channel 的固定文案。扩展扫描还找到一个
  WebSocket TLS source 丢失点和若干有意的 tool/wire 投影。MCP 中 20 个
  `reqwest::Error` 点已经可以直接使用现有 `McpError::http` 保留 source。
- `PermissionMode` 在 config、tools、core 和 app-server 内部保持 typed。
  `Option<String>` 只出现在 agent frontmatter 的短暂 parse 状态、历史 child
  session 持久化、background view 或明确 wire/transcript 兼容边界。

本报告是描述性审计，不断言这些 warning 本身就是 bug。表格用于精确查找，
比趋势图更适合逐位置复核，因此没有添加图表。

## 基线、口径与复现

审计对象：`b9ed5da6b012aee51d9c40604abb74b6babb1492`，运行前原工作树状态为
`## main...origin/main`，无 tracked 或 untracked 改动。

| 工具 | 版本 |
| --- | --- |
| rustc | `1.97.1 (8bab26f4f 2026-07-14)`，LLVM 22.1.6，`aarch64-apple-darwin` |
| cargo | `1.97.1 (c980f4866 2026-06-30)` |
| Clippy | `0.1.97 (8bab26f4f6 2026-07-14)` |

完整、稳定排序的位置清单由以下命令输出到 stdout：

```sh
scripts/rust-maintenance-report.py > /tmp/orbcode-rust-maintenance-report.md
```

要严格复现本快照的 commit 元数据，可从保留本脚本的 checkout 指向一个干净的
`b9ed5da` worktree：

```sh
git worktree add /tmp/orbcode-slice0-baseline b9ed5da
scripts/rust-maintenance-report.py \
  --repo /tmp/orbcode-slice0-baseline \
  > /tmp/orbcode-rust-maintenance-report.md
```

脚本只依赖 Python 标准库以及仓库要求的 Rust 工具链，不下载工具、不写源码，
并执行以下三组 JSON 诊断命令：

```sh
cargo clippy --workspace --lib --no-deps --message-format=json -- \
  -W clippy::pedantic -W clippy::unwrap_used

cargo clippy --workspace --bins --no-deps --message-format=json -- \
  -W clippy::pedantic -W clippy::unwrap_used

cargo clippy --workspace --all-targets --message-format=json -- \
  -W clippy::pedantic -W clippy::unwrap_used
```

报告按 lint、crate、文件、行、列排序并去重。`workspace-lib-command-raw` 是
命令本身的复现数字；`classified-production-library` 才是排除 compatibility
后的风险口径。这个区别解释了临时计划中的 1,817 与纯 production 的 1,799
为什么同时成立。

非 Clippy 源码候选用以下只读命令复现；对应 raw 数字依次为 120、46、11、4，
随后由本报告逐项做语义分类：

```sh
rg -n 'tokio::spawn' --glob '**/src/**/*.rs' --glob '!target/**'
rg -n 'map_err\([^\n]*to_string\(\)' \
  core/src tools/src mcp/src app-server/src app-server-client/src \
  app-server-transport/src app-server-protocol/src
rg -n 'Option\s*<\s*Option\s*<' --glob '*.rs' --glob '!target/**'
rg -n 'permission_mode\s*:\s*Option\s*<\s*String\s*>' \
  --glob '*.rs' --glob '!target/**'
```

### Scope totals

| scope | all warnings | selected warnings | `unwrap_used` |
| --- | ---: | ---: | ---: |
| workspace lib command（raw） | 1,817 | 1,500 | 15 |
| classified production library | 1,799 | 1,484 | 15 |
| workspace bin command（raw，含其 workspace lib 输入） | 1,878 | 1,536 | 16 |
| classified production other target | 74 | 47 | 1 |
| workspace all targets | 3,792 | 3,189 | 1,383 |

### Production-library 选定类别

| lint | count | lint | count |
| --- | ---: | --- | ---: |
| `missing_errors_doc` | 541 | `must_use_candidate` | 508 |
| `cast_possible_truncation` | 72 | `too_many_lines` | 62 |
| `doc_markdown` | 59 | `cast_lossless` | 46 |
| `match_same_arms` | 35 | `needless_pass_by_value` | 35 |
| `cast_possible_wrap` | 28 | `manual_let_else` | 27 |
| `cast_sign_loss` | 26 | `map_unwrap_or` | 21 |
| `unwrap_used` | 15 | `cast_precision_loss` | 9 |

原始命令比 production 分类多出的 16 个选定 warning 是
`compat-fixtures` 的 13 个 `must_use_candidate` 和 3 个 `doc_markdown`。

### Production/test/compatibility 分类

| classification | 判定 | all warnings | selected | unwrap |
| --- | --- | ---: | ---: | ---: |
| production-library | 出现在专用 `--lib` run，且不属于 compatibility/fixture | 1,799 | 1,484 | 15 |
| production-other-target | 非 test 的 binary target；当前全部来自 `orbcode` CLI | 74 | 47 | 1 |
| test | test 路径、`#[cfg(test)] mod tests`，或仅 all-targets 出现的 library 诊断 | 1,893 | 1,636 | 1,366 |
| compatibility | `compat-fixtures/**`；字节兼容支持代码，不计生产风险下降 | 19 | 17 | 1 |
| fixture | `tests/fixtures/**` 可执行 fixture | 5 | 5 | 0 |
| generated | `target/` 或明确 generated 路径 | 0 | 0 | 0 |
| dependency | workspace 外的 span；Clippy 类别为 0 | 2 | 0 | 0 |

`dependency` 的 2 条是非选定 warning；脚本从 production 汇总中排除它们。
Rust JSON 诊断没有直接标出 span 是否位于 `#[cfg(test)]` item。脚本不猜源码
范围，而是把独立 `--lib` 和 `--bins` production run 的 location key 当作真值；
只在 all-targets 出现的 workspace span 才归 test。`--bins` raw 会重报其输入的
workspace library warning，所以只有去重、优先匹配 library 后的 classified
production-other 数字可用于风险结论。本次所有 production unwrap 和后续批次
位置还经过人工复核。

## 15 个 production-library unwrap 都已分类

| # | 位置 | 输入与可触发性 | 分类 / 当前 panic 后果 | 后续处置与聚焦证明 |
| ---: | --- | --- | --- | --- |
| 1 | `app-server/src/protocol_handler/permissions.rs:34` | wire rules 是外部输入，但同一分支先证明 `len() == 1` | 已验证不变量；仅代码漂移可使进程 panic | 改为 `let Some(rule)` 或带理由的 `expect`；`cargo test -p orbcode-app-server wire_to_core` |
| 2 | `config/src/config.rs:824` | model option 可来自设置，但 `is_none() || ...unwrap()` 短路证明为 `Some` | 已验证不变量；仅表达式重排会 panic | 用 `is_none_or`/pattern 消除 unwrap；`cargo test -p orbcode-config available_models` |
| 3 | `core/src/interaction_runtime.rs:148` | pending map 是内部状态；只受先前持锁 panic 的 poison 影响 | mutex poisoning；之后注册交互会 panic | 明确 recover-or-fail 策略；`cargo test -p orbcode-core interaction_runtime` |
| 4 | `core/src/interaction_runtime.rs:164` | 同上 | mutex poisoning；解析用户答复时 panic | 同 #3 |
| 5 | `core/src/interaction_runtime.rs:190` | request 已在同一锁 guard 下 `get` 并验证，期间无并发删除 | 锁内已验证不变量；代码漂移时 panic | `let Some` + internal error，保持非法答复仍 pending；interaction runtime tests |
| 6 | `core/src/interaction_runtime.rs:202` | 内部 pending map | mutex poisoning；取消单请求时 panic | 同 #3 |
| 7 | `core/src/interaction_runtime.rs:236` | 内部 pending map | mutex poisoning；批量取消时 panic | 同 #3 |
| 8 | `core/src/interaction_runtime.rs:256` | 内部 pending map | mutex poisoning；诊断读取时 panic | 同 #3 |
| 9 | `core/src/interaction_runtime.rs:260` | 内部 pending map | mutex poisoning；session pending 查询时 panic | 同 #3 |
| 10 | `mcp/src/registry/mod.rs:123` | stdio client slot；先前持锁 panic 才会 poison | mutex poisoning；MCP 调用 panic | 将 poison 映射为 `McpError` 或显式 recover；`cargo test -p orbcode-mcp registry` |
| 11 | `mcp/src/registry/mod.rs:133` | stdio lease return；同上 | mutex poisoning；Drop/return 路径 panic | 同 #10，并覆盖 client return 后可复用 |
| 12 | `mcp/src/registry/mod.rs:158` | HTTP client slot；同上 | mutex poisoning；MCP 调用 panic | 同 #10 |
| 13 | `mcp/src/registry/mod.rs:172` | HTTP lease return；同上 | mutex poisoning；Drop 路径 panic | 同 #10 |
| 14 | `tools/src/web_search.rs:418` | 外部 `%XX` URL 字节，但 match guard 已验证 high digit | 静态/分支证明；当前不会由坏 `%` escape 触发 | 匹配 `to_digit` 结果或局部 allow；`cargo test -p orbcode-tools url_decode` |
| 15 | `tools/src/web_search.rs:419` | 同 #14 的 low digit | 静态/分支证明 | 同 #14，保留 malformed escape 原样输出 |

额外的 production binary 点 `cli/src/args.rs:362` 来自
`CliPermissionMode::to_possible_value().unwrap()`；值由实现 `ValueEnum` 的 enum
自身产生，不是任意字符串。建议换成带 enum 不变量说明的 `expect`，并用
`cargo test -p orbcode permission_mode_conversion_round_trip` 证明所有变体。

结论：本批没有“未经验证的外部/持久化输入直接触发”的 unwrap，因此后续最高
优先级实现批次不应为了凑数量扩大到 test-only unwrap。

## 数值转换按领域归档

下表覆盖 181 个 production-library 数值 lint；同一表达式可在不同 lint 下各记
一次，因此这是 lint location 数，不是独立表达式数。

| 领域 | truncation | wrap | sign loss | precision loss | lossless | 合计 | 候选策略 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| TUI 坐标、终端尺寸、滚动 offset | 18 | 17 | 20 | 5 | 19 | 79 | 显示边界 clamp/saturate；无损处 `From` |
| buffer、文本和解析输入长度 | 19 | 0 | 0 | 3 | 0 | 22 | 外部长度 `try_from`，显示/preview 长度 saturate |
| token、usage、cost | 26 | 0 | 2 | 0 | 21 | 49 | provider 值 checked/saturating；内部 widen 用 `From` |
| 预算 | 0 | 0 | 0 | 0 | 0 | 0 | 当前选定 lint 无命中；继续保持 `u64` typed budget |
| timestamp、duration、日期窗口 | 6 | 11 | 3 | 1 | 4 | 25 | duration saturate，timestamp checked；禁止负值转 unsigned |
| 协议整数、frame/payload 长度 | 3 | 0 | 1 | 0 | 2 | 6 | wire 输入 checked；已知宽化用 `From` |
| **合计** | **72** | **28** | **26** | **9** | **46** | **181** |  |

领域映射的主要文件如下，完整行列由报告脚本输出：

- TUI：`tui/src/{custom_terminal,render/**,overlays/**,tui_theme}.rs`。
- buffer：`tools/src/{fs_text,glob_tool,grep_tool,notebook,payload,web_*}.rs`、
  `session-store/src/tool_results.rs`、`core/src/context/memory.rs`。
- token/usage/cost：`config/src/token_accounting.rs`、`core/src/model_cost.rs`、
  `core/src/session_manager/{mod,session_*}.rs`、`protocol/src/{cost,usage}.rs`、
  `model-provider/src/{http,stream}/**`、`tools/src/task_tools.rs`。
- time：`model-provider/src/rate_limit.rs`、`core/src/{hooks,hook_runner/command}.rs`、
  `core/src/overview.rs`、`mcp/src/{registry/mod,oauth/**}.rs`、
  `session-store/src/files/{gc,listing,session_index}.rs`、process/transport timeout。
- protocol：MCP WebSocket frame 长度和 `protocol/src/session.rs`。

## 58 个 production spawn 的 owner 与终止策略

表中一行可合并同一 owner、相同策略的相邻 spawn，但列出的每个行号都已计数。
“未观察”表示 JoinHandle panic/error 没有被 await/log；不等于已证明存在 bug。

| 位置（count） | owner | handle / 正常完成 / cancel-shutdown | error/panic | 结论 |
| --- | --- | --- | --- | --- |
| `app-server-client/src/child_stdio_transport.rs:245,254,261,266` (4) | child transport | 三个 I/O handle 交给 detached supervisor；child exit、fault 或 Drop shutdown 后 supervisor join | child 状态可观察；三个 JoinError 被忽略，supervisor panic 未观察 | 内层终止已证明；补 supervisor owner/JoinError 观察属于中优先级 |
| `app-server-client/src/in_process.rs:85,102` (2) | in-process transport | 两个 handle 存在 struct；Drop abort | task 返回 `()`，panic 未 log | owner 和 shutdown 已证明 |
| `app-server-client/src/ndjson_transport.rs:56` (1) | NDJSON transport | handle 存 struct，EOF 退出；Drop 只 detach | I/O/parse error 被折叠，panic 未观察 | 待确认：Drop 是否应 abort/await |
| `app-server-client/src/websocket_transport.rs:51,66` (2) | WebSocket transport | handles 存 struct；socket/channel close 退出，Drop detach | errors 多数结束 loop，panic 未观察 | 待确认：Drop owner |
| `app-server-client/src/lib.rs:475,485` (2) | AppClient routers | 显式 documented detach；transport channel close 退出 | send failure 使 route 结束，panic 未观察 | best-effort detach 可接受，建议结构化 log panic |
| `app-server-transport/src/stdio.rs:211,234` (2) | stdio connection | 局部 handles 由 `select!` await；另一侧 abort | `TransportError` 向 caller 返回 | await/abort 完整 |
| `app-server-transport/src/websocket.rs:216,239` (2) | WebSocket connection | 同上 | transport error 向 caller 返回 | await/abort 完整 |
| `app-server/src/message_processor.rs:375,422,500` (3) | connection subscriptions | 存 `active_subscriptions`；完成后 prune，processor Drop abort | 完成 handle 被 drop，panic 未观察 | cancellation 完整；应在 prune 时观察 JoinError |
| `app-server/src/message_processor.rs:588` (1) | ACP response resolver | 短任务 detached，一次 map remove/send 后结束 | panic 未观察 | 有意 best-effort detach |
| `app-server/src/message_processor.rs:700,756,870` (3) | permission/MCP trust/AskUser request | detached；response、timeout 或 sink close 后清 pending 并 deny/cancel | 业务错误转 fallback，panic 未观察 | 终止已证明；可统一监督/log |
| `app-server/src/background_api.rs:302` (1) | background subscription | broadcast/consumer close或 terminal 后退出 | 无 fallible return，panic 未观察 | 有意 detach |
| `app-server/src/background_api.rs:387` (1) | background job cancel bridge | 只在 cancel flag 为 true 时退出；handle 不保存 | 无 fallible return，panic 未观察 | **待修：正常 job 完成没有 shutdown signal，可能永久轮询** |
| `cli/src/main.rs:264,295` (2) | serve command | 局部 handle；ready oneshot 正常完成，server 返回后 abort | 无 fallible return | 完整 |
| `cli/src/headless.rs:1092` (1) | headless stdin control | detached；EOF/channel closure 驱动外层 loop | reader error 经 frame/closure 表达，panic 未观察 | 有意 detach |
| `cli/src/acp_sdk/mod.rs:431` (1) | ACP connection | detached pump；server-request rx close退出 | panic 未观察 | 有意 detach，pending state 负责请求取消 |
| `cli/src/acp_sdk/server_requests.rs:31,40,49` (3) | ACP request | 每请求 detached；请求完成/取消后退出 | fallback/日志处理业务错误，panic 未观察 | 有意 detach |
| `core/src/session_manager/mod.rs:1847` (1) | turn | detached driver；active-turn cancel flag；退出时清权限、interaction、active state | `run_turn_loop` 通过 stream 表达错误，panic 未观察 | owner 可追踪；建议监督 panic |
| `core/src/tool_runtime.rs:781` (1) | tool invocation | handle 返回 caller；ask channel drop 后 loop 退出 | `()`，panic 未观察 | 生命周期由 tool context 间接证明 |
| `core/src/tool_runtime.rs:829` (1/request) | AskUser timeout | detached 固定 sleep 300 秒；超时后 cancel no-op 或真实请求 | 无 fallible return | **待修：早答请求仍保留 task 300 秒；应由请求 guard cancel** |
| `core/src/session_manager/session_background_agent.rs:561,566` (2) | background agent | outer detached，以 task record/cancel flag 为 owner；inner forwarder await | loop result 写 terminal record，forwarder JoinError 忽略 | lifecycle 可观察；补 panic 状态 |
| `core/src/session_manager/session_goal.rs:285,291` (2) | persistent goal turn | outer supervisor detached；inner driver await；active-turn cancel | terminal/checkpoint error写 stream，driver JoinError 忽略 | 基本完整；补 JoinError 映射 |
| `core/src/session_manager/session_response.rs:485,488` (2) | streamed tool | outer handle 归 `StreamedToolUseExecution`，finish/interrupt/Drop 管理；inner watcher abort+await | outer JoinError 转 `CoreError` | 完整 |
| `core/src/session_manager/session_workflows.rs:499` (1) | workflow | detached；record/cancel flag/progress registry owner；退出 unregister | finalize error打印 | observe/log detach |
| `core/src/session_manager/session_workflows.rs:929` (1) | inline child agent | drain handle 在返回前 await | JoinError 忽略但无业务 error | 终止已证明 |
| `core/src/hook_runner/command.rs:116,154` (2) | hook process | best-effort stdin writers；process wait/timeout owns pipe close与 kill-on-drop | write error有意忽略 | 注释充分的 best-effort detach |
| `mcp/src/transport/stdio.rs:314` (1) | MCP runtime | stderr handle 存 client；shutdown timeout-await；Drop kill child使 pipe close | shutdown JoinError忽略，Drop 不 await | 正常 shutdown完整；Drop 路径待确认 |
| `tools/src/process.rs:25,30` (2) | tool process | 两个 reader await；cancel 时 abort | I/O/JoinError 向 `ToolError` 返回 | 完整 |
| `tools/src/bash.rs:99,107` (2) | Bash task | 两 reader在 terminal transition 前 await；process cancel/timeout kill | I/O/JoinError 向 `ToolError` 返回 | 完整 |
| `tools/src/skills.rs:205,217` (2) | MCP discovery | discovery handle直接 await，超时后转交 late monitor；late monitor await/log | JoinError被 log | observe/log 完整 |
| `tui/src/app.rs:130,373` (2) | statusline local command | detached，结果 channel + in-flight flag | 业务 error放 result；panic 未观察 | UI owner 明确，退出时 runtime 回收；可补监督 |
| `tui/src/app.rs:505` (1) | background task UI | consumer/terminal结束 | subscribe error直接结束，panic 未观察 | 有意 detach |
| `tui/src/commands/async_local.rs:303,323`; `tui/src/commands/tui_local.rs:303`; `tui/src/commands/compact.rs:59` (4) | local command | detached；完成事件回主 loop | 业务 error在 event 内，panic 未观察 | 有意 detach |
| **合计** |  |  |  | **58** |

优先级判断：先处理 `background_api.rs:387` 和 `tool_runtime.rs:829`；其余
“panic 未观察”应按 owner 分小批补齐，不应一次重构整个 async 生命周期。

## Error source 丢失候选

初始文本命令：

```sh
rg -n 'map_err\([^\n]*to_string\(\)' \
  core/src tools/src mcp/src app-server/src app-server-client/src \
  app-server-transport/src app-server-protocol/src
```

它返回 46 行：32 行 stringify 了捕获的 error，14 行只是把固定 timeout 或
channel 文案构造成 `String`，后者没有可保留的底层 source。扩展检查
`format!("...{error}")` 后，最高信号候选如下。

| 边界与位置 | 数量 | source / 泄密判断 | 建议 |
| --- | ---: | --- | --- |
| MCP reqwest：`oauth/browser.rs:313,330,376,378,382`; `oauth/discovery.rs:76,101,151,153,156,179,181,184`; `oauth/ssrf.rs:180`; `oauth/token.rs:81,102`; `transport/streamable_http.rs:39,120,177,215` | 20 | 内部应保留 `reqwest::Error`；Display 可能含 endpoint URL，但未见 header/body 被拼入 | 用现有 `McpError::http(error)`；增加 source 与 URL userinfo/query 不外泄测试 |
| MCP 其他：`registry/trust.rs:102`; `transport/websocket.rs:301` | 2 | trust 把 store error 塞入新 `io::Error`；TLS 把 tungstenite error 格式化 | 为各自 source 增 typed variant；保留当前公开文案 |
| core adapters：`retry.rs:368`; `session_manager/session_stream.rs:226,251,305` | 4 | 内部 config/progress error 被投影为 `String`/ToolError/ProviderError | 如果上层类型支持 source 则保留；否则标明这是 provider/tool 边界 |
| app/client/transport：`app-server/settings.rs:354`; `app-server-client/ssh_remote.rs:170`; `app-server-transport/websocket.rs:230` | 3 | 内部 source 丢失；输出最终会进入 app protocol/CLI | typed wrapper + wire 最后一跳 stringify |
| tools：`grep_tool.rs:517`; `interaction.rs:24,91`; `notebook.rs:318` | 4 | JoinError、validation、UTF-8 error 被投影为 ToolError 字符串 | parser validation 可视为 tool boundary；JoinError/UTF-8 应优先保留 source |
| 14 个 timeout/channel 固定文案 | 14 | 没有被丢弃的底层业务 source；`Elapsed` 信息不增加诊断价值 | 保持字符串化，避免误算成 source 修复收益 |
| 格式化的 app-client transport errors：`child_stdio_transport.rs:213`; `ndjson_transport.rs:35,45,112`; `websocket_transport.rs:35,41` | 6 | process/I/O/WebSocket source 丢失 | 后续 client error typed source 批次；不与 MCP 批次混合 |
| 格式化的 core tool/goal adapters：`hook_runner/command.rs:114`; `session_agent_tool.rs:147`; `session_background_agent.rs:506`; `session_goal_tools.rs:246`; `tool_flow.rs:149` | 5 | serde、persistence、JoinError 在内部提前变成文案 | 能在 crate 内继续传递的改 typed source；tool/wire 最后一跳再投影 |
| 有意 user-facing parser/tool 投影：`core/src/hooks.rs:277,389,431,479,521`; `tools/src/file_state.rs:153,166,169` | 8 | hook contract/工具输出需要字符串；serde 错误不含原 payload，file path 有意展示 | 保持边界投影；不要把 raw JSON/body 加入消息 |
| URL/HTTP tool 投影：`tools/src/web_fetch.rs:44,147`; `web_search.rs:212,255`; `web_search_adapters.rs:298` | 5 | `raw` URL 和 reqwest Display 可能带 userinfo/query；未见 header/body | 在 wire 前 redact credential/query；内部可保留 source |

OAuth response body 的服务端 `error_description` 是有意的用户诊断，但不得附加
token、authorization header 或完整 request body。本 Slice 没有发现把 header
或 request body直接格式化进上述 error 的代码；URL credential/query 仍需用
专门的负向测试证明。

## Nested option 与 permission string 边界

| 位置 | 三态/字符串语义 | owner 与存在理由 | 结论 |
| --- | --- | --- | --- |
| `session-store/src/transcript_schema.rs:20,126,341` | `PresentJsonValue = Option<Option<Value>>`：absent / null / value | transcript loader；`sessionEffort` 的 null 必须清掉先前 override | 明确兼容边界，保留 |
| `app-server-protocol/src/contracts.rs:92,140` | goal budget：absent 不改、null 清除、整数设置 | wire PATCH DTO，自定义 deserializer保留 field presence | 明确协议边界，保留 |
| `core/src/session_manager/session_goal.rs:29-30,646` | budget 与 stop reason 的 no-change / clear / set | goal mutation owner；进入 `SessionGoal` 前收敛为 typed `Option<T>` | 内部短距离三态合理；不继续向下扩散 |
| `tui/src/commands/goal.rs:31,35,283,335` | CLI 未提供 flag / `--no-budget` / `--budget N` | `/goal` parser，直接映射 wire 三态 | 明确 UI 输入边界，保留 |
| `tui/src/overlays/mod.rs:41` | picker 未提交 / automatic(default) / explicit effort | model picker owner，提交后转 typed override | 明确 selection 三态，保留 |
| `session-store/src/child_sessions.rs:43,66` | `permissionMode: Option<String>` | 历史 child metadata JSON；未知未来值需可读/可 round-trip | 持久化兼容边界，保留；读取后 parse typed |
| `protocol/src/background_task_view.rs:158` | `permission_mode: Option<String>` | background task view/JSON compatibility DTO | wire/view 边界，保留 |
| `config/src/agents.rs:320` | frontmatter parse 临时 `Option<String>` | 同函数内 trim + `PermissionMode::parse`，最终 `AgentDefinition` typed | 合理 parse staging，不是内部泄漏 |
| `core/.../session_{agent_tool,workflows}.rs:182,915` | typed `PermissionMode` 显式转 string | 仅在写 child-session persistence 时发生 | 边界正确 |
| `cli/src/stream_json.rs:135`; `protocol/src/control.rs:264` | 非 Option 的 compatibility string | stream-json/control wire byte contract | 最后一跳投影，保留 |

`RuntimeModelOverride` 已在 config 和 app-server-protocol 中用 enum 表达
Inherit/Default/Model，不再依赖 nested option；goal 持久化后的预算是
`Option<u64>`；effort 的 nested option 只存在 picker selection 边界。这些 owner
清晰，不应为消除 `Option<Option<T>>` 而破坏 absent/null/value 语义。

## 后续三个独立实现批次

### Batch 1：provider 外部数值输入采用 checked/saturating 转换

- 主要 crate：`orbcode-model-provider`。
- 文件：`model-provider/src/http/anthropic.rs:86,133`、
  `stream/anthropic.rs:137,162,180`、`stream/openai.rs:401,408,411,414`、
  `stream/openai_responses.rs:432,436,440,444`、
  `rate_limit.rs:135,138,151`。
- 处理：外部 usage/rate-limit 数值用 checked 或 saturating 转换；纯 widening
  用 `From`。先定义超过 `u32`/duration 上限时是 clamp 还是 typed error，并用
  provider fixture固定选择。
- 不处理：TUI 坐标、tools buffer、protocol DTO、15 个已证明 unwrap。
- 预期减少：该文件边界内的 `cast_possible_truncation`、`cast_sign_loss`、
  `cast_precision_loss`、`cast_lossless`；不设全局数量目标。
- 验证：

  ```sh
  cargo test -p orbcode-model-provider
  cargo clippy -p orbcode-model-provider --lib -- \
    -W clippy::cast_possible_truncation -W clippy::cast_sign_loss \
    -W clippy::cast_precision_loss -W clippy::cast_lossless
  ```

- 不变量：正常 provider usage 字段与 stream 累计值不变；超大/负数不能 wrap；
  provider request/response bytes 和公开 DTO 不变。

### Batch 2：AskUser timeout task 归请求生命周期所有

- 主要 crate/严格文件：`orbcode-core`，仅 `core/src/tool_runtime.rs:781,829`
  及同文件测试；必要的现有 `InteractionRuntime` API 调用不改公开 surface。
- 处理：让 timeout handle/取消 token 由 pending request 或 forwarder 所有；答复、
  delivery failure、tool cancel、turn cancel 都停止 timer；forwarder panic 被观察。
- 不处理：`background_api.rs:387`、goal/background workflow、transport/TUI detach。
- 预期减少：不以 lint 数量为目标；消除一个 correctness-critical unowned spawn
  family，并为 deliberate detach 留精确注释。
- 验证：

  ```sh
  cargo test -p orbcode-core interaction_runtime
  cargo test -p orbcode-core ask_user
  cargo test -p orbcode-core tool_runtime
  ```

- 不变量：同一 request id 只 resolve/cancel 一次；无效答复仍保持 pending；超时、
  interrupt、disconnect 的 `AskUserCancellationReason` 与 stream 顺序不变。

### Batch 3：MCP reqwest source 保留与无损 cast

- 主要 crate：`orbcode-mcp`。
- 文件：上表 20 个 reqwest 位置；机械 lint 仅限
  `mcp/src/registry/mod.rs:52`、`mcp/src/transport/websocket.rs:442,446`。
- 处理：`McpError::Http(error.to_string())` 改为现有 `McpError::http(error)`；为
  source chain、公开 Display 和 URL redaction 加测试；四个 widening cast 用
  `From`。若 RPC retry 依赖 `Http(String)` 文案匹配，先让
  `request_provably_not_delivered` 同时覆盖 `HttpWithSource`，不得改变 retry 判定。
- 不处理：timeout 固定文案、WebSocket tungstenite source、trust-store error、
  OAuth server response body、其他 crate 的 ClientError/ToolError。
- 预期减少：20 个 direct source-loss candidate 和 4 个 `cast_lossless`；不改变
  `missing_errors_doc` 等机械噪声。
- 验证：

  ```sh
  cargo test -p orbcode-mcp http_error_retains_reqwest_source
  cargo test -p orbcode-mcp request_provably_not_delivered
  cargo test -p orbcode-mcp oauth
  cargo test -p orbcode-mcp
  ```

- 不变量：MCP trust 与 allow rule 双重 gate 不变；token/header/body 不进入 Display；
  retry、timeout、OAuth 用户文案及 server config 兼容不变。

`app-server/src/background_api.rs:387` 是确认过的后续候选，但与 Batch 2 不同
crate/owner；为保持小批次边界，它排在前三批之后单独处理。

## lint allow 与代码改写规则

1. 外部输入、持久化数据、wire 数值或并发状态可触发 panic/wrap 时必须改代码，
   不允许用 `#[allow]` 隐藏。
2. `cast_lossless` 用 `From`/`Into`；TUI 展示边界用 clamp/saturate；协议和业务值
   用 `try_from` 或 typed error。只有同一作用域能证明上下界且转换语义必须保持
   bit-exact 时才允许局部 allow，并在旁边写出界限。
3. `unwrap_used` 只有编译期/static enum、同一分支已验证或锁内不可变不变量可用
   单表达式 `expect`/局部 allow；mutex poison 和外部输入不得靠 allow。
4. detached spawn 必须同时写明 owner、正常完成信号、cancel/shutdown、panic/error
   观察策略。缺一项就标“待确认”，不能仅用 `_handle` 压制 must-use。
5. 内部 error 优先 typed `#[source]`；只在 tool result、app protocol、CLI/wire 的
   最后一跳转字符串。任何投影都不得带 token、header、request body 或未脱敏 URL。
6. test、fixture、generated 和 compatibility warning 不计 production 风险下降；
   需要修时按自身可读性/fixture 稳定性立项，不能为了改善生产数字顺手改 golden。
7. 不增加 workspace/crate 级 `clippy::pedantic` allow，不追求本 Slice warning
   清零；`missing_errors_doc`、`must_use_candidate`、`doc_markdown` 只随有所有权的
   API 批次处理。

## 限制、稳健性与完成检查

- 行号绑定 `b9ed5da`；后续提交必须重新运行脚本，不可沿用本快照数字。
- Clippy warning 数会随工具链变化；版本是复现条件而非环境说明。
- spawn 审计证明了源码中可见的终止路径，但未做 scheduler/fault injection，故
  所有“panic 未观察”保持候选而非 bug 结论。
- source 泄密判断基于 Display 构造和数据流审阅；Batch 3 仍需用含 userinfo、
  query、伪 token/header/body 的负向测试完成证明。
- 本 Slice 只新增审计脚本和本报告，没有改 `.rs`、协议 DTO、fixture、golden、
  transcript、CLI 输出或公开 API。

2026-08-15 在 Slice 0 worktree 中的完成验证：

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all --check` | 通过 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 |
| `cargo check --workspace` | 通过 |
| `cargo test --workspace` | 通过 |
| `scripts/audit-public-surface.sh` | 通过 |
| `scripts/audit-brand.sh` | 通过 |
| `git diff --check` | 通过 |
| `scripts/check-docs.sh` | 通过 |
| 报告脚本在干净 `b9ed5da` checkout 上重跑 | 通过；raw/classified 数字与本文一致 |

标准完成检查命令：

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
cargo test --workspace
scripts/audit-public-surface.sh
scripts/audit-brand.sh
git diff --check
```
