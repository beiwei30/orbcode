use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

/// Intern a string into a process-`'static` slot, reusing the leaked allocation
/// for identical content.
///
/// `SlashCommandSpec` stores `&'static str`, and `register_dynamic_slash_commands`
/// re-runs on every MCP/plugin/skill hot-reload. Leaking a fresh copy of each
/// (usually unchanged) name/description on every reload leaked unboundedly;
/// interning bounds the leak to the set of *distinct* strings ever registered.
fn intern_str(value: String) -> &'static str {
    static INTERNED: LazyLock<Mutex<HashSet<&'static str>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));
    let mut set = INTERNED.lock().expect("string interner poisoned");
    if let Some(existing) = set.get(value.as_str()) {
        return existing;
    }
    let leaked: &'static str = Box::leak(value.into_boxed_str());
    set.insert(leaked);
    leaked
}

/// Intern a slice of already-interned strings, reusing the leaked slice for an
/// identical alias set (so the small pointer array isn't re-leaked per reload).
fn intern_alias_slice(aliases: Vec<&'static str>) -> &'static [&'static str] {
    if aliases.is_empty() {
        return &[];
    }
    static INTERNED: LazyLock<Mutex<HashMap<Vec<&'static str>, &'static [&'static str]>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let mut map = INTERNED.lock().expect("alias interner poisoned");
    if let Some(existing) = map.get(&aliases) {
        return existing;
    }
    let leaked: &'static [&'static str] = Box::leak(aliases.clone().into_boxed_slice());
    map.insert(aliases, leaked);
    leaked
}

use super::{
    AsyncLocalSlashCommand, BuiltinPromptSlashCommand, LocalOutputSlashCommand,
    SlashCommandExecution, SlashCommandExpansionId, SlashCommandFeedback, SlashCommandSource,
    SlashCommandSpec, TuiLocalSlashCommand, WorkflowSlashCommandId,
};

pub(crate) const BUILTIN_SLASH_COMMANDS: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        name: "agents",
        aliases: &[],
        description: "List available agent definitions",
        argument_hint: None,
        source: SlashCommandSource::Tooling,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Agents),
        feedback: SlashCommandFeedback::DIRECT_DEFERRED,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "add-dir",
        aliases: &["add-directory"],
        description: "Add a directory to the allowed list for file access",
        argument_hint: Some("[path]"),
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::AddDir),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "branch",
        aliases: &[],
        description: "Create and switch to a new git branch",
        argument_hint: Some("[name]"),
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Branch),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "clear",
        aliases: &["new", "reset"],
        description: "Clear conversation history and start fresh",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Clear),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "compact",
        aliases: &[],
        description: "Compact conversation history into a provider summary",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Compact),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "copy",
        aliases: &[],
        description: "Copy the latest assistant response to the clipboard",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Copy),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "context",
        aliases: &["ctx"],
        description: "Show current context window usage",
        argument_hint: Some("[--full]"),
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Context),
        feedback: SlashCommandFeedback::DIRECT_DEFERRED,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "config",
        aliases: &[],
        description: "Open terminal settings controls",
        argument_hint: Some("[model|theme|effort|editor-mode]"),
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Config),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "instructions",
        aliases: &[],
        description: "Show current system instructions and context",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Instructions),
        feedback: SlashCommandFeedback::DIRECT_DEFERRED,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "doctor",
        aliases: &[],
        description: "Run environment, auth, sandbox, and toolchain checks",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Doctor),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "diff",
        aliases: &[],
        description: "Show the current git workspace diff",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Diff),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "effort",
        aliases: &[],
        description: "Set effort level for model usage",
        argument_hint: Some("[low|medium|high|max|auto]"),
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Effort),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "exit",
        aliases: &["quit"],
        description: "Exit the REPL",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::Exit,
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "files",
        aliases: &[],
        description: "List recently referenced files and working directories",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Files),
        feedback: SlashCommandFeedback::DIRECT_DEFERRED,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "fork",
        aliases: &[],
        description: "Fork this session into a new conversation",
        argument_hint: Some("[title]"),
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Fork),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "goal",
        aliases: &[],
        description: "Show or manage this session's persistent goal",
        argument_hint: Some("[create|edit|pause|resume|clear|budget]"),
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Goal),
        feedback: SlashCommandFeedback::DIRECT_DEFERRED,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "help",
        aliases: &["?"],
        description: "Show help and available commands",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Help),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "jobs",
        aliases: &["background"],
        description: "Show background jobs overlay",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Help),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "hooks",
        aliases: &[],
        description: "List configured hooks and their trust status",
        argument_hint: None,
        source: SlashCommandSource::Tooling,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Hooks),
        feedback: SlashCommandFeedback::DIRECT_DEFERRED,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "init",
        aliases: &[],
        description: "Initialize a new CLAUDE.md file with codebase documentation",
        argument_hint: None,
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::BuiltinPrompt(BuiltinPromptSlashCommand::Init),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "keybindings",
        aliases: &[],
        description: "Open the keybindings configuration file in your editor",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Keybindings),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "trace",
        aliases: &["last-request", "llm-request"],
        description: "Show the latest LLM/tool/hook debug trace",
        argument_hint: None,
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::LocalOutput(LocalOutputSlashCommand::LastRequest),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "login",
        aliases: &[],
        description: "Show or configure provider auth metadata",
        argument_hint: Some("[provider --env-var VAR]"),
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Login),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "logout",
        aliases: &[],
        description: "Remove persisted provider auth metadata",
        argument_hint: Some("[provider]"),
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Logout),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "mcp",
        aliases: &[],
        description: "Manage MCP servers: list/status, add/remove, trust, inspect tools",
        argument_hint: Some(
            "capabilities|servers|list|status|resources|tools|add|remove|trust|distrust|untrust|read|call",
        ),
        source: SlashCommandSource::Tooling,
        execution: SlashCommandExecution::LocalOutput(LocalOutputSlashCommand::McpInspection),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "memory",
        aliases: &[],
        description: "Show Claude memory files for this workspace",
        argument_hint: Some("[auto on|off]"),
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Memory),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "model",
        aliases: &[],
        description: "Select the model for future requests",
        argument_hint: Some("[model]"),
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Model),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "output-style",
        aliases: &[],
        description: "Switch the active output style for assistant responses",
        argument_hint: Some("[style]"),
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::OutputStyle),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "permissions",
        aliases: &[],
        description: "Choose the session permission preset",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Permissions),
        feedback: SlashCommandFeedback::SUMMARY_DIRECT_DEFERRED,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "plan",
        aliases: &[],
        description: "Enable plan mode or show the current workspace plan",
        argument_hint: Some("[open|<description>]"),
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Plan),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "release-notes",
        aliases: &[],
        description: "View cached release notes",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::ReleaseNotes),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "rename",
        aliases: &[],
        description: "Rename the current session title",
        argument_hint: Some("<title>"),
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Rename),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "review",
        aliases: &[],
        description: "Review code changes using the code-review skill",
        argument_hint: Some("[--comment]"),
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::BuiltinPrompt(BuiltinPromptSlashCommand::Review),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "resume",
        aliases: &["session"],
        description: "Resume a previous conversation",
        argument_hint: Some("[session-id]"),
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Resume),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "rewind",
        aliases: &["checkpoint"],
        description: "Restore the conversation to a previous point",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Rewind),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "sandbox",
        aliases: &["sandbox-toggle"],
        description: "Configure sandbox command exclusions",
        argument_hint: Some("[exclude \"command pattern\"]"),
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Sandbox),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "sessions",
        aliases: &[],
        description: "Browse persisted project sessions",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Sessions),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "skills",
        aliases: &[],
        description: "List available skills and their sources",
        argument_hint: None,
        source: SlashCommandSource::Tooling,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Skills),
        feedback: SlashCommandFeedback::DIRECT_DEFERRED,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "status",
        aliases: &[],
        description: "Show session, model, provider, sandbox, and tool status",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Status),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "stats",
        aliases: &[],
        description: "Show project activity statistics",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Stats),
        feedback: SlashCommandFeedback::SUMMARY_DIRECT_DEFERRED,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "tool",
        aliases: &[],
        description: "Invoke a registered tool directly",
        argument_hint: Some("<name> [input]"),
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::Provider,
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "tools",
        aliases: &[],
        description: "List registered tools visible to the provider",
        argument_hint: None,
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::LocalOutput(LocalOutputSlashCommand::Tools),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "usage",
        aliases: &[],
        description: "Show token usage for this session",
        argument_hint: None,
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Usage),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "cost",
        aliases: &[],
        description: "Show cumulative cost for this session",
        argument_hint: None,
        source: SlashCommandSource::Provider,
        execution: SlashCommandExecution::AsyncLocal(AsyncLocalSlashCommand::Cost),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "theme",
        aliases: &[],
        description: "Change the theme",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Theme),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
    SlashCommandSpec {
        name: "vim",
        aliases: &[],
        description: "Toggle vim prompt editing mode",
        argument_hint: None,
        source: SlashCommandSource::Local,
        execution: SlashCommandExecution::TuiLocal(TuiLocalSlashCommand::Vim),
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: false,
        source_label: None,
    },
];

static RECENCY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn recency_store() -> &'static RwLock<HashMap<String, u64>> {
    static STORE: OnceLock<RwLock<HashMap<String, u64>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(HashMap::new()))
}

pub(crate) fn record_slash_command_use(name: &str) {
    let tick = RECENCY_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    recency_store()
        .write()
        .expect("recency store poisoned")
        .insert(name.to_string(), tick);
}

pub(in crate::slash_commands) fn slash_command_recency(name: &str) -> Option<u64> {
    recency_store()
        .read()
        .expect("recency store poisoned")
        .get(name)
        .copied()
}

#[cfg(test)]
pub(crate) fn clear_recency() {
    recency_store()
        .write()
        .expect("recency store poisoned")
        .clear();
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct McpPromptRef {
    pub(crate) server_id: &'static str,
    pub(crate) prompt_name: &'static str,
}

pub(in crate::slash_commands) struct SlashCommandRegistry {
    pub(in crate::slash_commands) commands: Vec<SlashCommandSpec>,
    pub(in crate::slash_commands) expansion_bodies: Vec<String>,
    pub(in crate::slash_commands) mcp_prompt_refs: Vec<McpPromptRef>,
    pub(in crate::slash_commands) workflow_names: Vec<String>,
}

impl SlashCommandRegistry {
    fn with_builtins() -> Self {
        Self {
            commands: BUILTIN_SLASH_COMMANDS.to_vec(),
            expansion_bodies: Vec::new(),
            mcp_prompt_refs: Vec::new(),
            workflow_names: Vec::new(),
        }
    }
}

pub(in crate::slash_commands) fn registry() -> &'static RwLock<SlashCommandRegistry> {
    static REGISTRY: OnceLock<RwLock<SlashCommandRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(SlashCommandRegistry::with_builtins()))
}

/// Snapshot of the registered slash commands. The vector is cloned (the
/// specs are `Copy`, so cloning is cheap), giving callers a stable view that
/// will not change while they iterate.
pub(crate) fn slash_commands() -> Vec<SlashCommandSpec> {
    registry()
        .read()
        .expect("slash registry poisoned")
        .commands
        .clone()
}

/// Replace the currently registered dynamic commands. Built-in commands are
/// always retained as the prefix of the registry. Subsequent calls overwrite
/// any previously registered dynamic commands so refresh remains idempotent.
pub(crate) fn register_dynamic_slash_commands(
    dynamic: Vec<crate::dynamic_slash_commands::DynamicSlashCommandSpec>,
) {
    let mut guard = registry().write().expect("slash registry poisoned");
    guard.commands.truncate(BUILTIN_SLASH_COMMANDS.len());
    guard.expansion_bodies.clear();
    guard.mcp_prompt_refs.clear();
    guard.workflow_names.clear();
    let registry = &mut *guard;
    for spec in dynamic {
        let (registered, body) = build_dynamic_entry(
            spec,
            registry.expansion_bodies.len(),
            &mut registry.mcp_prompt_refs,
            &mut registry.workflow_names,
        );
        registry.expansion_bodies.push(body);
        registry.commands.push(registered);
    }
}

fn build_dynamic_entry(
    spec: crate::dynamic_slash_commands::DynamicSlashCommandSpec,
    expansion_index: usize,
    mcp_prompt_refs: &mut Vec<McpPromptRef>,
    workflow_names: &mut Vec<String>,
) -> (SlashCommandSpec, String) {
    use super::{ExtensionSource, McpPromptExpansionId};
    use crate::dynamic_slash_commands::DynamicSlashCommandSource;

    let name: &'static str = intern_str(spec.name);
    let description: &'static str = intern_str(spec.description);
    let argument_hint: Option<&'static str> = spec.argument_hint.map(intern_str);
    let aliases: &'static [&'static str] =
        intern_alias_slice(spec.aliases.into_iter().map(intern_str).collect());
    let source = SlashCommandSource::Extension(match &spec.source {
        DynamicSlashCommandSource::User => ExtensionSource::User,
        DynamicSlashCommandSource::Project => ExtensionSource::Project,
        DynamicSlashCommandSource::Plugin { .. } => ExtensionSource::Plugin,
        DynamicSlashCommandSource::Skill => ExtensionSource::Skill,
        DynamicSlashCommandSource::McpPrompt { .. } => ExtensionSource::McpPrompt,
        DynamicSlashCommandSource::Workflow { .. } => ExtensionSource::Workflow,
    });
    let source_label: Option<&'static str> = match &spec.source {
        DynamicSlashCommandSource::User => Some("user"),
        DynamicSlashCommandSource::Project => Some("project"),
        DynamicSlashCommandSource::Plugin { plugin_name, .. } => {
            Some(intern_str(plugin_name.clone()))
        }
        DynamicSlashCommandSource::Skill => Some("skill"),
        DynamicSlashCommandSource::McpPrompt { server_id } => Some(intern_str(server_id.clone())),
        DynamicSlashCommandSource::Workflow { source } => Some(match source {
            orbcode_app_server_client::WorkflowSource::Project => "project",
            orbcode_app_server_client::WorkflowSource::User => "user",
        }),
    };
    let execution = if let Some(ref mcp_info) = spec.mcp_prompt {
        let id = mcp_prompt_refs.len();
        mcp_prompt_refs.push(McpPromptRef {
            server_id: intern_str(mcp_info.server_id.clone()),
            prompt_name: intern_str(mcp_info.prompt_name.clone()),
        });
        SlashCommandExecution::McpPromptExpansion(McpPromptExpansionId(id))
    } else {
        if let Some(workflow_name) = spec.workflow_name {
            let id = workflow_names.len();
            workflow_names.push(workflow_name);
            SlashCommandExecution::Workflow(WorkflowSlashCommandId(id))
        } else {
            SlashCommandExecution::PromptExpansion(SlashCommandExpansionId(expansion_index))
        }
    };
    let registered = SlashCommandSpec {
        name,
        aliases,
        description,
        argument_hint,
        source,
        execution,
        feedback: SlashCommandFeedback::DEFAULT,
        hidden: spec.hidden,
        source_label,
    };
    (registered, spec.prompt_body)
}

pub(crate) fn slash_command_expansion_body(id: SlashCommandExpansionId) -> Option<String> {
    let guard = registry().read().expect("slash registry poisoned");
    guard.expansion_bodies.get(id.0).cloned()
}

pub(crate) fn mcp_prompt_ref(id: super::McpPromptExpansionId) -> Option<McpPromptRef> {
    let guard = registry().read().expect("slash registry poisoned");
    guard.mcp_prompt_refs.get(id.0).copied()
}

pub(crate) fn workflow_slash_command_name(id: super::WorkflowSlashCommandId) -> Option<String> {
    let guard = registry().read().expect("slash registry poisoned");
    guard.workflow_names.get(id.0).cloned()
}

#[cfg(test)]
mod interner_tests {
    use super::{intern_alias_slice, intern_str};

    #[test]
    fn intern_str_reuses_allocation_for_identical_content() {
        let a = intern_str("hot-reload-command".to_string());
        let b = intern_str("hot-reload-command".to_string());
        // Same content → same leaked allocation (no per-reload leak).
        assert!(
            std::ptr::eq(a, b),
            "identical strings must share one allocation"
        );
        let c = intern_str("different-command".to_string());
        assert!(!std::ptr::eq(a, c));
    }

    #[test]
    fn intern_alias_slice_reuses_allocation_for_identical_set() {
        let one = intern_str("alias-one".to_string());
        let two = intern_str("alias-two".to_string());
        let a = intern_alias_slice(vec![one, two]);
        let b = intern_alias_slice(vec![one, two]);
        assert!(
            std::ptr::eq(a.as_ptr(), b.as_ptr()),
            "identical alias sets must share one leaked slice"
        );
        assert!(intern_alias_slice(vec![]).is_empty());
    }
}
