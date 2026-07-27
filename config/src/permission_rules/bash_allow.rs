use super::{
    PermissionRule, analyze_bash_command, bash_argv_command_bodies, bash_command_matches_pattern,
    bash_shell_command_string_bodies, extract_command_from_tool_input, has_unescaped_wildcard,
    is_bare_shell_prefix, is_bash_prefix_token_like, is_bash_subcommand_like,
    nested_bash_command_candidates, normalize_bash_command_for_rule, tokenize_bash_words,
    tool_name_matches_rule, unescape_rule_literal,
};

pub fn bash_command_allowed_by_rules<'a>(
    rules: &'a [PermissionRule],
    tool_name: &str,
    tool_input: &str,
) -> Option<&'a PermissionRule> {
    let command = extract_command_from_tool_input(tool_input)?;
    let command = command.trim();
    let rules = rules
        .iter()
        .filter(|rule| tool_name_matches_rule(tool_name, &rule.tool_name))
        .collect::<Vec<_>>();

    for rule in &rules {
        let Some(content) = &rule.rule_content else {
            return Some(rule);
        };
        if !bash_rule_is_prefix_or_wildcard(content)
            && command == unescape_rule_literal(content).trim()
        {
            return Some(rule);
        }
    }

    let analysis = analyze_bash_command(command);
    let subcommands = if analysis.subcommands.is_empty() {
        vec![command.to_string()]
    } else {
        analysis.subcommands
    };

    let mut first_match = None;
    for subcommand in subcommands {
        let matched = rules.iter().find(|rule| {
            rule.rule_content
                .as_ref()
                .is_some_and(|content| bash_atomic_command_matches_allow_rule(&subcommand, content))
        })?;
        first_match.get_or_insert(*matched);
    }

    first_match
}

pub fn suggested_bash_permission_rules(command: &str) -> Vec<String> {
    const MAX_GROUPED_BASH_SUGGESTIONS: usize = 5;

    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    if bash_command_requires_single_exact_suggestion(trimmed) {
        return vec![suggested_single_bash_permission_rule(trimmed)];
    }

    let analysis = analyze_bash_command(trimmed);
    let candidates = if analysis.subcommands.len() > 1 {
        analysis.subcommands
    } else {
        vec![trimmed.to_string()]
    };

    let mut suggestions = Vec::new();
    for candidate in candidates {
        let rule = suggested_single_bash_permission_rule(&candidate);
        if !suggestions.iter().any(|existing| existing == &rule) {
            suggestions.push(rule);
        }
        if suggestions.len() >= MAX_GROUPED_BASH_SUGGESTIONS {
            break;
        }
    }
    suggestions
}

fn suggested_single_bash_permission_rule(command: &str) -> String {
    let trimmed = command.trim();
    if bash_command_uses_shell_command_string(trimmed) {
        return escape_bash_permission_literal(trimmed);
    }
    if bash_command_uses_argv_command_execution(trimmed) {
        return escape_bash_permission_literal(trimmed);
    }
    if bash_command_needs_exact_suggestion(trimmed) {
        return escape_bash_permission_literal(trimmed);
    }

    let Some(normalized) = normalize_bash_command_for_rule(trimmed, false) else {
        return escape_bash_permission_literal(trimmed);
    };
    let mut tokens = normalized.split_whitespace();
    let Some(first_token) = tokens.next() else {
        return escape_bash_permission_literal(trimmed);
    };
    if !is_bash_prefix_token_like(first_token) || is_bare_shell_prefix(first_token) {
        return escape_bash_permission_literal(trimmed);
    }
    if let Some(second_token) = tokens.next()
        && is_bash_subcommand_like(second_token)
    {
        return format!("{first_token} {second_token}:*");
    }
    format!("{first_token}:*")
}

fn bash_command_requires_single_exact_suggestion(command: &str) -> bool {
    command.contains('\n')
        || command.contains('`')
        || bash_command_uses_shell_control_syntax(command)
        || bash_command_uses_shell_command_string(command)
        || bash_command_uses_argv_command_execution(command)
        || !nested_bash_command_candidates(command).is_empty()
}

fn bash_command_needs_exact_suggestion(command: &str) -> bool {
    let analysis = analyze_bash_command(command);
    analysis.too_complex || analysis.subcommands.len() > 1
}

fn escape_bash_permission_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('*', "\\*")
}

pub(super) fn bash_atomic_command_matches_allow_rule(command: &str, pattern: &str) -> bool {
    if !bash_rule_is_prefix_or_wildcard(pattern) {
        return command.trim() == unescape_rule_literal(pattern).trim();
    }

    if bash_command_uses_shell_command_string(command)
        || bash_command_uses_argv_command_execution(command)
    {
        return false;
    }

    let analysis = analyze_bash_command(command);
    if analysis.too_complex || analysis.subcommands.len() > 1 {
        return false;
    }
    bash_single_command_candidate_matches_rule(command, pattern, false)
}

pub(super) fn bash_single_command_candidate_matches_rule(
    command: &str,
    pattern: &str,
    strip_all_env_vars: bool,
) -> bool {
    let mut candidates = vec![command.trim().to_string()];
    if let Some(normalized) = normalize_bash_command_for_rule(command, strip_all_env_vars)
        && normalized != candidates[0]
    {
        candidates.push(normalized);
    }

    candidates
        .iter()
        .any(|candidate| bash_command_matches_pattern(candidate, pattern))
}

fn bash_rule_is_prefix_or_wildcard(pattern: &str) -> bool {
    let pattern = pattern.trim();
    pattern.ends_with(":*") || has_unescaped_wildcard(pattern)
}

fn bash_command_uses_shell_control_syntax(command: &str) -> bool {
    analyze_bash_command(command)
        .subcommands
        .iter()
        .any(|subcommand| {
            tokenize_bash_words(subcommand).is_some_and(|tokens| {
                tokens.first().is_some_and(|token| {
                    matches!(
                        token.as_str(),
                        "if" | "then"
                            | "elif"
                            | "else"
                            | "fi"
                            | "while"
                            | "until"
                            | "do"
                            | "done"
                            | "case"
                            | "in"
                            | "esac"
                    )
                })
            })
        })
}

fn bash_command_uses_shell_command_string(command: &str) -> bool {
    !bash_shell_command_string_bodies(command).is_empty()
}

fn bash_command_uses_argv_command_execution(command: &str) -> bool {
    !bash_argv_command_bodies(command).is_empty()
}
