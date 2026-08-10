# Desktop and SSH CLI Products

Status: Deferred
Decision date: 2026-08-08

## Decision

Orbcode does not currently ship a Desktop application or an SSH-specific CLI
flow. The Tauri Desktop product, its packaging/signing pipeline, and the
`orbcode remote --ssh` command have been removed so the supported surface does
not imply product readiness that the project does not need today.

The existing endpoint form, `orbcode remote ENDPOINT --token TOKEN`, remains an
experimental app-server client. It is not an SSH product and requires the
operator to manage the endpoint and authentication explicitly.

## Infrastructure retained

The following pieces are product-neutral and remain maintained:

- the canonical app-server protocol and generated schemas;
- the generated TypeScript DTOs and transport-independent protocol client;
- `orbcode serve --stdio`, Unix-socket, and WebSocket transports;
- bounded child-stdio process supervision in `orbcode-app-server-client`;
- the reviewed system-OpenSSH transport, launch policy, diagnostics, and
  focused library tests in `orbcode-app-server-client`;
- reference-host, protocol-correlation, compatibility, and child-transport
  tests that do not depend on a Desktop UI or SSH CLI.

These components are foundations, not user-visible support commitments. Their
public APIs may evolve while both products are deferred.

## Product surface intentionally absent

- no `clients/desktop` application or UI spike;
- no Desktop bundle, updater, signing, notarization, or release artifact;
- no Desktop-specific release gates or manual smoke checklist;
- no `remote --ssh`, `--remote-cwd`, `--remote-orbcode`, or `--ssh-option` CLI
  flags;
- no saved SSH profiles or Desktop/SSH product claims in user documentation.

## Guardrails while deferred

- Do not add Desktop or SSH-CLI behavior to `core`; both should remain protocol
  clients when work resumes.
- Keep authentication, framing, request correlation, and lifecycle ownership
  separated by the current protocol and transport boundaries.
- Keep SSH execution on the system OpenSSH client, without a shell-bearing
  option escape hatch or weakened host-key policy.
- Do not expose retained infrastructure as a supported command or packaged
  product without updating this plan and adding product-level verification.

## Conditions for restarting product work

Before either product is revived, agree on:

1. the concrete user and deployment use case;
2. ownership of authentication, secrets, host verification, and child cleanup;
3. compatibility/version negotiation and reconnect semantics;
4. interaction requirements for permissions, AskUser, sessions, cancellation,
   goals, errors, and accessibility;
5. distribution, signing, release, telemetry, and support ownership;
6. product-level automated tests and a clean-machine manual acceptance flow.

Desktop and SSH CLI may restart independently. Reusing the same protocol does
not require them to share a release schedule or UI lifecycle.

## Suggested future sequence

1. Revalidate the app-server and transport contracts with focused integration
   tests.
2. Choose one product and write its threat model and acceptance criteria.
3. Add the smallest explicit experimental surface without changing core turn
   semantics.
4. Add lifecycle, failure-path, compatibility, and release evidence before
   changing its maturity claim.

## Current verification baseline

When retained infrastructure changes, run at least:

```sh
cargo test -p orbcode-app-server-client
cargo test -p orbcode --test child_stdio_client_e2e
cargo test -p orbcode --test transport_server_request_e2e
npm --prefix clients/typescript run check:types
```

Use `cargo check --workspace` and the repository's standard formatting and diff
checks before merging broader changes.
