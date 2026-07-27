//! Environment/shell runner command body extraction:
//! direnv, nix-shell, nix CLI (develop/shell), guix, watchexec, entr, screen.

use super::super::wrapper_spec;

// ─── direnv ─────────────────────────────────────────────────────────────────

pub(in crate::permission_rules) fn direnv_exec_argv_command_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_direnv_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if direnv_global_flag_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    if tokens.get(index).is_none_or(|token| token != "exec") {
        return None;
    }
    index += 1;

    if tokens.get(index).is_some_and(|token| token == "--") {
        index += 1;
    }
    let directory = tokens.get(index)?;
    if directory.starts_with('-') {
        return None;
    }
    index += 1;

    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

fn is_direnv_command_token(token: &str) -> bool {
    matches!(
        token,
        "direnv" | "/usr/bin/direnv" | "/usr/local/bin/direnv" | "/opt/homebrew/bin/direnv"
    )
}

fn direnv_global_flag_option(token: &str) -> bool {
    matches!(token, "help" | "--help" | "-h" | "version")
}

// ─── nix-shell ──────────────────────────────────────────────────────────────

pub(in crate::permission_rules) fn nix_shell_run_command_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_nix_shell_command_token(token))
    {
        return None;
    }

    let mut body = None;
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            break;
        } else if matches!(token.as_str(), "--help" | "-h") {
            return None;
        } else if token == "--run" {
            body = tokens
                .get(index + 1)
                .filter(|body| !body.trim().is_empty())
                .cloned();
            index += 2;
        } else if let Some(value) = token.strip_prefix("--run=") {
            if !value.trim().is_empty() {
                body = Some(value.to_string());
            }
            index += 1;
        } else if nix_shell_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if nix_shell_inline_value_option(token) || nix_shell_flag_option(token) {
            index += 1;
        } else {
            index += 1;
        }
    }

    body
}

fn is_nix_shell_command_token(token: &str) -> bool {
    matches!(
        token,
        "nix-shell"
            | "/usr/bin/nix-shell"
            | "/usr/local/bin/nix-shell"
            | "/opt/homebrew/bin/nix-shell"
    )
}

fn nix_shell_flag_option(token: &str) -> bool {
    matches!(
        token,
        "--pure"
            | "--keep"
            | "--packages"
            | "--version"
            | "--verbose"
            | "-v"
            | "--quiet"
            | "--show-trace"
    )
}

fn nix_shell_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-p" | "--packages"
            | "-I"
            | "--include"
            | "-A"
            | "--attr"
            | "-E"
            | "--expr"
            | "--arg"
            | "--argstr"
            | "--keep"
    )
}

fn nix_shell_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--packages=")
        .or_else(|| token.strip_prefix("-p").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--include="))
        .or_else(|| token.strip_prefix("-I").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--attr="))
        .or_else(|| token.strip_prefix("-A").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--expr="))
        .or_else(|| token.strip_prefix("-E").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--keep="))
        .is_some_and(|value| !value.is_empty())
}

// ─── nix CLI (nix develop / nix shell) ─────────────────────────────────────

pub(in crate::permission_rules) fn nix_cli_command_argv_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_nix_command_token(token))
    {
        return None;
    }

    let mut index = wrapper_spec::scan_options_block(tokens, 1, &NIX_CLI_GLOBAL_OPTIONS)?;

    if !tokens
        .get(index)
        .is_some_and(|subcommand| matches!(subcommand.as_str(), "develop" | "shell"))
    {
        return None;
    }
    index += 1;

    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return None;
        } else if matches!(token.as_str(), "--command" | "-c") {
            return tokens.get(index + 1..).and_then(join_non_empty_tokens);
        } else if let Some(body) = token.strip_prefix("--command=") {
            return (!body.trim().is_empty()).then(|| body.to_string());
        } else if nix_cli_subcommand_flag_option(token) {
            index += 1;
        } else if nix_cli_subcommand_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if nix_cli_subcommand_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            index += 1;
        }
    }

    None
}

fn is_nix_command_token(token: &str) -> bool {
    matches!(
        token,
        "nix" | "/usr/bin/nix" | "/usr/local/bin/nix" | "/opt/homebrew/bin/nix"
    )
}

const NIX_CLI_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--verbose",
        "-v",
        "--quiet",
        "--debug",
        "--show-trace",
        "--offline",
        "--refresh",
        "--no-update-lock-file",
        "--no-write-lock-file",
        "--no-registries",
        "--impure",
        "--pure-eval",
    ],
    long_options_with_value: &[
        "--extra-experimental-features",
        "--store",
        "--inputs-from",
        "--override-input",
    ],
    inline_for_all_long_options: true,
    forbidden_long: &["--help", "-h", "--version"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn nix_cli_subcommand_flag_option(token: &str) -> bool {
    matches!(
        token,
        "--ignore-environment"
            | "--impure"
            | "--offline"
            | "--refresh"
            | "--no-update-lock-file"
            | "--no-write-lock-file"
    )
}

fn nix_cli_subcommand_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "--inputs-from" | "--override-input" | "--profile" | "--redirect"
    )
}

fn nix_cli_subcommand_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--inputs-from=")
        .or_else(|| token.strip_prefix("--override-input="))
        .or_else(|| token.strip_prefix("--profile="))
        .or_else(|| token.strip_prefix("--redirect="))
        .is_some_and(|value| !value.is_empty())
}

fn join_non_empty_tokens(tokens: &[String]) -> Option<String> {
    (!tokens.is_empty())
        .then(|| tokens.join(" "))
        .filter(|body| !body.trim().is_empty())
}

// ─── guix ───────────────────────────────────────────────────────────────────

pub(in crate::permission_rules) fn guix_shell_argv_command_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_guix_command_token(token))
    {
        return None;
    }

    let mut index = wrapper_spec::scan_options_block(tokens, 1, &GUIX_GLOBAL_OPTIONS)?;

    if !tokens
        .get(index)
        .is_some_and(|subcommand| matches!(subcommand.as_str(), "shell" | "environment"))
    {
        return None;
    }
    index += 1;

    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return tokens.get(index + 1..).and_then(join_non_empty_tokens);
        } else if matches!(token.as_str(), "--help" | "-h") {
            return None;
        } else if guix_shell_flag_option(token) {
            index += 1;
        } else if guix_shell_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if guix_shell_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            index += 1;
        }
    }

    None
}

fn is_guix_command_token(token: &str) -> bool {
    matches!(
        token,
        "guix" | "/usr/bin/guix" | "/usr/local/bin/guix" | "/opt/homebrew/bin/guix"
    )
}

const GUIX_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["--no-substitutes", "--fallback", "--rounds"],
    long_options_with_value: &["--substitute-urls", "--cores", "-c"],
    inline_value_long_prefixes: &["--substitute-urls", "--cores"],
    short_inline_value_chars: "c",
    forbidden_long: &["--help", "-h", "--version", "-V"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn guix_shell_flag_option(token: &str) -> bool {
    matches!(
        token,
        "--container"
            | "-C"
            | "--network"
            | "-N"
            | "--pure"
            | "--development"
            | "-D"
            | "--check"
            | "--rebuild-cache"
    )
}

fn guix_shell_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "--manifest"
            | "-m"
            | "--expression"
            | "-e"
            | "--file"
            | "-f"
            | "--preserve"
            | "-E"
            | "--expose"
            | "--share"
            | "--root"
            | "-r"
    )
}

fn guix_shell_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--manifest=")
        .or_else(|| token.strip_prefix("-m").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--expression="))
        .or_else(|| token.strip_prefix("-e").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--file="))
        .or_else(|| token.strip_prefix("-f").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--preserve="))
        .or_else(|| token.strip_prefix("-E").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--expose="))
        .or_else(|| token.strip_prefix("--share="))
        .or_else(|| token.strip_prefix("--root="))
        .or_else(|| token.strip_prefix("-r").filter(|value| !value.is_empty()))
        .is_some_and(|value| !value.is_empty())
}

// ─── watchexec ──────────────────────────────────────────────────────────────

pub(in crate::permission_rules) fn watchexec_argv_command_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_watchexec_command_token(token))
    {
        return None;
    }
    let index = wrapper_spec::scan_options_block(tokens, 1, &WATCHEXEC_OPTIONS)?;
    tokens.get(index..).and_then(join_non_empty_tokens)
}

const WATCHEXEC_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--clear",
        "-c",
        "--restart",
        "-r",
        "--postpone",
        "--no-vcs-ignore",
        "--no-project-ignore",
        "--no-global-ignore",
        "--ignore-nothing",
        "--poll",
    ],
    long_options_with_value: &[
        "--watch",
        "-w",
        "--exts",
        "-e",
        "--filter",
        "-f",
        "--ignore",
        "-i",
        "--debounce",
        "-d",
        "--shell",
        "--signal",
        "-s",
        "--env",
        "-E",
        "--emit-events-to",
        "--on-busy-update",
    ],
    inline_value_long_prefixes: &[
        "--watch",
        "-w",
        "--exts",
        "-e",
        "--filter",
        "-f",
        "--ignore",
        "-i",
        "--debounce",
        "-d",
        "--shell",
        "--signal",
        "-s",
        "--env",
        "-E",
        "--emit-events-to",
        "--on-busy-update",
    ],
    forbidden_long: &["--help", "-h", "--version", "-V"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn is_watchexec_command_token(token: &str) -> bool {
    matches!(
        token,
        "watchexec"
            | "/usr/bin/watchexec"
            | "/usr/local/bin/watchexec"
            | "/opt/homebrew/bin/watchexec"
    )
}

// ─── entr ───────────────────────────────────────────────────────────────────

pub(in crate::permission_rules) fn entr_argv_command_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_entr_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return tokens.get(index + 1..).and_then(join_non_empty_tokens);
        } else if matches!(token.as_str(), "--help" | "-h") {
            return None;
        } else if entr_short_flag_group(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            return tokens.get(index..).and_then(join_non_empty_tokens);
        }
    }

    None
}

fn is_entr_command_token(token: &str) -> bool {
    matches!(
        token,
        "entr" | "/usr/bin/entr" | "/usr/local/bin/entr" | "/opt/homebrew/bin/entr"
    )
}

fn entr_short_flag_group(token: &str) -> bool {
    token
        .strip_prefix('-')
        .filter(|flags| !flags.is_empty())
        .is_some_and(|flags| flags.chars().all(|flag| "0acdnprsz".contains(flag)))
}

// ─── screen ─────────────────────────────────────────────────────────────────

pub(in crate::permission_rules) fn screen_argv_command_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_screen_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if screen_non_command_option(token) {
            return None;
        } else if token.starts_with('-') {
            index += screen_option_width(tokens, index)?;
        } else {
            break;
        }
    }

    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

fn screen_non_command_option(token: &str) -> bool {
    matches!(token, "-list" | "-ls" | "-wipe" | "-v" | "-r" | "-x" | "-X")
        || token
            .strip_prefix('-')
            .filter(|value| !value.starts_with('-'))
            .is_some_and(|value| {
                value
                    .chars()
                    .any(|character| matches!(character, 'r' | 'R' | 'x' | 'X' | 'v'))
            })
}

fn screen_option_width(tokens: &[String], index: usize) -> Option<usize> {
    let token = tokens.get(index)?;
    let mut chars = token.strip_prefix('-')?.chars().peekable();
    chars.peek()?;
    while let Some(character) = chars.next() {
        if screen_option_takes_value(character) {
            if chars.peek().is_some() {
                return Some(1);
            }
            tokens.get(index + 1)?;
            return Some(2);
        }
        if !screen_flag_option(character) {
            return None;
        }
    }

    Some(1)
}

fn screen_flag_option(character: char) -> bool {
    matches!(
        character,
        'a' | 'A' | 'd' | 'i' | 'L' | 'm' | 'O' | 'q' | 'U'
    )
}

fn screen_option_takes_value(character: char) -> bool {
    matches!(character, 'c' | 'e' | 'h' | 'p' | 's' | 'S' | 't' | 'T')
}

fn is_screen_command_token(token: &str) -> bool {
    matches!(
        token,
        "screen" | "/usr/bin/screen" | "/usr/local/bin/screen" | "/opt/homebrew/bin/screen"
    )
}
