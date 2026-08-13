mod registry;

#[cfg(test)]
pub(crate) use registry::clear_recency;
use registry::slash_command_recency;
#[cfg(test)]
use registry::{BUILTIN_SLASH_COMMANDS, registry};
pub(crate) use registry::{
    mcp_prompt_ref, record_slash_command_use, register_dynamic_slash_commands,
    slash_command_expansion_body, slash_commands, workflow_slash_command_name,
};

use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, Debug)]
pub(crate) struct SlashCommandSpec {
    pub(crate) name: &'static str,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) description: &'static str,
    pub(crate) argument_hint: Option<&'static str>,
    pub(crate) source: SlashCommandSource,
    pub(crate) execution: SlashCommandExecution,
    pub(crate) feedback: SlashCommandFeedback,
    pub(crate) hidden: bool,
    pub(crate) source_label: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlashCommandSource {
    Local,
    Provider,
    Tooling,
    Extension(ExtensionSource),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExtensionSource {
    User,
    Project,
    Plugin,
    Skill,
    McpPrompt,
    Workflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlashCommandExecution {
    TuiLocal(TuiLocalSlashCommand),
    AsyncLocal(AsyncLocalSlashCommand),
    LocalOutput(LocalOutputSlashCommand),
    PromptExpansion(SlashCommandExpansionId),
    McpPromptExpansion(McpPromptExpansionId),
    Workflow(WorkflowSlashCommandId),
    /// Submit a fixed built-in prompt to the provider, just like a dynamic
    /// prompt-expansion command but with a statically known body (`/init`).
    BuiltinPrompt(BuiltinPromptSlashCommand),
    /// Exit the TUI cleanly (`/exit`).
    Exit,
    Provider,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BuiltinPromptSlashCommand {
    Init,
    Review,
}

/// Index into the global prompt-expansion body table. Lookup is constant
/// time so `SlashCommandSpec` can remain `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandExpansionId(pub(crate) usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct McpPromptExpansionId(pub(crate) usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowSlashCommandId(pub(crate) usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandFeedback {
    pub(crate) deferred: SlashCommandDeferredFeedback,
    pub(crate) show_summary: bool,
}

impl SlashCommandFeedback {
    pub(crate) const DEFAULT: Self = Self {
        deferred: SlashCommandDeferredFeedback::Direct,
        show_summary: false,
    };
    pub(crate) const DIRECT_DEFERRED: Self = Self {
        deferred: SlashCommandDeferredFeedback::Direct,
        show_summary: false,
    };
    pub(crate) const SUMMARY_DIRECT_DEFERRED: Self = Self {
        deferred: SlashCommandDeferredFeedback::Direct,
        show_summary: true,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlashCommandDeferredFeedback {
    Hidden,
    Quoted,
    Direct,
}

impl SlashCommandDeferredFeedback {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Quoted => "quoted",
            Self::Direct => "direct",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "hidden" => Some(Self::Hidden),
            "quoted" => Some(Self::Quoted),
            "direct" => Some(Self::Direct),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TuiLocalSlashCommand {
    AddDir,
    Branch,
    Clear,
    Compact,
    Config,
    Copy,
    Effort,
    Files,
    Fork,
    Goal,
    Help,
    Keybindings,
    Login,
    Logout,
    Model,
    OutputStyle,
    Permissions,
    Plan,
    ReleaseNotes,
    Rename,
    Resume,
    Rewind,
    Sandbox,
    Sessions,
    Theme,
    Vim,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AsyncLocalSlashCommand {
    Agents,
    Context,
    Cost,
    Diff,
    Doctor,
    Hooks,
    Instructions,
    Memory,
    Permissions,
    Skills,
    Stats,
    Status,
    Usage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalOutputSlashCommand {
    LastRequest,
    McpInspection,
    Tools,
}

impl LocalOutputSlashCommand {
    pub(crate) fn handles_args(self, args: &str) -> bool {
        match self {
            Self::LastRequest => args.is_empty(),
            Self::Tools => args.is_empty(),
            // `read`/`call` are intentionally excluded: they fall through to the
            // persisted-system handler so their output is recorded in the transcript.
            Self::McpInspection => matches!(
                args.split_whitespace().next(),
                Some(
                    "capabilities"
                        | "servers"
                        | "list"
                        | "status"
                        | "resources"
                        | "tools"
                        | "add"
                        | "remove"
                        | "trust"
                        | "distrust"
                        | "untrust"
                        | "auth"
                )
            ),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SlashCommandInvocation<'a> {
    pub(crate) spec: SlashCommandSpec,
    pub(crate) args: &'a str,
}

#[derive(Clone, Debug)]
pub(crate) enum SuggestionEntry {
    Command(SlashCommandSpec),
    GroupHeader(String),
}

#[derive(Clone, Debug)]
pub(crate) struct SlashCommandSuggestionView {
    pub(crate) entries: Vec<SuggestionEntry>,
    pub(crate) selected: usize,
    pub(crate) start: usize,
    pub(crate) visible_count: usize,
}

impl SlashCommandSuggestionView {
    pub(crate) fn selected_command(&self) -> Option<SlashCommandSpec> {
        match self.entries.get(self.selected) {
            Some(SuggestionEntry::Command(spec)) => Some(*spec),
            _ => None,
        }
    }

    pub(crate) fn command_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| matches!(entry, SuggestionEntry::Command(_)))
            .count()
    }
}

pub(crate) const SLASH_COMMAND_VISIBLE_ROWS: usize = 6;

pub(crate) fn slash_command_suggestions(input: &str) -> Vec<SlashCommandSpec> {
    if !input.starts_with('/') {
        return Vec::new();
    }
    let body = input[1..].trim_start();
    if body.contains(char::is_whitespace) {
        return Vec::new();
    }
    let query = body.to_ascii_lowercase();
    let all = slash_commands();
    let exact_match_present = all
        .iter()
        .any(|command| slash_command_name_matches(*command, &query));
    let mut suggestions: Vec<SlashCommandSpec> = all
        .iter()
        .copied()
        .filter(|command| {
            if command.hidden {
                // Hidden commands only surface when typed exactly.
                return exact_match_present && slash_command_name_matches(*command, &query);
            }
            slash_command_match_rank(*command, &query).is_some()
        })
        .collect();
    suggestions.sort_by(|left, right| {
        let left_rank = composite_rank(*left, &query);
        let right_rank = composite_rank(*right, &query);
        left_rank
            .cmp(&right_rank)
            .then_with(|| {
                let lr = slash_command_recency(left.name).unwrap_or(0);
                let rr = slash_command_recency(right.name).unwrap_or(0);
                rr.cmp(&lr)
            })
            .then_with(|| left.name.len().cmp(&right.name.len()))
            .then_with(|| left.name.cmp(right.name))
    });
    suggestions
}

fn suggestion_group_key(spec: &SlashCommandSpec) -> GroupKey {
    match spec.source {
        SlashCommandSource::Local | SlashCommandSource::Provider | SlashCommandSource::Tooling => {
            GroupKey::Builtin
        }
        SlashCommandSource::Extension(ExtensionSource::Project) => GroupKey::Project,
        SlashCommandSource::Extension(ExtensionSource::User) => GroupKey::User,
        SlashCommandSource::Extension(ExtensionSource::Plugin) => {
            GroupKey::Plugin(spec.source_label.unwrap_or("plugin").to_string())
        }
        SlashCommandSource::Extension(ExtensionSource::Skill) => GroupKey::Skill,
        SlashCommandSource::Extension(ExtensionSource::McpPrompt) => {
            GroupKey::McpPrompt(spec.source_label.unwrap_or("mcp").to_string())
        }
        SlashCommandSource::Extension(ExtensionSource::Workflow) => GroupKey::Workflow,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum GroupKey {
    Builtin,
    Project,
    User,
    Plugin(String),
    Skill,
    McpPrompt(String),
    Workflow,
}

impl GroupKey {
    fn header_label(&self) -> String {
        match self {
            Self::Builtin => "Built-in".to_string(),
            Self::Project => "Project".to_string(),
            Self::User => "User".to_string(),
            Self::Plugin(name) => format!("Plugin: {name}"),
            Self::Skill => "Skill".to_string(),
            Self::McpPrompt(name) => format!("MCP: {name}"),
            Self::Workflow => "Workflow".to_string(),
        }
    }
}

fn build_suggestion_entries(commands: &[SlashCommandSpec]) -> Vec<SuggestionEntry> {
    let mut entries = Vec::new();
    let has_multiple_groups = commands
        .windows(2)
        .any(|pair| suggestion_group_key(&pair[0]) != suggestion_group_key(&pair[1]));
    if !has_multiple_groups {
        return commands
            .iter()
            .map(|cmd| SuggestionEntry::Command(*cmd))
            .collect();
    }
    let mut current_group: Option<GroupKey> = None;
    for command in commands {
        let key = suggestion_group_key(command);
        if current_group.as_ref() != Some(&key) {
            entries.push(SuggestionEntry::GroupHeader(key.header_label()));
            current_group = Some(key);
        }
        entries.push(SuggestionEntry::Command(*command));
    }
    entries
}

pub(crate) fn slash_command_suggestion_view(
    input: &str,
    selected: usize,
) -> Option<SlashCommandSuggestionView> {
    let commands = slash_command_suggestions(input);
    if commands.is_empty() {
        return None;
    }
    let entries = build_suggestion_entries(&commands);
    let selected = clamp_to_command_entry(&entries, selected);
    let visible_count = entries.len().min(SLASH_COMMAND_VISIBLE_ROWS);
    let start = slash_command_view_start(selected, entries.len(), visible_count);
    Some(SlashCommandSuggestionView {
        entries,
        selected,
        start,
        visible_count,
    })
}

fn clamp_to_command_entry(entries: &[SuggestionEntry], target: usize) -> usize {
    let command_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, entry)| match entry {
            SuggestionEntry::Command(_) => Some(i),
            SuggestionEntry::GroupHeader(_) => None,
        })
        .collect();
    if command_indices.is_empty() {
        return 0;
    }
    let command_ordinal = target.min(command_indices.len().saturating_sub(1));
    command_indices[command_ordinal]
}

pub(crate) fn next_command_entry(entries: &[SuggestionEntry], current: usize) -> usize {
    entries
        .iter()
        .enumerate()
        .skip(current + 1)
        .find(|(_, entry)| matches!(entry, SuggestionEntry::Command(_)))
        .map_or(current, |(i, _)| i)
}

pub(crate) fn prev_command_entry(entries: &[SuggestionEntry], current: usize) -> usize {
    entries[..current]
        .iter()
        .enumerate()
        .rev()
        .find(|(_, entry)| matches!(entry, SuggestionEntry::Command(_)))
        .map_or(current, |(i, _)| i)
}

pub(crate) fn slash_command_view_start(
    selected: usize,
    total: usize,
    visible_count: usize,
) -> usize {
    if total <= visible_count {
        return 0;
    }
    selected
        .saturating_sub(visible_count / 2)
        .min(total.saturating_sub(visible_count))
}

pub(crate) fn suggestion_scrollbar_active(
    row: usize,
    total: usize,
    start: usize,
    visible_count: usize,
) -> bool {
    if total <= visible_count {
        return true;
    }
    let track = visible_count.max(1);
    let thumb_len = (track * visible_count).div_ceil(total).clamp(1, track);
    let max_thumb_start = track.saturating_sub(thumb_len);
    let max_view_start = total.saturating_sub(visible_count);
    let thumb_start = (start * max_thumb_start + max_view_start / 2)
        .checked_div(max_view_start)
        .unwrap_or(0);
    row >= thumb_start && row < thumb_start + thumb_len
}

pub(crate) fn slash_command_scrollbar_active(
    row: usize,
    view: &SlashCommandSuggestionView,
) -> bool {
    suggestion_scrollbar_active(row, view.entries.len(), view.start, view.visible_count)
}

pub(crate) fn slash_command_column_width(
    entries: &[SuggestionEntry],
    terminal_width: usize,
) -> usize {
    let min_command_width = entries
        .iter()
        .filter_map(|entry| match entry {
            SuggestionEntry::Command(spec) => Some(spec),
            SuggestionEntry::GroupHeader(_) => None,
        })
        .take(6)
        .map(|command| display_width_str(command.name).saturating_add(1))
        .max()
        .unwrap_or(12);
    let cap = terminal_width
        .saturating_mul(37)
        .saturating_div(100)
        .clamp(24, 56);
    cap.max(min_command_width)
}

pub(crate) fn exact_slash_command(input: &str) -> Option<SlashCommandSpec> {
    let invocation = slash_command_invocation(input)?;
    invocation.args.is_empty().then_some(invocation.spec)
}

#[cfg(test)]
pub(crate) fn async_local_slash_command(input: &str) -> Option<AsyncLocalSlashCommand> {
    match exact_slash_command(input)?.execution {
        SlashCommandExecution::AsyncLocal(command) => Some(command),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn local_output_slash_command(input: &str) -> Option<(LocalOutputSlashCommand, String)> {
    let invocation = slash_command_invocation(input)?;
    match invocation.spec.execution {
        SlashCommandExecution::LocalOutput(command) => Some((command, invocation.args.to_string())),
        _ => None,
    }
}

pub(crate) fn slash_command_invocation(input: &str) -> Option<SlashCommandInvocation<'_>> {
    let rest = input.strip_prefix('/')?;
    let trimmed = rest.trim_start();
    let command_len = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let (command_name, command_rest) = trimmed.split_at(command_len);
    if command_name.is_empty() {
        return None;
    }
    let spec = slash_commands()
        .into_iter()
        .find(|command| slash_command_name_matches(*command, command_name))?;
    Some(SlashCommandInvocation {
        spec,
        args: command_rest.trim(),
    })
}

pub(crate) fn canonicalize_slash_command_line(input: &str) -> String {
    let Some(rest) = input.strip_prefix('/') else {
        return input.to_string();
    };
    let trimmed = rest.trim_start();
    let command_len = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let (command_name, command_rest) = trimmed.split_at(command_len);
    if command_name.is_empty() {
        return input.to_string();
    }
    let Some(command) = slash_commands()
        .into_iter()
        .find(|command| slash_command_name_matches(*command, command_name))
    else {
        return input.to_string();
    };
    format!("/{}{}", command.name, command_rest)
}

#[cfg(test)]
pub(crate) fn render_slash_command_help() -> String {
    let mut lines = vec!["Slash commands:".to_string()];
    for command in slash_commands() {
        if command.hidden {
            continue;
        }
        let aliases = if command.aliases.is_empty() {
            String::new()
        } else {
            format!(" aliases: {}", command.aliases.join(", "))
        };
        lines.push(format!(
            "{:<40}{:<10}{}{}",
            command.usage(),
            command.source.label(),
            command.description,
            aliases
        ));
    }
    lines.push(String::new());
    lines.push("MCP subcommands:".to_string());
    lines.push("/mcp capabilities".to_string());
    lines.push("/mcp list (alias: servers)".to_string());
    lines.push("/mcp status".to_string());
    lines.push("/mcp add <id> <transport> <endpoint> [summary]".to_string());
    lines.push("/mcp remove <server>".to_string());
    lines.push("/mcp trust <server>".to_string());
    lines.push("/mcp distrust <server>".to_string());
    lines.push("/mcp untrust <server>".to_string());
    lines.push("/mcp resources <server>".to_string());
    lines.push("/mcp tools <server>".to_string());
    lines.push("/mcp read <server> <uri>".to_string());
    lines.push("/mcp call <server> <tool> [input]".to_string());
    lines.push(String::new());
    lines.push("Controls:".to_string());
    lines.push("PgUp/PgDn/Home/End browse transcript (End follows latest)".to_string());
    lines.push(
        "Drag inside the transcript to select text; top/bottom edge drag auto-scrolls, and release copies it"
            .to_string(),
    );
    lines.push("Ctrl+R open session picker".to_string());
    lines.push("Alt/Shift/Ctrl+Enter insert newline".to_string());
    lines.push("Ctrl+A / Ctrl+E jump to input start/end".to_string());
    lines.push("Ctrl+U clear the current prompt".to_string());
    lines.push(
        "Up/Down move through multiline input, and browse ~/.claude/history.jsonl when you're at the top."
            .to_string(),
    );
    lines.join("\n")
}

impl SlashCommandSpec {
    pub(crate) fn usage(self) -> String {
        match self.argument_hint {
            Some(hint) => format!("/{} {}", self.name, hint),
            None => format!("/{}", self.name),
        }
    }
}

#[cfg(test)]
impl SlashCommandSource {
    fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Provider => "provider",
            Self::Tooling => "tooling",
            Self::Extension(ExtensionSource::User) => "user",
            Self::Extension(ExtensionSource::Project) => "project",
            Self::Extension(ExtensionSource::Plugin) => "plugin",
            Self::Extension(ExtensionSource::Skill) => "skill",
            Self::Extension(ExtensionSource::McpPrompt) => "mcp",
            Self::Extension(ExtensionSource::Workflow) => "workflow",
        }
    }
}

fn slash_command_name_matches(command: SlashCommandSpec, name: &str) -> bool {
    command.name == name || command.aliases.contains(&name)
}

fn slash_command_match_rank(command: SlashCommandSpec, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    if command.name == query {
        return Some(0);
    }
    if command.aliases.contains(&query) {
        return Some(1);
    }
    if command.name.starts_with(query) {
        return Some(2);
    }
    if command.aliases.iter().any(|alias| alias.starts_with(query)) {
        return Some(3);
    }
    if let Some(score) = fuzzy_match_score(command.name, query) {
        return Some(4 + score);
    }
    command
        .aliases
        .iter()
        .filter_map(|alias| fuzzy_match_score(alias, query))
        .min()
        .map(|score| 10_000 + score)
}

/// Combine the textual match rank with a per-source bias. Built-in commands
/// stay ahead of dynamic extensions for the same textual match so heavy
/// plugin libraries can't push core commands off the visible window. Hidden
/// commands are not displayed but, when matched exactly, are still returned
/// from invocation lookups; they are bumped to the end of the suggestion
/// list as a safety net.
fn composite_rank(command: SlashCommandSpec, query: &str) -> usize {
    let textual = slash_command_match_rank(command, query).unwrap_or(usize::MAX / 2);
    let source_weight: usize = match command.source {
        SlashCommandSource::Local | SlashCommandSource::Provider | SlashCommandSource::Tooling => 0,
        SlashCommandSource::Extension(ExtensionSource::Project) => 1,
        SlashCommandSource::Extension(ExtensionSource::User) => 2,
        SlashCommandSource::Extension(ExtensionSource::Plugin) => 3,
        SlashCommandSource::Extension(ExtensionSource::Skill) => 4,
        SlashCommandSource::Extension(ExtensionSource::McpPrompt) => 5,
        SlashCommandSource::Extension(ExtensionSource::Workflow) => 6,
    };
    let hidden_weight: usize = if command.hidden { 1_000_000 } else { 0 };
    textual
        .saturating_add(source_weight)
        .saturating_add(hidden_weight)
}

pub(crate) fn fuzzy_match_score(candidate: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }

    let candidate = candidate.to_ascii_lowercase();
    let query = query.to_ascii_lowercase();
    let mut cursor = 0usize;
    let mut first_index = None;
    let mut last_index = 0usize;
    let mut gaps = 0usize;
    let mut consecutive = 0usize;

    for needle in query.chars() {
        let mut found = None;
        for (offset, character) in candidate[cursor..].char_indices() {
            if character == needle {
                found = Some(cursor + offset);
                break;
            }
        }
        let index = found?;
        if let Some(previous) = first_index.replace(first_index.unwrap_or(index)) {
            let gap = index.saturating_sub(last_index + 1);
            gaps += gap;
            if gap == 0 {
                consecutive += 1;
            }
            first_index = Some(previous);
        }
        last_index = index;
        cursor = index + needle.len_utf8();
    }

    let first = first_index.unwrap_or(0);
    let span = last_index.saturating_sub(first);
    Some(first * 20 + gaps * 4 + span.saturating_sub(consecutive))
}

fn display_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0).max(1)
}

fn display_width_str(text: &str) -> usize {
    text.chars().map(display_width).sum()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::OnceLock;

    use super::*;
    use crate::dynamic_slash_commands::{DynamicSlashCommandSource, DynamicSlashCommandSpec};

    /// Serialises tests that mutate the shared registry. The global registry
    /// is process-wide, so parallel tests would otherwise race on the data
    /// they each expect to find.
    fn test_guard() -> &'static Mutex<()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD.get_or_init(|| Mutex::new(()))
    }

    fn reset_registry() {
        let mut guard = registry().write().expect("slash registry poisoned");
        guard.commands.truncate(BUILTIN_SLASH_COMMANDS.len());
        guard.expansion_bodies.clear();
        guard.mcp_prompt_refs.clear();
        guard.workflow_names.clear();
    }

    fn reset_recency() {
        registry::clear_recency();
    }

    fn make_dynamic(name: &str, source: DynamicSlashCommandSource) -> DynamicSlashCommandSpec {
        DynamicSlashCommandSpec {
            name: name.to_string(),
            aliases: Vec::new(),
            description: format!("Run {name}"),
            argument_hint: None,
            source,
            hidden: false,
            prompt_body: format!("Body for {name}: $ARGUMENTS"),
            mcp_prompt: None,
            workflow_name: None,
        }
    }

    #[test]
    fn dynamic_registry_lifecycle() {
        // Single serial test covers register/resolve, suggestion ranking,
        // hidden visibility, and idempotent re-registration so the shared
        // process-wide registry stays well-defined throughout.
        let _lock = test_guard().lock().expect("test guard poisoned");
        reset_registry();

        register_dynamic_slash_commands(vec![
            make_dynamic("review-pr", DynamicSlashCommandSource::User),
            make_dynamic(
                "demo:greet",
                DynamicSlashCommandSource::Plugin {
                    plugin_id: "demo@market".into(),
                    plugin_name: "demo".into(),
                },
            ),
        ]);

        let invocation = slash_command_invocation("/review-pr 42").expect("invocation");
        assert_eq!(invocation.spec.name, "review-pr");
        assert_eq!(invocation.args, "42");
        match invocation.spec.execution {
            SlashCommandExecution::PromptExpansion(id) => assert_eq!(
                slash_command_expansion_body(id).as_deref(),
                Some("Body for review-pr: $ARGUMENTS")
            ),
            _ => panic!("expected prompt expansion"),
        }

        let dem_suggestions = slash_command_suggestions("/dem");
        let dem_names: Vec<_> = dem_suggestions.iter().map(|spec| spec.name).collect();
        assert!(dem_names.contains(&"demo:greet"));

        // Built-in `config` must outrank an extension that fuzzy-matches the
        // same query, even though the extension is also a perfect prefix.
        reset_registry();
        register_dynamic_slash_commands(vec![make_dynamic(
            "config-export",
            DynamicSlashCommandSource::Project,
        )]);
        let config_suggestions = slash_command_suggestions("/config");
        let config_names: Vec<_> = config_suggestions.iter().map(|spec| spec.name).collect();
        assert_eq!(config_names.first().copied(), Some("config"));

        // Hidden commands stay out of suggestion lists but remain
        // directly invocable.
        reset_registry();
        register_dynamic_slash_commands(vec![DynamicSlashCommandSpec {
            name: "secret-tool".into(),
            aliases: Vec::new(),
            description: "Restricted helper".into(),
            argument_hint: None,
            source: DynamicSlashCommandSource::User,
            hidden: true,
            prompt_body: "do the thing".into(),
            mcp_prompt: None,
            workflow_name: None,
        }]);
        let prefix_matches = slash_command_suggestions("/sec");
        assert!(
            prefix_matches.iter().all(|spec| spec.name != "secret-tool"),
            "hidden commands should not surface from fuzzy/prefix queries"
        );
        let exact = slash_command_invocation("/secret-tool").expect("exact invocation");
        assert_eq!(exact.spec.name, "secret-tool");

        // Re-registering replaces the previous dynamic list.
        reset_registry();
        register_dynamic_slash_commands(vec![make_dynamic(
            "first",
            DynamicSlashCommandSource::User,
        )]);
        register_dynamic_slash_commands(vec![make_dynamic(
            "second",
            DynamicSlashCommandSource::User,
        )]);
        let all = slash_commands();
        assert!(all.iter().any(|spec| spec.name == "second"));
        assert!(all.iter().all(|spec| spec.name != "first"));

        reset_registry();
    }

    #[test]
    fn group_headers_appear_between_builtin_and_extension_commands() {
        let _lock = test_guard().lock().expect("test guard poisoned");
        reset_registry();
        reset_recency();

        register_dynamic_slash_commands(vec![make_dynamic(
            "deploy",
            DynamicSlashCommandSource::User,
        )]);

        let view = slash_command_suggestion_view("/", 0).expect("view");
        let headers: Vec<&str> = view
            .entries
            .iter()
            .filter_map(|entry| match entry {
                SuggestionEntry::GroupHeader(label) => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            headers.contains(&"Built-in"),
            "expected Built-in header, got: {headers:?}"
        );
        assert!(
            headers.contains(&"User"),
            "expected User header, got: {headers:?}"
        );

        reset_registry();
        reset_recency();
    }

    #[test]
    fn plugin_group_header_shows_plugin_name() {
        let _lock = test_guard().lock().expect("test guard poisoned");
        reset_registry();
        reset_recency();

        register_dynamic_slash_commands(vec![make_dynamic(
            "my-plugin:lint",
            DynamicSlashCommandSource::Plugin {
                plugin_id: "my-plugin@market".into(),
                plugin_name: "my-plugin".into(),
            },
        )]);

        let view = slash_command_suggestion_view("/", 0).expect("view");
        let headers: Vec<&str> = view
            .entries
            .iter()
            .filter_map(|entry| match entry {
                SuggestionEntry::GroupHeader(label) => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            headers.contains(&"Plugin: my-plugin"),
            "expected 'Plugin: my-plugin' header, got: {headers:?}"
        );

        reset_registry();
        reset_recency();
    }

    #[test]
    fn recency_boost_reorders_within_source_group() {
        let _lock = test_guard().lock().expect("test guard poisoned");
        reset_registry();
        reset_recency();

        // stats (len 5) sorts before status (len 6) by name-length tiebreaker
        let baseline = slash_command_suggestions("/sta");
        let stats_pos = baseline
            .iter()
            .position(|spec| spec.name == "stats")
            .expect("stats in suggestions");
        let status_pos = baseline
            .iter()
            .position(|spec| spec.name == "status")
            .expect("status in suggestions");
        assert!(
            stats_pos < status_pos,
            "before recency: stats ({stats_pos}) should come before status ({status_pos})"
        );

        record_slash_command_use("status");

        let boosted = slash_command_suggestions("/sta");
        let stats_pos2 = boosted
            .iter()
            .position(|spec| spec.name == "stats")
            .expect("stats in boosted suggestions");
        let status_pos2 = boosted
            .iter()
            .position(|spec| spec.name == "status")
            .expect("status in boosted suggestions");
        assert!(
            status_pos2 < stats_pos2,
            "after recency: status ({status_pos2}) should come before stats ({stats_pos2})"
        );

        reset_registry();
        reset_recency();
    }

    #[test]
    fn group_headers_are_not_selectable() {
        let _lock = test_guard().lock().expect("test guard poisoned");
        reset_registry();
        reset_recency();

        register_dynamic_slash_commands(vec![make_dynamic(
            "deploy",
            DynamicSlashCommandSource::User,
        )]);

        let view = slash_command_suggestion_view("/", 0).expect("view");
        assert!(
            matches!(view.entries[view.selected], SuggestionEntry::Command(_)),
            "selected entry should always be a Command"
        );

        for target in 0..view.command_count() {
            let view = slash_command_suggestion_view("/", target).expect("view");
            assert!(
                matches!(view.entries[view.selected], SuggestionEntry::Command(_)),
                "selected at ordinal {target} should be a Command, not a header"
            );
        }

        let first_cmd = view.selected;
        let next = next_command_entry(&view.entries, first_cmd);
        assert!(
            matches!(view.entries[next], SuggestionEntry::Command(_)),
            "next_command_entry should skip headers"
        );

        let prev = prev_command_entry(&view.entries, next);
        assert!(
            matches!(view.entries[prev], SuggestionEntry::Command(_)),
            "prev_command_entry should skip headers"
        );
        assert_eq!(prev, first_cmd, "prev should return to first command");

        reset_registry();
        reset_recency();
    }

    #[test]
    fn no_group_headers_when_only_builtins() {
        let _lock = test_guard().lock().expect("test guard poisoned");
        reset_registry();
        reset_recency();

        let view = slash_command_suggestion_view("/", 0).expect("view");
        let header_count = view
            .entries
            .iter()
            .filter(|entry| matches!(entry, SuggestionEntry::GroupHeader(_)))
            .count();
        assert_eq!(
            header_count, 0,
            "no group headers when all commands are built-in"
        );

        reset_registry();
        reset_recency();
    }

    #[test]
    fn mcp_prompt_group_header_shows_server_name() {
        let _lock = test_guard().lock().expect("test guard poisoned");
        reset_registry();
        reset_recency();

        register_dynamic_slash_commands(vec![DynamicSlashCommandSpec {
            name: "my-server:summarize".into(),
            aliases: Vec::new(),
            description: "Summarize content".into(),
            argument_hint: Some("<text>".into()),
            source: DynamicSlashCommandSource::McpPrompt {
                server_id: "my-server".into(),
            },
            hidden: false,
            prompt_body: String::new(),
            mcp_prompt: Some(crate::dynamic_slash_commands::McpPromptInfo {
                server_id: "my-server".into(),
                prompt_name: "summarize".into(),
            }),
            workflow_name: None,
        }]);

        let view = slash_command_suggestion_view("/", 0).expect("view");
        let headers: Vec<&str> = view
            .entries
            .iter()
            .filter_map(|entry| match entry {
                SuggestionEntry::GroupHeader(label) => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            headers.contains(&"MCP: my-server"),
            "expected 'MCP: my-server' header, got: {headers:?}"
        );

        let invocation =
            slash_command_invocation("/my-server:summarize hello").expect("invocation");
        assert_eq!(invocation.spec.name, "my-server:summarize");
        assert_eq!(invocation.args, "hello");
        assert!(matches!(
            invocation.spec.execution,
            SlashCommandExecution::McpPromptExpansion(_)
        ));

        reset_registry();
        reset_recency();
    }

    #[test]
    fn mcp_prompt_ref_lookup() {
        let _lock = test_guard().lock().expect("test guard poisoned");
        reset_registry();

        register_dynamic_slash_commands(vec![DynamicSlashCommandSpec {
            name: "srv:do-thing".into(),
            aliases: Vec::new(),
            description: "Do a thing".into(),
            argument_hint: None,
            source: DynamicSlashCommandSource::McpPrompt {
                server_id: "srv".into(),
            },
            hidden: false,
            prompt_body: String::new(),
            mcp_prompt: Some(crate::dynamic_slash_commands::McpPromptInfo {
                server_id: "srv".into(),
                prompt_name: "do-thing".into(),
            }),
            workflow_name: None,
        }]);

        let invocation = slash_command_invocation("/srv:do-thing").expect("invocation");
        if let SlashCommandExecution::McpPromptExpansion(id) = invocation.spec.execution {
            let ref_entry = mcp_prompt_ref(id).expect("mcp_prompt_ref should return entry");
            assert_eq!(ref_entry.server_id, "srv");
            assert_eq!(ref_entry.prompt_name, "do-thing");
        } else {
            panic!("expected McpPromptExpansion");
        }

        reset_registry();
    }

    #[test]
    fn workflow_dynamic_command_registers_execution_variant() {
        let _lock = test_guard().lock().expect("test guard poisoned");
        reset_registry();

        register_dynamic_slash_commands(vec![DynamicSlashCommandSpec {
            name: "workflow:acp:check".into(),
            aliases: Vec::new(),
            description: "Run ACP check".into(),
            argument_hint: Some("<args>".into()),
            source: DynamicSlashCommandSource::Workflow {
                source: orbcode_app_server_client::WorkflowSource::Project,
            },
            hidden: false,
            prompt_body: String::new(),
            mcp_prompt: None,
            workflow_name: Some("acp:check".into()),
        }]);

        let invocation =
            slash_command_invocation("/workflow:acp:check fast").expect("workflow invocation");
        assert_eq!(invocation.args, "fast");
        if let SlashCommandExecution::Workflow(id) = invocation.spec.execution {
            assert_eq!(
                workflow_slash_command_name(id).as_deref(),
                Some("acp:check")
            );
        } else {
            panic!("expected workflow execution");
        }

        reset_registry();
    }
}
