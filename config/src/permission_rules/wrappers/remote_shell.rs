use super::super::wrapper_spec;

pub(in crate::permission_rules) fn ssh_remote_command_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_ssh_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if ssh_non_command_option(token) {
            return None;
        } else if ssh_short_flag_group(token) {
            index += 1;
        } else if ssh_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if ssh_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    let destination = tokens.get(index)?;
    if destination.starts_with('-') {
        return None;
    }
    index += 1;

    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

pub(in crate::permission_rules) fn sshpass_argv_command_bodies(tokens: &[String]) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_sshpass_command_token(token))
    {
        return Vec::new();
    }

    let Some(index) = wrapper_spec::scan_options_block(tokens, 1, &SSHPASS_OPTIONS) else {
        return Vec::new();
    };

    let Some(command_tokens) = tokens.get(index..) else {
        return Vec::new();
    };
    let mut bodies = Vec::new();
    if let Some(body) = join_non_empty_tokens(command_tokens) {
        bodies.push(body);
    }
    if let Some(body) = ssh_remote_command_body(command_tokens)
        && !bodies.iter().any(|existing| existing == &body)
    {
        bodies.push(body);
    }
    for body in ssh_option_command_string_bodies(command_tokens) {
        if !bodies.iter().any(|existing| existing == &body) {
            bodies.push(body);
        }
    }
    if let Some(body) = rsync_remote_shell_command_body(command_tokens)
        && !bodies.iter().any(|existing| existing == &body)
    {
        bodies.push(body);
    }

    bodies
}

fn is_sshpass_command_token(token: &str) -> bool {
    matches!(
        token,
        "sshpass" | "/usr/bin/sshpass" | "/usr/local/bin/sshpass" | "/opt/homebrew/bin/sshpass"
    )
}

fn join_non_empty_tokens(tokens: &[String]) -> Option<String> {
    (!tokens.is_empty())
        .then(|| tokens.join(" "))
        .filter(|body| !body.trim().is_empty())
}

const SSHPASS_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["-e", "-v"],
    long_options_with_value: &["-p", "-f", "-d", "-P"],
    short_inline_value_chars: "pfdP",
    forbidden_long: &["-h", "-V", "--help", "--version"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

pub(in crate::permission_rules) fn ssh_command_invokes_askpass_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_ssh_command_token(token))
    {
        return false;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if ssh_query_option(token) {
            return false;
        } else if ssh_askpass_short_flag_group(token) {
            index += 1;
        } else if ssh_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if ssh_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            break;
        }
    }

    tokens
        .get(index)
        .is_some_and(|destination| !destination.starts_with('-'))
}

fn ssh_query_option(token: &str) -> bool {
    matches!(token, "-G" | "-Q" | "-V")
        || token
            .strip_prefix('-')
            .filter(|value| !value.starts_with('-'))
            .is_some_and(|value| {
                value
                    .chars()
                    .any(|character| matches!(character, 'G' | 'V'))
            })
        || token
            .strip_prefix("-Q")
            .is_some_and(|value| !value.is_empty())
}

fn ssh_askpass_short_flag_group(token: &str) -> bool {
    token.starts_with('-')
        && !token.starts_with("--")
        && token.len() > 1
        && token.trim_start_matches('-').chars().all(|character| {
            matches!(
                character,
                '4' | '6'
                    | 'A'
                    | 'a'
                    | 'C'
                    | 'f'
                    | 'g'
                    | 'K'
                    | 'k'
                    | 'M'
                    | 'N'
                    | 'n'
                    | 'q'
                    | 's'
                    | 'T'
                    | 't'
                    | 'X'
                    | 'x'
                    | 'Y'
                    | 'y'
            )
        })
}

fn ssh_non_command_option(token: &str) -> bool {
    matches!(token, "-G" | "-N" | "-Q" | "-V" | "-W")
        || token
            .strip_prefix('-')
            .filter(|value| !value.starts_with('-'))
            .is_some_and(|value| {
                value
                    .chars()
                    .any(|character| matches!(character, 'G' | 'N' | 'V'))
            })
        || token
            .strip_prefix("-Q")
            .is_some_and(|value| !value.is_empty())
        || token
            .strip_prefix("-W")
            .is_some_and(|value| !value.is_empty())
}

fn ssh_short_flag_group(token: &str) -> bool {
    token.starts_with('-')
        && !token.starts_with("--")
        && token.len() > 1
        && token.trim_start_matches('-').chars().all(|character| {
            matches!(
                character,
                '4' | '6'
                    | 'A'
                    | 'a'
                    | 'C'
                    | 'f'
                    | 'g'
                    | 'K'
                    | 'k'
                    | 'M'
                    | 'n'
                    | 'q'
                    | 's'
                    | 'T'
                    | 't'
                    | 'X'
                    | 'x'
                    | 'Y'
                    | 'y'
            )
        })
}

fn ssh_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-B" | "-b"
            | "-c"
            | "-D"
            | "-E"
            | "-e"
            | "-F"
            | "-I"
            | "-i"
            | "-J"
            | "-L"
            | "-l"
            | "-m"
            | "-O"
            | "-o"
            | "-P"
            | "-p"
            | "-R"
            | "-S"
            | "-w"
    )
}

fn ssh_inline_value_option(token: &str) -> bool {
    let mut chars = token.chars();
    if chars.next() != Some('-') {
        return false;
    }
    matches!(
        chars.next(),
        Some(
            'B' | 'b'
                | 'c'
                | 'D'
                | 'E'
                | 'e'
                | 'F'
                | 'I'
                | 'i'
                | 'J'
                | 'L'
                | 'l'
                | 'm'
                | 'O'
                | 'o'
                | 'P'
                | 'p'
                | 'R'
                | 'S'
                | 'w'
        )
    ) && chars.next().is_some()
}

pub(in crate::permission_rules) fn ssh_option_command_string_bodies(
    tokens: &[String],
) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_ssh_command_token(token))
    {
        return Vec::new();
    }

    let mut bodies = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            break;
        } else if token == "-o" {
            let Some(option) = tokens.get(index + 1) else {
                return Vec::new();
            };
            push_ssh_command_option_body(option, &mut bodies);
            index += 2;
        } else if let Some(option) = token.strip_prefix("-o").filter(|value| !value.is_empty()) {
            push_ssh_command_option_body(option, &mut bodies);
            index += 1;
        } else if ssh_non_command_option(token) {
            return bodies;
        } else if ssh_short_flag_group(token) {
            index += 1;
        } else if ssh_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return bodies;
            }
            index += 2;
        } else if ssh_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return bodies;
        } else {
            break;
        }
    }

    bodies
}

fn is_ssh_command_token(token: &str) -> bool {
    matches!(
        token,
        "ssh" | "/bin/ssh" | "/usr/bin/ssh" | "/usr/local/bin/ssh" | "/opt/homebrew/bin/ssh"
    )
}

fn push_ssh_command_option_body(option: &str, bodies: &mut Vec<String>) {
    let option = option.trim_start();
    for name in ["ProxyCommand", "LocalCommand", "RemoteCommand"] {
        if let Some(body) = ssh_command_option_value(option, name)
            && !body.trim().is_empty()
            && !bodies.iter().any(|existing| existing == body.trim())
        {
            bodies.push(body.trim().to_string());
        }
    }
}

fn ssh_command_option_value<'a>(option: &'a str, name: &str) -> Option<&'a str> {
    let option_name = option.get(..name.len())?;
    if !option_name.eq_ignore_ascii_case(name) {
        return None;
    }
    let value = option.get(name.len()..)?.trim_start();
    value
        .strip_prefix('=')
        .map(str::trim_start)
        .or_else(|| (!value.is_empty()).then_some(value))
}

pub(in crate::permission_rules) fn tar_command_option_bodies(tokens: &[String]) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_tar_command_token(token))
    {
        return Vec::new();
    }

    let mut bodies = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            break;
        } else if token == "--checkpoint-action" {
            let Some(action) = tokens.get(index + 1) else {
                return bodies;
            };
            push_tar_checkpoint_exec_body(action, &mut bodies);
            index += 2;
        } else if let Some(action) = token.strip_prefix("--checkpoint-action=") {
            push_tar_checkpoint_exec_body(action, &mut bodies);
            index += 1;
        } else if token == "--to-command" {
            let Some(body) = tokens.get(index + 1) else {
                return bodies;
            };
            push_unique_command_body(body, &mut bodies);
            index += 2;
        } else if let Some(body) = token.strip_prefix("--to-command=") {
            push_unique_command_body(body, &mut bodies);
            index += 1;
        } else if token == "-I" || token == "--use-compress-program" {
            let Some(body) = tokens.get(index + 1) else {
                return bodies;
            };
            push_unique_command_body(body, &mut bodies);
            index += 2;
        } else if let Some(body) = token.strip_prefix("-I").filter(|body| !body.is_empty()) {
            push_unique_command_body(body, &mut bodies);
            index += 1;
        } else if let Some(body) = token.strip_prefix("--use-compress-program=") {
            push_unique_command_body(body, &mut bodies);
            index += 1;
        } else {
            index += 1;
        }
    }

    bodies
}

fn is_tar_command_token(token: &str) -> bool {
    matches!(
        token,
        "tar"
            | "gtar"
            | "/bin/tar"
            | "/usr/bin/tar"
            | "/usr/local/bin/tar"
            | "/usr/local/bin/gtar"
            | "/opt/homebrew/bin/tar"
            | "/opt/homebrew/bin/gtar"
    )
}

fn push_tar_checkpoint_exec_body(action: &str, bodies: &mut Vec<String>) {
    if let Some(body) = action.strip_prefix("exec=") {
        push_unique_command_body(body, bodies);
    }
}

pub(in crate::permission_rules) fn rsync_remote_shell_command_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_rsync_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            break;
        } else if matches!(token.as_str(), "-e" | "--rsh") {
            return tokens
                .get(index + 1)
                .cloned()
                .filter(|body| !body.trim().is_empty());
        } else if let Some(body) = token
            .strip_prefix("--rsh=")
            .or_else(|| token.strip_prefix("-e").filter(|value| !value.is_empty()))
        {
            return (!body.trim().is_empty()).then(|| body.trim().to_string());
        }
        index += 1;
    }

    None
}

pub(in crate::permission_rules) fn is_rsync_command_token(token: &str) -> bool {
    matches!(
        token,
        "rsync" | "/usr/bin/rsync" | "/usr/local/bin/rsync" | "/opt/homebrew/bin/rsync"
    )
}

pub(in crate::permission_rules) fn openssh_transfer_command_string_bodies(
    tokens: &[String],
) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_openssh_transfer_command_token(token))
    {
        return Vec::new();
    }

    let mut bodies = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            break;
        } else if matches!(token.as_str(), "-h" | "-V" | "--help" | "--version") {
            return bodies;
        } else if token == "-S" {
            let Some(body) = tokens.get(index + 1) else {
                return bodies;
            };
            push_unique_command_body(body, &mut bodies);
            index += 2;
        } else if let Some(body) = token.strip_prefix("-S").filter(|body| !body.is_empty()) {
            push_unique_command_body(body, &mut bodies);
            index += 1;
        } else if token == "-o" {
            let Some(option) = tokens.get(index + 1) else {
                return bodies;
            };
            push_ssh_command_option_body(option, &mut bodies);
            index += 2;
        } else if let Some(option) = token.strip_prefix("-o").filter(|value| !value.is_empty()) {
            push_ssh_command_option_body(option, &mut bodies);
            index += 1;
        } else if openssh_transfer_short_flag_group(token) {
            index += 1;
        } else if openssh_transfer_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return bodies;
            }
            index += 2;
        } else if openssh_transfer_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return bodies;
        } else {
            break;
        }
    }

    bodies
}

fn push_unique_command_body(body: &str, bodies: &mut Vec<String>) {
    let body = body.trim();
    if !body.is_empty() && !bodies.iter().any(|existing| existing == body) {
        bodies.push(body.to_string());
    }
}

fn is_openssh_transfer_command_token(token: &str) -> bool {
    matches!(
        token,
        "scp"
            | "sftp"
            | "/bin/scp"
            | "/bin/sftp"
            | "/usr/bin/scp"
            | "/usr/bin/sftp"
            | "/usr/local/bin/scp"
            | "/usr/local/bin/sftp"
            | "/opt/homebrew/bin/scp"
            | "/opt/homebrew/bin/sftp"
    )
}

fn openssh_transfer_short_flag_group(token: &str) -> bool {
    token.starts_with('-')
        && !token.starts_with("--")
        && token.len() > 1
        && token.trim_start_matches('-').chars().all(|character| {
            matches!(
                character,
                '3' | '4' | '6' | 'A' | 'a' | 'C' | 'f' | 'N' | 'O' | 'p' | 'q' | 'r' | 'v'
            )
        })
}

fn openssh_transfer_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-B" | "-b" | "-c" | "-D" | "-F" | "-i" | "-J" | "-l" | "-P" | "-R" | "-X"
    )
}

fn openssh_transfer_inline_value_option(token: &str) -> bool {
    let mut chars = token.chars();
    if chars.next() != Some('-') {
        return false;
    }
    matches!(
        chars.next(),
        Some('B' | 'b' | 'c' | 'D' | 'F' | 'i' | 'J' | 'l' | 'P' | 'R' | 'X')
    ) && chars.next().is_some()
}
