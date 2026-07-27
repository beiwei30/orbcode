use std::path::Path;

use serde_json::Value;

pub(crate) fn parse_tool_input(input: &str) -> Option<Value> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        serde_json::from_str(trimmed).ok()
    }
}

pub(crate) fn first_string_field(value: Option<&Value>, keys: &[&str]) -> Option<String> {
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

pub(crate) fn display_tool_path(path: &str, cwd: &Path) -> String {
    let path = Path::new(path);
    if let Ok(relative) = path.strip_prefix(cwd) {
        let rendered = relative.display().to_string();
        if !rendered.is_empty() {
            return rendered;
        }
    }

    let rendered = path.display().to_string();
    if let Ok(home) = std::env::var("HOME")
        && let Some(suffix) = rendered.strip_prefix(&home)
    {
        return format!("~{suffix}");
    }

    rendered
}

pub(crate) fn is_search_command(command: &str) -> bool {
    let lowered = command.to_ascii_lowercase();
    if (lowered.contains("| head") || lowered.contains("| wc") || lowered.contains("| tail"))
        && (lowered.contains("find ")
            || lowered.starts_with("find ")
            || lowered.contains(" fd ")
            || lowered.starts_with("fd "))
    {
        return false;
    }
    lowered.contains(" rg ")
        || lowered.starts_with("rg ")
        || lowered.contains(" grep ")
        || lowered.starts_with("grep ")
        || lowered.contains("find ")
        || lowered.contains("fd ")
}

pub(crate) fn is_list_command(command: &str) -> bool {
    let lowered = command.to_ascii_lowercase();
    lowered.starts_with("ls ")
        || lowered == "ls"
        || lowered.starts_with("tree ")
        || lowered == "tree"
        || lowered.starts_with("du ")
}

pub(crate) fn is_read_command(command: &str) -> bool {
    let lowered = command.to_ascii_lowercase();
    lowered.starts_with("cat ")
        || lowered.starts_with("sed ")
        || lowered.starts_with("head ")
        || lowered.starts_with("tail ")
        || lowered.starts_with("wc ")
}

pub(crate) fn extract_search_pattern_from_command(command: &str) -> Option<String> {
    command
        .split_whitespace()
        .find(|segment| segment.contains('*') || segment.contains('/') || segment.contains('.'))
        .map(|segment| format!("\"{segment}\""))
}

pub(crate) fn extract_read_path_from_command(command: &str, cwd: &Path) -> Option<String> {
    command
        .split_whitespace()
        .rev()
        .find(|segment| !segment.starts_with('-') && *segment != "|" && !segment.starts_with('$'))
        .map(|segment| {
            if segment.starts_with('/') {
                display_tool_path(segment, cwd)
            } else {
                segment.to_string()
            }
        })
}
