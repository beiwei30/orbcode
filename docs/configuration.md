# Configuration

[English](configuration.md) · [简体中文](zh-CN/configuration.md)

Orb Code deliberately understands Claude Code-compatible settings while adding
its own `ORBCODE_*` controls. Use the smallest scope that fits: a process
environment variable for secrets, local settings for one checkout, project
settings for shareable policy, and user settings for personal defaults.

## Home directory

The active home is resolved in this order:

1. Non-empty `ORBCODE_HOME`.
2. Non-empty `CLAUDE_CONFIG_DIR`.
3. `~/.orbcode`, but only when that directory already exists.
4. `~/.claude`.

Using `~/.claude` by default lets both CLIs share compatible settings, prompt
history, credentials, MCP configuration, and JSONL transcripts. Orb Code never
creates `~/.orbcode` automatically. Creating it is an explicit opt-in to empty,
separate state; nothing is copied. `orbcode doctor` warns when an empty
`~/.orbcode` shadows a populated `~/.claude`.

## Settings layers

Persisted layers merge from lowest to highest precedence:

1. User: `<home>/settings.json`.
2. Project: `<project>/.claude/settings.json`.
3. Local: `<project>/.claude/settings.local.json`.
4. Managed: enterprise `managed-settings.json` and sorted drop-ins.

Later scalar values replace earlier values. Collections such as environment
entries, permission rules, additional directories, and hooks are combined
according to their typed ownership rules. Managed policy can lock a top-level
key; in-app mutations then fail with a managed-policy message.

`--settings <FILE_OR_JSON>` adds a per-invocation overlay. It accepts a file or
inline JSON beginning with `{`. The overlay handles model, environment,
permission rules, additional directories, and budget controls; it is not a
general writable fifth settings layer.

## Common settings

This example uses the user/project/local schema implemented by the config and
app-server layers:

```json
{
  "model": "sonnet",
  "theme": "auto",
  "editorMode": "normal",
  "alwaysThinkingEnabled": false,
  "outputStyle": "default",
  "autoMemoryEnabled": true,
  "env": {
    "PROVIDER_TYPE": "anthropic"
  },
  "permissions": {
    "allow": ["Read", "Grep", "Bash(cargo check:*)"],
    "ask": ["Bash(git push:*)"],
    "deny": ["Read(./secrets/**)"],
    "additionalDirectories": ["../shared"]
  },
  "maxBudgetUsd": 2.0,
  "maxBudgetUsdStrictUnknownPricing": true,
  "statusline": {
    "command": "git branch --show-current",
    "refreshInterval": 30
  }
}
```

Supported themes are `auto`, `dark`, `light`, `dark-daltonized`,
`light-daltonized`, `dark-ansi`, and `light-ansi`. `editorMode` accepts
`normal`/`emacs` or `vim`. The statusline refresh interval is clamped to its
valid 1–3600 second range by falling back to 30 seconds when invalid.

`maxBudgetUsd` stops priced API work at the configured spend ceiling.
Subscription requests and unknown/custom model prices cannot always be mapped
to API dollars. Set `maxBudgetUsdStrictUnknownPricing` to reject that uncertainty
instead of warning and continuing.

See [Permissions and sandboxing](permissions.md) for the rule and sandbox
fields, [Extensions](extensions.md) for hooks/plugins/styles, and [MCP](mcp.md)
for `mcpServers`.

## Provider and model selection

Precedence starts with the explicit `--provider`, then `PROVIDER_TYPE` from the
process environment and settings `env`. Compatibility fallbacks are lower
priority. The default is Anthropic.

There is no `--model` flag. Select a model with `/model`, `model` in settings,
or a provider model environment variable. `opus`, `sonnet`, and `haiku` are
family aliases resolved for the active provider. ChatGPT subscription auth uses
its Responses endpoint and defaults to `gpt-5.6-sol` when no model is explicit.

```json
{
  "model": "gpt-4o",
  "env": {
    "PROVIDER_TYPE": "openai"
  }
}
```

Keep API keys out of committed settings. A process API key also takes
precedence over stored ChatGPT subscription credentials.

## Environment variables

For compatibility aliases, resolution is: canonical process variable, alias
process variable, canonical settings `env`, then alias settings `env`.

| Canonical | Compatible alias | Purpose |
| --- | --- | --- |
| `PROVIDER_TYPE` | `ORBCODE_PROVIDER`, `CLAUDE_CODE_USE_OPENAI=true` | Select `anthropic` or `openai`. |
| `ORBCODE_ANTHROPIC_API_KEY` | `ANTHROPIC_API_KEY` | Anthropic API key. |
| `ORBCODE_ANTHROPIC_AUTH_TOKEN` | `ANTHROPIC_AUTH_TOKEN` | Anthropic bearer token. |
| `ORBCODE_OAUTH_TOKEN` | `CLAUDE_CODE_OAUTH_TOKEN` | Anthropic-compatible OAuth token. |
| `ORBCODE_OPENAI_API_KEY` | `OPENAI_API_KEY` | OpenAI-compatible API key. |
| `ORBCODE_ANTHROPIC_BASE_URL` | `ANTHROPIC_BASE_URL` | Anthropic endpoint override. |
| `ORBCODE_OPENAI_BASE_URL` | `OPENAI_BASE_URL` | OpenAI-compatible endpoint override; ignored for ChatGPT subscription auth. |
| `ORBCODE_ANTHROPIC_MODEL` | `ANTHROPIC_MODEL` | Anthropic model override. |
| `ORBCODE_OPENAI_MODEL` | `OPENAI_MODEL` | OpenAI model override. |
| `ORBCODE_MAX_OUTPUT_TOKENS` | `CLAUDE_CODE_MAX_OUTPUT_TOKENS` | Output-token cap. |
| `ORBCODE_MAX_CONTEXT_TOKENS` | `CLAUDE_CODE_MAX_CONTEXT_TOKENS` | Context-window cap. |
| `ORBCODE_AUTO_COMPACT_WINDOW` | `CLAUDE_CODE_AUTO_COMPACT_WINDOW` | Auto-compaction threshold. |
| `ORBCODE_API_TIMEOUT_MS` | `API_TIMEOUT_MS` | Provider HTTP timeout. |
| `ORBCODE_API_MAX_RETRIES` | `API_MAX_RETRIES` | Provider retry budget. |
| `ORBCODE_WEB_ALLOWED_DOMAINS` | `CLAUDE_CODE_WEB_ALLOWED_DOMAINS` | Web allowlist. |
| `ORBCODE_WEB_BLOCKED_DOMAINS` | `CLAUDE_CODE_WEB_BLOCKED_DOMAINS` | Web denylist. |

Frequently used Orb Code-only variables include `ORBCODE_HOME`,
`ORBCODE_FALLBACK_PROVIDER`, `ORBCODE_MAX_RETRIES`, `ORBCODE_ALLOW_TOOLS`,
`ORBCODE_ALLOW_NETWORK`, `ORBCODE_ALLOWED_TOOLS`,
`ORBCODE_DISALLOWED_TOOLS`, `ORBCODE_SANDBOX_MODE`,
`ORBCODE_SANDBOX_NETWORK`, and `ORBCODE_TRUSTED_PROJECT`.

The complete compatibility alias table, including family-specific model keys,
custom headers/body/metadata, timeouts, retry delays, and tool timeouts, is kept
next to its tests in `config/src/env_compat.rs`. Diagnostic-only variables are
listed in [Troubleshooting](troubleshooting.md#diagnostic-switches).

## Proxies

Provider requests and ChatGPT login/refresh use destination-aware proxy
selection in this order:

1. Lowercase `https_proxy` / `http_proxy` in merged settings `env`.
2. Process `HTTPS_PROXY`, `HTTP_PROXY`, or `ALL_PROXY`, including lowercase
   forms.
3. `ORBCODE_PROXY`, `CLAUDE_CODE_PROXY`, or `ANTHROPIC_PROXY_URL`.
4. Static macOS System Configuration HTTP/HTTPS proxy and exceptions.
5. Direct connection.

HTTPS falls back to the HTTP proxy if needed. Loopback destinations stay
direct. Explicit proxies honor `no_proxy`/`NO_PROXY`; macOS exceptions support
wildcards and IP CIDRs. PAC JavaScript is not evaluated, so configure an
explicit proxy when the system only supplies PAC.

```json
{
  "env": {
    "https_proxy": "http://127.0.0.1:7890",
    "http_proxy": "http://127.0.0.1:7890",
    "no_proxy": "localhost,127.0.0.1,::1"
  }
}
```

## Project and home files

| Path | Purpose |
| --- | --- |
| `CLAUDE.md`, `.claude/CLAUDE.md` | Project and directory instructions. |
| `.claude/rules/*.md`, `<home>/rules/*.md` | Additional instruction rules. |
| `.claude/settings.json` | Shareable project settings. |
| `.claude/settings.local.json` | Local project settings; normally gitignored. |
| `.claude/agents/*.md`, `<home>/agents/*.md` | Agent definitions. |
| `.claude/skills/<name>/SKILL.md`, `<home>/skills/...` | Skills. |
| `.claude/commands/*.md`, `<home>/commands/*.md` | Custom slash commands. |
| `.claude/output-styles/*.md`, `<home>/output-styles/*.md` | Output styles. |
| `.mcp.json` | Project MCP servers. |
| `<home>/keybindings.json` | TUI keymap overrides. |
| `<home>/plugins/installed_plugins.json` | Installed-plugin index. |
| `<home>/auth.json` | Orb Code provider credentials. |
| `<home>/projects/<slug>/*.jsonl` | Session transcripts. |

Read [Extensions](extensions.md) before committing executable hooks or plugin
configuration. Project/local hooks are disabled when the project is untrusted.

## Managed policy

Enterprise managed settings can constrain models, remap model IDs, allow or
deny MCP servers, require managed-only hooks/permissions/MCP, disable bypass
permissions, require plugin-only customization, force a login method, and
restrict HTTP hook URLs and exported environment variables. Denies win and
managed keys are read-only to the TUI/app-server mutation APIs.

Relevant keys include `availableModels`, `modelOverrides`, `allowedMcpServers`,
`deniedMcpServers`, `allowManagedHooksOnly`,
`allowManagedPermissionRulesOnly`, `allowManagedMcpServersOnly`,
`strictPluginOnlyCustomization`, `forceLoginMethod`,
`permissions.disableBypassPermissionsMode`, `allowedHttpHookUrls`, and
`httpHookAllowedEnvVars`.

For ownership and raw-JSON compatibility boundaries, see
[Settings architecture](settings-architecture.md).
