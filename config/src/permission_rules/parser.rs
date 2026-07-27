//! Permission-rule string parsing.
//!
//! Splits the raw `Tool(pattern)` rule text into a [`PermissionRule`] and
//! validates the shape on edit. The matching machinery (path matching,
//! bash-rule evaluation, etc.) lives in `mod.rs`; this submodule is the
//! syntax layer.

use super::{PermissionRule, canonical_tool_name};

pub(super) fn parse(raw: &str) -> PermissionRule {
    let trimmed = raw.trim();
    let Some(open) = find_first_unescaped(trimmed, '(') else {
        return PermissionRule {
            raw: trimmed.to_string(),
            tool_name: canonical_tool_name(trimmed),
            rule_content: None,
        };
    };

    let Some(close) = find_last_unescaped(trimmed, ')') else {
        return PermissionRule {
            raw: trimmed.to_string(),
            tool_name: canonical_tool_name(trimmed),
            rule_content: None,
        };
    };

    if close <= open || close != trimmed.len().saturating_sub(1) || open == 0 {
        return PermissionRule {
            raw: trimmed.to_string(),
            tool_name: canonical_tool_name(trimmed),
            rule_content: None,
        };
    }

    let tool_name = &trimmed[..open];
    let raw_content = &trimmed[open + 1..close];
    let content = unescape_rule_content(raw_content);

    PermissionRule {
        raw: trimmed.to_string(),
        tool_name: canonical_tool_name(tool_name),
        rule_content: if content.is_empty() || content == "*" {
            None
        } else {
            Some(content)
        },
    }
}

pub(super) fn normalize_for_edit(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("permission rule cannot be empty".to_string());
    }

    let first_open = find_first_unescaped(trimmed, '(');
    let last_close = find_last_unescaped(trimmed, ')');
    if first_open.is_some() || last_close.is_some() {
        let Some(open) = first_open else {
            return Err("permission rule has a closing `)` without an opening `(`".to_string());
        };
        let Some(close) = last_close else {
            return Err("permission rule has an opening `(` without a closing `)`".to_string());
        };
        if open == 0 {
            return Err("permission rule must include a tool name before `(`".to_string());
        }
        if close <= open || close != trimmed.len().saturating_sub(1) {
            return Err("permission rule must use the form Tool(pattern)".to_string());
        }
    }

    Ok(parse(trimmed).raw)
}

fn find_first_unescaped(value: &str, needle: char) -> Option<usize> {
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == needle {
            return Some(index);
        }
    }
    None
}

fn find_last_unescaped(value: &str, needle: char) -> Option<usize> {
    let mut escaped = false;
    let mut result = None;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == needle {
            result = Some(index);
        }
    }
    result
}

fn unescape_rule_content(value: &str) -> String {
    value.replace("\\(", "(").replace("\\)", ")")
}
