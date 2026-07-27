use std::path::Path;

use orbcode_config::{mcp_permission_target, suggested_bash_permission_rules};
use orbcode_protocol::PermissionRequest;
use serde_json::Value;

use crate::render::text_utils::collapse_inline_whitespace;

pub(crate) fn grep_regex_display_line(pattern: &str) -> String {
    format!("Regex {}", collapse_inline_whitespace(pattern))
}

pub(crate) fn string_value_any(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(ToString::to_string)
}

pub(crate) fn bool_value_any(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(key).and_then(Value::as_bool))
}

pub(crate) fn bash_permission_requests_sandbox_escalation(value: &Value) -> bool {
    string_value_any(value, &["sandbox_permissions", "sandboxPermissions"])
        .is_some_and(|permission| permission.trim() == "require_escalated")
        || bool_value_any(value, &["dangerouslyDisableSandbox"]).unwrap_or(false)
}

#[allow(dead_code)]
pub(crate) fn suggested_permission_rule(request: &PermissionRequest) -> Option<String> {
    suggested_permission_rules(request).into_iter().next()
}

pub(crate) fn suggested_permission_rules(request: &PermissionRequest) -> Vec<String> {
    let Ok(parsed) = serde_json::from_str::<Value>(&request.tool_input) else {
        return Vec::new();
    };
    match canonical_permission_tool_name(&request.tool_name).as_str() {
        "bash" => {
            let command = ["command", "cmd", "script"]
                .iter()
                .find_map(|key| parsed.get(key).and_then(Value::as_str));
            command
                .map(suggested_bash_permission_rules)
                .unwrap_or_default()
        }
        "file-read" | "file-write" | "file-edit" | "notebook-edit" => {
            let Some(path) = string_value_any(&parsed, &["file_path", "filePath", "path"]) else {
                return Vec::new();
            };
            file_permission_rule_for_path(&path).into_iter().collect()
        }
        "glob" => {
            let Some(path) = string_value_any(&parsed, &["path", "base"]) else {
                return Vec::new();
            };
            vec![format!("Glob({})", directory_rule_pattern(&path))]
        }
        "grep" => {
            let Some(path) = string_value_any(&parsed, &["path"]) else {
                return Vec::new();
            };
            vec![format!("Grep({})", directory_rule_pattern(&path))]
        }
        "call-mcp-tool" | "read-mcp-resource" | "list-mcp-resources" | "list-mcp-tools" => {
            mcp_permission_target(&request.tool_name, &request.tool_input)
                .into_iter()
                .collect()
        }
        "workflow" => vec!["Workflow".to_string()],
        _ => Vec::new(),
    }
}

fn file_permission_rule_for_path(path: &str) -> Option<String> {
    let parent = Path::new(path).parent()?;
    let parent = parent.to_str()?.trim();
    if parent.is_empty() || parent == "/" {
        return None;
    }
    Some(format!("File({})", directory_rule_pattern(parent)))
}

fn directory_rule_pattern(path: &str) -> String {
    let normalized = path.trim_end_matches('/');
    if normalized.is_empty() || normalized == "." {
        "./**".to_string()
    } else {
        format!("{normalized}/**")
    }
}

pub(crate) fn canonical_permission_tool_name(name: &str) -> String {
    match name {
        "Agent" | "agent" | "Task" | "task" => "Agent".to_string(),
        "Bash" | "bash" => "bash".to_string(),
        "Read" | "read" | "file-read" => "file-read".to_string(),
        "Write" | "write" | "file-write" => "file-write".to_string(),
        "Edit" | "edit" | "file-edit" => "file-edit".to_string(),
        "Glob" | "glob" => "glob".to_string(),
        "Grep" | "grep" => "grep".to_string(),
        "NotebookEdit" | "notebook-edit" => "notebook-edit".to_string(),
        "WebFetch" | "web-fetch" => "web-fetch".to_string(),
        "WebSearch" | "web-search" => "web-search".to_string(),
        "AskUserQuestion" | "ask-user-question" => "ask-user-question".to_string(),
        "TodoWrite" | "todo-write" => "todo-write".to_string(),
        "TaskCreate" | "task-create" => "task-create".to_string(),
        "TaskGet" | "task-get" => "task-get".to_string(),
        "TaskList" | "task-list" => "task-list".to_string(),
        "TaskUpdate" | "task-update" => "task-update".to_string(),
        "TaskOutput" | "TaskOutputTool" | "AgentOutputTool" | "BashOutputTool" | "task-output" => {
            "task-output".to_string()
        }
        "TaskStop" | "KillShell" | "task-stop" => "task-stop".to_string(),
        "EnterPlanMode" | "enter-plan-mode" => "enter-plan-mode".to_string(),
        "ExitPlanMode" | "exit-plan-mode" => "exit-plan-mode".to_string(),
        "VerifyPlanExecution" | "verify-plan-execution" => "verify-plan-execution".to_string(),
        "Skill" | "skill" => "skill".to_string(),
        "ToolSearch" | "tool-search" => "tool-search".to_string(),
        "Workflow" | "workflow" => "workflow".to_string(),
        "LSP" | "lsp" => "lsp".to_string(),
        "ListMcpResourcesTool" | "listMcpResources" | "list-mcp-resources" => {
            "list-mcp-resources".to_string()
        }
        "ListMcpToolsTool" | "listMcpTools" | "list-mcp-tools" => "list-mcp-tools".to_string(),
        "ReadMcpResourceTool" | "readMcpResource" | "read-mcp-resource" => {
            "read-mcp-resource".to_string()
        }
        "CallMcpTool" | "callMcpTool" | "call-mcp-tool" => "call-mcp-tool".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn human_tool_name(name: &str) -> String {
    match canonical_permission_tool_name(name).as_str() {
        "Agent" => "Agent".to_string(),
        "bash" => "Bash".to_string(),
        "file-read" => "Read".to_string(),
        "file-write" => "Write".to_string(),
        "file-edit" => "Edit".to_string(),
        "glob" => "Glob".to_string(),
        "grep" => "Grep".to_string(),
        "notebook-edit" => "Notebook edit".to_string(),
        "web-fetch" => "Web fetch".to_string(),
        "web-search" => "Web search".to_string(),
        "ask-user-question" => "Ask user question".to_string(),
        "todo-write" => "Todo write".to_string(),
        "task-create" => "Task create".to_string(),
        "task-get" => "Task get".to_string(),
        "task-list" => "Task list".to_string(),
        "task-update" => "Task update".to_string(),
        "task-output" => "Task output".to_string(),
        "task-stop" => "Task stop".to_string(),
        "enter-plan-mode" => "Enter plan mode".to_string(),
        "exit-plan-mode" => "Exit plan mode".to_string(),
        "verify-plan-execution" => "Verify plan execution".to_string(),
        "skill" => "Skill".to_string(),
        "tool-search" => "Tool search".to_string(),
        "workflow" => "Workflow".to_string(),
        "lsp" => "LSP".to_string(),
        "list-mcp-resources" => "List MCP resources".to_string(),
        "list-mcp-tools" => "List MCP tools".to_string(),
        "read-mcp-resource" => "Read MCP resource".to_string(),
        "call-mcp-tool" => "Call MCP tool".to_string(),
        other => human_field_label(other),
    }
}

pub(crate) fn friendly_bash_permission_rule_label(rule: &str) -> String {
    if rule.trim().ends_with(":*") {
        rule.to_string()
    } else {
        unescape_bash_permission_literal(rule.trim())
    }
}

pub(crate) fn friendly_bash_permission_rules_label(rules: &[String]) -> String {
    if rules.len() == 1 {
        return friendly_bash_permission_rule_label(&rules[0]);
    }

    let mut commands = Vec::new();
    for rule in rules {
        let label = friendly_bash_permission_rule_label(rule);
        let command = label
            .strip_suffix(":*")
            .unwrap_or(&label)
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        if !command.is_empty() && !commands.iter().any(|existing| existing == &command) {
            commands.push(command);
        }
    }

    match commands.as_slice() {
        [] => String::new(),
        [one] => format!("{one} command"),
        [one, two] => format!("{one} and {two} commands"),
        [one, two, rest @ ..] => format!("{one}, {two}, and {} more commands", rest.len()),
    }
}

pub(crate) fn friendly_permission_rule_label(rule: &str) -> String {
    let trimmed = rule.trim();
    if trimmed.starts_with("mcp__") {
        return friendly_mcp_permission_rule_label(trimmed);
    }
    let Some((tool, pattern)) = split_permission_rule(trimmed) else {
        return trimmed.to_string();
    };
    let scope = friendly_rule_scope(&pattern);
    match tool {
        "File" => format!("file operations in {scope}"),
        "Grep" => format!("searches under {scope}"),
        "Glob" => format!("file searches under {scope}"),
        _ => trimmed.to_string(),
    }
}

fn split_permission_rule(rule: &str) -> Option<(&str, String)> {
    let open = rule.find('(')?;
    let close = rule.rfind(')')?;
    (close > open && close == rule.len().saturating_sub(1))
        .then(|| (&rule[..open], rule[open + 1..close].to_string()))
}

fn friendly_rule_scope(pattern: &str) -> String {
    pattern
        .strip_suffix("/**")
        .unwrap_or(pattern)
        .strip_prefix("./")
        .unwrap_or_else(|| pattern.strip_suffix("/**").unwrap_or(pattern))
        .to_string()
}

fn friendly_mcp_permission_rule_label(rule: &str) -> String {
    let parts = rule.split("__").collect::<Vec<_>>();
    match parts.as_slice() {
        ["mcp", server, "*"] => format!("all MCP calls on {server}"),
        ["mcp", server, tool] => format!("MCP {server}::{tool}"),
        _ => rule.to_string(),
    }
}

fn unescape_bash_permission_literal(value: &str) -> String {
    let mut result = String::new();
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            if character == '*' || character == '\\' {
                result.push(character);
            } else {
                result.push('\\');
                result.push(character);
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            result.push(character);
        }
    }
    if escaped {
        result.push('\\');
    }
    result
}

pub(crate) fn human_field_label(key: &str) -> String {
    match key {
        "-i" => return "Case insensitive".to_string(),
        "-n" => return "Line numbers".to_string(),
        "uri" => return "URI".to_string(),
        "url" => return "URL".to_string(),
        "input" => return "Tool input".to_string(),
        "lsp" | "LSP" => return "LSP".to_string(),
        _ => {}
    }

    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous_was_lower_or_digit = false;
    for character in key.replace(['_', '-'], " ").chars() {
        if character.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous_was_lower_or_digit = false;
            continue;
        }
        if character.is_uppercase() && previous_was_lower_or_digit && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        previous_was_lower_or_digit = character.is_lowercase() || character.is_ascii_digit();
        current.push(character);
    }
    if !current.is_empty() {
        words.push(current);
    }
    if words.is_empty() {
        return key.to_string();
    }
    words
        .into_iter()
        .map(|word| {
            let lower = word.to_ascii_lowercase();
            let mut chars = lower.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => lower,
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn file_name_for_display(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or(path)
        .to_string()
}

pub(crate) fn parent_path_for_display(path: &str) -> String {
    Path::new(path)
        .parent()
        .and_then(|parent| parent.to_str())
        .unwrap_or("")
        .to_string()
}
