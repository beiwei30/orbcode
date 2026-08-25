# Rust maintenance current-head inventory

## Conclusion

This inventory audits `daa1f041ca5ebf2c2c6a5d7ff6dd1e27394e22a3`
(`daa1f04`). At the start of the audit, both `HEAD` and `origin/main` resolved to
that commit and the dedicated worktree was clean. Production library and binary
`unwrap_used` remain at zero. No production Rust, protocol DTO, test, fixture,
snapshot, golden, transcript, or public behavior is changed by this slice.

The bounded implementation handoffs are:

- P3-02: app-server connection-owned subscription pump handles;
- P3-03: MCP trust-setting persistence source retention;
- P3-04: MCP WebSocket frame length conversions;
- P3-05: two equivalent-arm matches in the private TUI overlay layout module.

All other matches below are deferred or refuted explicitly. A child plan may
refine only its accepted family; it must not absorb a deferred family without a
new current-head inventory decision.

## Baseline and reproducibility

The initial, unmodified checkout recorded:

| Field | Evidence |
| --- | --- |
| Worktree | `/Users/iluo/github/beiwei30/orbcode-rust-maintenance-p3-01` |
| Branch | `codex/rust-maintenance-p3-01-reinventory` |
| `HEAD` | `daa1f041ca5ebf2c2c6a5d7ff6dd1e27394e22a3` |
| `origin/main` | `daa1f041ca5ebf2c2c6a5d7ff6dd1e27394e22a3` |
| Initial status | `## codex/rust-maintenance-p3-01-reinventory`, with no tracked or untracked changes |
| rustc | `1.97.1 (8bab26f4f 2026-07-14)`, LLVM 22.1.6, host `aarch64-apple-darwin` |
| Cargo | `1.97.1 (c980f4866 2026-06-30)` |
| Clippy | `0.1.97 (8bab26f4f6 2026-07-14)` |
| Python | `3.9.6` |
| Report script | version 2, Git blob `c539ad3e3f1dd1ba1bc1d003b333c15dc1a3ac67` |

The report is produced by three Clippy JSON runs and the same classification
boundary used by Slice 0:

```sh
scripts/rust-maintenance-report.py \
  > /tmp/orbcode-rust-maintenance-current-head-1.md
scripts/rust-maintenance-report.py \
  > /tmp/orbcode-rust-maintenance-current-head-2.md
diff -u \
  /tmp/orbcode-rust-maintenance-current-head-1.md \
  /tmp/orbcode-rust-maintenance-current-head-2.md
```

The first two clean-checkout runs demonstrated a reproducibility defect rather
than a classification/count defect. `cli/tests/support/chatgpt_auth.rs` is
compiled into several integration-test targets. Cargo emits those diagnostics
in an unspecified order, while the old deduplicator retained the first target
name seen for a location. The two reports therefore alternated between
`chatgpt_auth_state_e2e` and `chatgpt_auth_device_e2e` at seven locations.

Script version 2 ranks equal-classification duplicates by target name, after
the existing classification priority. This preserves one row per lint, crate,
path, line, and column while making the displayed target deterministic. Two
consecutive post-fix runs were byte-identical, including classifications and
locations; both had SHA-256
`c792588f4d4121c34d0e217269288a2e3463cd6c6e52b2e78fd812c23e483e01`.
The generated report correctly records the worktree as dirty after the script
fix; the audited Rust sources remain exactly those at `daa1f04`.

## Current lint snapshot

| Scope | Slice 0 `b9ed5da` | Runtime-risk `abcc9b5` | Current `daa1f04` | Current versus `abcc9b5` |
| --- | ---: | ---: | ---: | ---: |
| classified production library, all warnings | 1,799 | 1,761 | 1,766 | +5 |
| classified production library, selected | 1,484 | 1,446 | 1,449 | +3 |
| production library `unwrap_used` | 15 | 0 | 0 | 0 |
| classified production other target, all warnings | 74 | 73 | 73 | 0 |
| classified production other target, selected | 47 | 46 | 46 | 0 |
| production other target `unwrap_used` | 1 | 0 | 0 | 0 |
| workspace all targets, all warnings | 3,792 | 3,757 | 3,786 | +29 |
| workspace all targets, selected | 3,189 | 3,150 | 3,166 | +16 |
| workspace all targets, `unwrap_used` | 1,383 | 1,367 | 1,367 | 0 |

The `abcc9b5` all-target totals above were regenerated with the same toolchain
and version-2 ordering logic in a clean detached worktree; the runtime-risk
report itself recorded only its production totals and an all-target unwrap
split of 1,367 test plus one compatibility hit. The rerun corrects that stale
split to 1,366 test plus one compatibility hit, 1,367 total; this does not
change the zero-production-unwrap conclusion.

From Slice 0 to `abcc9b5`, the runtime-risk work removed 15 production-library
unwraps, one binary unwrap, and 23 production-library numeric diagnostics. From
`abcc9b5` to `daa1f04`, production selected growth is exactly one
`manual_let_else` and two `map_unwrap_or` diagnostics in the current ChatGPT
OAuth implementation. The additional two production warnings and 13 selected
test warnings come from later auth/TUI production and test code. More precisely,
the production delta is three selected plus two non-selected warnings; the test
delta is 13 selected within 24 total warnings. Counts do not prove correctness:
the completed runtime-risk source and lifecycle tests remain the authority for
those fixes.

Current selected production-library counts are:

| Lint | Count | Lint | Count |
| --- | ---: | --- | ---: |
| `missing_errors_doc` | 541 | `must_use_candidate` | 508 |
| `too_many_lines` | 62 | `doc_markdown` | 59 |
| `cast_possible_truncation` | 56 | `cast_lossless` | 41 |
| `match_same_arms` | 35 | `needless_pass_by_value` | 35 |
| `cast_possible_wrap` | 28 | `manual_let_else` | 28 |
| `cast_sign_loss` | 25 | `map_unwrap_or` | 23 |
| `cast_precision_loss` | 8 | `unwrap_used` | 0 |

The production binary contributes 46 selected warnings: ten
`cast_possible_truncation`, ten `too_many_lines`, nine
`needless_pass_by_value`, five `cast_lossless`, four `doc_markdown`, three each
of `map_unwrap_or` and `match_same_arms`, and one each of
`cast_possible_wrap` and `manual_let_else`. Its `unwrap_used` count is zero.

## Production task inventory

The current text scan has 122 `tokio::spawn` matches under `**/src/**/*.rs`.
After excluding test-only locations, 57 are production sites. Slice 0 had 58
production sites: the completed AskUser change removed its detached timeout
`tokio::spawn` and now owns each request task through `JoinSet::spawn`. Three
later test-only spawns explain why the raw text total nevertheless rose from
120 to 122. The background cancellation bridge still occupies one production
`tokio::spawn` location, but now has terminal and consumer-close exits.

“Failure not observed” below means the task can terminate without its
`JoinError` being awaited or logged; it does not by itself prove a bug.

| Current sites (count) | Owner | Normal completion and cancellation/shutdown | Error or panic policy | Disposition |
| --- | --- | --- | --- | --- |
| `app-server-client/src/child_stdio_transport.rs:245,254,261,266` (4) | child stdio transport | detached supervisor joins stdout/stdin/stderr after child exit, fault, or Drop-triggered shutdown | child state is observable; three inner `JoinError`s and supervisor panic are not | **Defer:** separate client-transport owner batch |
| `app-server-client/src/in_process.rs:85,102` (2) | in-process transport | handles are stored; Drop aborts | tasks return `()`; panic is not observed | **Defer:** lower risk than selected prune loss |
| `app-server-client/src/ndjson_transport.rs:56` (1) | NDJSON connection | EOF ends reader; handle is stored; Drop detaches | I/O/parse failures collapse to connection close; panic is not observed | **Defer:** Drop/reader policy needs its own transport tests |
| `app-server-client/src/websocket_transport.rs:51,66` (2) | WebSocket connection | channel/socket close ends tasks; handles are stored; Drop detaches | loop errors become closure; panic is not observed | **Defer:** separate transport owner |
| `app-server-client/src/lib.rs:475,485` (2) | `AppClient` routers | explicitly detached; transport channel close ends each router | send failure ends routing; panic is not observed | **Defer:** documented best-effort routing |
| `app-server-transport/src/stdio.rs:211,234` (2) | stdio connection | local handles are selected/awaited; the peer is aborted | `TransportError` returns to caller | **Refute:** terminal/error policy is complete |
| `app-server-transport/src/websocket.rs:216,239` (2) | WebSocket connection | local handles are selected/awaited; the peer is aborted | `TransportError` returns to caller | **Refute:** terminal/error policy is complete |
| `app-server/src/message_processor.rs:375,422,500` (3) | connection `active_subscriptions` | stream/sink closure ends pumps; processor Drop aborts; finished handles are pruned | `prune_finished_subscriptions` drops a finished handle without observing normal completion versus panic | **Accept P3-02:** first stored-handle/prune family |
| `app-server/src/message_processor.rs:588` (1) | pending response map | one lock/remove/send and then exit | panic is not observed; send failure is harmless after removal | **Defer:** bounded best-effort resolver |
| `app-server/src/message_processor.rs:700,756,870` (3) | permission, MCP-trust, and AskUser pending requests | response, timeout, sink close, or pump guard cleanup retires pending state | fallback is explicit; waiter panic is not observed | **Defer:** distinct pending-request owner; do not mix with subscription handles |
| `app-server/src/background_api.rs:302` (1) | background progress receiver | consumer/broadcast close ends forwarding | infallible body; panic is not observed | **Defer:** best-effort subscription |
| `app-server/src/background_api.rs:391` (1) | returned background-turn receiver | terminal event, source/consumer close, or forwarded cancellation exits | infallible body; owner-release and cancellation E2E already cover it | **Refute:** runtime-risk fix complete |
| `cli/src/main.rs:264,295` (2) | serve command | readiness completes; server return aborts info task | infallible body | **Refute:** local await/abort policy is complete |
| `cli/src/headless.rs:1092` (1) | headless stdin-control loop | EOF/channel closure drives outer loop | reader errors are represented by frames/closure; panic is not observed | **Defer:** intentional CLI detach |
| `cli/src/acp_sdk/mod.rs:431`; `cli/src/acp_sdk/server_requests.rs:31,40,49` (4) | ACP connection and per-request handlers | server-request channel close or request completion ends tasks | request failures have fallback/log paths; pump panic is not observed | **Defer:** ACP owner family |
| `core/src/session_manager/mod.rs:1847` (1) | active turn | active-turn cancellation; exit clears permission, interaction, and active-turn state | normal errors are streamed; driver panic can skip cleanup and is not observed | **Defer:** correctness-relevant but separate core owner |
| `core/src/tool_runtime.rs:792` (1) | tool invocation AskUser forwarder | returned handle is awaited; per-request `JoinSet` drains on channel close | request and forwarder `JoinError`s map to `CoreError` | **Refute:** runtime-risk fix complete |
| `core/src/session_manager/session_background_agent.rs:561,566` (2) | background-agent record | task record/cancel flag owns outer task; inner forwarder ends on channel close and is awaited | terminal record observes loop result; forwarder `JoinError` is ignored | **Defer:** background-agent owner batch |
| `core/src/session_manager/session_goal.rs:285,291` (2) | persistent goal turn | outer supervisor owns inner turn; cancellation/terminal state is streamed | inner `JoinError` is not distinguished from normal completion | **Defer:** persistent-goal owner batch |
| `core/src/session_manager/session_response.rs:485,488` (2) | `StreamedToolUseExecution` | finish/interrupt/Drop owns outer handle; cancellation watcher is abort/awaited | outer `JoinError` becomes `CoreError`; expected watcher abort is awaited | **Refute:** owner and failure policy are complete |
| `core/src/session_manager/session_workflows.rs:499,929` (2) | workflow run and inline child-agent drain | workflow record/cancel flag owns detached run; drain is awaited before return | finalize errors print; drain `JoinError` is ignored | **Defer:** two workflow-local policies, not the selected connection family |
| `core/src/hook_runner/command.rs:116,154` (2) | hook child process | child wait/timeout and pipe close bound best-effort stdin writers | write errors are intentionally ignored | **Refute:** documented best-effort pipe writes |
| `mcp/src/transport/stdio.rs:314` (1) | MCP stdio client | handle is stored and timeout-awaited on shutdown; child Drop closes pipe | shutdown `JoinError` is ignored; Drop does not await | **Defer:** MCP stdio lifecycle batch |
| `tools/src/process.rs:25,30`; `tools/src/bash.rs:99,107` (4) | tool child process | readers are awaited; cancellation aborts and kills process | I/O and `JoinError` retain source in `ToolError` | **Refute:** terminal/error policy is complete |
| `tools/src/skills.rs:205,217` (2) | MCP skill discovery | primary discovery is awaited; late monitor is bounded and logs | `JoinError` is logged | **Refute:** observe/log policy is complete |
| `tui/src/app.rs:131,373,505` (3) | statusline command and background-task UI | result/event channels and in-flight state own tasks; runtime shutdown is the outer bound | business errors return in events; panic is not observed | **Defer:** best-effort UI owner, lowest risk |
| `tui/src/commands/async_local.rs:303,323`; `tui/src/commands/tui_local.rs:303`; `tui/src/commands/compact.rs:59` (4) | one TUI command operation | completion event returns to main loop | business errors are events; panic is not observed | **Defer:** per-command UI family |
| **Total** |  |  |  | **57 production `tokio::spawn` sites** |

## Error source and redaction inventory

The original direct query now returns 26 lines:

```sh
rg -n 'map_err\([^\n]*to_string\(\)' \
  core/src tools/src mcp/src app-server/src app-server-client/src \
  app-server-transport/src app-server-protocol/src
```

Semantic review separates them into ten real early source losses, two
intentional validation-to-tool-result projections, and fourteen timeout/channel
messages with no useful underlying source. An expanded `format!("...{error}")`
review adds the client transport, MCP TLS, HTTP tool, hook, goal, and workflow
families below. Text matches are candidates, not findings.

| Family and current sites | Chain classification | Redaction exposure | Disposition |
| --- | --- | --- | --- |
| MCP reqwest mappings: `mcp/src/oauth/{browser:313,328,374,376,377;discovery:76,101,151,153,156,179,181,184;device:119,121,124,175,178;ssrf:180;token:81,99}` and `mcp/src/transport/streamable_http.rs:39,120,177,215` (25 production mappings) | concrete source retained by `McpError::HttpWithSource` | URL userinfo/query/fragment are removed before storage; Display/Debug canaries already pass | **Refute:** completed runtime-risk family; do not reopen |
| Tool process readers: `tools/src/process.rs:48,51`, `tools/src/bash.rs:143,150` | `ToolError::ExecutionFailedWithSource` retains I/O/`JoinError` | no command/stdout/stderr is added by the mapping; existing error text remains final tool output | **Refute:** source already retained |
| MCP trust settings: `mcp/src/registry/trust.rs:102`, `set_server_trust_visible_to` | `ConfigError` is stringified into `io::Error`, so `McpError::Io` has no config/serde source | settings path/parse detail is low-to-medium risk; raw settings content must remain absent | **Accept P3-03:** existing `McpError::Io` can own the source without public enum change |
| App-client transport: `app-server-client/src/{ndjson_transport:35,45,112;websocket_transport:35,41;child_stdio_transport:213;ssh_remote:170}` | I/O, tungstenite, child-launch, or `ClientError` source is converted to `ClientError::Transport(String)` / `SshRemoteError::Launch(String)` | endpoint, auth-write context, child diagnostics, and public Debug need canaries before any wrapper change | **Defer:** public error enums and transport lifecycle require a separate owner batch |
| App-server transport WebSocket writer: `app-server-transport/src/websocket.rs:230` | tungstenite source becomes `TransportError::WebSocket(String)` | protocol/response metadata could appear in dependency Debug | **Defer:** public `TransportError` boundary |
| MCP WebSocket TLS: `mcp/src/transport/websocket.rs:301`, `connect_websocket_tls` | rustls I/O source becomes legacy `McpError::Http(String)` | host, certificate detail, and dependency Debug need negative canaries | **Defer:** distinct TLS owner; not the trust-settings batch |
| Core/app adapters: `core/src/retry.rs:368`, `core/src/session_manager/session_stream.rs:226,251,305`, `app-server/src/settings.rs:354` | auth/config/progress sources are projected into string-only provider/tool/core variants | auth-store content, tool input/progress, and paths are medium-to-high risk | **Defer:** each adapter has a different final boundary and public text contract |
| Tool Join/UTF-8: `tools/src/grep_tool.rs:517`, `tools/src/notebook.rs:318` | `JoinError` and `FromUtf8Error` sources are lost | panic payload can contain arbitrary text; `FromUtf8Error` Debug contains the rejected bytes | **Defer:** a safe wrapper must not expose panic payload or notebook bytes |
| Tool HTTP/parser projections: `tools/src/web_fetch.rs:44,106-147,390-412,584-585`, `tools/src/web_search.rs:212-285`, `tools/src/web_search_adapters.rs:234-298`, `tools/src/lsp.rs:30-34` | current `ToolError::{InvalidInput,ExecutionFailed}(String)` is the intentional user/tool-result projection, although selected internal HTTP sources could be retained earlier | raw URLs, queries, response bodies, filesystem paths, and provider text are high risk; several messages intentionally show bounded user input | **Defer:** keep final projection stable; any internal source batch needs URL/body/path canaries first |
| Core hook/goal/tool/workflow projections: `core/src/hook_runner/command.rs:74-123`, `core/src/tool_flow.rs:149`, `core/src/session_manager/{session_agent_tool:147,session_background_agent:506,session_goal_tools:246,session_workflows:757,849}` | current string is a tool, goal, progress, or transcript-facing final projection | hook command/input, persisted paths, child output, and workflow input are high risk | **Defer:** retain typed sources only inside each owner; do not change final bytes/messages |
| Validation/final parser boundaries: `tools/src/interaction.rs:24,91`, `core/src/hooks.rs:277,389,431,479,521`, `tools/src/file_state.rs:153,166,169`, `app-server/src/protocol_handler.rs:236` | intentional validation or wire/tool-result string projection | serde/validation errors currently omit raw payloads; file-state messages intentionally show the requested path | **Refute as early source loss:** preserve the projection and its bounded text |
| Fixed timeout/channel messages: `tools/src/interaction.rs:43`; `mcp/src/transport/{streamable_http:214;websocket:67,93,300}`; `mcp/src/oauth/{browser:203,312,373;discovery:100,150,178;token:80;device:118,174}` (14) | no underlying business source is discarded; timeout/channel identity is the complete diagnosis | fixed static messages, no secret-bearing source | **Refute:** do not wrap solely to increase source depth |

No reviewed mapping directly formats an authorization header or request body.
That negative source review is not a substitute for the canary tests required
by P3-03, especially for URL, auth-store, child-stderr, and HTTP response data.

## Numeric inventory by semantic domain

There are 158 production-library and 16 production-binary numeric diagnostics,
174 total. A single expression can contribute more than one lint location, so
these are diagnostic counts, not distinct expressions.

| Semantic domain | Library | Binary | Total | Current owners and policy | Disposition |
| --- | ---: | ---: | ---: | --- | --- |
| External/wire/OS values | 8 | 0 | 8 | MCP WebSocket frame lengths; protocol usage deserialization; tool payload/content lengths; child PID. Reject invalid wire/input, prove bounds before allocation or OS calls. | **Accept only MCP frame family for P3-04; defer the other owners.** |
| Counters, token usage, and cost | 28 | 5 | 33 | config token thresholds; core/protocol cost and usage; CLI stream-json accumulators; internal file/result counters. Widen with `From`, saturate where a smaller wrapped value would be misleading. | **Defer:** multiple owners and wire/display contracts. |
| Time, timestamps, durations, and windows | 36 | 11 | 47 | hook/turn/tool durations; session activity/GC timestamps; MCP OAuth/backoff; CLI elapsed time and stale-day windows. Reject negative/excessive configured time and use checked/saturating elapsed conversion per owner. | **Defer:** requires several independent time-policy batches. |
| Buffers, text, and preview sizes | 7 | 0 | 7 | memory-file cap; task log remaining bytes; tool-result/file-size display; token estimate lengths. Checked allocation lengths; display-only formatting may saturate. | **Defer:** separate core/tools/session-store owners. |
| TUI coordinates and display columns | 79 | 0 | 79 | `tui/src/{custom_terminal,bottom_pane,overlays,render,tui_theme}.rs`. Clamp/saturate to terminal/buffer bounds and preserve UTF-8/display-column behavior. | **Defer:** too broad for this handoff and independent of the selected MCP frame owner. |
| **Total** | **158** | **16** | **174** |  |  |

The selected external/wire family is exactly:

- `mcp/src/transport/websocket.rs::websocket_client_frame` at lines 404 and
  407: `usize` payload length encoded into the 7-bit/16-bit WebSocket forms;
- the same function at line 410 uses `usize -> u64` for the 64-bit form but does
  not currently produce a selected lint on this target; it remains part of the
  same proof boundary;
- `mcp/src/transport/websocket.rs::read_websocket_frame` at line 464: peer
  `u64` length converted to `usize` after the 8 MiB protocol check.

The runtime-risk provider conversions remain closed: current production
library numeric diagnostics are 158 versus Slice 0's 181, exactly the 23
diagnostics removed by that completed batch. No new provider numeric regression
was found.

## Mechanical candidate inventory

Broad API/documentation and size refactors are excluded, not queued:

| Family | Current production-library count | Disposition |
| --- | ---: | --- |
| `missing_errors_doc` | 541 | Exclude public-API documentation churn. |
| `must_use_candidate` | 508 | Exclude public ownership/signature churn. |
| `doc_markdown` | 59 | Exclude documentation-only churn. |
| `too_many_lines` | 62 | Exclude refactors without an independent owner reason. |

The mechanically plausible text candidates total 137 across production
library and binary targets: 38 `match_same_arms`, 29 `manual_let_else`, 26
`map_unwrap_or`, and 44 `needless_pass_by_value`. They are not all approved.
Generated code, compatibility/fixture/test paths, public signature changes, and
any file owned by P3-02 through P3-04 are excluded.

P3-05 may take only `tui/src/overlays/layout.rs` `match_same_arms` at lines 98
and 186. Both functions are crate-private, both rewrites stay in one file, and
the file is outside the selected task, error, and numeric families. The first
match returns the same cursor style for equivalent overlay variants; the second
has two no-op arms whose separate comments describe where those variants were
already rendered. The observable contract is the cursor style for every
overlay variant plus byte-identical normal overlay rendering. The other 135
plausible matches are deferred pending their own call-graph review; P3-05 must
not treat this report as approval to clean them.

## Exact child-plan handoffs

### P3-02 task failure observability

| Candidate family | Exact current files | Current disposition | Failure proof before implementation | Verification | Conflict boundary |
| --- | --- | --- | --- | --- | --- |
| Connection-owned active subscription pumps; risk is silently discarding a completed handle that panicked. | `app-server/src/message_processor.rs`: `active_subscriptions`, `prune_finished_subscriptions` (164-166), spawn/insert sites 375-390, 422-437, 500-514, and `Drop` 608-611. | **Accept.** Do not include pending-request waiters, client transports, core turns, or TUI tasks. | Add local test-only pump failure injection. Prove normal completion releases an `Arc` canary; prune observes injected panic distinctly from normal completion/expected abort; processor Drop terminates within a bounded timeout; pending request and subscription state clear exactly once; no retired pump writes afterward. Keep `processor_drop_aborts_active_subscriptions`. | `cargo test -p orbcode-app-server message_processor`; `cargo test -p orbcode-app-server processor_drop_aborts_active_subscriptions`; `cargo test -p orbcode-app-server`; then common workspace gates. | Own only `app-server/src/message_processor.rs` and its local tests. It cannot be edited concurrently with another app-server subscription/pending-request batch. |

### P3-03 error source and redaction

| Candidate family | Exact current files | Current disposition | Failure proof before implementation | Verification | Conflict boundary |
| --- | --- | --- | --- | --- | --- |
| MCP trust-setting persistence; risk is losing `ConfigError`/serde/I/O source while keeping the public `McpError::Io` message boundary. | `mcp/src/registry/trust.rs::set_server_trust_visible_to`, especially line 102; focused tests in `mcp/src/tests/mod.rs` near `set_server_trust_persists_to_settings_layer_without_trust_json` (3087). | **Accept.** Preserve `McpError::Io`; do not add a public variant or include MCP TLS/client/tool errors. | Pre-create malformed `settings.json` containing unique file-content/token canaries, then call `set_server_trust`. Before the production change, assert the chain is missing; after it, assert non-empty nested `Error::source()`. Require canaries absent from `Display`, `Debug`, and any captured protocol result. Re-run successful trust/deny persistence and trust/allow gates. | `cargo test -p orbcode-mcp set_server_trust`; `cargo test -p orbcode-mcp`; then common workspace gates. | Own `mcp/src/registry/trust.rs` and the smallest local test location. If `mcp/src/tests/mod.rs` is used, sequence against P3-04, which may also need that file. Do not edit `mcp/src/error.rs` public variants. |

### P3-04 numeric conversions

| Candidate family | Exact current files | Current disposition | Failure proof before implementation | Verification | Conflict boundary |
| --- | --- | --- | --- | --- | --- |
| MCP WebSocket frame lengths; risk is architecture-dependent or unchecked encoding/allocation at a peer-controlled wire boundary. | `mcp/src/transport/websocket.rs::websocket_client_frame` lines 399-414 and `read_websocket_frame` lines 428-470; selected diagnostics at 404, 407, and 464. | **Accept.** Keep the 8 MiB limit and wire bytes. Do not include OAuth time, registry backoff, tools buffers, protocol DTO widths, or TUI coordinates. | Add deterministic in-memory frames for lengths 0, 125, 126, 65,535, 65,536, 8 MiB, 8 MiB + 1, and encoded `u64::MAX`. Prove outgoing length-prefix bytes at each boundary, incoming over-limit rejection before allocation, and safe conversion on 32-bit via checked conversion/unit-level bound rather than a 64-bit-only inference. | `cargo test -p orbcode-mcp websocket_frame`; `cargo clippy -p orbcode-mcp --lib -- -W clippy::cast_possible_truncation`; `cargo test -p orbcode-mcp`; then common workspace gates. | Own `mcp/src/transport/websocket.rs` and its focused tests. Sequence with P3-03 if both touch `mcp/src/tests/mod.rs`; do not edit trust/TLS error mapping in this slice. |

### P3-05 mechanical Clippy

| Candidate family | Exact current files | Current disposition | Failure proof before implementation | Verification | Conflict boundary |
| --- | --- | --- | --- | --- | --- |
| TUI overlay-layout equivalent match arms; risk is accidentally changing cursor selection or the stage at which an overlay renders. | `tui/src/overlays/layout.rs::overlay_cursor_style` at line 98 and `draw_overlay_after_layout` at line 186. | **Accept exactly these two `match_same_arms` locations.** Do not include the line-80 pass-by-value candidate or any other TUI/file family. | Add a table-driven test that enumerates all `OverlayState` variants relevant to cursor style and records the expected `SetCursorStyle`/`None`; preserve existing frame captures for TranscriptPager/request-status overlays. Re-run a focused lint before and after to prove only the two approved locations move. | `cargo test -p orbcode-tui overlay_cursor_style`; `cargo test -p orbcode-tui frame_capture`; `cargo clippy -p orbcode-tui --lib -- -W clippy::match_same_arms`; `cargo test -p orbcode-tui`; then common workspace gates. | Own `tui/src/overlays/layout.rs` and one focused local test file/module. Do not touch TUI numeric, command-task, dynamic-command, fixture, snapshot, or golden files. |

## Common verification gates

Each child must first run only its listed focused owner commands, then the
common gates:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
cargo test --workspace
scripts/check-docs.sh
scripts/audit-public-surface.sh
scripts/audit-brand.sh
git diff --check
```

P3-01 itself does not add a second global warning-count gate. Its report script
already runs the required pedantic/unwrap Clippy snapshots, and pedantic totals
remain an inventory rather than a product metric.
