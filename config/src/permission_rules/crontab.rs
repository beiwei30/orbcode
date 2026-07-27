//! Crontab / `at` / `batch` command-token recognition and crontab schedule
//! body extraction.
//!
//! These helpers feed the heredoc / here-string deny-rule scanners in
//! `mod.rs` (e.g. `bash_tokens_invoke_crontab_stdin`) and the crontab
//! body decoders called whenever a heredoc-or-here-string-fed command
//! is recognised as `crontab -`. They are pure functions over strings
//! and token vectors, with no dependency on the bash AST or other
//! parser machinery, so they live in their own submodule.

pub(super) fn crontab_non_stdin_option(token: &str) -> bool {
    matches!(token, "-e" | "-l" | "-r")
        || token
            .strip_prefix('-')
            .filter(|value| !value.starts_with('-'))
            .is_some_and(|value| {
                value
                    .chars()
                    .any(|character| matches!(character, 'e' | 'l' | 'r'))
            })
}

pub(super) fn crontab_flag_option(token: &str) -> bool {
    matches!(token, "-i")
}

pub(super) fn crontab_option_takes_value(token: &str) -> bool {
    matches!(token, "-u")
}

pub(super) fn crontab_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("-u")
        .is_some_and(|value| !value.is_empty())
}

pub(super) fn crontab_command_invokes_editor_from_tokens(tokens: &[String]) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_crontab_command_token(token))
    {
        return false;
    }

    let mut edit = false;
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            break;
        } else if token == "-e" {
            edit = true;
            index += 1;
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
            let Some(short_flags) = token
                .strip_prefix('-')
                .filter(|value| !value.starts_with('-'))
            else {
                return false;
            };
            if short_flags.contains('e') {
                edit = true;
            }
            if short_flags
                .chars()
                .any(|character| character == 'l' || character == 'r')
            {
                return false;
            }
            index += 1;
        } else {
            break;
        }
    }

    edit
}

pub(super) fn is_at_or_batch_command_token(token: &str) -> bool {
    is_at_command_token(token) || is_batch_command_token(token)
}

pub(super) fn is_at_command_token(token: &str) -> bool {
    matches!(
        token,
        "at" | "/usr/bin/at" | "/usr/local/bin/at" | "/opt/homebrew/bin/at"
    )
}

pub(super) fn is_batch_command_token(token: &str) -> bool {
    matches!(
        token,
        "batch" | "/usr/bin/batch" | "/usr/local/bin/batch" | "/opt/homebrew/bin/batch"
    )
}

pub(super) fn is_crontab_command_token(token: &str) -> bool {
    matches!(
        token,
        "crontab" | "/usr/bin/crontab" | "/usr/local/bin/crontab" | "/opt/homebrew/bin/crontab"
    )
}

pub(super) fn crontab_command_bodies(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(crontab_line_command_body)
        .collect::<Vec<_>>()
}

fn crontab_line_command_body(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || is_crontab_env_assignment(trimmed) {
        return None;
    }

    if let Some(command) = crontab_macro_command_body(trimmed) {
        return Some(command);
    }

    let mut fields = trimmed.split_whitespace();
    for _ in 0..5 {
        fields.next()?;
    }
    let command_start = fields.next()?;
    let offset = trimmed.find(command_start)?;
    trimmed
        .get(offset..)
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(crontab_command_before_stdin_marker)
}

fn crontab_macro_command_body(line: &str) -> Option<String> {
    let (schedule, command) = line.split_once(char::is_whitespace)?;
    matches!(
        schedule,
        "@reboot"
            | "@yearly"
            | "@annually"
            | "@monthly"
            | "@weekly"
            | "@daily"
            | "@midnight"
            | "@hourly"
    )
    .then(|| command.trim())
    .filter(|command| !command.is_empty())
    .map(crontab_command_before_stdin_marker)
}

fn is_crontab_env_assignment(line: &str) -> bool {
    let Some((name, _)) = line.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
        && name
            .chars()
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
}

fn crontab_command_before_stdin_marker(command: &str) -> String {
    let mut escaped = false;
    for (index, character) in command.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == '%' {
            return command[..index].trim().to_string();
        }
    }
    command.trim().to_string()
}

pub(super) fn at_non_stdin_option(token: &str) -> bool {
    matches!(token, "-c" | "-d" | "-f" | "-l" | "-r")
        || token
            .strip_prefix('-')
            .filter(|value| !value.starts_with('-'))
            .is_some_and(|value| {
                value
                    .chars()
                    .any(|character| matches!(character, 'c' | 'd' | 'f' | 'l' | 'r'))
            })
}

pub(super) fn at_flag_option(token: &str) -> bool {
    matches!(token, "-m" | "-M" | "-V")
        || token
            .strip_prefix('-')
            .filter(|value| !value.starts_with('-'))
            .is_some_and(|value| {
                value
                    .chars()
                    .all(|character| matches!(character, 'm' | 'M' | 'V'))
            })
}

pub(super) fn at_option_takes_value(token: &str) -> bool {
    matches!(token, "-q")
}

pub(super) fn at_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("-q")
        .is_some_and(|value| !value.is_empty())
}
