//! Wrapper-stripping logic for bash command normalization.
//!
//! Iteratively peels off privilege escalation wrappers, scheduling wrappers,
//! and shell-command-string wrappers so the underlying "real" command can be
//! matched against permission rules.

use super::shell_bodies::{is_shell_command_token, shell_command_string_prefix_width};
use super::strip_leading_env_assignments;
use super::wrapper_spec;
use super::wrappers::env_wrap::{
    COMMAND_WRAPPER_SPEC, EXEC_WRAPPER_SPEC, env_wrapper_prefix_width, expand_env_split_string,
    is_env_command_token,
};
use super::wrappers::namespace::{
    BUBBLEWRAP_SPEC, CHROOT_SPEC, FIREJAIL_SPEC, NSENTER_SPEC, SYSTEMD_RUN_SPEC, UNSHARE_SPEC,
};
use super::wrappers::privilege::{DOAS_SPEC, PKEXEC_SPEC, RUNUSER_SPEC, SETPRIV_SPEC, SUDO_SPEC};
use super::wrappers::scheduling::{
    CHRT_SPEC, DBUS_RUN_SESSION_SPEC, EXTERNAL_TIME_SPEC, FLOCK_SPEC, GPG_AGENT_SPEC, IONICE_SPEC,
    NUMACTL_SPEC, PRLIMIT_SPEC, SETARCH_PERSONALITY_SPEC, SETARCH_SPEC, SETSID_SPEC,
    SSH_AGENT_SPEC, STDBUF_SPEC, STRACE_SPEC, TASKSET_SPEC, TIME_KEYWORD_SPEC, TIMEOUT_SPEC,
    WATCH_SPEC, XARGS_SPEC, is_nice_command_token, is_nohup_command_token,
};

pub(super) fn strip_bash_wrappers(tokens: &mut Vec<String>, strip_all_env_vars: bool) {
    strip_bash_wrappers_with_shell_command_strings(tokens, strip_all_env_vars, true);
}

pub(super) fn strip_bash_wrappers_with_shell_command_strings(
    tokens: &mut Vec<String>,
    strip_all_env_vars: bool,
    strip_shell_command_strings: bool,
) {
    loop {
        let before = tokens.clone();
        strip_one_bash_wrapper(tokens, strip_all_env_vars, strip_shell_command_strings);
        if *tokens == before {
            break;
        }
    }
}

/// Table-driven dispatch for the bulk of wrapper unwrappers. Each entry
/// names a WrapperSpec and whether the dispatch is gated on
/// `strip_all_env_vars`. The few wrappers with custom pre/post handling
/// (env's `expand_env_split_string`/`strip_leading_env_assignments`,
/// builtin/nohup/nice's bespoke positional handling,
/// shell_command_string's separate width parser) stay as explicit branches
/// in `strip_one_bash_wrapper`.
struct WrapperDispatch {
    spec: &'static wrapper_spec::WrapperSpec,
    requires_strip_all: bool,
}

const WRAPPER_DISPATCH_TABLE: &[WrapperDispatch] = &[
    // Privilege / namespace / scheduling / sandbox wrappers, only stripped
    // when the env-var stripping mode is on (deny-rule scan path).
    WrapperDispatch {
        spec: &COMMAND_WRAPPER_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &EXEC_WRAPPER_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &SUDO_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &DOAS_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &RUNUSER_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &SETPRIV_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &PRLIMIT_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &NUMACTL_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &SETARCH_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &SETARCH_PERSONALITY_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &PKEXEC_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &CHROOT_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &FLOCK_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &WATCH_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &SETSID_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &SSH_AGENT_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &GPG_AGENT_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &DBUS_RUN_SESSION_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &IONICE_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &CHRT_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &TASKSET_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &UNSHARE_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &NSENTER_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &STRACE_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &BUBBLEWRAP_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &FIREJAIL_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &SYSTEMD_RUN_SPEC,
        requires_strip_all: true,
    },
    WrapperDispatch {
        spec: &XARGS_SPEC,
        requires_strip_all: true,
    },
    // Always-strip wrappers (apply on both deny and rule-shape paths).
    WrapperDispatch {
        spec: &TIME_KEYWORD_SPEC,
        requires_strip_all: false,
    },
    WrapperDispatch {
        spec: &EXTERNAL_TIME_SPEC,
        requires_strip_all: false,
    },
    WrapperDispatch {
        spec: &STDBUF_SPEC,
        requires_strip_all: false,
    },
    WrapperDispatch {
        spec: &TIMEOUT_SPEC,
        requires_strip_all: false,
    },
];

fn strip_one_bash_wrapper(
    tokens: &mut Vec<String>,
    strip_all_env_vars: bool,
    strip_shell_command_strings: bool,
) {
    let Some(first) = tokens.first().map(String::as_str) else {
        return;
    };

    // env: expand `-S` split-string before the standard prefix strip, and run
    // `strip_leading_env_assignments` after so subsequent loop iterations see
    // the unwrapped command at index 0.
    if strip_all_env_vars && is_env_command_token(first) {
        if let Some(expanded) = expand_env_split_string(tokens) {
            *tokens = expanded;
        } else if let Some(width) = env_wrapper_prefix_width(tokens) {
            tokens.drain(0..width);
            strip_leading_env_assignments(tokens, true);
        }
        return;
    }

    // builtin: no options — just drop the keyword (and an optional `--`).
    if strip_all_env_vars && first == "builtin" {
        tokens.remove(0);
        if tokens.first().is_some_and(|token| token == "--") {
            tokens.remove(0);
        }
        return;
    }

    // shell command strings (bash -c '...', sh -c '...') are handled by a
    // dedicated prefix scanner because the trailing argument is the entire
    // script, not a wrapped command.
    if strip_all_env_vars && strip_shell_command_strings && is_shell_command_token(first) {
        if let Some(width) = shell_command_string_prefix_width(tokens) {
            tokens.drain(0..width);
        }
        return;
    }

    // nohup: keyword + optional `--`.
    if is_nohup_command_token(first) {
        tokens.remove(0);
        if tokens.first().is_some_and(|token| token == "--") {
            tokens.remove(0);
        }
        return;
    }

    // nice: keyword + optional priority (`-n N`, `-nN`, or `-N`) + optional `--`.
    if is_nice_command_token(first) {
        tokens.remove(0);
        if tokens.first().is_some_and(|token| token == "-n") && tokens.get(1).is_some() {
            tokens.drain(0..2);
        } else if tokens.first().is_some_and(|token| {
            token
                .strip_prefix("-n")
                .is_some_and(|value| !value.is_empty() && value.parse::<i32>().is_ok())
        }) {
            tokens.remove(0);
        } else if tokens
            .first()
            .is_some_and(|token| token.starts_with('-') && token[1..].parse::<i32>().is_ok())
        {
            tokens.remove(0);
        }
        if tokens.first().is_some_and(|token| token == "--") {
            tokens.remove(0);
        }
        return;
    }

    // Bulk path: walk the dispatch table.
    for dispatch in WRAPPER_DISPATCH_TABLE {
        if dispatch.requires_strip_all && !strip_all_env_vars {
            continue;
        }
        if !dispatch.spec.aliases.contains(&first) {
            continue;
        }
        if let Some(width) = wrapper_spec::wrapper_prefix_width(tokens, dispatch.spec) {
            tokens.drain(0..width);
        }
        return;
    }
}
