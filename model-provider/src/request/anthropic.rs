use std::fmt::Write as _;

use orbcode_protocol::{
    EffortLevel, MessageRole, ToolResultContent, TranscriptBlock, TranscriptMessage, TurnContext,
};
use serde_json::{Value, json};

use crate::ProviderRequest;

use super::{apply_extra_body, deserialize_block_payload, truncate_tool_result_for_provider};

pub fn build_anthropic_request_body(request: &ProviderRequest) -> Value {
    let mut messages = anthropic_messages(&request.messages, &request.prompt);
    if let Some(user_context) = anthropic_user_context_message(&request.context) {
        messages.insert(0, user_context);
    }
    let thinking_budget = anthropic_thinking_budget(request);
    let default_max_tokens = thinking_budget.map_or(4096, |budget| budget + 4096);
    let max_tokens = request
        .options
        .max_output_tokens
        .map_or(default_max_tokens, u64::from);

    let mut body = json!({
        "model": request.model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": messages,
        "system": anthropic_system_prompt(request),
    });

    if request.disable_thinking {
        body["thinking"] = json!({ "type": "disabled" });
    } else if let Some(budget) = thinking_budget {
        body["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": budget,
        });
    }

    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": sanitize_anthropic_json_schema(tool.input_schema.clone()),
                    })
                })
                .collect(),
        );
        body["tool_choice"] = json!({ "type": "auto" });
    }

    if let Some(temperature) = request.options.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(metadata) = &request.options.metadata {
        body["metadata"] = metadata.clone();
    }
    apply_extra_body(&mut body, &request.options.extra_body);
    apply_anthropic_betas(&mut body, &request.options.anthropic_betas);

    body
}

/// Remove tool-search beta fields from a JSON message array before sending it
/// to a count-tokens endpoint.
///
/// Mirrors TypeScript's `stripSearchExtraToolsFieldsFromMessages`: it drops the
/// `caller` field (and any other non-canonical keys) from `tool_use` blocks and
/// filters `tool_reference` blocks out of `tool_result` content. These fields
/// are only accepted when the tool-search beta header is set, so leaving them on
/// a count-tokens request produces a 400.
pub fn strip_search_extra_tools_fields(messages: &mut [Value]) {
    for message in messages.iter_mut() {
        let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        for block in content.iter_mut() {
            let Some(obj) = block.as_object_mut() else {
                continue;
            };
            match obj.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    let id = obj.get("id").cloned();
                    let name = obj.get("name").cloned();
                    let input = obj.get("input").cloned();
                    obj.clear();
                    obj.insert("type".to_string(), Value::String("tool_use".to_string()));
                    if let Some(id) = id {
                        obj.insert("id".to_string(), id);
                    }
                    if let Some(name) = name {
                        obj.insert("name".to_string(), name);
                    }
                    if let Some(input) = input {
                        obj.insert("input".to_string(), input);
                    }
                }
                Some("tool_result") => {
                    if let Some(inner) = obj.get_mut("content").and_then(Value::as_array_mut) {
                        inner.retain(|c| !is_tool_reference_block(c));
                        if inner.is_empty() {
                            obj.insert(
                                "content".to_string(),
                                json!([{ "type": "text", "text": "[tool references]" }]),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Build a Bedrock-flavored count-tokens request body.
///
/// Mirrors TypeScript's `countTokensWithBedrock` request shape: the Anthropic
/// version is the Bedrock literal, betas ride on `anthropic_beta`, and a dummy
/// user message is injected when there are no messages so tool-only counts are
/// still accurate.
pub fn build_bedrock_count_tokens_request_body(request: &ProviderRequest) -> Value {
    let mut messages = anthropic_messages(&request.messages, &request.prompt);
    if let Some(user_context) = anthropic_user_context_message(&request.context) {
        messages.insert(0, user_context);
    }
    strip_search_extra_tools_fields(&mut messages);
    if messages.is_empty() {
        messages.push(json!({ "role": "user", "content": "foo" }));
    }

    let thinking_enabled = !request.disable_thinking && request.effort.is_some();
    let mut body = json!({
        "anthropic_version": "bedrock-2023-05-31",
        "messages": messages,
        "max_tokens": if thinking_enabled { 2048 } else { 1 },
    });

    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": sanitize_anthropic_json_schema(tool.input_schema.clone()),
                    })
                })
                .collect(),
        );
    }

    if !request.options.anthropic_betas.is_empty() {
        let mut seen = std::collections::HashSet::new();
        let betas: Vec<Value> = request
            .options
            .anthropic_betas
            .iter()
            .filter(|beta| seen.insert((*beta).clone()))
            .map(|beta| Value::String(beta.clone()))
            .collect();
        body["anthropic_beta"] = Value::Array(betas);
    }

    if thinking_enabled {
        body["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": 1024,
        });
    }

    body
}

pub fn build_anthropic_count_tokens_request_body(request: &ProviderRequest) -> Value {
    let mut messages = anthropic_messages(&request.messages, &request.prompt);
    if let Some(user_context) = anthropic_user_context_message(&request.context) {
        messages.insert(0, user_context);
    }
    strip_search_extra_tools_fields(&mut messages);
    let thinking_budget = anthropic_thinking_budget(request);

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "system": anthropic_system_prompt(request),
    });

    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": sanitize_anthropic_json_schema(tool.input_schema.clone()),
                    })
                })
                .collect(),
        );
    }

    if request.disable_thinking {
        body["thinking"] = json!({ "type": "disabled" });
    } else if let Some(budget) = thinking_budget {
        body["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": budget,
        });
    }

    apply_extra_body(&mut body, &request.options.extra_body);
    apply_anthropic_betas(&mut body, &request.options.anthropic_betas);

    body
}

pub(super) fn anthropic_messages(
    messages: &[TranscriptMessage],
    fallback_prompt: &str,
) -> Vec<Value> {
    let mut serialized = messages
        .iter()
        .filter_map(anthropic_message)
        .collect::<Vec<_>>();

    if serialized.is_empty() {
        serialized.push(json!({
            "role": "user",
            "content": fallback_prompt,
        }));
    }

    serialized
}

fn anthropic_system_prompt(request: &ProviderRequest) -> String {
    let mut sections = Vec::new();
    if !request.system_prompt.trim().is_empty() {
        sections.push(request.system_prompt.trim_end().to_string());
    }
    sections.extend(request.messages.iter().filter_map(|message| {
        if message.role != MessageRole::System {
            return None;
        }
        let content = message.content.trim();
        (!content.is_empty()).then(|| content.to_string())
    }));
    sections.join("\n\n")
}

pub(super) fn anthropic_user_context_message(context: &TurnContext) -> Option<Value> {
    let mut entries = Vec::new();
    entries.push((
        "currentDate",
        format!("Today's date is {}.", context.current_date),
    ));
    entries.push((
        "currentWorkingDirectory",
        format!("The current working directory is {}.", context.cwd),
    ));
    if !context.additional_directories.is_empty() {
        entries.push((
            "additionalWorkingDirectories",
            format!(
                "Additional working directories are: {}.",
                context.additional_directories.join(", ")
            ),
        ));
    }
    if let Some(repo_root) = context.repo_root.as_deref() {
        entries.push((
            "repositoryRoot",
            format!("The repository root is {repo_root}."),
        ));
    }
    if let Some(relative) = context.cwd_relative_to_repo.as_deref() {
        entries.push((
            "repositorySubdirectory",
            format!(
                "The current working directory is the `{relative}` subdirectory inside the repository."
            ),
        ));
    }
    if let Some(git_branch) = context.git_branch.as_deref() {
        entries.push((
            "gitBranch",
            format!("The current git branch is {git_branch}."),
        ));
    }
    if let Some(default_branch) = context.git_default_branch.as_deref() {
        entries.push((
            "gitDefaultBranch",
            format!("Main branch (you will usually use this for PRs): {default_branch}"),
        ));
    }
    if let Some(state) = context.git_worktree_state
        && !matches!(state, orbcode_protocol::WorktreeState::Normal)
    {
        entries.push((
            "gitWorktreeState",
            format!("Worktree state: {}.", state.as_label()),
        ));
    }
    if let Some(user) = context.git_user.as_deref() {
        entries.push(("gitUser", format!("Git user: {user}")));
    }
    if let Some(remote) = context.git_remote.as_deref() {
        entries.push(("gitRemote", format!("Git remote: {remote}")));
    }
    if let Some(git_status) = context.git_status.as_deref() {
        entries.push(("gitStatus", format!("Status:\n{git_status}")));
    }
    if let Some(commits) = context.git_recent_commits.as_deref() {
        entries.push(("gitRecentCommits", format!("Recent commits:\n{commits}")));
    }
    if !context.additional_directory_details.is_empty() {
        let block = context
            .additional_directory_details
            .iter()
            .map(|detail| {
                let mut line = format!("- {}", detail.path);
                if detail.has_claude_md {
                    line.push_str(" (CLAUDE.md)");
                }
                if let Some(branch) = detail.git_branch.as_deref() {
                    write!(line, " [branch: {branch}]").expect("writing to String cannot fail");
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n");
        entries.push(("additionalDirectoryDetails", block));
    }
    if let Some(trusted) = context.trusted_project {
        let label = if trusted { "trusted" } else { "not trusted" };
        entries.push(("trustedProject", format!("Project trust: {label}.")));
    }
    if let Some(claude_md) = context.claude_md.as_deref()
        && !claude_md.trim().is_empty()
    {
        entries.push(("claudeMd", claude_md.to_string()));
    }

    if entries.is_empty() {
        return None;
    }

    let reminder = format!(
        concat!(
            "<system-reminder>\n",
            "As you answer the user's questions, you can use the following context:\n",
            "{entries}\n\n",
            "IMPORTANT: this context may or may not be relevant to your tasks. ",
            "You should not respond to this context unless it is highly relevant to your task.\n",
            "</system-reminder>\n"
        ),
        entries = entries
            .into_iter()
            .map(|(key, value)| format!("# {key}\n{value}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    Some(json!({
        "role": "user",
        "content": [{
            "type": "text",
            "text": reminder,
        }],
    }))
}

fn apply_anthropic_betas(body: &mut Value, betas: &[String]) {
    if betas.is_empty() {
        return;
    }
    if let Value::Object(map) = body {
        let existing = map
            .remove("anthropic_beta")
            .and_then(|value| match value {
                Value::Array(items) => Some(items),
                _ => None,
            })
            .unwrap_or_default();
        let mut seen = std::collections::BTreeSet::new();
        let mut merged: Vec<Value> = Vec::with_capacity(existing.len() + betas.len());
        for item in existing {
            if let Some(beta) = item.as_str() {
                if seen.insert(beta.to_string()) {
                    merged.push(Value::String(beta.to_string()));
                }
            } else {
                merged.push(item);
            }
        }
        for beta in betas {
            if seen.insert(beta.clone()) {
                merged.push(Value::String(beta.clone()));
            }
        }
        map.insert("anthropic_beta".to_string(), Value::Array(merged));
    }
}

fn anthropic_thinking_budget_tokens(effort: EffortLevel) -> u64 {
    match effort {
        EffortLevel::Low => 1024,
        EffortLevel::Medium => 4096,
        EffortLevel::Max => 16384,
        _ => 8192,
    }
}

fn anthropic_thinking_budget(request: &ProviderRequest) -> Option<u64> {
    if request.disable_thinking {
        return None;
    }
    request
        .options
        .max_thinking_tokens
        .map(u64::from)
        .or_else(|| request.effort.map(anthropic_thinking_budget_tokens))
}

fn is_tool_reference_block(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("tool_reference")
}

/// The Anthropic API rejects `anyOf`/`oneOf`/`allOf` at the top level of a
/// tool `input_schema` (nested uses are fine). Our catalog uses top-level
/// `anyOf` to express "one of these alias keys is required"; strip those root
/// combinators so the request validates while leaving the property shape and
/// any nested combinators intact.
fn sanitize_anthropic_json_schema(schema: Value) -> Value {
    match schema {
        Value::Object(mut object) => {
            object.remove("anyOf");
            object.remove("oneOf");
            object.remove("allOf");
            Value::Object(object)
        }
        other => other,
    }
}

pub(super) fn anthropic_message(message: &TranscriptMessage) -> Option<Value> {
    let role = match message.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        _ => return None,
    };

    let content = if message.blocks.is_empty() {
        if message.content.trim().is_empty() {
            Vec::new()
        } else {
            vec![json!({
                "type": "text",
                "text": message.content,
            })]
        }
    } else {
        message
            .blocks
            .iter()
            .filter_map(anthropic_block)
            .collect::<Vec<_>>()
    };

    if content.is_empty() {
        None
    } else {
        Some(json!({
            "role": role,
            "content": content,
        }))
    }
}

fn anthropic_block(block: &TranscriptBlock) -> Option<Value> {
    match block {
        TranscriptBlock::Text { text } => Some(json!({
            "type": "text",
            "text": text,
        })),
        TranscriptBlock::Thinking { text, signature } => Some(json!({
            "type": "thinking",
            "thinking": text,
            "signature": signature.clone().unwrap_or_default(),
        })),
        TranscriptBlock::ToolUse { id, name, input } => Some(json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": deserialize_block_payload(input),
        })),
        TranscriptBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => Some(json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": anthropic_tool_result_content(content),
            "is_error": is_error,
        })),
        _ => None,
    }
}

fn anthropic_tool_result_content(content: &ToolResultContent) -> Value {
    let value = content.anthropic_value();
    match value {
        Value::String(text) => Value::String(truncate_tool_result_for_provider(&text)),
        Value::Array(_) => {
            let compact = serde_json::to_string(&value).unwrap_or_else(|_| value.to_string());
            let truncated = truncate_tool_result_for_provider(&compact);
            if truncated == compact {
                value
            } else {
                Value::String(truncated)
            }
        }
        // `ToolResultContent::anthropic_value` currently returns only string
        // or array, but keep this deterministic if that evolves.
        other => {
            let compact = serde_json::to_string(&other).unwrap_or_else(|_| other.to_string());
            Value::String(truncate_tool_result_for_provider(&compact))
        }
    }
}

#[cfg(test)]
mod tool_result_content_tests {
    use orbcode_protocol::TranscriptJsonField;

    use super::*;

    #[test]
    fn mixed_loaded_content_preserves_supported_blocks_and_member_order() {
        let original = json!([
            {"type": "text", "text": "first"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AA=="}},
            {"type": "structured", "payload": {"answer": 42}},
            {"type": "text", "text": "last"}
        ]);
        let content = ToolResultContent::from_loaded(TranscriptJsonField::Value(original));

        let mapped = anthropic_tool_result_content(&content);
        let items = mapped.as_array().expect("native content array");
        assert_eq!(items.len(), 4);
        assert_eq!(items[0], json!({"type": "text", "text": "first"}));
        assert_eq!(items[1]["type"], "image");
        assert_eq!(
            serde_json::from_str::<Value>(items[2]["text"].as_str().expect("JSON text"))
                .expect("structured member JSON"),
            json!({"type": "structured", "payload": {"answer": 42}})
        );
        assert_eq!(items[3], json!({"type": "text", "text": "last"}));
    }

    #[test]
    fn null_absent_and_object_map_to_valid_lossless_strings() {
        let absent = ToolResultContent::from_loaded(TranscriptJsonField::Absent);
        let null = ToolResultContent::from_loaded(TranscriptJsonField::Null);
        let object = ToolResultContent::from_loaded(TranscriptJsonField::Value(json!({
            "answer": 42,
            "nested": [true, null]
        })));

        assert_eq!(anthropic_tool_result_content(&absent), "");
        assert_eq!(anthropic_tool_result_content(&null), "");
        assert_eq!(
            serde_json::from_str::<Value>(
                anthropic_tool_result_content(&object)
                    .as_str()
                    .expect("object JSON")
            )
            .expect("parse object projection"),
            json!({"answer": 42, "nested": [true, null]})
        );
    }
}
