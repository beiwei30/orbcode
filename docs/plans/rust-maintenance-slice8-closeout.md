# Rust maintenance Slice 8 closeout

## Conclusion

The bounded Rust idiomatic maintenance plan is closed on top of
`746aacec794470ae5325bfa71e83846fccb6027c` (`746aace`). Every family selected
by the `daa1f04` current-head inventory landed in its assigned owner boundary;
all unselected findings remain explicitly deferred rather than being reported
as fixed. Production library and binary `unwrap_used` remain at zero, standard
workspace gates remain the required bar, and `clippy::pedantic` remains an
advisory inventory rather than a deny gate.

This closeout changes no Rust production code, public API, protocol DTO,
serialization, transcript, CLI/TUI output, fixture, snapshot, or golden. It
adds sustainable guardrails for the three deliberately narrow boundaries that
the parent plan asked to keep from spreading.

## Audited state

The work ran in the dedicated worktree
`/Users/iluo/github/beiwei30/orbcode-slice-8-closeout` on branch
`chore/slice-8-closeout`. Both `HEAD` and `origin/main` resolved to
`746aacec794470ae5325bfa71e83846fccb6027c`. A first clean report captured the
selected-child state before closeout changes. Verification then exposed and
fixed two test-only, load-sensitive races: WebSocket listener readiness and a
slow MCP stdio request's completion synchronization. The report was run twice
again against the final worktree so the final all-target counts include both
test changes. Production Rust remained identical to `746aace`.

| Field | Evidence |
| --- | --- |
| rustc | `1.97.1 (8bab26f4f 2026-07-14)`, LLVM 22.1.6, host `aarch64-apple-darwin` |
| Cargo | `1.97.1 (c980f4866 2026-06-30)` |
| Clippy | `0.1.97 (8bab26f4f6 2026-07-14)` |
| Report script | version 2, Git blob `c539ad3e3f1dd1ba1bc1d003b333c15dc1a3ac67` |
| Clean selected-child report SHA-256 | `bb4a8f49c6bed5102eff6e057137b347230666173582095b8d2c636168ac275d` for both runs |
| Final closeout report SHA-256 | `da48915927bcbed49f19d122de9fb65db4f38171051abf0f80ff10d31376e3fa` for both runs |

Reproduction:

```sh
scripts/rust-maintenance-report.py > /tmp/orbcode-slice8-final-1.md
scripts/rust-maintenance-report.py > /tmp/orbcode-slice8-final-2.md
shasum -a 256 \
  /tmp/orbcode-slice8-final-1.md \
  /tmp/orbcode-slice8-final-2.md
diff -u \
  /tmp/orbcode-slice8-final-1.md \
  /tmp/orbcode-slice8-final-2.md
```

The two outputs were byte-identical. The report is not checked in as a global
warning golden: only the focused counts and dispositions below are durable.

## Selected P3 work disposition

| Plan | Selected owner family | Landed evidence | Closeout disposition |
| --- | --- | --- | --- |
| P3-01 | Current-head reinventory and bounded handoff | `c7e765b` | Complete; `rust-maintenance-current-head-inventory.md` remains the authoritative detailed classification. |
| P3-02 | Connection-owned app-server subscription pumps | `c100f5f` | Complete; finished handles are joined, panic/unexpected cancellation is observable, expected Drop abort remains quiet. |
| P3-03 | MCP trust-settings persistence source | `bfdb327` | Complete; `ConfigError` remains in the source chain while Display/Debug/protocol canaries remain redacted. |
| P3-04 | MCP WebSocket frame lengths | `f4d0be5` | Complete; outgoing conversion is checked, incoming over-limit input is rejected before allocation, and the 32-bit bound is proved. |
| P3-05 | Two private TUI overlay equivalent-arm matches | `746aace` | Complete; both warnings are removed and cursor/render behavior is covered across every overlay variant and frame captures. |

No selected child absorbed an inventory family that P3-01 deferred.

## Before and after counts

The “before” column is the reproducible P3-01 snapshot at `daa1f04`; the
“after” column is the final closeout worktree. Counts are diagnostics
deduplicated by lint, crate, path, line, and column. They are evidence for the
selected work, not a global quality score.

| Scope | `daa1f04` before | Final worktree | Change |
| --- | ---: | ---: | ---: |
| Classified production library, all warnings | 1,766 | 1,761 | -5 |
| Classified production library, selected warnings | 1,449 | 1,444 | -5 |
| Production library `unwrap_used` | 0 | 0 | 0 |
| Classified production other target, all warnings | 73 | 73 | 0 |
| Classified production other target, selected warnings | 46 | 46 | 0 |
| Production other target `unwrap_used` | 0 | 0 | 0 |
| Workspace all targets, all warnings | 3,786 | 3,785 | -1 |
| Workspace all targets, selected warnings | 3,166 | 3,160 | -6 |
| Workspace all targets `unwrap_used` | 1,367 | 1,364 | -3 |

The focused production-library reductions are exact:

| Lint family | `daa1f04` | Final worktree | Reason |
| --- | ---: | ---: | --- |
| `cast_possible_truncation` | 56 | 53 | The three selected MCP frame conversions now use proved widening or checked conversion. |
| `match_same_arms` | 35 | 33 | The two selected private TUI matches were merged. |
| Every other selected family | unchanged | unchanged | P3-02/P3-03 are behavioral risk fixes, and unselected lint families were not churned. |

At clean `746aace`, the children had added three all-target warnings while
reducing the five selected production diagnostics above. The closeout's
test-only WebSocket readiness fix then removed three test `unwrap_used`
diagnostics. Replacing the MCP test's literal fixed wait with event
synchronization removed one additional non-selected test diagnostic, so the
final all-warning total is one below the P3-01 snapshot and all-target
`unwrap_used` is lower by three. Neither test movement is reported as
production risk reduction.

## Sustainable guardrails

`scripts/audit-rust-maintenance.py` is now part of `scripts/check.sh`. It masks
comments and literals, excludes dedicated and `cfg(test)` code, and checks
stable semantic anchors instead of line numbers. Its focused self-tests cover
test-only exclusion, comments/literals, brace ownership, multiple spawns in one
owner, and the permission/nested-option scans.

The audit deliberately covers only:

1. **String-typed permission declarations:** six reviewed occurrences. Two are
   child-session persistence fields, one is the background-task compatibility
   DTO, two are stream/control wire projections, and one is a same-function
   parser staging variable converted to `PermissionMode` before runtime state.
2. **Raw nested options:** eleven reviewed occurrences. They are the goal PATCH
   wire/parser/runtime/UI chain (nine occurrences), transcript
   `PresentJsonValue` (one), and TUI `EffortOverrideSelection` (one). Each keeps
   absent/null/value semantics within its named owner boundary.
3. **Production `tokio::spawn`:** 55 static call sites across 35 owning-function
   anchors. The checked-in allow-list records a lifecycle disposition for each
   anchor: 17 sites are complete/observed, 14 are bounded best-effort, and 24
   remain assigned to a named deferred owner family.

A new occurrence, changed count, or move to another spawn owner fails the gate
until its lifecycle is classified. Intentional updates use:

```sh
scripts/audit-rust-maintenance.py --list-spawns
```

The command prints `UNCLASSIFIED`; it does not automatically bless a new
spawn. There is no update mode for pedantic totals.

## Deferred findings

The following work is intentionally not part of this closed maintenance plan:

- **Task lifecycle:** 24 static spawn sites remain in named client transport,
  ACP, core turn/background/goal/workflow, and MCP stdio owner families. Their
  present completion/cancellation behavior and remaining panic/`JoinError` gap
  are recorded in the current-head inventory and spawn allow-list. Reopening
  requires a focused owner plan and deterministic failure test.
- **Numeric diagnostics:** 171 production library/binary diagnostics remain
  across external values, counters/cost, time, buffers, and TUI display
  coordinates. Different domains require different checked, saturation, or
  clamping policies; a global cast rewrite would be unsafe.
- **Mechanical diagnostics:** 135 plausible `match_same_arms`,
  `manual_let_else`, `map_unwrap_or`, and `needless_pass_by_value` diagnostics
  remain across production library/binary targets. They need private call-graph
  review and are not valuable as a bulk target.
- **Error sources:** client transports, MCP TLS, core/tool adapters, and
  parser/final projections remain separated by owner and redaction risk. Fixed
  timeout/channel text has no underlying source to retain. None is claimed as
  fixed by the trust-settings batch.
- **Broad pedantic categories:** `missing_errors_doc`, `must_use_candidate`,
  `doc_markdown`, and `too_many_lines`, plus test/fixture/compatibility
  findings, remain outside the maintenance objective.
- **Protocol/API and representation changes:** public `Debug`,
  `non_exhaustive`, broad signature changes, transcript representation, and
  runtime-model semantics require independent API/product ownership.

These are deferred because the expected signal does not justify continued
cross-owner churn, not because their current warning count is zero.

## Focused verification

The closeout reran the proof commands for every selected implementation child:

| Command | Result |
| --- | --- |
| `python3 scripts/audit-rust-maintenance.py --self-test` | Passed; 4 focused scanner tests. |
| `scripts/audit-rust-maintenance.py` | Passed; 6 permission, 11 nested-option, and 55 spawn occurrences matched their reviewed boundaries. |
| `cargo test -p orbcode-app-server message_processor` | Passed; 40 focused tests, including normal completion, panic, unexpected cancellation, Drop, and pending-state cleanup. |
| `cargo test -p orbcode-app-server processor_drop_aborts_active_subscriptions` | Passed. |
| `cargo test -p orbcode-mcp set_server_trust` | Passed; 3 source/redaction/persistence tests. |
| `cargo test -p orbcode-mcp websocket_frame` | Passed; 4 outgoing/incoming/limit/platform-boundary tests. |
| `cargo test -p orbcode-tui overlay_cursor_style` | Passed; every overlay variant is enumerated. |
| `cargo test -p orbcode-tui frame_capture` | Passed; 58 frame-capture tests. |
| `cargo test -p orbcode-app-server-transport --lib` | Passed; 24 tests after replacing fixed-sleep startup with the transport's bound-address readiness signal. |
| `cargo test -p orbcode-tools skills::loader_tests::bounded_mcp_skill_discovery_timeout_does_not_cancel_stdio_request -- --exact` | Passed in 10 consecutive runs after replacing the fixed completion wait with a fake-server event. |
| `cargo test -p orbcode-tools --lib` | Passed; 346 tests passed and 13 repository-designated tests remained ignored. |

## Final gate

Two pre-fix workspace runs reproduced `ConnectionRefused` in
`websocket_disconnect` and
`tcp_without_upgrade_times_out_and_server_continues` under parallel load. The
tests released a temporary port, spawned a server that still had to re-bind,
slept for a fixed 100 ms, and then raced the client against that bind. Both
tests and the full transport crate passed in isolation, confirming the
load-sensitive startup race. The test helper now starts the existing
`run_websocket_transport_with_bound_addr` path on `127.0.0.1:0` and awaits its
bounded oneshot readiness signal before connecting. No production transport
behavior changed and no failure is suppressed or retried by the gate.

A later final workspace run exposed the same fixed-time assumption in
`bounded_mcp_skill_discovery_timeout_does_not_cancel_stdio_request`: under
load, its one-second sleep could finish before the first stdio client completed
initialization and entered the registry, so the follow-up probe could start a
second process. The fake MCP server now records receipt of `prompts/list`, and
the test waits with a bounded deadline for that event before probing connection
reuse. This keeps the cancellation/lifecycle assertion intact without assuming
a scheduler speed. The exact test passed 10 consecutive runs and the full
`orbcode-tools` library suite passed. No production MCP behavior changed.

The complete post-both-fixes canonical run passed:

| Command | Result |
| --- | --- |
| `scripts/check.sh` | Passed end to end: docs, fmt, Clippy, normal/no-default-feature checks, maintenance/public-surface/brand audits, and `cargo test --workspace`. Repository-designated manual, Node-dependent, release-boundary, and PTY tests remained ignored as declared by the canonical gate. |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed. |
| `cargo check --workspace` | Passed. |
| `cargo test --workspace` | Passed as the test stage of the complete post-fix `scripts/check.sh` run. |
| `scripts/check-docs.sh` | Passed. |
| `scripts/audit-rust-maintenance.py` | Passed. |
| `scripts/audit-public-surface.sh` | Passed. |
| `scripts/audit-brand.sh` | Passed. |
| `git diff --check` | Passed in both the code worktree and external plan repository. |
