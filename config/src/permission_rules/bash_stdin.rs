//! Bash heredoc / here-string body extraction for the deny-rule scanner.
//!
//! The four `bash_*_stdin_*_bodies` entry points run an AST-first path:
//! tree-sitter via [`super::bash_ast::heredocs`] / [`super::bash_ast::here_strings`]
//! handles the common forms, and small AST-tokenized line compensators cover
//! the three shapes tree-sitter-bash does not parse:
//! - `<<DELIM cmd` (heredoc operator before the recipient command)
//! - `cat <<A; bash <<B` (multi-heredoc-per-line, AST returns one
//!   confused heredoc whose body slurps both bodies)
//! - `N<<<word` (fd-prefixed here-string, AST classifies as
//!   `file_redirect`)

use super::bash_ast;
use super::crontab::{
    at_flag_option, at_inline_value_option, at_non_stdin_option, at_option_takes_value,
    crontab_command_bodies, crontab_flag_option, crontab_inline_value_option,
    crontab_non_stdin_option, crontab_option_takes_value, is_at_or_batch_command_token,
    is_batch_command_token, is_crontab_command_token,
};
use super::{
    is_shell_combined_command_option, is_shell_command_token, is_shell_flag_option,
    strip_bash_wrappers_with_shell_command_strings, strip_leading_env_assignments,
    tokenize_bash_words,
};

pub(super) fn bash_shell_stdin_heredoc_bodies(command: &str) -> Vec<String> {
    let mut bodies = bash_shell_stdin_heredoc_bodies_via_ast(command);
    extend_dedup(&mut bodies, bash_shell_stdin_heredoc_bodies_gap(command));
    bodies
}

pub(super) fn bash_crontab_stdin_heredoc_command_bodies(command: &str) -> Vec<String> {
    let mut bodies = bash_crontab_stdin_heredoc_command_bodies_via_ast(command);
    extend_dedup(
        &mut bodies,
        bash_crontab_stdin_heredoc_command_bodies_gap(command),
    );
    bodies
}

pub(super) fn bash_shell_stdin_here_string_bodies(command: &str) -> Vec<String> {
    let mut bodies = bash_shell_stdin_here_string_bodies_via_ast(command);
    extend_dedup(
        &mut bodies,
        bash_shell_stdin_here_string_bodies_gap(command),
    );
    bodies
}

pub(super) fn bash_crontab_stdin_here_string_command_bodies(command: &str) -> Vec<String> {
    let mut bodies = bash_crontab_stdin_here_string_command_bodies_via_ast(command);
    extend_dedup(
        &mut bodies,
        bash_crontab_stdin_here_string_command_bodies_gap(command),
    );
    bodies
}

fn bash_shell_stdin_heredoc_bodies_via_ast(command: &str) -> Vec<String> {
    if has_multiple_heredoc_introducers_per_line(command) {
        return Vec::new();
    }
    bash_ast::heredocs(command)
        .into_iter()
        .filter(ast_heredoc_recipient_invokes_shell_stdin)
        .filter_map(|heredoc| body_with_trailing_newline_if_nonempty(heredoc.body))
        .collect()
}

fn bash_crontab_stdin_heredoc_command_bodies_via_ast(command: &str) -> Vec<String> {
    if has_multiple_heredoc_introducers_per_line(command) {
        return Vec::new();
    }
    bash_ast::heredocs(command)
        .into_iter()
        .filter(ast_heredoc_recipient_invokes_crontab_stdin)
        .flat_map(|heredoc| crontab_command_bodies(&heredoc.body))
        .collect()
}

fn bash_shell_stdin_here_string_bodies_via_ast(command: &str) -> Vec<String> {
    bash_ast::here_strings(command)
        .into_iter()
        .filter(ast_here_string_recipient_invokes_shell_stdin)
        .filter(|here| !here.body.trim().is_empty())
        .map(|here| here.body)
        .collect()
}

fn bash_crontab_stdin_here_string_command_bodies_via_ast(command: &str) -> Vec<String> {
    bash_ast::here_strings(command)
        .into_iter()
        .filter(ast_here_string_recipient_invokes_crontab_stdin)
        .flat_map(|here| crontab_command_bodies(&here.body))
        .collect()
}

fn ast_heredoc_recipient_invokes_shell_stdin(heredoc: &bash_ast::Heredoc) -> bool {
    ast_recipient_invokes(heredoc.recipient_tokens.as_deref(), heredoc.fd, true)
}

fn ast_heredoc_recipient_invokes_crontab_stdin(heredoc: &bash_ast::Heredoc) -> bool {
    ast_recipient_invokes(heredoc.recipient_tokens.as_deref(), heredoc.fd, false)
}

fn ast_here_string_recipient_invokes_shell_stdin(here: &bash_ast::HereString) -> bool {
    ast_recipient_invokes(here.recipient_tokens.as_deref(), here.fd, true)
}

fn ast_here_string_recipient_invokes_crontab_stdin(here: &bash_ast::HereString) -> bool {
    ast_recipient_invokes(here.recipient_tokens.as_deref(), here.fd, false)
}

fn ast_recipient_invokes(tokens: Option<&[String]>, fd: u32, shell_stdin: bool) -> bool {
    let Some(tokens) = tokens else {
        return false;
    };
    let mut tokens = tokens.to_vec();
    strip_leading_env_assignments(&mut tokens, true);
    strip_bash_wrappers_with_shell_command_strings(&mut tokens, true, false);
    if shell_stdin {
        bash_tokens_invoke_stdin_script(&tokens, fd)
    } else {
        bash_tokens_invoke_crontab_stdin(&tokens, fd)
    }
}

fn body_with_trailing_newline_if_nonempty(mut body: String) -> Option<String> {
    if body.trim().is_empty() {
        return None;
    }
    if !body.ends_with('\n') {
        body.push('\n');
    }
    Some(body)
}

fn extend_dedup(bodies: &mut Vec<String>, additions: impl IntoIterator<Item = String>) {
    for body in additions {
        if !bodies.iter().any(|existing| existing == &body) {
            bodies.push(body);
        }
    }
}

fn bash_crontab_stdin_here_string_command_bodies_gap(command: &str) -> Vec<String> {
    let mut bodies = Vec::new();

    for here_string in gap_here_strings(command) {
        if here_string.body.trim().is_empty() {
            continue;
        }
        if !tokens_invoke_crontab_stdin(&here_string.recipient_tokens, here_string.fd)
            && !bash_command_invokes_crontab_stdin(&here_string.suffix, here_string.fd)
        {
            continue;
        }
        bodies.extend(crontab_command_bodies(&here_string.body));
    }

    bodies
}

fn bash_crontab_stdin_heredoc_command_bodies_gap(command: &str) -> Vec<String> {
    heredoc_gap_bodies(command, false)
        .into_iter()
        .flat_map(|body| crontab_command_bodies(&body))
        .collect()
}

fn bash_shell_stdin_here_string_bodies_gap(command: &str) -> Vec<String> {
    gap_here_strings(command)
        .into_iter()
        .filter(|here_string| !here_string.body.trim().is_empty())
        .filter(|here_string| {
            tokens_invoke_stdin_script(&here_string.recipient_tokens, here_string.fd)
                || bash_command_invokes_stdin_script(&here_string.suffix, here_string.fd)
        })
        .map(|here_string| here_string.body)
        .collect()
}

fn bash_shell_stdin_heredoc_bodies_gap(command: &str) -> Vec<String> {
    heredoc_gap_bodies(command, true)
}

fn heredoc_gap_bodies(command: &str, shell_stdin: bool) -> Vec<String> {
    let mut pending = Vec::<GapHeredoc>::new();
    let mut bodies = Vec::new();

    for line in command.split('\n') {
        if let Some(active) = pending.first_mut() {
            let delimiter_line = if active.strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if delimiter_line == active.delimiter {
                let active = pending.remove(0);
                if active.collect && !active.body.trim().is_empty() {
                    bodies.push(active.body);
                }
            } else if active.collect {
                active.body.push_str(line);
                active.body.push('\n');
            }
            continue;
        }

        for heredoc in gap_heredocs_in_line(line) {
            let collect = if shell_stdin {
                tokens_invoke_stdin_script(&heredoc.recipient_tokens, heredoc.fd)
                    || bash_command_invokes_stdin_script(&heredoc.suffix, heredoc.fd)
            } else {
                tokens_invoke_crontab_stdin(&heredoc.recipient_tokens, heredoc.fd)
                    || bash_command_invokes_crontab_stdin(&heredoc.suffix, heredoc.fd)
            };
            pending.push(GapHeredoc {
                delimiter: heredoc.delimiter,
                quoted: heredoc.quoted,
                strip_tabs: heredoc.strip_tabs,
                collect,
                body: String::new(),
            });
        }
    }

    bodies
}

fn gap_here_strings(command: &str) -> Vec<GapHereString> {
    let mut here_strings = Vec::new();

    for line in command.split('\n') {
        for segment in gap_line_segments(line) {
            let mut offset = 0usize;
            while let Some(operator) = find_operator(&segment, "<<<", offset) {
                let Some((body, suffix)) = segment
                    .get(operator + 3..)
                    .and_then(first_word_value_and_suffix)
                else {
                    offset = operator + 3;
                    continue;
                };
                let Some(prefix) = segment
                    .get(..operator)
                    .and_then(|prefix| stdin_redirection_prefix(prefix.trim()))
                else {
                    offset = operator + 3;
                    continue;
                };
                here_strings.push(GapHereString {
                    body,
                    fd: prefix.fd,
                    recipient_tokens: prefix.tokens,
                    suffix: suffix.to_string(),
                });
                offset = operator + 3;
            }
        }
    }

    here_strings
}

fn gap_heredocs_in_line(line: &str) -> Vec<GapHeredocIntro> {
    let mut heredocs = Vec::new();

    for segment in gap_line_segments(line) {
        let mut offset = 0usize;
        while let Some(operator) = find_operator(&segment, "<<", offset) {
            if segment
                .get(operator..)
                .is_some_and(|rest| rest.starts_with("<<<"))
            {
                offset = operator + 3;
                continue;
            }
            let strip_tabs = segment
                .get(operator..)
                .is_some_and(|rest| rest.starts_with("<<-"));
            let delimiter_start = operator + if strip_tabs { 3 } else { 2 };
            let Some(word) = segment
                .get(delimiter_start..)
                .and_then(bash_ast::first_word)
            else {
                offset = delimiter_start;
                continue;
            };
            let Some(prefix) = segment
                .get(..operator)
                .and_then(|prefix| stdin_redirection_prefix(prefix.trim()))
            else {
                offset = delimiter_start + word.end_byte;
                continue;
            };
            let suffix = segment
                .get(delimiter_start + word.end_byte..)
                .unwrap_or_default();
            heredocs.push(GapHeredocIntro {
                delimiter: word.value,
                quoted: heredoc_delimiter_is_quoted(&word.raw),
                strip_tabs,
                fd: prefix.fd,
                recipient_tokens: prefix.tokens,
                suffix: suffix.to_string(),
            });
            offset = delimiter_start + word.end_byte;
        }
    }

    heredocs
}

fn heredoc_delimiter_is_quoted(raw: &str) -> bool {
    raw.contains('\'') || raw.contains('"') || raw.contains('\\')
}

fn has_multiple_heredoc_introducers_per_line(command: &str) -> bool {
    command
        .split('\n')
        .any(|line| gap_heredocs_in_line(line).len() > 1)
}

fn gap_line_segments(line: &str) -> Vec<String> {
    bash_ast::analyze(line)
        .subcommands
        .into_iter()
        .flat_map(|segment| {
            segment
                .split(';')
                .map(str::trim)
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn find_operator(command: &str, operator: &str, offset: usize) -> Option<usize> {
    command
        .get(offset..)?
        .match_indices(operator)
        .find_map(|(index, _)| {
            let operator_index = offset + index;
            let prefix = command.get(..operator_index)?.trim();
            let suffix = command.get(operator_index + operator.len()..)?;
            if prefix.is_empty()
                || stdin_redirection_prefix(prefix).is_some()
                || first_word_value_and_suffix(suffix).is_some()
            {
                Some(operator_index)
            } else {
                None
            }
        })
}

fn first_word_value_and_suffix(command: &str) -> Option<(String, &str)> {
    let trimmed = command.trim_start();
    let leading_width = command.len().saturating_sub(trimmed.len());
    let word = bash_ast::first_word(trimmed)?;
    let suffix = command.get(leading_width + word.end_byte..)?;
    (!word.value.is_empty()).then_some((word.value, suffix))
}

fn tokens_invoke_stdin_script(tokens: &[String], fd: u32) -> bool {
    let mut tokens = tokens.to_vec();
    strip_leading_env_assignments(&mut tokens, true);
    strip_bash_wrappers_with_shell_command_strings(&mut tokens, true, false);
    bash_tokens_invoke_stdin_script(&tokens, fd)
}

fn tokens_invoke_crontab_stdin(tokens: &[String], fd: u32) -> bool {
    let mut tokens = tokens.to_vec();
    strip_leading_env_assignments(&mut tokens, true);
    strip_bash_wrappers_with_shell_command_strings(&mut tokens, true, false);
    bash_tokens_invoke_crontab_stdin(&tokens, fd)
}

fn stdin_redirection_prefix(prefix: &str) -> Option<BashStdinRedirectionPrefix> {
    let mut tokens = tokenize_bash_words(prefix)?;
    let fd = if tokens
        .last()
        .is_some_and(|token| token.chars().all(|character| character.is_ascii_digit()))
    {
        tokens.pop()?.parse::<u32>().ok()?
    } else {
        0
    };
    Some(BashStdinRedirectionPrefix { tokens, fd })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GapHereString {
    body: String,
    fd: u32,
    recipient_tokens: Vec<String>,
    suffix: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GapHeredocIntro {
    delimiter: String,
    quoted: bool,
    strip_tabs: bool,
    fd: u32,
    recipient_tokens: Vec<String>,
    suffix: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GapHeredoc {
    delimiter: String,
    quoted: bool,
    strip_tabs: bool,
    collect: bool,
    body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GapScannedHeredoc {
    body_start: usize,
    redirect_end: usize,
    body: String,
    quoted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BashStdinRedirectionPrefix {
    tokens: Vec<String>,
    fd: u32,
}

fn bash_tokens_invoke_stdin_script(tokens: &[String], fd: u32) -> bool {
    bash_tokens_invoke_shell_stdin(tokens, fd)
        || bash_tokens_invoke_shell_fd_script(tokens, fd)
        || bash_tokens_source_stdin(tokens, fd)
        || bash_tokens_invoke_at_stdin(tokens, fd)
}

fn bash_command_invokes_stdin_script(command: &str, fd: u32) -> bool {
    let Some(mut prefix) = stdin_redirection_prefix(command.trim()) else {
        return false;
    };
    strip_leading_env_assignments(&mut prefix.tokens, true);
    strip_bash_wrappers_with_shell_command_strings(&mut prefix.tokens, true, false);
    bash_tokens_invoke_stdin_script(&prefix.tokens, fd)
}

fn bash_command_invokes_crontab_stdin(command: &str, fd: u32) -> bool {
    let Some(mut prefix) = stdin_redirection_prefix(command.trim()) else {
        return false;
    };
    strip_leading_env_assignments(&mut prefix.tokens, true);
    strip_bash_wrappers_with_shell_command_strings(&mut prefix.tokens, true, false);
    bash_tokens_invoke_crontab_stdin(&prefix.tokens, fd)
}

fn bash_tokens_invoke_shell_stdin(tokens: &[String], fd: u32) -> bool {
    if fd != 0 {
        return false;
    }
    let Some(shell) = tokens.first() else {
        return false;
    };
    if !is_shell_command_token(shell) {
        return false;
    }

    let mut index = 1usize;
    let mut reads_stdin = tokens.len() == 1;
    let mut saw_stdin_option = false;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            reads_stdin = saw_stdin_option || tokens.get(index).is_none();
            continue;
        }
        if token == "-c" || is_shell_combined_command_option(token) {
            return false;
        }
        if shell_option_takes_value(token) {
            index += 2;
            reads_stdin = saw_stdin_option || tokens.get(index).is_none();
            continue;
        }
        if shell_option_has_inline_value(token) {
            index += 1;
            reads_stdin = saw_stdin_option || tokens.get(index).is_none();
            continue;
        }
        if token == "-s" || token.starts_with("-s") || token.starts_with("-") && token.contains('s')
        {
            reads_stdin = true;
            saw_stdin_option = true;
            index += 1;
            continue;
        }
        if is_shell_flag_option(token) {
            reads_stdin = saw_stdin_option || tokens.get(index + 1).is_none();
            index += 1;
            continue;
        }
        return reads_stdin;
    }

    reads_stdin
}

fn bash_tokens_invoke_shell_fd_script(tokens: &[String], fd: u32) -> bool {
    let Some(shell) = tokens.first() else {
        return false;
    };
    if !is_shell_command_token(shell) {
        return false;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "-c" || is_shell_combined_command_option(token) {
            return false;
        }
        if token == "--" {
            index += 1;
            continue;
        }
        if shell_option_takes_value(token) {
            index += 2;
            continue;
        }
        if shell_option_has_inline_value(token) {
            index += 1;
            continue;
        }
        if is_shell_flag_option(token) {
            index += 1;
            continue;
        }
        return bash_token_is_fd_path(token, fd);
    }

    false
}

pub(super) fn shell_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-O" | "+O" | "-o" | "+o" | "--init-file" | "--rcfile"
    )
}

pub(super) fn shell_option_has_inline_value(token: &str) -> bool {
    token.starts_with("--init-file=") || token.starts_with("--rcfile=")
}

fn bash_tokens_source_stdin(tokens: &[String], fd: u32) -> bool {
    let [source, target, ..] = tokens else {
        return false;
    };
    matches!(source.as_str(), "source" | ".")
        && (bash_token_is_stdin_path(target, fd) || bash_token_is_fd_path(target, fd))
}

fn bash_tokens_invoke_at_stdin(tokens: &[String], fd: u32) -> bool {
    if fd != 0 {
        return false;
    }
    let Some(command) = tokens.first().map(String::as_str) else {
        return false;
    };
    if !is_at_or_batch_command_token(command) {
        return false;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if at_non_stdin_option(token) {
            return false;
        } else if at_flag_option(token) {
            index += 1;
        } else if at_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if at_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            break;
        }
    }

    is_batch_command_token(command) || tokens.get(index).is_some()
}

fn bash_tokens_invoke_crontab_stdin(tokens: &[String], fd: u32) -> bool {
    if fd != 0 {
        return false;
    }
    let Some(command) = tokens.first().map(String::as_str) else {
        return false;
    };
    if !is_crontab_command_token(command) {
        return false;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if token == "-" {
            break;
        } else if crontab_non_stdin_option(token) {
            return false;
        } else if crontab_flag_option(token) {
            index += 1;
        } else if crontab_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if crontab_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            break;
        }
    }

    tokens.get(index).is_none_or(|token| token == "-")
}

fn bash_token_is_stdin_path(token: &str, fd: u32) -> bool {
    fd == 0 && matches!(token, "/dev/stdin" | "/dev/fd/0" | "/proc/self/fd/0")
}

fn bash_token_is_fd_path(token: &str, fd: u32) -> bool {
    if bash_token_is_stdin_path(token, fd) {
        return true;
    }
    let fd = fd.to_string();
    token
        .strip_prefix("/dev/fd/")
        .or_else(|| token.strip_prefix("/proc/self/fd/"))
        .is_some_and(|value| value == fd)
}

pub(super) fn scan_bash_heredocs(command: &str) -> BashHeredocScan {
    let ast_heredocs = bash_ast::heredocs(command);
    if ast_heredocs.is_empty()
        || has_multiple_heredoc_introducers_per_line(command)
        || !command.ends_with('\n')
    {
        let gap_heredocs = scan_gap_heredocs(command);
        if !gap_heredocs.is_empty() {
            let unquoted_heredoc_bodies = gap_heredocs
                .iter()
                .filter(|heredoc| !heredoc.quoted)
                .map(|heredoc| heredoc_body_with_trailing_newline(&heredoc.body))
                .collect();
            return BashHeredocScan {
                command_without_heredoc_bodies: command_without_gap_heredoc_bodies(
                    command,
                    &gap_heredocs,
                ),
                unquoted_heredoc_bodies,
            };
        }
    }

    let unquoted_heredoc_bodies = ast_heredocs
        .into_iter()
        .filter(|heredoc| !heredoc.quoted)
        .map(|heredoc| heredoc_body_with_trailing_newline(&heredoc.body))
        .collect();
    BashHeredocScan {
        command_without_heredoc_bodies: bash_ast::command_without_heredoc_bodies(command),
        unquoted_heredoc_bodies,
    }
}

fn scan_gap_heredocs(command: &str) -> Vec<GapScannedHeredoc> {
    let mut pending = Vec::<GapScannedHeredocPending>::new();
    let mut heredocs = Vec::new();
    let mut line_start = 0usize;

    for raw_line in command.split_inclusive('\n') {
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        if let Some(active) = pending.first_mut() {
            let delimiter_line = if active.strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if delimiter_line == active.delimiter {
                let active = pending.remove(0);
                heredocs.push(GapScannedHeredoc {
                    body_start: active.body_start,
                    redirect_end: line_start + raw_line.len(),
                    body: active.body,
                    quoted: active.quoted,
                });
            } else {
                active.body.push_str(line);
                if raw_line.ends_with('\n') {
                    active.body.push('\n');
                }
            }
            line_start += raw_line.len();
            continue;
        }

        for heredoc in gap_heredocs_in_line(line) {
            pending.push(GapScannedHeredocPending {
                delimiter: heredoc.delimiter,
                quoted: heredoc.quoted,
                strip_tabs: heredoc.strip_tabs,
                body_start: line_start + raw_line.len(),
                body: String::new(),
            });
        }
        line_start += raw_line.len();
    }

    for active in pending {
        heredocs.push(GapScannedHeredoc {
            body_start: active.body_start,
            redirect_end: command.len(),
            body: active.body,
            quoted: active.quoted,
        });
    }

    heredocs
}

fn command_without_gap_heredoc_bodies(command: &str, heredocs: &[GapScannedHeredoc]) -> String {
    let mut output = String::with_capacity(command.len());
    let mut cursor = 0usize;
    for heredoc in heredocs {
        if heredoc.body_start < cursor {
            continue;
        }
        if let Some(prefix) = command.get(cursor..heredoc.body_start) {
            output.push_str(prefix);
        }
        if let Some(blanked) = command.get(heredoc.body_start..heredoc.redirect_end) {
            for character in blanked.chars() {
                if character == '\n' {
                    output.push('\n');
                } else {
                    output.push(' ');
                }
            }
        }
        cursor = heredoc.redirect_end;
    }
    if let Some(rest) = command.get(cursor..) {
        output.push_str(rest);
    }
    output
}

fn heredoc_body_with_trailing_newline(body: &str) -> String {
    let mut body = body.to_string();
    if !body.ends_with('\n') && !body.is_empty() {
        body.push('\n');
    }
    body
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GapScannedHeredocPending {
    delimiter: String,
    quoted: bool,
    strip_tabs: bool,
    body_start: usize,
    body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BashHeredocScan {
    pub(super) command_without_heredoc_bodies: String,
    pub(super) unquoted_heredoc_bodies: Vec<String>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn shell_heredoc_gap_handles_operator_before_recipient() {
        let bodies = super::bash_shell_stdin_heredoc_bodies("<<EOF bash\nrm -rf /\nEOF\n");
        assert_eq!(bodies, vec!["rm -rf /\n".to_string()]);
    }

    #[test]
    fn shell_heredoc_gap_handles_multiple_introducers_per_line() {
        let bodies =
            super::bash_shell_stdin_heredoc_bodies("cat <<A; bash <<B\nsafe\nA\nrm -rf /\nB\n");
        assert_eq!(bodies, vec!["rm -rf /\n".to_string()]);
    }

    #[test]
    fn shell_heredoc_gap_avoids_ast_body_slurp_on_multiple_introducers() {
        let bodies =
            super::bash_shell_stdin_heredoc_bodies("bash <<A; cat <<B\nsafe\nA\nrm -rf /\nB\n");
        assert_eq!(bodies, vec!["safe\n".to_string()]);
    }

    #[test]
    fn shell_here_string_gap_handles_fd_prefixed_form() {
        let bodies = super::bash_shell_stdin_here_string_bodies("bash /dev/fd/3 3<<< 'rm -rf /'");
        assert_eq!(bodies, vec!["rm -rf /".to_string()]);
    }

    #[test]
    fn scan_heredocs_blanks_body_without_final_newline() {
        let scan = super::scan_bash_heredocs("cat <<EOF\nrm -rf /tmp/example\nEOF");
        assert!(
            !scan.command_without_heredoc_bodies.contains("rm -rf"),
            "{scan:?}"
        );
        assert_eq!(
            scan.unquoted_heredoc_bodies,
            vec!["rm -rf /tmp/example\n".to_string()]
        );
    }
}
