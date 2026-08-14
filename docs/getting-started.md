# Getting started

[English](getting-started.md) · [简体中文](zh-CN/getting-started.md)

Orb Code is an alpha-stage native Rust reimplementation of the Claude Code CLI.
It ships as one `orbcode` binary and, by default, uses the same `~/.claude`
settings and session formats.

## Requirements

- A rustup-managed stable Rust toolchain with Rust 2024 support (1.85 or newer).
- `git` for repository context.
- `ripgrep` (`rg`) for the fastest `Grep` implementation. Orb Code has a slower
  built-in fallback.
- `sandbox-exec` on macOS (preinstalled) or `bwrap` on Linux only when you turn
  on OS sandboxing.

Run `orbcode doctor` after installation to inspect the active home, provider,
credentials, tool permissions, sandbox runner, sessions, MCP servers, and
external binaries. Network probes are off by default.

## Install from source

There is no crate or npm package release yet.

```bash
git clone https://github.com/beiwei30/orbcode.git
cd orbcode

# Install on PATH.
cargo install --path cli

# Or leave a release binary at target/release/orbcode.
cargo build --release -p orbcode
```

Tag builds publish archives for Linux x86-64, Apple Silicon macOS, and Windows
x86-64. Until the project has a stable release channel, treat binaries and
on-disk additions as alpha software. To produce the same archive locally, run
`scripts/package-release.sh --out-dir dist`.

From a checkout, you can use `cargo run -p orbcode -- <arguments>` or
`scripts/run.sh <arguments>` instead of installing.

## Authenticate

### Anthropic

Anthropic is the default provider. Use an API key or the compatible OAuth token
environment variable:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
# Alternatively: export CLAUDE_CODE_OAUTH_TOKEN="..."

orbcode -p "reply with OK"
```

`ORBCODE_ANTHROPIC_API_KEY`, `ORBCODE_ANTHROPIC_AUTH_TOKEN`, and
`ORBCODE_OAUTH_TOKEN` are the corresponding Orb Code-prefixed names.

### OpenAI API

```bash
export OPENAI_API_KEY="sk-..."
orbcode --provider openai -p "reply with OK"
```

Set `OPENAI_BASE_URL` for a compatible Chat Completions endpoint and
`OPENAI_MODEL` to select its model. OpenAI support is beta because compatible
servers vary and server-side token counting is unavailable.

### ChatGPT/Codex subscription

Orb Code can use a ChatGPT subscription through an experimental, separate
login path:

```bash
# Browser callback with PKCE.
orbcode auth login --provider openai --method chatgpt

# Device-code flow for a headless host.
orbcode auth login --provider openai --method chatgpt --device-code

orbcode auth status
env -u OPENAI_API_KEY -u ORBCODE_OPENAI_API_KEY \
  orbcode --provider openai -p "reply with OK"
```

Credentials are stored in `<home>/auth.json`. Orb Code does not read or modify
`~/.codex/auth.json`. An explicit OpenAI API key wins over the saved subscription
login. Use `orbcode auth logout --provider openai` to remove it.

Gemini and Grok appear in the provider enum for compatibility work but have no
active adapters; selecting either returns `unsupported_provider`.

## First run

Run the interactive terminal UI from the repository you want to work on:

```bash
cd my-project
orbcode
```

Enter a prompt and press Enter. Use `/help` for commands and keybindings,
`/permissions` to inspect the active permission preset, and `/doctor` for the
runtime health report. `Shift+Tab` cycles Ask for approval, Approve for me, Full
Access, and Plan.

Useful non-interactive forms:

```bash
orbcode -p "explain this repository"
orbcode -p --output-format json "list the workspace crates"
orbcode --continue                    # latest workspace session in the TUI
orbcode sessions                      # persisted sessions
```

The default interactive preset grants common workspace-scoped operations and
asks before crossing its boundary. A raw headless configuration is more
conservative unless you select a preset or add rules. Review [permissions and
sandboxing](permissions.md) before granting broad unattended access.

## Keep Orb Code state separate

By default, Orb Code uses `~/.claude` so compatible settings and transcripts are
shared. To opt into a separate home, create it before launch:

```bash
mkdir ~/.orbcode
```

No data is copied. An empty `~/.orbcode` therefore has no credentials, settings,
or sessions. Remove it to return to the shared default, or set `ORBCODE_HOME` to
an explicit directory. See [home directory resolution](configuration.md#home-directory).

## Next steps

- Learn the [TUI, sessions, and background jobs](user-guide.md).
- Set durable defaults in [configuration](configuration.md).
- Restrict tools with [permissions and sandboxing](permissions.md).
- Connect external tools with [MCP](mcp.md).
- Use Orb Code from scripts with the [CLI reference](cli-reference.md) and
  [stream-JSON guide](stream-json.md).
