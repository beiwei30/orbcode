//! Shared tool-activity title formatter.
//!
//! Produces a human-readable label for a `(tool_name, tool_input_json)` pair
//! suitable for display in any client surface (TUI, ACP, web). This crate has
//! no concept of a workspace cwd, so paths are rendered as-is — overlays that
//! want cwd-relative paths can wrap the result.

use std::fmt::Write as _;

use serde_json::Value;

const TITLE_TRUNCATION_LIMIT: usize = 120;

/// Format a tool-activity title for the given `(name, input)` pair.
///
/// `input` is the tool's raw JSON input as a string. Invalid JSON is treated
/// the same as an empty input — the function falls back to the tool name (or
/// to a `name(description)` form when a top-level `description` field is
/// present in the parsed input).
pub fn format_tool_title(name: &str, input: &str) -> String {
    let parsed_input = parse_tool_input(input);
    let lowered = name.to_ascii_lowercase();

    if lowered == "agent" {
        let agent_type =
            first_string_field(parsed_input.as_ref(), &["subagent_type", "subagentType"])
                .map(|agent_type| {
                    if agent_type == "worker" {
                        "Agent".to_string()
                    } else {
                        agent_type
                    }
                })
                .unwrap_or_else(|| "Agent".to_string());
        if let Some(description) = first_string_field(parsed_input.as_ref(), &["description"]) {
            return format!("{agent_type}({})", collapse_inline_whitespace(&description));
        }
        return agent_type;
    }

    if matches!(
        lowered.as_str(),
        "taskcreate" | "taskget" | "tasklist" | "taskupdate" | "taskoutput" | "taskstop"
    ) {
        let label = if lowered == "taskcreate" {
            first_string_field(parsed_input.as_ref(), &["title", "description"])
        } else {
            first_string_field(
                parsed_input.as_ref(),
                &["task_id", "taskId", "id", "shell_id", "shellId"],
            )
        };
        if let Some(label) = label {
            return format!("{name}({})", collapse_inline_whitespace(&label));
        }
    }

    if lowered == "skill"
        && let Some(skill_name) = first_string_field(parsed_input.as_ref(), &["name", "skill"])
    {
        return format!("Skill({})", collapse_inline_whitespace(&skill_name));
    }

    if lowered == "toolsearch"
        && let Some(query) = first_string_field(parsed_input.as_ref(), &["query", "pattern"])
    {
        return format!("ToolSearch({})", collapse_inline_whitespace(&query));
    }

    if matches!(lowered.as_str(), "read" | "file-read") {
        if let Some(file_path) =
            first_string_field(parsed_input.as_ref(), &["file_path", "filePath", "path"])
        {
            let mut summary = file_path;
            if let Some(pages) = parsed_input
                .as_ref()
                .and_then(|value| value.get("pages"))
                .and_then(Value::as_u64)
            {
                write!(summary, " · pages {pages}").expect("writing to String cannot fail");
            } else {
                let offset = parsed_input
                    .as_ref()
                    .and_then(|value| value.get("offset"))
                    .and_then(Value::as_u64);
                let limit = parsed_input
                    .as_ref()
                    .and_then(|value| value.get("limit"))
                    .and_then(Value::as_u64);
                if let Some(offset) = offset {
                    if let Some(limit) = limit {
                        write!(
                            summary,
                            " · lines {offset}-{}",
                            offset + limit.saturating_sub(1)
                        )
                        .expect("writing to String cannot fail");
                    } else {
                        write!(summary, " · from line {offset}")
                            .expect("writing to String cannot fail");
                    }
                }
            }
            return format!("Read({})", truncate_chars(&summary, TITLE_TRUNCATION_LIMIT));
        }
        return "Read".to_string();
    }

    if matches!(
        lowered.as_str(),
        "grep" | "glob" | "websearch" | "web-search"
    ) {
        if lowered == "grep" {
            let regex = first_string_field(parsed_input.as_ref(), &["pattern", "query"]);
            let path = first_string_field(parsed_input.as_ref(), &["path"]);
            let mut parts = Vec::new();
            if let Some(regex) = regex {
                parts.push(format!("regex: {}", collapse_inline_whitespace(&regex)));
            }
            if let Some(path) = path {
                parts.push(format!("in: {}", collapse_inline_whitespace(&path)));
            }
            if parts.is_empty() {
                return "Search".to_string();
            }
            return format!(
                "Search({})",
                truncate_chars(&parts.join(", "), TITLE_TRUNCATION_LIMIT)
            );
        }

        let mut parts = Vec::new();
        if let Some(pattern) =
            first_string_field(parsed_input.as_ref(), &["pattern", "query", "glob"])
        {
            parts.push(format!(
                "pattern: \"{}\"",
                collapse_inline_whitespace(&pattern)
            ));
        }
        if let Some(path) =
            first_string_field(parsed_input.as_ref(), &["path", "file_path", "filePath"])
        {
            parts.push(format!("path: \"{}\"", collapse_inline_whitespace(&path)));
        }
        if parts.is_empty() {
            return "Search".to_string();
        }
        return format!(
            "Search({})",
            truncate_chars(&parts.join(", "), TITLE_TRUNCATION_LIMIT)
        );
    }

    if matches!(
        lowered.as_str(),
        "edit" | "file-edit" | "update" | "write" | "file-write"
    ) {
        let label = if matches!(lowered.as_str(), "edit" | "file-edit" | "update") {
            "Update"
        } else {
            "Write"
        };
        if let Some(file_path) =
            first_string_field(parsed_input.as_ref(), &["file_path", "filePath", "path"])
        {
            return format!(
                "{label}({})",
                truncate_chars(&file_path, TITLE_TRUNCATION_LIMIT)
            );
        }
        return label.to_string();
    }

    if lowered.starts_with("mcp__")
        && let Some((server, tool)) = lowered.strip_prefix("mcp__").and_then(|rest| {
            let parts: Vec<&str> = rest.splitn(2, "__").collect();
            if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
                Some((parts[0], parts[1]))
            } else {
                None
            }
        })
    {
        return format!("{server}:{tool}");
    }

    if matches!(lowered.as_str(), "bash" | "shell") {
        if let Some(command) =
            first_string_field(parsed_input.as_ref(), &["command", "cmd", "script"])
        {
            return format!(
                "{name}({})",
                truncate_chars(
                    &collapse_inline_whitespace(&command),
                    TITLE_TRUNCATION_LIMIT
                )
            );
        }
        if let Some(description) = first_string_field(parsed_input.as_ref(), &["description"]) {
            return format!("{name}({})", collapse_inline_whitespace(&description));
        }
    }

    if lowered == "lsp"
        && let Some(operation) = first_string_field(parsed_input.as_ref(), &["operation"])
    {
        return format!("LSP({})", collapse_inline_whitespace(&operation));
    }

    if matches!(
        lowered.as_str(),
        "listmcpresourcestool" | "listmcptoolstool" | "readmcpresourcetool" | "callmcptool"
    ) {
        let server = first_string_field(parsed_input.as_ref(), &["server_id", "serverId"]);
        let item = if lowered == "callmcptool" {
            first_string_field(parsed_input.as_ref(), &["tool_name", "tool"])
        } else if lowered == "readmcpresourcetool" {
            first_string_field(parsed_input.as_ref(), &["uri"])
        } else {
            None
        };
        if let Some(server) = server {
            if let Some(item) = item {
                return format!(
                    "{name}({}.{})",
                    collapse_inline_whitespace(&server),
                    collapse_inline_whitespace(&item)
                );
            }
            return format!("{name}({})", collapse_inline_whitespace(&server));
        }
    }

    if let Some(description) = first_string_field(parsed_input.as_ref(), &["description"]) {
        return format!("{name}({})", collapse_inline_whitespace(&description));
    }

    name.to_string()
}

fn parse_tool_input(input: &str) -> Option<Value> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        serde_json::from_str(trimmed).ok()
    }
}

fn first_string_field(value: Option<&Value>, keys: &[&str]) -> Option<String> {
    let value = value?;
    for key in keys {
        if let Some(found) = value.get(*key).and_then(Value::as_str)
            && !found.trim().is_empty()
        {
            return Some(found.to_string());
        }
    }
    None
}

fn collapse_inline_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut output = String::new();
    let total = input.chars().count();
    for ch in input.chars().take(max_chars) {
        output.push(ch);
    }
    if total > max_chars && max_chars > 1 {
        output.pop();
        output.push('…');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_with_description_returns_subagent_type() {
        let title = format_tool_title(
            "Agent",
            r#"{"subagent_type":"Explore","description":"orbcode project structure"}"#,
        );
        assert_eq!(title, "Explore(orbcode project structure)");
    }

    #[test]
    fn agent_worker_subtype_renders_as_agent() {
        let title = format_tool_title(
            "Agent",
            r#"{"subagent_type":"worker","description":"do thing"}"#,
        );
        assert_eq!(title, "Agent(do thing)");
    }

    #[test]
    fn agent_without_description_returns_agent_only() {
        let title = format_tool_title("Agent", r#"{"subagent_type":"Explore"}"#);
        assert_eq!(title, "Explore");
    }

    #[test]
    fn bash_command_truncates_inline_whitespace() {
        let title = format_tool_title("Bash", r#"{"command":"ls   -la\n  /tmp"}"#);
        assert_eq!(title, "Bash(ls -la /tmp)");
    }

    #[test]
    fn bash_falls_back_to_description() {
        let title = format_tool_title("Bash", r#"{"description":"list files"}"#);
        assert_eq!(title, "Bash(list files)");
    }

    #[test]
    fn read_with_path_only() {
        let title = format_tool_title("Read", r#"{"file_path":"/repo/Cargo.toml"}"#);
        assert_eq!(title, "Read(/repo/Cargo.toml)");
    }

    #[test]
    fn read_with_offset_and_limit() {
        let title = format_tool_title(
            "Read",
            r#"{"file_path":"/repo/main.rs","offset":10,"limit":5}"#,
        );
        assert_eq!(title, "Read(/repo/main.rs · lines 10-14)");
    }

    #[test]
    fn read_pages_takes_precedence_over_offset() {
        let title = format_tool_title(
            "Read",
            r#"{"file_path":"/doc.pdf","pages":3,"offset":1,"limit":2}"#,
        );
        assert_eq!(title, "Read(/doc.pdf · pages 3)");
    }

    #[test]
    fn write_uses_path() {
        let title = format_tool_title("Write", r#"{"file_path":"/tmp/x.txt","content":"hi"}"#);
        assert_eq!(title, "Write(/tmp/x.txt)");
    }

    #[test]
    fn edit_renders_as_update_with_path() {
        let title = format_tool_title("Edit", r#"{"file_path":"/tmp/x.txt"}"#);
        assert_eq!(title, "Update(/tmp/x.txt)");
    }

    #[test]
    fn grep_renders_regex_and_path() {
        let title = format_tool_title("Grep", r#"{"pattern":"foo","path":"src/"}"#);
        assert_eq!(title, "Search(regex: foo, in: src/)");
    }

    #[test]
    fn glob_renders_pattern_and_path() {
        let title = format_tool_title("Glob", r#"{"pattern":"**/*.rs","path":"."}"#);
        assert_eq!(title, "Search(pattern: \"**/*.rs\", path: \".\")");
    }

    #[test]
    fn skill_renders_skill_name() {
        let title = format_tool_title("Skill", r#"{"name":"verify"}"#);
        assert_eq!(title, "Skill(verify)");
    }

    #[test]
    fn taskcreate_uses_title_field() {
        let title = format_tool_title("TaskCreate", r#"{"title":"Build feature","status":"todo"}"#);
        assert_eq!(title, "TaskCreate(Build feature)");
    }

    #[test]
    fn taskget_uses_id() {
        let title = format_tool_title("TaskGet", r#"{"task_id":"task-42"}"#);
        assert_eq!(title, "TaskGet(task-42)");
    }

    #[test]
    fn mcp_prefixed_tool_name_renders_server_and_tool() {
        let title = format_tool_title("mcp__github__create_issue", "{}");
        assert_eq!(title, "github:create_issue");
    }

    #[test]
    fn websearch_renders_query() {
        let title = format_tool_title("WebSearch", r#"{"query":"rust async"}"#);
        assert_eq!(title, "Search(pattern: \"rust async\")");
    }

    #[test]
    fn lsp_renders_operation() {
        let title = format_tool_title("LSP", r#"{"operation":"hover"}"#);
        assert_eq!(title, "LSP(hover)");
    }

    #[test]
    fn unknown_tool_falls_back_to_name() {
        let title = format_tool_title("WeirdTool", r#"{"unknown":"x"}"#);
        assert_eq!(title, "WeirdTool");
    }

    #[test]
    fn unknown_tool_with_description_uses_description() {
        let title = format_tool_title("WeirdTool", r#"{"description":"do thing"}"#);
        assert_eq!(title, "WeirdTool(do thing)");
    }

    #[test]
    fn invalid_json_falls_back_to_name() {
        let title = format_tool_title("Read", "not json");
        assert_eq!(title, "Read");
    }

    #[test]
    fn long_path_is_truncated() {
        let long = "a".repeat(200);
        let input = format!(r#"{{"file_path":"{long}"}}"#);
        let title = format_tool_title("Read", &input);
        assert!(title.starts_with("Read("));
        assert!(title.ends_with(")"));
        assert!(title.contains('…'));
    }
}
