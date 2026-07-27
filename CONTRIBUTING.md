# Contributing to Orb Code

Thanks for taking the time. Orb Code is alpha software with an unusual
constraint — it aims for byte-level compatibility with the TypeScript Claude Code
CLI — so a few things that look like cleanup are actually breaking changes. This
page is the short version; [`AGENTS.md`](AGENTS.md) is the full contributor guide
and [`CLAUDE.md`](CLAUDE.md) is the architecture reference.

## Before you start

- **Small fixes** — bugs, docs, tests — just open a pull request.
- **Anything larger** — a new tool, a provider, a change to the transcript
  format or the wire protocol — please open an issue first. Compatibility
  constraints make some designs unavailable, and it is cheaper to find that out
  before you write the code.

## Getting set up

You need a current stable Rust toolchain. The workspace is edition 2024, so 1.85
is the floor imposed by the edition itself; there is no declared MSRV and CI
builds on whatever `stable` is, so if you are on an older release, update before
reporting a build failure.

```sh
git clone https://github.com/beiwei30/orbcode
cd orbcode
cargo build
cargo run -p orbcode -- --help
```

## The one command that matters

```sh
scripts/check.sh
```

This is the canonical gate, and CI runs this exact script: rustfmt, clippy over
all targets, `cargo check` (with and without default features), the public API
surface audit, the brand audit, then the full workspace test suite. If it is
green locally it will be green in CI.

While iterating:

```sh
scripts/check.sh --quick              # skip tests
scripts/check.sh --crate orbcode-core # one crate's tests
cargo test -p orbcode-config settings # one test by name
```

Two suites are intentionally outside that gate and run in their own workflows:
the `#[ignore]d` PTY end-to-end tests (`scripts/check.sh --pty-e2e`, which needs
a quiet, uncontended machine) and the cross-platform build/packaging smoke.

## Things that will fail review

- **Renaming a TypeScript-CLI compatibility name.** `CLAUDE_CONFIG_DIR`,
  `ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN`, `~/.claude`, `settings.json`,
  `history.jsonl` and friends are a contract with the TypeScript CLI, not our
  branding. Our own env keys are `ORBCODE_*`. `scripts/audit-brand.sh` fails
  both on a leaked pre-rename name and on a compatibility name disappearing.
- **Editing a golden fixture to make a test pass.** The files under
  `compat-fixtures/fixtures/` and `tui/testdata/` are captured output. If your
  change moves them, the on-disk or on-wire format changed — say so explicitly in
  the PR and explain why it is safe. Two of the TUI goldens embed a
  width-truncated absolute path, so even a string-length change can break them.
- **Replacing the permission rule parser with string matching.** Bash rules are
  parsed into an AST (`config/src/permission_rules/`) precisely so that
  `Bash(git commit:*)` cannot be fooled by shell structure.
- **Hand-formatting.** rustfmt is the authority; `cargo fmt --all` before you
  push.

## Where code goes

The crates form a strict dependency DAG (`protocol` → `config` →
`model-provider` → `session-store` → `mcp` → `tools` → `core` → `app-server` →
`tui` → `cli`). Put a shared type in the lowest crate that needs it — usually
`protocol` — and re-export it through that crate's `lib.rs`.

Keep unit tests in the affected file's `#[cfg(test)] mod tests`. Use a
crate-level `tests/` directory only for public integration flows. For provider
errors, retries and fallback, drive behaviour through a `mock://` URL (the
`mock-provider` feature) rather than special-casing prompt text.

## Commits and pull requests

Short, imperative commit subjects under about 72 characters, describing the
change rather than the activity ("Reject managed-locked setting mutations", not
"fix bug"). In the pull request, describe the user-visible change, list the
verification commands you ran with their results, and link the issue if there is
one. Include a terminal screenshot only for a meaningful TUI behaviour change.

## Security

Please do not open a public issue for a vulnerability — see
[SECURITY.md](SECURITY.md). And never commit provider tokens, transcripts, or
local runtime state; when testing auth flows, use disposable values and keep
state outside the repository.

By contributing you agree that your contributions are licensed under the
[Apache-2.0](LICENSE) license.
