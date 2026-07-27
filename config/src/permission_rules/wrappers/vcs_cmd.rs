use super::super::wrapper_spec;
use super::pager::{git_command_invokes_pager_from_tokens, push_git_pager_config};

pub(in crate::permission_rules) fn git_env_config_command_string_bodies(
    configs: &[String],
    tokens: &[String],
) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return Vec::new();
    }

    let Some(index) = git_command_start_index(tokens) else {
        return Vec::new();
    };
    let Some(subcommand) = tokens.get(index) else {
        return Vec::new();
    };
    let args = tokens.get(index + 1..).unwrap_or_default();

    let mut aliases = Vec::new();
    let mut askpasses = Vec::new();
    let mut core_editors = Vec::new();
    let mut sequence_editors = Vec::new();
    let mut external_diffs = Vec::new();
    let mut helpers = Vec::new();
    let mut pagers = Vec::new();
    let mut ssh_commands = Vec::new();
    let mut tool_commands = Vec::new();
    for config in configs {
        push_git_shell_alias_config(config, &mut aliases);
        push_git_askpass_config(config, &mut askpasses);
        push_git_editor_config(config, &mut core_editors, &mut sequence_editors);
        push_git_external_diff_config(config, &mut external_diffs);
        push_git_credential_helper_config(config, &mut helpers);
        push_git_pager_config(config, &mut pagers);
        push_git_ssh_command_config(config, &mut ssh_commands);
        push_git_tool_command_config(config, &mut tool_commands);
    }

    let mut bodies = Vec::new();
    if git_uses_ssh_command_config(subcommand)
        && let Some(body) = ssh_commands.last()
    {
        push_unique_command_body(body, &mut bodies);
    }
    if git_command_may_invoke_askpass(subcommand)
        && let Some(body) = askpasses.last()
    {
        push_unique_command_body(body, &mut bodies);
    }
    if git_command_invokes_external_diff(subcommand, args)
        && let Some(body) = external_diffs.last()
    {
        push_unique_command_body(body, &mut bodies);
    }
    if git_command_invokes_editor(subcommand, args) {
        let body = match subcommand.as_str() {
            "rebase" => sequence_editors.last().or_else(|| core_editors.last()),
            _ => core_editors.last(),
        };
        if let Some(body) = body {
            push_unique_command_body(body, &mut bodies);
        }
    }
    if git_command_invokes_pager_from_tokens(tokens)
        && let Some(body) = pagers.last()
    {
        push_unique_command_body(body, &mut bodies);
    }
    if git_invokes_credential_helper(subcommand) {
        for body in helpers {
            push_unique_command_body(&body, &mut bodies);
        }
    }
    if matches!(subcommand.as_str(), "difftool" | "mergetool")
        && let Some(tool) = git_selected_tool_name(args)
        && let Some((_, _, body)) = tool_commands
            .iter()
            .rev()
            .find(|(kind, name, _)| kind == subcommand && name == &tool)
    {
        push_unique_command_body(body, &mut bodies);
    }
    if let Some((_, body)) = aliases
        .iter()
        .rev()
        .find(|(name, _)| Some(name) == tokens.get(index))
        && let Some(body) = git_shell_alias_body_with_args(body, args)
    {
        push_unique_command_body(&body, &mut bodies);
    }

    bodies
}

fn git_shell_alias_body_with_args(body: &str, args: &[String]) -> Option<String> {
    let mut body = body.strip_prefix('!')?.trim().to_string();
    if body.is_empty() {
        return None;
    }
    if !args.is_empty() {
        body.push(' ');
        body.push_str(&args.join(" "));
    }
    Some(body)
}

fn push_unique_command_body(body: &str, bodies: &mut Vec<String>) {
    let body = body.trim();
    if !body.is_empty() && !bodies.iter().any(|existing| existing == body) {
        bodies.push(body.to_string());
    }
}

pub(in crate::permission_rules) fn svn_command_invokes_editor_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_svn_command_token(token))
    {
        return false;
    }

    let Some(index) = svn_subcommand_index(tokens) else {
        return false;
    };
    let Some(subcommand) = tokens.get(index).map(String::as_str) else {
        return false;
    };
    let args = tokens.get(index + 1..).unwrap_or_default();
    match subcommand {
        "commit" | "ci" => svn_commit_invokes_editor(args),
        "propedit" | "pedit" | "pe" => svn_propedit_invokes_editor(args),
        _ => false,
    }
}

pub(in crate::permission_rules) fn is_svn_command_token(token: &str) -> bool {
    matches!(
        token,
        "svn" | "/usr/bin/svn" | "/usr/local/bin/svn" | "/opt/homebrew/bin/svn"
    )
}

fn svn_subcommand_index(tokens: &[String]) -> Option<usize> {
    let index = wrapper_spec::scan_options_block(tokens, 1, &SVN_GLOBAL_OPTIONS)?;
    tokens.get(index).map(|_| index)
}

const SVN_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--quiet",
        "-q",
        "--non-interactive",
        "--trust-server-cert",
        "--trust-server-cert-failures",
        "--no-auth-cache",
    ],
    long_options_with_value: &[
        "--username",
        "--password",
        "--config-dir",
        "--config-option",
        "--auth-type",
        "--editor-cmd",
        "--encoding",
    ],
    inline_for_all_long_options: true,
    forbidden_long: &["--help", "-h", "--version"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn svn_commit_invokes_editor(args: &[String]) -> bool {
    let mut has_target = false;
    let mut index = 0usize;
    while let Some(token) = args.get(index) {
        if token == "--" {
            return !svn_tokens_after(args, index).is_empty();
        } else if matches!(token.as_str(), "--help" | "-h") {
            return false;
        } else if svn_message_option_takes_value(token) {
            return false;
        } else if svn_message_inline_value_option(token) {
            return false;
        } else if svn_commit_flag_option(token) {
            index += 1;
        } else if svn_commit_option_takes_value(token) {
            if args.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if svn_commit_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            has_target = true;
            index += 1;
        }
    }

    has_target
}

fn svn_propedit_invokes_editor(args: &[String]) -> bool {
    let mut positional_count = 0usize;
    let mut index = 0usize;
    while let Some(token) = args.get(index) {
        if token == "--" {
            positional_count += svn_tokens_after(args, index).len();
            break;
        } else if matches!(token.as_str(), "--help" | "-h") {
            return false;
        } else if svn_propedit_flag_option(token) {
            index += 1;
        } else if svn_propedit_option_takes_value(token) {
            if args.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if svn_propedit_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            positional_count += 1;
            index += 1;
        }
    }

    positional_count >= 1
}

fn svn_tokens_after(tokens: &[String], index: usize) -> &[String] {
    tokens.get(index + 1..).unwrap_or_default()
}

fn svn_message_option_takes_value(token: &str) -> bool {
    matches!(token, "--message" | "-m" | "--file" | "-F")
}

fn svn_message_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--message=")
        .or_else(|| token.strip_prefix("-m").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--file="))
        .or_else(|| token.strip_prefix("-F").filter(|value| !value.is_empty()))
        .is_some_and(|value| !value.is_empty())
}

fn svn_commit_flag_option(token: &str) -> bool {
    matches!(
        token,
        "--keep-locks" | "--no-unlock" | "--force-log" | "--include-externals"
    )
}

fn svn_commit_option_takes_value(token: &str) -> bool {
    matches!(token, "--depth" | "--changelist")
}

fn svn_commit_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--depth=")
        .or_else(|| token.strip_prefix("--changelist="))
        .is_some_and(|value| !value.is_empty())
}

fn svn_propedit_flag_option(token: &str) -> bool {
    matches!(token, "--revprop")
}

fn svn_propedit_option_takes_value(token: &str) -> bool {
    matches!(token, "--revision" | "-r" | "--changelist")
}

fn svn_propedit_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--revision=")
        .or_else(|| token.strip_prefix("-r").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--changelist="))
        .is_some_and(|value| !value.is_empty())
}

pub(in crate::permission_rules) fn hg_command_invokes_editor_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_hg_command_token(token))
    {
        return false;
    }

    let Some(index) = hg_subcommand_index(tokens) else {
        return false;
    };
    let Some(subcommand) = tokens.get(index).map(String::as_str) else {
        return false;
    };
    let args = tokens.get(index + 1..).unwrap_or_default();
    hg_command_invokes_editor(subcommand, args)
}

fn is_hg_command_token(token: &str) -> bool {
    matches!(
        token,
        "hg" | "/usr/bin/hg" | "/usr/local/bin/hg" | "/opt/homebrew/bin/hg"
    )
}

fn hg_subcommand_index(tokens: &[String]) -> Option<usize> {
    let index = wrapper_spec::scan_options_block(tokens, 1, &HG_GLOBAL_OPTIONS)?;
    tokens.get(index).map(|_| index)
}

const HG_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--quiet",
        "-q",
        "--verbose",
        "-v",
        "--debug",
        "--traceback",
        "--noninteractive",
    ],
    long_options_with_value: &[
        "--repository",
        "-R",
        "--cwd",
        "--config",
        "--encoding",
        "--encodingmode",
        "--time",
    ],
    inline_value_long_prefixes: &[
        "--repository",
        "--cwd",
        "--config",
        "--encoding",
        "--encodingmode",
        "--time",
    ],
    short_inline_value_chars: "R",
    forbidden_long: &["--help", "-h", "--version"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

pub(in crate::permission_rules) fn hg_editor_command_config_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_hg_command_token(token))
    {
        return None;
    }

    let mut editors = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return None;
        } else if matches!(token.as_str(), "--help" | "-h" | "--version") {
            return None;
        } else if token == "--config" {
            let config = tokens.get(index + 1)?;
            push_hg_editor_config(config, &mut editors);
            index += 2;
        } else if let Some(config) = token.strip_prefix("--config=") {
            push_hg_editor_config(config, &mut editors);
            index += 1;
        } else if hg_global_flag_option(token) {
            index += 1;
        } else if hg_global_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if hg_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    let subcommand = tokens.get(index)?;
    let args = tokens.get(index + 1..).unwrap_or_default();
    if hg_command_invokes_editor(subcommand, args) {
        editors.last().cloned()
    } else {
        None
    }
}

fn push_hg_editor_config(config: &str, editors: &mut Vec<String>) {
    let Some((name, value)) = config.split_once('=') else {
        return;
    };
    let value = value.trim();
    if !value.is_empty() && name.trim().eq_ignore_ascii_case("ui.editor") {
        editors.push(value.to_string());
    }
}

pub(in crate::permission_rules) fn hg_ssh_command_config_body(tokens: &[String]) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_hg_command_token(token))
    {
        return None;
    }

    let mut ssh_commands = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return None;
        } else if matches!(token.as_str(), "--help" | "-h" | "--version") {
            return None;
        } else if token == "--config" {
            let config = tokens.get(index + 1)?;
            push_hg_ssh_config(config, &mut ssh_commands);
            index += 2;
        } else if let Some(config) = token.strip_prefix("--config=") {
            push_hg_ssh_config(config, &mut ssh_commands);
            index += 1;
        } else if hg_global_flag_option(token) {
            index += 1;
        } else if hg_global_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if hg_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    let subcommand = tokens.get(index)?;
    let args = tokens.get(index + 1..).unwrap_or_default();
    if !hg_subcommand_may_use_ssh_command(subcommand) || hg_args_have_help(args) {
        return None;
    }
    hg_ssh_option_body(args).or_else(|| ssh_commands.last().cloned())
}

fn push_hg_ssh_config(config: &str, ssh_commands: &mut Vec<String>) {
    let Some((name, value)) = config.split_once('=') else {
        return;
    };
    let value = value.trim();
    if !value.is_empty() && name.trim().eq_ignore_ascii_case("ui.ssh") {
        ssh_commands.push(value.to_string());
    }
}

fn hg_subcommand_may_use_ssh_command(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "clone" | "pull" | "push" | "incoming" | "outgoing" | "in" | "out" | "identify" | "id"
    )
}

fn hg_args_have_help(args: &[String]) -> bool {
    args.iter()
        .take_while(|token| token.as_str() != "--")
        .any(|token| matches!(token.as_str(), "--help" | "-h"))
}

fn hg_ssh_option_body(args: &[String]) -> Option<String> {
    let mut index = 0usize;
    while let Some(token) = args.get(index) {
        if token == "--" {
            return None;
        } else if token == "--ssh" {
            return args
                .get(index + 1)
                .map(|body| body.trim())
                .filter(|body| !body.is_empty())
                .map(str::to_string);
        } else if let Some(body) = token.strip_prefix("--ssh=") {
            return (!body.trim().is_empty()).then(|| body.trim().to_string());
        }
        index += 1;
    }

    None
}

fn hg_command_invokes_editor(subcommand: &str, args: &[String]) -> bool {
    match subcommand {
        "commit" | "ci" => hg_commit_invokes_editor(args),
        "histedit" => hg_histedit_invokes_editor(args),
        _ => false,
    }
}

fn hg_global_flag_option(token: &str) -> bool {
    matches!(
        token,
        "--quiet" | "-q" | "--verbose" | "-v" | "--debug" | "--traceback" | "--noninteractive"
    )
}

fn hg_global_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "--repository" | "-R" | "--cwd" | "--config" | "--encoding" | "--encodingmode" | "--time"
    )
}

fn hg_global_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--repository=")
        .or_else(|| token.strip_prefix("-R").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--cwd="))
        .or_else(|| token.strip_prefix("--config="))
        .or_else(|| token.strip_prefix("--encoding="))
        .or_else(|| token.strip_prefix("--encodingmode="))
        .or_else(|| token.strip_prefix("--time="))
        .is_some_and(|value| !value.is_empty())
}

fn hg_commit_invokes_editor(args: &[String]) -> bool {
    let mut index = 0usize;
    while let Some(token) = args.get(index) {
        if token == "--" {
            return true;
        } else if matches!(token.as_str(), "--help" | "-h") {
            return false;
        } else if hg_commit_message_option_takes_value(token) {
            return false;
        } else if hg_commit_message_inline_value_option(token) {
            return false;
        } else if hg_commit_flag_option(token) {
            index += 1;
        } else if hg_commit_option_takes_value(token) {
            if args.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if hg_commit_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            index += 1;
        }
    }

    true
}

fn hg_commit_message_option_takes_value(token: &str) -> bool {
    matches!(token, "--message" | "-m" | "--logfile" | "-l")
}

fn hg_commit_message_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--message=")
        .or_else(|| token.strip_prefix("-m").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--logfile="))
        .or_else(|| token.strip_prefix("-l").filter(|value| !value.is_empty()))
        .is_some_and(|value| !value.is_empty())
}

fn hg_commit_flag_option(token: &str) -> bool {
    matches!(
        token,
        "--addremove" | "-A" | "--close-branch" | "--amend" | "--secret" | "--subrepos" | "-S"
    )
}

fn hg_commit_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "--date" | "-d" | "--user" | "-u" | "--include" | "-I" | "--exclude" | "-X"
    )
}

fn hg_commit_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--date=")
        .or_else(|| token.strip_prefix("-d").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--user="))
        .or_else(|| token.strip_prefix("-u").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--include="))
        .or_else(|| token.strip_prefix("-I").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--exclude="))
        .or_else(|| token.strip_prefix("-X").filter(|value| !value.is_empty()))
        .is_some_and(|value| !value.is_empty())
}

fn hg_histedit_invokes_editor(args: &[String]) -> bool {
    let mut has_target = false;
    let mut index = 0usize;
    while let Some(token) = args.get(index) {
        if token == "--" {
            return !hg_tokens_after(args, index).is_empty();
        } else if matches!(token.as_str(), "--help" | "-h" | "--continue" | "--abort") {
            return false;
        } else if token == "--edit-plan" {
            return true;
        } else if token == "--commands" {
            return false;
        } else if token.starts_with("--commands=") {
            return false;
        } else if hg_histedit_rev_option_takes_value(token) {
            if args.get(index + 1).is_none() {
                return false;
            }
            has_target = true;
            index += 2;
        } else if hg_histedit_rev_inline_value_option(token) {
            has_target = true;
            index += 1;
        } else if hg_histedit_flag_option(token) {
            index += 1;
        } else if hg_histedit_option_takes_value(token) {
            if args.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if hg_histedit_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            has_target = true;
            index += 1;
        }
    }

    has_target
}

fn hg_tokens_after(tokens: &[String], index: usize) -> &[String] {
    tokens.get(index + 1..).unwrap_or_default()
}

fn hg_histedit_flag_option(token: &str) -> bool {
    matches!(token, "--keep" | "--force")
}

fn hg_histedit_rev_option_takes_value(token: &str) -> bool {
    matches!(token, "--rev" | "-r")
}

fn hg_histedit_rev_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--rev=")
        .or_else(|| token.strip_prefix("-r").filter(|value| !value.is_empty()))
        .is_some_and(|value| !value.is_empty())
}

fn hg_histedit_option_takes_value(token: &str) -> bool {
    matches!(token, "--outgoing")
}

fn hg_histedit_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--outgoing=")
        .is_some_and(|value| !value.is_empty())
}

pub(in crate::permission_rules) fn git_bisect_run_command_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return None;
    }

    let mut index = git_command_start_index(tokens)?;
    if tokens.get(index).is_none_or(|token| token != "bisect") {
        return None;
    }
    index += 1;

    if tokens.get(index).is_none_or(|token| token != "run") {
        return None;
    }
    index += 1;

    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

pub(in crate::permission_rules) fn git_submodule_foreach_command_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return None;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if git_global_flag_option(token) {
            index += 1;
        } else if git_global_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if git_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    if tokens.get(index).is_none_or(|token| token != "submodule") {
        return None;
    }
    index += 1;

    while tokens
        .get(index)
        .is_some_and(|token| matches!(token.as_str(), "-q" | "--quiet"))
    {
        index += 1;
    }

    if tokens.get(index).is_none_or(|token| token != "foreach") {
        return None;
    }
    index += 1;

    if tokens
        .get(index)
        .is_some_and(|token| token == "--recursive")
    {
        index += 1;
    }

    tokens
        .get(index)
        .cloned()
        .filter(|body| !body.trim().is_empty())
}

pub(in crate::permission_rules) fn git_shell_alias_command_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return None;
    }

    let mut aliases = Vec::<(String, String)>::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if git_global_flag_option(token) {
            index += 1;
        } else if token == "-c" {
            let config = tokens.get(index + 1)?;
            push_git_shell_alias_config(config, &mut aliases);
            index += 2;
        } else if let Some(config) = token.strip_prefix("-c").filter(|config| !config.is_empty()) {
            push_git_shell_alias_config(config, &mut aliases);
            index += 1;
        } else if git_global_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if git_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    let alias_name = tokens.get(index)?;
    let (_, body) = aliases.iter().rev().find(|(name, _)| name == alias_name)?;
    let mut body = body.strip_prefix('!')?.trim().to_string();
    if body.is_empty() {
        return None;
    }
    if let Some(args) = tokens.get(index + 1..)
        && !args.is_empty()
    {
        body.push(' ');
        body.push_str(&args.join(" "));
    }
    Some(body)
}

pub(in crate::permission_rules) fn git_ssh_command_config_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return None;
    }

    let mut ssh_commands = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return None;
        } else if git_global_flag_option(token) {
            index += 1;
        } else if token == "-c" {
            let config = tokens.get(index + 1)?;
            push_git_ssh_command_config(config, &mut ssh_commands);
            index += 2;
        } else if let Some(config) = token.strip_prefix("-c").filter(|config| !config.is_empty()) {
            push_git_ssh_command_config(config, &mut ssh_commands);
            index += 1;
        } else if git_global_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if git_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    if !tokens
        .get(index)
        .is_some_and(|subcommand| git_uses_ssh_command_config(subcommand))
    {
        return None;
    }

    ssh_commands.last().cloned()
}

fn push_git_ssh_command_config(config: &str, ssh_commands: &mut Vec<String>) {
    let Some((name, value)) = config.split_once('=') else {
        return;
    };
    if name.eq_ignore_ascii_case("core.sshCommand") && !value.trim().is_empty() {
        ssh_commands.push(value.trim().to_string());
    }
}

fn git_uses_ssh_command_config(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "clone" | "fetch" | "pull" | "push" | "ls-remote"
    )
}

pub(in crate::permission_rules) fn git_askpass_command_config_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return None;
    }

    let mut askpasses = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return None;
        } else if git_global_flag_option(token) {
            index += 1;
        } else if token == "-c" {
            let config = tokens.get(index + 1)?;
            push_git_askpass_config(config, &mut askpasses);
            index += 2;
        } else if let Some(config) = token.strip_prefix("-c").filter(|config| !config.is_empty()) {
            push_git_askpass_config(config, &mut askpasses);
            index += 1;
        } else if git_global_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if git_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    if tokens
        .get(index)
        .is_some_and(|subcommand| git_command_may_invoke_askpass(subcommand))
    {
        askpasses.last().cloned()
    } else {
        None
    }
}

fn push_git_askpass_config(config: &str, askpasses: &mut Vec<String>) {
    let Some((name, value)) = config.split_once('=') else {
        return;
    };
    let value = value.trim();
    if name.eq_ignore_ascii_case("core.askPass") && !value.is_empty() {
        askpasses.push(value.to_string());
    }
}

pub(in crate::permission_rules) fn git_command_invokes_askpass_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return false;
    }

    let Some(index) = git_command_start_index(tokens) else {
        return false;
    };
    tokens
        .get(index)
        .is_some_and(|subcommand| git_command_may_invoke_askpass(subcommand))
}

fn git_command_may_invoke_askpass(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "clone" | "fetch" | "pull" | "push" | "ls-remote" | "credential"
    )
}

pub(in crate::permission_rules) fn git_external_diff_command_config_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return None;
    }

    let mut external_diffs = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return None;
        } else if git_global_flag_option(token) {
            index += 1;
        } else if token == "-c" {
            let config = tokens.get(index + 1)?;
            push_git_external_diff_config(config, &mut external_diffs);
            index += 2;
        } else if let Some(config) = token.strip_prefix("-c").filter(|config| !config.is_empty()) {
            push_git_external_diff_config(config, &mut external_diffs);
            index += 1;
        } else if git_global_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if git_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    let subcommand = tokens.get(index)?;
    let args = tokens.get(index + 1..).unwrap_or_default();
    if git_command_invokes_external_diff(subcommand, args) {
        external_diffs.last().cloned()
    } else {
        None
    }
}

fn push_git_external_diff_config(config: &str, external_diffs: &mut Vec<String>) {
    let Some((name, value)) = config.split_once('=') else {
        return;
    };
    let value = value.trim();
    if name.eq_ignore_ascii_case("diff.external") && !value.is_empty() {
        external_diffs.push(value.to_string());
    }
}

pub(in crate::permission_rules) fn git_command_invokes_external_diff_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return false;
    }

    let Some(index) = git_command_start_index(tokens) else {
        return false;
    };
    let Some(subcommand) = tokens.get(index) else {
        return false;
    };
    git_command_invokes_external_diff(subcommand, tokens.get(index + 1..).unwrap_or_default())
}

fn git_command_invokes_external_diff(subcommand: &str, args: &[String]) -> bool {
    matches!(subcommand, "diff" | "show" | "log") && !git_args_disable_external_diff(args)
}

fn git_args_disable_external_diff(args: &[String]) -> bool {
    args.iter()
        .take_while(|token| token.as_str() != "--")
        .any(|token| token == "--no-ext-diff")
}

pub(in crate::permission_rules) fn git_editor_command_config_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return None;
    }

    let mut core_editors = Vec::new();
    let mut sequence_editors = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return None;
        } else if git_global_flag_option(token) {
            index += 1;
        } else if token == "-c" {
            let config = tokens.get(index + 1)?;
            push_git_editor_config(config, &mut core_editors, &mut sequence_editors);
            index += 2;
        } else if let Some(config) = token.strip_prefix("-c").filter(|config| !config.is_empty()) {
            push_git_editor_config(config, &mut core_editors, &mut sequence_editors);
            index += 1;
        } else if git_global_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if git_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    let subcommand = tokens.get(index)?;
    let args = tokens.get(index + 1..).unwrap_or_default();
    let git_command_invokes_editor = git_command_invokes_editor(subcommand, args);
    match subcommand.as_str() {
        "rebase" if git_command_invokes_editor => sequence_editors
            .last()
            .or_else(|| core_editors.last())
            .cloned(),
        _ if git_command_invokes_editor => core_editors.last().cloned(),
        _ => None,
    }
}

pub(in crate::permission_rules) fn git_command_invokes_editor_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return false;
    }

    let Some(index) = git_command_start_index(tokens) else {
        return false;
    };
    let Some(subcommand) = tokens.get(index) else {
        return false;
    };
    let args = tokens.get(index + 1..).unwrap_or_default();
    git_command_invokes_editor(subcommand, args)
}

pub(in crate::permission_rules) fn git_command_invokes_sequence_editor_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return false;
    }

    let Some(index) = git_command_start_index(tokens) else {
        return false;
    };
    tokens
        .get(index)
        .is_some_and(|subcommand| subcommand == "rebase")
        && git_rebase_invokes_sequence_editor(tokens.get(index + 1..).unwrap_or_default())
}

fn git_command_invokes_editor(subcommand: &str, args: &[String]) -> bool {
    match subcommand {
        "commit" => git_commit_invokes_editor(args),
        "tag" => git_tag_invokes_editor(args),
        "rebase" => git_rebase_invokes_sequence_editor(args),
        _ => false,
    }
}

fn push_git_editor_config(
    config: &str,
    core_editors: &mut Vec<String>,
    sequence_editors: &mut Vec<String>,
) {
    let Some((name, value)) = config.split_once('=') else {
        return;
    };
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    if name.eq_ignore_ascii_case("core.editor") {
        core_editors.push(value.to_string());
    } else if name.eq_ignore_ascii_case("sequence.editor") {
        sequence_editors.push(value.to_string());
    }
}

fn git_commit_invokes_editor(args: &[String]) -> bool {
    !git_args_have_non_editor_message_option(args)
}

fn git_tag_invokes_editor(args: &[String]) -> bool {
    let mut annotated = false;
    let mut index = 0usize;
    while let Some(token) = args.get(index) {
        if token == "--" {
            break;
        } else if matches!(token.as_str(), "-a" | "--annotate" | "-s" | "--sign") {
            annotated = true;
            index += 1;
        } else if git_message_option_takes_value(token) {
            return false;
        } else if git_message_inline_value_option(token) {
            return false;
        } else if token.starts_with('-') {
            index += 1;
        } else {
            index += 1;
        }
    }

    annotated
}

fn git_rebase_invokes_sequence_editor(args: &[String]) -> bool {
    args.iter().any(|token| {
        matches!(token.as_str(), "-i" | "--interactive")
            || token.starts_with("--interactive=")
            || git_short_option_group_contains(token, 'i')
    })
}

fn git_args_have_non_editor_message_option(args: &[String]) -> bool {
    args.iter().any(|token| {
        git_message_option_takes_value(token)
            || git_message_inline_value_option(token)
            || matches!(token.as_str(), "-C" | "--reuse-message")
    })
}

fn git_message_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-m" | "--message" | "-F" | "--file" | "-C" | "--reuse-message"
    )
}

fn git_message_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--message=")
        .or_else(|| token.strip_prefix("--file="))
        .or_else(|| token.strip_prefix("--reuse-message="))
        .is_some_and(|value| !value.is_empty())
        || token
            .strip_prefix("-m")
            .or_else(|| token.strip_prefix("-F"))
            .or_else(|| token.strip_prefix("-C"))
            .is_some_and(|value| !value.is_empty())
}

fn git_short_option_group_contains(token: &str, flag: char) -> bool {
    token.starts_with('-')
        && !token.starts_with("--")
        && token.len() > 2
        && token
            .trim_start_matches('-')
            .chars()
            .any(|character| character == flag)
}

pub(in crate::permission_rules) fn git_tool_command_config_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return None;
    }

    let mut tool_commands = Vec::<(String, String, String)>::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return None;
        } else if git_global_flag_option(token) {
            index += 1;
        } else if token == "-c" {
            let config = tokens.get(index + 1)?;
            push_git_tool_command_config(config, &mut tool_commands);
            index += 2;
        } else if let Some(config) = token.strip_prefix("-c").filter(|config| !config.is_empty()) {
            push_git_tool_command_config(config, &mut tool_commands);
            index += 1;
        } else if git_global_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if git_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            break;
        }
    }

    let subcommand = tokens.get(index)?;
    if !matches!(subcommand.as_str(), "difftool" | "mergetool") {
        return None;
    }
    let tool = git_selected_tool_name(tokens.get(index + 1..).unwrap_or_default())?;
    tool_commands
        .iter()
        .rev()
        .find(|(kind, name, _)| kind == subcommand && name == &tool)
        .map(|(_, _, body)| body.clone())
}

pub(in crate::permission_rules) fn git_credential_helper_command_bodies(
    tokens: &[String],
) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return Vec::new();
    }

    let mut helpers = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return Vec::new();
        } else if git_global_flag_option(token) {
            index += 1;
        } else if token == "-c" {
            let Some(config) = tokens.get(index + 1) else {
                return Vec::new();
            };
            push_git_credential_helper_config(config, &mut helpers);
            index += 2;
        } else if let Some(config) = token.strip_prefix("-c").filter(|config| !config.is_empty()) {
            push_git_credential_helper_config(config, &mut helpers);
            index += 1;
        } else if git_global_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return Vec::new();
            }
            index += 2;
        } else if git_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return Vec::new();
        } else {
            break;
        }
    }

    if tokens
        .get(index)
        .is_some_and(|subcommand| git_invokes_credential_helper(subcommand))
    {
        helpers
    } else {
        Vec::new()
    }
}

fn push_git_credential_helper_config(config: &str, helpers: &mut Vec<String>) {
    let Some((name, value)) = config.split_once('=') else {
        return;
    };
    if !name.eq_ignore_ascii_case("credential.helper") {
        return;
    }
    let Some(body) = value.trim().strip_prefix('!').map(str::trim) else {
        return;
    };
    if !body.is_empty() && !helpers.iter().any(|existing| existing == body) {
        helpers.push(body.to_string());
    }
}

fn git_invokes_credential_helper(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "credential" | "clone" | "fetch" | "pull" | "push" | "ls-remote"
    )
}

fn push_git_tool_command_config(config: &str, tool_commands: &mut Vec<(String, String, String)>) {
    let Some((key, value)) = config.split_once('=') else {
        return;
    };
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    let Some((kind, rest)) = key.split_once('.') else {
        return;
    };
    if !matches!(kind, "difftool" | "mergetool") {
        return;
    }
    let Some((name, field)) = rest.rsplit_once('.') else {
        return;
    };
    if !name.is_empty() && field == "cmd" {
        tool_commands.push((kind.to_string(), name.to_string(), value.to_string()));
    }
}

fn git_selected_tool_name(args: &[String]) -> Option<String> {
    let mut index = 0usize;
    while let Some(token) = args.get(index) {
        if token == "--" {
            return None;
        } else if matches!(token.as_str(), "-t" | "--tool") {
            return tokens_get_non_empty(args, index + 1);
        } else if let Some(value) = token.strip_prefix("--tool=") {
            return (!value.is_empty()).then(|| value.to_string());
        } else if let Some(value) = token.strip_prefix("-t").filter(|value| !value.is_empty()) {
            return Some(value.to_string());
        }
        index += 1;
    }

    None
}

fn tokens_get_non_empty(tokens: &[String], index: usize) -> Option<String> {
    tokens
        .get(index)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(in crate::permission_rules) fn git_filter_branch_command_bodies(
    tokens: &[String],
) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return Vec::new();
    }

    let Some(mut index) = git_command_start_index(tokens) else {
        return Vec::new();
    };
    if tokens
        .get(index)
        .is_none_or(|token| token != "filter-branch")
    {
        return Vec::new();
    }
    index += 1;

    let mut bodies = Vec::new();
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            break;
        } else if git_filter_branch_command_option(token) {
            let Some(body) = tokens.get(index + 1) else {
                return Vec::new();
            };
            push_unique_git_filter_branch_body(body, &mut bodies);
            index += 2;
        } else if let Some(body) = git_filter_branch_inline_command_option(token) {
            push_unique_git_filter_branch_body(body, &mut bodies);
            index += 1;
        } else if git_filter_branch_flag_option(token) {
            index += 1;
        } else if git_filter_branch_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return Vec::new();
            }
            index += 2;
        } else if git_filter_branch_inline_value_option(token) {
            index += 1;
        } else {
            break;
        }
    }

    bodies
}

pub(in crate::permission_rules) fn git_rebase_exec_command_bodies(
    tokens: &[String],
) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return Vec::new();
    }

    let Some(mut index) = git_command_start_index(tokens) else {
        return Vec::new();
    };
    if tokens.get(index).is_none_or(|token| token != "rebase") {
        return Vec::new();
    }
    index += 1;

    let mut bodies = Vec::new();
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            break;
        } else if matches!(token.as_str(), "-x" | "--exec") {
            let Some(body) = tokens.get(index + 1) else {
                return Vec::new();
            };
            push_unique_git_filter_branch_body(body, &mut bodies);
            index += 2;
        } else if let Some(body) = token.strip_prefix("--exec=") {
            push_unique_git_filter_branch_body(body, &mut bodies);
            index += 1;
        } else if git_rebase_flag_option(token) {
            index += 1;
        } else if git_rebase_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return Vec::new();
            }
            index += 2;
        } else if git_rebase_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return Vec::new();
        } else {
            break;
        }
    }

    bodies
}

pub(in crate::permission_rules) fn git_difftool_extcmd_command_bodies(
    tokens: &[String],
) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return Vec::new();
    }

    let Some(mut index) = git_command_start_index(tokens) else {
        return Vec::new();
    };
    if tokens.get(index).is_none_or(|token| token != "difftool") {
        return Vec::new();
    }
    index += 1;

    let mut bodies = Vec::new();
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            break;
        } else if matches!(token.as_str(), "-x" | "--extcmd") {
            let Some(body) = tokens.get(index + 1) else {
                return bodies;
            };
            push_unique_git_filter_branch_body(body, &mut bodies);
            index += 2;
        } else if let Some(body) = token
            .strip_prefix("--extcmd=")
            .or_else(|| token.strip_prefix("-x").filter(|value| !value.is_empty()))
        {
            push_unique_git_filter_branch_body(body, &mut bodies);
            index += 1;
        } else {
            index += 1;
        }
    }

    bodies
}

pub(in crate::permission_rules) fn git_command_start_index(tokens: &[String]) -> Option<usize> {
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return Some(index + 1);
        } else if git_global_flag_option(token) {
            index += 1;
        } else if git_global_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if git_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            return Some(index);
        }
    }
    None
}

fn push_unique_git_filter_branch_body(body: &str, bodies: &mut Vec<String>) {
    let body = body.trim();
    if !body.is_empty() && !bodies.iter().any(|existing| existing == body) {
        bodies.push(body.to_string());
    }
}

fn git_filter_branch_command_option(token: &str) -> bool {
    matches!(
        token,
        "--setup"
            | "--env-filter"
            | "--tree-filter"
            | "--index-filter"
            | "--parent-filter"
            | "--msg-filter"
            | "--commit-filter"
            | "--tag-name-filter"
    )
}

fn git_filter_branch_inline_command_option(token: &str) -> Option<&str> {
    token
        .strip_prefix("--setup=")
        .or_else(|| token.strip_prefix("--env-filter="))
        .or_else(|| token.strip_prefix("--tree-filter="))
        .or_else(|| token.strip_prefix("--index-filter="))
        .or_else(|| token.strip_prefix("--parent-filter="))
        .or_else(|| token.strip_prefix("--msg-filter="))
        .or_else(|| token.strip_prefix("--commit-filter="))
        .or_else(|| token.strip_prefix("--tag-name-filter="))
        .filter(|body| !body.trim().is_empty())
}

fn git_filter_branch_flag_option(token: &str) -> bool {
    matches!(token, "--prune-empty" | "-f" | "--force")
}

fn git_filter_branch_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "--subdirectory-filter" | "--original" | "-d" | "--state-branch"
    )
}

fn git_filter_branch_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--subdirectory-filter=")
        .or_else(|| token.strip_prefix("--original="))
        .or_else(|| token.strip_prefix("-d"))
        .or_else(|| token.strip_prefix("--state-branch="))
        .is_some_and(|value| !value.is_empty())
}

fn git_rebase_flag_option(token: &str) -> bool {
    matches!(
        token,
        "-i" | "--interactive"
            | "--root"
            | "--keep-base"
            | "--continue"
            | "--skip"
            | "--abort"
            | "--quit"
            | "--edit-todo"
            | "--show-current-patch"
            | "--merge"
            | "--apply"
            | "--no-keep-empty"
            | "--keep-empty"
            | "--autosquash"
            | "--no-autosquash"
            | "--autostash"
            | "--no-autostash"
            | "--rebase-merges"
            | "--no-rebase-merges"
            | "--reschedule-failed-exec"
            | "--no-reschedule-failed-exec"
            | "--fork-point"
            | "--no-fork-point"
    )
}

fn git_rebase_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "--onto"
            | "--empty"
            | "--strategy"
            | "-s"
            | "--strategy-option"
            | "-X"
            | "--whitespace"
            | "--gpg-sign"
            | "-S"
    )
}

fn git_rebase_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--onto=")
        .or_else(|| token.strip_prefix("--empty="))
        .or_else(|| token.strip_prefix("--strategy="))
        .or_else(|| token.strip_prefix("-s"))
        .or_else(|| token.strip_prefix("--strategy-option="))
        .or_else(|| token.strip_prefix("-X"))
        .or_else(|| token.strip_prefix("--rebase-merges="))
        .or_else(|| token.strip_prefix("--whitespace="))
        .or_else(|| token.strip_prefix("--gpg-sign="))
        .or_else(|| token.strip_prefix("-S"))
        .is_some_and(|value| !value.is_empty())
}

pub(in crate::permission_rules) fn push_git_shell_alias_config(
    config: &str,
    aliases: &mut Vec<(String, String)>,
) {
    let Some((key, value)) = config.split_once('=') else {
        return;
    };
    let Some(name) = key.strip_prefix("alias.") else {
        return;
    };
    if name.is_empty() || value.trim().is_empty() {
        return;
    }
    aliases.push((name.to_string(), value.to_string()));
}

pub(in crate::permission_rules) fn is_git_command_token(token: &str) -> bool {
    matches!(
        token,
        "git" | "/usr/bin/git" | "/usr/local/bin/git" | "/opt/homebrew/bin/git"
    )
}

pub(in crate::permission_rules) fn git_global_flag_option(token: &str) -> bool {
    matches!(
        token,
        "-p" | "--paginate" | "-P" | "--no-pager" | "--bare" | "--no-replace-objects"
    )
}

pub(in crate::permission_rules) fn git_global_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-C" | "-c"
            | "--config-env"
            | "--exec-path"
            | "--git-dir"
            | "--namespace"
            | "--super-prefix"
            | "--work-tree"
    )
}

pub(in crate::permission_rules) fn git_global_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("-C")
        .or_else(|| token.strip_prefix("-c"))
        .or_else(|| token.strip_prefix("--config-env="))
        .or_else(|| token.strip_prefix("--exec-path="))
        .or_else(|| token.strip_prefix("--git-dir="))
        .or_else(|| token.strip_prefix("--namespace="))
        .or_else(|| token.strip_prefix("--super-prefix="))
        .or_else(|| token.strip_prefix("--work-tree="))
        .is_some_and(|value| !value.is_empty())
}
