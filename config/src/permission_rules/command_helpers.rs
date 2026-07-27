//! Helpers for extracting command bodies from `find -exec`, `parallel`, and
//! `socat` invocations.

use super::wrapper_spec;

// ─── find -exec ──────────────────────────────────────────────────────────────

pub(super) fn find_exec_command_bodies(tokens: &[String]) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_find_command_token(token))
    {
        return Vec::new();
    }

    let mut bodies = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if matches!(token.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir") {
            let command_start = index + 1;
            let mut end = command_start;
            while let Some(command_token) = tokens.get(end) {
                if command_token == ";" || command_token == "+" {
                    break;
                }
                end += 1;
            }
            if tokens.get(end).is_some()
                && end > command_start
                && let Some(body) = tokens.get(command_start..end).map(|body| body.join(" "))
                && !bodies.iter().any(|existing| existing == &body)
            {
                bodies.push(body);
            }
            index = end.saturating_add(1);
        } else {
            index += 1;
        }
    }

    bodies
}

fn is_find_command_token(token: &str) -> bool {
    matches!(
        token,
        "find" | "/usr/bin/find" | "/usr/local/bin/find" | "/opt/homebrew/bin/find"
    )
}

// ─── parallel ────────────────────────────────────────────────────────────────

pub(super) fn parallel_command_bodies(tokens: &[String]) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_parallel_command_token(token))
    {
        return Vec::new();
    }

    let Some(index) = wrapper_spec::scan_options_block(tokens, 1, &PARALLEL_OPTIONS) else {
        return Vec::new();
    };

    let Some(first_body_token) = tokens.get(index) else {
        return Vec::new();
    };

    let mut bodies = Vec::new();
    if is_parallel_input_separator(first_body_token) {
        if is_parallel_literal_input_separator(first_body_token) {
            push_parallel_literal_argument_commands(tokens, index + 1, &mut bodies);
        }
        return bodies;
    }

    let command_start = index;
    let command_end = tokens
        .get(command_start..)
        .and_then(|body| {
            body.iter()
                .position(|token| is_parallel_input_separator(token))
        })
        .map_or(tokens.len(), |position| command_start + position);
    if command_end > command_start {
        bodies.push(tokens[command_start..command_end].join(" "));
    }

    bodies
}

fn is_parallel_command_token(token: &str) -> bool {
    matches!(
        token,
        "parallel" | "/usr/bin/parallel" | "/usr/local/bin/parallel" | "/opt/homebrew/bin/parallel"
    )
}

fn push_parallel_literal_argument_commands(
    tokens: &[String],
    mut index: usize,
    bodies: &mut Vec<String>,
) {
    while let Some(token) = tokens.get(index) {
        if is_parallel_input_separator(token) {
            if !is_parallel_literal_input_separator(token) {
                break;
            }
            index += 1;
            continue;
        }
        if !token.trim().is_empty() && !bodies.iter().any(|existing| existing == token) {
            bodies.push(token.clone());
        }
        index += 1;
    }
}

fn is_parallel_input_separator(token: &str) -> bool {
    matches!(token, ":::" | ":::+") || matches!(token, "::::" | "::::+")
}

fn is_parallel_literal_input_separator(token: &str) -> bool {
    matches!(token, ":::" | ":::+")
}

pub(super) const PARALLEL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "-k",
        "-m",
        "-q",
        "-t",
        "-u",
        "-v",
        "-X",
        "--bar",
        "--eta",
        "--group",
        "--keep-order",
        "--line-buffer",
        "--pipe",
        "--progress",
        "--quote",
        "--semaphore",
        "--tag",
        "--tty",
        "--ungroup",
        "--verbose",
        "--will-cite",
        "--xargs",
    ],
    long_options_with_value: &[
        "-a",
        "-C",
        "-d",
        "-E",
        "-j",
        "-L",
        "-n",
        "-N",
        "-P",
        "-S",
        "--arg-file",
        "--colsep",
        "--delimiter",
        "--env",
        "--halt",
        "--joblog",
        "--jobs",
        "--load",
        "--max-args",
        "--max-lines",
        "--max-procs",
        "--memfree",
        "--nice",
        "--results",
        "--retries",
        "--sshlogin",
        "--timeout",
        "--tmpdir",
        "--workdir",
    ],
    inline_value_long_prefixes: &[
        "--arg-file",
        "--colsep",
        "--delimiter",
        "--env",
        "--halt",
        "--joblog",
        "--jobs",
        "--load",
        "--max-args",
        "--max-lines",
        "--max-procs",
        "--memfree",
        "--nice",
        "--results",
        "--retries",
        "--sshlogin",
        "--timeout",
        "--tmpdir",
        "--workdir",
    ],
    short_inline_value_chars: "aCdEjLnNPS",
    forbidden_long: &["--dry-run"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

// ─── socat ───────────────────────────────────────────────────────────────────

pub(super) fn socat_shell_command_bodies(tokens: &[String]) -> Vec<String> {
    socat_address_tokens(tokens)
        .into_iter()
        .filter_map(|address| {
            socat_address_body(address, "SYSTEM").or_else(|| socat_address_body(address, "SHELL"))
        })
        .filter(|body| !body.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>()
}

pub(super) fn socat_exec_command_bodies(tokens: &[String]) -> Vec<String> {
    socat_address_tokens(tokens)
        .into_iter()
        .filter_map(|address| socat_address_body(address, "EXEC"))
        .filter(|body| !body.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>()
}

fn socat_address_tokens(tokens: &[String]) -> Vec<&str> {
    if !tokens
        .first()
        .is_some_and(|token| is_socat_command_token(token))
    {
        return Vec::new();
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if token == "-" {
            break;
        } else if socat_flag_option(token) {
            index += 1;
        } else if socat_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return Vec::new();
            }
            index += 2;
        } else if socat_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return Vec::new();
        } else {
            break;
        }
    }

    tokens
        .get(index..)
        .unwrap_or_default()
        .iter()
        .take(2)
        .map(String::as_str)
        .collect::<Vec<_>>()
}

fn socat_address_body<'a>(address: &'a str, kind: &str) -> Option<&'a str> {
    let (head, body) = address.split_once(':')?;
    head.eq_ignore_ascii_case(kind)
        .then_some(body)
        .map(socat_address_command_before_options)
        .filter(|body| !body.trim().is_empty())
}

fn socat_address_command_before_options(command: &str) -> &str {
    command
        .split_once(',')
        .map_or(command, |(body, _)| body)
        .trim()
}

fn socat_flag_option(token: &str) -> bool {
    matches!(
        token,
        "--experimental" | "--statistics" | "-s" | "-u" | "-U" | "-g" | "-0" | "-4" | "-6"
    ) || token.strip_prefix("-d").is_some_and(|value| {
        value
            .chars()
            .all(|character| character == 'd' || character.is_ascii_digit())
    }) || token == "-D"
        || token == "-v"
        || token == "-x"
}

fn socat_option_takes_value(token: &str) -> bool {
    matches!(token, "-r" | "-R" | "-b" | "-L" | "-W")
}

fn socat_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("-ly")
        .or_else(|| token.strip_prefix("-lf"))
        .or_else(|| token.strip_prefix("-lm"))
        .or_else(|| token.strip_prefix("-lp"))
        .or_else(|| token.strip_prefix("-S"))
        .or_else(|| token.strip_prefix("-t"))
        .or_else(|| token.strip_prefix("-T"))
        .is_some_and(|value| !value.is_empty())
}

fn is_socat_command_token(token: &str) -> bool {
    matches!(
        token,
        "socat" | "/usr/bin/socat" | "/usr/local/bin/socat" | "/opt/homebrew/bin/socat"
    )
}
