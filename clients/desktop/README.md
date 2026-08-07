# Orbcode Desktop

This is the production Tauri thin client for local and SSH-backed Orbcode
sessions. The Rust host owns immutable assets, one child process, canonical
protocol IPC, sanitized exit diagnostics, and child cleanup. The renderer uses
only generated DTOs and the shared TypeScript correlation client; the host
links neither AppServer nor core.

The UI provides session list/create/load/rename/fork/delete, ordered transcript
blocks, streaming/cancel/reconnect, permission/MCP trust/AskUser requests,
session controls with managed-lock metadata, and keyboard/screen-reader paths.
Local and SSH modes use the same controller after transport creation. SSH
profiles persist only name, target, remote cwd, and remote Orbcode path in
renderer storage. They never contain keys, passwords, tokens, provider
credentials, SSH options, or transcript data.

System OpenSSH owns host verification, config, agent, ProxyJump, and MFA. A GUI
launch has no reliable terminal for password or host-key prompts; connect once
in Terminal or configure an agent/approved system authentication flow before
using the desktop app. The supported SSH path opens no listener and uses no
bearer token. Raw endpoint/token mode is intentionally not offered by desktop
v1; the CLI endpoint mode remains experimental and operator-managed only.
The desktop host supplies `ConnectTimeout=20`, `ServerAliveInterval=15`, and
`ServerAliveCountMax=3` unless a reviewed value is explicitly supplied. A
healthy SSH connection survives laptop sleep; after wake, an unreachable peer
becomes a resumable disconnect within the OpenSSH keepalive bound instead of
remaining indefinitely stale.

Local startup resolves the exact-version `bin/orbcode` resource first. Debug
builds may set `ORBCODE_DESKTOP_DEV_BINARY` to an absolute executable only when
that resource is absent. The UI marks the override, does not receive its path,
and never persists it. Release builds ignore the override. Initialization must
match desktop version `0.0.1`, protocol `1.0`, streaming, and every required
method before session bootstrap can mutate state.

Development checks:

    npm ci
    npm run check:types
    npm run build:web
    npm run test:a11y
    npm run test:compat
    cargo test --manifest-path src-tauri/Cargo.toml
    cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings

The mock-provider product E2E is owned by Cargo so its test binary has the
test-only provider feature:

    cargo test -p orbcode --test desktop_ui_e2e -- --ignored --nocapture

Build an unsigned release `.app` with the matching helper and audit it:

    npm run build:release-app
    ../../scripts/audit-desktop-artifact.sh 'src-tauri/target/release/bundle/macos/Orbcode Desktop.app'

Tag builds import Developer ID credentials from CI, sign the nested helper and
outer app with Hardened Runtime, submit to Apple, staple, run Gatekeeper, and
audit the result. Credentialed operators use
`scripts/sign-notarize-desktop.sh`. See `docs/desktop-release-smoke.md` for the
clean-machine gate. V1 has no updater and downloads no executable or UI.
