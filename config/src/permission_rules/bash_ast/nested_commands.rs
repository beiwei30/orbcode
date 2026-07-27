//! Nested command candidate extraction from tree-sitter-bash parse trees.

use tree_sitter::Node;

use super::{extract_command_words, parse, walk_named_descendants};

/// Return the body text of every nested command-bearing construct in
/// `command`. Each candidate is the literal source slice between the
/// construct's opening and closing brackets, trimmed of surrounding
/// whitespace. Duplicates are suppressed in document order.
///
/// Containers walked:
/// - `command_substitution` (`$(...)` and `` `...` ``)
/// - `subshell` (`(...)`)
/// - `process_substitution` (`<(...)` and `>(...)`)
/// - `compound_statement` (`{...}` standalone groups, function bodies, etc.)
///
/// Replaces the legacy `collect_nested_bash_command_candidates` quote-state
/// walker. Tree-sitter recognises nested containers inside
/// arithmetic expansion, parameter expansion, and quoted strings as
/// distinct AST nodes, so a single descendant walk catches every form the
/// bespoke scanner handled across `$((...))`, `${...}`, and double-quoted
/// substrings.
pub fn nested_command_candidates(command: &str) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    collect_nested_command_candidates(command, &mut candidates);
    for body in time_coproc_brace_bodies(command) {
        push_unique_candidate(&body, &mut candidates);
        let mut nested_candidates = Vec::new();
        collect_nested_command_candidates(&body, &mut nested_candidates);
        for candidate in nested_candidates {
            push_unique_candidate(&candidate, &mut candidates);
        }
    }
    candidates
}

/// Extract command bodies from `time { ... }`, `time -p { ... }`,
/// `coproc { ... }`, and `coproc NAME { ... }` brace forms.
///
/// Tree-sitter-bash does not recognise the `{` after `time` or
/// `coproc NAME` as a `compound_statement` opener — it tokenises the
/// `{` as a plain `word` and the matching `}` becomes a separate
/// `command` whose only word is `}`. Walk every `command` node, detect
/// the `time`/`coproc` prefix, then text-scan forward from the open
/// brace to find the matching close brace while respecting quoting and
/// nested `(){}`. Returns the inner body text trimmed of surrounding
/// whitespace and a trailing semicolon.
pub fn time_coproc_brace_bodies(command: &str) -> Vec<String> {
    let Some(tree) = parse(command) else {
        return Vec::new();
    };
    let src = command.as_bytes();
    let mut bodies = Vec::new();
    walk_named_descendants(tree.root_node(), |node| {
        if node.kind() != "command" {
            return;
        }
        if let Some(body) = extract_time_coproc_brace_body(node, src, command)
            && !bodies.iter().any(|existing: &String| existing == &body)
        {
            bodies.push(body);
        }
    });
    bodies
}

fn extract_time_coproc_brace_body(
    command_node: Node<'_>,
    src: &[u8],
    full_source: &str,
) -> Option<String> {
    let mut words: Vec<(&str, usize)> = Vec::new();
    let mut cursor = command_node.walk();
    for child in command_node.named_children(&mut cursor) {
        match child.kind() {
            "command_name" => {
                let inner = child.named_child(0)?;
                let text = inner.utf8_text(src).ok()?;
                words.push((text, inner.start_byte()));
            }
            "word" => {
                let text = child.utf8_text(src).ok()?;
                words.push((text, child.start_byte()));
            }
            _ => return None,
        }
    }
    let first = words.first()?.0;
    let brace_word_index = match first {
        "time" => time_brace_word_index(&words)?,
        "coproc" => coproc_brace_word_index(&words)?,
        _ => return None,
    };
    let (_, brace_byte) = words.get(brace_word_index)?;
    let body = scan_braced_body(full_source, *brace_byte)?;
    Some(body)
}

fn time_brace_word_index(words: &[(&str, usize)]) -> Option<usize> {
    let candidate = words.get(1)?.0;
    if candidate == "{" {
        return Some(1);
    }
    if candidate == "-p" && words.get(2).map(|w| w.0) == Some("{") {
        return Some(2);
    }
    None
}

fn coproc_brace_word_index(words: &[(&str, usize)]) -> Option<usize> {
    let candidate = words.get(1)?.0;
    if candidate == "{" {
        return Some(1);
    }
    if is_bash_simple_name(candidate) && words.get(2).map(|w| w.0) == Some("{") {
        return Some(2);
    }
    None
}

fn is_bash_simple_name(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(ch) if ch.is_ascii_alphabetic() || ch == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// Starting at `open_brace_byte` (where `full_source[open_brace_byte] ==
/// b'{'`), scan forward to the matching `}` while honouring single/
/// double quotes, ANSI-C `$'...'` quotes, escapes, and nested `()`/`{}`.
/// Returns the inner text trimmed of surrounding whitespace and a single
/// trailing semicolon.
fn scan_braced_body(source: &str, open_brace_byte: usize) -> Option<String> {
    let bytes = source.as_bytes();
    if bytes.get(open_brace_byte).copied() != Some(b'{') {
        return None;
    }
    let mut depth = 1usize;
    let mut paren_depth = 0usize;
    let mut i = open_brace_byte + 1;
    let body_start = i;
    while i < bytes.len() {
        let ch = bytes[i];
        match ch {
            b'\\' => {
                i = i.saturating_add(2);
            }
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i = i.saturating_add(2),
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'$' if bytes.get(i + 1).copied() == Some(b'\'') => {
                i += 2;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i = i.saturating_add(2),
                        b'\'' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'(' => {
                paren_depth = paren_depth.saturating_add(1);
                i += 1;
            }
            b')' => {
                paren_depth = paren_depth.saturating_sub(1);
                i += 1;
            }
            b'{' if paren_depth == 0 => {
                depth = depth.saturating_add(1);
                i += 1;
            }
            b'}' if paren_depth == 0 => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let body = source.get(body_start..i)?;
                    let trimmed = body.trim().trim_end_matches(';').trim().to_string();
                    if trimmed.is_empty() {
                        return None;
                    }
                    return Some(trimmed);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn collect_nested_command_candidates(command: &str, candidates: &mut Vec<String>) {
    let Some(tree) = parse(command) else {
        return;
    };
    let src = command.as_bytes();
    let mut expansion_inner_texts: Vec<String> = Vec::new();
    walk_named_descendants(tree.root_node(), |node| {
        if is_nested_container_kind(node.kind()) {
            if let Some(inner) = nested_container_inner_text(node, src) {
                push_unique_candidate(inner.trim(), candidates);
            }
            return;
        }
        if matches!(node.kind(), "expansion" | "arithmetic_expansion")
            && let Some(inner) = nested_container_inner_text(node, src)
        {
            let trimmed = inner.trim().to_string();
            if !trimmed.is_empty()
                && trimmed != command.trim()
                && !expansion_inner_texts
                    .iter()
                    .any(|existing| existing == &trimmed)
            {
                expansion_inner_texts.push(trimmed);
            }
        }
    });
    for inner in expansion_inner_texts {
        let mut nested_candidates = Vec::new();
        collect_nested_command_candidates(&inner, &mut nested_candidates);
        for candidate in nested_candidates {
            push_unique_candidate(&candidate, candidates);
        }
        if let Some(literal_command) = scan_literal_substitution_in_expansion(&inner) {
            push_unique_candidate(&literal_command, candidates);
        }
    }
}

fn push_unique_candidate(candidate: &str, candidates: &mut Vec<String>) {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return;
    }
    if !candidates.iter().any(|existing| existing == trimmed) {
        candidates.push(trimmed.to_string());
    }
}

/// Recover a substitution body that tree-sitter parsed as a literal word
/// inside an `expansion`/`arithmetic_expansion`. Handles the
/// `${VAR:-`cmd`}` and `${VAR:-$(cmd)}` shapes where tree-sitter does not
/// expose a nested `command_substitution`.
fn scan_literal_substitution_in_expansion(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut start = 0usize;
    while start < bytes.len() {
        match bytes[start] {
            b'`' => {
                let after = start + 1;
                let close = bytes[after..]
                    .iter()
                    .position(|&b| b == b'`')
                    .map(|idx| after + idx)?;
                let body = text.get(after..close)?.trim();
                if !body.is_empty() {
                    return Some(body.to_string());
                }
                start = close + 1;
            }
            b'$' if bytes.get(start + 1) == Some(&b'(') && bytes.get(start + 2) != Some(&b'(') => {
                let inner_start = start + 2;
                let close = matching_paren_close(bytes, inner_start)?;
                let body = text.get(inner_start..close)?.trim();
                if !body.is_empty() {
                    return Some(body.to_string());
                }
                start = close + 1;
            }
            _ => start += 1,
        }
    }
    None
}

fn matching_paren_close(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth: usize = 1;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn is_nested_container_kind(kind: &str) -> bool {
    matches!(
        kind,
        "command_substitution" | "subshell" | "process_substitution" | "compound_statement"
    )
}

/// Extract the literal source slice between the first and last children of
/// `node`. For containers this strips the surrounding operator tokens
/// (`$(`, `(`, `` ` ``, `{`, `<(`, etc.) without re-parsing the contents.
fn nested_container_inner_text(node: Node<'_>, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    let first = children.first()?;
    let last = children.last()?;
    let inner_start = if first.is_named() {
        node.start_byte()
    } else {
        first.end_byte()
    };
    let inner_end = if last.is_named() {
        node.end_byte()
    } else {
        last.start_byte()
    };
    if inner_end < inner_start {
        return None;
    }
    let text = std::str::from_utf8(src.get(inner_start..inner_end)?).ok()?;
    Some(text.to_string())
}

// `extract_command_words` is used only for recipient detection; suppress
// dead-code warning since it is re-exported at the parent level.
#[allow(dead_code)]
fn _use_extract_command_words() {
    let _ = extract_command_words;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_command_candidates_extracts_dollar_paren_body() {
        let candidates = nested_command_candidates("echo $(rm -rf /)");
        assert_eq!(candidates, vec!["rm -rf /".to_string()]);
    }

    #[test]
    fn nested_command_candidates_extracts_backtick_body() {
        let candidates = nested_command_candidates("echo `rm -rf /`");
        assert_eq!(candidates, vec!["rm -rf /".to_string()]);
    }

    #[test]
    fn nested_command_candidates_extracts_subshell_body() {
        let candidates = nested_command_candidates("(rm -rf /); ls");
        assert_eq!(candidates, vec!["rm -rf /".to_string()]);
    }

    #[test]
    fn nested_command_candidates_extracts_brace_group_body() {
        let candidates = nested_command_candidates("{ rm -rf /; }");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].trim(), "rm -rf /;");
    }

    #[test]
    fn nested_command_candidates_extracts_function_body() {
        let candidates = nested_command_candidates("cleanup() { rm -rf /; }");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].trim(), "rm -rf /;");
    }

    #[test]
    fn nested_command_candidates_extracts_process_substitution() {
        let candidates = nested_command_candidates("diff <(rm -rf /) /etc/passwd");
        assert_eq!(candidates, vec!["rm -rf /".to_string()]);
    }

    #[test]
    fn nested_command_candidates_extracts_substitution_inside_parameter_expansion() {
        let candidates = nested_command_candidates(r#"echo "${TARGET:-$(rm -rf /)}""#);
        assert_eq!(candidates, vec!["rm -rf /".to_string()]);
    }

    #[test]
    fn nested_command_candidates_extracts_substitution_inside_arithmetic_expansion() {
        let candidates = nested_command_candidates("echo $((1 + $(rm -rf /)))");
        assert_eq!(candidates, vec!["rm -rf /".to_string()]);
    }

    #[test]
    fn nested_command_candidates_extracts_nested_substitutions() {
        let candidates = nested_command_candidates("echo $(foo $(rm -rf /))");
        assert_eq!(
            candidates,
            vec!["foo $(rm -rf /)".to_string(), "rm -rf /".to_string()],
        );
    }

    #[test]
    fn nested_command_candidates_returns_empty_for_simple_command() {
        assert!(nested_command_candidates("echo hello").is_empty());
    }

    #[test]
    fn nested_command_candidates_deduplicates() {
        let candidates = nested_command_candidates("echo $(rm) && cat $(rm)");
        assert_eq!(candidates, vec!["rm".to_string()]);
    }

    #[test]
    fn nested_command_candidates_extracts_time_brace_body() {
        let candidates = nested_command_candidates("time { rm -rf /; }");
        assert!(
            candidates.iter().any(|candidate| candidate == "rm -rf /"),
            "candidates = {candidates:?}"
        );
    }

    #[test]
    fn nested_command_candidates_extracts_time_p_brace_body() {
        let candidates = nested_command_candidates("time -p { rm -rf /; }");
        assert!(
            candidates.iter().any(|candidate| candidate == "rm -rf /"),
            "candidates = {candidates:?}"
        );
    }

    #[test]
    fn nested_command_candidates_extracts_coproc_anonymous_brace_body() {
        let candidates = nested_command_candidates("coproc { rm -rf /; }");
        assert!(
            candidates.iter().any(|candidate| candidate == "rm -rf /"),
            "candidates = {candidates:?}"
        );
    }

    #[test]
    fn nested_command_candidates_extracts_coproc_named_brace_body() {
        let candidates = nested_command_candidates("coproc NAME { rm -rf /; }");
        assert!(
            candidates.iter().any(|candidate| candidate == "rm -rf /"),
            "candidates = {candidates:?}"
        );
    }

    #[test]
    fn nested_command_candidates_does_not_misclassify_plain_brace_group() {
        let candidates = nested_command_candidates("{ rm -rf /; }");
        assert_eq!(candidates, vec!["rm -rf /;".to_string()]);
    }

    #[test]
    fn time_coproc_brace_bodies_recurses_into_nested_substitutions() {
        let candidates = nested_command_candidates("time { echo $(rm -rf /); }");
        assert!(
            candidates.iter().any(|candidate| candidate == "rm -rf /"),
            "candidates = {candidates:?}"
        );
    }

    #[test]
    fn time_coproc_brace_bodies_skips_pipeline_form() {
        assert!(time_coproc_brace_bodies("time rm -rf /").is_empty());
    }

    #[test]
    fn nested_command_candidates_extracts_time_subshell_body() {
        let candidates = nested_command_candidates("time (rm -rf /)");
        assert_eq!(candidates, vec!["rm -rf /".to_string()]);
    }
}
