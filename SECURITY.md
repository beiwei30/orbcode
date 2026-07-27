# Security Policy

## Supported versions

Orb Code is alpha software and there is no release train yet. Only the `main`
branch is supported: fixes land there, and there are no backports.

| Version | Supported |
| ------- | --------- |
| `main`  | Yes       |
| tagged `v0.0.x` builds | Only via the fix landing on `main` |

## Reporting a vulnerability

**Please do not open a public issue.** Use GitHub's private vulnerability
reporting: go to the [Security tab](https://github.com/beiwei30/orbcode/security)
and choose *Report a vulnerability*. That creates a private advisory thread
visible only to you and the maintainers.

Helpful things to include, roughly in order of usefulness:

- the commit SHA you tested (`orbcode --version` prints it),
- your OS and how you launched it (TUI, `-p/--print`, `serve`, an MCP client),
- the relevant configuration — permission mode, allow/deny rules, MCP servers,
  settings layers in play — since almost every boundary here is
  configuration-dependent,
- a minimal reproduction, and what you expected the boundary to do instead.

This is a small project maintained on a best-effort basis. Expect an
acknowledgement within about a week; please allow up to 90 days before public
disclosure, and tell us if you have a deadline of your own.

## What is in scope

The interesting boundaries in this codebase, and the ones worth reporting:

- **Permission bypass** — anything that gets a tool call executed when a deny
  rule should have stopped it, or without the approval prompt the configured
  permission mode requires. The bash rule parser
  (`config/src/permission_rules/`) is an AST over the real shell grammar
  specifically so that shell structure cannot smuggle a command past a rule; a
  string that defeats it is a bug in this class.
- **MCP trust bypass** — an untrusted or denied MCP server's tool being invoked.
  Trust and allow rules are meant to be independent gates: neither can substitute
  for the other.
- **Sandbox escape** — a sandboxed tool invocation reaching outside the sandbox
  on any host platform.
- **App-server transport auth bypass** — reaching the app server over a Unix
  socket or WebSocket without the auth token, or a WebSocket connection accepted
  from an `Origin` that should have been rejected.
- **SSRF guard bypass** — reaching link-local, loopback or internal addresses
  through `WebFetch` or the MCP OAuth flow when the guards
  (`tools/src/web_fetch.rs`, `mcp/src/oauth/ssrf.rs`) should have blocked it.
- **Credential exposure** — API keys or OAuth tokens written into transcripts,
  logs, terminal traces, error messages, or files with permissive modes.
- **Transcript or settings path traversal** — escaping the project directory
  under `~/.claude/projects/` via a crafted session id, project path, or
  settings value.

## What is not a vulnerability

- **A tool doing exactly what an allow rule permits**, including destructive
  commands. `Bash(rm:*)` in your settings means `rm` runs. Tool execution is off
  by default and every widening is a deliberate, local configuration act.
- **A prompt injection that results in a permission prompt.** The prompt is the
  boundary. Injection that causes execution *without* the prompt is in scope, and
  we want to hear about it.
- **Anything that requires an attacker who already has your user account** —
  write access to `~/.claude`, your settings files, or the process environment.
  Everything downstream of that is game over by design.
- **Vulnerabilities in the model providers themselves**, in the upstream
  TypeScript Claude Code CLI, or in third-party MCP servers you chose to trust.
- **A dependency advisory with no reachable path in this code.** Still worth an
  issue or a pull request bumping the dependency — just not an advisory.

## Disclosure

Once a fix is on `main`, we will publish the GitHub advisory with credit to the
reporter unless you would rather stay anonymous.
