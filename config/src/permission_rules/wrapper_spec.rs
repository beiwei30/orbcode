//! Declarative wrapper-prefix matcher shared across argv unwrappers.
//!
//! Each "wrapper" (sudo, doas, runuser, sg, setpriv, prlimit, numactl,
//! setarch, pkexec, chroot, flock, watch, setsid, ssh-agent, ionice, chrt,
//! taskset, unshare, nsenter, strace, bwrap, firejail, systemd-run, time,
//! nohup, nice, stdbuf, timeout, xargs, …) follows the same argv shape:
//! `wrapper [options]* [--] command [command args]*`.
//!
//! The legacy implementation gives each wrapper its own 4-5 helper functions
//! (`is_X_command_token`, `X_wrapper_prefix_width`, `X_flag_option`,
//! `X_option_takes_value`, `X_inline_value_option`). That is ~150 lines per
//! wrapper × 30 wrappers ≈ 4500 lines of nearly-identical boilerplate.
//!
//! `WrapperSpec` captures the per-wrapper data declaratively so each wrapper
//! becomes a single `const` definition plus a shared `prefix_width`
//! function. The plan is to migrate wrappers in step 8 batches.

/// Declarative description of a wrapper's argv-level flag surface. Built as
/// `&'static` data so callers can keep wrappers in `const` definitions.
pub struct WrapperSpec {
    /// Tokens that identify the wrapper executable. Always includes the
    /// bare name (`"sudo"`); usually includes the common absolute paths
    /// (`/usr/bin/sudo`, `/usr/local/bin/sudo`, `/opt/homebrew/bin/sudo`).
    pub aliases: &'static [&'static str],
    /// Long flags that take no value (`--background`, `--login`, …).
    pub long_flags: &'static [&'static str],
    /// Short single-character flags that may appear bundled after a single
    /// `-` (e.g. `-AbE` packs `-A`, `-b`, `-E`). Stored as a string of
    /// characters; order does not matter.
    pub short_flag_chars: &'static str,
    /// Long options that consume the next token as their value
    /// (`--user root`).
    pub long_options_with_value: &'static [&'static str],
    /// Long options that consume the next TWO tokens as values
    /// (`bwrap --bind SRC DEST`).
    pub long_options_with_two_values: &'static [&'static str],
    /// Short options that consume the next token as their value
    /// (`-u root`). Stored as their full short form including the dash
    /// (`"-u"`) so the matcher can compare against tokens directly.
    pub short_options_with_value: &'static [&'static str],
    /// Extra long-option prefixes that accept an inline `--key=value` form.
    /// When `inline_for_all_long_options` is true, every entry of
    /// `long_options_with_value` is also matched as an inline prefix; in
    /// that case this field carries only the *extras* (long flags that have
    /// no separate-token form but still accept `=VALUE` syntax). Stored
    /// without the trailing `=`.
    pub inline_value_long_prefixes: &'static [&'static str],
    /// When true, every entry of `long_options_with_value` is also accepted
    /// in the inline `--key=value` form. Covers the GNU-style default where
    /// `--user root` and `--user=root` are equivalent. Eliminates the need
    /// to repeat the option list in `inline_value_long_prefixes`.
    pub inline_for_all_long_options: bool,
    /// Short options whose tokens may pack the value (`-uroot`,
    /// `-pPROMPT`). Stored as the single character without the dash.
    pub short_inline_value_chars: &'static str,
    /// When true, every entry of `short_options_with_value` (e.g. `-u`) is
    /// also accepted in the inline `-uVALUE` form. Eliminates the need to
    /// repeat the same character set in `short_inline_value_chars`.
    pub inline_for_all_short_options: bool,
    /// Tokens that immediately abort wrapper unwrapping when seen
    /// (`--dump`, `-p`, `--pid`, `--help`, `-c`/`--command`, etc.). Used
    /// for query-only or attach-only modes that don't wrap a command.
    pub forbidden_long: &'static [&'static str],
    /// Inline-value prefixes that abort unwrapping when seen
    /// (`--pid=123`, `--attach=123`). Compared with `starts_with` + `=`.
    pub forbidden_inline_prefixes: &'static [&'static str],
    /// Short single-char inline-value flags that abort unwrapping (e.g.
    /// `c` for `flock` rejects `-cCMD`). Stored as a character set.
    pub forbidden_short_inline_chars: &'static str,
    /// Long flags that must appear at least once before the wrapped
    /// command. If empty, no requirement; otherwise unwrapping fails when
    /// none of these were seen.
    pub required_one_of_long: &'static [&'static str],
    /// Short options (with-value form) that must appear at least once.
    /// Treated the same way as `required_one_of_long` but matched against
    /// `short_options_with_value` and `short_inline_value_chars` tokens.
    pub required_one_of_short_with_value: &'static [&'static str],
    /// Inline-value long prefixes whose presence satisfies the
    /// `required_one_of` check (`--user=root` for runuser).
    pub required_one_of_inline_prefixes: &'static [&'static str],
    /// Single short option characters whose inline form (`-uUSER`)
    /// satisfies the `required_one_of` check.
    pub required_one_of_short_inline_chars: &'static str,
    /// When true, the wrapped command must start AFTER a `--` operand —
    /// loose option parsing alone is not enough (e.g. `dbus-run-session`
    /// canonical form, `nsenter`/`unshare`).
    pub require_double_dash: bool,
    /// Number of positional argument tokens that must appear between the
    /// last option and the wrapped command (e.g. `chroot NEWROOT cmd` has
    /// `positional_args_before_command = 1`). Each positional argument is
    /// required; if fewer tokens follow, `wrapper_prefix_width` returns
    /// `None`. An optional `--` after the positional arguments is allowed
    /// and skipped.
    pub positional_args_before_command: usize,
    /// Long options that, if seen during parsing, satisfy the
    /// `positional_args_before_command` requirement (so taskset's
    /// `-c CPU-LIST` form doesn't also require a MASK positional).
    pub positional_satisfied_by_short_option: &'static [&'static str],
    /// If non-empty AND `tokens[1]` is in this set, consume that token
    /// before option parsing (e.g. `setarch ARCH [options] cmd` where
    /// ARCH must be one of i386, x86_64, etc.). Optional — when the
    /// token is not in the set, parsing continues normally.
    pub optional_leading_token_set: &'static [&'static str],
    /// Long inline-value prefixes that, if seen during parsing, satisfy
    /// the positional requirement (`--cpu-list=...`).
    pub positional_satisfied_by_inline_prefix: &'static [&'static str],
    /// When true, tokens matching `NAME=VALUE` (a bash environment
    /// assignment) are absorbed into the option chain (e.g. `env FOO=bar
    /// baz qux` consumes `FOO=bar` as part of the wrapper, then `baz` is
    /// the command).
    pub accept_env_assignment_tokens: bool,
}

/// Build a `&'static [&'static str]` alias list covering the bare name and
/// the three common install prefixes:
///   `["foo", "/usr/bin/foo", "/usr/local/bin/foo", "/opt/homebrew/bin/foo"]`
///
/// Usage in a const context inside this crate:
///   `..WrapperSpec::with_aliases(common_aliases!("foo"))`
macro_rules! common_aliases {
    ($name:literal) => {
        &[
            $name,
            concat!("/usr/bin/", $name),
            concat!("/usr/local/bin/", $name),
            concat!("/opt/homebrew/bin/", $name),
        ]
    };
}
pub(crate) use common_aliases;

impl WrapperSpec {
    /// Build a spec with only the alias list filled. All other fields
    /// default to empty/false; consumers extend by struct-update syntax:
    /// `WrapperSpec { long_flags: &[...], ..WrapperSpec::with_aliases(&[...]) }`.
    pub const fn with_aliases(aliases: &'static [&'static str]) -> Self {
        Self {
            aliases,
            long_flags: &[],
            short_flag_chars: "",
            long_options_with_value: &[],
            long_options_with_two_values: &[],
            short_options_with_value: &[],
            inline_value_long_prefixes: &[],
            inline_for_all_long_options: false,
            short_inline_value_chars: "",
            inline_for_all_short_options: false,
            forbidden_long: &[],
            forbidden_inline_prefixes: &[],
            forbidden_short_inline_chars: "",
            required_one_of_long: &[],
            required_one_of_short_with_value: &[],
            required_one_of_inline_prefixes: &[],
            required_one_of_short_inline_chars: "",
            require_double_dash: false,
            positional_args_before_command: 0,
            positional_satisfied_by_short_option: &[],
            positional_satisfied_by_inline_prefix: &[],
            optional_leading_token_set: &[],
            accept_env_assignment_tokens: false,
        }
    }
}

fn looks_like_bash_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl WrapperSpec {
    fn matches_alias(&self, token: &str) -> bool {
        self.aliases.contains(&token)
    }

    fn matches_long_flag(&self, token: &str) -> bool {
        self.long_flags.contains(&token)
    }

    fn matches_long_option_with_value(&self, token: &str) -> bool {
        self.long_options_with_value.contains(&token)
    }

    fn matches_long_option_with_two_values(&self, token: &str) -> bool {
        self.long_options_with_two_values.contains(&token)
    }

    fn matches_short_option_with_value(&self, token: &str) -> bool {
        self.short_options_with_value.contains(&token)
    }

    fn matches_inline_value_long(&self, token: &str) -> bool {
        let check = |prefix: &str| {
            token.starts_with(prefix) && token.as_bytes().get(prefix.len()) == Some(&b'=')
        };
        if self.inline_value_long_prefixes.iter().any(|p| check(p)) {
            return true;
        }
        self.inline_for_all_long_options && self.long_options_with_value.iter().any(|p| check(p))
    }

    fn matches_short_inline_value(&self, token: &str) -> bool {
        let mut chars = token.chars();
        let Some('-') = chars.next() else {
            return false;
        };
        let Some(second) = chars.next() else {
            return false;
        };
        if chars.next().is_none() {
            return false;
        }
        if self.short_inline_value_chars.contains(second) {
            return true;
        }
        if self.inline_for_all_short_options {
            self.short_options_with_value.iter().any(|opt| {
                let bytes = opt.as_bytes();
                bytes.len() == 2 && bytes[0] == b'-' && bytes[1] as char == second
            })
        } else {
            false
        }
    }

    /// Recognise a bundled short-flag token (e.g. `-AbE`) where every
    /// character after `-` appears in `short_flag_chars`. Returns false if
    /// the token contains any character outside the allowed set so callers
    /// reject unknown bundles instead of treating them as flags.
    fn matches_bundled_short_flag(&self, token: &str) -> bool {
        if self.short_flag_chars.is_empty() {
            return false;
        }
        let Some(body) = token
            .strip_prefix('-')
            .filter(|rest| !rest.starts_with('-'))
        else {
            return false;
        };
        if body.is_empty() {
            return false;
        }
        body.chars().all(|c| self.short_flag_chars.contains(c))
    }

    fn matches_forbidden(&self, token: &str) -> bool {
        if self.forbidden_long.contains(&token) {
            return true;
        }
        if self.forbidden_inline_prefixes.iter().any(|prefix| {
            token.starts_with(prefix) && token.as_bytes().get(prefix.len()) == Some(&b'=')
        }) {
            return true;
        }
        if !self.forbidden_short_inline_chars.is_empty() {
            let mut chars = token.chars();
            if chars.next() == Some('-')
                && chars
                    .next()
                    .is_some_and(|c| self.forbidden_short_inline_chars.contains(c))
                && (token.len() == 2 || chars.next().is_some())
            {
                return true;
            }
        }
        false
    }

    fn token_satisfies_required(&self, token: &str) -> bool {
        if self.required_one_of_long.contains(&token) {
            return true;
        }
        if self.required_one_of_short_with_value.contains(&token) {
            return true;
        }
        if self.required_one_of_inline_prefixes.iter().any(|prefix| {
            token.starts_with(prefix) && token.as_bytes().get(prefix.len()) == Some(&b'=')
        }) {
            return true;
        }
        if !self.required_one_of_short_inline_chars.is_empty() {
            let mut chars = token.chars();
            if chars.next() == Some('-')
                && chars
                    .next()
                    .is_some_and(|c| self.required_one_of_short_inline_chars.contains(c))
                && chars.next().is_some()
            {
                return true;
            }
        }
        false
    }

    fn has_required_constraint(&self) -> bool {
        !self.required_one_of_long.is_empty()
            || !self.required_one_of_short_with_value.is_empty()
            || !self.required_one_of_inline_prefixes.is_empty()
            || !self.required_one_of_short_inline_chars.is_empty()
    }
}

/// Find the index where the wrapped command begins inside `tokens`, given a
/// matching `spec`. Returns `None` if `tokens[0]` is not a wrapper alias
/// or if the option chain runs to the end without exposing a command token.
///
/// Mirrors the per-wrapper `X_wrapper_prefix_width` shape:
/// - Accepts any number of recognised flags / options between the wrapper
///   name and the wrapped command.
/// - Stops at the first non-option token (the command name) or after a
///   `--` separator.
/// - Returns `None` on an unrecognised dash-prefixed token to preserve the
///   bespoke implementations' fail-closed posture for unknown flags.
pub fn wrapper_prefix_width(tokens: &[String], spec: &WrapperSpec) -> Option<usize> {
    if !tokens
        .first()
        .map(String::as_str)
        .is_some_and(|first| spec.matches_alias(first))
    {
        return None;
    }

    let mut index = 1usize;
    // Consume an optional leading positional token (e.g. setarch's ARCH).
    if !spec.optional_leading_token_set.is_empty()
        && tokens
            .get(index)
            .is_some_and(|token| spec.optional_leading_token_set.iter().any(|t| *t == token))
    {
        index += 1;
    }
    let mut saw_double_dash = false;
    let mut satisfied_required = !spec.has_required_constraint();
    let mut positional_satisfied = false;
    while let Some(token) = tokens.get(index) {
        if spec.matches_forbidden(token) {
            return None;
        }
        if !satisfied_required && spec.token_satisfies_required(token) {
            satisfied_required = true;
        }
        if !positional_satisfied
            && (spec
                .positional_satisfied_by_short_option
                .iter()
                .any(|opt| *opt == token)
                || spec
                    .positional_satisfied_by_inline_prefix
                    .iter()
                    .any(|prefix| {
                        token.starts_with(prefix)
                            && token.as_bytes().get(prefix.len()) == Some(&b'=')
                    }))
        {
            positional_satisfied = true;
        }
        if token == "--" {
            index += 1;
            saw_double_dash = true;
            break;
        } else if spec.matches_long_flag(token) || spec.matches_bundled_short_flag(token) {
            index += 1;
        } else if spec.matches_long_option_with_two_values(token) {
            tokens.get(index + 2)?;
            index += 3;
        } else if spec.matches_long_option_with_value(token)
            || spec.matches_short_option_with_value(token)
        {
            tokens.get(index + 1)?;
            index += 2;
        } else if spec.matches_inline_value_long(token) || spec.matches_short_inline_value(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else if spec.accept_env_assignment_tokens && looks_like_bash_env_assignment(token) {
            index += 1;
        } else {
            break;
        }
    }

    if !satisfied_required {
        return None;
    }
    if spec.require_double_dash && !saw_double_dash {
        return None;
    }

    // Consume positional arguments that must appear between the last option
    // and the wrapped command. Skipped entirely when a `positional_satisfied`
    // option was seen (e.g. taskset's `-c CPU-LIST` form).
    if !positional_satisfied {
        for _ in 0..spec.positional_args_before_command {
            tokens.get(index)?;
            index += 1;
        }
    }

    // Allow an optional `--` separator between positional arguments and the
    // command. This mirrors how wrappers like `timeout 30 -- cmd` accept
    // either `timeout 30 cmd` or `timeout 30 -- cmd`. Also covers
    // taskset's `--cpu-list=0 -- cmd` form.
    if (spec.positional_args_before_command > 0 || positional_satisfied)
        && tokens.get(index).is_some_and(|token| token == "--")
    {
        index += 1;
    }

    tokens.get(index).is_some().then_some(index)
}

/// Subcommand-option scan: walks options starting at `tokens[start]` using
/// the same flag/option/inline/forbidden/required logic as
/// `wrapper_prefix_width`, but skips the alias check, the optional leading
/// token, and the trailing positional/double-dash handling. Used by
/// scanners that already know the command name (it lives at `tokens[0]`)
/// and need to find where the wrapper's option block ends so the
/// subcommand verb / positional / target can be inspected.
///
/// Returns the index of the first non-option token (or one past `--`),
/// or `None` if a forbidden token appears, an unrecognised dash-prefixed
/// token appears, an option-with-value is missing its value, or a
/// `required_one_of_*` constraint is not satisfied.
pub fn scan_options_block(tokens: &[String], start: usize, spec: &WrapperSpec) -> Option<usize> {
    let mut index = start;
    let mut satisfied_required = !spec.has_required_constraint();
    while let Some(token) = tokens.get(index) {
        if spec.matches_forbidden(token) {
            return None;
        }
        if !satisfied_required && spec.token_satisfies_required(token) {
            satisfied_required = true;
        }
        if token == "--" {
            index += 1;
            break;
        } else if spec.matches_long_flag(token) || spec.matches_bundled_short_flag(token) {
            index += 1;
        } else if spec.matches_long_option_with_two_values(token) {
            tokens.get(index + 2)?;
            index += 3;
        } else if spec.matches_long_option_with_value(token)
            || spec.matches_short_option_with_value(token)
        {
            tokens.get(index + 1)?;
            index += 2;
        } else if spec.matches_inline_value_long(token) || spec.matches_short_inline_value(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else if spec.accept_env_assignment_tokens && looks_like_bash_env_assignment(token) {
            index += 1;
        } else {
            break;
        }
    }
    if !satisfied_required {
        return None;
    }
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SPEC: WrapperSpec = WrapperSpec {
        long_flags: &["--verbose", "--quiet"],
        short_flag_chars: "vq",
        long_options_with_value: &["--user", "--prompt"],
        short_options_with_value: &["-u", "-p"],
        inline_value_long_prefixes: &["--user", "--prompt"],
        short_inline_value_chars: "up",
        ..WrapperSpec::with_aliases(&["foo", "/usr/local/bin/foo"])
    };

    fn words(parts: &[&str]) -> Vec<String> {
        parts.iter().map(std::string::ToString::to_string).collect()
    }

    #[test]
    fn rejects_unknown_executable() {
        assert_eq!(
            wrapper_prefix_width(&words(&["bar", "cmd"]), &TEST_SPEC),
            None
        );
    }

    #[test]
    fn finds_command_after_simple_wrapper() {
        assert_eq!(
            wrapper_prefix_width(&words(&["foo", "cmd", "arg"]), &TEST_SPEC),
            Some(1),
        );
    }

    #[test]
    fn skips_long_flag() {
        assert_eq!(
            wrapper_prefix_width(&words(&["foo", "--verbose", "cmd"]), &TEST_SPEC),
            Some(2),
        );
    }

    #[test]
    fn skips_long_option_with_value() {
        assert_eq!(
            wrapper_prefix_width(&words(&["foo", "--user", "root", "cmd"]), &TEST_SPEC),
            Some(3),
        );
    }

    #[test]
    fn skips_short_option_with_value() {
        assert_eq!(
            wrapper_prefix_width(&words(&["foo", "-u", "root", "cmd"]), &TEST_SPEC),
            Some(3),
        );
    }

    #[test]
    fn skips_inline_long_value() {
        assert_eq!(
            wrapper_prefix_width(&words(&["foo", "--user=root", "cmd"]), &TEST_SPEC),
            Some(2),
        );
    }

    #[test]
    fn skips_inline_short_value() {
        assert_eq!(
            wrapper_prefix_width(&words(&["foo", "-uroot", "cmd"]), &TEST_SPEC),
            Some(2),
        );
    }

    #[test]
    fn skips_bundled_short_flags() {
        assert_eq!(
            wrapper_prefix_width(&words(&["foo", "-vq", "cmd"]), &TEST_SPEC),
            Some(2),
        );
    }

    #[test]
    fn double_dash_terminates_options() {
        assert_eq!(
            wrapper_prefix_width(&words(&["foo", "--", "-not-a-flag"]), &TEST_SPEC),
            Some(2),
        );
    }

    #[test]
    fn rejects_unknown_long_flag() {
        assert_eq!(
            wrapper_prefix_width(&words(&["foo", "--unknown", "cmd"]), &TEST_SPEC),
            None,
        );
    }

    #[test]
    fn returns_none_when_no_command_follows() {
        assert_eq!(wrapper_prefix_width(&words(&["foo"]), &TEST_SPEC), None);
        assert_eq!(
            wrapper_prefix_width(&words(&["foo", "--verbose"]), &TEST_SPEC),
            None,
        );
    }

    #[test]
    fn recognises_absolute_path_aliases() {
        assert_eq!(
            wrapper_prefix_width(&words(&["/usr/local/bin/foo", "cmd"]), &TEST_SPEC),
            Some(1),
        );
    }

    const FORBIDDEN_SPEC: WrapperSpec = WrapperSpec {
        long_flags: &["--ok"],
        forbidden_long: &["--dump", "-p", "--pid"],
        forbidden_inline_prefixes: &["--pid"],
        forbidden_short_inline_chars: "c",
        ..WrapperSpec::with_aliases(&["bar"])
    };

    #[test]
    fn forbidden_long_aborts() {
        assert_eq!(
            wrapper_prefix_width(&words(&["bar", "--dump", "cmd"]), &FORBIDDEN_SPEC),
            None,
        );
        assert_eq!(
            wrapper_prefix_width(&words(&["bar", "--pid"]), &FORBIDDEN_SPEC),
            None,
        );
    }

    #[test]
    fn forbidden_inline_prefix_aborts() {
        assert_eq!(
            wrapper_prefix_width(&words(&["bar", "--pid=123", "cmd"]), &FORBIDDEN_SPEC),
            None,
        );
    }

    #[test]
    fn forbidden_short_inline_aborts() {
        assert_eq!(
            wrapper_prefix_width(&words(&["bar", "-cCMD", "cmd"]), &FORBIDDEN_SPEC),
            None,
        );
        assert_eq!(
            wrapper_prefix_width(&words(&["bar", "-c"]), &FORBIDDEN_SPEC),
            None,
        );
    }

    const REQUIRED_SPEC: WrapperSpec = WrapperSpec {
        long_flags: &["--daemon"],
        long_options_with_value: &["--user"],
        inline_value_long_prefixes: &["--user"],
        required_one_of_long: &["--daemon"],
        required_one_of_inline_prefixes: &["--user"],
        required_one_of_short_with_value: &["--user"],
        ..WrapperSpec::with_aliases(&["baz"])
    };

    #[test]
    fn required_flag_must_appear() {
        assert_eq!(
            wrapper_prefix_width(&words(&["baz", "cmd"]), &REQUIRED_SPEC),
            None,
        );
        assert_eq!(
            wrapper_prefix_width(&words(&["baz", "--daemon", "cmd"]), &REQUIRED_SPEC),
            Some(2),
        );
        assert_eq!(
            wrapper_prefix_width(&words(&["baz", "--user=root", "cmd"]), &REQUIRED_SPEC),
            Some(2),
        );
    }

    const DOUBLE_DASH_SPEC: WrapperSpec = WrapperSpec {
        long_flags: &["--quiet"],
        require_double_dash: true,
        ..WrapperSpec::with_aliases(&["qux"])
    };

    const POSITIONAL_SPEC: WrapperSpec = WrapperSpec {
        long_flags: &["--quiet"],
        positional_args_before_command: 1,
        ..WrapperSpec::with_aliases(&["wrap1"])
    };

    #[test]
    fn positional_argument_required_before_command() {
        assert_eq!(
            wrapper_prefix_width(&words(&["wrap1", "cmd"]), &POSITIONAL_SPEC),
            None,
        );
        assert_eq!(
            wrapper_prefix_width(&words(&["wrap1", "ROOT", "cmd"]), &POSITIONAL_SPEC),
            Some(2),
        );
        assert_eq!(
            wrapper_prefix_width(
                &words(&["wrap1", "--quiet", "ROOT", "cmd"]),
                &POSITIONAL_SPEC
            ),
            Some(3),
        );
        assert_eq!(
            wrapper_prefix_width(&words(&["wrap1", "ROOT", "--", "cmd"]), &POSITIONAL_SPEC,),
            Some(3),
        );
    }

    const AUTO_INLINE_SPEC: WrapperSpec = WrapperSpec {
        long_options_with_value: &["--user", "--prompt"],
        short_options_with_value: &["-u", "-p"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        ..WrapperSpec::with_aliases(&["auto"])
    };

    #[test]
    fn auto_inline_long_accepts_equal_form() {
        assert_eq!(
            wrapper_prefix_width(&words(&["auto", "--user=root", "cmd"]), &AUTO_INLINE_SPEC),
            Some(2),
        );
        assert_eq!(
            wrapper_prefix_width(
                &words(&["auto", "--user", "root", "cmd"]),
                &AUTO_INLINE_SPEC
            ),
            Some(3),
        );
    }

    #[test]
    fn auto_inline_short_accepts_packed_form() {
        assert_eq!(
            wrapper_prefix_width(&words(&["auto", "-uroot", "cmd"]), &AUTO_INLINE_SPEC),
            Some(2),
        );
        assert_eq!(
            wrapper_prefix_width(&words(&["auto", "-u", "root", "cmd"]), &AUTO_INLINE_SPEC),
            Some(3),
        );
    }

    #[test]
    fn scan_options_block_returns_first_non_option_index() {
        const OPTIONS: WrapperSpec = WrapperSpec {
            long_flags: &["--quiet"],
            long_options_with_value: &["--user"],
            ..WrapperSpec::with_aliases(&[])
        };
        assert_eq!(
            scan_options_block(
                &words(&["svn", "--quiet", "--user", "root", "commit"]),
                1,
                &OPTIONS
            ),
            Some(4),
        );
        assert_eq!(
            scan_options_block(&words(&["svn", "commit"]), 1, &OPTIONS),
            Some(1),
        );
        assert_eq!(
            scan_options_block(&words(&["svn", "--unknown"]), 1, &OPTIONS),
            None,
        );
    }

    #[test]
    fn scan_options_block_handles_double_dash_separator() {
        const OPTIONS: WrapperSpec = WrapperSpec {
            long_flags: &["--quiet"],
            ..WrapperSpec::with_aliases(&[])
        };
        assert_eq!(
            scan_options_block(&words(&["svn", "--quiet", "--", "weird-arg"]), 1, &OPTIONS),
            Some(3),
        );
    }

    #[test]
    fn require_double_dash_enforced() {
        assert_eq!(
            wrapper_prefix_width(&words(&["qux", "cmd"]), &DOUBLE_DASH_SPEC),
            None,
        );
        assert_eq!(
            wrapper_prefix_width(&words(&["qux", "--", "cmd"]), &DOUBLE_DASH_SPEC),
            Some(2),
        );
        assert_eq!(
            wrapper_prefix_width(&words(&["qux", "--quiet", "--", "cmd"]), &DOUBLE_DASH_SPEC),
            Some(3),
        );
    }
}
