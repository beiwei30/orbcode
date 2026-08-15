# Integrations

[English](integrations.md) · [简体中文](zh-CN/integrations.md)

Orb Code has two integration boundaries: an ACP adapter for editors and a
canonical app-server protocol for thin clients. Both are experimental even
though many underlying session/tool operations are stable.

## Agent Client Protocol (ACP)

Run the ACP v1 adapter over stdio:

```bash
orbcode acp
```

It maps ACP session creation/loading, prompts, streamed text/thought/tool
updates, model selection, thought controls, permissions, history restoration,
and compatible MCP setup onto the same in-process app server as the TUI.

ACP advertises only capabilities it can complete. Its stable permission request
maps one selected option; it does not advertise Orb Code's richer canonical
multi-question interaction schema. Image/resource/audio prompt content follows
the current capability matrix rather than being accepted optimistically.

Read the detailed [ACP support matrix](acp-support.md) before building a client.

### Zed

The tested integration launches the binary as a custom agent. Build/install it,
then point Zed's agent configuration at `orbcode acp`. The recorded smoke guide
covers lifecycle, streaming, restored history, model/thought controls,
permissions, MCP, content types, and shutdown:

- [ACP with Zed](acp-zed-smoke.md)

The versions in that guide are a recorded baseline, not a permanent support
promise for every future Zed/ACP version.

## App-server protocol

The protocol is version `1.0` with request/response envelopes, stream-event
notifications, and server-originated permission, MCP trust, and interactive
question requests. Clients initialize with their streaming, experimental
method, persistent-goal, and interactive-question capabilities.

Stable request families cover session bootstrap/list/fork/rename/rewind,
turn submit/steer/cancel, permission rules, settings, context/usage, MCP,
tools, auth, and diagnostics. Experimental families include persistent goals,
some session controls, background tasks, and dynamic workflows. The server
returns stable and experimental method lists during initialization; clients
must not assume an experimental method remains unchanged across releases.

### Run a server

`serve` is intentionally hidden from top-level help while the product surface
is experimental:

```bash
# Parent-owned stdio, implicitly trusted.
orbcode serve --stdio

# Unix socket; token is generated and printed if omitted.
orbcode serve --socket /tmp/orbcode.sock --auth-token "$TOKEN"

# WebSocket with Origin allowlisting.
orbcode serve --websocket 127.0.0.1:8080 --auth-token "$TOKEN" \
  --allowed-origin https://client.example
```

Socket and WebSocket listeners support sequential reconnects with one active
client at a time. They require token authentication. WebSocket can additionally
reject unmatched `Origin` values. Do not bind an unaudited endpoint to a public
interface.

### Remote TUI

```bash
orbcode remote /tmp/orbcode.sock --token "$TOKEN"
orbcode remote ws://127.0.0.1:8080 --token "$TOKEN"
```

Remote mode starts no embedded local core: sessions, tools, permissions,
settings, and streams come from the server. A local/remote TUI can supervise
experimental persistent goals when both sides negotiate the capability.

### Client libraries and deferred products

The workspace contains in-process, NDJSON, WebSocket, child-stdio, and reviewed
OpenSSH transport infrastructure plus generated TypeScript contracts. These are
building blocks, not a packaged Desktop app or `remote --ssh` product. Desktop,
SSH CLI, remote-control bridge, voice, and computer use remain deferred. See
the [deferred product decision](plans/desktop-and-ssh-products.md).

Use [duplex stream-JSON](stream-json.md) when a program owns one CLI process and
only needs turn/control correlation. Use app-server when it needs a longer-lived
multi-session facade and accepts the experimental protocol commitment.
