# Model Context Protocol (MCP)

[English](mcp.md) · [简体中文](zh-CN/mcp.md)

Orb Code can load MCP servers from Claude Code-compatible files, enabled
plugins, and per-invocation overlays. Tools, resources, and prompts are
supported. Calls are protected by both permission and server trust.

## Configure a server

A project `.mcp.json` uses the standard `mcpServers` object:

```json
{
  "mcpServers": {
    "local-files": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
      "env": { "LOG_LEVEL": "warn" }
    },
    "issues": {
      "type": "streamable_http",
      "url": "https://mcp.example.com/mcp",
      "headers": { "X-Tenant": "${TENANT_ID}" }
    },
    "events": {
      "type": "websocket",
      "url": "wss://mcp.example.com/events"
    }
  }
}
```

String values support `${VAR}` and `${VAR:-default}` expansion. A missing
variable without a default is a load diagnostic, never a silent empty string.

Discovery sources, in merge order, include user `settings.json`, ancestor
`.mcp.json` files, project and local settings, repeated `--mcp-config` inputs,
and enabled plugin MCP definitions. Settings can disable configured servers.
Managed policy can allow, deny, or require managed-only servers.

Add a persistent registry entry from the CLI:

```bash
orbcode mcp add issues \
  --transport streamable-http \
  --endpoint https://mcp.example.com/mcp \
  --auth bearer-env:MCP_TOKEN \
  --summary "Issue tracker" --enabled

orbcode mcp remove issues
```

Auth specs accepted by `mcp add` are `none`, `bearer-env:VARIABLE`, and
`header:Name=Value`. Prefer an environment-backed bearer or OAuth over a literal
secret in shell history/settings.

## Transports

| Transport | Status | Notes |
| --- | --- | --- |
| `stdio` | Stable | Starts a local command with args, env, and optional cwd. |
| `streamable_http` | Stable | Canonical remote transport; JSON and SSE response modes plus MCP session management. |
| `http`, `https`, legacy `sse` | Compatibility aliases | Loaded as the remote HTTP family. |
| `websocket` | Beta | Real JSON-RPC over `ws://` or `wss://`. |

`orbcode mcp capabilities` prints the live transport inventory.

The legacy `sse` spelling accepted in persisted MCP configuration is only a
compatibility input normalized to the remote HTTP family. It is not ACP
`McpServer::Sse` support and does not implement the legacy endpoint-event plus
message-POST transport.

### Streamable HTTP protocol boundary

The retained client proposes MCP `2024-11-05` during initialization and then
uses the version selected by the server in subsequent protocol headers. It is a
request/response client: each JSON-RPC call is sent with POST, and that POST may
return either JSON or a finite SSE response body containing the correlated
response. Session IDs, expiry recovery, and DELETE shutdown are supported.

A standalone GET event reader, `Last-Event-ID` recovery, server notification
routing, and subscription/listen APIs are not advertised or implemented. A
2025-era standalone GET client remains deferred until a supported server has a
reproducible workflow that POST responses cannot satisfy. A modern 2026-era
subscription lifecycle requires a separate protocol-era plan rather than an
implicit extension of the current transport.

## Inspect and use servers

```bash
orbcode mcp servers
orbcode mcp diagnose issues
orbcode mcp tools issues
orbcode mcp resources issues
orbcode mcp read issues 'issue://123'
orbcode mcp prompts issues
orbcode mcp prompt issues triage '{"severity":"high"}'
orbcode mcp call issues search '{"query":"is:open"}'
```

`diagnose` probes configuration without the runtime call permission gate so you
can separate connection/auth failures from policy failures. It does not grant
future calls.

Model-facing tool names are `mcp__<server>__<tool>`. Trusted prompt definitions
also appear in TUI slash suggestions and the skill catalog. Resource results and
prompt messages keep their MCP content types rather than being flattened into
tool calls.

## Trust and permissions

New servers begin with unknown trust. Manage it explicitly:

```bash
orbcode mcp trust issues       # approved
orbcode mcp distrust issues    # denied
orbcode mcp untrust issues     # clear; ask again next time
```

A call succeeds only when the server is trusted and the normal rule engine
allows its `mcp__...` name. Trust alone cannot grant tool execution. Permission
alone cannot start an unknown or denied server. Session-owned MCP servers keep
session-local trust; durable definitions persist trust in compatible settings
and the registry store.

## OAuth

OAuth tokens are stored separately from server definitions and redacted from
status/control output.

```bash
orbcode mcp auth status

# Import an existing token.
orbcode mcp auth login issues --access-token "$TOKEN" \
  --refresh-token "$REFRESH" --token-endpoint https://auth.example.com/token

# Device authorization.
orbcode mcp auth device-login issues --client-id orbcode-cli \
  --scope mcp.read --scope mcp.write

# Browser PKCE. Omit --client-id to try RFC 7591 dynamic registration.
orbcode mcp auth browser-login issues --scope mcp.read

orbcode mcp auth logout issues
```

Endpoint discovery fills omitted authorization/token/registration endpoints
where the server advertises them. Browser login uses PKCE and a loopback
callback. Public endpoints receive TLS/SSRF checks; deliberately local MCP
configurations retain their local-flow handling.

## Hot reload and failures

Configured files are watched and changed servers are added, removed, or
restarted. A configuration parse/expansion error is reported with its source
and path rather than partially applying an ambiguous server. Revoking trust
shuts down a live stdio client.

Troubleshooting sequence:

1. `orbcode mcp servers` for load/status/trust/auth summaries.
2. `orbcode mcp diagnose <server>` for transport and handshake details.
3. `orbcode mcp auth status` for token expiry/refresh readiness.
4. Check the matching `mcp__server__tool` permission.
5. Set `ORBCODE_DOCTOR_MCP_PROBE=1` only when you want `doctor` to contact
   configured servers.
