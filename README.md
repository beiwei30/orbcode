# Orb Code

[English](README.md) · [简体中文](README.zh-CN.md)

A native Rust reimplementation of the Claude Code CLI, shipped as one
`orbcode` binary.

[![CI](https://github.com/beiwei30/orbcode/actions/workflows/ci.yml/badge.svg)](https://github.com/beiwei30/orbcode/actions/workflows/ci.yml)
![Status: alpha](https://img.shields.io/badge/status-alpha-orange)
![Rust 2024](https://img.shields.io/badge/rust-2024-informational)
[![Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

Orb Code aims for byte-level compatibility with the TypeScript CLI where it
matters: JSONL transcripts, settings layering, `CLAUDE.md` and `.mcp.json`
discovery, common CLI flags, and stream-JSON events. By default it uses the same
`~/.claude` home, so compatible sessions and configuration need no migration.

> **Alpha (`0.0.1`).** There is no crate/npm package or stable release channel
> yet. Interfaces can change between commits. Check the honest
> [feature-status matrix](docs/feature-status.md) before depending on an
> experimental surface.

## Why Orb Code?

- One native binary: no Node.js runtime at launch.
- Interactive TUI plus text, JSON, and duplex stream-JSON automation.
- Anthropic, OpenAI-compatible APIs, and experimental ChatGPT/Codex
  subscription login.
- Workspace-aware tools, structured Bash permissions, OS sandboxing, hooks,
  agents, skills, plugins, background jobs, and MCP.
- A layered Rust workspace with compatibility fixtures and focused integration
  tests for contributors.

## Install

Orb Code currently builds from source and requires Rust 1.85 or newer:

```bash
git clone https://github.com/beiwei30/orbcode.git
cd orbcode
cargo install --path cli
```

Runtime helpers are `git` and preferably `ripgrep`. OS sandboxing additionally
uses `sandbox-exec` on macOS or `bwrap` on Linux. Run `orbcode doctor` to inspect
your setup.

## Start in a minute

Anthropic is the default provider:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."

orbcode                                      # interactive TUI
orbcode -p "explain this repository"         # one headless turn
orbcode --continue                           # latest workspace session
orbcode doctor
```

Or use a ChatGPT/Codex subscription without an OpenAI API key:

```bash
orbcode auth login --provider openai --method chatgpt
# Headless host: add --device-code

orbcode --provider openai -p "reply with OK"
```

Inside the TUI, start with `/help`, `/permissions`, and `/doctor`. `Shift+Tab`
cycles Ask for approval, Approve for me, Full Access, and Plan.

For source-checkout use without installation:

```bash
cargo run -p orbcode -- -p "summarize the workspace"
# or: scripts/run.sh -p "summarize the workspace"
```

Read [Getting started](docs/getting-started.md) for auth options, state-home
selection, and the first-run walkthrough.

## Documentation

| I want to… | Read |
| --- | --- |
| Install and run my first prompt | [Getting started](docs/getting-started.md) |
| Learn the TUI, sessions, tools, and jobs | [User guide](docs/user-guide.md) |
| Configure providers, models, settings, and proxies | [Configuration](docs/configuration.md) |
| Control tool access safely | [Permissions and sandboxing](docs/permissions.md) |
| Add agents, skills, hooks, or plugins | [Extensions](docs/extensions.md) |
| Connect MCP servers | [MCP](docs/mcp.md) |
| Script the CLI or consume NDJSON | [CLI reference](docs/cli-reference.md) · [Stream-JSON](docs/stream-json.md) |
| Integrate an editor or thin client | [ACP and app-server integrations](docs/integrations.md) |
| Check support or diagnose a failure | [Feature status](docs/feature-status.md) · [Troubleshooting](docs/troubleshooting.md) |

The complete English manual is in [`docs/`](docs/README.md); the Chinese mirror
is in [`docs/zh-CN/`](docs/zh-CN/README.md).

## Contributing

Contributions are welcome, especially focused compatibility fixtures, missing
provider/tool behavior, TUI polish, cross-platform sandbox validation, and
documentation corrections backed by code or tests.

```bash
scripts/check.sh          # fmt, clippy, check, tests
scripts/check.sh --quick  # skip tests while iterating
```

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a PR. Architecture and
crate ownership are documented in [CLAUDE.md](CLAUDE.md); repository conventions
are in [AGENTS.md](AGENTS.md). Please use [SECURITY.md](SECURITY.md) for private
vulnerability reports.

## License and affiliation

[Apache-2.0](LICENSE).

Orb Code is an independent, unofficial reimplementation. It is not affiliated
with, authorized by, endorsed by, or sponsored by Anthropic. “Anthropic”,
“Claude”, and “Claude Code” are trademarks of Anthropic PBC and are used only to
identify the compatible CLI, formats, and API. Project questions belong in this
repository, not Anthropic support.
