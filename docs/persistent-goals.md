# Experimental Persistent Goals

[English](persistent-goals.md) · [简体中文](zh-CN/persistent-goals.md)

Persistent goals are a session-scoped, transcript-backed capability. A session
has at most one current goal, while prior snapshots and checkpoints remain in
the JSONL transcript. The feature is experimental: a connection must set both
`capabilities.experimental_methods` and `capabilities.persistent_goals` during
`initialize` before the server advertises or accepts the method family.

## App-server methods

All four methods are experimental typed contracts:

- `session/goal/get`: `SessionIdParams -> SessionGoalGetResult`.
- `session/goal/set`: `SessionGoalSetParams -> SessionGoalSetResult`. Mutating
  an existing goal requires `expected_revision`; replacing a terminal goal also
  requires the explicit user-facing `replace` flag. A missing `token_budget`
  leaves it unchanged, JSON `null` clears it, and a positive integer sets it.
- `session/goal/clear`: `SessionIdParams -> SessionGoalClearResult`.
- `session/goal/continue`: `SessionGoalContinueParams ->
  SessionGoalContinueResult`. The request identifies both `goal_id` and
  `expected_revision`.

Continuation results are tagged by `outcome`. `started` includes a new ordinary
`subscription_id`, `turn_id`, and refreshed goal. `not_started` includes the
refreshed goal (or null) and one of: `missing`, `stale_revision`, `inactive`,
`usage_limited`, `budget_limited`, `pending_user_input`, `active_turn`, or
`client_not_capable`. Every subscription still ends at its normal terminal
event; a goal never changes stream terminal semantics.

## Canonical state and authority

`SessionGoal` contains `goal_id`, `revision`, `session_id`, `objective`,
`status`, optional `token_budget`, cumulative `tokens_used`, cumulative
`elapsed_seconds`, `created_at`, `updated_at`, optional `stop_reason`, and
optional `last_goal_turn_id`.

| From | Allowed destination and authority |
| --- | --- |
| none | `active` through user set or model create |
| `active` | `paused` by user/system; `blocked` or `complete` by model/user; limited by system |
| `paused` | `active`, `complete`, or clear by user |
| `blocked` | `active`, `complete`, or clear by user |
| `usage_limited` | `active` after conditions change, or clear, by user |
| `budget_limited` | `active` only after an explicit budget increase, or clear, by user |
| `complete` | a new goal through explicit create/replace, or clear, by user |

Model tools are limited to `get_goal`, `create_goal`, and `update_goal`.
`update_goal` may only choose `complete` or `blocked`. Pause, resume, budget
changes, usage-limit recovery, clear, and replacement are user/system
authority. The repeated-blocker rule is model guidance plus auditable turn
history in this version; Orb Code does not claim semantic equivalence detection
for blocker text.

A successful `update_goal` to `complete` returns `final_usage` when the goal has
an explicit token budget. It includes cumulative `tokens_used`, `token_budget`,
and `elapsed_seconds`, and is persisted before tool success is emitted.

## TUI command and scheduling policy

The local and remote TUI support these ordinary slash-output forms:

- `/goal` or `/goal show` displays objective, status, token usage/budget,
  elapsed time, revision, and any stop reason.
- `/goal create [--budget N] <objective>` creates and starts a goal. The
  shorthand `/goal [--budget N] <objective>` is equivalent.
- `/goal edit [--budget N|--no-budget] <objective>` edits the current goal.
- `/goal pause`, `/goal resume`, and `/goal clear` control lifecycle.
- `/goal budget N|none` changes or removes the token budget. Increasing or
  removing an exhausted budget also makes that goal active again.

After every terminal goal turn, the TUI submits queued user follow-up input
first. It requests another continuation only when there is no user follow-up,
active turn, or pending server request. Ctrl-C and terminal shutdown interrupt
the current turn and persist an active goal as paused.

## Client support

| Client surface | Goal methods/tools | Automatic continuation |
| --- | --- | --- |
| Local TUI | Yes | Yes, one ordinary subscription at a time |
| Remote TUI over socket/WebSocket | Yes | Yes, same policy as local TUI |
| ACP adapter | Yes, through typed `AppClient` | Yes, inside one ACP prompt lifecycle |
| Explicit goal-capable `AppClient` | Yes | Caller-owned through `continue_goal` |
| Default `AppClient`, `-p/--print`, `prompt`, background jobs | No | No; one prompt remains one terminal result |
| Raw app-server connection | Only after both experimental capability bits | Caller-owned |

Interactive-question support and persistent-goal supervision are separate
capabilities. In particular, enabling duplex stream-json questions does not
silently turn a headless prompt into a multi-turn goal runner.

## Transcript contract

Goal metadata uses these ordered JSONL record discriminants:

- `goal`: complete snapshot using camel-case transcript keys (`goalId`,
  `tokenBudget`, `tokensUsed`, `elapsedSeconds`, `createdAt`, `updatedAt`,
  `stopReason`, and `lastGoalTurnId`).
- `goal-cleared`: explicit tombstone; a later tombstone wins over every earlier
  snapshot.
- `goal-turn-start`: start checkpoint with `goalId`, `goalRevision`, `turnId`,
  and `timestamp`.
- `goal-turn-terminal`: terminal checkpoint with the same identity,
  `terminalKind`, canonical `usage`, elapsed delta, and `timestamp`.

The last valid snapshot or later tombstone determines current state. Unknown
fields on all four records are forward-compatible data and must survive a full
rewrite. A malformed goal record is also retained as inert metadata: it cannot
overwrite the last valid snapshot or apply a tombstone/turn boundary. Records
stay ordered relative to message boundaries so fork and rewind derive the state
visible at the selected point. A start checkpoint without a matching terminal
checkpoint is recovered as paused/interrupted and is never automatically
restarted. Old transcripts with none of these records decode to no goal.

`session/clear` creates a new session with no goal and keeps the old transcript
resumable. `session/delete` removes the goal with its transcript. Compaction,
fork, and rewind preserve the goal state visible at their selected transcript
boundary.

## Restart and failure behavior

Every mutation, start, and terminal accounting change is appended before the
new state is returned or its terminal event is forwarded. A failed append leaves
the previous persisted state authoritative and releases any reserved turn gate.

| Event | Persisted result |
| --- | --- |
| Normal turn finish | Goal remains active unless the model completed/blocked it or its token budget was reached |
| Explicit goal budget reached | `budget_limited`; resume requires a larger or removed budget |
| Provider rate/account limit | `usage_limited` |
| Provider error or unclassified interruption | `paused` with a stop reason |
| Cancel, Ctrl-C, owner disconnect, close, or EOF | `paused`; no detached turn remains |
| Process loss after start but before terminal checkpoint | Recovered as `paused`/interrupted on load |
| Malformed later goal record | Retained but ignored; last valid state remains current |

Automatic continuation is always client-supervised. Disconnect, EOF, close,
cancel, or process interruption cancels the owned turn and pauses an active
goal; no daemon-owned detached runner exists in the first version.
