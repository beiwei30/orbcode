//! Shell command body extraction helpers.
//!
//! Functions that extract the "inner command" from shell-wrapping commands
//! like `sh -c '...'`, `eval ...`, `trap ... SIGNAL`, `su -c '...'`,
//! `sg GROUP -c '...'`, `flock ... -c '...'`, `script -c '...'`, and
//! `tmux new-session '...'`.

use super::bash_stdin::{shell_option_has_inline_value, shell_option_takes_value};
use super::wrappers::privilege::is_runuser_command_token;
use super::wrappers::scheduling::is_flock_command_token;

// ─── shell -c ───────────────────────────────────────────────────────────────

pub(super) fn shell_command_string_body(tokens: &[String]) -> Option<String> {
    let index = shell_command_string_prefix_width(tokens)?;
    tokens.get(index).cloned()
}

pub(super) fn shell_command_string_prefix_width(tokens: &[String]) -> Option<usize> {
    let shell = tokens.first()?;
    if !is_shell_command_token(shell) {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "-c" {
            return tokens.get(index + 1).map(|_| index + 1);
        }
        if is_shell_combined_command_option(token) {
            return tokens.get(index + 1).map(|_| index + 1);
        }
        if token == "--" {
            return None;
        } else if shell_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if shell_option_has_inline_value(token) {
            index += 1;
        } else if is_shell_flag_option(token) {
            index += 1;
        } else {
            return None;
        }
    }
    None
}

pub(super) fn is_shell_command_token(token: &str) -> bool {
    matches!(
        token,
        "bash"
            | "sh"
            | "zsh"
            | "dash"
            | "ksh"
            | "/bin/bash"
            | "/bin/sh"
            | "/bin/zsh"
            | "/bin/dash"
            | "/bin/ksh"
            | "/usr/bin/bash"
            | "/usr/bin/sh"
            | "/usr/bin/zsh"
            | "/usr/bin/dash"
            | "/usr/bin/ksh"
            | "/usr/local/bin/bash"
            | "/usr/local/bin/sh"
            | "/usr/local/bin/zsh"
            | "/usr/local/bin/dash"
            | "/usr/local/bin/ksh"
            | "/opt/homebrew/bin/bash"
            | "/opt/homebrew/bin/sh"
            | "/opt/homebrew/bin/zsh"
            | "/opt/homebrew/bin/dash"
            | "/opt/homebrew/bin/ksh"
    )
}

pub(super) fn is_shell_combined_command_option(token: &str) -> bool {
    token.starts_with('-') && !token.starts_with("--") && token.len() > 2 && token.ends_with('c')
}

pub(super) fn is_shell_flag_option(token: &str) -> bool {
    if shell_long_flag_option(token) {
        return true;
    }

    token.starts_with('-')
        && !token.starts_with("--")
        && token.len() > 1
        && token
            .trim_start_matches('-')
            .chars()
            .all(|character| character != 'c')
}

fn shell_long_flag_option(token: &str) -> bool {
    matches!(
        token,
        "--debugger"
            | "--dump-po-strings"
            | "--dump-strings"
            | "--help"
            | "--login"
            | "--noediting"
            | "--noprofile"
            | "--norc"
            | "--posix"
            | "--pretty-print"
            | "--restricted"
            | "--verbose"
            | "--version"
    )
}

// ─── eval ───────────────────────────────────────────────────────────────────

pub(super) fn eval_command_string_body(tokens: &[String]) -> Option<String> {
    if tokens.first().is_none_or(|token| token != "eval") {
        return None;
    }

    let mut index = 1usize;
    if tokens.get(index).is_some_and(|token| token == "--") {
        index += 1;
    }

    tokens.get(index).map(|_| tokens[index..].join(" "))
}

// ─── trap ───────────────────────────────────────────────────────────────────

pub(super) fn trap_command_string_body(tokens: &[String]) -> Option<String> {
    if tokens.first().is_none_or(|token| token != "trap") {
        return None;
    }

    let mut index = 1usize;
    if tokens.get(index).is_some_and(|token| token == "--") {
        index += 1;
    }

    let action = tokens.get(index)?;
    if matches!(action.as_str(), "-l" | "-p" | "-") || action.is_empty() {
        return None;
    }

    tokens.get(index + 1).map(|_| action.to_string())
}

// ─── su ─────────────────────────────────────────────────────────────────────

pub(super) fn su_command_string_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_su_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if matches!(token.as_str(), "-c" | "--command" | "--session-command") {
            return tokens.get(index + 1).cloned();
        }
        if let Some(value) = token
            .strip_prefix("--command=")
            .or_else(|| token.strip_prefix("--session-command="))
        {
            return (!value.is_empty()).then(|| value.to_string());
        }
        index += 1;
    }

    None
}

fn is_su_command_token(token: &str) -> bool {
    matches!(
        token,
        "su" | "/bin/su" | "/usr/bin/su" | "/usr/local/bin/su" | "/opt/homebrew/bin/su"
    )
}

// ─── runuser ────────────────────────────────────────────────────────────────

pub(super) fn runuser_command_string_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_runuser_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if matches!(token.as_str(), "-c" | "--command" | "--session-command") {
            return tokens.get(index + 1).cloned();
        }
        if let Some(value) = token
            .strip_prefix("--command=")
            .or_else(|| token.strip_prefix("--session-command="))
        {
            return (!value.is_empty()).then(|| value.to_string());
        }
        index += 1;
    }

    None
}

// ─── sg ─────────────────────────────────────────────────────────────────────

pub(super) fn sg_command_string_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_sg_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    if tokens
        .get(index)
        .is_some_and(|token| matches!(token.as_str(), "-" | "-l"))
    {
        index += 1;
    }
    let group = tokens.get(index)?;
    if group.starts_with('-') {
        return None;
    }
    index += 1;

    if tokens.get(index).is_some_and(|token| token == "-c") {
        index += 1;
    } else if tokens
        .get(index)
        .is_some_and(|token| token.starts_with('-'))
    {
        return None;
    }

    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

fn is_sg_command_token(token: &str) -> bool {
    matches!(
        token,
        "sg" | "/bin/sg" | "/usr/bin/sg" | "/usr/local/bin/sg" | "/opt/homebrew/bin/sg"
    )
}

// ─── flock -c ───────────────────────────────────────────────────────────────

pub(super) fn flock_command_string_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_flock_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "-c" || token == "--command" {
            return tokens.get(index + 1).cloned();
        }
        if let Some(value) = token
            .strip_prefix("-c")
            .filter(|value| !value.is_empty())
            .or_else(|| token.strip_prefix("--command="))
        {
            return (!value.is_empty()).then(|| value.to_string());
        }
        index += 1;
    }

    None
}

// ─── script -c ──────────────────────────────────────────────────────────────

pub(super) fn script_command_string_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_script_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "-c" || token == "--command" {
            return tokens.get(index + 1).cloned();
        }
        if let Some(value) = token
            .strip_prefix("-c")
            .filter(|value| !value.is_empty())
            .or_else(|| token.strip_prefix("--command="))
        {
            return (!value.is_empty()).then(|| value.to_string());
        }
        index += 1;
    }

    None
}

fn is_script_command_token(token: &str) -> bool {
    matches!(
        token,
        "script" | "/usr/bin/script" | "/usr/local/bin/script" | "/opt/homebrew/bin/script"
    )
}

// ─── tmux ───────────────────────────────────────────────────────────────────

pub(super) fn tmux_command_string_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_tmux_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if tmux_global_flag_option(token) {
            index += 1;
        } else if tmux_global_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if tmux_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    let subcommand = tokens.get(index)?;
    if !tmux_shell_command_subcommand(subcommand) {
        return None;
    }
    index += 1;

    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if tmux_subcommand_flag_option(subcommand, token) {
            index += 1;
        } else if tmux_subcommand_option_takes_value(subcommand, token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if tmux_subcommand_inline_value_option(subcommand, token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    match tokens.get(index..) {
        Some([body]) if !body.trim().is_empty() => Some(body.clone()),
        _ => None,
    }
}

fn is_tmux_command_token(token: &str) -> bool {
    matches!(
        token,
        "tmux" | "/usr/bin/tmux" | "/usr/local/bin/tmux" | "/opt/homebrew/bin/tmux"
    )
}

fn tmux_global_flag_option(token: &str) -> bool {
    matches!(token, "-2" | "-C" | "-D" | "-l" | "-N" | "-u" | "-v" | "-V")
}

fn tmux_global_option_takes_value(token: &str) -> bool {
    matches!(token, "-f" | "-L" | "-S")
}

fn tmux_global_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("-f")
        .or_else(|| token.strip_prefix("-L"))
        .or_else(|| token.strip_prefix("-S"))
        .is_some_and(|value| !value.is_empty())
}

fn tmux_shell_command_subcommand(token: &str) -> bool {
    matches!(
        token,
        "new-session" | "new-window" | "split-window" | "respawn-pane" | "respawn-window"
    )
}

fn tmux_subcommand_flag_option(subcommand: &str, token: &str) -> bool {
    match subcommand {
        "new-session" => matches!(token, "-A" | "-d" | "-D" | "-E" | "-P" | "-X"),
        "new-window" => matches!(token, "-a" | "-d" | "-k" | "-P"),
        "split-window" => matches!(token, "-b" | "-d" | "-f" | "-h" | "-I" | "-P" | "-v" | "-Z"),
        "respawn-pane" | "respawn-window" => matches!(token, "-k"),
        _ => false,
    }
}

fn tmux_subcommand_option_takes_value(subcommand: &str, token: &str) -> bool {
    match subcommand {
        "new-session" => matches!(
            token,
            "-c" | "-e" | "-F" | "-f" | "-n" | "-s" | "-t" | "-x" | "-y"
        ),
        "new-window" => matches!(token, "-c" | "-e" | "-F" | "-n" | "-t"),
        "split-window" => matches!(token, "-c" | "-e" | "-F" | "-l" | "-p" | "-t"),
        "respawn-pane" | "respawn-window" => matches!(token, "-c" | "-e" | "-t"),
        _ => false,
    }
}

fn tmux_subcommand_inline_value_option(subcommand: &str, token: &str) -> bool {
    let Some(option) = token.get(..2) else {
        return false;
    };
    token.len() > 2 && tmux_subcommand_option_takes_value(subcommand, option)
}
