//! Tree-sitter-bash parsing primitives for permission-rule analysis.
//!
//! Mirrors the shape of codex's `codex-rs/shell-command/src/bash.rs` so future
//! work can lift larger helpers (word extraction, heredoc body scanning,
//! command-bearing option detection) without re-deriving the substrate.
//!
//! This module intentionally exposes a very small API:
//! - `parse`: build a `Tree` for a bash script.
//! - `walk_named_descendants`: invoke a callback for every named node in
//!   document order. Used by higher layers to find specific node kinds.
//!
//! Higher-level traversal helpers (word-only sequences, heredoc bodies,
//! here-string bodies, nested command bodies) land in subsequent steps of the
//! tree-sitter migration plan.

mod analysis;
mod here_string;
pub(crate) mod heredoc;
mod nested_commands;
mod words;

pub use analysis::analyze;
pub use here_string::{HereString, here_strings};
pub use heredoc::{Heredoc, command_without_heredoc_bodies, heredocs};
pub use nested_commands::nested_command_candidates;

use words::{
    ansi_c_string_value, concatenation_value, extract_command_words, literal_word_value,
    raw_string_value, string_value, variable_assignment_value,
};

use tree_sitter::Node;
use tree_sitter::Parser;
use tree_sitter::Tree;
use tree_sitter_bash::LANGUAGE as BASH;

use words::{case_arm_extract_command_words, first_word_node_value, tokenize_tree_words};

/// Parse `src` as bash and return the resulting tree. Returns `None` only when
/// the tree-sitter parser itself refuses input (e.g. invalid UTF-8 byte
/// boundaries from an upstream caller).
///
/// The returned tree may have errors — callers that need a clean parse should
/// inspect `tree.root_node().has_error()`. Returning a tree even on partial
/// parse failures matches codex's `try_parse_shell` so deny-rule scanners can
/// still inspect successfully parsed subtrees.
pub fn parse(src: &str) -> Option<Tree> {
    let lang = BASH.into();
    let mut parser = Parser::new();
    parser.set_language(&lang).ok()?;
    parser.parse(src, None)
}

/// Invoke `visit` for every named descendant of `root` in document order
/// (start-byte ascending). The root itself is included.
///
/// Used by AST-based scanners that need to find every node of a particular
/// kind (e.g. every `heredoc_redirect`, every `command_substitution`) without
/// each one re-walking the tree.
pub fn walk_named_descendants<'tree, F>(root: Node<'tree>, mut visit: F)
where
    F: FnMut(Node<'tree>),
{
    let mut stack = vec![root];
    let mut ordered = Vec::new();
    while let Some(node) = stack.pop() {
        if node.is_named() {
            ordered.push(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    ordered.sort_by_key(Node::start_byte);
    for node in ordered {
        visit(node);
    }
}

/// Tokenize `command` into argv-style words using tree-sitter.
///
/// Returns `Some(words)` when `command` is a single safe command — that is,
/// a command line with no command/process substitutions, no parameter
/// expansions, no redirections, no subshells/groups, and no compound
/// separators (`;`, `|`, `&&`, `||`, `&`). Returns `None` otherwise.
///
/// Variable assignments that prefix the command (`FOO=bar baz`) emit as a
/// single token (`"FOO=bar"`) so callers can detect environment-prefix
/// wrappers. Double-quoted strings are accepted only when they contain no
/// expansions; single-quoted, double-quoted (literal), and `$'...'` ANSI-C
/// strings are unquoted to their literal value. Whitespace-only or empty
/// input returns `Some(vec![])`.
pub fn tokenize_words(command: &str) -> Option<Vec<String>> {
    if command.chars().all(char::is_whitespace) {
        return Some(Vec::new());
    }
    let tree = parse(command)?;
    let root = tree.root_node();
    let src = command.as_bytes();
    tokenize_tree_words(root, src)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstWord {
    pub value: String,
    pub raw: String,
    pub end_byte: usize,
}

pub fn first_word(command: &str) -> Option<FirstWord> {
    let first_byte = command
        .char_indices()
        .find_map(|(index, character)| (!character.is_whitespace()).then_some(index))?;
    let tree = parse(command)?;
    let src = command.as_bytes();
    let mut candidates = Vec::new();
    walk_named_descendants(tree.root_node(), |node| {
        if node.start_byte() == first_byte && first_word_node_value(node, src).is_some() {
            candidates.push(node);
        }
    });
    candidates.sort_by_key(tree_sitter::Node::end_byte);
    let node = candidates.into_iter().next()?;
    Some(FirstWord {
        value: first_word_node_value(node, src)?,
        raw: node.utf8_text(src).ok()?.to_string(),
        end_byte: node.end_byte(),
    })
}

pub fn case_arm_body_start(command: &str) -> Option<usize> {
    let prefix = "case __orbcode_probe in\n";
    let wrapped = format!("{prefix}{command}\n;;\nesac\n");
    let tree = parse(&wrapped)?;
    let src = wrapped.as_bytes();
    let command_start = prefix.len();
    let command_end = command_start + command.len();
    let mut starts = Vec::new();
    walk_named_descendants(tree.root_node(), |node| {
        if node.kind() == "command"
            && node.start_byte() >= command_start
            && node.start_byte() < command_end
            && case_arm_extract_command_words(node, src).is_some()
        {
            starts.push(node.start_byte() - command_start);
        }
    });
    starts.into_iter().min()
}

// ─── Internal helpers ───────────────────────────────────────────────────────

fn find_single_simple_command(root: Node<'_>) -> Option<Node<'_>> {
    let mut current = root;
    loop {
        match current.kind() {
            "program" => {
                let mut cursor = current.walk();
                let children: Vec<_> = current.named_children(&mut cursor).collect();
                if children.len() != 1 {
                    return None;
                }
                current = children[0];
            }
            "command" => return Some(current),
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    fn kinds_of(src: &str) -> Vec<&'static str> {
        let tree = parse(src).expect("parser construction");
        let mut kinds = Vec::new();
        walk_named_descendants(tree.root_node(), |node| {
            kinds.push(node.kind());
        });
        kinds
    }

    #[test]
    fn parses_simple_command() {
        let kinds = kinds_of("ls -1");
        assert!(kinds.contains(&"program"));
        assert!(kinds.contains(&"command"));
        assert!(kinds.contains(&"command_name"));
    }

    #[test]
    fn parses_pipeline_and_list() {
        let kinds = kinds_of("ls && pwd | wc -l");
        assert!(kinds.contains(&"list"));
        assert!(kinds.contains(&"pipeline"));
    }

    #[test]
    fn parses_heredoc_redirect() {
        let kinds = kinds_of("cat <<'EOF'\nhello\nEOF\n");
        assert!(kinds.contains(&"heredoc_redirect"));
    }

    #[test]
    fn parses_herestring_redirect() {
        let kinds = kinds_of("cat <<< 'hello'");
        assert!(kinds.contains(&"herestring_redirect"));
    }

    #[test]
    fn parses_command_substitution() {
        let kinds = kinds_of("echo $(pwd)");
        assert!(kinds.contains(&"command_substitution"));
    }

    #[test]
    fn parses_ansi_c_quoted_string() {
        let kinds = kinds_of("echo $'hi'");
        assert!(
            kinds.contains(&"ansi_c_string"),
            "expected ansi_c_string node, got {kinds:?}"
        );
    }

    #[test]
    fn walk_visits_descendants_in_document_order() {
        let src = "echo a && echo b";
        let tree = parse(src).unwrap();
        let mut commands_text = Vec::new();
        walk_named_descendants(tree.root_node(), |node| {
            if node.kind() == "command" {
                let text = node.utf8_text(src.as_bytes()).unwrap().to_string();
                commands_text.push(text);
            }
        });
        assert_eq!(commands_text, vec!["echo a", "echo b"]);
    }

    #[test]
    fn returns_tree_even_when_parse_has_errors() {
        let tree = parse("ls &&").expect("tree-sitter returns a tree for partial parses");
        assert!(tree.root_node().has_error());
    }

    #[test]
    fn tokenize_words_handles_simple_command() {
        assert_eq!(
            tokenize_words("ls -1"),
            Some(vec!["ls".to_string(), "-1".to_string()])
        );
    }

    #[test]
    fn tokenize_words_handles_quoted_strings() {
        assert_eq!(
            tokenize_words(r#"echo "hello world""#),
            Some(vec!["echo".to_string(), "hello world".to_string()])
        );
        assert_eq!(
            tokenize_words("echo 'hello world'"),
            Some(vec!["echo".to_string(), "hello world".to_string()])
        );
    }

    #[test]
    fn tokenize_words_handles_ansi_c_quoted_strings() {
        assert_eq!(
            tokenize_words("echo $'hi\\nthere'"),
            Some(vec!["echo".to_string(), "hi\nthere".to_string()])
        );
    }

    #[test]
    fn tokenize_words_handles_variable_assignment_prefix() {
        assert_eq!(
            tokenize_words("FOO=bar baz qux"),
            Some(vec![
                "FOO=bar".to_string(),
                "baz".to_string(),
                "qux".to_string(),
            ])
        );
    }

    #[test]
    fn tokenize_words_handles_concatenation() {
        assert_eq!(
            tokenize_words(r#"echo "/usr"'/'"local"/bin"#),
            Some(vec!["echo".to_string(), "/usr/local/bin".to_string()])
        );
    }

    #[test]
    fn tokenize_words_handles_empty_and_whitespace_input() {
        assert_eq!(tokenize_words(""), Some(Vec::new()));
        assert_eq!(tokenize_words("   "), Some(Vec::new()));
    }

    #[test]
    fn tokenize_words_rejects_command_substitution() {
        assert_eq!(tokenize_words("echo $(pwd)"), None);
        assert_eq!(tokenize_words("echo `pwd`"), None);
    }

    #[test]
    fn tokenize_words_rejects_parameter_expansion() {
        assert_eq!(tokenize_words("echo $HOME"), None);
        assert_eq!(tokenize_words("echo ${HOME}"), None);
        assert_eq!(tokenize_words(r#"echo "hi $USER""#), None);
    }

    #[test]
    fn tokenize_words_rejects_redirections() {
        assert_eq!(tokenize_words("ls > out.txt"), None);
        assert_eq!(tokenize_words(">/tmp/out rm foo"), None);
    }

    #[test]
    fn tokenize_words_rejects_compound_separators() {
        assert_eq!(tokenize_words("foo; bar"), None);
        assert_eq!(tokenize_words("foo | bar"), None);
        assert_eq!(tokenize_words("foo && bar"), None);
        assert_eq!(tokenize_words("foo & bar"), None);
    }

    #[test]
    fn tokenize_words_rejects_subshells_and_groups() {
        assert_eq!(tokenize_words("(ls)"), None);
        assert_eq!(tokenize_words("{ ls; }"), None);
    }

    #[allow(dead_code)]
    fn dump_tree(src: &str) -> String {
        let tree = parse(src).expect("parse");
        let mut buf = String::new();
        let bytes = src.as_bytes();
        walk_named_descendants(tree.root_node(), |node| {
            let text =
                std::str::from_utf8(&bytes[node.start_byte()..node.end_byte()]).unwrap_or("<utf8>");
            let snippet = text
                .chars()
                .take(40)
                .collect::<String>()
                .replace('\n', "\\n");
            writeln!(
                buf,
                "{:30} bytes={}..{} text={:?}",
                node.kind(),
                node.start_byte(),
                node.end_byte(),
                snippet,
            )
            .expect("writing to String cannot fail");
        });
        buf
    }

    #[test]
    fn probe_coproc_named_block_shape() {
        let _dump = dump_tree("coproc NAME { rm -rf /; }\n");
    }

    #[test]
    fn probe_coproc_anonymous_compound_shape() {
        let _dump = dump_tree("coproc { rm -rf /; }\n");
    }

    #[test]
    fn probe_coproc_simple_command_shape() {
        let _dump = dump_tree("coproc rm -rf /\n");
    }

    #[test]
    fn probe_time_block_shape() {
        let _dump = dump_tree("time { rm -rf /; }\n");
    }

    #[test]
    fn probe_time_subshell_shape() {
        let _dump = dump_tree("time (rm -rf /)\n");
    }

    #[test]
    fn probe_time_p_block_shape() {
        let _dump = dump_tree("time -p { rm -rf /; }\n");
    }

    #[test]
    fn probe_time_pipeline_shape() {
        let _dump = dump_tree("time rm -rf / | wc -l\n");
    }
}
