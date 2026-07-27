use super::super::wrapper_spec::{self, common_aliases};

pub(in crate::permission_rules) const PRLIMIT_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["--noheadings", "--raw", "--verbose"],
        long_options_with_value: &["--output"],
        short_options_with_value: &["-o"],
        // prlimit's long options accept either `--output VALUE` or `--output=VALUE`,
        // and most resource-limit flags are inline-only (e.g. `--nofile=1024` has
        // no separate-token form). Auto-derive `--output=` from
        // long_options_with_value; keep the resource-limit list as inline extras.
        inline_value_long_prefixes: &[
            "--as",
            "--core",
            "--cpu",
            "--data",
            "--fsize",
            "--locks",
            "--memlock",
            "--msgqueue",
            "--nice",
            "--nofile",
            "--nproc",
            "--rss",
            "--rtprio",
            "--rttime",
            "--sigpending",
            "--stack",
        ],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        forbidden_long: &["-p", "--pid"],
        forbidden_inline_prefixes: &["--pid"],
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("prlimit"))
    };

pub(in crate::permission_rules) const NUMACTL_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["--all", "-a", "--localalloc", "-l", "--balancing", "-b"],
        long_options_with_value: &[
            "--interleave",
            "--membind",
            "--cpunodebind",
            "--physcpubind",
            "--preferred",
            "--preferred-many",
        ],
        short_options_with_value: &["-i", "-m", "-N", "-C", "-p", "-P"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        forbidden_long: &["--show", "--hardware", "-H"],
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("numactl"))
    };

const SETARCH_ARCH_TOKENS: &[&str] = &[
    "linux32", "linux64", "i386", "i486", "i586", "i686", "x86_64", "amd64", "arm", "armv7l",
    "aarch64", "ppc", "ppc64", "ppc64le", "s390", "s390x",
];

const SETARCH_LONG_FLAGS: &[&str] = &[
    "--32bit",
    "--fdpic-funcptrs",
    "--short-inode",
    "--addr-compat-layout",
    "--addr-no-randomize",
    "--whole-seconds",
    "--sticky-timeouts",
    "--read-implies-exec",
    "--mmap-page-zero",
    "--uname-2.6",
];
const SETARCH_SHORT_FLAGS: &str = "BFILRSTXZ3";

// Two specs cover the bespoke `setarch_wrapper_prefix_width(allow_arch)`:
// - SETARCH_SPEC: called via the `setarch` command, ARCH positional is
//   optional and consumed when tokens[1] is in SETARCH_ARCH_TOKENS.
// - SETARCH_PERSONALITY_SPEC: called via the `linux32` / `linux64` aliases,
//   ARCH is implied by the command name so no leading positional is
//   consumed.
pub(in crate::permission_rules) const SETARCH_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: SETARCH_LONG_FLAGS,
        short_flag_chars: SETARCH_SHORT_FLAGS,
        forbidden_long: &["--list", "--show"],
        optional_leading_token_set: SETARCH_ARCH_TOKENS,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("setarch"))
    };

pub(in crate::permission_rules) const SETARCH_PERSONALITY_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: SETARCH_LONG_FLAGS,
        short_flag_chars: SETARCH_SHORT_FLAGS,
        forbidden_long: &["--list", "--show"],
        ..wrapper_spec::WrapperSpec::with_aliases(&[
            "linux32",
            "linux64",
            "/usr/bin/linux32",
            "/usr/bin/linux64",
            "/usr/local/bin/linux32",
            "/usr/local/bin/linux64",
            "/opt/homebrew/bin/linux32",
            "/opt/homebrew/bin/linux64",
        ])
    };

pub(in crate::permission_rules) const FLOCK_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "--shared",
            "--exclusive",
            "--unlock",
            "--nonblock",
            "--close",
            "--no-fork",
            "--verbose",
        ],
        short_flag_chars: "sxunoF",
        long_options_with_value: &["--timeout", "--conflict-exit-code"],
        short_options_with_value: &["-w", "-E"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        forbidden_long: &["-c", "--command"],
        forbidden_short_inline_chars: "c",
        positional_args_before_command: 1,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("flock"))
    };

pub(in crate::permission_rules) const WATCH_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "--beep",
            "--color",
            "--errexit",
            "--chgexit",
            "--precise",
            "--equexit",
            "--no-title",
            "--exec",
            "--differences",
        ],
        short_flag_chars: "bcegpqtx",
        long_options_with_value: &["--interval"],
        short_options_with_value: &["-n", "-d"],
        inline_value_long_prefixes: &["--differences"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("watch"))
    };

pub(in crate::permission_rules) const SETSID_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["--ctty", "--fork", "--wait"],
        short_flag_chars: "cfw",
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("setsid"))
    };

pub(in crate::permission_rules) const SSH_AGENT_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["-c", "-s", "-D", "-d"],
        short_options_with_value: &["-a", "-E", "-O", "-P", "-t"],
        inline_for_all_short_options: true,
        forbidden_long: &["-k"],
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("ssh-agent"))
    };

pub(in crate::permission_rules) const GPG_AGENT_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "--verbose",
            "-v",
            "--quiet",
            "-q",
            "--batch",
            "--sh",
            "--csh",
            "--allow-preset-passphrase",
            "--allow-loopback-pinentry",
            "--no-grab",
            "--debug-quick-random",
            "--daemon",
        ],
        long_options_with_value: &[
            "--homedir",
            "--options",
            "--log-file",
            "--pinentry-program",
            "--scdaemon-program",
            "--extra-socket",
            "--browser-socket",
            "--debug",
            "--debug-level",
            "--default-cache-ttl",
            "--max-cache-ttl",
        ],
        inline_for_all_long_options: true,
        forbidden_long: &["--help", "-h", "--version", "--server"],
        required_one_of_long: &["--daemon"],
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("gpg-agent"))
    };

pub(in crate::permission_rules) const DBUS_RUN_SESSION_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_options_with_value: &["--config-file", "--dbus-daemon"],
        inline_for_all_long_options: true,
        forbidden_long: &["--help", "--version"],
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("dbus-run-session"))
    };

pub(in crate::permission_rules) const IONICE_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["--ignore"],
        short_flag_chars: "t",
        long_options_with_value: &["--class"],
        short_options_with_value: &["-c", "-n", "-p"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("ionice"))
    };

pub(in crate::permission_rules) const CHRT_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "--batch",
            "--deadline",
            "--fifo",
            "--idle",
            "--other",
            "--rr",
            "--reset-on-fork",
        ],
        short_flag_chars: "bdfiorR",
        long_options_with_value: &["--sched-runtime", "--sched-period", "--sched-deadline"],
        short_options_with_value: &["-T", "-P", "-D"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        forbidden_long: &["-p", "--pid"],
        // chrt's positional argument is a PRIORITY number; the bespoke
        // implementation also validated `priority.chars().all(is_ascii_digit)`.
        // WrapperSpec accepts any positional, slightly widening deny-rule
        // unwrapping (allow rules never go through this path).
        positional_args_before_command: 1,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("chrt"))
    };

pub(in crate::permission_rules) const TASKSET_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["--all-tasks"],
        short_flag_chars: "a",
        long_options_with_value: &["--cpu-list"],
        short_options_with_value: &["-c"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        forbidden_long: &["-p", "--pid"],
        forbidden_inline_prefixes: &["--pid"],
        // Default mode: `taskset MASK cmd` requires a positional MASK before
        // the command. `-c CPU-LIST` and `--cpu-list=...` forms supply the
        // cpu set as an option value, so the positional is satisfied by the
        // option and skipped.
        positional_args_before_command: 1,
        positional_satisfied_by_short_option: &["-c"],
        positional_satisfied_by_inline_prefix: &["--cpu-list"],
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("taskset"))
    };

pub(in crate::permission_rules) const STRACE_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "-f",
            "-ff",
            "-q",
            "-qq",
            "-r",
            "-t",
            "-tt",
            "-ttt",
            "-T",
            "-x",
            "-xx",
            "-y",
            "-yy",
            "-v",
            "-D",
            "-I",
            "--follow-forks",
            "--quiet",
            "--relative-timestamps",
            "--syscall-times",
            "--strings-in-hex",
            "--decode-fds",
            "--no-abbrev",
        ],
        long_options_with_value: &[
            "--output",
            "--trace",
            "--string-limit",
            "--user",
            "--env",
            "--trace-path",
        ],
        short_options_with_value: &["-o", "-e", "-s", "-u", "-E", "-P"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        forbidden_long: &["-p", "--attach"],
        forbidden_inline_prefixes: &["--attach"],
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("strace"))
    };

pub(in crate::permission_rules) const TIME_KEYWORD_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["-p"],
        ..wrapper_spec::WrapperSpec::with_aliases(&["time"])
    };

pub(in crate::permission_rules) const EXTERNAL_TIME_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "-p",
            "-a",
            "-h",
            "-l",
            "-v",
            "--append",
            "--portability",
            "--quiet",
            "--verbose",
        ],
        long_options_with_value: &["--format", "--output"],
        short_options_with_value: &["-f", "-o"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(&[
            "/usr/bin/time",
            "/usr/local/bin/time",
            "/opt/homebrew/bin/time",
            "gtime",
            "/usr/local/bin/gtime",
            "/opt/homebrew/bin/gtime",
        ])
    };

pub(in crate::permission_rules) const STDBUF_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_options_with_value: &["--input", "--output", "--error"],
        short_options_with_value: &["-i", "-o", "-e"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("stdbuf"))
    };

pub(in crate::permission_rules) const TIMEOUT_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["--foreground", "--preserve-status", "--verbose", "-v"],
        long_options_with_value: &["--kill-after", "--signal"],
        short_options_with_value: &["-k", "-s"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        // timeout takes a required DURATION (e.g. `30s`, `10m`) before the
        // command. The bespoke implementation validated NUMBER[smhd] form;
        // WrapperSpec accepts any positional, slightly widening deny-rule
        // unwrapping.
        positional_args_before_command: 1,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("timeout"))
    };

pub(in crate::permission_rules) const XARGS_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["--null", "--no-run-if-empty", "--verbose", "--exit"],
        short_flag_chars: "0rtx",
        long_options_with_value: &[
            "--arg-file",
            "--delimiter",
            "--eof",
            "--replace",
            "--max-lines",
            "--max-args",
            "--max-procs",
            "--max-chars",
        ],
        short_options_with_value: &["-a", "-d", "-E", "-I", "-i", "-L", "-l", "-n", "-P", "-s"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("xargs"))
    };

pub(in crate::permission_rules) fn is_nohup_command_token(token: &str) -> bool {
    matches!(
        token,
        "nohup" | "/usr/bin/nohup" | "/usr/local/bin/nohup" | "/opt/homebrew/bin/nohup"
    )
}

pub(in crate::permission_rules) fn is_nice_command_token(token: &str) -> bool {
    matches!(
        token,
        "nice" | "/usr/bin/nice" | "/usr/local/bin/nice" | "/opt/homebrew/bin/nice"
    )
}

pub(in crate::permission_rules) fn is_flock_command_token(token: &str) -> bool {
    FLOCK_SPEC.aliases.contains(&token)
}

pub(in crate::permission_rules) fn xargs_wrapper_prefix_width(tokens: &[String]) -> Option<usize> {
    wrapper_spec::wrapper_prefix_width(tokens, &XARGS_SPEC)
}

pub(in crate::permission_rules) fn is_xargs_command_token(token: &str) -> bool {
    XARGS_SPEC.aliases.contains(&token)
}
