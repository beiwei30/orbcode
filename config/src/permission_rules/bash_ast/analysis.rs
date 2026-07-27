//! AST-based command analysis: subcommand splitting and complexity detection.

use tree_sitter::Node;

use super::{parse, walk_named_descendants};

/// Result of splitting a bash command line into its top-level subcommands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    /// Source text of each top-level statement between separators
    /// (`;`, `\n`, `&`, `&&`, `||`, `|`). Compound statements (`if`, `case`,
    /// loops, function definitions) and complex constructs (subshells,
    /// brace groups, redirected statements) appear as a single subcommand
    /// holding their full source text.
    pub subcommands: Vec<String>,
    /// True when the command contains shell features that prevent
    /// confident allow-rule matching: substitutions, process substitution,
    /// subshells/brace groups, redirections, control flow, background `&`,
    /// or unbalanced quoting. Mirrors the legacy `BashCommandAnalysis`
    /// `too_complex` flag.
    pub too_complex: bool,
}

/// Decompose `command` into its subcommands and detect complexity that
/// should force exact (rather than grouped) permission-rule suggestions.
///
/// Walks the tree-sitter-bash AST to locate every separator
/// (`;`, `\n`, `|`, `&&`, `||`, `&`) that the parser places in a `program`,
/// `list`, or `pipeline` node, then text-splits the original source at those
/// byte ranges. This keeps subcommand text identical to the legacy bespoke
/// splitter (including any redirection suffix such as `2>/dev/null` that the
/// downstream suggestion code uses to decide between grouped and exact
/// rules) while leaning on the parser to recognize quoted, escaped, and
/// substituted regions instead of re-implementing them.
pub fn analyze(command: &str) -> Analysis {
    let mut analysis = Analysis::default();
    let trimmed_input = command.trim();

    let Some(tree) = parse(command) else {
        if !trimmed_input.is_empty() {
            analysis.subcommands.push(trimmed_input.to_string());
        }
        analysis.too_complex = true;
        return analysis;
    };
    let root = tree.root_node();
    if root.has_error() {
        analysis.too_complex = true;
    }

    let mut separators: Vec<(usize, usize)> = Vec::new();
    collect_separator_byte_ranges(root, &mut separators, &mut analysis.too_complex);
    separators.sort_by_key(|&(start, _)| start);

    let mut last = 0usize;
    for (start, end) in &separators {
        push_text_subcommand(command, last, *start, &mut analysis.subcommands);
        last = *end;
    }
    push_text_subcommand(command, last, command.len(), &mut analysis.subcommands);

    if analysis.subcommands.is_empty() && !trimmed_input.is_empty() {
        analysis.subcommands.push(trimmed_input.to_string());
    }

    walk_named_descendants(root, |node| {
        if is_complexity_marker_node(node.kind()) {
            analysis.too_complex = true;
        }
    });

    analysis
}

/// Record byte ranges of every separator token in the tree. `&&`, `||`,
/// `;`, `;;`, `|`, and `\n` are pure separators; bare `&` is also a separator
/// but additionally flags `too_complex` because it backgrounds the
/// preceding command.
///
/// Walks every named node and inspects its anonymous children. This mirrors
/// the legacy bespoke splitter, which split greedily on any unquoted
/// separator character regardless of structural context (e.g. inside
/// `if`/`case`/loops, command substitutions, or subshells) — tree-sitter's
/// quoting rules already keep separators inside `string` / `raw_string` /
/// `ansi_c_string` opaque, so this descent does not over-split quoted text.
fn collect_separator_byte_ranges(
    node: Node<'_>,
    positions: &mut Vec<(usize, usize)>,
    too_complex: &mut bool,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            collect_separator_byte_ranges(child, positions, too_complex);
            continue;
        }
        match child.kind() {
            ";" | ";;" | "&&" | "||" | "|" | "\n" => {
                positions.push((child.start_byte(), child.end_byte()));
            }
            "&" => {
                positions.push((child.start_byte(), child.end_byte()));
                *too_complex = true;
            }
            _ => {}
        }
    }
}

fn push_text_subcommand(source: &str, start: usize, end: usize, subcommands: &mut Vec<String>) {
    if let Some(slice) = source.get(start..end) {
        let trimmed = slice.trim();
        if !trimmed.is_empty() {
            subcommands.push(trimmed.to_string());
        }
    }
}

fn is_complexity_marker_node(kind: &str) -> bool {
    matches!(
        kind,
        "command_substitution"
            | "process_substitution"
            | "subshell"
            | "compound_statement"
            | "if_statement"
            | "case_statement"
            | "while_statement"
            | "for_statement"
            | "c_style_for_statement"
            | "select_statement"
            | "function_definition"
            | "file_redirect"
            | "heredoc_redirect"
            | "herestring_redirect"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_splits_simple_separators() {
        let analysis = analyze("ls; pwd | wc -l");
        assert_eq!(
            analysis.subcommands,
            vec!["ls".to_string(), "pwd".to_string(), "wc -l".to_string()],
        );
        assert!(!analysis.too_complex);
    }

    #[test]
    fn analyze_flags_compound_statements_complex() {
        let analysis = analyze("if true; then echo ok; fi");
        assert_eq!(
            analysis.subcommands,
            vec![
                "if true".to_string(),
                "then echo ok".to_string(),
                "fi".to_string(),
            ],
        );
        assert!(analysis.too_complex);
    }

    #[test]
    fn analyze_flags_command_substitution_complex() {
        let analysis = analyze("foo $(bar)");
        assert!(analysis.too_complex);
    }

    #[test]
    fn analyze_flags_backtick_substitution_complex() {
        let analysis = analyze("foo `bar`");
        assert!(analysis.too_complex);
    }

    #[test]
    fn analyze_flags_subshell_complex() {
        let analysis = analyze("(rm -rf /); ls");
        assert_eq!(analysis.subcommands.len(), 2);
        assert!(analysis.too_complex);
    }

    #[test]
    fn analyze_flags_redirection_complex() {
        let analysis = analyze("foo > out");
        assert!(analysis.too_complex);
    }

    #[test]
    fn analyze_flags_background_complex() {
        let analysis = analyze("foo & bar");
        assert!(analysis.too_complex);
        assert_eq!(
            analysis.subcommands,
            vec!["foo".to_string(), "bar".to_string()]
        );
    }

    #[test]
    fn analyze_handles_empty_input() {
        let analysis = analyze("");
        assert!(analysis.subcommands.is_empty());
        assert!(!analysis.too_complex);
    }

    #[test]
    fn analyze_keeps_single_command_when_no_separator() {
        let analysis = analyze("rm -rf /tmp/example");
        assert_eq!(
            analysis.subcommands,
            vec!["rm -rf /tmp/example".to_string()],
        );
        assert!(!analysis.too_complex);
    }

    #[test]
    fn analyze_unbalanced_quotes_marks_too_complex() {
        let analysis = analyze("echo 'unterminated");
        assert!(analysis.too_complex);
    }
}
