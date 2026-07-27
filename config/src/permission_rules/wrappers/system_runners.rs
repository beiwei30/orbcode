//! System/language ecosystem package runner command body extraction:
//! python (poetry/pipenv/uv/pdm/hatch), conda/mamba, ruby (bundle/rbenv/mise).

use super::super::wrapper_spec;

// ─── python project runners ─────────────────────────────────────────────────

pub(in crate::permission_rules) fn python_project_runner_argv_command_body(
    tokens: &[String],
) -> Option<String> {
    let start = python_project_runner_body_start(tokens)?;
    let index = wrapper_spec::scan_options_block(tokens, start, &PYTHON_PROJECT_RUNNER_OPTIONS)?;
    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

const PYTHON_PROJECT_RUNNER_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--isolated",
        "--no-sync",
        "--locked",
        "--frozen",
        "--active",
        "--all-extras",
        "--no-dev",
        "--with-editable",
    ],
    long_options_with_value: &[
        "--python",
        "-p",
        "--py",
        "--env",
        "-e",
        "--with",
        "--with-requirements",
        "--extra",
        "--group",
    ],
    inline_value_long_prefixes: &[
        "--python",
        "--py",
        "--env",
        "--with",
        "--with-requirements",
        "--extra",
        "--group",
    ],
    short_inline_value_chars: "pe",
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn python_project_runner_body_start(tokens: &[String]) -> Option<usize> {
    let first = tokens.first()?;
    if is_uvx_command_token(first) {
        return Some(1);
    }

    let mut index = 1usize;
    if is_poetry_command_token(first) {
        index = skip_poetry_global_options(tokens, index)?;
        tokens
            .get(index)
            .is_some_and(|token| token == "run")
            .then_some(index + 1)
    } else if is_pipenv_command_token(first) {
        index = skip_pipenv_global_options(tokens, index)?;
        tokens
            .get(index)
            .is_some_and(|token| token == "run")
            .then_some(index + 1)
    } else if is_uv_command_token(first) {
        index = skip_uv_global_options(tokens, index)?;
        tokens
            .get(index)
            .is_some_and(|token| token == "run")
            .then_some(index + 1)
    } else if is_pdm_command_token(first) {
        index = skip_pdm_global_options(tokens, index)?;
        tokens
            .get(index)
            .is_some_and(|token| token == "run")
            .then_some(index + 1)
    } else if is_hatch_command_token(first) {
        index = skip_hatch_global_options(tokens, index)?;
        tokens
            .get(index)
            .is_some_and(|token| token == "run")
            .then_some(index + 1)
    } else {
        None
    }
}

fn skip_poetry_global_options(tokens: &[String], index: usize) -> Option<usize> {
    wrapper_spec::scan_options_block(tokens, index, &POETRY_GLOBAL_OPTIONS)
}

fn skip_pipenv_global_options(tokens: &[String], index: usize) -> Option<usize> {
    wrapper_spec::scan_options_block(tokens, index, &PIPENV_GLOBAL_OPTIONS)
}

fn skip_uv_global_options(tokens: &[String], index: usize) -> Option<usize> {
    wrapper_spec::scan_options_block(tokens, index, &UV_GLOBAL_OPTIONS)
}

fn skip_pdm_global_options(tokens: &[String], index: usize) -> Option<usize> {
    wrapper_spec::scan_options_block(tokens, index, &PDM_GLOBAL_OPTIONS)
}

fn skip_hatch_global_options(tokens: &[String], index: usize) -> Option<usize> {
    wrapper_spec::scan_options_block(tokens, index, &HATCH_GLOBAL_OPTIONS)
}

fn is_poetry_command_token(token: &str) -> bool {
    matches!(
        token,
        "poetry" | "/usr/bin/poetry" | "/usr/local/bin/poetry" | "/opt/homebrew/bin/poetry"
    )
}

fn is_pipenv_command_token(token: &str) -> bool {
    matches!(
        token,
        "pipenv" | "/usr/bin/pipenv" | "/usr/local/bin/pipenv" | "/opt/homebrew/bin/pipenv"
    )
}

fn is_uv_command_token(token: &str) -> bool {
    matches!(
        token,
        "uv" | "/usr/bin/uv" | "/usr/local/bin/uv" | "/opt/homebrew/bin/uv"
    )
}

fn is_uvx_command_token(token: &str) -> bool {
    matches!(
        token,
        "uvx" | "/usr/bin/uvx" | "/usr/local/bin/uvx" | "/opt/homebrew/bin/uvx"
    )
}

fn is_pdm_command_token(token: &str) -> bool {
    matches!(
        token,
        "pdm" | "/usr/bin/pdm" | "/usr/local/bin/pdm" | "/opt/homebrew/bin/pdm"
    )
}

fn is_hatch_command_token(token: &str) -> bool {
    matches!(
        token,
        "hatch" | "/usr/bin/hatch" | "/usr/local/bin/hatch" | "/opt/homebrew/bin/hatch"
    )
}

const POETRY_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--help",
        "-h",
        "--version",
        "-V",
        "--quiet",
        "-q",
        "--no-interaction",
        "-n",
    ],
    long_options_with_value: &["--directory", "-C", "--project", "-P"],
    inline_value_long_prefixes: &["--directory", "--project"],
    short_inline_value_chars: "CP",
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

const PIPENV_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--help",
        "-h",
        "--version",
        "--bare",
        "--clear",
        "--site-packages",
        "--three",
        "--two",
        "--verbose",
        "--quiet",
        "--where",
        "--venv",
        "--py",
    ],
    long_options_with_value: &["--python", "--pypi-mirror"],
    inline_for_all_long_options: true,
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

const UV_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--help",
        "-h",
        "--version",
        "-V",
        "--quiet",
        "-q",
        "--verbose",
        "-v",
    ],
    long_options_with_value: &["--directory", "--project", "--config-file"],
    inline_for_all_long_options: true,
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

const PDM_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["--verbose", "-v", "--quiet", "-q", "--pep582"],
    long_options_with_value: &["--project", "-p", "--config", "-c"],
    inline_value_long_prefixes: &["--project", "--config"],
    short_inline_value_chars: "pc",
    forbidden_long: &["--help", "-h", "--version", "-V"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

const HATCH_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["--verbose", "-v", "--quiet", "-q", "--no-color"],
    long_options_with_value: &["--config"],
    inline_for_all_long_options: true,
    forbidden_long: &["--help", "-h", "--version", "-V"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

// ─── conda / mamba / micromamba ─────────────────────────────────────────────

pub(in crate::permission_rules) fn conda_run_argv_command_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_conda_command_token(token))
    {
        return None;
    }

    let mut index = wrapper_spec::scan_options_block(tokens, 1, &CONDA_GLOBAL_OPTIONS)?;

    if tokens.get(index).is_none_or(|token| token != "run") {
        return None;
    }
    index += 1;

    let index = wrapper_spec::scan_options_block(tokens, index, &CONDA_RUN_OPTIONS)?;
    tokens.get(index..).and_then(join_non_empty_tokens)
}

fn is_conda_command_token(token: &str) -> bool {
    matches!(
        token,
        "conda"
            | "mamba"
            | "micromamba"
            | "/usr/bin/conda"
            | "/usr/bin/mamba"
            | "/usr/bin/micromamba"
            | "/usr/local/bin/conda"
            | "/usr/local/bin/mamba"
            | "/usr/local/bin/micromamba"
            | "/opt/homebrew/bin/conda"
            | "/opt/homebrew/bin/mamba"
            | "/opt/homebrew/bin/micromamba"
    )
}

const CONDA_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--debug",
        "--json",
        "--verbose",
        "-v",
        "--quiet",
        "-q",
        "--no-plugins",
        "--offline",
    ],
    long_options_with_value: &["--config", "--rc-file"],
    inline_for_all_long_options: true,
    forbidden_long: &["--help", "-h", "--version", "-V"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

const CONDA_RUN_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--dev",
        "--debug-wrapper-scripts",
        "--no-capture-output",
        "--live-stream",
    ],
    long_options_with_value: &["--name", "-n", "--prefix", "-p", "--cwd"],
    inline_value_long_prefixes: &["--name", "--prefix", "--cwd"],
    short_inline_value_chars: "np",
    forbidden_long: &["--help", "-h"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

// ─── ruby project runners ───────────────────────────────────────────────────

pub(in crate::permission_rules) fn ruby_project_runner_argv_command_body(
    tokens: &[String],
) -> Option<String> {
    let start = ruby_project_runner_body_start(tokens)?;
    let index = wrapper_spec::scan_options_block(tokens, start, &RUBY_PROJECT_RUNNER_OPTIONS)?;
    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

const RUBY_PROJECT_RUNNER_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["--keep-file-descriptors"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn ruby_project_runner_body_start(tokens: &[String]) -> Option<usize> {
    let first = tokens.first()?;
    let mut index = 1usize;
    if is_bundle_command_token(first) {
        index = skip_bundle_global_options(tokens, index)?;
        tokens
            .get(index)
            .is_some_and(|token| token == "exec")
            .then_some(index + 1)
    } else if is_version_manager_command_token(first) {
        tokens
            .get(index)
            .is_some_and(|token| token == "exec")
            .then_some(index + 1)
    } else {
        None
    }
}

fn skip_bundle_global_options(tokens: &[String], index: usize) -> Option<usize> {
    wrapper_spec::scan_options_block(tokens, index, &BUNDLE_GLOBAL_OPTIONS)
}

const BUNDLE_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["--verbose", "--quiet", "--no-color"],
    long_options_with_value: &["--gemfile"],
    inline_for_all_long_options: true,
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn is_bundle_command_token(token: &str) -> bool {
    matches!(
        token,
        "bundle"
            | "bundler"
            | "/usr/bin/bundle"
            | "/usr/bin/bundler"
            | "/usr/local/bin/bundle"
            | "/usr/local/bin/bundler"
            | "/opt/homebrew/bin/bundle"
            | "/opt/homebrew/bin/bundler"
    )
}

fn is_version_manager_command_token(token: &str) -> bool {
    matches!(
        token,
        "rbenv"
            | "pyenv"
            | "nodenv"
            | "asdf"
            | "mise"
            | "/usr/bin/rbenv"
            | "/usr/bin/pyenv"
            | "/usr/bin/nodenv"
            | "/usr/bin/asdf"
            | "/usr/bin/mise"
            | "/usr/local/bin/rbenv"
            | "/usr/local/bin/pyenv"
            | "/usr/local/bin/nodenv"
            | "/usr/local/bin/asdf"
            | "/usr/local/bin/mise"
            | "/opt/homebrew/bin/rbenv"
            | "/opt/homebrew/bin/pyenv"
            | "/opt/homebrew/bin/nodenv"
            | "/opt/homebrew/bin/asdf"
            | "/opt/homebrew/bin/mise"
    )
}

fn join_non_empty_tokens(tokens: &[String]) -> Option<String> {
    (!tokens.is_empty())
        .then(|| tokens.join(" "))
        .filter(|body| !body.trim().is_empty())
}
