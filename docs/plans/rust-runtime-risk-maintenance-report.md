# Rust 运行时风险维护完成报告

> 历史快照说明（2026-08-23）：本文中的“当前”数字和行号描述的是
> `abcc9b581e55c7e0d710391bf66546ddde5012e6` 完成态，不再代表 `main` 当前头。
> `daa1f041ca5ebf2c2c6a5d7ff6dd1e27394e22a3` 的权威数字、位置和 P3
> 交接清单见 [`rust-maintenance-current-head-inventory.md`](rust-maintenance-current-head-inventory.md)。

## 结论

本轮已按 Slice 0 的权威清单处理确认的高信号运行时风险，未改变协议 DTO、
stream-json/transcript bytes、CLI/TUI 用户输出或公开 API。实现基于
`130b1eb`（包含 Slice 0 报告），位于 `maintenance/runtime-risk` 分支；五个实现
提交分别覆盖 provider 数值边界、AskUser task 生命周期、MCP HTTP source、
background cancel supervisor 和 production unwrap。

重新运行 `scripts/rust-maintenance-report.py` 后，production-library
`unwrap_used` 从 15 降至 0，production binary 从 1 降至 0。测试、fixture 和
compatibility 中的 unwrap 没有为了改善指标而清理。pedantic 总量不是本任务的
目标，仍保留在本次文件边界之外的低风险和 API 文档类 warning。

## 结果摘要

| production 分类 | Slice 0 基线 | 本轮实现后 | 变化 |
| --- | ---: | ---: | ---: |
| library 全部 warning | 1,799 | 1,761 | -38 |
| library 选定 warning | 1,484 | 1,446 | -38 |
| library `unwrap_used` | 15 | 0 | -15 |
| binary 选定 warning | 47 | 46 | -1 |
| binary `unwrap_used` | 1 | 0 | -1 |
| `cast_possible_truncation` | 72 | 56 | -16 |
| `cast_lossless` | 46 | 41 | -5 |
| `cast_sign_loss` | 26 | 25 | -1 |
| `cast_precision_loss` | 9 | 8 | -1 |
| `cast_possible_wrap` | 28 | 28 | 0 |

计数使用 Slice 0 同一工具链和同一分类脚本：Rust/Clippy 1.97.1，按 lint、crate、
文件、行、列去重。本文完成时曾把 all-target `unwrap_used` 记为 1,367 个 test
和 1 个 compatibility 命中；P3-01 在同一 `abcc9b5` commit、同一工具链上的
可复现重跑校正为 **1,366 个 test + 1 个 compatibility = 1,367 个 all-target**。
这不改变 production library/binary 均为 0 的结论，也不把这些非生产命中纳入
风险下降口径。

## 已处理批次

### 1. Provider 数值转换

提交：`fa8ac17 Harden provider numeric conversions`

- Anthropic/Bedrock token count 和 Anthropic stream block index 从 `u64` 转
  `usize` 时饱和到平台上限，避免 32 位目标截断。
- OpenAI Chat Completions 与 Responses 的 usage 从 `u64` 转 `u32` 时饱和到
  `u32::MAX`；缺失 total 的组件聚合使用 `saturating_add`。
- `Retry-After`、reset timestamp 和 jitter 聚合使用饱和运算；重试指数在转换前
  已 clamp 到 32，并以具体 `expect` 记录不变量。
- jitter 的 `NaN` 明确按 0 处理，正常值 clamp 到 `0.0..=1.0`；无损整数转浮点
  使用 `From`。

这里选择 saturation 是因为这些字段用于计量、展示和退避上界：wrap 会产生更小、
更宽松的值，而 saturation 保持单调性且不改变正常 provider 数据。新增测试覆盖
`u64::MAX`、组件相加溢出、极端 duration、reset timestamp 和 `NaN`。

### 2. AskUser timeout task 所有权

提交：`1d936cd Own AskUser timeout tasks`

- 每个 pending request 由 `JoinSet` 监督一个 response/receiver-close/timeout
  竞速任务；早答、tool/turn cancel、receiver drop 和 timeout 都会结束该任务。
- request guard 在异常退出时清除 pending interaction，避免 300 秒孤儿 timer。
- 外层 AskUser forwarder 在 tool invocation 收尾时被 await；request task 和
  forwarder 的 `JoinError` 都映射为可观察的 `CoreError`。
- 保持 request id 单次 resolve、非法答复继续 pending、取消原因和 stream 顺序。

确定性测试覆盖早答停止 timer、timeout 单次取消、receiver drop、request task
panic 和 forwarder panic。

### 3. MCP HTTP source 与 URL 脱敏

提交：`d3f9d44 Preserve MCP HTTP error sources`

- Slice 0 列出的 20 个 `reqwest::Error` 转换统一进入现有
  `McpError::HttpWithSource`，`Error::source()` 不再在内部 MCP 边界丢失。
- source 存储前调用 `without_url()`；兼容 Display 使用移除 userinfo、query 和
  fragment 的 URL。Display 和 Debug 均有 canary 负向测试。
- RPC “请求可证明尚未送达”的 retry 判断同时识别 typed reqwest connect error，
  不依赖新 variant 的字符串文案。
- 同文件边界内四个 widening cast 改用 `From`。

保留 legacy `McpError::Http(String)` 以维持现有 public enum surface 和已有调用者；
新 reqwest 路径使用 sourced variant。未把 `reqwest` 类型暴露到更高层 crate。

### 4. Background cancel supervisor 终止

提交：`f064aa0 Stop background cancel supervisors`

- 返回给 caller 的 receiver 现在拥有一个 event proxy supervisor；它逐事件转发
  source stream，并在 terminal event、source close、consumer close 或转发取消后
  退出。
- cancel token 以 50 ms 有界间隔轮询并跳过 missed ticks；`cancel_turn` 最多转发
  一次。
- 正常成功路径测试使用 `Arc` 生命周期 canary，证明 terminal 后 supervisor
  释放 owner；取消 E2E 继续证明 in-flight tool 收到取消。

该任务没有 fallible 返回值，owner、正常完成信号和取消边界现在都写在调用点。

### 5. Production unwrap 与 panic

提交：`a15886b Eliminate production unwrap panics`

| 位置组 | disposition |
| --- | --- |
| app-server permission wire | 单规则用 `let Some` 安全提取；非法输入继续返回 `InvalidParams` |
| config model option | 用 `Option::is_none_or` 表达短路条件 |
| core interaction pending map | 统一恢复 poisoned mutex；锁内 remove 漂移返回 `UnknownRequest` |
| MCP stdio/HTTP client slot | 统一恢复 replaceable slot；中断 take 由既有 typed “client unavailable” 路径报告 |
| tools DDG `%XX` | 显式十六进制 parser；非法/不完整 escape 原样保留 |
| CLI clap enum | 保留 enum 静态不变量，并换成说明依据的 `expect` |

core 和 MCP 均新增可重复的 mutex poison 测试；tools 增加非法 `%zz` 和尾部 `%`
边界。最终 production library/binary `unwrap_used` 均为 0。

## Production spawn 完成性审计

Slice 0 已逐项记录 58 个 production spawn 的 owner、正常完成、cancel/shutdown、
error/panic 观察和最终策略。本轮重新核对该表后，两个被标为最高信号的 family
已经修复：

| family | owner | 正常结束 / cancel | error 或 panic 观察 |
| --- | --- | --- | --- |
| AskUser request timer/forwarder | tool invocation + per-request `JoinSet` | response、receiver close、timeout、tool/turn 结束 | `JoinError` 转 `CoreError`，outer forwarder 被 await |
| background cancel bridge | 返回的 event receiver | terminal、source/consumer close；token 触发一次 `cancel_turn` | task 无 fallible return；E2E 验证 owner 释放 |

其余 spawn 保留 Slice 0 已记录的现有策略：直接 await/abort 的 transport/process
任务、由 struct handle/Drop 拥有的任务、由 channel/EOF/terminal 收敛的有意 detach，
以及有界且不会写 retired state 的 drain。表中“panic 未观察”的中优先级候选没有
被误报为本轮已修复；它们需要按各自 owner 分批，而不是引入第二套全局 supervisor。

## 保留项及理由

- TUI 坐标、tools buffer、protocol DTO 和本轮文件外的数值 lint：Slice 0 将其
  分类为不同领域；未经独立边界测试不做批量 cast 重写。
- MCP trust-store 和 WebSocket TLS source、其他 crate 的 ClientError/ToolError：
  权威 Batch 3 明确排除。tools parser/validation 的字符串是 tool-result 最后一跳；
  JoinError/UTF-8 等内部 source 候选应另立局部 typed-error 批次。
- timeout/channel 固定文案：没有可保留的底层业务 source。
- test、fixture、golden 和 compatibility warning：保持字节与夹具稳定，不为指标
  清理。
- `missing_errors_doc`、`must_use_candidate`、`doc_markdown` 等 pedantic warning：
  不属于运行时风险目标，因此不宣称 pedantic 清零。

这些保留项都没有新增 allow，也没有扩大公开签名或跨 crate 暴露实现错误类型。

## 提交与验证

实现提交顺序如下，每个提交均可独立审阅：

1. `fa8ac17 Harden provider numeric conversions`
2. `1d936cd Own AskUser timeout tasks`
3. `d3f9d44 Preserve MCP HTTP error sources`
4. `f064aa0 Stop background cancel supervisors`
5. `a15886b Eliminate production unwrap panics`

批次开发期间已通过相应 crate 的聚焦/全量测试、`cargo fmt --all --check`、
workspace Clippy `-D warnings`、`cargo check --workspace` 和 `git diff --check`。
最终门禁结果：

| 检查 | 结果 |
| --- | --- |
| `cargo test --workspace` | 通过；仅仓库既有环境依赖/压力测试保持 ignored |
| `scripts/audit-public-surface.sh` | 通过 |
| `scripts/audit-brand.sh` | 通过 |
| `scripts/check.sh` | 通过；包含 docs、fmt、Clippy、check、public surface、brand 和 tests |
| `scripts/check-docs.sh` | 通过 |
| `git diff --check` | 通过 |
| `scripts/rust-maintenance-report.py` | 通过；production lib/bin unwrap 均为 0 |
