use super::super::tokenize_bash_words;
use super::super::wrapper_spec::{self, common_aliases};

pub(in crate::permission_rules) fn is_env_command_token(token: &str) -> bool {
    ENV_WRAPPER_SPEC.aliases.contains(&token)
}

pub(in crate::permission_rules) fn expand_env_split_string(
    tokens: &[String],
) -> Option<Vec<String>> {
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        let (split_string, next_index) = if token == "-S" || token == "--split-string" {
            (tokens.get(index + 1)?.as_str(), index + 2)
        } else if let Some(value) = token.strip_prefix("-S") {
            if value.is_empty() {
                return None;
            }
            (value, index + 1)
        } else if let Some(value) = token.strip_prefix("--split-string=") {
            if value.is_empty() {
                return None;
            }
            (value, index + 1)
        } else {
            index += 1;
            continue;
        };

        let split_tokens = tokenize_bash_words(split_string)?;
        if split_tokens.is_empty() {
            return None;
        }
        let mut expanded = Vec::new();
        expanded.extend_from_slice(&tokens[..index]);
        expanded.extend(split_tokens);
        expanded.extend_from_slice(&tokens[next_index..]);
        return Some(expanded);
    }

    None
}

// env recognizes both options and NAME=VALUE environment assignments
// interleaved before the wrapped command. The `-` short token (read from
// stdin) is treated as a long_flag for simplicity.
const ENV_WRAPPER_SPEC: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["-i", "-", "--ignore-environment", "-0", "--null"],
    long_options_with_value: &["--argv0", "--chdir", "--unset"],
    short_options_with_value: &["-a", "-C", "-u"],
    inline_for_all_long_options: true,
    short_inline_value_chars: "u",
    accept_env_assignment_tokens: true,
    ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("env"))
};

pub(in crate::permission_rules) fn env_wrapper_prefix_width(tokens: &[String]) -> Option<usize> {
    wrapper_spec::wrapper_prefix_width(tokens, &ENV_WRAPPER_SPEC)
}

pub(in crate::permission_rules) const COMMAND_WRAPPER_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["-p"],
        // `command` only — `builtin` has no options and is handled separately
        // in strip_one_bash_wrapper; `exec` has its own option list and gets
        // EXEC_WRAPPER_SPEC.
        ..wrapper_spec::WrapperSpec::with_aliases(&["command"])
    };

pub(in crate::permission_rules) const EXEC_WRAPPER_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["-c", "-l"],
        short_options_with_value: &["-a"],
        ..wrapper_spec::WrapperSpec::with_aliases(&["exec"])
    };
