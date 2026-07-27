//! Word and value extraction helpers for tree-sitter-bash nodes.
//!
//! Provides functions to extract literal string values from various bash
//! node types (words, strings, concatenations, variable assignments) and
//! to tokenize command nodes into argv-style word lists.

use tree_sitter::Node;

use super::find_single_simple_command;

/// Tree-sitter flags inputs like `function NAME`, `coproc NAME` (without a
/// body), and similar partial constructs as parse errors. The bespoke
/// tokenizer simply split such inputs on whitespace, which is what the
/// compound-construct boundary detectors (`bash_function_body_boundary`,
/// `bash_coproc_body_boundary`, `bash_time_compound_boundary`) rely on when
/// matching a `function NAME { ... }` body's prefix.
///
/// To preserve that behaviour without re-introducing a hand-rolled split,
/// recover a token sequence from the ERROR node's literal children — but
/// fail-closed for any unsafe shape (quoting, expansions, operator
/// characters), since this path is only meant to recover keyword-prefix
/// situations that the bespoke implementation also accepted.
pub(in crate::permission_rules::bash_ast) fn error_node_partial_tokens(
    root: Node<'_>,
    src: &[u8],
) -> Option<Vec<String>> {
    if root.kind() != "program" {
        return None;
    }
    let mut cursor = root.walk();
    let children: Vec<_> = root.children(&mut cursor).collect();
    if children.len() != 1 {
        return None;
    }
    let error = children[0];
    if error.kind() != "ERROR" {
        return None;
    }

    let mut tokens = Vec::new();
    let mut child_cursor = error.walk();
    for child in error.children(&mut child_cursor) {
        if child.is_named() {
            match child.kind() {
                "word" | "number" => tokens.push(literal_word_value(child, src)?),
                "string" => tokens.push(string_value(child, src)?),
                "raw_string" => tokens.push(raw_string_value(child, src)?),
                "ansi_c_string" => tokens.push(ansi_c_string_value(child, src)?),
                "concatenation" => tokens.push(concatenation_value(child, src)?),
                "variable_assignment" => tokens.push(variable_assignment_value(child, src)?),
                _ => return None,
            }
        } else {
            let text = child.utf8_text(src).ok()?;
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return None;
            }
            tokens.push(trimmed.to_string());
        }
    }
    if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

/// Standalone variable assignments at program level (`FOO=bar`,
/// `FOO=bar BAZ=qux`) are parsed as a sequence of `variable_assignment` nodes
/// directly under `program` rather than wrapped in a `command`. Treat each one
/// as a literal `NAME=value` token so callers detecting environment-prefix
/// wrappers work without special casing.
pub(in crate::permission_rules::bash_ast) fn standalone_variable_assignment_tokens(
    root: Node<'_>,
    src: &[u8],
) -> Option<Vec<String>> {
    if root.kind() != "program" {
        return None;
    }
    let mut cursor = root.walk();
    let mut tokens = Vec::new();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "variable_assignment" {
            return None;
        }
        tokens.push(variable_assignment_value(child, src)?);
    }
    if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

pub(super) fn extract_command_words(command: Node<'_>, src: &[u8]) -> Option<Vec<String>> {
    if command.kind() != "command" {
        return None;
    }
    let mut words = Vec::new();
    let mut cursor = command.walk();
    for child in command.named_children(&mut cursor) {
        match child.kind() {
            "command_name" => {
                let inner = child.named_child(0)?;
                let value = literal_word_value(inner, src)?;
                words.push(value);
            }
            "variable_assignment" => {
                let value = variable_assignment_value(child, src)?;
                words.push(value);
            }
            other => {
                let value = match other {
                    "word" | "number" => literal_word_value(child, src)?,
                    "string" => string_value(child, src)?,
                    "raw_string" => raw_string_value(child, src)?,
                    "ansi_c_string" => ansi_c_string_value(child, src)?,
                    "concatenation" => concatenation_value(child, src)?,
                    _ => return None,
                };
                words.push(value);
            }
        }
    }
    Some(words)
}

pub(super) fn literal_word_value(node: Node<'_>, src: &[u8]) -> Option<String> {
    match node.kind() {
        "word" | "number" => {
            if node.named_children(&mut node.walk()).next().is_some() {
                return None;
            }
            let raw = node.utf8_text(src).ok()?;
            Some(unescape_unquoted_word(raw))
        }
        "string" => string_value(node, src),
        "raw_string" => raw_string_value(node, src),
        "ansi_c_string" => ansi_c_string_value(node, src),
        "concatenation" => concatenation_value(node, src),
        _ => None,
    }
}

pub(in crate::permission_rules::bash_ast) fn first_word_node_value(
    node: Node<'_>,
    src: &[u8],
) -> Option<String> {
    match node.kind() {
        "word" | "number" | "string" | "raw_string" | "ansi_c_string" | "concatenation" => {
            literal_word_value(node, src)
        }
        "variable_assignment" => variable_assignment_value(node, src),
        _ => None,
    }
}

/// Unescape an unquoted bash word: every `\X` becomes `X`, keeping AST
/// tokens argv-equivalent to the bash shell's own word splitting.
fn unescape_unquoted_word(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(super) fn string_value(node: Node<'_>, src: &[u8]) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        if part.kind() != "string_content" {
            return None;
        }
    }
    let raw = node.utf8_text(src).ok()?;
    let stripped = raw.strip_prefix('"').and_then(|t| t.strip_suffix('"'))?;
    Some(unescape_double_quoted(stripped))
}

pub(super) fn raw_string_value(node: Node<'_>, src: &[u8]) -> Option<String> {
    if node.kind() != "raw_string" {
        return None;
    }
    let raw = node.utf8_text(src).ok()?;
    let stripped = raw.strip_prefix('\'').and_then(|t| t.strip_suffix('\''))?;
    Some(stripped.to_string())
}

pub(super) fn ansi_c_string_value(node: Node<'_>, src: &[u8]) -> Option<String> {
    if node.kind() != "ansi_c_string" {
        return None;
    }
    let raw = node.utf8_text(src).ok()?;
    let stripped = raw.strip_prefix("$'").and_then(|t| t.strip_suffix('\''))?;
    Some(decode_ansi_c_escapes(stripped))
}

pub(super) fn concatenation_value(node: Node<'_>, src: &[u8]) -> Option<String> {
    if node.kind() != "concatenation" {
        return None;
    }
    let mut value = String::new();
    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        let part_value = match part.kind() {
            "word" | "number" => literal_word_value(part, src)?,
            "string" => string_value(part, src)?,
            "raw_string" => raw_string_value(part, src)?,
            "ansi_c_string" => ansi_c_string_value(part, src)?,
            _ => return None,
        };
        value.push_str(&part_value);
    }
    if value.is_empty() { None } else { Some(value) }
}

pub(super) fn variable_assignment_value(node: Node<'_>, src: &[u8]) -> Option<String> {
    if node.kind() != "variable_assignment" {
        return None;
    }
    let mut cursor = node.walk();
    let mut children = node.named_children(&mut cursor);
    let name_node = children.next()?;
    if name_node.kind() != "variable_name" {
        return None;
    }
    let name = name_node.utf8_text(src).ok()?.to_string();
    let value = match children.next() {
        None => String::new(),
        Some(value_node) => match value_node.kind() {
            "word" | "number" => literal_word_value(value_node, src)?,
            "string" => string_value(value_node, src)?,
            "raw_string" => raw_string_value(value_node, src)?,
            "ansi_c_string" => ansi_c_string_value(value_node, src)?,
            "concatenation" => concatenation_value(value_node, src)?,
            _ => return None,
        },
    };
    Some(format!("{name}={value}"))
}

fn unescape_double_quoted(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn decode_ansi_c_escapes(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(next) = chars.next() else {
            out.push('\\');
            break;
        };
        out.push(match next {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '\\' => '\\',
            '\'' => '\'',
            other => other,
        });
    }
    out
}

/// Tokenize a parsed command tree into argv-style words.
///
/// Called from `bash_ast::tokenize_words` after tree-sitter parsing.
pub(in crate::permission_rules::bash_ast) fn tokenize_tree_words(
    root: Node<'_>,
    src: &[u8],
) -> Option<Vec<String>> {
    if root.has_error() {
        return error_node_partial_tokens(root, src);
    }
    if let Some(tokens) = standalone_variable_assignment_tokens(root, src) {
        return Some(tokens);
    }
    let command_node = find_single_simple_command(root)?;
    extract_command_words(command_node, src)
}

/// Extract command words from a `case` arm body, used by `case_arm_body_start`.
pub(in crate::permission_rules::bash_ast) fn case_arm_extract_command_words(
    node: Node<'_>,
    src: &[u8],
) -> Option<Vec<String>> {
    extract_command_words(node, src)
}
