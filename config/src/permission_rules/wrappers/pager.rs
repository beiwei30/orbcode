use super::vcs_cmd::{
    git_global_flag_option, git_global_inline_value_option, git_global_option_takes_value,
    is_git_command_token,
};

pub(in crate::permission_rules) fn push_git_pager_config(config: &str, pagers: &mut Vec<String>) {
    let Some((name, value)) = config.split_once('=') else {
        return;
    };
    let value = value.trim();
    if name.eq_ignore_ascii_case("core.pager") && !value.is_empty() {
        pagers.push(value.to_string());
    }
}

pub(in crate::permission_rules) fn git_command_invokes_pager_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return false;
    }

    let mut forced_pager = false;
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if matches!(token.as_str(), "-P" | "--no-pager") || token == "--" {
            return false;
        } else if git_global_paginate_flag(token) {
            forced_pager = true;
            index += 1;
        } else if git_global_flag_option(token) {
            index += 1;
        } else if git_global_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if git_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            break;
        }
    }

    forced_pager || git_command_invokes_pager_at(tokens, index)
}

pub(in crate::permission_rules) fn git_pager_command_config_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return None;
    }

    let mut pagers = Vec::new();
    let mut forced_pager = false;
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if matches!(token.as_str(), "-P" | "--no-pager") {
            return None;
        } else if token == "--" {
            return None;
        } else if git_global_paginate_flag(token) {
            forced_pager = true;
            index += 1;
        } else if git_global_flag_option(token) {
            index += 1;
        } else if token == "-c" {
            let config = tokens.get(index + 1)?;
            push_git_pager_config(config, &mut pagers);
            index += 2;
        } else if let Some(config) = token.strip_prefix("-c").filter(|config| !config.is_empty()) {
            push_git_pager_config(config, &mut pagers);
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

    if forced_pager || git_command_invokes_pager_at(tokens, index) {
        pagers.last().cloned()
    } else {
        None
    }
}

fn git_command_invokes_pager_at(tokens: &[String], index: usize) -> bool {
    tokens
        .get(index)
        .is_some_and(|subcommand| matches!(subcommand.as_str(), "log" | "show" | "diff"))
}

fn git_global_paginate_flag(token: &str) -> bool {
    matches!(token, "-p" | "--paginate")
}

pub(in crate::permission_rules) fn man_pager_command_option_body(
    tokens: &[String],
) -> Option<String> {
    let (pager, invokes_pager) = man_pager_command(tokens)?;
    if invokes_pager { pager } else { None }
}

pub(in crate::permission_rules) fn man_command_invokes_pager_from_tokens(
    tokens: &[String],
) -> bool {
    man_pager_command(tokens).is_some_and(|(_, invokes_pager)| invokes_pager)
}

fn man_pager_command(tokens: &[String]) -> Option<(Option<String>, bool)> {
    if !tokens
        .first()
        .is_some_and(|token| is_man_command_token(token))
    {
        return None;
    }

    let mut pager = None;
    let mut has_page = false;
    let mut no_pager = false;
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            has_page = tokens.get(index + 1).is_some();
            break;
        } else if token == "-P" || token == "--pager" {
            let body = tokens.get(index + 1)?;
            pager = Some(body.clone());
            index += 2;
        } else if let Some(body) = token
            .strip_prefix("--pager=")
            .or_else(|| token.strip_prefix("-P").filter(|value| !value.is_empty()))
        {
            pager = Some(body.to_string());
            index += 1;
        } else if man_no_pager_option(token) {
            no_pager = true;
            index += 1;
        } else if man_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if man_inline_value_option(token) || man_flag_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            has_page = true;
            index += 1;
        }
    }

    Some((pager, has_page && !no_pager))
}

fn man_no_pager_option(token: &str) -> bool {
    matches!(
        token,
        "-f" | "--whatis" | "-k" | "--apropos" | "-K" | "-w" | "--where" | "--path" | "-W"
    )
}

fn man_flag_option(token: &str) -> bool {
    matches!(
        token,
        "-a" | "--all"
            | "-c"
            | "--catman"
            | "-d"
            | "--debug"
            | "-D"
            | "--default"
            | "-h"
            | "--help"
            | "-i"
            | "--ignore-case"
            | "-I"
            | "--match-case"
            | "-l"
            | "--local-file"
            | "-u"
            | "--update"
            | "-V"
            | "--version"
    )
}

fn man_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-C" | "--config-file"
            | "-L"
            | "--locale"
            | "-m"
            | "--systems"
            | "-M"
            | "--manpath"
            | "-S"
            | "-s"
            | "--sections"
            | "-e"
            | "--extension"
            | "-p"
            | "--preprocessor"
            | "-r"
            | "--prompt"
            | "-E"
            | "--encoding"
    )
}

fn man_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("-C")
        .or_else(|| token.strip_prefix("-L"))
        .or_else(|| token.strip_prefix("-m"))
        .or_else(|| token.strip_prefix("-M"))
        .or_else(|| token.strip_prefix("-S"))
        .or_else(|| token.strip_prefix("-s"))
        .or_else(|| token.strip_prefix("-e"))
        .or_else(|| token.strip_prefix("-p"))
        .or_else(|| token.strip_prefix("-r"))
        .or_else(|| token.strip_prefix("-E"))
        .is_some_and(|value| !value.is_empty())
        || token
            .strip_prefix("--config-file=")
            .or_else(|| token.strip_prefix("--locale="))
            .or_else(|| token.strip_prefix("--systems="))
            .or_else(|| token.strip_prefix("--manpath="))
            .or_else(|| token.strip_prefix("--sections="))
            .or_else(|| token.strip_prefix("--extension="))
            .or_else(|| token.strip_prefix("--preprocessor="))
            .or_else(|| token.strip_prefix("--prompt="))
            .or_else(|| token.strip_prefix("--encoding="))
            .is_some_and(|value| !value.is_empty())
}

fn is_man_command_token(token: &str) -> bool {
    matches!(
        token,
        "man" | "/usr/bin/man" | "/usr/local/bin/man" | "/opt/homebrew/bin/man"
    )
}

pub(in crate::permission_rules) fn browser_opener_invokes_browser_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_browser_opener_command_token(token))
    {
        return false;
    }

    let index = 1usize;
    if let Some(token) = tokens.get(index) {
        if token == "--" {
            return tokens.get(index + 1).is_some();
        } else if browser_opener_help_option(token) {
            return false;
        } else {
            return !token.starts_with('-');
        }
    }

    false
}

pub(in crate::permission_rules) fn systemctl_command_invokes_editor_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_systemctl_command_token(token))
    {
        return false;
    }

    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return false;
        } else if matches!(token.as_str(), "--help" | "-h" | "--version") {
            return false;
        } else if systemctl_global_flag_option(token) {
            index += 1;
        } else if systemctl_global_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if systemctl_global_inline_value_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            break;
        }
    }

    if tokens.get(index).is_none_or(|token| token != "edit") {
        return false;
    }
    index += 1;

    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if matches!(token.as_str(), "--help" | "-h") {
            return false;
        } else if systemctl_edit_flag_option(token) {
            index += 1;
        } else if systemctl_edit_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if systemctl_edit_inline_value_option(token) {
            index += 1;
        } else {
            return !token.starts_with('-');
        }
    }

    tokens.get(index).is_some()
}

fn is_systemctl_command_token(token: &str) -> bool {
    matches!(
        token,
        "systemctl"
            | "/usr/bin/systemctl"
            | "/usr/local/bin/systemctl"
            | "/opt/homebrew/bin/systemctl"
    )
}

fn systemctl_global_flag_option(token: &str) -> bool {
    matches!(
        token,
        "--system"
            | "--user"
            | "--global"
            | "--no-pager"
            | "--no-legend"
            | "--plain"
            | "--quiet"
            | "-q"
            | "--wait"
            | "--dry-run"
    )
}

fn systemctl_global_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "--host"
            | "-H"
            | "--machine"
            | "-M"
            | "--type"
            | "-t"
            | "--state"
            | "--property"
            | "-p"
            | "--signal"
            | "-s"
    )
}

fn systemctl_global_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--host=")
        .or_else(|| token.strip_prefix("-H").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--machine="))
        .or_else(|| token.strip_prefix("-M").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--type="))
        .or_else(|| token.strip_prefix("-t").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--state="))
        .or_else(|| token.strip_prefix("--property="))
        .or_else(|| token.strip_prefix("-p").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--signal="))
        .or_else(|| token.strip_prefix("-s").filter(|value| !value.is_empty()))
        .is_some_and(|value| !value.is_empty())
}

fn systemctl_edit_flag_option(token: &str) -> bool {
    matches!(token, "--full" | "--force" | "--runtime")
}

fn systemctl_edit_option_takes_value(token: &str) -> bool {
    matches!(token, "--drop-in")
}

fn systemctl_edit_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--drop-in=")
        .is_some_and(|value| !value.is_empty())
}

fn is_browser_opener_command_token(token: &str) -> bool {
    matches!(
        token,
        "xdg-open"
            | "sensible-browser"
            | "x-www-browser"
            | "gnome-open"
            | "/usr/bin/xdg-open"
            | "/usr/bin/sensible-browser"
            | "/usr/bin/x-www-browser"
            | "/usr/bin/gnome-open"
            | "/usr/local/bin/xdg-open"
            | "/usr/local/bin/sensible-browser"
            | "/usr/local/bin/x-www-browser"
            | "/usr/local/bin/gnome-open"
            | "/opt/homebrew/bin/xdg-open"
            | "/opt/homebrew/bin/sensible-browser"
            | "/opt/homebrew/bin/x-www-browser"
            | "/opt/homebrew/bin/gnome-open"
    )
}

fn browser_opener_help_option(token: &str) -> bool {
    matches!(token, "-h" | "--help" | "-v" | "--version" | "--manual")
}

pub(in crate::permission_rules) fn less_command_invokes_preprocessor_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_less_command_token(token))
    {
        return false;
    }

    let mut has_file = false;
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            has_file = tokens
                .get(index + 1..)
                .unwrap_or_default()
                .iter()
                .any(|value| value != "-");
            break;
        } else if less_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if less_inline_value_option(token) || less_flag_option(token) {
            index += 1;
        } else if token.starts_with('-') {
            return false;
        } else {
            if token != "-" {
                has_file = true;
            }
            index += 1;
        }
    }

    has_file
}

fn is_less_command_token(token: &str) -> bool {
    matches!(
        token,
        "less" | "/usr/bin/less" | "/usr/local/bin/less" | "/opt/homebrew/bin/less"
    )
}

fn less_flag_option(token: &str) -> bool {
    matches!(
        token,
        "-?" | "--help"
            | "-a"
            | "--search-skip-screen"
            | "-A"
            | "--SEARCH-SKIP-SCREEN"
            | "-b"
            | "-B"
            | "--auto-buffers"
            | "-c"
            | "--clear-screen"
            | "-C"
            | "--CLEAR-SCREEN"
            | "-d"
            | "--dumb"
            | "-e"
            | "--quit-at-eof"
            | "-E"
            | "--QUIT-AT-EOF"
            | "-f"
            | "--force"
            | "-F"
            | "--quit-if-one-screen"
            | "-g"
            | "--hilite-search"
            | "-G"
            | "--HILITE-SEARCH"
            | "-i"
            | "--ignore-case"
            | "-I"
            | "--IGNORE-CASE"
            | "-K"
            | "--quit-on-intr"
            | "-m"
            | "--long-prompt"
            | "-M"
            | "--LONG-PROMPT"
            | "-n"
            | "--line-numbers"
            | "-N"
            | "--LINE-NUMBERS"
            | "-q"
            | "--quiet"
            | "-Q"
            | "--QUIET"
            | "-R"
            | "--RAW-CONTROL-CHARS"
            | "-S"
            | "--chop-long-lines"
            | "-w"
            | "--hilite-unread"
            | "-X"
            | "--no-init"
    )
}

fn less_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-j" | "--jump-target"
            | "-k"
            | "--lesskey-file"
            | "-P"
            | "--prompt"
            | "-t"
            | "--tag"
            | "-T"
            | "--tag-file"
            | "-x"
            | "--tabs"
    )
}

fn less_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("-j")
        .or_else(|| token.strip_prefix("-k"))
        .or_else(|| token.strip_prefix("-P"))
        .or_else(|| token.strip_prefix("-t"))
        .or_else(|| token.strip_prefix("-T"))
        .or_else(|| token.strip_prefix("-x"))
        .is_some_and(|value| !value.is_empty())
        || token
            .strip_prefix("--jump-target=")
            .or_else(|| token.strip_prefix("--lesskey-file="))
            .or_else(|| token.strip_prefix("--prompt="))
            .or_else(|| token.strip_prefix("--tag="))
            .or_else(|| token.strip_prefix("--tag-file="))
            .or_else(|| token.strip_prefix("--tabs="))
            .is_some_and(|value| !value.is_empty())
}
