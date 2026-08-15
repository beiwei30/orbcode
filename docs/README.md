# Orb Code documentation

[English](README.md) · [简体中文](zh-CN/README.md)

This manual is organized around what you are trying to do. The executable is
the authority for flags (`orbcode --help`), while these pages explain how the
pieces work together.

## Start here

- [Getting started](getting-started.md) — install, authenticate, and run your
  first interactive or headless prompt.
- [User guide](user-guide.md) — the TUI, tools, sessions, background jobs,
  context, plans, and persistent goals.
- [Troubleshooting](troubleshooting.md) — diagnose auth, sandbox, MCP, home
  directory, and session problems.

## Configure and extend

- [Configuration](configuration.md) — home resolution, settings layers,
  supported settings, environment variables, project files, and proxies.
- [Permissions and sandboxing](permissions.md) — presets, rules, project
  boundaries, OS sandboxes, and MCP's second trust gate.
- [Extensions](extensions.md) — instructions, agents, skills, commands, output
  styles, hooks, keybindings, and plugins.
- [MCP](mcp.md) — configure servers and use tools, resources, prompts, OAuth,
  trust, and hot reload.

## Automate and integrate

- [CLI reference](cli-reference.md) — global options, every command family,
  recipes, and exit codes.
- [Headless and stream-JSON](stream-json.md) — output modes, duplex NDJSON,
  controls, permission callbacks, and event compatibility.
- [Integrations](integrations.md) — ACP, Zed, and the experimental app-server
  protocol.

## Project status and compatibility

- [Feature status](feature-status.md) — what is stable, experimental, deferred,
  or deliberately unsupported.
- [Claude Code compatibility](compatibility.md) — state shared without
  conversion and known differences.
- [Interactive questions](interactive-questions.md) — the capability-gated
  `AskUserQuestion` contract.
- [Persistent goals](persistent-goals.md) — goal state, supervision, recovery,
  and client support.

## Contributor and design documentation

- [Contributing](../CONTRIBUTING.md) — development setup and the verification
  workflow.
- [Architecture](../CLAUDE.md) — crate boundaries and request flow.
- [Repository guidelines](../AGENTS.md) — detailed contributor conventions.
- [Settings architecture](settings-architecture.md) — typed ownership and raw
  JSON boundaries.
- [ACP support matrix](acp-support.md) and [Zed smoke guide](acp-zed-smoke.md).
- [`plans/`](plans/) — design records and deferred product plans; these are not
  user-facing promises.

Documentation should describe the current implementation, not an intended
future surface. When behavior and prose disagree, check the command's `--help`,
the owning crate's public module documentation, and focused tests, then fix the
documentation.
