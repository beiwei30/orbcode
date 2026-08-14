# Permissions and sandboxing

[English](permissions.md) · [简体中文](zh-CN/permissions.md)

Permissions decide whether a tool may run. Sandboxing limits what a permitted
`Bash` process can do at the operating-system level. They are complementary:
an allow decision does not create a sandbox, and a sandbox does not grant a
tool permission.

## Interactive presets

The TUI exposes three complete policies, plus Plan mode. Use `/permissions` or
`Shift+Tab` to change them.

| Preset | CLI mode | Boundary handling | Sandbox |
| --- | --- | --- | --- |
| Ask for approval | `default` | Ask the user before network, external side effects, sandbox escalation, or access outside allowed roots. | Workspace write, network off. |
| Approve for me | `auto` | Apply the runtime's automatic boundary review. | Workspace write, network off. |
| Full Access | `bypassPermissions` | Never ask; tools may cross boundaries. | `danger-full-access`, network on. |
| Plan | `plan` | Hide execution tools. | No model-authored execution. |

Compatibility aliases `acceptEdits` and `dontAsk` parse as `default` and
`bypassPermissions`. Managed policy can disable Full Access; attempts then stay
on a safer preset.

Explicit CLI/environment restrictions retain their provenance so an implicitly
selected default preset does not silently weaken them.

## Permission rules

Rules live under `permissions.allow`, `permissions.ask`, and
`permissions.deny`, or come from `--allowed-tools` and `--disallowed-tools`.
Precedence is always `deny` > `ask` > `allow`.

```json
{
  "permissions": {
    "allow": [
      "Read",
      "Grep",
      "Bash(cargo check:*)",
      "mcp__issues__search"
    ],
    "ask": ["Bash(git push:*)"],
    "deny": ["Read(./secrets/**)", "Bash(rm:*)"],
    "additionalDirectories": ["../shared-lib"]
  }
}
```

Rules may name an entire tool (`Read`) or constrain its argument using the
Claude Code-compatible parenthesized form. Lists passed on the CLI may be comma
or space separated; parentheses are preserved while splitting.

`Bash` rules are evaluated against a tree-sitter Bash syntax tree. Pipelines,
subshells, command substitutions, operators, and compound commands cannot evade
a deny by merely changing whitespace. Keep rules narrow and test surprising
cases with `orbcode tool Bash '{"command":"..."}'`.

Remembered interactive approvals become session rules. Managed permission
rules remain authoritative and can be configured as the only accepted source.

## Workspace boundary

The working directory and every `--add-dir` / `permissions.additionalDirectories`
entry form the allowed roots. Path-aware tools can operate inside them under the
Ask/Auto presets; paths outside require boundary review. Symlink and path
normalization checks prevent simple traversal around the boundary.

`ORBCODE_TRUSTED_PROJECT=0` marks the working directory untrusted. This disables
project-supplied hooks; it does not mean arbitrary tool execution becomes safe.

## Master switches

- `--allow-tools true|false` / `ORBCODE_ALLOW_TOOLS` controls model-authored
  local tools and mutation.
- `--allow-network true|false` / `ORBCODE_ALLOW_NETWORK` controls network-backed
  tools such as `WebFetch` and `WebSearch`.
- `ORBCODE_PROVIDER_NETWORK` independently controls provider API traffic.

Provider access and web-tool access are separate so a model call can be allowed
while outbound tool traffic is blocked.

## OS sandbox modes

Select a Bash sandbox per invocation:

```bash
orbcode --sandbox-mode workspace-write --sandbox-network false \
  --add-dir ../shared-lib -p "run the focused tests"
```

| Mode | Behavior |
| --- | --- |
| `danger-full-access` | No OS sandbox. This is the low-level configuration default and the Full Access preset. |
| `workspace-write` | Read/write within allowed roots; boundary and network policy are projected into the platform runner. |
| `read-only` | Filesystem writes are blocked by the platform runner. |

macOS uses `sandbox-exec`. Linux uses Bubblewrap and fails closed when `bwrap`
is missing. The Windows runner's arguments are tested, but host validation is
still experimental. Run `orbcode doctor` to see which runner is available.

The persistent `sandbox` object controls the TUI's local sandbox preference:

```json
{
  "sandbox": {
    "enabled": true,
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    "excludedCommands": ["docker:*"],
    "filesystem": {
      "allowWrite": ["./tmp"],
      "denyWrite": ["./secrets"],
      "denyRead": ["./private"],
      "allowRead": ["./private/public.md"]
    },
    "network": {
      "allowedDomains": ["example.com"],
      "allowUnixSockets": ["/tmp/service.sock"],
      "allowAllUnixSockets": false,
      "allowLocalBinding": false,
      "httpProxyPort": 8080,
      "socksProxyPort": 1080
    }
  }
}
```

`allowUnsandboxedCommands: false` is strict: a model command must run in the
sandbox unless it matches `excludedCommands`. Exclusion means deliberately run
outside the sandbox, not deny the command, so pair it with permission rules.

## MCP trust is a second gate

Every MCP call needs both:

1. A matching permission, such as `mcp__issues__search`.
2. A trusted server.

Trust cannot bypass a missing allow decision, and an allow rule cannot bypass
unknown/denied trust. Use `orbcode mcp trust`, `distrust`, or `untrust`. The
trust decision is persisted in compatible settings keys; revoking trust shuts
down the active stdio client.

## Safe unattended use

Prefer an explicit, minimal policy over Full Access:

```bash
orbcode --allow-tools true \
  --allowed-tools "Read,Grep,Bash(cargo check:*),Bash(cargo test:*)" \
  --disallowed-tools "Bash(git push:*),Bash(rm:*)" \
  --sandbox-mode workspace-write --sandbox-network false \
  -p "diagnose the failing test"
```

Use a disposable `ORBCODE_HOME` in CI, do not store provider tokens in the
repository, and treat enabled hooks/plugins/MCP servers as executable code.
