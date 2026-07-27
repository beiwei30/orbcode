use super::super::{
    crontab::crontab_command_invokes_editor_from_tokens, is_bash_env_assignment,
    strip_bash_wrappers_with_shell_command_strings,
};
use super::container_cli::{
    kubectl_command_invokes_editor_from_tokens, kubectl_command_invokes_external_diff_from_tokens,
};
use super::env_wrap::{expand_env_split_string, is_env_command_token};
use super::pager::{
    browser_opener_invokes_browser_from_tokens, git_command_invokes_pager_from_tokens,
    less_command_invokes_preprocessor_from_tokens, man_command_invokes_pager_from_tokens,
    systemctl_command_invokes_editor_from_tokens,
};
use super::privilege::{sudo_command_invokes_askpass, sudo_command_invokes_editor_from_tokens};
use super::remote_shell::{is_rsync_command_token, ssh_command_invokes_askpass_from_tokens};
use super::vcs_cmd::{
    git_command_invokes_askpass_from_tokens, git_command_invokes_editor_from_tokens,
    git_command_invokes_external_diff_from_tokens, git_command_invokes_sequence_editor_from_tokens,
    git_env_config_command_string_bodies, git_global_flag_option, git_global_inline_value_option,
    git_global_option_takes_value, hg_command_invokes_editor_from_tokens, is_git_command_token,
    is_svn_command_token, svn_command_invokes_editor_from_tokens,
};

pub(in crate::permission_rules) fn env_assignment_command_string_bodies(
    tokens: &[String],
) -> Vec<String> {
    let mut bodies = Vec::new();
    push_leading_env_assignment_command_string_bodies(tokens, &mut bodies);
    push_env_wrapper_assignment_command_string_bodies(tokens, &mut bodies);
    push_leading_git_env_config_command_string_bodies(tokens, &mut bodies);
    push_env_wrapper_git_env_config_command_string_bodies(tokens, &mut bodies);
    bodies
}

fn push_leading_env_assignment_command_string_bodies(tokens: &[String], bodies: &mut Vec<String>) {
    let mut assignments = Vec::new();
    let mut index = 0usize;
    while let Some(token) = tokens.get(index) {
        if is_bash_env_assignment(token, true) {
            collect_command_string_env_assignment(token, &mut assignments);
            index += 1;
        } else {
            break;
        }
    }
    push_matching_env_assignment_command_string_bodies(&assignments, &tokens[index..], bodies);
}

fn push_leading_git_env_config_command_string_bodies(tokens: &[String], bodies: &mut Vec<String>) {
    let mut assignments = Vec::new();
    let mut index = 0usize;
    while let Some(token) = tokens.get(index) {
        if is_bash_env_assignment(token, true) {
            collect_env_assignment(token, &mut assignments);
            index += 1;
        } else {
            break;
        }
    }
    push_matching_git_env_config_command_string_bodies(&assignments, &tokens[index..], bodies);
}

fn push_env_wrapper_assignment_command_string_bodies(tokens: &[String], bodies: &mut Vec<String>) {
    if let Some(expanded) = expand_env_split_string(tokens)
        && expanded != tokens
    {
        push_env_wrapper_assignment_command_string_bodies(&expanded, bodies);
    }

    if !tokens
        .first()
        .is_some_and(|token| is_env_command_token(token))
    {
        return;
    }

    let mut assignments = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if matches!(
            token.as_str(),
            "-i" | "-" | "--ignore-environment" | "-0" | "--null"
        ) {
            index += 1;
        } else if matches!(
            token.as_str(),
            "-a" | "--argv0" | "-C" | "--chdir" | "-u" | "--unset"
        ) {
            if tokens.get(index + 1).is_none() {
                return;
            }
            index += 2;
        } else if token.starts_with("-u")
            || token.starts_with("--argv0=")
            || token.starts_with("--chdir=")
            || token.starts_with("--unset=")
        {
            index += 1;
        } else if token == "--" {
            index += 1;
            break;
        } else if is_bash_env_assignment(token, true) {
            collect_command_string_env_assignment(token, &mut assignments);
            index += 1;
        } else if token.starts_with('-') {
            return;
        } else {
            break;
        }
    }

    push_matching_env_assignment_command_string_bodies(&assignments, &tokens[index..], bodies);
}

fn push_env_wrapper_git_env_config_command_string_bodies(
    tokens: &[String],
    bodies: &mut Vec<String>,
) {
    if let Some(expanded) = expand_env_split_string(tokens)
        && expanded != tokens
    {
        push_env_wrapper_git_env_config_command_string_bodies(&expanded, bodies);
    }

    if !tokens
        .first()
        .is_some_and(|token| is_env_command_token(token))
    {
        return;
    }

    let mut assignments = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if matches!(
            token.as_str(),
            "-i" | "-" | "--ignore-environment" | "-0" | "--null"
        ) {
            index += 1;
        } else if matches!(
            token.as_str(),
            "-a" | "--argv0" | "-C" | "--chdir" | "-u" | "--unset"
        ) {
            if tokens.get(index + 1).is_none() {
                return;
            }
            index += 2;
        } else if token.starts_with("-u")
            || token.starts_with("--argv0=")
            || token.starts_with("--chdir=")
            || token.starts_with("--unset=")
        {
            index += 1;
        } else if token == "--" {
            index += 1;
            break;
        } else if is_bash_env_assignment(token, true) {
            collect_env_assignment(token, &mut assignments);
            index += 1;
        } else if token.starts_with('-') {
            return;
        } else {
            break;
        }
    }

    push_matching_git_env_config_command_string_bodies(&assignments, &tokens[index..], bodies);
}

fn collect_env_assignment(token: &str, assignments: &mut Vec<(String, String)>) {
    let Some((name, value)) = token.split_once('=') else {
        return;
    };
    assignments.push((name.to_string(), value.to_string()));
}

fn collect_command_string_env_assignment<'a>(
    token: &'a str,
    assignments: &mut Vec<(&'a str, &'a str)>,
) {
    let Some((name, value)) = token.split_once('=') else {
        return;
    };
    if is_command_string_env_var(name)
        && let Some(body) = command_string_env_body(name, value)
    {
        assignments.push((name, body));
    }
}

fn is_command_string_env_var(name: &str) -> bool {
    matches!(
        name,
        "GIT_SSH_COMMAND"
            | "GIT_PROXY_COMMAND"
            | "GIT_ASKPASS"
            | "SSH_ASKPASS"
            | "GIT_EXTERNAL_DIFF"
            | "KUBECTL_EXTERNAL_DIFF"
            | "KUBE_EDITOR"
            | "GIT_EDITOR"
            | "GIT_SEQUENCE_EDITOR"
            | "SUDO_EDITOR"
            | "SYSTEMD_EDITOR"
            | "SVN_EDITOR"
            | "HGEDITOR"
            | "SUDO_ASKPASS"
            | "VISUAL"
            | "EDITOR"
            | "GIT_PAGER"
            | "MANPAGER"
            | "PAGER"
            | "BROWSER"
            | "LESSOPEN"
            | "LESSCLOSE"
            | "RSYNC_RSH"
            | "SVN_SSH"
    )
}

fn command_string_env_body<'a>(name: &str, value: &'a str) -> Option<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if name == "LESSOPEN" {
        return value
            .strip_prefix("||")
            .or_else(|| value.strip_prefix('|'))
            .map(str::trim)
            .filter(|body| !body.is_empty());
    }
    Some(value)
}

fn push_matching_env_assignment_command_string_bodies(
    assignments: &[(&str, &str)],
    command_tokens: &[String],
    bodies: &mut Vec<String>,
) {
    if assignments.is_empty() || command_tokens.is_empty() {
        return;
    }

    for (name, body) in assignments {
        if env_assignment_command_applies_to_target(name, command_tokens)
            && !bodies.iter().any(|existing| existing == body)
        {
            bodies.push((*body).to_string());
        }
    }
}

fn push_matching_git_env_config_command_string_bodies(
    assignments: &[(String, String)],
    command_tokens: &[String],
    bodies: &mut Vec<String>,
) {
    if command_tokens.is_empty() {
        return;
    }

    let mut tokens = command_tokens.to_vec();
    strip_bash_wrappers_with_shell_command_strings(&mut tokens, true, false);
    let mut configs = git_env_config_assignments(assignments);
    configs.extend(git_config_env_option_assignments(assignments, &tokens));
    if configs.is_empty() {
        return;
    }
    for body in git_env_config_command_string_bodies(&configs, &tokens) {
        if !bodies.iter().any(|existing| existing == &body) {
            bodies.push(body);
        }
    }
}

fn git_env_config_assignments(assignments: &[(String, String)]) -> Vec<String> {
    let Some(count) = env_assignment_last_value(assignments, "GIT_CONFIG_COUNT")
        .and_then(|value| value.parse::<usize>().ok())
    else {
        return Vec::new();
    };

    let mut configs = Vec::new();
    for index in 0..count {
        let key_name = format!("GIT_CONFIG_KEY_{index}");
        let value_name = format!("GIT_CONFIG_VALUE_{index}");
        let Some(key) = env_assignment_last_value(assignments, &key_name) else {
            continue;
        };
        let Some(value) = env_assignment_last_value(assignments, &value_name) else {
            continue;
        };
        if !key.trim().is_empty() {
            configs.push(format!("{}={}", key.trim(), value.trim()));
        }
    }
    configs
}

fn git_config_env_option_assignments(
    assignments: &[(String, String)],
    tokens: &[String],
) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_git_command_token(token))
    {
        return Vec::new();
    }

    let mut configs = Vec::new();
    let mut index = 1usize;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            break;
        } else if token == "--config-env" {
            let Some(spec) = tokens.get(index + 1) else {
                return Vec::new();
            };
            push_git_config_env_assignment(spec, assignments, &mut configs);
            index += 2;
        } else if let Some(spec) = token.strip_prefix("--config-env=") {
            push_git_config_env_assignment(spec, assignments, &mut configs);
            index += 1;
        } else if git_global_flag_option(token) {
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

    configs
}

fn push_git_config_env_assignment(
    spec: &str,
    assignments: &[(String, String)],
    configs: &mut Vec<String>,
) {
    let Some((key, env_name)) = spec.split_once('=') else {
        return;
    };
    let key = key.trim();
    let env_name = env_name.trim();
    if key.is_empty() || env_name.is_empty() {
        return;
    }
    let Some(value) = env_assignment_last_value(assignments, env_name) else {
        return;
    };
    configs.push(format!("{key}={}", value.trim()));
}

fn env_assignment_last_value<'a>(
    assignments: &'a [(String, String)],
    name: &str,
) -> Option<&'a str> {
    assignments
        .iter()
        .rev()
        .find(|(assignment_name, _)| assignment_name == name)
        .map(|(_, value)| value.as_str())
}

fn env_assignment_command_applies_to_target(name: &str, command_tokens: &[String]) -> bool {
    if name == "SUDO_ASKPASS" {
        return sudo_command_invokes_askpass(command_tokens);
    }
    if name == "SUDO_EDITOR" {
        return sudo_command_invokes_editor_from_tokens(command_tokens);
    }
    if name == "SYSTEMD_EDITOR" {
        return systemctl_command_invokes_editor_from_tokens(command_tokens);
    }
    if name == "SVN_EDITOR" {
        return svn_command_invokes_editor_from_tokens(command_tokens);
    }
    if name == "HGEDITOR" {
        return hg_command_invokes_editor_from_tokens(command_tokens);
    }

    let mut tokens = command_tokens.to_vec();
    strip_bash_wrappers_with_shell_command_strings(&mut tokens, true, false);
    let Some(command) = tokens.first().map(String::as_str) else {
        return false;
    };

    match name {
        "GIT_SSH_COMMAND" | "GIT_PROXY_COMMAND" => is_git_command_token(command),
        "GIT_ASKPASS" => git_command_invokes_askpass_from_tokens(&tokens),
        "SSH_ASKPASS" => {
            git_command_invokes_askpass_from_tokens(&tokens)
                || ssh_command_invokes_askpass_from_tokens(&tokens)
        }
        "GIT_EXTERNAL_DIFF" => git_command_invokes_external_diff_from_tokens(&tokens),
        "KUBECTL_EXTERNAL_DIFF" => kubectl_command_invokes_external_diff_from_tokens(&tokens),
        "KUBE_EDITOR" => kubectl_command_invokes_editor_from_tokens(&tokens),
        "RSYNC_RSH" => is_rsync_command_token(command),
        "SVN_SSH" => is_svn_command_token(command),
        "GIT_EDITOR" => git_command_invokes_editor_from_tokens(&tokens),
        "VISUAL" | "EDITOR" => {
            git_command_invokes_editor_from_tokens(&tokens)
                || crontab_command_invokes_editor_from_tokens(&tokens)
                || kubectl_command_invokes_editor_from_tokens(&tokens)
                || svn_command_invokes_editor_from_tokens(&tokens)
                || hg_command_invokes_editor_from_tokens(&tokens)
                || sudo_command_invokes_editor_from_tokens(command_tokens)
                || systemctl_command_invokes_editor_from_tokens(command_tokens)
        }
        "GIT_SEQUENCE_EDITOR" => git_command_invokes_sequence_editor_from_tokens(&tokens),
        "GIT_PAGER" => git_command_invokes_pager_from_tokens(&tokens),
        "MANPAGER" => man_command_invokes_pager_from_tokens(&tokens),
        "PAGER" => {
            git_command_invokes_pager_from_tokens(&tokens)
                || man_command_invokes_pager_from_tokens(&tokens)
        }
        "BROWSER" => browser_opener_invokes_browser_from_tokens(&tokens),
        "LESSOPEN" | "LESSCLOSE" => less_command_invokes_preprocessor_from_tokens(&tokens),
        _ => false,
    }
}
