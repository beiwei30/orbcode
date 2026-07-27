use super::super::wrapper_spec::{self, common_aliases};

pub(in crate::permission_rules) const SUDO_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "--background",
            "--askpass",
            "--bell",
            "--login",
            "--non-interactive",
            "--preserve-env",
            "--reset-timestamp",
            "--set-home",
            "--stdin",
            "--shell",
            "--validate",
        ],
        short_flag_chars: "AbEHhiKknPSsVv",
        long_options_with_value: &[
            "--chdir",
            "--close-from",
            "--group",
            "--host",
            "--login-class",
            "--prompt",
            "--role",
            "--user",
        ],
        short_options_with_value: &[
            "-C", "-c", "-D", "-g", "-h", "-p", "-R", "-r", "-T", "-t", "-U", "-u",
        ],
        // long_options_with_value mirrored automatically; --preserve-env is a
        // long_flag that also accepts `--preserve-env=VAR1,VAR2`.
        inline_value_long_prefixes: &["--preserve-env"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("sudo"))
    };

fn is_sudo_command_token(token: &str) -> bool {
    SUDO_SPEC.aliases.contains(&token)
}

pub(in crate::permission_rules) fn sudo_command_invokes_askpass(tokens: &[String]) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_sudo_command_token(token))
    {
        return false;
    }

    let mut askpass = false;
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if sudo_askpass_option(token) {
            askpass = true;
            index += 1;
        } else if sudo_flag_option(token) {
            index += 1;
        } else if sudo_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if sudo_inline_value_option(token) {
            index += 1;
        } else if sudo_short_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            break;
        }
    }

    askpass && tokens.get(index).is_some()
}

pub(in crate::permission_rules) fn sudo_command_invokes_editor_from_tokens(
    tokens: &[String],
) -> bool {
    let Some(command) = tokens.first().map(String::as_str) else {
        return false;
    };
    if matches!(
        command,
        "sudoedit" | "/usr/bin/sudoedit" | "/usr/local/bin/sudoedit" | "/opt/homebrew/bin/sudoedit"
    ) {
        return sudo_edit_command_has_target(tokens, 1);
    }
    if !is_sudo_command_token(command) {
        return false;
    }

    let mut edit = false;
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if sudo_edit_option(token) {
            edit = true;
            index += 1;
        } else if sudo_flag_option(token) {
            index += 1;
        } else if sudo_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if sudo_inline_value_option(token) {
            index += 1;
        } else if sudo_short_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            break;
        }
    }

    edit && sudo_edit_command_has_target(tokens, index)
}

fn sudo_edit_option(token: &str) -> bool {
    matches!(token, "-e" | "--edit")
        || token
            .strip_prefix('-')
            .filter(|value| !value.starts_with('-'))
            .is_some_and(|value| value.chars().any(|character| character == 'e'))
}

fn sudo_edit_command_has_target(tokens: &[String], mut index: usize) -> bool {
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            continue;
        }
        return !token.starts_with('-');
    }
    false
}

fn sudo_askpass_option(token: &str) -> bool {
    token == "--askpass"
        || token == "-A"
        || (token.starts_with('-')
            && !token.starts_with("--")
            && token.len() > 2
            && token.trim_start_matches('-').chars().all(|character| {
                matches!(
                    character,
                    'A' | 'b'
                        | 'E'
                        | 'H'
                        | 'h'
                        | 'i'
                        | 'K'
                        | 'k'
                        | 'n'
                        | 'P'
                        | 'S'
                        | 's'
                        | 'V'
                        | 'v'
                )
            })
            && token.trim_start_matches('-').contains('A'))
}

fn sudo_flag_option(token: &str) -> bool {
    if matches!(
        token,
        "--background"
            | "--askpass"
            | "--bell"
            | "--login"
            | "--non-interactive"
            | "--preserve-env"
            | "--reset-timestamp"
            | "--set-home"
            | "--stdin"
            | "--shell"
            | "--validate"
    ) {
        return true;
    }

    token.starts_with('-')
        && token.len() > 1
        && token.trim_start_matches('-').chars().all(|character| {
            matches!(
                character,
                'A' | 'b' | 'E' | 'H' | 'h' | 'i' | 'K' | 'k' | 'n' | 'P' | 'S' | 's' | 'V' | 'v'
            )
        })
}

fn sudo_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-C" | "-c"
            | "-D"
            | "-g"
            | "-h"
            | "-p"
            | "-R"
            | "-r"
            | "-T"
            | "-t"
            | "-U"
            | "-u"
            | "--chdir"
            | "--close-from"
            | "--group"
            | "--host"
            | "--login-class"
            | "--prompt"
            | "--role"
            | "--user"
    )
}

fn sudo_inline_value_option(token: &str) -> bool {
    token.starts_with("--chdir=")
        || token.starts_with("--close-from=")
        || token.starts_with("--group=")
        || token.starts_with("--host=")
        || token.starts_with("--login-class=")
        || token.starts_with("--prompt=")
        || token.starts_with("--preserve-env=")
        || token.starts_with("--role=")
        || token.starts_with("--user=")
}

fn sudo_short_inline_value_option(token: &str) -> bool {
    let mut chars = token.chars();
    chars.next() == Some('-')
        && matches!(
            chars.next(),
            Some('C' | 'c' | 'D' | 'g' | 'h' | 'p' | 'R' | 'r' | 'T' | 't' | 'U' | 'u')
        )
        && chars.next().is_some()
}

pub(in crate::permission_rules) const DOAS_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        short_flag_chars: "ns",
        short_options_with_value: &["-u"],
        inline_for_all_short_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("doas"))
    };

// The legacy bespoke also required `--user=NAME` to be non-empty; the
// WrapperSpec inline-value matcher accepts an empty value after `=`. This is
// a conservative widening for deny-rule unwrapping only — allow rules never
// pass through wrapper unwrapping. Same trade-off applies to several other
// wrappers below (pkexec, flock).
pub(in crate::permission_rules) const RUNUSER_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["--preserve-environment", "--pty"],
        short_flag_chars: "mpP",
        long_options_with_value: &["--user", "--group", "--supp-group"],
        short_options_with_value: &["-u", "-g", "-G"],
        inline_for_all_long_options: true,
        short_inline_value_chars: "u",
        forbidden_long: &["-c", "--command", "--session-command"],
        forbidden_inline_prefixes: &["--command", "--session-command"],
        required_one_of_long: &["--user"],
        required_one_of_short_with_value: &["-u"],
        required_one_of_inline_prefixes: &["--user"],
        required_one_of_short_inline_chars: "u",
        ..wrapper_spec::WrapperSpec::with_aliases(&[
            "runuser",
            "/usr/bin/runuser",
            "/usr/sbin/runuser",
            "/usr/local/bin/runuser",
            "/opt/homebrew/bin/runuser",
        ])
    };

pub(in crate::permission_rules) fn is_runuser_command_token(token: &str) -> bool {
    RUNUSER_SPEC.aliases.contains(&token)
}

pub(in crate::permission_rules) const SETPRIV_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "--no-new-privs",
            "--clear-groups",
            "--keep-groups",
            "--init-groups",
            "--reset-env",
        ],
        long_options_with_value: &[
            "--reuid",
            "--regid",
            "--euid",
            "--egid",
            "--ruid",
            "--rgid",
            "--groups",
            "--inh-caps",
            "--ambient-caps",
            "--bounding-set",
            "--securebits",
            "--pdeathsig",
            "--selinux-label",
            "--apparmor-profile",
        ],
        inline_for_all_long_options: true,
        forbidden_long: &["--dump"],
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("setpriv"))
    };

pub(in crate::permission_rules) const PKEXEC_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["--disable-internal-agent"],
        long_options_with_value: &["--user"],
        inline_for_all_long_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("pkexec"))
    };
