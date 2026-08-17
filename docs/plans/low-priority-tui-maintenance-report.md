# Low-priority TUI maintenance report

## Baseline and scope

This work was re-audited on `abcc9b581e55c7e0d710391bf66546ddde5012e6`
(`Harden runtime risk boundaries`) in the dedicated
`codex/low-priority-tui-maintenance` worktree. The source worktree was clean and
no other worktree owned the affected TUI files. The toolchain was:

| Tool | Version |
| --- | --- |
| rustc | `1.97.1 (8bab26f4f 2026-07-14)` |
| Clippy | `0.1.97 (8bab26f4f6 2026-07-14)` |

The maintenance scope remains render-only TUI state. It does not change the
protocol, transcript persistence, permissions, resize debounce, or app-server
wire shape.

## Finding disposition

| Finding | Current-head evidence | Decision / result |
| --- | --- | --- |
| Completed-panel TTL redraw repeats | Both panel `tick` methods returned `true` on every post-TTL call while finished rows remained. | **Accepted and fixed.** Each panel now records whether expiry was reported. Active/empty/session reset clears the latch; a new terminal task/status generation starts a fresh TTL without letting metadata-only refreshes extend it. |
| Deferred assistant infrastructure | Before removal, the only `Some(DeferredAssistantMessage { ... })` construction was the synthetic frame-capture test. Production event paths only cleared or consumed the field. | **Accepted and removed.** The type, `TuiState` field, finalize/commit helpers, draw-transaction call, and synthetic test are gone. Real completion and frame-capture tests remain authoritative. |
| Logical vertical desired column is a byte offset | `move_cursor_logical_vertical` used `input_cursor - current_line_start`; this diverged from input layout on wide/multibyte glyphs. | **Accepted and fixed.** Desired column is now the display column defined by the existing input-layout width helper. Logical and visual repeated motion share the same clamp-and-restore behavior and still store byte cursors at valid UTF-8 boundaries. |
| Initial/diff draw metric units differ | Initial frames used a bespoke estimator; diff frames used `updates.len()`, while exact cursor/style/print/clear counts already existed for both paths. | **Accepted: actual logical terminal commands.** `draw_command_count` is now the sum of cursor, style, print, and clear commands plus the disable/enable line-wrap pair. Initial and resize/full-diff paths use the same unit; an unchanged incremental frame has the documented fixed floor of five. The existing diagnostic JSON key is retained. |
| Dynamic slash command collides with builtin | Builtins are the registry prefix, all dynamic entries are appended, suggestions include both exact entries, and invocation uses the first name/alias match. The focused regression test records that a colliding user/project command is discoverable but the builtin executes. | **Deferred product decision; no behavior change in this maintenance batch.** See the contract and reopen conditions below. |
| Permission picker caret offset | The current permission UI is the three-preset picker. `permission_picker_cursor` always returns `None`; no search query or `box_width - 6` caret calculation remains. Existing tests cover 40-, 80-, and 200-column layouts. | **Refuted by replacement.** The obsolete search-picker finding is closed; the removed search UI must not be reintroduced as a fix. |

## Deferred assistant production-behavior proof

The removal is covered by production-state transitions rather than a manually
constructible dead state:

- `thinking_only_completion_commits_one_thinking_block` proves thinking-only
  completion commits once.
- `assistant_message_completed_commits_virtual_transcript_once` and
  `vt100_final_answer_head_and_tail_commit_once_without_live_chrome_interleave`
  prove final assistant messages commit once.
- `turn_finished_commits_assistant_message_before_duration_note` preserves the
  history ordering contract.
- the `frame_capture` suite exercises draw/resize transactions without a
  deferred-message side channel.
- `assistant_message_with_tool_use_keeps_request_active` preserves tool-use
  completion behavior.

A source search under `tui/src` for `DeferredAssistantMessage`,
`deferred_assistant_message`, `commit_deferred_assistant_message`, and
`finalize_deferred_assistant_message` must remain empty.

## Slash collision decision

Status: **Deferred**.

This batch intentionally preserves the current behavior: builtins occupy the
registry prefix and therefore win exact name or alias dispatch; colliding
dynamic commands can still appear in suggestions but are unreachable through
that spelling. This records the behavior; it does not endorse silent collision
as the final UX.

The issue reopens only when a product/compatibility owner defines all of the
following in one contract:

1. whether builtin names and aliases are reserved or explicitly overridable;
2. whether a collision is rejected, hidden, warned, or exposed through a new
   namespace;
3. user-versus-project precedence and whether aliases have identical rules;
4. consistent treatment for plugin, MCP prompt, skill, and workflow commands;
5. the TypeScript CLI compatibility requirement for discovery, completion,
   expansion, dispatch, hot reload, and usage recency.

Absent an override requirement, the preferred proposal is to reserve builtin
names/aliases, omit colliding dynamic entries from the executable registry, and
surface an actionable source-aware diagnostic. Implementing that proposal or a
new user/project namespace requires a separate approved product-contract plan.

## Verification ownership

The implementation remains split by owner:

| Slice | Files / behavior | Focused verification |
| --- | --- | --- |
| TTL latch | `background_agent_panel.rs`, `transcript_task_cards.rs` | pre-TTL, first expiry, repeated expiry, active reset, unseen completed generation, metadata refresh, empty/session reset |
| Deferred state removal | prompt/state, stream events, terminal transaction, fixture initializers | thinking-only, final answer, tool-use completion, history order, all frame captures |
| Display column | input layout, motion, motion tests | ASCII, CJK, emoji/ZWJ, combining, tab, short/empty/long, repeat/visual, CRLF and char-boundary assertions |
| Draw metric | custom terminal and diagnostic docs | initial/full-diff equality of units, unchanged floor, single/style/wide/resize coverage, render metric cache tests |
| Decisions | this report and collision regression | current discovery/menu/expansion/dispatch evidence; no namespace or precedence mutation |

## Final verification

All required gates passed against the completed worktree:

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | Passed |
| `cargo clippy -p orbcode-tui --all-targets -- -D warnings` | Passed |
| `cargo check -p orbcode-tui --tests` | Passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `cargo check --workspace` | Passed |
| `cargo test --workspace` | Passed on the complete rerun; repository-designated manual/timeout tests remained ignored |
| `scripts/check-docs.sh` | Passed |
| `scripts/audit-public-surface.sh` | Passed |
| `scripts/audit-brand.sh` | Passed |
| `git diff --check` | Passed |

The first workspace-test run had one load-sensitive three-second timeout in
`broken_stdin_cancels_pending_request_and_reports_reason` while the workspace
was still compiling. The exact test and its full `child_stdio_transport` target
both passed immediately afterward, and the subsequent complete
`cargo test --workspace` rerun also passed, including that test.
