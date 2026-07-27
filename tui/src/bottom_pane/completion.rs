use std::path::{Path, PathBuf};

use orbcode_app_server_client::McpSlashSuggestionCatalog;

use crate::slash_commands::{
    self, SLASH_COMMAND_VISIBLE_ROWS, SlashCommandSpec, SlashCommandSuggestionView,
    SuggestionEntry, fuzzy_match_score, next_command_entry, prev_command_entry,
    slash_command_invocation, slash_command_suggestion_view, suggestion_scrollbar_active,
};
use crate::state::TuiState;

pub(crate) const ADD_DIR_COMPLETION_VISIBLE_ROWS: usize = 10;

fn command_ordinal(entries: &[SuggestionEntry], entry_index: usize) -> usize {
    entries[..=entry_index]
        .iter()
        .filter(|entry| matches!(entry, SuggestionEntry::Command(_)))
        .count()
        .saturating_sub(1)
}

#[derive(Clone, Debug)]
pub(crate) struct AddDirCompletion {
    pub(crate) label: String,
    pub(crate) replacement: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AddDirCompletionView {
    pub(crate) suggestions: Vec<AddDirCompletion>,
    pub(crate) selected: usize,
    pub(crate) start: usize,
    pub(crate) visible_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct SlashArgumentCompletion {
    pub(crate) label: String,
    pub(crate) replacement: String,
    pub(crate) description: String,
    pub(crate) completable: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SlashArgumentCompletionView {
    pub(crate) suggestions: Vec<SlashArgumentCompletion>,
    pub(crate) selected: usize,
    pub(crate) start: usize,
    pub(crate) visible_count: usize,
}

pub(crate) fn add_dir_completion_view(
    input: &str,
    cursor: usize,
    cwd: &Path,
    selected: usize,
) -> Option<AddDirCompletionView> {
    add_dir_completion_slice_and_view(input, cursor, cwd, selected).map(|(_, view)| view)
}

impl TuiState {
    pub(crate) fn slash_command_view(&self) -> Option<SlashCommandSuggestionView> {
        slash_command_suggestion_view(&self.input, self.slash_command_selected)
    }

    pub(crate) fn add_dir_completion_view(&self) -> Option<AddDirCompletionView> {
        add_dir_completion_view(
            &self.input,
            self.input_cursor,
            &self.cwd,
            self.slash_command_selected,
        )
    }

    pub(crate) fn slash_argument_completion_view(&self) -> Option<SlashArgumentCompletionView> {
        slash_argument_completion_slice_and_view(
            &self.input,
            self.input_cursor,
            self.slash_command_selected,
            &self.mcp_slash_suggestions,
        )
        .map(|(_, view)| view)
    }

    /// True when any completion popup (slash command, slash argument, or
    /// add-directory) is currently offering suggestions for the input. Used to
    /// let Tab drive completion rather than being hijacked for other actions.
    pub(crate) fn has_active_completion_popup(&self) -> bool {
        self.add_dir_completion_view().is_some()
            || self.slash_argument_completion_view().is_some()
            || self.slash_command_view().is_some()
    }

    pub(crate) fn move_input_suggestion_selection(&mut self, direction: isize) -> bool {
        if self.add_dir_completion_view().is_some() {
            return self.move_add_dir_completion_selection(direction);
        }
        if self.slash_argument_completion_view().is_some() {
            return self.move_slash_argument_completion_selection(direction);
        }
        self.move_slash_command_selection(direction)
    }

    fn move_add_dir_completion_selection(&mut self, direction: isize) -> bool {
        let Some(view) = self.add_dir_completion_view() else {
            return false;
        };
        if view.suggestions.len() <= 1 {
            return false;
        }
        let max_index = view.suggestions.len().saturating_sub(1);
        self.slash_command_selected = if direction < 0 {
            view.selected.saturating_sub(1)
        } else {
            view.selected.saturating_add(1).min(max_index)
        };
        self.set_status_line(format!(
            "Directory {}/{}",
            self.slash_command_selected + 1,
            view.suggestions.len()
        ));
        true
    }

    fn move_slash_argument_completion_selection(&mut self, direction: isize) -> bool {
        let Some(view) = self.slash_argument_completion_view() else {
            return false;
        };
        if view.suggestions.len() <= 1 {
            return false;
        }
        let max_index = view.suggestions.len().saturating_sub(1);
        self.slash_command_selected = if direction < 0 {
            view.selected.saturating_sub(1)
        } else {
            view.selected.saturating_add(1).min(max_index)
        };
        self.set_status_line(format!(
            "Slash argument {}/{}",
            self.slash_command_selected + 1,
            view.suggestions.len()
        ));
        true
    }

    fn move_slash_command_selection(&mut self, direction: isize) -> bool {
        let Some(view) = self.slash_command_view() else {
            return false;
        };
        if view.command_count() <= 1 {
            return false;
        }
        let new_selected = if direction < 0 {
            prev_command_entry(&view.entries, view.selected)
        } else {
            next_command_entry(&view.entries, view.selected)
        };
        self.slash_command_selected = command_ordinal(&view.entries, new_selected);
        self.set_status_line(format!(
            "Slash command {}/{}",
            self.slash_command_selected + 1,
            view.command_count()
        ));
        true
    }

    pub(crate) fn complete_selected_slash_command(&mut self) -> bool {
        let Some(view) = self.slash_command_view() else {
            return false;
        };
        let Some(command) = view.selected_command() else {
            return false;
        };
        self.input = format!("/{} ", command.name);
        self.input_cursor = self.input.len();
        self.prompt_history_index = None;
        self.slash_command_selected = 0;
        self.set_status_line(format!("Completed /{}.", command.name));
        true
    }

    pub(crate) fn complete_selected_slash_argument_completion(&mut self) -> bool {
        let Some((argument_start, view)) = slash_argument_completion_slice_and_view(
            &self.input,
            self.input_cursor,
            self.slash_command_selected,
            &self.mcp_slash_suggestions,
        ) else {
            return false;
        };
        let Some(suggestion) = view.suggestions.get(view.selected) else {
            return false;
        };
        if !suggestion.completable {
            return false;
        }
        self.input
            .replace_range(argument_start..self.input_cursor, &suggestion.replacement);
        self.input_cursor = argument_start + suggestion.replacement.len();
        self.prompt_history_index = None;
        self.slash_command_selected = 0;
        self.set_status_line(format!("Completed {}.", suggestion.label));
        true
    }

    pub(crate) fn complete_selected_add_dir_completion(&mut self) -> bool {
        let Some((argument_start, view)) = add_dir_completion_slice_and_view(
            &self.input,
            self.input_cursor,
            &self.cwd,
            self.slash_command_selected,
        ) else {
            return false;
        };
        let Some(suggestion) = view.suggestions.get(view.selected) else {
            return false;
        };
        self.input
            .replace_range(argument_start..self.input_cursor, &suggestion.replacement);
        self.input_cursor = argument_start + suggestion.replacement.len();
        self.prompt_history_index = None;
        self.slash_command_selected = 0;
        self.set_status_line(format!("Completed {}.", suggestion.label));
        true
    }
}

pub(crate) fn slash_argument_completion_slice_and_view(
    input: &str,
    cursor: usize,
    selected: usize,
    mcp_suggestions: &McpSlashSuggestionCatalog,
) -> Option<(usize, SlashArgumentCompletionView)> {
    let (argument_start, argument, completions) =
        slash_argument_completion_argument(input, cursor, mcp_suggestions)?;
    let argument_lower = argument.to_ascii_lowercase();
    let mut suggestions = completions
        .into_iter()
        .filter(|completion| {
            // Case-insensitive prefix match so `/tool r` offers `Read` etc.
            !completion.completable
                || completion
                    .label
                    .to_ascii_lowercase()
                    .starts_with(&argument_lower)
        })
        .collect::<Vec<_>>();
    if suggestions.is_empty() {
        return None;
    }
    if suggestions.iter().all(|suggestion| !suggestion.completable) {
        suggestions.truncate(1);
    }
    let selected = selected.min(suggestions.len().saturating_sub(1));
    let visible_count = suggestions.len().min(SLASH_COMMAND_VISIBLE_ROWS);
    let start =
        slash_commands::slash_command_view_start(selected, suggestions.len(), visible_count);
    Some((
        argument_start,
        SlashArgumentCompletionView {
            suggestions,
            selected,
            start,
            visible_count,
        },
    ))
}

pub(crate) fn slash_argument_completion_argument<'a>(
    input: &'a str,
    cursor: usize,
    mcp_suggestions: &McpSlashSuggestionCatalog,
) -> Option<(usize, &'a str, Vec<SlashArgumentCompletion>)> {
    if cursor != input.len() {
        return None;
    }
    let invocation = slash_command_invocation(input)?;
    let hint = invocation.spec.argument_hint?;
    let rest = input.strip_prefix('/')?;
    let trimmed = rest.trim_start();
    let leading_ws = rest.len().saturating_sub(trimmed.len());
    let command_len = trimmed.find(char::is_whitespace)?;
    let after_command = 1 + leading_ws + command_len;
    let raw_args = input[after_command..].trim_start();
    let tokens = raw_args.split_whitespace().collect::<Vec<_>>();
    let trailing_space = input.ends_with(char::is_whitespace);
    let argument = if trailing_space {
        ""
    } else {
        tokens.last().copied().unwrap_or("")
    };
    let argument_start = if trailing_space {
        input.len()
    } else {
        input.len().saturating_sub(argument.len())
    };
    let argument_index = if trailing_space {
        tokens.len()
    } else {
        tokens.len().saturating_sub(1)
    };
    let completions = dynamic_slash_argument_completion_templates(
        invocation.spec,
        raw_args,
        argument_index,
        mcp_suggestions,
    )
    .unwrap_or_else(|| slash_argument_completion_templates(invocation.spec, hint, argument_index));
    if completions.is_empty() {
        return Some((
            argument_start,
            argument,
            vec![SlashArgumentCompletion {
                label: hint.to_string(),
                replacement: String::new(),
                description: format!("arguments for /{}", invocation.spec.name),
                completable: false,
            }],
        ));
    }
    Some((argument_start, argument, completions))
}

fn dynamic_slash_argument_completion_templates(
    command: SlashCommandSpec,
    raw_args: &str,
    argument_index: usize,
    mcp_suggestions: &McpSlashSuggestionCatalog,
) -> Option<Vec<SlashArgumentCompletion>> {
    match command.name {
        "tool" if argument_index == 0 => {
            let mut completions = provider_tool_argument_completions();
            completions.extend(
                mcp_suggestions
                    .tools
                    .iter()
                    .map(|tool| SlashArgumentCompletion {
                        label: tool.provider_name.clone(),
                        replacement: format!("{} ", tool.provider_name),
                        description: if tool.description.trim().is_empty() {
                            format!("MCP tool from `{}`", tool.server_id)
                        } else {
                            tool.description.clone()
                        },
                        completable: true,
                    }),
            );
            Some(completions)
        }
        "mcp" => mcp_argument_completions(raw_args, argument_index, mcp_suggestions),
        _ => None,
    }
}

fn mcp_argument_completions(
    raw_args: &str,
    argument_index: usize,
    mcp_suggestions: &McpSlashSuggestionCatalog,
) -> Option<Vec<SlashArgumentCompletion>> {
    let tokens = raw_args.split_whitespace().collect::<Vec<_>>();
    let subcommand = tokens.first().copied().unwrap_or("");
    match (argument_index, subcommand) {
        (1, "resources" | "tools" | "read" | "call") => Some(
            mcp_suggestions
                .servers
                .iter()
                .map(|server| SlashArgumentCompletion {
                    label: server.id.clone(),
                    replacement: format!("{} ", server.id),
                    description: if server.summary.trim().is_empty() {
                        "MCP server".to_string()
                    } else {
                        server.summary.clone()
                    },
                    completable: true,
                })
                .collect(),
        ),
        (2, "read") => {
            let server_id = tokens.get(1).copied().unwrap_or("");
            Some(
                mcp_suggestions
                    .resources
                    .iter()
                    .filter(|resource| resource.server_id == server_id)
                    .map(|resource| SlashArgumentCompletion {
                        label: resource.uri.clone(),
                        replacement: format!("{} ", resource.uri),
                        description: if resource.description.trim().is_empty() {
                            resource.name.clone()
                        } else {
                            resource.description.clone()
                        },
                        completable: true,
                    })
                    .collect(),
            )
        }
        (2, "call") => {
            let server_id = tokens.get(1).copied().unwrap_or("");
            Some(
                mcp_suggestions
                    .tools
                    .iter()
                    .filter(|tool| tool.server_id == server_id)
                    .map(|tool| SlashArgumentCompletion {
                        label: tool.name.clone(),
                        replacement: format!("{} ", tool.name),
                        description: if tool.description.trim().is_empty() {
                            format!("MCP tool `{}`", tool.provider_name)
                        } else {
                            tool.description.clone()
                        },
                        completable: true,
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

fn provider_tool_argument_completions() -> Vec<SlashArgumentCompletion> {
    [
        ("Agent", "Launch a local synchronous subagent"),
        ("Bash", "Execute a shell command"),
        ("Read", "Read file content"),
        ("Edit", "Apply an exact file replacement"),
        ("Write", "Write file content"),
        ("Glob", "Enumerate matching files"),
        ("Grep", "Search file content"),
        ("NotebookEdit", "Append a notebook cell"),
        ("WebFetch", "Fetch a URL"),
        ("WebSearch", "Search the web"),
        ("TodoWrite", "Persist a todo list snapshot"),
        ("TaskCreate", "Create a persistent task"),
        ("TaskGet", "Read a persistent task"),
        ("TaskList", "List persistent tasks"),
        ("TaskUpdate", "Update a persistent task"),
        ("TaskOutput", "Read background task output"),
        ("TaskStop", "Stop a background task"),
        ("EnterPlanMode", "Enter plan mode"),
        ("ExitPlanMode", "Exit plan mode"),
        (
            "VerifyPlanExecution",
            "Capture a plan verification snapshot",
        ),
        ("Skill", "Load a skill"),
        ("ToolSearch", "Search tool schemas"),
        ("LSP", "Run code intelligence queries"),
        ("ListMcpResourcesTool", "List MCP resources"),
        ("ListMcpToolsTool", "List MCP tools"),
        ("ReadMcpResourceTool", "Read an MCP resource"),
        ("CallMcpTool", "Call an MCP tool"),
    ]
    .into_iter()
    .map(|(label, description)| SlashArgumentCompletion {
        label: label.to_string(),
        replacement: format!("{label} "),
        description: description.to_string(),
        completable: true,
    })
    .collect()
}

pub(crate) fn slash_argument_completion_templates(
    command: SlashCommandSpec,
    hint: &str,
    argument_index: usize,
) -> Vec<SlashArgumentCompletion> {
    let tokens = hint
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split_whitespace()
        .collect::<Vec<_>>();
    let Some(token) = tokens.get(argument_index).copied() else {
        return Vec::new();
    };
    if !token.contains('|') {
        return Vec::new();
    }
    token
        .split('|')
        .filter_map(|choice| slash_argument_choice_completion(command, choice))
        .collect()
}

pub(crate) fn slash_argument_choice_completion(
    command: SlashCommandSpec,
    choice: &str,
) -> Option<SlashArgumentCompletion> {
    let label = choice
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_matches(',');
    if label.is_empty()
        || label.starts_with('<')
        || label.ends_with('>')
        || label.contains('"')
        || label
            .chars()
            .all(|character| character.is_ascii_uppercase())
    {
        return None;
    }
    Some(SlashArgumentCompletion {
        label: label.to_string(),
        replacement: format!("{label} "),
        description: slash_argument_choice_description(command.name, label).to_string(),
        completable: true,
    })
}

pub(crate) fn slash_argument_choice_description(command: &str, label: &str) -> &'static str {
    match (command, label) {
        ("mcp", "capabilities") => "show modeled MCP transport capabilities",
        ("mcp", "servers") => "list configured MCP servers",
        ("mcp", "resources") => "list resources from an MCP server",
        ("mcp", "tools") => "list tools from an MCP server",
        ("mcp", "read") => "read an MCP resource",
        ("mcp", "call") => "call an MCP tool",
        _ => "complete slash command argument",
    }
}

pub(crate) fn add_dir_completion_slice_and_view(
    input: &str,
    cursor: usize,
    cwd: &Path,
    selected: usize,
) -> Option<(usize, AddDirCompletionView)> {
    let (argument_start, argument) = add_dir_completion_argument(input, cursor)?;
    let suggestions = add_dir_completion_suggestions(cwd, argument);
    if suggestions.is_empty() {
        return None;
    }
    let selected = selected.min(suggestions.len().saturating_sub(1));
    let visible_count = suggestions.len().min(ADD_DIR_COMPLETION_VISIBLE_ROWS);
    let start =
        slash_commands::slash_command_view_start(selected, suggestions.len(), visible_count);
    Some((
        argument_start,
        AddDirCompletionView {
            suggestions,
            selected,
            start,
            visible_count,
        },
    ))
}

pub(crate) fn add_dir_completion_argument(input: &str, cursor: usize) -> Option<(usize, &str)> {
    if cursor != input.len() {
        return None;
    }
    let rest = input.strip_prefix('/')?;
    let command_len = rest.find(char::is_whitespace)?;
    let command = &rest[..command_len];
    if command != "add-dir" && command != "add-directory" {
        return None;
    }
    let mut argument_start = 1 + command_len;
    while input
        .as_bytes()
        .get(argument_start)
        .is_some_and(u8::is_ascii_whitespace)
    {
        argument_start += 1;
    }
    let argument = &input[argument_start..cursor];
    if argument.contains('\n') {
        return None;
    }
    Some((argument_start, argument))
}

pub(crate) fn add_dir_completion_suggestions(cwd: &Path, argument: &str) -> Vec<AddDirCompletion> {
    let (parent_text, prefix) = split_add_dir_completion_argument(argument);
    let Some(parent_path) = resolve_add_dir_completion_parent(cwd, &parent_text) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(parent_path) else {
        return Vec::new();
    };
    let mut suggestions = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_dir() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && prefix.is_empty() {
                return None;
            }
            let score = add_dir_completion_match_score(&name, &prefix)?;
            Some((
                score,
                AddDirCompletion {
                    label: format!("{name}/"),
                    replacement: format!("{parent_text}{name}/"),
                },
            ))
        })
        .collect::<Vec<_>>();
    suggestions.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| {
                left.1
                    .label
                    .to_ascii_lowercase()
                    .cmp(&right.1.label.to_ascii_lowercase())
            })
            .then_with(|| left.1.label.cmp(&right.1.label))
    });
    suggestions
        .into_iter()
        .map(|(_, suggestion)| suggestion)
        .collect()
}

pub(crate) fn add_dir_completion_match_score(name: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    let lower_name = name.to_ascii_lowercase();
    let lower_query = query.to_ascii_lowercase();
    if lower_name == lower_query {
        return Some(0);
    }
    if lower_name.starts_with(&lower_query) {
        return Some(1);
    }
    fuzzy_match_score(name, query).map(|score| 100 + score)
}

pub(crate) fn split_add_dir_completion_argument(argument: &str) -> (String, String) {
    if argument.ends_with('/') || argument.ends_with('\\') {
        return (argument.to_string(), String::new());
    }
    let slash = argument.rfind('/');
    let backslash = argument.rfind('\\');
    let Some(index) = slash.into_iter().chain(backslash).max() else {
        return (String::new(), argument.to_string());
    };
    let split = index + 1;
    (argument[..split].to_string(), argument[split..].to_string())
}

pub(crate) fn resolve_add_dir_completion_parent(cwd: &Path, parent_text: &str) -> Option<PathBuf> {
    if parent_text.is_empty() {
        return Some(cwd.to_path_buf());
    }
    if parent_text == "~/" || parent_text == "~\\" || parent_text == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }
    if let Some(rest) = parent_text
        .strip_prefix("~/")
        .or_else(|| parent_text.strip_prefix("~\\"))
    {
        return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest));
    }
    let path = PathBuf::from(parent_text);
    if path.is_absolute() {
        Some(path)
    } else {
        Some(cwd.join(path))
    }
}

pub(crate) fn add_dir_completion_scrollbar_active(row: usize, view: &AddDirCompletionView) -> bool {
    suggestion_scrollbar_active(row, view.suggestions.len(), view.start, view.visible_count)
}
