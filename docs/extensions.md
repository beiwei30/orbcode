# Extensions and customization

[English](extensions.md) · [简体中文](zh-CN/extensions.md)

Orb Code discovers compatible instructions, commands, agents, skills, output
styles, hooks, keybindings, and installed plugins. Project-supplied executable
customization requires a trusted project and can be restricted by managed
policy.

## Instructions and memory

Orb Code builds the instruction context from managed, user, project, and
directory-scoped sources. Common locations are:

- `CLAUDE.md` and `.claude/CLAUDE.md` in or above the working directory.
- `<home>/CLAUDE.md` for user instructions.
- `<home>/rules/*.md` and `.claude/rules/*.md` for additional rules.
- Directory-scoped `.claude/CLAUDE.md` files as tools enter nested paths.

Use `orbcode context`, `/instructions`, and `/memory` to inspect what was
loaded. Auto-memory is controlled by `autoMemoryEnabled`; the TUI owns creation
and updates of its memory file.

## Custom slash commands

Markdown files under `<home>/commands/` and `.claude/commands/` become slash
commands. The file name supplies the command name and the body supplies the
prompt template. Enabled plugins can contribute commands as well. Command
availability is shown in slash completion; avoid naming a custom command after
a built-in command.

## Agents

Agent definitions live in `<home>/agents/*.md` and `.claude/agents/*.md` as
Markdown with YAML frontmatter. Definitions can specify a description, model,
tools, permission mode, skills, memory, background behavior, and hooks; the body
is the agent prompt.

Project definitions outrank user definitions, which outrank built-ins. Plugin
agents are namespaced to avoid collisions. Agent-authored `Stop` hooks are
executed as `SubagentStop`, matching the child loop's event.

Use `/agents` to inspect the resolved catalog. The `Agent` tool runs a local
subagent synchronously; durable asynchronous work is represented by background
tasks/workflows instead.

## Skills

A skill is a directory containing `SKILL.md`:

```text
.claude/skills/release-check/
├── SKILL.md
├── scripts/
└── references/
```

The frontmatter names and describes the skill; the body contains operating
instructions. The `Skill` tool loads it on demand and `/skills` shows discovery
and warnings.

Resolution priority is project, plugin, user, bundled, then MCP prompt skills.
MCP prompts are only exposed as skills when their server is trusted. Plugin and
MCP names are scoped so one extension cannot silently replace another.

## Output styles

`<home>/output-styles/*.md` and `.claude/output-styles/*.md` define prompt
styles. Built-ins include `default`, `Explanatory`, and `Learning`. Project
styles outrank user and plugin definitions; plugin values are namespaced.
Choose one with `/output-style` or the `outputStyle` setting. Managed policy can
lock the choice.

## Hooks

Command hooks are configured in the `hooks` settings object and may also come
from agents, skills, or enabled plugins. Implemented events are:

| Event | When it runs |
| --- | --- |
| `UserPromptSubmit` | After user input is accepted, before the provider turn. |
| `PreToolUse` | Before a permitted tool is dispatched. |
| `PostToolUse` | After successful tool completion. |
| `PostToolUseFailure` | After tool failure; may attach context and recommend retry. |
| `Stop` | When the main agent is about to stop. |
| `StopFailure` | When stop-hook processing fails. |
| `SubagentStop` | When a child agent is about to stop. |

The runtime understands compatible decisions such as allow/deny/ask,
`updatedInput`, `additionalContext`, stop feedback, and failure retry guidance.
A deny still wins after a `PreToolUse` input rewrite; hooks cannot rewrite an
input to evade configured or remembered denials.

Project and local settings hooks do not run for an untrusted project. Managed
policy can require managed-only hooks, restrict HTTP hook URLs, and constrain
which environment variables an HTTP hook receives. Events such as
`SessionStart`, `SessionEnd`, `PreCompact`, and `Notification` are not
implemented. Use `/hooks` to inspect the effective set.

Hooks execute code with your account's authority. Review commands and never
commit secrets in hook configuration.

## Keybindings, theme, and statusline

`<home>/keybindings.json` overlays the built-in keymap. Configurable actions are
limited to behavior-safe TUI operations such as transcript/todo/job toggles,
history search, input clearing, and line start/end. `/keybindings` shows the
loaded map and warnings.

Use `/theme` for auto/dark/light, daltonized, and ANSI palettes; `/vim` switches
between normal and Vim input modes. `statusline.command` is client-owned and
runs at the configured refresh interval (default 30 seconds).

## Plugins

The experimental plugin loader reads `<home>/plugins/installed_plugins.json`
in its supported v1 or v2 shape and then applies `enabledPlugins` from settings.
An installed plugin can contribute:

- commands, agents, skills, hooks, and output styles;
- `.mcp.json` servers;
- tools declared by the plugin manifest.

The manifest is `.claude-plugin/plugin.json` or a top-level `plugin.json`.
Plugin tools use provider-facing names such as
`plugin__<plugin>__<tool>`. Other contributed names are scoped where needed to
prevent replacement of built-ins or another plugin's surface.

Installing a plugin means trusting its code, hooks, MCP processes, and tool
adapters. Orb Code currently consumes an installed-plugin index; it does not
ship a marketplace installation UI. Managed `strictPluginOnlyCustomization`
can require selected customization surfaces to come only from plugins.
