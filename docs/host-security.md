# Desktop Host and Remote Shell Security Model

Status: implemented desktop product and release-gate baseline. This document
describes the current app-server transports, Tauri host, renderer, packaging,
and credentialed macOS release path at 2026-08-06. A public release remains
conditional on the tag job and recorded clean-machine sign-off.

## Security goals

- Keep provider credentials, SSH credentials, connection tokens, transcript
  content, prompts, tool inputs/results, and settings values inside only the
  processes that require them.
- Treat the renderer as compromiseable. A renderer compromise may drive the
  current user's already-authorized protocol connection, but must not yield
  arbitrary process execution, file access, stored credentials, or a reusable
  connection secret.
- Reuse the canonical app-server protocol and its size/correlation rules. Do
  not add a privileged desktop-only domain API.
- Use child stdio for local and SSH product paths. Do not expose a public
  listener for supported remote use.
- Ensure one window owns one active child/connection generation and that all
  children and pending server requests reach a terminal state on disconnect.

Availability against a malicious local administrator, a compromised core, or
a compromised remote SSH host is not a goal. Secret recovery from process
memory by the same user or an administrator is reduced, not eliminated.

## Trusted components and boundaries

| Component | Trusted for | Must not do |
| --- | --- | --- |
| Signed desktop host | Verify packaged assets/binary selection, construct children, relay bounded envelopes, redact diagnostics, supervise window and child lifecycle | Interpret session business logic, expose arbitrary shell commands, or persist secrets silently |
| Packaged renderer assets | Render protocol state and collect user decisions | Read files/config/transcripts directly, construct child processes, or receive SSH/provider credentials |
| Generated TypeScript client | Correlate requests, preserve notification order, and respond exactly once to server requests | Invent desktop-only DTOs or silently retry non-idempotent protocol mutations |
| `orbcode serve --stdio` child | Own core/config/provider/MCP/session behavior | Trust renderer data outside the protocol and permission model |
| System `ssh` client | SSH encryption, config, agent, host-key verification, ProxyJump, MFA, and terminal prompts | Reveal keys or passwords to the desktop host/renderer |
| Remote host and remote `orbcode` | Remote execution and persistence explicitly selected by the user | Be treated as sharing the local machine's trust merely because SSH authenticated it |
| Tauri/platform WebView and OS | Enforce WebView, IPC, code-signing, process, and Keychain boundaries | Be assumed immune to platform vulnerabilities |
| Build/sign/notarize pipeline | Produce immutable first-party UI and a signed host/helper pair | Embed signing credentials, development URLs, writable UI, or unverified downloaded binaries |

The app-server protocol boundary is not an authorization boundary by itself.
Core permission rules and MCP trust remain authoritative after a client is
connected.

## Renderer compromise impact

A compromised renderer can read anything already rendered in that window,
submit protocol requests available to that connection, misrepresent UI, and
answer an outstanding permission/trust/question request as the user. The host
therefore exposes only a bounded envelope relay and lifecycle commands with no
arbitrary executable, argument, environment, URL, filesystem, or logging API.

The renderer must not receive provider tokens, environment blocks, SSH keys,
SSH agent handles, connection bearer tokens, raw crash diagnostics, transcript
paths, or signing/update credentials. Each successful connect/reconnect creates
a new connection generation; the host rejects commands for an older generation
and the desktop IPC transport drops older-generation events and responses.
Disconnect or replacement closes the child pipes and cancels pending
permission, MCP trust, AskUser, and goal requests so that a reloaded renderer
cannot answer a stale request.

## Current transport behavior

- `serve --stdio` reads and writes NDJSON on inherited pipes. It has no bearer
  token because possession of the pipes is the trust boundary. It exits at
  stdin EOF.
- Unix socket and WebSocket listeners authenticate the first line/message with
  a token. They process one active client and then accept a later sequential
  client; they do not arbitrate simultaneous writers.
- WebSocket additionally supports an Origin allow list, handshake/auth timeout,
  and message/frame bounds. Origin checking is defense in depth, not a bearer
  token replacement.
- Socket/WebSocket readiness is emitted after bind succeeds. Auto-generated
  tokens currently appear in the structured connection-info output. When
  stdout is a terminal the same values are emitted through tracing. This is
  acceptable only for the experimental explicit endpoint mode and is a known
  logging gap; the desktop and SSH product paths use stdio and no token.
- `orbcode remote ENDPOINT --token TOKEN` currently places an explicitly
  supplied token in process arguments. This remains experimental and must not
  be presented as the secure Internet remote product.
- `orbcode remote --ssh TARGET` launches the absolute system OpenSSH binary,
  carries the canonical protocol over the SSH child's stdio, and exposes no
  app-server listener or bearer token. SSH launch, host-key, authentication,
  connection, remote-binary, initialize, and timeout failures are classified
  separately from a bounded redacted stderr tail.
- `clients/desktop` is the production Tauri thin client. Its five scoped IPC
  commands connect local, connect SSH, relay a generated `ClientMessage`,
  disconnect the active generation, and report non-secret status. The host
  links `app-server-client` with its in-process feature disabled, and its normal
  dependency tree contains neither AppServer nor core.
- A host-observed child exit emits only generation, PID, reason, termination,
  success, code, and signal. The shared client closes pending requests and the
  renderer retains the session identity for an explicit resumable reconnect.
- The local browser prototype binds HTTP and WebSocket to `127.0.0.1`, restricts
  WebSocket Origin, and exposes its ephemeral token to its same-origin page.
  It is protocol evidence, not the desktop security design.

Plain `ws://` on a non-loopback address is unsupported. `wss://` does not make
an unmanaged listener a product path because token distribution, ownership,
and lifecycle remain unsolved. Explicitly managed tunnels may use the existing
transport under the operator's security policy.

## Credential and secret inventory

Every permitted location for a connection credential is listed here. Any new
location requires a threat-model update.

| Secret/credential | Permitted locations | Forbidden locations |
| --- | --- | --- |
| Local desktop connection credential | None; stdio pipe ownership replaces a token | Renderer, URLs, argv, environment, files, logs, crash reports, analytics |
| Experimental socket/WebSocket token | Server/client process memory; one readiness pipe read by the controlling host; explicit operator terminal output | URL query/fragment, localStorage/sessionStorage, packaged assets, transcripts, analytics, default diagnostics |
| SSH private key/password/MFA response | System SSH client, agent, Keychain/system credential UI, or terminal controlled by SSH | Desktop host memory when avoidable, renderer, profiles, orbcode protocol, logs, crash reports |
| SSH target/path/cwd/options | Reviewed host profile and host process memory; profile name/target/path/cwd may be stored in renderer localStorage because these fields are explicitly non-secret but privacy-sensitive | Analytics and default logs; SSH options are not persisted; credentials must never be entered as a profile field |
| Provider/MCP credentials | Core child environment/config and provider/MCP process boundary | Desktop host APIs, renderer, connection profile, URL, diagnostics, analytics |
| Transcript/prompt/tool/settings content | Core and the active protocol stream; renderer memory only while required to display/use it | Host logs, crash uploads, analytics, localStorage, desktop-owned shadow files |
| Signing/notarization/update credentials | CI secret store, temporary signing keychain, Apple service | Repository, app bundle, helper, renderer, runtime logs, crash reports |

Connection credentials may transiently exist in kernel pipe/socket buffers and
the address spaces of the two endpoints. Core dumps and swap are OS-level
residual risks; release crash handling must exclude payloads and environment.

## Local attacker scope

Another unprivileged local process must not be able to attach to desktop stdio
pipes. Experimental Unix sockets require restrictive directory/socket
permissions plus the bearer token. Loopback WebSocket protects against remote
network access but still requires the token and Origin validation because a
local process or hostile webpage can reach loopback.

The desktop development binary override is an explicit trust decision. Debug
builds consider the absolute `ORBCODE_DESKTOP_DEV_BINARY` path only when the
fixed bundle resource is unavailable. The active warning remains visible for
that connection, the selected path is not returned to the renderer, nothing is
persisted, and release builds ignore the variable. The signed bundle binary
wins by default. Protocol initialization is the first renderer request; the
renderer requires the exact host/helper version, protocol 1.0, streaming, and
its complete method inventory before it calls session bootstrap.

## SSH boundary

The host constructs an argv vector directly and launches `/usr/bin/ssh`
without a local shell. The target and reviewed SSH options are separate argv
items. The SSH protocol necessarily hands its remote command to the account's
login shell, so the remote cwd and binary path are encoded as individual POSIX
shell words by one documented routine. Fake-SSH tests pin the full argv and
adversarial quote handling. The resulting remote command starts
`orbcode serve --stdio` and nothing else.

OpenSSH owns `known_hosts`, host-key prompts, certificates, agents, config,
ProxyJump, and MFA. The desktop does not store a password or private key and
does not weaken `StrictHostKeyChecking`. If SSH requires an interactive prompt
that cannot be safely hosted, the product documents a system-terminal preflight
rather than scraping the prompt. Raw endpoint/token mode is omitted from
desktop v1; the experimental CLI surface is not secure Internet remote access.
The desktop host adds reviewed defaults `ConnectTimeout=20`,
`ServerAliveInterval=15`, and `ServerAliveCountMax=3` unless the same key was
explicitly supplied. This bounds initial connection and dead-peer detection.
If a healthy socket survives laptop sleep it continues normally; if the peer
is unreachable after wake, OpenSSH exits after its keepalive bound and the
generation-tagged exit path leaves the session resumable.

The remote host is a separate trust domain: it can see remote transcripts,
provider credentials configured there, remote cwd content, and tool activity.
The UI always labels local versus remote trust and cwd. Supported SSH mode
opens no app-server network port and distributes no bearer token.

## Envelope, request, and lifecycle policy

- Enforce the app-server transport payload limit before child-stdio writes and
  while reading child stdout. Tauri owns initial typed IPC deserialization;
  decoded envelopes still pass through the transport's outbound bound before
  reaching the child. Protocol stdout is never mixed with child stderr.
- Preserve NDJSON order and request IDs exactly. The host does not parse
  prompts or settings for logging.
- A window has one monotonically increasing connection generation. Replacement
  first closes/reaps the old child, resolves pending calls, then exposes the
  new generation.
- Permission, MCP trust, AskUser, and goal responses are exactly once. UI close,
  reload, transport EOF, or child exit resolves them as cancelled.
- Normal close sends protocol shutdown when available, closes stdin, waits a
  bounded interval, requests termination, waits again, then kills and reaps.
  App quit cannot complete while a local or SSH child is unaccounted for.
- Stderr is drained independently into a fixed-size in-memory capture with a
  small redaction-context allowance. Unknown partial lines at the truncation
  boundary are discarded, known secret environment values are registered for
  exact redaction, and redaction happens before diagnostics are exposed. The
  desktop IPC deliberately omits the stderr tail entirely and returns only PID,
  exit reason, termination mode, code, and signal. Raw environment, argv,
  prompts, protocol lines, and settings are never logged or returned by the
  host.

## Assets, navigation, IPC, and updates

Release assets are compiled into the signed application and are not writable
after installation. There is no release `devUrl`, remote navigation, remote
script/style/font, renderer devtools, permissive CSP, Node integration, or
generic Tauri shell/filesystem/network plugin. New windows and external URLs
are denied unless a narrowly reviewed host action handles a known-safe URL.

Every custom command is declared in Tauri's app manifest and granted to named
windows through the smallest capability. IPC input is size-bounded and typed;
the host selects executable paths and allowed argument shapes.

V1 has no auto-updater. Release packaging stages the workspace-matching helper,
uses an immutable resource mapping, enables Hardened Runtime with an empty
entitlement set, and signs the helper before the outer app. A credentialed tag
job notarizes, staples, runs Gatekeeper, and rejects credential files,
development URLs, world-writable resources, or a non-Developer-ID signature.
A later updater needs its own design covering signed
metadata, rollback, helper/app atomicity, downgrade prevention, and an update
origin isolated from renderer control. No runtime component downloads and
executes an unsigned binary or UI bundle.

## Logging and crash policy

Default logs contain event category, connection generation, bounded timing,
sanitized error category, child exit status, and build/protocol versions only.
They exclude protocol envelopes, prompts, transcript text/path, tool input and
output, settings values, environments, full SSH argv, tokens, and credentials.
Diagnostic export is opt-in, previews every included field, and applies the
same redaction before writing.

Crash upload and analytics are out of scope for v1. If approved later, they
must be payload-free by construction; filtering after capture is insufficient.

## Release security gates

- Inspect the final app and helper signatures, Hardened Runtime flags,
  entitlements, notarization ticket, and Gatekeeper assessment.
- Search the artifact for development URLs, tokens, signing material, writable
  remote UI sources, fixture secrets, and forbidden environment names.
- Verify incompatible host/core versions fail before bootstrap or mutation.
- Run clean-machine local/SSH, permission, reconnect, crash, reload, sleep, and
  quit smokes and confirm no orphan process.
- Record the exact CI jobs and manual evidence for the shipped version.
