//! Node.js ecosystem package runner command body extraction:
//! npm/npx, pnpm/pnpx, yarn, bun/bunx.

use super::super::wrapper_spec;

// ─── npm / npx ───────────────────────────────────────────────────────────────

pub(in crate::permission_rules) fn npm_exec_command_string_body(
    tokens: &[String],
) -> Option<String> {
    let mut index = npm_exec_body_start(tokens)?;
    while let Some(token) = tokens.get(index) {
        if matches!(token.as_str(), "-c" | "--call") {
            return tokens
                .get(index + 1)
                .cloned()
                .filter(|body| !body.trim().is_empty());
        }
        if let Some(value) = npm_exec_inline_call_value(token) {
            return (!value.trim().is_empty()).then(|| value.to_string());
        }
        if token == "--" {
            return None;
        } else if npm_exec_flag_option(token) {
            index += 1;
        } else if npm_exec_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if npm_exec_inline_value_option(token) {
            index += 1;
        } else {
            return None;
        }
    }

    None
}

pub(in crate::permission_rules) fn npm_exec_argv_command_body(tokens: &[String]) -> Option<String> {
    let start = npm_exec_body_start(tokens)?;
    let index = wrapper_spec::scan_options_block(tokens, start, &NPM_EXEC_OPTIONS)?;
    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

const NPM_EXEC_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["--workspaces", "--include-workspace-root", "--yes", "-y"],
    long_options_with_value: &["--package", "-p", "--workspace", "-w"],
    inline_value_long_prefixes: &["--package", "--workspace"],
    short_inline_value_chars: "pw",
    forbidden_long: &["--call"],
    forbidden_inline_prefixes: &["--call"],
    forbidden_short_inline_chars: "c",
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn npm_exec_body_start(tokens: &[String]) -> Option<usize> {
    let first = tokens.first()?;
    let mut index = 1usize;
    if !is_npx_command_token(first) {
        if !is_npm_command_token(first) {
            return None;
        }
        if !tokens
            .get(index)
            .is_some_and(|token| matches!(token.as_str(), "exec" | "x"))
        {
            return None;
        }
        index += 1;
    }
    Some(index)
}

fn is_npm_command_token(token: &str) -> bool {
    matches!(
        token,
        "npm" | "/usr/bin/npm" | "/usr/local/bin/npm" | "/opt/homebrew/bin/npm"
    )
}

fn is_npx_command_token(token: &str) -> bool {
    matches!(
        token,
        "npx" | "/usr/bin/npx" | "/usr/local/bin/npx" | "/opt/homebrew/bin/npx"
    )
}

fn npm_exec_inline_call_value(token: &str) -> Option<&str> {
    token
        .strip_prefix("--call=")
        .or_else(|| token.strip_prefix("-c").filter(|value| !value.is_empty()))
}

fn npm_exec_flag_option(token: &str) -> bool {
    matches!(
        token,
        "--workspaces" | "--include-workspace-root" | "--yes" | "-y"
    )
}

fn npm_exec_option_takes_value(token: &str) -> bool {
    matches!(token, "--package" | "-p" | "--workspace" | "-w")
}

fn npm_exec_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--package=")
        .or_else(|| token.strip_prefix("-p").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--workspace="))
        .or_else(|| token.strip_prefix("-w").filter(|value| !value.is_empty()))
        .is_some_and(|value| !value.is_empty())
}

// ─── pnpm / pnpx ────────────────────────────────────────────────────────────

pub(in crate::permission_rules) fn pnpm_exec_argv_command_body(
    tokens: &[String],
) -> Option<String> {
    let start = pnpm_exec_body_start(tokens)?;
    let index = wrapper_spec::scan_options_block(tokens, start, &PNPM_EXEC_OPTIONS)?;
    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

const PNPM_EXEC_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--recursive",
        "-r",
        "--parallel",
        "--stream",
        "--aggregate-output",
        "--shell-mode",
        "-c",
    ],
    long_options_with_value: &[
        "--package",
        "--workspace",
        "-w",
        "--filter",
        "-F",
        "--dir",
        "-C",
    ],
    inline_value_long_prefixes: &["--package", "--workspace", "--filter", "--dir"],
    short_inline_value_chars: "wFC",
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn pnpm_exec_body_start(tokens: &[String]) -> Option<usize> {
    let first = tokens.first()?;
    if is_pnpx_command_token(first) {
        return Some(1);
    }
    if !is_pnpm_command_token(first) {
        return None;
    }
    let index = wrapper_spec::scan_options_block(tokens, 1, &PNPM_GLOBAL_OPTIONS)?;
    if tokens
        .get(index)
        .is_some_and(|token| matches!(token.as_str(), "exec" | "dlx"))
    {
        Some(index + 1)
    } else {
        None
    }
}

const PNPM_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--recursive",
        "-r",
        "--workspace-root",
        "-w",
        "--help",
        "-h",
        "--version",
        "-v",
    ],
    long_options_with_value: &["--dir", "-C", "--filter", "-F"],
    inline_value_long_prefixes: &["--dir", "--filter"],
    short_inline_value_chars: "CF",
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn is_pnpm_command_token(token: &str) -> bool {
    matches!(
        token,
        "pnpm" | "/usr/bin/pnpm" | "/usr/local/bin/pnpm" | "/opt/homebrew/bin/pnpm"
    )
}

fn is_pnpx_command_token(token: &str) -> bool {
    matches!(
        token,
        "pnpx" | "/usr/bin/pnpx" | "/usr/local/bin/pnpx" | "/opt/homebrew/bin/pnpx"
    )
}

// ─── yarn ────────────────────────────────────────────────────────────────────

pub(in crate::permission_rules) fn yarn_exec_argv_command_body(
    tokens: &[String],
) -> Option<String> {
    let start = yarn_exec_body_start(tokens)?;
    let index = wrapper_spec::scan_options_block(tokens, start, &YARN_EXEC_OPTIONS)?;
    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

const YARN_EXEC_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["--quiet", "-q", "--interactive", "-i"],
    long_options_with_value: &["--package", "-p"],
    inline_value_long_prefixes: &["--package"],
    short_inline_value_chars: "p",
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn yarn_exec_body_start(tokens: &[String]) -> Option<usize> {
    let first = tokens.first()?;
    if !is_yarn_command_token(first) {
        return None;
    }
    let index = wrapper_spec::scan_options_block(tokens, 1, &YARN_GLOBAL_OPTIONS)?;
    if tokens
        .get(index)
        .is_some_and(|token| matches!(token.as_str(), "exec" | "dlx"))
    {
        Some(index + 1)
    } else {
        None
    }
}

const YARN_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--help",
        "-h",
        "--version",
        "-v",
        "--verbose",
        "--silent",
        "-s",
    ],
    long_options_with_value: &["--cwd"],
    inline_for_all_long_options: true,
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn is_yarn_command_token(token: &str) -> bool {
    matches!(
        token,
        "yarn" | "/usr/bin/yarn" | "/usr/local/bin/yarn" | "/opt/homebrew/bin/yarn"
    )
}

// ─── bun / bunx ──────────────────────────────────────────────────────────────

pub(in crate::permission_rules) fn bun_exec_argv_command_body(tokens: &[String]) -> Option<String> {
    let start = bun_exec_body_start(tokens)?;
    let index = wrapper_spec::scan_options_block(tokens, start, &BUN_EXEC_OPTIONS)?;
    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

const BUN_EXEC_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["--bun"],
    long_options_with_value: &["--package", "-p"],
    inline_value_long_prefixes: &["--package"],
    short_inline_value_chars: "p",
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn bun_exec_body_start(tokens: &[String]) -> Option<usize> {
    let first = tokens.first()?;
    if is_bunx_command_token(first) {
        return Some(1);
    }
    if !is_bun_command_token(first) {
        return None;
    }
    let index = wrapper_spec::scan_options_block(tokens, 1, &BUN_GLOBAL_OPTIONS)?;
    if tokens.get(index).is_some_and(|token| token == "x") {
        Some(index + 1)
    } else {
        None
    }
}

const BUN_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["--bun", "--silent"],
    long_options_with_value: &["--cwd", "-C"],
    inline_value_long_prefixes: &["--cwd"],
    short_inline_value_chars: "C",
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn is_bun_command_token(token: &str) -> bool {
    matches!(
        token,
        "bun" | "/usr/bin/bun" | "/usr/local/bin/bun" | "/opt/homebrew/bin/bun"
    )
}

fn is_bunx_command_token(token: &str) -> bool {
    matches!(
        token,
        "bunx" | "/usr/bin/bunx" | "/usr/local/bin/bunx" | "/opt/homebrew/bin/bunx"
    )
}
