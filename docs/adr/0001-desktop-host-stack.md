# ADR 0001: Tauri 2 for the macOS Desktop Host

- Status: accepted
- Date: 2026-08-06
- Decision owners: desktop host maintainers
- Scope: first signed desktop host; UI and protocol packages remain portable

## Context

Orbcode already has one agent core, one app-server protocol, generated
TypeScript DTOs, and stdio/socket/WebSocket transports. The desktop product
must supervise `orbcode serve --stdio` as a child and relay canonical protocol
envelopes. It must not link `AppServer`, expose a second domain API, load remote
UI, or give the renderer process and filesystem access.

The first delivery target is Developer ID distribution on macOS. The choice is
between Tauri 2, Electron, and a native Swift/AppKit or SwiftUI host.

## Decision drivers

The host choice is evaluated against:

- installed and compressed bundle size;
- reuse of the Rust workspace, generated TypeScript DTOs, and web UI code;
- renderer privilege separation and IPC allow-listing;
- accessibility and keyboard behavior;
- Developer ID signing, Hardened Runtime, notarization, and helper signing;
- reliable stdio child ownership and bounded shutdown;
- secure storage availability without making secrets renderer-readable;
- updater isolation and the ability to ship without an updater;
- deterministic host and renderer test automation.

## Evidence

The time-boxed spike lives at `clients/desktop-spike`. It uses Tauri 2.11.5 and
contains no `AppServer` dependency. Its renderer imports the generated
TypeScript protocol DTOs, invokes one allow-listed host command, sends a
canonical `initialize` envelope, receives the canonical response from a fixed
test child over NDJSON stdio, and observes that the child was reaped. The test
uses Tauri's mock WebView IPC path rather than calling the command function
directly. The child path is host-owned; it is not renderer input.

The spike also establishes these configuration constraints:

- packaged assets only, with no development URL;
- strict CSP and no remote source in `connect-src`;
- global Tauri APIs disabled;
- one explicitly named window and one generated command permission;
- navigation rejected unless it targets the packaged Tauri origin;
- developer tools disabled and bundling disabled because this is a spike, not
  a release artifact.

Relevant upstream evidence:

- Tauri capabilities constrain which windows may call which commands and are
  intended to reduce the impact of a compromised frontend:
  <https://v2.tauri.app/security/capabilities/>.
- Tauri recommends an explicit restrictive CSP and avoiding remote scripts:
  <https://v2.tauri.app/security/csp/>.
- Tauri supports embedded external binaries and command argument permissions:
  <https://v2.tauri.app/develop/sidecar/>.
- Tauri documents Developer ID signing and notarization configuration:
  <https://v2.tauri.app/distribute/sign/macos/>.
- Apple requires signed executables, Hardened Runtime, a secure timestamp, and
  valid entitlements for notarization:
  <https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution>.
- Electron's own security guide requires sandboxing, context isolation, a
  restrictive CSP, navigation limits, IPC sender validation, and staying
  current with Chromium:
  <https://www.electronjs.org/docs/latest/tutorial/security>.

Signing and notarization credentials are intentionally not used in the spike.
That end-to-end evidence belongs to the release gate, where the actual bundled
`orbcode` helper and final entitlements can be inspected together.

## Options considered

| Criterion | Tauri 2 | Electron | Native Swift host |
| --- | --- | --- | --- |
| Bundle/runtime | Reuses the platform WebView; Rust host adds no bundled browser runtime | Bundles Chromium and Node, creating the largest update and patch surface | Smallest platform-specific host |
| Protocol/UI reuse | Direct Rust and TypeScript reuse | Direct TypeScript reuse; Rust lifecycle needs a native helper or Node bridge | Generated TypeScript UI cannot be reused directly without adding WKWebView bridging |
| Privilege boundary | Rust commands plus capability/permission allow lists; system WebView remains an attack surface | Main/preload/renderer boundary is viable but requires continuous Chromium and Electron hardening | Strongest opportunity for a narrow native boundary, but every typed client and UI bridge is new code |
| Child supervision | Rust `std::process`/Tokio ownership fits the existing codebase; spike proves fixed-child stdio and reap | Node child APIs are mature, but the privileged main process becomes a second language runtime | Native process APIs are capable but require a new implementation and test harness |
| Accessibility | Platform WebView semantics and focus APIs; final product still needs VoiceOver and keyboard smoke | Chromium accessibility is mature; final product still needs the same smoke | Best native control integration, at the cost of duplicating the UI |
| Signing/notarization | Documented Developer ID and notarization flow; helper still needs explicit nested signing verification | Mature packaging ecosystem, but both the Electron runtime and helper must be signed | Best Xcode integration |
| Secure storage | Rust can call Keychain without exposing it to the renderer; v1 should avoid storing connection secrets entirely | Main process can call Keychain modules, increasing native dependency surface | Direct Keychain access |
| Updater isolation | Updater is optional and is omitted in v1 unless separately approved | Updater is available but also omitted in v1 | Must be designed from scratch or delegated to distribution tooling |
| Automation | Rust unit/integration tests, Tauri mock IPC, and a later desktop driver | Mature Spectron successors and Playwright support | XCUITest is strong but macOS-only and adds a separate test stack |

## Decision

Use Tauri 2 with the platform WebView for the first macOS host. Keep the Rust
host limited to process lifecycle, canonical envelope relay, trusted asset and
window policy, diagnostics, and packaging. The renderer owns typed protocol
correlation through the generated TypeScript client. The core remains a
separate `orbcode serve --stdio` child.

The production desktop package will replace the disposable probe with the
generic child-stdio transport. It may use Tauri's sidecar packaging mechanism
for the signed bundled binary, but renderer-facing shell/process plugins are
not allowed; child construction stays in Rust.

## Rejected alternatives

Electron is rejected for v1 because it adds a bundled Chromium/Node runtime and
its patch surface without improving protocol reuse or child supervision enough
to justify that cost. It remains the fallback if the platform WebView fails a
measured accessibility requirement that cannot be fixed in the product UI.

A native Swift host is rejected because it would require a second UI and IPC
client implementation, weakening byte-level protocol convergence. It remains
the fallback if final Developer ID signing, helper supervision, or WebView
accessibility evidence invalidates Tauri.

## Consequences and invalidation gates

- macOS WebKit behavior becomes part of the supported UI matrix.
- Linux and Windows source portability does not imply packaging support in v1.
- Every IPC command must be declared in the app manifest and granted to the
  smallest window capability; arbitrary command/process launch is forbidden.
- Release builds contain only immutable assets, no `devUrl`, no devtools, and
  no remote navigation.
- The app and bundled helper must be signed together and inspected before
  notarization. No Hardened Runtime exception is accepted without a recorded
  reason.
- The decision must be revisited if VoiceOver/keyboard smoke fails, Tauri cannot
  supervise the signed helper without an orphan, or notarization requires an
  entitlement that materially weakens the threat model.
