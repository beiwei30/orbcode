//! Here-string (`<<<`) extraction from tree-sitter-bash parse trees.

use tree_sitter::Node;

use super::heredoc::recipient_command_tokens;
use super::{
    ansi_c_string_value, concatenation_value, literal_word_value, parse, raw_string_value,
    string_value, variable_assignment_value, walk_named_descendants,
};

/// Per-here-string data extracted from a tree-sitter parse. Mirrors the
/// `Heredoc` struct but for `<<<` (here-string) redirects, which tree-sitter
/// always parses as a `herestring_redirect` node inside the containing
/// `command` (regardless of operator position relative to the command name).
///
/// Tree-sitter-bash classifies `N<<<word` (fd-prefixed here-strings) as a
/// `file_redirect`, not a `herestring_redirect`, so AST consumers must keep a
/// tight text fallback for that shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HereString {
    /// Literal body text (unquoted single, double, ANSI-C, or word value).
    pub body: String,
    /// True when the body was a quoted/escaped form
    /// (`'...'`, `"..."`, or `$'...'`).
    pub quoted: bool,
    /// File descriptor this here-string redirects from. `0` for `<<<word`,
    /// other digits for `N<<<word`.
    pub fd: u32,
    /// Argv-style tokens of the recipient command (all words on the
    /// `command` except the `herestring_redirect` itself).
    pub recipient_tokens: Option<Vec<String>>,
}

/// Walk `command` and return every here-string the tree-sitter parser
/// recognised. Returns an empty vector if the parse failed entirely.
pub fn here_strings(command: &str) -> Vec<HereString> {
    let Some(tree) = parse(command) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let src = command.as_bytes();
    walk_named_descendants(tree.root_node(), |node| {
        if node.kind() != "herestring_redirect" {
            return;
        }
        if let Some(here_string) = extract_here_string(node, src) {
            out.push(here_string);
        }
    });
    out
}

fn extract_here_string(node: Node<'_>, src: &[u8]) -> Option<HereString> {
    let mut cursor = node.walk();
    let mut body_node = None;
    let mut fd: u32 = 0;
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "word" | "number" | "string" | "raw_string" | "ansi_c_string" | "concatenation" => {
                body_node = Some(child);
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
    let (body, quoted) = match body_node.kind() {
        "string" => (string_value(body_node, src)?, true),
        "raw_string" => (raw_string_value(body_node, src)?, true),
        "ansi_c_string" => (ansi_c_string_value(body_node, src)?, true),
        "word" | "number" => (literal_word_value(body_node, src)?, false),
        "concatenation" => (concatenation_value(body_node, src)?, true),
        _ => return None,
    };
    let recipient_tokens = here_string_recipient_tokens(node, src);
    Some(HereString {
        body,
        quoted,
        fd,
        recipient_tokens,
    })
}

fn here_string_recipient_tokens(node: Node<'_>, src: &[u8]) -> Option<Vec<String>> {
    let parent = node.parent()?;
    match parent.kind() {
        "command" => command_words_skipping_redirects(parent, src),
        "redirected_statement" => recipient_command_tokens(node, src),
        _ => None,
    }
}

fn command_words_skipping_redirects(command: Node<'_>, src: &[u8]) -> Option<Vec<String>> {
    if command.kind() != "command" {
        return None;
    }
    let mut words = Vec::new();
    let mut cursor = command.walk();
    for child in command.named_children(&mut cursor) {
        match child.kind() {
            "heredoc_redirect" | "herestring_redirect" | "file_redirect" => {}
            "command_name" => {
                let inner = child.named_child(0)?;
                words.push(literal_word_value(inner, src)?);
            }
            "variable_assignment" => words.push(variable_assignment_value(child, src)?),
            "word" | "number" => words.push(literal_word_value(child, src)?),
            "string" => words.push(string_value(child, src)?),
            "raw_string" => words.push(raw_string_value(child, src)?),
            "ansi_c_string" => words.push(ansi_c_string_value(child, src)?),
            "concatenation" => words.push(concatenation_value(child, src)?),
            _ => return None,
        }
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn here_strings_extracts_single_quoted_body() {
        let here_strings = here_strings("bash <<< 'rm -rf /'");
        assert_eq!(here_strings.len(), 1);
        let hs = &here_strings[0];
        assert_eq!(hs.body, "rm -rf /");
        assert!(hs.quoted);
        assert_eq!(hs.fd, 0);
        assert_eq!(
            hs.recipient_tokens.as_deref(),
            Some(&["bash".to_string()][..]),
        );
    }

    #[test]
    fn here_strings_extracts_ansi_c_body() {
        let here_strings = here_strings(r"bash <<< $'rm -rf /'");
        assert_eq!(here_strings.len(), 1);
        assert_eq!(here_strings[0].body, "rm -rf /");
    }

    #[test]
    fn here_strings_extracts_double_quoted_body() {
        let here_strings = here_strings(r#"bash <<< "rm -rf /""#);
        assert_eq!(here_strings.len(), 1);
        assert_eq!(here_strings[0].body, "rm -rf /");
    }

    #[test]
    fn here_strings_extracts_unquoted_word_body() {
        let here_strings = here_strings("echo <<< rmtext");
        assert_eq!(here_strings.len(), 1);
        let hs = &here_strings[0];
        assert_eq!(hs.body, "rmtext");
        assert!(!hs.quoted);
    }

    #[test]
    fn here_strings_handles_redirect_before_recipient() {
        let here_strings = here_strings("<<< 'rm' bash");
        assert_eq!(here_strings.len(), 1);
        assert_eq!(
            here_strings[0].recipient_tokens.as_deref(),
            Some(&["bash".to_string()][..]),
        );
    }

    #[test]
    fn here_strings_unrecognised_for_fd_prefixed_form() {
        let here_strings = here_strings("bash 3<<< 'rm -rf /'");
        assert!(here_strings.is_empty());
    }

    #[test]
    fn here_strings_handle_multiple_per_line_correctly() {
        let here_strings = here_strings("cat <<< 'safe'; bash <<< 'rm -rf /'");
        assert_eq!(here_strings.len(), 2);
        assert_eq!(here_strings[0].body, "safe");
        assert_eq!(
            here_strings[0].recipient_tokens.as_deref(),
            Some(&["cat".to_string()][..])
        );
        assert_eq!(here_strings[1].body, "rm -rf /");
        assert_eq!(
            here_strings[1].recipient_tokens.as_deref(),
            Some(&["bash".to_string()][..])
        );
    }
}
