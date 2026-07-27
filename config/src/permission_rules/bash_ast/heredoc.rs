//! Heredoc extraction from tree-sitter-bash parse trees.

use tree_sitter::Node;

use super::{extract_command_words, parse, walk_named_descendants};

/// Per-heredoc data extracted from a tree-sitter parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heredoc {
    /// Byte range of the body text (does not include the opening operator
    /// line or the closing delimiter line).
    pub body_start: usize,
    pub body_end: usize,
    /// Literal body text from `body_start..body_end`. Already trimmed of the
    /// trailing newline that tree-sitter includes inside `heredoc_body`.
    pub body: String,
    /// True when the heredoc delimiter is quoted (`<<'EOF'`, `<<"EOF"`, or
    /// `<<\EOF`). Quoted heredocs are opaque — substitutions in the body do
    /// not expand.
    pub quoted: bool,
    /// True when the heredoc strips leading tabs (`<<-EOF`).
    pub strip_tabs: bool,
    /// File descriptor this heredoc redirects from. `0` for `<<DELIM`,
    /// other digits for `N<<DELIM`.
    pub fd: u32,
    /// Byte position of the `<<` (or `<<-`) operator. Callers use this to
    /// associate the heredoc with the recipient command.
    pub operator_byte: usize,
    /// Byte range of the entire `heredoc_redirect` node (operator through
    /// closing delimiter line) so callers can replace it without disturbing
    /// neighboring text.
    pub redirect_start: usize,
    pub redirect_end: usize,
    /// Argv-style tokens of the command receiving this heredoc as stdin
    /// (e.g. `["bash", "-l"]` for `bash -l <<EOF`). `None` when the
    /// recipient cannot be tokenised cleanly (unsupported quoting,
    /// substitutions, etc.).
    pub recipient_tokens: Option<Vec<String>>,
}

/// Walk `command` and return every heredoc the tree-sitter parser
/// recognised. Returns an empty vector if the parse failed entirely.
pub fn heredocs(command: &str) -> Vec<Heredoc> {
    let Some(tree) = parse(command) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let src = command.as_bytes();
    walk_named_descendants(tree.root_node(), |node| {
        if node.kind() != "heredoc_redirect" {
            return;
        }
        if let Some(heredoc) = extract_heredoc(node, src) {
            out.push(heredoc);
        }
    });
    out
}

fn extract_heredoc(node: Node<'_>, src: &[u8]) -> Option<Heredoc> {
    let mut cursor = node.walk();
    let mut body_node = None;
    let mut start_text: Option<&str> = None;
    let mut fd: u32 = 0;
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "heredoc_body" | "simple_heredoc_body" => body_node = Some(child),
            "heredoc_start" => {
                start_text = Some(child.utf8_text(src).ok()?);
            }
            "file_descriptor" => {
                if let Ok(text) = child.utf8_text(src) {
                    fd = text.parse::<u32>().unwrap_or(0);
                }
            }
            _ => {}
        }
    }
    let body_node = body_node?;
    let body_text = body_node.utf8_text(src).ok()?;
    let body_trimmed = body_text.strip_suffix('\n').unwrap_or(body_text);
    let quoted = start_text.is_some_and(heredoc_delimiter_is_quoted);

    let mut cursor = node.walk();
    let mut operator_byte = node.start_byte();
    let mut strip_tabs = false;
    for child in node.children(&mut cursor) {
        if child.is_named() {
            continue;
        }
        match child.kind() {
            "<<" => {
                operator_byte = child.start_byte();
            }
            "<<-" => {
                operator_byte = child.start_byte();
                strip_tabs = true;
            }
            _ => {}
        }
    }

    let recipient_tokens = recipient_command_tokens(node, src);

    Some(Heredoc {
        body_start: body_node.start_byte(),
        body_end: body_node.end_byte(),
        body: body_trimmed.to_string(),
        quoted,
        strip_tabs,
        fd,
        operator_byte,
        redirect_start: node.start_byte(),
        redirect_end: node.end_byte(),
        recipient_tokens,
    })
}

/// Return the argv tokens of the command that receives `heredoc_redirect`
/// as stdin. The recipient is the sibling `command` node inside the
/// containing `redirected_statement`. Falls back to `None` when the parse
/// shape does not expose a single recipient command (e.g. heredocs
/// attached to lists or pipelines, malformed input).
pub(super) fn recipient_command_tokens(heredoc_node: Node<'_>, src: &[u8]) -> Option<Vec<String>> {
    let parent = heredoc_node.parent()?;
    if parent.kind() != "redirected_statement" {
        return None;
    }
    let mut cursor = parent.walk();
    for child in parent.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "heredoc_redirect" | "file_redirect" | "herestring_redirect"
        ) {
            continue;
        }
        if child.kind() == "command" {
            return extract_command_words(child, src);
        }
        // Lists/pipelines attached to a heredoc -> first inner command.
        let mut inner = child.walk();
        for grandchild in child.named_children(&mut inner) {
            if grandchild.kind() == "command" {
                return extract_command_words(grandchild, src);
            }
        }
        return None;
    }
    None
}

fn heredoc_delimiter_is_quoted(raw: &str) -> bool {
    raw.contains('\'') || raw.contains('"') || raw.contains('\\')
}

/// Build a copy of `command` with each heredoc body and closing delimiter
/// line replaced by blanks. Preserves byte offsets by substituting blanks
/// for the original text (line-breaks kept so line counts match).
///
/// Used by deny-rule scanners that walk the surrounding command structure
/// and must not see separators or wrapper-like text contained in a
/// heredoc's body.
pub fn command_without_heredoc_bodies(command: &str) -> String {
    let hdocs = heredocs(command);
    if hdocs.is_empty() {
        return command.to_string();
    }
    let mut output = String::with_capacity(command.len());
    let mut cursor = 0usize;
    // Sort by body_start to walk in order.
    let mut ranges: Vec<_> = hdocs
        .iter()
        .map(|h| (h.body_start, h.redirect_end))
        .collect();
    ranges.sort_by_key(|&(start, _)| start);
    for (body_start, redirect_end) in ranges {
        if body_start < cursor {
            continue;
        }
        if let Some(prefix) = command.get(cursor..body_start) {
            output.push_str(prefix);
        }
        let body_end = redirect_end.min(command.len());
        if let Some(blanked) = command.get(body_start..body_end) {
            for c in blanked.chars() {
                if c == '\n' {
                    output.push('\n');
                } else {
                    // Replace non-newline characters with spaces so byte
                    // offsets stay aligned for downstream scanners.
                    output.push(' ');
                }
            }
        }
        cursor = body_end;
    }
    if let Some(rest) = command.get(cursor..) {
        output.push_str(rest);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heredocs_extracts_unquoted_body() {
        let heredocs = heredocs("cat <<EOF\nhello world\nEOF\n");
        assert_eq!(heredocs.len(), 1);
        let heredoc = &heredocs[0];
        assert_eq!(heredoc.body, "hello world");
        assert!(!heredoc.quoted);
        assert!(!heredoc.strip_tabs);
    }

    #[test]
    fn heredocs_recognises_quoted_delimiter() {
        let heredocs = heredocs("cat <<'EOF'\nhello\nEOF\n");
        assert_eq!(heredocs.len(), 1);
        assert!(heredocs[0].quoted);
    }

    #[test]
    fn heredocs_recognises_tab_stripping_operator() {
        let heredocs = heredocs("cat <<-EOF\n\thello\nEOF\n");
        assert_eq!(heredocs.len(), 1);
        assert!(heredocs[0].strip_tabs);
    }

    #[test]
    fn heredocs_records_operator_position() {
        let src = "cat <<EOF\nhello\nEOF\n";
        let hdocs = heredocs(src);
        assert_eq!(hdocs.len(), 1);
        let op = hdocs[0].operator_byte;
        assert_eq!(&src[op..op + 2], "<<");
    }

    #[test]
    fn command_without_heredoc_bodies_blanks_body_lines() {
        let blanked = command_without_heredoc_bodies("cat <<EOF\nhello\nEOF\necho done\n");
        assert!(blanked.contains("cat "));
        assert!(blanked.contains("echo done"));
        assert!(!blanked.contains("hello"));
    }

    #[test]
    fn command_without_heredoc_bodies_passthrough_when_no_heredoc() {
        let original = "echo hello && echo world";
        assert_eq!(command_without_heredoc_bodies(original), original);
    }

    #[test]
    fn heredocs_captures_recipient_tokens_for_shell_stdin() {
        let heredocs = heredocs("bash -l <<EOF\nrm -rf /\nEOF\n");
        assert_eq!(heredocs.len(), 1);
        assert_eq!(
            heredocs[0].recipient_tokens.as_deref(),
            Some(&["bash".to_string(), "-l".to_string()][..]),
        );
        assert_eq!(heredocs[0].fd, 0);
    }

    #[test]
    fn heredocs_captures_explicit_fd_redirect() {
        let heredocs = heredocs("bash 3<<EOF\nrm -rf /\nEOF\n");
        assert_eq!(heredocs.len(), 1);
        assert_eq!(heredocs[0].fd, 3);
    }

    #[test]
    fn heredocs_unrecognised_when_operator_precedes_recipient() {
        let heredocs = heredocs("<<EOF bash\nrm -rf /\nEOF\n");
        assert!(heredocs.is_empty());
    }

    #[test]
    fn heredocs_confused_on_multiple_per_line() {
        let heredocs = heredocs("cat <<A; bash <<B\nsafe\nA\nrm -rf /\nB\n");
        assert_eq!(heredocs.len(), 1);
        assert_eq!(
            heredocs[0].recipient_tokens.as_deref(),
            Some(&["cat".to_string()][..])
        );
    }

    #[test]
    fn heredocs_returns_wrong_body_when_multi_per_line_with_shell_first() {
        let heredocs = heredocs("bash <<A; cat <<B\nsafe\nA\nrm -rf /\nB\n");
        assert_eq!(heredocs.len(), 1);
        assert_eq!(
            heredocs[0].recipient_tokens.as_deref(),
            Some(&["bash".to_string()][..]),
        );
        assert_eq!(heredocs[0].body, "safe\nA\nrm -rf /");
    }

    #[test]
    fn heredocs_recipient_tokens_include_env_prefix_and_assignments() {
        let result = heredocs("env FOO=bar bash <<EOF\nrm\nEOF\n");
        assert_eq!(
            result[0].recipient_tokens.as_deref(),
            Some(&["env".to_string(), "FOO=bar".to_string(), "bash".to_string()][..]),
        );
        let result2 = heredocs("FOO=bar bash <<EOF\nrm\nEOF\n");
        assert_eq!(
            result2[0].recipient_tokens.as_deref(),
            Some(&["FOO=bar".to_string(), "bash".to_string()][..]),
        );
    }
}
