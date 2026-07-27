use orbcode_session_store::deserialize_block_payload;
use serde_json::{Value, json};

pub(crate) fn initial_tool_progress_record(
    tool_use_id: &str,
    tool_name: &str,
    tool_input: &str,
) -> Value {
    json!({
        "data": {
            "type": "tool_progress",
            "status": initial_tool_progress_status(tool_name, tool_input),
            "message": {
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": tool_use_id,
                            "name": tool_name,
                            "input": deserialize_block_payload(tool_input),
                        }
                    ]
                }
            }
        }
    })
}

pub(crate) fn initial_tool_progress_status(tool_name: &str, tool_input: &str) -> &'static str {
    match classify_initial_tool_progress(tool_name, tool_input) {
        InitialToolProgressKind::LaunchAgent => "Launching agent",
        InitialToolProgressKind::Search => "Searching for 1 pattern",
        InitialToolProgressKind::Read => "Reading 1 file",
        InitialToolProgressKind::List => "Listing 1 directory",
        InitialToolProgressKind::ListMcpResources => "Listing MCP resources",
        InitialToolProgressKind::Bash => "Running 1 bash command",
        InitialToolProgressKind::Work => "Working",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InitialToolProgressKind {
    LaunchAgent,
    Search,
    Read,
    List,
    ListMcpResources,
    Bash,
    Work,
}

fn classify_initial_tool_progress(tool_name: &str, tool_input: &str) -> InitialToolProgressKind {
    match tool_name.to_ascii_lowercase().as_str() {
        "agent" => InitialToolProgressKind::LaunchAgent,
        "file-read" | "read" | "read-mcp-resource" | "readmcpresourcetool" => {
            InitialToolProgressKind::Read
        }
        "glob" | "grep" | "web-search" | "websearch" => InitialToolProgressKind::Search,
        "list-mcp-resources" | "listmcpresourcestool" => InitialToolProgressKind::ListMcpResources,
        "bash" => classify_bash_progress(tool_input),
        _ => InitialToolProgressKind::Work,
    }
}

fn classify_bash_progress(tool_input: &str) -> InitialToolProgressKind {
    let payload = deserialize_block_payload(tool_input);
    let command = payload
        .as_object()
        .and_then(|object| {
            ["command", "cmd", "script"]
                .iter()
                .find_map(|key| object.get(*key).and_then(Value::as_str))
        })
        .unwrap_or(tool_input)
        .trim();
    let lowered = command.to_ascii_lowercase();

    if is_search_command(&lowered) {
        InitialToolProgressKind::Search
    } else if is_list_command(&lowered) {
        InitialToolProgressKind::List
    } else if is_read_command(&lowered) {
        InitialToolProgressKind::Read
    } else {
        InitialToolProgressKind::Bash
    }
}

fn is_search_command(command: &str) -> bool {
    command.contains(" rg ")
        || command.starts_with("rg ")
        || command.contains(" grep ")
        || command.starts_with("grep ")
        || command.contains("find ")
        || command.contains("fd ")
}

fn is_list_command(command: &str) -> bool {
    command.starts_with("ls ")
        || command == "ls"
        || command.starts_with("tree ")
        || command == "tree"
        || command.starts_with("du ")
}

fn is_read_command(command: &str) -> bool {
    command.starts_with("cat ")
        || command.starts_with("sed ")
        || command.starts_with("head ")
        || command.starts_with("tail ")
        || command.starts_with("wc ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_tool_progress_status_classifies_bash_inspection_commands() {
        assert_eq!(
            initial_tool_progress_status("bash", r#"{"command":"rg TurnContext orbcode"}"#),
            "Searching for 1 pattern"
        );
        assert_eq!(
            initial_tool_progress_status("bash", r#"{"command":"ls -la"}"#),
            "Listing 1 directory"
        );
        assert_eq!(
            initial_tool_progress_status("bash", r#"{"command":"cat README.md"}"#),
            "Reading 1 file"
        );
        assert_eq!(
            initial_tool_progress_status("Read", r#"{"file_path":"/tmp/README.md"}"#),
            "Reading 1 file"
        );
    }
}
