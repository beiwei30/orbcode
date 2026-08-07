# Orbcode Desktop Host Spike

This is the disposable Slice 0 Tauri 2 spike for ADR 0001. It is deliberately
outside the root Cargo workspace and is not a product or release package.

The renderer imports `clients/typescript/src/generated/protocol.ts`, sends one
canonical `initialize` request through an allow-listed Tauri IPC command, and
the Rust host relays the unchanged NDJSON line to a fixed test-child mode of
the same executable. Closing child stdin must make that child exit; the host
waits, escalates to kill on timeout, and always reaps it.

Run the evidence from this directory:

```sh
npm ci
npm run check:types
npm run build:web
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri -- build --no-bundle
```

The integration test invokes the command through Tauri's mock WebView IPC path
and verifies the canonical response and graceful child reap. `tauri dev` opens
the same probe UI if a visual check is useful.

Security constraints demonstrated here:

- no renderer-selected executable, argv, cwd, or environment;
- one IPC command declared by the app manifest and one-window capability;
- request/response limits and newline rejection before stdio relay;
- packaged assets, strict CSP, disabled global Tauri API and devtools;
- remote navigation rejection; and
- no `AppServer` or core dependency in the host.

The probe test child depends on `orbcode-app-server-protocol` only to emit a
canonical fixture response. Production replaces the whole probe with the
generic child-stdio client transport from Slice 1.
