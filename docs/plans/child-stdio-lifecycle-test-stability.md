# Child Stdio Lifecycle Test Stability Closeout

Date: 2026-08-26

Baseline: `feac5416edf113ed0c40aafb39ac99775da7a831`

Environment: macOS Darwin 25.6.0 on arm64 (`kaku`, 10 logical CPUs),
`rustc 1.97.1 (8bab26f4f 2026-07-14)`, and
`cargo 1.97.1 (c980f4866 2026-06-30)`. `RUST_TEST_THREADS` was unset, so the
default-parallel target used the host's normal libtest parallelism.

## Classification

Both historical failure shapes came from test-only scheduling assumptions: a
shared escalation budget and a missing writer handoff. No production lifecycle
defect was demonstrated, so
`app-server-client/src/child_stdio_transport.rs` and the
`deferred:client-stdio-lifecycle` spawn disposition remain unchanged.

The pre-change target applied 100 ms graceful and terminate phases to every
fixture, including fixtures expected to exit normally after stdin EOF. An
unmodified default-parallel stress loop failed on its fifth invocation in
`canonical_initialize_works_through_app_client`: the final
`ShutdownRequested` diagnostics had `success == false`. Isolated canonical and
broken-stdin runs and the complete target with `--test-threads=1` passed. A
temporary strict diagnostic assertion also reproduced the same ordering in the
normal protocol-semantics test as `termination == Terminated` where
`Graceful` was required.

The canonical causal order was:

1. dropping the owner sent shutdown;
2. the supervisor acknowledged writer close and delivered stdin EOF;
3. the shared 100 ms grace expired before the normal fixture was scheduled to
   finish its EOF path;
4. the supervisor sent terminate and published stable `ShutdownRequested`
   diagnostics with an unsuccessful signaled exit.

The original broken-stdin fixture closed its stdin descriptor before publishing
`fixture/ready`, but no earlier event proved that the parent writer had
completed an I/O handoff. After ready, `request_raw` inserted the pending request
and queued the next writer command. The writer's failed `write_all` or `flush`
is the only path that clears that request and publishes `StdinIo`. A temporary
investigation test gave the request an already-expired outer deadline: Tokio
first polled the request far enough to enqueue the command, then the watchdog
won before the separately spawned writer ran. Once scheduling resumed, the
same command produced `StdinIo` and the supervisor reaped the fixture well
before its 30-second hold could end naturally.

After the first full-gate finding below was repaired, a default-parallel stress
loop also reached the ten-second watchdog in the broken-stdin request. That
proved timeout size alone did not supply the missing handoff. The final fixture
first reads and responds to `fixture/arm-broken-stdin`, then closes stdin and
publishes ready. The faulting request therefore follows a completed parent
write, child read/response, descriptor close, and ready event. Its exact
`StdinIo` diagnostics now prove the next writer attempt, pending release, and
held-child reap without treating elapsed time as readiness.

## Implementation

`app-server-client/tests/child_stdio_transport.rs` now separates four timing
purposes:

- ready normal and graceful-fault paths receive ten seconds to process stdin
  EOF while retaining the production two-second terminate phase;
- the deliberately held broken-stdin fault uses the production two-second
  graceful and terminate phases to bound its expected escalation;
- only `shutdown_escalates_after_eof_and_remains_bounded` compresses both
  escalation phases to 100 ms;
- a twenty-second outer watchdog bounds complete asynchronous operations, while
  fixture messages and final process state remain the progress evidence.

The final assertions retain exact lifecycle proof. Normal owner shutdown must
be `ShutdownRequested`, `Graceful`, exit code zero, no signal, and successful.
Malformed and oversized stdout keep their exact reason and graceful exit
diagnostics. Broken stdin must be alive at readiness, release its request,
publish exactly `StdinIo`, and be reaped through terminate or kill with the
corresponding signal. Redaction, payload, early-exit, protocol, backpressure,
repeated shutdown, and explicit escalation assertions remain active.

The fixture change is confined to this two-phase broken-stdin handshake. No
production Rust, public API, protocol DTO, wire format, diagnostics schema,
redaction contract, retry policy, ignored test, serial annotation, or
maintenance classification changed.

The second clean-worktree full gate exposed one additional ordering in
`outbound_payload_limit_is_enforced_before_write`. The oversized request is
rejected locally before the writer sees it, so the test had spawned a child and
immediately requested shutdown without first proving that the child had been
scheduled. Under workspace load its two-second normal grace expired first and
diagnostics reported `ShutdownRequested`, `Terminated`, and `success == false`.
The test now completes one small echo round trip as its readiness handoff before
checking the pre-write payload rejection. This preserves the payload boundary
while making the later graceful-exit assertion about a known-ready fixture.

A subsequent default-parallel stress loop reached a second normal-shutdown
ordering in `canonical_initialize_works_through_app_client`: despite completed
initialize readiness, the production-default two-second grace expired before
the fixture consumed EOF, again yielding `ShutdownRequested`, `Terminated`, and
`success == false`. This demonstrated that readiness and graceful-exit budget
are independent. The final ten-second lifecycle grace is scoped only to paths
that must exit naturally; it does not change the held fault's escalation or the
explicit 100 ms slow-shutdown proof.

## Verification

After all three orderings above were addressed, the final default-parallel
stress gate passed 20 consecutive target runs without retrying an individual
failure. Final focused and repository gates are recorded in the branch handoff
after they run against the clean final diff.
