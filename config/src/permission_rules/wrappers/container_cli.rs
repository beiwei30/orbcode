use super::super::wrapper_spec;

pub(in crate::permission_rules) fn container_cli_argv_command_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_container_cli_command_token(token))
    {
        return None;
    }

    let mut index = container_cli_subcommand_index(tokens)?;
    if tokens.get(index).is_some_and(|token| token == "compose") {
        return container_compose_argv_command_body(tokens, index + 1);
    }
    if tokens.get(index).is_some_and(|token| token == "container") {
        index += 1;
    }
    let subcommand = tokens.get(index)?;
    if !matches!(subcommand.as_str(), "exec" | "run") {
        return None;
    }
    index += 1;

    let mut index =
        wrapper_spec::scan_options_block(tokens, index, &CONTAINER_CLI_SUBCOMMAND_OPTIONS)?;

    tokens.get(index)?;
    index += 1;

    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

fn container_cli_subcommand_index(tokens: &[String]) -> Option<usize> {
    let index = wrapper_spec::scan_options_block(tokens, 1, &CONTAINER_CLI_GLOBAL_OPTIONS)?;
    tokens.get(index).map(|_| index)
}

const CONTAINER_CLI_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["-D", "--debug", "--tls", "--tlsverify"],
    long_options_with_value: &[
        "-c",
        "--context",
        "--config",
        "-H",
        "--host",
        "-l",
        "--log-level",
        "--tlscacert",
        "--tlscert",
        "--tlskey",
    ],
    inline_value_long_prefixes: &[
        "--context",
        "--config",
        "--host",
        "--log-level",
        "--tlscacert",
        "--tlscert",
        "--tlskey",
    ],
    short_inline_value_chars: "cHl",
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

const CONTAINER_CLI_SUBCOMMAND_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "-d",
        "--detach",
        "-i",
        "--interactive",
        "-t",
        "--tty",
        "--privileged",
        "-P",
        "--publish-all",
        "--rm",
        "--init",
        "--read-only",
        "--no-healthcheck",
        "--disable-content-trust",
        "--sig-proxy",
        "--use-api-socket",
    ],
    short_flag_chars: "ditPq",
    long_options_with_value: &[
        "-a",
        "--attach",
        "-c",
        "--cpu-shares",
        "-e",
        "--env",
        "-h",
        "--hostname",
        "-l",
        "--label",
        "-m",
        "--memory",
        "-p",
        "--publish",
        "-u",
        "--user",
        "-v",
        "--volume",
        "-w",
        "--workdir",
        "--add-host",
        "--annotation",
        "--cap-add",
        "--cap-drop",
        "--cidfile",
        "--cpus",
        "--cpuset-cpus",
        "--cpuset-mems",
        "--detach-keys",
        "--device",
        "--dns",
        "--dns-option",
        "--dns-search",
        "--entrypoint",
        "--env-file",
        "--expose",
        "--gpus",
        "--group-add",
        "--health-cmd",
        "--ip",
        "--ip6",
        "--ipc",
        "--isolation",
        "--label-file",
        "--link",
        "--log-driver",
        "--log-opt",
        "--mount",
        "--name",
        "--network",
        "--network-alias",
        "--pid",
        "--platform",
        "--pull",
        "--restart",
        "--runtime",
        "--security-opt",
        "--shm-size",
        "--stop-signal",
        "--stop-timeout",
        "--sysctl",
        "--tmpfs",
        "--ulimit",
        "--userns",
        "--uts",
        "--volume-driver",
        "--volumes-from",
    ],
    inline_value_long_prefixes: &[
        "--attach",
        "--cpu-shares",
        "--env",
        "--hostname",
        "--label",
        "--memory",
        "--publish",
        "--user",
        "--volume",
        "--workdir",
        "--add-host",
        "--annotation",
        "--cap-add",
        "--cap-drop",
        "--cidfile",
        "--cpus",
        "--cpuset-cpus",
        "--cpuset-mems",
        "--detach-keys",
        "--device",
        "--dns",
        "--dns-option",
        "--dns-search",
        "--entrypoint",
        "--env-file",
        "--expose",
        "--gpus",
        "--group-add",
        "--health-cmd",
        "--ip",
        "--ip6",
        "--ipc",
        "--isolation",
        "--label-file",
        "--link",
        "--log-driver",
        "--log-opt",
        "--mount",
        "--name",
        "--network",
        "--network-alias",
        "--pid",
        "--platform",
        "--pull",
        "--restart",
        "--runtime",
        "--security-opt",
        "--shm-size",
        "--stop-signal",
        "--stop-timeout",
        "--sysctl",
        "--tmpfs",
        "--ulimit",
        "--userns",
        "--uts",
        "--volume-driver",
        "--volumes-from",
    ],
    short_inline_value_chars: "acehlmpuvw",
    forbidden_long: &["--help"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn is_container_cli_command_token(token: &str) -> bool {
    matches!(
        token,
        "docker"
            | "podman"
            | "/usr/bin/docker"
            | "/usr/bin/podman"
            | "/usr/local/bin/docker"
            | "/usr/local/bin/podman"
            | "/opt/homebrew/bin/docker"
            | "/opt/homebrew/bin/podman"
    )
}

fn container_compose_argv_command_body(tokens: &[String], index: usize) -> Option<String> {
    let mut index =
        wrapper_spec::scan_options_block(tokens, index, &CONTAINER_COMPOSE_GLOBAL_OPTIONS)?;
    let subcommand = tokens.get(index)?;
    if !matches!(subcommand.as_str(), "exec" | "run") {
        return None;
    }
    index += 1;

    let mut index =
        wrapper_spec::scan_options_block(tokens, index, &CONTAINER_COMPOSE_EXEC_RUN_OPTIONS)?;
    tokens.get(index)?;
    index += 1;

    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

const CONTAINER_COMPOSE_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &["--compatibility", "--dry-run", "--verbose"],
    long_options_with_value: &[
        "-f",
        "--file",
        "-p",
        "--project-name",
        "--profile",
        "--env-file",
        "--project-directory",
        "--parallel",
        "--ansi",
        "--progress",
    ],
    inline_value_long_prefixes: &[
        "--file",
        "--project-name",
        "--profile",
        "--env-file",
        "--project-directory",
        "--parallel",
        "--ansi",
        "--progress",
    ],
    short_inline_value_chars: "fp",
    forbidden_long: &["--help"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

const CONTAINER_COMPOSE_EXEC_RUN_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "-d",
        "--detach",
        "-T",
        "--no-TTY",
        "--privileged",
        "--rm",
        "--service-ports",
        "--use-aliases",
        "--no-deps",
        "--build",
        "--quiet",
    ],
    long_options_with_value: &[
        "-e",
        "--env",
        "-u",
        "--user",
        "-w",
        "--workdir",
        "--index",
        "--name",
        "--entrypoint",
        "-p",
        "--publish",
        "--pull",
        "-v",
        "--volume",
        "--cap-add",
        "--cap-drop",
        "--label",
    ],
    inline_value_long_prefixes: &[
        "--env",
        "--user",
        "--workdir",
        "--index",
        "--name",
        "--entrypoint",
        "--publish",
        "--pull",
        "--volume",
        "--cap-add",
        "--cap-drop",
        "--label",
    ],
    short_inline_value_chars: "euwpv",
    forbidden_long: &["--help"],
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

pub(in crate::permission_rules) fn kubectl_exec_argv_command_body(
    tokens: &[String],
) -> Option<String> {
    if !tokens
        .first()
        .is_some_and(|token| is_kubectl_or_oc_command_token(token))
    {
        return None;
    }

    let mut index = kubectl_subcommand_index(tokens)?;
    if tokens.get(index).is_none_or(|token| token != "exec") {
        return None;
    }
    index += 1;

    while let Some(token) = tokens.get(index) {
        if token == "--" {
            index += 1;
            break;
        } else if kubectl_exec_flag_option(token) {
            index += 1;
        } else if kubectl_exec_option_takes_value(token) {
            tokens.get(index + 1)?;
            index += 2;
        } else if kubectl_exec_inline_value_option(token) || kubectl_exec_short_flag_group(token) {
            index += 1;
        } else if token.starts_with('-') {
            return None;
        } else {
            index += 1;
        }
    }

    tokens.get(index..).and_then(|body| {
        (!body.is_empty())
            .then(|| body.join(" "))
            .filter(|body| !body.trim().is_empty())
    })
}

pub(in crate::permission_rules) fn kubectl_command_invokes_external_diff_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_kubectl_command_token(token))
    {
        return false;
    }

    let Some(index) = kubectl_subcommand_index(tokens) else {
        return false;
    };
    tokens
        .get(index)
        .is_some_and(|subcommand| subcommand == "diff")
        && !tokens
            .get(index + 1..)
            .unwrap_or_default()
            .iter()
            .any(|token| matches!(token.as_str(), "-h" | "--help"))
}

pub(in crate::permission_rules) fn kubectl_command_invokes_editor_from_tokens(
    tokens: &[String],
) -> bool {
    if !tokens
        .first()
        .is_some_and(|token| is_kubectl_or_oc_command_token(token))
    {
        return false;
    }

    let Some(mut index) = kubectl_subcommand_index(tokens) else {
        return false;
    };
    if tokens
        .get(index)
        .is_none_or(|subcommand| subcommand != "edit")
    {
        return false;
    }
    index += 1;

    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return tokens.get(index + 1).is_some();
        } else if matches!(token.as_str(), "-h" | "--help") {
            return false;
        } else if kubectl_edit_target_option_takes_value(token) {
            return tokens.get(index + 1).is_some();
        } else if kubectl_edit_target_inline_value_option(token) {
            return true;
        } else if kubectl_edit_flag_option(token) {
            index += 1;
        } else if kubectl_edit_option_takes_value(token) {
            if tokens.get(index + 1).is_none() {
                return false;
            }
            index += 2;
        } else if kubectl_edit_inline_value_option(token) {
            index += 1;
        } else {
            return !token.starts_with('-');
        }
    }

    false
}

fn kubectl_subcommand_index(tokens: &[String]) -> Option<usize> {
    let index = wrapper_spec::scan_options_block(tokens, 1, &KUBECTL_GLOBAL_OPTIONS)?;
    tokens.get(index).map(|_| index)
}

const KUBECTL_GLOBAL_OPTIONS: wrapper_spec::WrapperSpec = wrapper_spec::WrapperSpec {
    long_flags: &[
        "--disable-compression",
        "--insecure-skip-tls-verify",
        "--match-server-version",
        "--warnings-as-errors",
    ],
    long_options_with_value: &[
        "-n",
        "--namespace",
        "-s",
        "--server",
        "-v",
        "--v",
        "--as",
        "--as-group",
        "--as-uid",
        "--as-user-extra",
        "--cache-dir",
        "--certificate-authority",
        "--client-certificate",
        "--client-key",
        "--cluster",
        "--context",
        "--kubeconfig",
        "--kuberc",
        "--log-flush-frequency",
        "--password",
        "--profile",
        "--profile-output",
        "--request-timeout",
        "--tls-server-name",
        "--token",
        "--user",
        "--username",
        "--vmodule",
    ],
    inline_value_long_prefixes: &[
        "--namespace",
        "--server",
        "--v",
        "--as",
        "--as-group",
        "--as-uid",
        "--as-user-extra",
        "--cache-dir",
        "--certificate-authority",
        "--client-certificate",
        "--client-key",
        "--cluster",
        "--context",
        "--kubeconfig",
        "--kuberc",
        "--log-flush-frequency",
        "--password",
        "--profile",
        "--profile-output",
        "--request-timeout",
        "--tls-server-name",
        "--token",
        "--user",
        "--username",
        "--vmodule",
    ],
    short_inline_value_chars: "nsv",
    ..wrapper_spec::WrapperSpec::with_aliases(&[])
};

fn kubectl_exec_flag_option(token: &str) -> bool {
    matches!(token, "-i" | "--stdin" | "-q" | "--quiet" | "-t" | "--tty")
}

fn kubectl_exec_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-c" | "--container" | "-f" | "--filename" | "--pod-running-timeout"
    )
}

fn kubectl_exec_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("-c")
        .or_else(|| token.strip_prefix("-f"))
        .is_some_and(|value| !value.is_empty())
        || token
            .strip_prefix("--container=")
            .or_else(|| token.strip_prefix("--filename="))
            .or_else(|| token.strip_prefix("--pod-running-timeout="))
            .is_some_and(|value| !value.is_empty())
}

fn kubectl_exec_short_flag_group(token: &str) -> bool {
    token.starts_with('-')
        && !token.starts_with("--")
        && token.len() > 2
        && token
            .trim_start_matches('-')
            .chars()
            .all(|character| matches!(character, 'i' | 'q' | 't'))
}

fn kubectl_edit_flag_option(token: &str) -> bool {
    matches!(
        token,
        "--save-config"
            | "--show-managed-fields"
            | "--allow-missing-template-keys"
            | "--windows-line-endings"
    )
}

fn kubectl_edit_target_option_takes_value(token: &str) -> bool {
    matches!(token, "--filename" | "-f" | "--kustomize" | "-k")
}

fn kubectl_edit_target_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--filename=")
        .or_else(|| token.strip_prefix("-f").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--kustomize="))
        .or_else(|| token.strip_prefix("-k").filter(|value| !value.is_empty()))
        .is_some_and(|value| !value.is_empty())
}

fn kubectl_edit_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "--output" | "-o" | "--field-manager" | "--subresource" | "--template" | "--validate"
    )
}

fn kubectl_edit_inline_value_option(token: &str) -> bool {
    token
        .strip_prefix("--output=")
        .or_else(|| token.strip_prefix("-o").filter(|value| !value.is_empty()))
        .or_else(|| token.strip_prefix("--field-manager="))
        .or_else(|| token.strip_prefix("--subresource="))
        .or_else(|| token.strip_prefix("--template="))
        .or_else(|| token.strip_prefix("--validate="))
        .is_some_and(|value| !value.is_empty())
}

fn is_kubectl_command_token(token: &str) -> bool {
    matches!(
        token,
        "kubectl" | "/usr/bin/kubectl" | "/usr/local/bin/kubectl" | "/opt/homebrew/bin/kubectl"
    )
}

fn is_kubectl_or_oc_command_token(token: &str) -> bool {
    is_kubectl_command_token(token)
        || matches!(
            token,
            "oc" | "/usr/bin/oc" | "/usr/local/bin/oc" | "/opt/homebrew/bin/oc"
        )
}
