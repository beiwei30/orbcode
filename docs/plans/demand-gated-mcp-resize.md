# MCP Streaming、ACP SSE 与 Resize 需求门控处置记录

## 结论

本记录在 `abcc9b581e55c7e0d710391bf66546ddde5012e6` 基线上，于
2026-08-17 重验 MCP Streamable HTTP、ACP MCP transport 和 TUI resize。仓库内没有
命名的受支持 server/client 失败 fixture，本次任务也没有收到计划要求的外部需求证据。因此
没有方向达到运行时实现门槛；新增 reader、legacy transport、resize trace 或 follow-up task
都不符合本任务的完成定义。

| 方向 | 处置 | 本次证据 | 重新打开条件 |
| --- | --- | --- | --- |
| 2025 standalone GET | Deferred | 当前只有 POST request/response；仓库无 GET reader、`Last-Event-ID` 或必要 workflow fixture。 | 提供 server 名称/版本、协商的 2025-era 版本、用户可见缺失 workflow、POST SSE 无法满足的证明、最小失败 fixture 和 405 降级预期。 |
| 2026 modern subscription | Deferred to separate plan | 当前 client 提出 `2024-11-05`；没有 discovery/stateless/subscription client 需求或 era migration 批准。 | 命名受支持 server/client、证明其要求 2026 era，并批准独立 migration 计划；计划必须覆盖 discovery、metadata/header、stateless、多 round-trip、listen/subscription 和 2025 compatibility。 |
| legacy ACP SSE | Rejected/Closed | ACP 只声明 HTTP；`McpServer::Http` 映射 Streamable HTTP，`McpServer::Sse` 在 session setup 前 typed reject。 | 同时给出 ACP host 与 MCP server 的名称/版本、必要 workflow、host 只能给出 SSE 且 HTTP 无法替代的证明、维护 owner 和复审/移除日期。 |
| resize debounce | Keep 150 ms / Closed | source-backed rebuild、deadline replacement、streaming/idle 和 frame fixtures 通过；没有支持终端上的可重复缺陷。 | 在命名的 terminal/tmux/OS 上提供可重复 trace，证明 rebuild 过多、最终尺寸遗漏或可见 corruption；再比较 150 ms 与候选值。 |
| post-reflow follow-up | Deferred | settle 后没有已知可重复的晚到最终尺寸；当前也没有 probe/task。 | 在支持终端上证明 settled rebuild 仍使用旧尺寸，并提供终端身份、事件时间、observed/rebuilt/final size 与视觉结果。 |

“没有需求证据”只表示本次输入和仓库 fixture 没有达到上述门槛，不是对所有外部 MCP
server、ACP host 或 terminal 的普遍性声明。

## A0：Streamable HTTP 当前状态

| 核验项 | `abcc9b5` 当前行为 | 权威证据 |
| --- | --- | --- |
| initialize proposal | 固定提出 `2024-11-05`。 | `mcp/src/transport/streamable_http.rs::initialize` |
| server-selected version | 保存 initialize result 的 `protocolVersion`，后续 POST 与 DELETE 使用 `mcp-protocol-version` header。 | `StreamableHttpMcpClient::initialize`、`protocol_headers`；`streamable_http_reuses_session_and_protocol_header` |
| session id | initialize 前清空旧值；response header 可更新 session；后续 POST 带 session header；shutdown `take` 后 DELETE。 | `request`、`shutdown`；`streamable_http_reuses_session_and_protocol_header`、两个 DELETE tests |
| POST JSON response | 支持 `application/json` 并按 JSON-RPC id/result 解析。 | `request`、`parse_http_json_rpc_response`；Streamable HTTP registry tests |
| POST SSE response | 支持有限 `text/event-stream` response body；跳过 notification/错误 id，选择 matching response。 | `parse_sse_skips_notifications_and_finds_matching_response`、`diagnose_http_server_sends_static_headers_and_parses_sse`、runtime fault fixture |
| expiry/retry | sessionful 404 标记 expiry；只对 idempotent method 重新 initialize 并重试一次，非幂等 call 不会因 expiry 重放。 | `McpRegistry::http_rpc`；`streamable_http_reinitializes_once_after_session_expiry_for_list` |
| shutdown/cleanup | 有 session 时 DELETE；404/405/501 是正常关闭；remove、session cleanup、policy prune、reload remove/restart 都释放 client。 | `shutdown`、registry cleanup call sites；`streamable_http_shutdown_sends_delete_on_remove`、`streamable_http_shutdown_sends_delete_on_reload_remove` |
| standalone GET / notification routing | 不存在 retained GET task、reader handle、`Last-Event-ID`、notification channel 或 subscription/listen runtime。 | 对 `mcp/`、`cli/`、`app-server/` 的符号搜索；现有 HTTP client 只在 OAuth discovery 使用 GET，transport RPC 使用 POST。 |

这不是 2025 或 2026 多 era client。当前只需要一个协议字符串，不应在没有第二个 accepted
era runtime 前加入空的 `McpProtocolEra` 抽象。若上述任一新 era 达到门槛，typed era
representation 必须先于 runtime 分支落地。

## B0：ACP legacy SSE 关闭证据

- `cli/src/acp_sdk/capabilities.rs` 只构造 `.http(true)`；unit test 与 official SDK/raw-process
  E2E 同时固定 `http=true`、`sse=false` 和 MCP-over-ACP omission。
- `cli/src/acp_sdk/mcp_setup.rs` 把 `McpServer::Http` 映射成
  `McpTransport::StreamableHttp`，把 `McpServer::Sse` 返回为 `-32602` invalid params；没有
  alias 到一般配置中的 legacy `sse` compatibility hint。
- new/load/resume 都在 bootstrap/preflight 前调用同一个完整列表转换函数。混合列表遇到
  unsupported transport 会整体返回错误；不会把已经转换的前缀提交到 registry。
- raw-process E2E 固定 session/new、load、resume rejection；新增的原子性断言证明 rejected
  session/new 不产生 session 或 `mcp/servers.json`。HTTP overlay 的 new/load/resume tests
  证明 supported path 不持久化。
- `cleanup_all_sessions` 覆盖 EOF/transport error，`session/close` 覆盖显式关闭；二者最终通过
  app-server session cleanup 移除 overlay。`docs/acp-support.md` 及中文版本明确只支持 stdio 与
  Streamable HTTP，并把 legacy SSE 记录为 rejected/unadvertised；Zed smoke checklist 也把
  SSE typed failure 作为预期结果。

正式处置：**Rejected/Closed：当前产品不支持 legacy ACP SSE；Streamable HTTP 通过
`McpServer::Http` 提供。**

## C0：Resize 当前状态

- `tui/src/app.rs` 对每个 observed resize 重新赋值唯一
  `resize_settle_deadline = now + resize_settle`；`tokio::select!` 只消费当前 deadline，消费前
  先清空它，然后仅在 `reflow_pending` 时执行一次 source-backed rebuild。
- 默认值是 150 ms；`ORBCODE_TUI_RESIZE_SETTLE_MS` 的合法 `u64` 覆盖默认值，missing/非法值
  回退 150 ms。纯函数测试固定默认和解析行为，但不把 75 ms 变成新默认。
- render/frame tests 覆盖 width、height、streaming、idle、turn completion、history purge、
  stale live rows、blank gap 与 pager pause。settle rebuild 仍调用
  `rebuild_committed_history_from_source`，没有第二套 renderer、post-reflow probe 或后台 task。
- Darwin 25.6.0 arm64、tmux 3.7b 上的 ignored PTY E2E 和 final-answer tmux smoke 通过；这次
  没有复现 resize 缺陷，也没有声称执行 C1/C4 的新测量矩阵。

正式处置：**debounce 保持 150 ms；post-reflow follow-up Deferred。** 诊断 override 保留，
但没有测量证据时不 clamp、调低默认值或增加 follow-up。

## 条件验收项适用性

本次没有 Accepted 方向，因此 A3、B2、C1-C4 的实现验收项没有被触发：没有新增 SSE
parser、reader task、notification channel、legacy endpoint validation、resize trace 或 follow-up
probe。也因此没有新增 lifecycle/threat surface 可以用窄测试替代验收。若任一方向以后满足 reopen
条件，必须从该方向的最小失败 fixture 重新开始，并一次性交付计划中列出的有界 parsing、secret
redaction、owner/cancellation/shutdown、focused E2E 和文档，不得把本记录当作这些条件实现的
预先批准。

## 验证记录

环境：Rust/Cargo 1.97.1，Darwin 25.6.0 arm64，tmux 3.7b。

计划列出的聚焦命令结果：

| 命令 | 结果 |
| --- | --- |
| `cargo test -p orbcode-mcp streamable_http` | PASS；10 passed |
| `cargo test -p orbcode-mcp parse_sse_skips_notifications_and_finds_matching_response` | PASS；1 passed（额外 correlation 核验） |
| `cargo test -p orbcode-mcp runtime_fault` | PASS；18 passed |
| `cargo test -p orbcode-mcp oauth` | PASS；25 passed |
| `cargo test -p orbcode-app-server mcp` | PASS；36 unit + 2 integration passed |
| `cargo test -p orbcode --test acp_server_request_e2e acp_session_mcp` | PASS；8 passed |
| `cargo test -p orbcode --test acp_server_request_e2e acp_initialize` | PASS；1 passed |
| `cargo test -p orbcode-tui resize` | PASS；13 passed，1 manual stress ignored |
| `cargo test -p orbcode-tui frame_capture` | PASS；59 passed |
| `cargo test -p orbcode --test tui_remote_pty_e2e -- --ignored` | PASS；3 passed |
| `scripts/tui-native-scrollback-tmux-smoke.sh --final-answer-smoke` | PASS |

最终门禁结果：

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo check --workspace` | PASS |
| `cargo test --workspace` | PASS；完整重跑全部通过，环境/网络/压力类既有 tests 保持 ignored |
| `scripts/audit-public-surface.sh` | PASS |
| `scripts/audit-brand.sh` | PASS |
| `scripts/check-docs.sh` | PASS |
| `git diff --check`（含新记录的 no-index check） | PASS |

第一次 workspace 全量运行中，负载敏感的
`canonical_initialize_works_through_app_client` 曾返回一次失败 diagnostics；该测试没有触及本次
文件。它随后孤立复跑通过，第二次完整 `cargo test --workspace` 也通过，最终结果以上表为准。
