# Feature status

[English](feature-status.md) · [简体中文](zh-CN/feature-status.md)

Orb Code is alpha software. “Stable” below means implemented, test-covered, and
the intended path inside the current alpha—not a cross-version compatibility
guarantee.

| Level | Meaning |
| --- | --- |
| Stable | Intended supported path with focused tests. |
| Beta | Useful day to day; behavior or UX can still change. |
| Experimental | Shape can change without a protocol/version promise. |
| Deferred | Deliberately absent from user/model-visible surfaces. |

## Interfaces

| Surface | Status | Notes |
| --- | --- | --- |
| Interactive TUI | Beta | Chat, tools, permission/model/session pickers, rewind, diff, native transcript pager, themes, Vim mode, dynamic slash commands. |
| Headless text/JSON | Stable | One turn via `-p` or `prompt`. |
| Duplex stream-JSON | Beta | Typed NDJSON controls and permission/question callbacks. |
| Background prompt jobs | Stable | Queue, list, logs/follow, attach, cancel, recovery metadata. |
| Persistent goals | Experimental | Transcript-backed, client-supervised multi-turn continuation. |
| Direct tool invocation | Beta | Debug one registered tool through permissions. |
| ACP | Experimental | ACP v1 over stdio; capability-limited mappings are documented. |
| App-server/remote | Experimental | Protocol 1.0 over stdio, Unix socket, and WebSocket. |

## Providers

| Provider/path | Status | Notes |
| --- | --- | --- |
| Anthropic | Stable | Streaming Messages, thinking/interleaved thinking, token counting, API key/bearer/OAuth. |
| OpenAI-compatible API key | Beta | Streaming Chat Completions, effort, endpoint override; no server token count. |
| ChatGPT/Codex subscription | Experimental | Browser/device login, refresh, Responses reasoning/function calls, fixed subscription backend. |
| Gemini, Grok | Not implemented | Accepted enum values return `unsupported_provider`; no request adapter. |
| Retry/fallback/rate limit | Stable | Error normalization, retry-after, retry budget, fallback after eligible exhaustion. |

## Tools and orchestration

| Group | Status | Notes |
| --- | --- | --- |
| Read/Edit/Write/Glob/Grep/Bash/NotebookEdit | Stable | Workspace boundaries, stale-write checks, structured Bash permissions. |
| WebFetch/WebSearch | Stable | Domain policy and network permission; curl and DuckDuckGo HTML paths. |
| Plans, todos, tasks | Beta | Persistent plan state, task statuses/logs/cancel. Verification is a snapshot, not automatic proof. |
| Local Agent | Beta | Configured synchronous subagent and child-session tracking. |
| Skills/ToolSearch | Beta | Bundled/user/project/plugin and trusted MCP prompt discovery. |
| AskUserQuestion | Experimental | Visible only to clients with the required interaction capability. |
| LSP | Experimental | Heuristic workspace queries, not a language-server client. |
| Workflow | Experimental | Generated durable dynamic workflow. |
| Goal tools | Experimental | Available only in supervised persistent-goal turns. |

Deferred tool names are kept out of the registry and provider schema until a
closed-loop implementation exists: PowerShell, Cron variants, Monitor, Sleep,
Browser, RemoteTrigger, Teams, Vault, ReviewArtifact, SyntheticOutput,
Marketplace, PushNotification, ScheduleWakeup, EnterWorktree, and ExitWorktree.
Use `orbcode tools` as the live registry rather than a numeric claim.

## Configuration and security

| Surface | Status | Notes |
| --- | --- | --- |
| Settings layering/home compatibility | Stable | User → Project → Local → Managed; opt-in separate home. |
| Permission rules and Bash parsing | Stable | Deny > ask > allow; structured compound-command checks. |
| Managed policy | Stable | Locks and restrictions for models, permissions, hooks, MCP, auth, plugins. |
| macOS/Linux sandbox | Beta | Seatbelt/Bubblewrap; missing required runner fails closed. |
| Windows sandbox | Experimental | Builder/tests exist; host validation remains opt-in. |
| Instructions/memory | Stable | Compatible CLAUDE.md/rules discovery. |
| Agents, skills, styles, keybindings | Beta | User/project/plugin discovery and precedence. |
| Hooks | Beta | Seven implemented events; partial relative to the TypeScript CLI. |
| Plugins | Experimental | Installed index and contributed commands/agents/skills/hooks/styles/MCP/tools; no marketplace UI. |

## MCP and persistence

| Surface | Status | Notes |
| --- | --- | --- |
| MCP stdio/Streamable HTTP | Stable | Tools, resources, prompts, JSON/SSE HTTP responses. |
| MCP WebSocket | Beta | JSON-RPC over ws/wss. |
| MCP OAuth | Beta | Imported tokens, refresh, device/browser PKCE, dynamic registration. |
| MCP trust and permissions | Stable | Independent gates; neither bypasses the other. |
| MCP hot reload | Beta | Add/remove/restart from discovered config changes. |
| JSONL transcripts/session operations | Stable | Compatible layout, ordered flush, resume/fork/rename. |
| Context estimation/compaction | Beta | Manual and automatic compaction with boundary events. |
| Transcript rewind | Beta | Conversation checkpoint only; no file restoration. |

`orbcode advanced` is the runtime authority for advanced slices. It currently
reports background sessions active and remote-control bridge, voice, and
computer use deferred.
