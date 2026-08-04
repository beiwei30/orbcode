# ChatGPT/Codex 订阅登录支持计划

状态：Phase 1–5 的核心链路、doctor 订阅探测、远程 app-server 登录协议、
无公网测试和真实账户 smoke 已完成；完整 fake OAuth CLI e2e 仍待补充。

工作分支：`codex/chatgpt-subscription-auth`

基线与参考源码：

- Orbcode `053e86ddb6082d8f8041294b6a1b7e927d0e5d09`
- OpenCode `1882c33827cf0ce5c948b69ab5a87ed8f6790cf8`（`dev`）
- OpenAI Codex `bb5054fe47abe73ecbbd454751066a28c89f4bb9`（`main`）

## 目标

让用户可以通过 ChatGPT/Codex 订阅登录 Orbcode，并使用订阅账户的 Codex 模型完成文本、推理和工具调用回合，同时继续完整支持现有 OpenAI API Key 与 OpenAI-compatible 服务。

目标用户流程：

```text
orbcode auth login --provider openai --method chatgpt
# 浏览器登录；无图形环境可增加 --device-code

orbcode auth status
cargo run -p orbcode -- prompt "回复 OK"
```

## 非目标

- 不自动读取、修改或共享 `~/.codex/auth.json`。两个进程共享可轮换的 refresh token 会产生竞态和互相登出风险。
- 不把 ChatGPT 订阅伪装成 OpenAI API 余额，也不承诺订阅模型与 API 模型目录完全一致。
- 第一版不实现 WebSocket Responses、远程模型目录缓存、账户用量面板或多 ChatGPT 账户切换。
- 不改变 Anthropic OAuth、OpenAI API Key 或自定义 `OPENAI_BASE_URL` 的既有行为。

## 已确认的现状

Orbcode 当前只有 OpenAI API Key + Chat Completions：

- `config/src/auth.rs` 能保存 API key 或一个无结构的 token，但没有 OpenAI OAuth token 集合、过期时间、账户 ID 或刷新流程。
- `core/src/config_provider.rs` 对 OpenAI 只设置 `api_key`，明确清空 `auth_token`。
- `model-provider/src/http/openai.rs` 固定请求 `{base_url}/chat/completions`。
- `model-provider/src/request/openai.rs` 和 `model-provider/src/stream/openai.rs` 只实现 Chat Completions 请求及 SSE。
- 默认 OpenAI 模型仍是 `gpt-4o`/`gpt-4o-mini`，不能直接作为 Codex 订阅通道默认值。

参考实现已经确认以下协议：

- 官方 Codex 和 OpenCode 使用同一 OAuth client id：`app_EMoamEEZ73f0CkXaXp7hrann`。
- 浏览器登录使用 Authorization Code + PKCE(S256) + state，默认回调端口为 1455；官方 Codex 另有 1457 回退端口。
- 无头登录使用 OpenAI 的 Codex device flow，最终仍交换出 access、refresh 和 ID tokens。
- ChatGPT 推理请求使用 `https://chatgpt.com/backend-api/codex/responses`，并发送 Bearer token 与 `ChatGPT-Account-ID`。
- Responses 请求必须使用 Responses wire protocol；不能只把现有 Chat Completions JSON 改一个 URL。
- `store: false` 的多轮推理需要请求 `reasoning.encrypted_content`，并在后续回合重放加密推理状态。

## 架构决策

### 1. 认证方式与 wire protocol 分离

同一个 `ProviderId::OpenAi` 支持两条明确路径：

| 认证 | Wire protocol | Endpoint | Base URL override |
| --- | --- | --- | --- |
| API key | Chat Completions | `OPENAI_BASE_URL` + `/chat/completions` | 保持支持 |
| ChatGPT OAuth | Responses | 固定 Codex backend + `/responses` | 禁止覆盖 |

不得仅根据 URL 或 token 字符串猜测模式。provider request 应携带显式的 OpenAI wire mode 和认证类型。

### 2. 认证所有权仍属于 `config`

在 `config` 中增加独立的 OpenAI OAuth 模块，由 `AuthManager` 负责：

- OAuth 启动、token 交换、token 刷新与本地注销。
- 结构化保存 `id_token`、`access_token`、`refresh_token`、`expires_at`、`account_id`、可选 `email`/`plan_type`。
- 在 access token 到期前 5 分钟刷新。
- 使用共享 async mutex 对刷新做 single-flight；refresh token 轮换时禁止并发重复使用旧 token。
- 原子写入认证文件；Unix 权限保持 `0600`；日志、错误和 `auth status` 不输出任何 token。

`AuthManager` 的 clone 必须共享刷新锁和当前凭证快照。`SessionManager` 获得同一实例，以便每次 provider attempt 前解析新凭证，并在 401 后执行一次受控恢复。

### 3. 保持认证优先级兼容

OpenAI 认证优先级暂定：

1. 环境或 settings 中显式的 `ORBCODE_OPENAI_API_KEY`/`OPENAI_API_KEY`
2. `orbcode auth login` 保存的 OpenAI API key
3. 保存的 ChatGPT OAuth 凭证

这保持现有环境变量行为不变。若 API key 遮蔽了 OAuth，`auth status` 必须显示 OAuth 为 ready 但 inactive，并给出明确提示。

### 4. 不向自定义端点泄露 ChatGPT token

当认证类型为 ChatGPT OAuth 时：

- 忽略 OpenAI-compatible `OPENAI_BASE_URL`，强制使用 Codex backend。
- 自定义 header 不能覆盖 `Authorization` 或 `ChatGPT-Account-ID`。
- 重定向策略不得把认证 header 带到其他 host。
- debug snapshot 只记录认证类型，不记录 token、JWT claims 或完整认证 URL。

### 5. Responses 状态映射复用现有流事件

新增 Responses request builder、HTTP sender 和 SSE adapter，但输出仍映射到现有 `ProviderStreamEvent`：

- `response.output_text.delta` → Text delta
- `response.reasoning_summary_text.delta` → Thinking delta
- reasoning item 的 `encrypted_content` → Thinking block 的 provider-opaque signature
- `response.function_call_arguments.delta` 和 function-call item → ToolUse
- `function_call_output` → 下一次请求中的 tool output
- `response.completed` → usage、stop reason、MessageStop
- `response.failed`/`response.incomplete`/`error` → 现有 provider error 分类

第一版复用 `TranscriptBlock::Thinking.signature` 保存 OpenAI 加密推理状态，不修改 JSONL transcript schema。OpenAI 与 Anthropic 互相 fallback 时继续剥离 thinking blocks，避免跨 provider 重放不兼容的 opaque 数据。

### 6. 模型与计费语义

- ChatGPT OAuth 且用户没有显式模型设置时，第一版默认 `gpt-5.6-sol`；该选择来自固定版本的本地 Codex 模型目录，后续应由服务端模型目录替代。
- 显式 `ORBCODE_OPENAI_MODEL` 继续优先，但订阅端点拒绝模型时给出“账户/套餐/模型可用性”提示。
- 补充 `gpt-5.6-sol` 的 effort、上下文和最大输出能力，避免沿用 `gpt-4o` 默认能力。
- 订阅回合继续统计 token，但不得套用 API 美元单价或触发 `maxBudgetUsd`。成本视图应显示 `subscription`/`not API-priced`，而不是伪造 `$0` 或按未知模型 fallback 价格累计。

## 分阶段实施

### Phase 0：协议可行性 spike

目标：用最少代码确认真实账户端到端路径，结果可丢弃，不进入正式架构。

- 使用固定 OAuth client id、PKCE 和 localhost:1455/1457 完成一次浏览器登录。
- 确认 `originator=orbcode` 可接受，并记录实际 token claims 的字段存在性，不记录字段值。
- 使用 access token + account id 向 Codex Responses endpoint 发送最小 `store:false` 请求。
- 验证 `gpt-5.6-sol`、纯文本回复和一次 function call。
- 若 OAuth client、redirect allowlist 或第三方 originator 被拒绝，在继续实现前停止并记录 blocker。

验收：独立 spike 可完成“回复 OK”，并能证明一次工具调用的 Responses 事件形状。

### Phase 1：结构化认证存储与刷新

主要文件：

- `config/src/auth.rs`
- 新增 `config/src/openai_oauth.rs`（名称可在实现时按模块边界微调）
- `config/Cargo.toml`
- `app-server/src/auth_api.rs`

任务：

- 增加 `AuthMethod::ChatGpt`，保持旧 serde/wire 值兼容。
- 增加结构化 ChatGPT credential source 和脱敏 status metadata。
- 实现 JWT claim 的容错解析，仅用于 account/plan/expiry metadata；不得将未验证 claim 当作授权证明。
- 实现 proactive refresh、rotated refresh token 持久化、single-flight 和永久 refresh failure 分类。
- 注销至少删除本地凭证；token revoke 作为同阶段的优先任务，若服务不可用也必须完成本地注销。
- 为 issuer、client id 和 backend URL 提供 test-only 注入点，生产设置中不暴露任意 endpoint 覆盖。

验收：配置单元测试覆盖旧 auth.json 读取、结构化凭证 round-trip、`0600`、过期刷新、并发刷新去重和无 secret 输出。

### Phase 2：浏览器与 device-code 登录 UX

主要文件：

- `cli/src/args.rs`
- `cli/src/commands/mod.rs`
- `app-server/src/auth_api.rs`
- 必要时扩展 `app-server-protocol` 与 `app-server-client`

任务：

- 新增 `--method chatgpt`，默认浏览器流程；增加 `--device-code`。
- 浏览器流程实现随机 state、PKCE S256、localhost callback、5 分钟超时、1455→1457 端口回退和浏览器打开失败时打印 URL。
- device flow 展示 verification URL/user code，按服务端 interval 轮询，15 分钟超时并支持 Ctrl-C 取消。
- 登录开始与完成分离，确保 URL/code 能在等待期间立即展示；不要让一个不返回信息的长 RPC 阻塞用户。
- `auth status` 显示 `chatgpt`、plan、expiry、active/shadowed；`auth logout --provider openai` 清理对应凭证。
- managed `forceLoginMethod` 保持 Anthropic 兼容；OpenAI 的 ChatGPT 策略不能误用 `claudeai` 字符串静默放行。

验收：使用本地假 OAuth server 的 CLI e2e 覆盖成功、state mismatch、拒绝、超时、取消和 headless 流程；测试不得打开真实浏览器或访问公网。

### Phase 3：OpenAI Responses provider

主要文件：

- `model-provider/src/types.rs`
- `model-provider/src/request/` 下新增 Responses builder
- `model-provider/src/http/` 下新增 Responses sender
- `model-provider/src/stream/` 下新增 Responses SSE adapter
- `model-provider/src/adapters/openai.rs`

任务：

- 为 OpenAI request 增加显式 wire mode；API key 默认保持 Chat Completions。
- 构建 Responses `instructions`、`input`、function tools、tool outputs、reasoning effort、`store:false`、`stream:true` 和 `include:["reasoning.encrypted_content"]`。
- 请求 ChatGPT backend 时注入 Bearer、account id 和必要 session headers。
- 解析文本、reasoning、function call、usage、completed/failed/incomplete 事件。
- 要求流必须以 `response.completed` 结束；提前断流进入现有可重试网络/stream 错误路径。
- Chat Completions 的所有 request/stream golden 保持不变。

验收：wiremock 测试锁定 URL、headers、请求 JSON、SSE 顺序、tool call 拼接、encrypted reasoning round-trip 和错误分类。

### Phase 4：core 集成、401 恢复与 fallback

主要文件：

- `core/src/config_provider.rs`
- `core/src/retry.rs`
- `core/src/session_manager/`
- `app-server/src/lib.rs`

任务：

- 将 provider request 准备改为可异步解析/刷新认证。
- AppServer 与 SessionManager 共享同一 `AuthManager`。
- 每个 attempt 前获取当前 token；401 时按“reload 当前账户 → refresh 一次 → retry 一次”恢复，禁止无限认证重试。
- 403 entitlement/workspace、429 subscription limit、`insufficient_quota` 和 `usage_not_included` 分别给出可操作提示。
- 确保 primary/fallback 切换重新解析目标 provider 的 wire mode 和认证，并继续遵守“已有内容不盲目重试”的规则。
- compaction、background agent、doctor probe 和 count-token 路径都不得误走 Chat Completions 或泄露 OAuth token。

验收：core 测试覆盖预刷新、401 单次恢复、refresh 失败、fallback、取消和并发 turn；无公网依赖。

### Phase 5：模型、成本、诊断和文档

主要文件：

- `config/src/model_resolver.rs`
- `core/src/model_cost.rs` 与 usage overview
- `app-server/src/doctor/`
- `README.md`

任务：

- 增加订阅模式默认模型和 GPT-5.6 能力元数据。
- 为 usage/cost 引入明确 billing basis；订阅 token 不计入 API 美元预算。
- doctor 检查 OAuth expiry、account id、wire mode、被 API key 遮蔽、model availability 和 endpoint reachability。
- 文档说明 API Key 与 ChatGPT 订阅是两条独立路径，并给出登录、状态、注销和 headless 示例。
- 更新 provider capability 文案，不再把 OpenAI 描述为仅 Chat Completions。

验收：`providers`、`auth status`、`doctor` 和 README 表述一致；旧 API key 配置样例继续通过。

### Phase 6：端到端与发布门禁

- 增加 fake OAuth + fake Codex Responses 的 CLI e2e：登录、prompt、工具调用、续轮、注销。
- 使用隔离的临时 `ORBCODE_HOME` 做一次人工真实账户 smoke test，不提交 token、日志或 transcript。
- 依次运行：

```sh
cargo fmt --all --check
cargo clippy --workspace
cargo check --workspace
cargo test --workspace
scripts/audit-brand.sh
```

- 对认证文件、debug 日志、provider trace 和错误输出执行 secret 扫描。

验收：完整检查通过；真实账户可以在无 `OPENAI_API_KEY` 的环境中执行 `prompt "回复 OK"` 和至少一个工具回合。

## 测试矩阵

| 层 | 必测场景 |
| --- | --- |
| OAuth | PKCE/state、1455/1457、device poll、timeout/cancel、token exchange error |
| Storage | 旧格式兼容、0600、atomic write、rotated refresh、脱敏 |
| Precedence | env API key、stored API key、ChatGPT、shadowed status |
| HTTP | 固定 host、Bearer/account header、禁止 header/base URL 覆盖 |
| Request | text、history、function tool、tool output、effort、encrypted reasoning |
| SSE | text/thinking/tool deltas、usage、completed、failed、early EOF |
| Retry | proactive refresh、401 单次恢复、429 backoff、fallback、cancel |
| Transcript | resume、compaction、provider fallback、opaque signature 保留/剥离 |
| Billing | token 统计保留、API 成本不误计、`maxBudgetUsd` 不误阻断 |
| Regression | 现有 OpenAI-compatible Chat Completions 与 Anthropic 全部测试 |

## 风险与停止条件

1. OAuth 与 ChatGPT backend 不是公开 OpenAI API 合同。虽然官方 Codex 开源实现和 OpenCode 都使用该路径，服务端仍可能变更。所有常量和协议应隔离在专用模块，并用 fixture 锁定。
2. 必须先完成 Phase 0。若 client id 不允许 Orbcode、redirect URI 不在 allowlist、或账户明确拒绝第三方 originator，不通过复制浏览器 cookie、导入 Codex refresh token 等方式绕过。
3. Refresh token 可能轮换且旧 token 立即失效。没有 single-flight 与原子持久化之前，不允许把 OAuth 接入并发 agent loop。
4. Responses 的 reasoning opaque state 是多轮正确性的组成部分。若不能可靠保存和重放，不能把“文本单轮成功”视为功能完成。
5. 订阅用量不是 API 美元成本。在 billing basis 修正前，不对外宣称 `/cost` 或 `maxBudgetUsd` 对订阅模式准确。

## 参考文件

Orbcode：

- `config/src/auth.rs`
- `config/src/config.rs`
- `core/src/config_provider.rs`
- `core/src/retry.rs`
- `model-provider/src/http/openai.rs`
- `model-provider/src/request/openai.rs`
- `model-provider/src/stream/openai.rs`

本地 OpenCode：

- `~/github/opencode/packages/opencode/src/plugin/openai/codex.ts`
- `~/github/opencode/packages/core/src/plugin/provider/openai.ts`
- `~/github/opencode/packages/opencode/test/plugin/codex.test.ts`
- `~/github/opencode/packages/opencode/src/provider/transform.ts`

本地官方 Codex：

- `~/github/codex/codex-rs/login/src/server.rs`
- `~/github/codex/codex-rs/login/src/device_code_auth.rs`
- `~/github/codex/codex-rs/login/src/auth/manager.rs`
- `~/github/codex/codex-rs/login/src/auth/storage.rs`
- `~/github/codex/codex-rs/model-provider/src/bearer_auth_provider.rs`
- `~/github/codex/codex-rs/model-provider-info/src/lib.rs`
- `~/github/codex/codex-rs/codex-api/src/common.rs`
- `~/github/codex/codex-rs/codex-api/src/sse/responses.rs`
- `~/github/codex/codex-rs/models-manager/models.json`
- `~/github/codex/codex-rs/app-server/README.md`

公开说明：

- https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan
- https://opencode.ai/docs/providers#openai
