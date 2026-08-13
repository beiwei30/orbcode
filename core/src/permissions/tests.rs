use orbcode_config::parse_tool_rule_list;
use orbcode_protocol::ModelPermissionPreset;
use orbcode_tools::ToolRegistry;

use super::{
    PermissionBoundaryReason, PermissionContext, PermissionEvaluation, PermissionGrantSource,
    PermissionRule, normalize_permission_rule_for_edit, suggested_bash_permission_rules,
};

#[test]
fn preset_evaluator_applies_workspace_and_network_boundaries() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&outside).expect("outside");
    let permissions = PermissionContext {
        cwd: workspace.clone(),
        allow_tools: true,
        ..PermissionContext::default()
    };
    let registry = ToolRegistry::foundation();
    let write = registry.spec("file-write").expect("write spec");
    let bash = registry.spec("bash").expect("bash spec");
    let network = registry.spec("web-fetch").expect("network spec");

    assert_eq!(
        permissions.evaluate_tool_call(
            Some(ModelPermissionPreset::AskForApproval.policy()),
            write,
            "file-write",
            r#"{"file_path":"src/lib.rs","content":"x"}"#,
            false,
            false,
        ),
        PermissionEvaluation::Allow {
            source: PermissionGrantSource::WorkspaceBoundary
        }
    );
    let outside_input = serde_json::json!({
        "file_path": outside.join("new.txt"),
        "content": "x"
    })
    .to_string();
    assert!(matches!(
        permissions.evaluate_tool_call(
            Some(ModelPermissionPreset::AskForApproval.policy()),
            write,
            "file-write",
            &outside_input,
            false,
            false,
        ),
        PermissionEvaluation::AskUser {
            reason: PermissionBoundaryReason::OutsideWorkspace { .. }
        }
    ));
    assert!(matches!(
        permissions.evaluate_tool_call(
            Some(ModelPermissionPreset::ApproveForMe.policy()),
            write,
            "file-write",
            &outside_input,
            false,
            false,
        ),
        PermissionEvaluation::AutoReview {
            reason: PermissionBoundaryReason::OutsideWorkspace { .. }
        }
    ));
    assert_eq!(
        permissions.evaluate_tool_call(
            Some(ModelPermissionPreset::FullAccess.policy()),
            write,
            "file-write",
            &outside_input,
            false,
            false,
        ),
        PermissionEvaluation::Allow {
            source: PermissionGrantSource::FullAccess
        }
    );
    assert_eq!(
        permissions.evaluate_tool_call(
            Some(ModelPermissionPreset::AskForApproval.policy()),
            bash,
            "bash",
            r#"{"command":"cargo test"}"#,
            false,
            false,
        ),
        PermissionEvaluation::Allow {
            source: PermissionGrantSource::WorkspaceBoundary
        }
    );
    assert!(matches!(
        permissions.evaluate_tool_call(
            Some(ModelPermissionPreset::AskForApproval.policy()),
            bash,
            "bash",
            r#"{"command":"cargo test","sandbox_permissions":"require_escalated"}"#,
            false,
            false,
        ),
        PermissionEvaluation::AskUser {
            reason: PermissionBoundaryReason::SandboxEscalation
        }
    ));
    assert!(matches!(
        permissions.evaluate_tool_call(
            Some(ModelPermissionPreset::AskForApproval.policy()),
            network,
            "web-fetch",
            r#"{"url":"https://example.com"}"#,
            false,
            false,
        ),
        PermissionEvaluation::AskUser {
            reason: PermissionBoundaryReason::Network
        }
    ));
}

#[test]
fn explicit_rules_and_hook_authorization_keep_precedence_over_presets() {
    let registry = ToolRegistry::foundation();
    let network = registry.spec("web-fetch").expect("network spec");
    let input = r#"{"url":"https://example.com"}"#;
    let deny = PermissionContext {
        denied_rules: vec![PermissionRule::parse("WebFetch")],
        ..PermissionContext::default()
    };
    assert!(matches!(
        deny.evaluate_tool_call(
            Some(ModelPermissionPreset::FullAccess.policy()),
            network,
            "web-fetch",
            input,
            true,
            true,
        ),
        PermissionEvaluation::Deny { .. }
    ));

    let ask = PermissionContext {
        ask_rules: vec![PermissionRule::parse("WebFetch")],
        ..PermissionContext::default()
    };
    assert!(matches!(
        ask.evaluate_tool_call(
            Some(ModelPermissionPreset::FullAccess.policy()),
            network,
            "web-fetch",
            input,
            false,
            false,
        ),
        PermissionEvaluation::AskUser {
            reason: PermissionBoundaryReason::ExplicitAskRule
        }
    ));
    assert_eq!(
        ask.evaluate_tool_call(
            Some(ModelPermissionPreset::AskForApproval.policy()),
            network,
            "web-fetch",
            input,
            true,
            false,
        ),
        PermissionEvaluation::Allow {
            source: PermissionGrantSource::Hook
        }
    );

    let allow = PermissionContext {
        allowed_rules: vec![PermissionRule::parse("WebFetch")],
        ..PermissionContext::default()
    };
    assert_eq!(
        allow.evaluate_tool_call(
            Some(ModelPermissionPreset::AskForApproval.policy()),
            network,
            "web-fetch",
            input,
            false,
            false,
        ),
        PermissionEvaluation::Allow {
            source: PermissionGrantSource::ConfiguredRule
        }
    );
}

#[test]
fn ask_rule_matches_tool_call_to_force_prompt() {
    let permissions = PermissionContext {
        ask_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        ..PermissionContext::default()
    };
    assert!(permissions.tool_should_ask("bash", r#"{"command":"rm -rf /tmp/x"}"#));
    assert!(!permissions.tool_should_ask("bash", r#"{"command":"ls"}"#));
}

#[test]
fn ask_rule_overlapping_allow_must_still_force_prompt() {
    // An `ask` rule and an `allow` rule can both match the same command. The
    // auto-approve fast paths (`tool_allowed_without_prompt` / config-allow) then
    // return "allowed", so every execution path — the streamed fast path AND the
    // hook/fallback path in `tool_runtime` — must check `tool_should_ask` FIRST
    // (deny > ask > allow). This documents the overlap the gate guards against.
    let permissions = PermissionContext {
        ask_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        allowed_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        allow_tools: true,
        allow_network: true,
        provider_allow_network: true,
        ..PermissionContext::default()
    };
    let rm = r#"{"command":"rm -rf /tmp/x"}"#;
    assert!(
        permissions.tool_allowed_without_prompt("bash", rm),
        "the overlapping allow rule matches, so relying on allow alone auto-runs"
    );
    assert!(
        permissions.tool_should_ask("bash", rm),
        "the ask rule also matches and must take precedence over the allow"
    );
}

#[test]
fn parses_cli_tool_lists_without_splitting_rule_content() {
    let rules = parse_tool_rule_list(&["Bash(npm run test), Read Edit(src/**)".to_string()]);
    assert_eq!(rules, vec!["Bash(npm run test)", "Read", "Edit(src/**)"]);
}

#[test]
fn parses_permission_rules_with_escaped_parentheses() {
    let rule = PermissionRule::parse(r#"Bash(python -c "print\(1\)")"#);
    assert_eq!(rule.tool_name, "bash");
    assert_eq!(
        rule.rule_content,
        Some(r#"python -c "print(1)""#.to_string())
    );
}

#[test]
fn validates_editable_permission_rule_shape() {
    assert_eq!(
        normalize_permission_rule_for_edit(" Read(src/**) ").expect("valid rule"),
        "Read(src/**)"
    );
    assert!(normalize_permission_rule_for_edit("Bash(").is_err());
    assert!(normalize_permission_rule_for_edit("(src/**)").is_err());
    assert!(normalize_permission_rule_for_edit("Read(src/**) trailing").is_err());
}

#[test]
fn matches_exact_bash_rules_with_escaped_literal_wildcards() {
    let rule = PermissionRule::parse(r#"Bash(echo "\*.rs")"#);

    assert!(rule.matches_tool_call("bash", r#"{"command":"echo \"*.rs\""}"#));
    assert!(!rule.matches_tool_call("bash", r#"{"command":"echo \"lib.rs\""}"#));
}

#[test]
fn matches_bash_prefix_rules_against_full_command_prefixes() {
    let rule = PermissionRule::parse("Bash(npm run:*)");
    assert!(rule.matches_tool_call("bash", r#"{"command":"npm run test"}"#));
    assert!(!rule.matches_tool_call("bash", r#"{"command":"npmx run test"}"#));
}

#[test]
fn bash_allow_rules_do_not_match_compound_commands_by_prefix() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::parse("Bash(git:*)")],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    assert!(
        permissions
            .tool_allowed("bash", r#"{"command":"git status && rm -rf /tmp/example"}"#)
            .is_none()
    );
}

#[test]
fn bash_allow_rules_do_not_auto_allow_sandbox_escalation() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::parse("Bash(echo:*)")],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"echo hi","sandbox_permissions":"require_escalated"}"#
            )
            .is_none()
    );
}

#[test]
fn bash_allow_rules_can_cover_each_atomic_subcommand() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![
            PermissionRule::parse("Bash(git status:*)"),
            PermissionRule::parse("Bash(cargo test:*)"),
        ],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"git status && cargo test --manifest-path orbcode/Cargo.toml"}"#
            )
            .is_some()
    );
}

#[test]
fn bash_grouped_suggestions_cover_compound_commands() {
    let command = r#"echo "=== app-server ===" && find orbcode/app-server/src -name "*.rs" | xargs wc -l 2>/dev/null | tail -1"#;

    assert_eq!(
        suggested_bash_permission_rules(command),
        vec![
            "echo:*".to_string(),
            "find:*".to_string(),
            r"xargs wc -l 2>/dev/null".to_string(),
            "tail:*".to_string(),
        ]
    );
}

#[test]
fn bash_grouped_rules_allow_later_matching_compound_commands() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![
            PermissionRule::parse("Bash(echo:*)"),
            PermissionRule::parse("Bash(find:*)"),
            PermissionRule::parse("Bash(xargs wc -l 2>/dev/null)"),
            PermissionRule::parse("Bash(tail:*)"),
        ],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"echo ok && find orbcode -name \"*.rs\" | xargs wc -l 2>/dev/null | tail -1"}"#
            )
            .is_some()
    );
}

#[test]
fn bash_grouped_suggestions_keep_substitution_as_exact_command() {
    let command = "echo $(date) && cargo test";

    assert_eq!(
        suggested_bash_permission_rules(command),
        vec!["echo $(date) && cargo test".to_string()]
    );
}

#[test]
fn bash_suggestions_keep_argv_execution_forms_exact() {
    assert_eq!(
        suggested_bash_permission_rules(r"find . -exec echo ok \;"),
        vec![r"find . -exec echo ok \\;".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"find . -ok echo ok \;"),
        vec![r"find . -ok echo ok \\;".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"parallel 'echo ok' ::: target"),
        vec![r"parallel 'echo ok' ::: target".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"ssh example.com echo ok"),
        vec![r"ssh example.com echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"sshpass -p secret ssh example.com echo ok"),
        vec![r"sshpass -p secret ssh example.com echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"ssh -o ProxyCommand='echo ok' example.com"),
        vec![r"ssh -o ProxyCommand='echo ok' example.com".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"scp -S 'echo ok' src host:dst"),
        vec![r"scp -S 'echo ok' src host:dst".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"git bisect run echo ok"),
        vec![r"git bisect run echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"npm exec -- echo ok"),
        vec![r"npm exec -- echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"npx echo ok"),
        vec![r"npx echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"pnpm exec echo ok"),
        vec![r"pnpm exec echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"pnpm dlx echo ok"),
        vec![r"pnpm dlx echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"pnpx echo ok"),
        vec![r"pnpx echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"yarn exec echo ok"),
        vec![r"yarn exec echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"yarn dlx echo ok"),
        vec![r"yarn dlx echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"bunx echo ok"),
        vec![r"bunx echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"bun x echo ok"),
        vec![r"bun x echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"poetry run echo ok"),
        vec![r"poetry run echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"pipenv run echo ok"),
        vec![r"pipenv run echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"uv run echo ok"),
        vec![r"uv run echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"uvx echo ok"),
        vec![r"uvx echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"pdm run echo ok"),
        vec![r"pdm run echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"hatch run echo ok"),
        vec![r"hatch run echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"conda run -n env echo ok"),
        vec![r"conda run -n env echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"mamba run -n env echo ok"),
        vec![r"mamba run -n env echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"micromamba run -n env echo ok"),
        vec![r"micromamba run -n env echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"bundle exec echo ok"),
        vec![r"bundle exec echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"rbenv exec echo ok"),
        vec![r"rbenv exec echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"pyenv exec echo ok"),
        vec![r"pyenv exec echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"asdf exec echo ok"),
        vec![r"asdf exec echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"direnv exec . echo ok"),
        vec![r"direnv exec . echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"nix develop . --command echo ok"),
        vec![r"nix develop . --command echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"nix shell nixpkgs#coreutils -c echo ok"),
        vec![r"nix shell nixpkgs#coreutils -c echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"guix shell -- echo ok"),
        vec![r"guix shell -- echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"watchexec -- echo ok"),
        vec![r"watchexec -- echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"entr echo ok"),
        vec![r"entr echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"screen -dmS cleanup echo ok"),
        vec![r"screen -dmS cleanup echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"docker exec app echo ok"),
        vec![r"docker exec app echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"docker run --rm alpine echo ok"),
        vec![r"docker run --rm alpine echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"docker compose exec app echo ok"),
        vec![r"docker compose exec app echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"kubectl exec pod/app -- echo ok"),
        vec![r"kubectl exec pod/app -- echo ok".to_string()]
    );
    assert_eq!(
        suggested_bash_permission_rules(r"socat - 'EXEC:echo ok'"),
        vec![r"socat - 'EXEC:echo ok'".to_string()]
    );
}

#[test]
fn bash_suggestions_keep_shell_control_compounds_exact() {
    for command in [
        "if true; then echo ok; fi",
        "while true; do echo ok; break; done",
        "for path in src tests; do echo \"$path\"; done",
        "select choice in yes no; do echo \"$choice\"; break; done",
        "case \"$target\" in safe) echo ok ;; esac",
        "bash -c 'echo ok'",
        "sh -lc 'echo ok'",
        "cleanup(){ echo ok; }; cleanup",
        "function cleanup { echo ok; }",
        "eval 'echo ok'",
        "coproc cleanup { echo ok; }",
        "trap 'echo ok' EXIT",
        "sg adm -c 'echo ok'",
        "git -c alias.cleanup='!echo ok' cleanup",
        "git submodule foreach 'echo ok'",
        "git filter-branch --tree-filter 'echo ok' HEAD",
        "git rebase --exec 'echo ok' main",
        "npm exec -c 'echo ok'",
        "npx --call='echo ok'",
        "nix-shell --run 'echo ok'",
        "tmux new-session -d -s cleanup 'echo ok'",
        "tmux split-window -h 'echo ok'",
        "time { echo ok; }",
        "time (echo ok)",
    ] {
        assert_eq!(
            suggested_bash_permission_rules(command),
            vec![command.to_string()],
            "{command}"
        );
    }
}

#[test]
fn bash_allow_rules_match_exact_compound_commands_with_escaped_literals() {
    let command = r#"grep -B 2 -A 10 "ToolSpec\s*\{" orbcode/tools/src/lib.rs | head -100"#;
    let escaped_rule = r#"grep -B 2 -A 10 "ToolSpec\\s\*\\{" orbcode/tools/src/lib.rs | head -100"#;
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::for_tool("bash", escaped_rule)],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    assert!(
        permissions
            .tool_allowed(
                "bash",
                &serde_json::json!({ "command": command }).to_string()
            )
            .is_some()
    );
    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"grep -B 2 -A 10 \"ToolSpec\\s*\\{\" orbcode/tools/src/lib.rs"}"#
            )
            .is_none()
    );
}

#[test]
fn bash_deny_rules_match_compound_subcommands() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::parse("Bash(echo:*)")],
        denied_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        ..PermissionContext::default()
    };

    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"echo ok && rm -rf /tmp/example"}"#)
            .is_some()
    );
}

#[test]
fn bash_deny_rules_match_nested_shell_structures() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::parse("Bash(echo:*)")],
        denied_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        ..PermissionContext::default()
    };

    for command in [
        "echo ok && (rm -rf /tmp/example)",
        "echo ok && { rm -rf /tmp/example; }",
        "echo $(rm -rf /tmp/example)",
        r#"echo "$(rm -rf /tmp/example)""#,
        "echo `rm -rf /tmp/example`",
        r#"echo "`rm -rf /tmp/example`""#,
        "echo $(( $(rm -rf /tmp/example) + 1 ))",
        "echo $(( `rm -rf /tmp/example` + 1 ))",
        r#"echo "$(( $(rm -rf /tmp/example) + 1 ))""#,
        "echo $[ $(rm -rf /tmp/example) + 1 ]",
        "echo $[ `rm -rf /tmp/example` + 1 ]",
        r#"echo "$[ $(rm -rf /tmp/example) + 1 ]""#,
        "echo ${missing:-$(rm -rf /tmp/example)}",
        "echo ${missing:-`rm -rf /tmp/example`}",
        r#"echo "${missing:-$(rm -rf /tmp/example)}""#,
        "cat <(rm -rf /tmp/example)",
        "cat >(rm -rf /tmp/example)",
        "echo ok > >(rm -rf /tmp/example)",
        "! rm -rf /tmp/example",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_some(),
            "{command}"
        );
    }
}

#[test]
fn bash_deny_rules_match_function_definition_bodies() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: Vec::new(),
        denied_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        ..PermissionContext::default()
    };

    for command in [
        "cleanup(){ rm -rf /tmp/example; }; cleanup",
        "cleanup() { rm -rf /tmp/example; }",
        "function cleanup { rm -rf /tmp/example; }",
        "function cleanup() { echo ok && rm -rf /tmp/example; }",
        "coproc cleanup { rm -rf /tmp/example; }",
        "coproc { echo ok && rm -rf /tmp/example; }",
        "time { rm -rf /tmp/example; }",
        "time (rm -rf /tmp/example)",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_some(),
            "{command}"
        );
    }
}

#[test]
fn bash_deny_rules_ignore_quoted_nested_shell_syntax() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: Vec::new(),
        denied_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        ..PermissionContext::default()
    };

    for command in [
        "echo '$(rm -rf /tmp/example)'",
        "echo \"literal (rm -rf /tmp/example)\"",
        "printf '%s' '<(rm -rf /tmp/example)'",
        "echo $((rm + 1))",
        "echo $((rm - rf))",
        "echo $[rm + 1]",
        "echo $[rm - rf]",
        "echo ${missing:-rm -rf /tmp/example}",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
}

#[test]
fn bash_deny_rules_handle_heredoc_body_expansion_semantics() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: Vec::new(),
        denied_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        ..PermissionContext::default()
    };

    for command in [
        "cat <<EOF\n$(rm -rf /tmp/example)\nEOF",
        "cat <<EOF\n`rm -rf /tmp/example`\nEOF",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_some(),
            "{command}"
        );
    }

    for command in [
        "cat <<EOF\nrm -rf /tmp/example\nEOF",
        "cat <<'EOF'\n$(rm -rf /tmp/example)\nEOF",
        "cat <<\"EOF\"\n`rm -rf /tmp/example`\nEOF",
        "cat <<\\EOF\n$(rm -rf /tmp/example)\nEOF",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
}

#[test]
fn bash_deny_rules_match_shell_stdin_heredoc_scripts() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: Vec::new(),
        denied_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        ..PermissionContext::default()
    };

    for command in [
        "bash <<EOF\nrm -rf /tmp/example\nEOF",
        "bash <<'EOF'\nrm -rf /tmp/example\nEOF",
        "sh -s <<EOF\nrm -rf /tmp/example\nEOF",
        "bash -s -- cleanup <<EOF\nrm -rf /tmp/example\nEOF",
        "bash -s -O extglob cleanup <<EOF\nrm -rf /tmp/example\nEOF",
        "bash -O extglob <<EOF\nrm -rf /tmp/example\nEOF",
        "/opt/homebrew/bin/bash -O extglob <<EOF\nrm -rf /tmp/example\nEOF",
        "bash --norc <<EOF\nrm -rf /tmp/example\nEOF",
        "bash --rcfile /tmp/bashrc <<EOF\nrm -rf /tmp/example\nEOF",
        "bash --rcfile=/tmp/bashrc <<EOF\nrm -rf /tmp/example\nEOF",
        "/usr/local/bin/zsh <<EOF\nrm -rf /tmp/example\nEOF",
        "env FOO=bar bash <<EOF\nrm -rf /tmp/example\nEOF",
        "command bash <<EOF\nrm -rf /tmp/example\nEOF",
        "source /dev/stdin <<EOF\nrm -rf /tmp/example\nEOF",
        ". /dev/stdin <<EOF\nrm -rf /tmp/example\nEOF",
        "at now <<EOF\nrm -rf /tmp/example\nEOF",
        "at -q a now <<EOF\nrm -rf /tmp/example\nEOF",
        "/usr/local/bin/at now <<EOF\nrm -rf /tmp/example\nEOF",
        "/opt/homebrew/bin/at now <<EOF\nrm -rf /tmp/example\nEOF",
        "batch <<EOF\nrm -rf /tmp/example\nEOF",
        "/usr/local/bin/batch <<EOF\nrm -rf /tmp/example\nEOF",
        "/opt/homebrew/bin/batch <<EOF\nrm -rf /tmp/example\nEOF",
        "crontab <<EOF\n* * * * * rm -rf /tmp/example\nEOF",
        "crontab - <<EOF\n@reboot rm -rf /tmp/example\nEOF",
        "crontab -u root - <<EOF\nMAILTO=root\n* * * * * echo ok && rm -rf /tmp/example\nEOF",
        "/usr/local/bin/crontab -u root - <<EOF\nMAILTO=root\n* * * * * echo ok && rm -rf /tmp/example\nEOF",
        "/opt/homebrew/bin/crontab - <<EOF\n@daily rm -rf /tmp/example\nEOF",
        "cat <<A; bash <<B\necho ok\nA\nrm -rf /tmp/example\nB",
        "cat <<A; at now <<B\necho ok\nA\nrm -rf /tmp/example\nB",
        "cat <<A; crontab <<B\necho ok\nA\n* * * * * rm -rf /tmp/example\nB",
        "<<EOF bash\nrm -rf /tmp/example\nEOF",
        "<<EOF at now\nrm -rf /tmp/example\nEOF",
        "<<EOF crontab\n* * * * * rm -rf /tmp/example\nEOF",
        "<<EOF source /dev/stdin\nrm -rf /tmp/example\nEOF",
        "bash /dev/fd/3 3<<EOF\nrm -rf /tmp/example\nEOF",
        "source /dev/fd/3 3<<EOF\nrm -rf /tmp/example\nEOF",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_some(),
            "{command}"
        );
    }

    assert!(
        permissions
            .tool_denied(
                "bash",
                &serde_json::json!({
                    "command": "bash script.sh <<EOF\nrm -rf /tmp/example\nEOF"
                })
                .to_string()
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                &serde_json::json!({
                    "command": "at -f /tmp/job now <<EOF\nrm -rf /tmp/example\nEOF"
                })
                .to_string()
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                &serde_json::json!({
                    "command": "/opt/homebrew/bin/at -f /tmp/job now <<EOF\nrm -rf /tmp/example\nEOF"
                })
                .to_string()
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                &serde_json::json!({
                    "command": "/usr/local/bin/batch -f /tmp/job <<EOF\nrm -rf /tmp/example\nEOF"
                })
                .to_string()
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                &serde_json::json!({
                    "command": "crontab -l <<EOF\n* * * * * rm -rf /tmp/example\nEOF"
                })
                .to_string()
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                &serde_json::json!({
                    "command": "/opt/homebrew/bin/crontab -l <<EOF\n* * * * * rm -rf /tmp/example\nEOF"
                })
                .to_string()
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                &serde_json::json!({
                    "command": "/usr/local/bin/crontab -r <<EOF\n* * * * * rm -rf /tmp/example\nEOF"
                })
                .to_string()
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                &serde_json::json!({
                    "command": "cat <<EOF\n* * * * * rm -rf /tmp/example\nEOF"
                })
                .to_string()
            )
            .is_none()
    );

    assert!(
        permissions
            .tool_denied(
                "bash",
                &serde_json::json!({
                    "command": "bash <<A; cat <<B\necho ok\nA\nrm -rf /tmp/example\nB"
                })
                .to_string()
            )
            .is_none()
    );
}

#[test]
fn bash_deny_rules_match_shell_stdin_here_strings() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: Vec::new(),
        denied_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        ..PermissionContext::default()
    };

    for command in [
        "bash <<< 'rm -rf /tmp/example'",
        "bash <<< $'rm -rf /tmp/example'",
        "sh -s <<< 'rm -rf /tmp/example'",
        "bash -s cleanup <<< 'rm -rf /tmp/example'",
        "bash -s -O extglob cleanup <<< 'rm -rf /tmp/example'",
        "bash -o errexit <<< 'rm -rf /tmp/example'",
        "bash --norc <<< 'rm -rf /tmp/example'",
        "/opt/homebrew/bin/bash --norc <<< 'rm -rf /tmp/example'",
        "bash --rcfile /tmp/bashrc <<< 'rm -rf /tmp/example'",
        "bash --rcfile=/tmp/bashrc <<< 'rm -rf /tmp/example'",
        "/usr/local/bin/zsh <<< 'rm -rf /tmp/example'",
        "env FOO=bar bash <<< 'rm -rf /tmp/example'",
        "command bash <<< 'rm -rf /tmp/example'",
        "source /dev/stdin <<< 'rm -rf /tmp/example'",
        ". /dev/stdin <<< 'rm -rf /tmp/example'",
        "at now <<< 'rm -rf /tmp/example'",
        "/usr/local/bin/at now <<< 'rm -rf /tmp/example'",
        "/opt/homebrew/bin/at now <<< 'rm -rf /tmp/example'",
        "batch <<< 'rm -rf /tmp/example'",
        "/usr/local/bin/batch <<< 'rm -rf /tmp/example'",
        "/opt/homebrew/bin/batch <<< 'rm -rf /tmp/example'",
        "crontab <<< '* * * * * rm -rf /tmp/example'",
        "crontab - <<< '@daily echo ok && rm -rf /tmp/example'",
        "/usr/local/bin/crontab - <<< '@daily echo ok && rm -rf /tmp/example'",
        "/opt/homebrew/bin/crontab <<< '@hourly rm -rf /tmp/example'",
        "cat <<< 'echo ok'; bash <<< 'rm -rf /tmp/example'",
        "cat <<< 'echo ok'; at now <<< 'rm -rf /tmp/example'",
        "cat <<< 'echo ok'; crontab <<< '* * * * * rm -rf /tmp/example'",
        "<<< 'rm -rf /tmp/example' bash",
        "<<< 'rm -rf /tmp/example' at now",
        "<<< '* * * * * rm -rf /tmp/example' crontab",
        "<<< 'rm -rf /tmp/example' source /dev/stdin",
        "bash /dev/fd/3 3<<< 'rm -rf /tmp/example'",
        "source /dev/fd/3 3<<< 'rm -rf /tmp/example'",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_some(),
            "{command}"
        );
    }

    for command in [
        "cat <<< 'rm -rf /tmp/example'",
        "cat <<< $'rm -rf /tmp/example'",
        "bash script.sh <<< 'rm -rf /tmp/example'",
        "source script.sh <<< 'rm -rf /tmp/example'",
        "source /dev/fd/4 3<<< 'rm -rf /tmp/example'",
        "at -f /tmp/job now <<< 'rm -rf /tmp/example'",
        "/opt/homebrew/bin/at -f /tmp/job now <<< 'rm -rf /tmp/example'",
        "crontab -l <<< '* * * * * rm -rf /tmp/example'",
        "/opt/homebrew/bin/crontab -l <<< '* * * * * rm -rf /tmp/example'",
        "cat <<< '* * * * * rm -rf /tmp/example'",
        "bash <<< 'echo ok'; cat <<< 'rm -rf /tmp/example'",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
}

#[test]
fn bash_deny_rules_match_shell_control_keyword_bodies() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: Vec::new(),
        denied_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        ..PermissionContext::default()
    };

    for command in [
        "if true; then rm -rf /tmp/example; fi",
        "if rm -rf /tmp/example; then echo removed; fi",
        "if false; then echo ok; else rm -rf /tmp/example; fi",
        "while true; do rm -rf /tmp/example; break; done",
        "until false; do rm -rf /tmp/example; done",
        "for path in src tests; do rm -rf /tmp/example; done",
        "select choice in yes no; do rm -rf /tmp/example; break; done",
        "elif rm -rf /tmp/example; then echo removed",
        "case \"$target\" in danger) rm -rf /tmp/example ;; *) echo ok ;; esac",
        "case \"$target\" in safe) echo ok ;; danger) rm -rf /tmp/example ;; esac",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_some(),
            "{command}"
        );
    }
}

#[test]
fn bash_deny_rules_strip_env_vars_and_safe_wrappers() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: Vec::new(),
        denied_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        ..PermissionContext::default()
    };

    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"FOO=bar timeout 5 nice -n 1 rm -rf /tmp/example"}"#
            )
            .is_some()
    );

    for command in [
        "env FOO=bar rm -rf /tmp/example",
        "env -i FOO=bar rm -rf /tmp/example",
        "env --ignore-environment FOO=bar rm -rf /tmp/example",
        "env -a cleanup rm -rf /tmp/example",
        "env --argv0=cleanup rm -rf /tmp/example",
        "env -C /tmp FOO=bar rm -rf /tmp/example",
        "env --chdir=/tmp FOO=bar rm -rf /tmp/example",
        "env -u FOO BAR=baz rm -rf /tmp/example",
        "env -S 'rm -rf /tmp/example'",
        "env -i -S 'FOO=bar sudo -n rm -rf /tmp/example'",
        "env --split-string='rm -rf /tmp/example'",
        "/usr/bin/env FOO=bar rm -rf /tmp/example",
        "/usr/bin/env -S 'sudo -n rm -rf /tmp/example'",
        "/usr/local/bin/env FOO=bar rm -rf /tmp/example",
        "/opt/homebrew/bin/env -S 'rm -rf /tmp/example'",
        "command rm -rf /tmp/example",
        "command -p rm -rf /tmp/example",
        "command -p -- rm -rf /tmp/example",
        "command -- rm -rf /tmp/example",
        "exec rm -rf /tmp/example",
        "exec -a cleanup rm -rf /tmp/example",
        "exec -- rm -rf /tmp/example",
        "sudo rm -rf /tmp/example",
        "sudo -E -n rm -rf /tmp/example",
        "sudo -u root -g wheel rm -rf /tmp/example",
        "sudo -uroot -gwheel rm -rf /tmp/example",
        "sudo --user=root -- rm -rf /tmp/example",
        "sudo --set-home rm -rf /tmp/example",
        "sudo -i rm -rf /tmp/example",
        "sudo --login rm -rf /tmp/example",
        "sudo --non-interactive rm -rf /tmp/example",
        "sudo --preserve-env=PATH rm -rf /tmp/example",
        "/usr/bin/sudo -H rm -rf /tmp/example",
        "/usr/local/bin/sudo -H rm -rf /tmp/example",
        "/opt/homebrew/bin/sudo --non-interactive rm -rf /tmp/example",
        "SUDO_ASKPASS='rm -rf /tmp/example' sudo -A echo ok",
        "SUDO_ASKPASS='echo ok && rm -rf /tmp/example' /usr/bin/sudo --askpass echo ok",
        "SUDO_ASKPASS='echo ok && rm -rf /tmp/example' /usr/local/bin/sudo --askpass echo ok",
        "SUDO_ASKPASS='rm -rf /tmp/example' /opt/homebrew/bin/sudo -A echo ok",
        "su -c 'rm -rf /tmp/example'",
        "/usr/local/bin/su -c 'rm -rf /tmp/example'",
        "su root -c 'rm -rf /tmp/example'",
        "su --command='rm -rf /tmp/example' root",
        "su --session-command='rm -rf /tmp/example' root",
        "runuser -u root -- rm -rf /tmp/example",
        "runuser -uroot rm -rf /tmp/example",
        "/usr/sbin/runuser --user root rm -rf /tmp/example",
        "/opt/homebrew/bin/runuser --user root rm -rf /tmp/example",
        "runuser -u root -c 'rm -rf /tmp/example'",
        "/usr/sbin/runuser --command='rm -rf /tmp/example' root",
        "/usr/local/bin/runuser --command='rm -rf /tmp/example' root",
        "sg adm -c 'rm -rf /tmp/example'",
        "sg -l adm -c 'rm -rf /tmp/example'",
        "sg - adm 'rm -rf /tmp/example'",
        "/usr/bin/sg adm -c 'echo ok && rm -rf /tmp/example'",
        "/opt/homebrew/bin/sg adm -c 'echo ok && rm -rf /tmp/example'",
        "setpriv --reuid 0 --regid 0 --clear-groups rm -rf /tmp/example",
        "setpriv --no-new-privs --reset-env -- rm -rf /tmp/example",
        "/usr/bin/setpriv --inh-caps=-all --ambient-caps=-all rm -rf /tmp/example",
        "/usr/local/bin/setpriv --no-new-privs --reset-env -- rm -rf /tmp/example",
        "prlimit --nofile=1024:2048 rm -rf /tmp/example",
        "prlimit --cpu=10 --as=1G -- rm -rf /tmp/example",
        "/usr/bin/prlimit --verbose --output=RESOURCE rm -rf /tmp/example",
        "/opt/homebrew/bin/prlimit --nofile=1024:2048 rm -rf /tmp/example",
        "numactl --cpunodebind=0 --membind=0 rm -rf /tmp/example",
        "numactl --localalloc --physcpubind 0-3 -- rm -rf /tmp/example",
        "/usr/bin/numactl -N0 -m0 rm -rf /tmp/example",
        "/opt/homebrew/bin/numactl --localalloc -- rm -rf /tmp/example",
        "setarch x86_64 -R rm -rf /tmp/example",
        "setarch --addr-no-randomize rm -rf /tmp/example",
        "setarch --uname-2.6 -- rm -rf /tmp/example",
        "/usr/local/bin/setarch x86_64 -R rm -rf /tmp/example",
        "linux32 -B rm -rf /tmp/example",
        "/usr/bin/linux64 --uname-2.6 -- rm -rf /tmp/example",
        "/opt/homebrew/bin/linux64 --uname-2.6 -- rm -rf /tmp/example",
        "doas rm -rf /tmp/example",
        "doas -u root -- rm -rf /tmp/example",
        "doas -uroot -- rm -rf /tmp/example",
        "/usr/bin/doas -u root -- rm -rf /tmp/example",
        "/usr/local/bin/doas -u root -- rm -rf /tmp/example",
        "pkexec rm -rf /tmp/example",
        "pkexec --user root rm -rf /tmp/example",
        "pkexec --disable-internal-agent rm -rf /tmp/example",
        "/usr/bin/pkexec --user root rm -rf /tmp/example",
        "/opt/homebrew/bin/pkexec --user root rm -rf /tmp/example",
        "chroot /tmp/root rm -rf /tmp/example",
        "/bin/chroot /tmp/root rm -rf /tmp/example",
        "/usr/bin/chroot /tmp/root rm -rf /tmp/example",
        "/usr/sbin/chroot /tmp/root rm -rf /tmp/example",
        "/opt/homebrew/bin/chroot /tmp/root rm -rf /tmp/example",
        "chroot --userspec=0:0 /tmp/root rm -rf /tmp/example",
        "chroot --groups wheel --skip-chdir /tmp/root rm -rf /tmp/example",
        "flock /tmp/lock rm -rf /tmp/example",
        "flock -n -w 5 /tmp/lock rm -rf /tmp/example",
        "flock -c 'rm -rf /tmp/example' /tmp/lock",
        "flock --command='rm -rf /tmp/example' /tmp/lock",
        "/usr/local/bin/flock /tmp/lock rm -rf /tmp/example",
        "/opt/homebrew/bin/flock -c 'echo ok && rm -rf /tmp/example' /tmp/lock",
        "watch rm -rf /tmp/example",
        "watch -n 1 rm -rf /tmp/example",
        "watch --interval=1 -- rm -rf /tmp/example",
        "/usr/local/bin/watch -n 1 rm -rf /tmp/example",
        "/opt/homebrew/bin/watch --interval=1 -- rm -rf /tmp/example",
        "setsid rm -rf /tmp/example",
        "setsid -w -- rm -rf /tmp/example",
        "/usr/local/bin/setsid -w -- rm -rf /tmp/example",
        "/opt/homebrew/bin/setsid rm -rf /tmp/example",
        "ssh-agent rm -rf /tmp/example",
        "ssh-agent -t 1h -- rm -rf /tmp/example",
        "/opt/homebrew/bin/ssh-agent -t 1h -- rm -rf /tmp/example",
        "gpg-agent --daemon rm -rf /tmp/example",
        "gpg-agent --homedir /tmp/gnupg --daemon rm -rf /tmp/example",
        "/usr/local/bin/gpg-agent --homedir /tmp/gnupg --daemon rm -rf /tmp/example",
        "dbus-run-session -- rm -rf /tmp/example",
        "dbus-run-session --config-file=/tmp/session.conf rm -rf /tmp/example",
        "/opt/homebrew/bin/dbus-run-session --config-file=/tmp/session.conf rm -rf /tmp/example",
        "ionice rm -rf /tmp/example",
        "ionice -c 2 -n 7 rm -rf /tmp/example",
        "ionice --class=idle -- rm -rf /tmp/example",
        "/usr/local/bin/ionice -c 2 -n 7 rm -rf /tmp/example",
        "chrt -f 99 rm -rf /tmp/example",
        "chrt --deadline --sched-runtime=1000000 0 rm -rf /tmp/example",
        "/usr/bin/chrt --rr 20 -- rm -rf /tmp/example",
        "/opt/homebrew/bin/chrt --rr 20 -- rm -rf /tmp/example",
        "taskset 0x1 rm -rf /tmp/example",
        "taskset -c 0 rm -rf /tmp/example",
        "taskset --cpu-list=0 -- rm -rf /tmp/example",
        "/usr/local/bin/taskset --cpu-list=0 -- rm -rf /tmp/example",
        "unshare -Ur rm -rf /tmp/example",
        "unshare --map-root-user --mount-proc=/proc rm -rf /tmp/example",
        "/usr/bin/unshare --user --map-user=0 --map-group=0 -- rm -rf /tmp/example",
        "/opt/homebrew/bin/unshare --user --map-root-user -- rm -rf /tmp/example",
        "nsenter --target 1 --mount --uts -- rm -rf /tmp/example",
        "nsenter -t1 -m -u rm -rf /tmp/example",
        "/usr/bin/nsenter --target=1 --net=/proc/1/ns/net -- rm -rf /tmp/example",
        "/usr/local/bin/nsenter --target 1 --mount -- rm -rf /tmp/example",
        "strace -f -o /tmp/trace.log rm -rf /tmp/example",
        "strace --trace=file --string-limit=80 -- rm -rf /tmp/example",
        "/usr/bin/strace -qq -e trace=file rm -rf /tmp/example",
        "/usr/local/bin/strace -f -o /tmp/trace.log rm -rf /tmp/example",
        "/opt/homebrew/bin/strace --trace=file -- rm -rf /tmp/example",
        "bwrap --ro-bind / / --dev /dev --proc /proc rm -rf /tmp/example",
        "bwrap --unshare-all --die-with-parent -- rm -rf /tmp/example",
        "/usr/bin/bwrap --setenv HOME /tmp --unsetenv PATH rm -rf /tmp/example",
        "/opt/homebrew/bin/bwrap --ro-bind / / rm -rf /tmp/example",
        "/usr/local/bin/bubblewrap --unshare-all -- rm -rf /tmp/example",
        "firejail --quiet --noprofile rm -rf /tmp/example",
        "firejail --private --net=none -- rm -rf /tmp/example",
        "/usr/bin/firejail --profile=/tmp/profile --blacklist=/tmp rm -rf /tmp/example",
        "/opt/homebrew/bin/firejail --quiet --noprofile rm -rf /tmp/example",
        "systemd-run --user --scope rm -rf /tmp/example",
        "systemd-run --unit cleanup --property Type=exec -- rm -rf /tmp/example",
        "/usr/bin/systemd-run --same-dir --setenv=FOO=bar rm -rf /tmp/example",
        "/usr/local/bin/systemd-run --user --scope rm -rf /tmp/example",
        "/opt/homebrew/bin/systemd-run --same-dir --setenv=FOO=bar rm -rf /tmp/example",
        "tar --checkpoint-action=exec='rm -rf /tmp/example' -cf out.tar .",
        "tar --checkpoint=1 --checkpoint-action 'exec=rm -rf /tmp/example' -cf out.tar .",
        "/usr/bin/tar -cf out.tar --checkpoint-action=exec='echo ok && rm -rf /tmp/example' .",
        "/opt/homebrew/bin/tar -cf out.tar --checkpoint-action=exec='echo ok && rm -rf /tmp/example' .",
        "tar -xf in.tar --to-command='rm -rf /tmp/example'",
        "gtar --to-command 'echo ok && rm -rf /tmp/example' -xf in.tar",
        "/usr/local/bin/gtar --to-command 'echo ok && rm -rf /tmp/example' -xf in.tar",
        "tar -I 'rm -rf /tmp/example' -cf out.tar .",
        "gtar --use-compress-program='echo ok && rm -rf /tmp/example' -cf out.tar .",
        "/opt/homebrew/bin/gtar --use-compress-program='echo ok && rm -rf /tmp/example' -cf out.tar .",
        "find . -exec rm -rf /tmp/example \\;",
        "/usr/bin/find . -execdir rm -rf /tmp/example +",
        "/usr/local/bin/find . -exec rm -rf /tmp/example \\;",
        "/opt/homebrew/bin/find . -okdir rm -rf /tmp/example +",
        "sudo find . -exec sh -c 'rm -rf /tmp/example' \\;",
        "find . -ok rm -rf /tmp/example \\;",
        "/usr/bin/find . -okdir rm -rf /tmp/example +",
        "parallel 'rm -rf /tmp/example' ::: target",
        "parallel --will-cite -j 2 rm -rf ::: /tmp/example",
        "parallel --semaphore rm -rf /tmp/example",
        "parallel ::: 'rm -rf /tmp/example'",
        "/usr/bin/parallel --jobs=2 'echo ok && rm -rf /tmp/example' ::: target",
        "/usr/local/bin/parallel --jobs=2 'rm -rf /tmp/example' ::: target",
        "/opt/homebrew/bin/parallel ::: 'rm -rf /tmp/example'",
        "ssh example.com rm -rf /tmp/example",
        "ssh -p 2222 -i /tmp/key example.com rm -rf /tmp/example",
        "/usr/bin/ssh -J bastion example.com 'echo ok && rm -rf /tmp/example'",
        "/usr/local/bin/ssh example.com rm -rf /tmp/example",
        "ssh -o ProxyCommand='rm -rf /tmp/example' example.com",
        "ssh -oLocalCommand='rm -rf /tmp/example' example.com",
        "ssh -o RemoteCommand='echo ok && rm -rf /tmp/example' example.com",
        "/opt/homebrew/bin/ssh -o ProxyCommand='rm -rf /tmp/example' example.com",
        "SSH_ASKPASS='rm -rf /tmp/example' ssh example.com",
        "env SSH_ASKPASS='echo ok && rm -rf /tmp/example' ssh -N example.com",
        "SSH_ASKPASS='rm -rf /tmp/example' /opt/homebrew/bin/ssh example.com",
        "GIT_SSH_COMMAND='rm -rf /tmp/example' git fetch origin",
        "GIT_PROXY_COMMAND='rm -rf /tmp/example' /usr/bin/git ls-remote origin",
        "GIT_SSH_COMMAND='rm -rf /tmp/example' /opt/homebrew/bin/git fetch origin",
        "env GIT_SSH_COMMAND='rm -rf /tmp/example' git fetch origin",
        "GIT_ASKPASS='rm -rf /tmp/example' git fetch origin",
        "SSH_ASKPASS='echo ok && rm -rf /tmp/example' /usr/bin/git ls-remote origin",
        "GIT_EXTERNAL_DIFF='rm -rf /tmp/example' git diff",
        "env GIT_EXTERNAL_DIFF='echo ok && rm -rf /tmp/example' git show HEAD",
        "git -c core.sshCommand='rm -rf /tmp/example' fetch origin",
        "git -C repo -c core.sshCommand='echo ok && rm -rf /tmp/example' pull",
        "/usr/bin/git -c core.sshCommand='rm -rf /tmp/example' ls-remote origin",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.sshCommand GIT_CONFIG_VALUE_0='rm -rf /tmp/example' git fetch origin",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.sshCommand GIT_CONFIG_VALUE_0='rm -rf /tmp/example' /opt/homebrew/bin/git fetch origin",
        "env GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.cleanup GIT_CONFIG_VALUE_0='!echo ok && rm -rf /tmp/example' git cleanup",
        "GIT_SSH_CMD='rm -rf /tmp/example' git --config-env=core.sshCommand=GIT_SSH_CMD fetch origin",
        "GIT_SSH_CMD='rm -rf /tmp/example' /usr/local/bin/git --config-env=core.sshCommand=GIT_SSH_CMD fetch origin",
        "env GIT_ALIAS_BODY='!echo ok && rm -rf /tmp/example' git --config-env alias.cleanup=GIT_ALIAS_BODY cleanup",
        "git -c core.askPass='rm -rf /tmp/example' clone https://example/repo",
        "/opt/homebrew/bin/git -c core.askpass='echo ok && rm -rf /tmp/example' push origin main",
        "git -c diff.external='rm -rf /tmp/example' diff HEAD",
        "/usr/bin/git -C repo -c diff.external='echo ok && rm -rf /tmp/example' log -p -1",
        "/opt/homebrew/bin/git -c diff.external='rm -rf /tmp/example' show HEAD",
        "git -c credential.helper='!rm -rf /tmp/example' credential fill",
        "git -c credential.helper='!echo ok && rm -rf /tmp/example' fetch origin",
        "/usr/bin/git -C repo -c credential.helper='!rm -rf /tmp/example' push origin main",
        "/opt/homebrew/bin/git -C repo -c credential.helper='!rm -rf /tmp/example' push origin main",
        "git -c difftool.cleanup.cmd='rm -rf /tmp/example' difftool --tool cleanup HEAD",
        "git -c mergetool.cleanup.cmd='echo ok && rm -rf /tmp/example' mergetool -t cleanup",
        "/usr/local/bin/git -c difftool.cleanup.cmd='rm -rf /tmp/example' difftool --tool cleanup HEAD",
        "/opt/homebrew/bin/git -c mergetool.cleanup.cmd='echo ok && rm -rf /tmp/example' mergetool -t cleanup",
        "git -c core.editor='rm -rf /tmp/example' commit",
        "git -c sequence.editor='echo ok && rm -rf /tmp/example' rebase -i main",
        "/usr/bin/git -C repo -c core.editor='rm -rf /tmp/example' tag -a v1.0",
        "/opt/homebrew/bin/git -C repo -c core.editor='rm -rf /tmp/example' tag -a v1.0",
        "GIT_EDITOR='rm -rf /tmp/example' git commit",
        "GIT_EDITOR='rm -rf /tmp/example' /usr/local/bin/git commit",
        "GIT_SEQUENCE_EDITOR='rm -rf /tmp/example' git rebase -i main",
        "GIT_SEQUENCE_EDITOR='echo ok && rm -rf /tmp/example' /usr/bin/git -C repo rebase --interactive main",
        "VISUAL='echo ok && rm -rf /tmp/example' git rebase -i main",
        "EDITOR='rm -rf /tmp/example' /usr/bin/git -C repo tag -a v1.0",
        "EDITOR='rm -rf /tmp/example' crontab -e",
        "VISUAL='echo ok && rm -rf /tmp/example' /usr/bin/crontab -u root -e",
        "VISUAL='echo ok && rm -rf /tmp/example' /usr/local/bin/crontab -u root -e",
        "EDITOR='echo ok && rm -rf /tmp/example' /opt/homebrew/bin/crontab -u root -e",
        "SUDO_EDITOR='rm -rf /tmp/example' sudoedit /etc/hosts",
        "SUDO_EDITOR='rm -rf /tmp/example' /usr/local/bin/sudoedit /etc/hosts",
        "SUDO_EDITOR='echo ok && rm -rf /tmp/example' sudo -e /etc/hosts",
        "SUDO_EDITOR='echo ok && rm -rf /tmp/example' /usr/local/bin/sudo -e /etc/hosts",
        "SUDO_EDITOR='rm -rf /tmp/example' /opt/homebrew/bin/sudo -e /etc/hosts",
        "VISUAL='rm -rf /tmp/example' sudoedit /etc/hosts",
        "SYSTEMD_EDITOR='rm -rf /tmp/example' systemctl edit ssh.service",
        "EDITOR='echo ok && rm -rf /tmp/example' systemctl --user edit app.service",
        "SYSTEMD_EDITOR='echo ok && rm -rf /tmp/example' /opt/homebrew/bin/systemctl --user edit app.service",
        "KUBE_EDITOR='rm -rf /tmp/example' kubectl edit deploy/app",
        "KUBE_EDITOR='echo ok && rm -rf /tmp/example' kubectl -n prod edit -f app.yaml",
        "EDITOR='rm -rf /tmp/example' oc edit route/app",
        "SVN_EDITOR='rm -rf /tmp/example' svn commit src",
        "SVN_EDITOR='echo ok && rm -rf /tmp/example' svn propedit svn:ignore .",
        "EDITOR='rm -rf /tmp/example' /usr/bin/svn ci src",
        "EDITOR='rm -rf /tmp/example' /usr/local/bin/svn ci src",
        "SVN_EDITOR='echo ok && rm -rf /tmp/example' /opt/homebrew/bin/svn propedit svn:ignore .",
        "HGEDITOR='rm -rf /tmp/example' hg commit",
        "HGEDITOR='echo ok && rm -rf /tmp/example' /usr/bin/hg commit src",
        "HGEDITOR='echo ok && rm -rf /tmp/example' /usr/local/bin/hg commit src",
        "EDITOR='rm -rf /tmp/example' hg histedit tip",
        "EDITOR='rm -rf /tmp/example' /opt/homebrew/bin/hg histedit tip",
        "HGEDITOR='rm -rf /tmp/example' hg histedit --rev tip",
        "hg --config ui.editor='rm -rf /tmp/example' commit",
        "/usr/bin/hg --config=ui.editor='echo ok && rm -rf /tmp/example' histedit tip",
        "/opt/homebrew/bin/hg --config=ui.editor='echo ok && rm -rf /tmp/example' histedit tip",
        "hg --config ui.editor='rm -rf /tmp/example' histedit --rev tip",
        "hg --config ui.ssh='rm -rf /tmp/example' pull ssh://example/repo",
        "/usr/local/bin/hg --config ui.ssh='rm -rf /tmp/example' pull ssh://example/repo",
        "/usr/bin/hg clone --ssh 'echo ok && rm -rf /tmp/example' ssh://example/repo",
        "/opt/homebrew/bin/hg clone --ssh 'echo ok && rm -rf /tmp/example' ssh://example/repo",
        "hg outgoing --ssh='rm -rf /tmp/example' ssh://example/repo",
        "GIT_PAGER='rm -rf /tmp/example' git log",
        "GIT_PAGER='rm -rf /tmp/example' git --paginate status",
        "PAGER='echo ok && rm -rf /tmp/example' git show HEAD",
        "git -c core.pager='rm -rf /tmp/example' diff HEAD",
        "git -c core.pager='echo ok && rm -rf /tmp/example' --paginate status",
        "/opt/homebrew/bin/git -c core.pager='echo ok && rm -rf /tmp/example' --paginate status",
        "MANPAGER='rm -rf /tmp/example' man ls",
        "PAGER='echo ok && rm -rf /tmp/example' /usr/bin/man 1 printf",
        "MANPAGER='echo ok && rm -rf /tmp/example' /opt/homebrew/bin/man 1 printf",
        "man -P 'rm -rf /tmp/example' ls",
        "man --pager='echo ok && rm -rf /tmp/example' git",
        "/usr/local/bin/man -P 'rm -rf /tmp/example' ls",
        "BROWSER='rm -rf /tmp/example' xdg-open https://example.com",
        "env BROWSER='echo ok && rm -rf /tmp/example' sensible-browser https://example.com",
        "/opt/homebrew/bin/env BROWSER='echo ok && rm -rf /tmp/example' sensible-browser https://example.com",
        "BROWSER='rm -rf /tmp/example' /usr/bin/x-www-browser https://example.com",
        "BROWSER='echo ok && rm -rf /tmp/example' /opt/homebrew/bin/xdg-open https://example.com",
        "BROWSER='rm -rf /tmp/example' /usr/local/bin/sensible-browser https://example.com",
        "BROWSER='rm -rf /tmp/example' /usr/local/bin/x-www-browser https://example.com",
        "BROWSER='echo ok && rm -rf /tmp/example' /opt/homebrew/bin/gnome-open https://example.com",
        "LESSOPEN='|rm -rf /tmp/example' less README.md",
        "env LESSOPEN='||echo ok && rm -rf /tmp/example' /usr/bin/less -R README.md",
        "LESSOPEN='||echo ok && rm -rf /tmp/example' /usr/local/bin/less README.md",
        "LESSOPEN='|rm -rf /tmp/example' /opt/homebrew/bin/less README.md",
        "LESSCLOSE='rm -rf /tmp/example' less README.md",
        "env LESSCLOSE='echo ok && rm -rf /tmp/example' /usr/bin/less -R README.md",
        "LESSCLOSE='echo ok && rm -rf /tmp/example' /usr/local/bin/less README.md",
        "LESSCLOSE='echo ok && rm -rf /tmp/example' /opt/homebrew/bin/less README.md",
        "RSYNC_RSH='echo ok && rm -rf /tmp/example' rsync -av src host:dst",
        "RSYNC_RSH='rm -rf /tmp/example' /opt/homebrew/bin/rsync -av src host:dst",
        "rsync -e 'rm -rf /tmp/example' src host:dst",
        "rsync --rsh='echo ok && rm -rf /tmp/example' src host:dst",
        "/usr/bin/rsync -av -e 'rm -rf /tmp/example' src host:dst",
        "/opt/homebrew/bin/rsync -av -e 'rm -rf /tmp/example' src host:dst",
        "/opt/homebrew/bin/rsync --rsh='echo ok && rm -rf /tmp/example' src host:dst",
        "sshpass -p secret ssh example.com rm -rf /tmp/example",
        "sshpass -e ssh -o ProxyCommand='rm -rf /tmp/example' example.com",
        "sshpass -f /tmp/pass rsync -e 'rm -rf /tmp/example' src host:dst",
        "sshpass -p secret rm -rf /tmp/example",
        "/opt/homebrew/bin/sshpass -p secret ssh example.com rm -rf /tmp/example",
        "/usr/local/bin/sshpass -e ssh -o ProxyCommand='rm -rf /tmp/example' example.com",
        "/opt/homebrew/bin/sshpass -f /tmp/pass rsync -e 'rm -rf /tmp/example' src host:dst",
        "scp -S 'rm -rf /tmp/example' src host:dst",
        "scp -o ProxyCommand='rm -rf /tmp/example' src host:dst",
        "/opt/homebrew/bin/scp -S 'rm -rf /tmp/example' src host:dst",
        "/usr/local/bin/scp -o ProxyCommand='rm -rf /tmp/example' src host:dst",
        "sftp -S 'rm -rf /tmp/example' host",
        "/usr/bin/sftp -oRemoteCommand='rm -rf /tmp/example' host",
        "/usr/local/bin/sftp -S 'rm -rf /tmp/example' host",
        "/opt/homebrew/bin/sftp -oRemoteCommand='rm -rf /tmp/example' host",
        "SVN_SSH='rm -rf /tmp/example' svn checkout svn+ssh://example/repo",
        "SVN_SSH='rm -rf /tmp/example' /opt/homebrew/bin/svn checkout svn+ssh://example/repo",
        "SVN_SSH='echo ok && rm -rf /tmp/example' /usr/local/bin/svn checkout svn+ssh://example/repo",
        "git bisect run rm -rf /tmp/example",
        "git -C repo bisect run sh -c 'rm -rf /tmp/example'",
        "/usr/bin/git -c protocol.file.allow=always bisect run rm -rf /tmp/example",
        "/usr/local/bin/git bisect run rm -rf /tmp/example",
        "/opt/homebrew/bin/git -C repo bisect run sh -c 'rm -rf /tmp/example'",
        "npm exec -- rm -rf /tmp/example",
        "npm exec --package=foo -- rm -rf /tmp/example",
        "/opt/homebrew/bin/npm exec -- rm -rf /tmp/example",
        "npx rm -rf /tmp/example",
        "/usr/local/bin/npx rm -rf /tmp/example",
        "npm exec -c 'rm -rf /tmp/example'",
        "/opt/homebrew/bin/npm exec -c 'rm -rf /tmp/example'",
        "npx --call='echo ok && rm -rf /tmp/example'",
        "pnpm exec rm -rf /tmp/example",
        "pnpm exec -- rm -rf /tmp/example",
        "pnpm dlx rm -rf /tmp/example",
        "pnpm --dir repo exec rm -rf /tmp/example",
        "/opt/homebrew/bin/pnpm --dir repo dlx rm -rf /tmp/example",
        "pnpx rm -rf /tmp/example",
        "/usr/local/bin/pnpx rm -rf /tmp/example",
        "yarn exec rm -rf /tmp/example",
        "yarn dlx rm -rf /tmp/example",
        "yarn --cwd repo exec rm -rf /tmp/example",
        "/opt/homebrew/bin/yarn --cwd repo dlx rm -rf /tmp/example",
        "bunx rm -rf /tmp/example",
        "/usr/local/bin/bunx rm -rf /tmp/example",
        "bun x rm -rf /tmp/example",
        "bun --cwd repo x rm -rf /tmp/example",
        "/opt/homebrew/bin/bun --cwd repo x rm -rf /tmp/example",
        "poetry run rm -rf /tmp/example",
        "poetry --directory repo run rm -rf /tmp/example",
        "pipenv run rm -rf /tmp/example",
        "pipenv --python 3.12 run rm -rf /tmp/example",
        "uv run rm -rf /tmp/example",
        "uv --directory repo run rm -rf /tmp/example",
        "uvx rm -rf /tmp/example",
        "pdm run rm -rf /tmp/example",
        "pdm --project repo run rm -rf /tmp/example",
        "hatch run rm -rf /tmp/example",
        "/opt/homebrew/bin/hatch run -e test rm -rf /tmp/example",
        "/opt/homebrew/bin/poetry run rm -rf /tmp/example",
        "/usr/local/bin/pipenv run rm -rf /tmp/example",
        "/opt/homebrew/bin/uv run rm -rf /tmp/example",
        "/usr/local/bin/uvx rm -rf /tmp/example",
        "/opt/homebrew/bin/pdm run rm -rf /tmp/example",
        "conda run -n env rm -rf /tmp/example",
        "mamba run --prefix /tmp/env rm -rf /tmp/example",
        "micromamba run -n env rm -rf /tmp/example",
        "/usr/local/bin/conda run -n env rm -rf /tmp/example",
        "/opt/homebrew/bin/mamba run --prefix /tmp/env rm -rf /tmp/example",
        "/usr/local/bin/micromamba run -n env rm -rf /tmp/example",
        "bundle exec rm -rf /tmp/example",
        "bundle --gemfile Gemfile.ci exec rm -rf /tmp/example",
        "bundler exec rm -rf /tmp/example",
        "rbenv exec rm -rf /tmp/example",
        "pyenv exec rm -rf /tmp/example",
        "nodenv exec rm -rf /tmp/example",
        "asdf exec rm -rf /tmp/example",
        "mise exec rm -rf /tmp/example",
        "/opt/homebrew/bin/bundle --gemfile Gemfile.ci exec rm -rf /tmp/example",
        "/usr/local/bin/bundler exec rm -rf /tmp/example",
        "/opt/homebrew/bin/rbenv exec rm -rf /tmp/example",
        "/usr/local/bin/pyenv exec rm -rf /tmp/example",
        "/opt/homebrew/bin/nodenv exec rm -rf /tmp/example",
        "/usr/local/bin/asdf exec rm -rf /tmp/example",
        "/opt/homebrew/bin/mise exec rm -rf /tmp/example",
        "direnv exec . rm -rf /tmp/example",
        "direnv exec /tmp/project rm -rf /tmp/example",
        "/usr/local/bin/direnv exec /tmp/project rm -rf /tmp/example",
        "/opt/homebrew/bin/direnv exec . rm -rf /tmp/example",
        "nix-shell --run 'rm -rf /tmp/example'",
        "nix-shell -p hello --run 'echo ok && rm -rf /tmp/example'",
        "/usr/local/bin/nix-shell --run='rm -rf /tmp/example'",
        "/opt/homebrew/bin/nix-shell --run 'rm -rf /tmp/example'",
        "nix develop . --command rm -rf /tmp/example",
        "nix shell nixpkgs#coreutils -c rm -rf /tmp/example",
        "nix --extra-experimental-features 'nix-command flakes' develop .#dev --command rm -rf /tmp/example",
        "/opt/homebrew/bin/nix develop . --command rm -rf /tmp/example",
        "/usr/local/bin/nix shell nixpkgs#coreutils -c rm -rf /tmp/example",
        "guix shell -- rm -rf /tmp/example",
        "guix shell coreutils -- rm -rf /tmp/example",
        "guix shell --container --network -- rm -rf /tmp/example",
        "guix environment -- rm -rf /tmp/example",
        "/opt/homebrew/bin/guix shell -- rm -rf /tmp/example",
        "watchexec -- rm -rf /tmp/example",
        "watchexec -w src rm -rf /tmp/example",
        "/usr/local/bin/watchexec -w src rm -rf /tmp/example",
        "entr -r rm -rf /tmp/example",
        "entr -s 'rm -rf /tmp/example'",
        "/opt/homebrew/bin/entr -r rm -rf /tmp/example",
        "screen -dmS cleanup rm -rf /tmp/example",
        "/usr/bin/screen -S cleanup -dm rm -rf /tmp/example",
        "/usr/local/bin/screen -dmS cleanup rm -rf /tmp/example",
        "docker exec app rm -rf /tmp/example",
        "docker exec -it app sh -c 'rm -rf /tmp/example'",
        "docker run --rm alpine rm -rf /tmp/example",
        "/usr/bin/docker --context desktop-linux container exec app rm -rf /tmp/example",
        "/opt/homebrew/bin/docker exec app rm -rf /tmp/example",
        "docker compose exec app rm -rf /tmp/example",
        "docker compose -f compose.yml run --rm app sh -c 'rm -rf /tmp/example'",
        "/usr/local/bin/docker compose exec app rm -rf /tmp/example",
        "podman exec app rm -rf /tmp/example",
        "/usr/local/bin/podman run --rm alpine rm -rf /tmp/example",
        "kubectl exec pod/app -- rm -rf /tmp/example",
        "kubectl -n prod exec pod/app -c web -it -- sh -c 'rm -rf /tmp/example'",
        "/opt/homebrew/bin/kubectl --context=prod exec deploy/app -- rm -rf /tmp/example",
        "/usr/local/bin/kubectl exec pod/app -- rm -rf /tmp/example",
        "KUBECTL_EXTERNAL_DIFF='rm -rf /tmp/example' kubectl diff -f app.yaml",
        "KUBECTL_EXTERNAL_DIFF='rm -rf /tmp/example' /usr/local/bin/kubectl diff -f app.yaml",
        "env KUBECTL_EXTERNAL_DIFF='echo ok && rm -rf /tmp/example' /opt/homebrew/bin/kubectl --context=prod diff -f app.yaml",
        "oc exec pod/app -- rm -rf /tmp/example",
        "/opt/homebrew/bin/oc exec pod/app -- rm -rf /tmp/example",
        "socat - 'EXEC:rm -rf /tmp/example'",
        "/opt/homebrew/bin/socat TCP-LISTEN:1 'EXEC:rm -rf /tmp/example'",
        "socat - 'SYSTEM:rm -rf /tmp/example'",
        "socat - 'SHELL:echo ok && rm -rf /tmp/example'",
        "/usr/local/bin/socat - 'SYSTEM:rm -rf /tmp/example'",
        "/opt/homebrew/bin/socat - 'SHELL:echo ok && rm -rf /tmp/example'",
        "script -q -c 'rm -rf /tmp/example' /tmp/typescript",
        "script --command='rm -rf /tmp/example' /tmp/typescript",
        "/usr/local/bin/script -q -c 'rm -rf /tmp/example' /tmp/typescript",
        "/opt/homebrew/bin/script --command='echo ok && rm -rf /tmp/example' /tmp/typescript",
        "tmux new-session -d -s cleanup 'rm -rf /tmp/example'",
        "tmux new-window -n cleanup 'echo ok && rm -rf /tmp/example'",
        "tmux split-window -h 'rm -rf /tmp/example'",
        "tmux respawn-pane -k 'rm -rf /tmp/example'",
        "/usr/local/bin/tmux new-window -n cleanup 'rm -rf /tmp/example'",
        "/opt/homebrew/bin/tmux split-window -h 'echo ok && rm -rf /tmp/example'",
        "bash -c 'rm -rf /tmp/example'",
        "bash -c $'rm -rf /tmp/example'",
        "bash -lc 'echo ok && rm -rf /tmp/example'",
        "bash -o pipefail -c 'rm -rf /tmp/example'",
        "bash -O extglob -c 'rm -rf /tmp/example'",
        "bash --norc -c 'rm -rf /tmp/example'",
        "/opt/homebrew/bin/bash --norc -c 'rm -rf /tmp/example'",
        "bash --rcfile /tmp/bashrc -c 'rm -rf /tmp/example'",
        "bash --rcfile=/tmp/bashrc -c 'rm -rf /tmp/example'",
        "sh -c 'rm -rf /tmp/example'",
        "/bin/bash -c 'rm -rf /tmp/example'",
        "/usr/local/bin/zsh -c 'rm -rf /tmp/example'",
        "env bash -c 'rm -rf /tmp/example'",
        "sudo bash -c 'rm -rf /tmp/example'",
        "/usr/bin/timeout 5 /usr/bin/nice -n1 /usr/bin/nohup /usr/bin/stdbuf -oL rm -rf /tmp/example",
        "/opt/homebrew/bin/timeout 5 /usr/local/bin/nice -n1 /opt/homebrew/bin/nohup /usr/local/bin/stdbuf -oL rm -rf /tmp/example",
        "timeout 5 -- rm -rf /tmp/example",
        "/usr/local/bin/timeout 5 -- rm -rf /tmp/example",
        "time -p rm -rf /tmp/example",
        "/usr/bin/time -p rm -rf /tmp/example",
        "/usr/bin/time -o /tmp/time.log -a rm -rf /tmp/example",
        "/usr/bin/time --format=%E --output=/tmp/time.log rm -rf /tmp/example",
        "/usr/local/bin/time --format=%E rm -rf /tmp/example",
        "/opt/homebrew/bin/gtime -o /tmp/time.log -a rm -rf /tmp/example",
        "nohup rm -rf /tmp/example",
        "/opt/homebrew/bin/nohup -- rm -rf /tmp/example",
        "nice -n1 rm -rf /tmp/example",
        "/usr/local/bin/nice -n1 rm -rf /tmp/example",
        "nice -n-5 rm -rf /tmp/example",
        "stdbuf -oL rm -rf /tmp/example",
        "/opt/homebrew/bin/stdbuf --output=L rm -rf /tmp/example",
        "stdbuf --output=L --error=0 rm -rf /tmp/example",
        "xargs rm -rf /tmp/example",
        "/usr/bin/xargs -0 -r rm -rf /tmp/example",
        "/usr/local/bin/xargs -0 -r rm -rf /tmp/example",
        "/opt/homebrew/bin/xargs -I REPL rm -rf REPL /tmp/example",
        "xargs -0 -r rm -rf /tmp/example",
        "xargs -I REPL rm -rf REPL /tmp/example",
        "eval 'rm -rf /tmp/example'",
        "eval $'rm -rf /tmp/example'",
        "eval 'echo ok && rm -rf /tmp/example'",
        "eval -- 'rm -rf /tmp/example'",
        "command eval 'rm -rf /tmp/example'",
        "command eval -- 'rm -rf /tmp/example'",
        "env FOO=bar eval 'rm -rf /tmp/example'",
        "builtin eval 'rm -rf /tmp/example'",
        "trap 'rm -rf /tmp/example' EXIT",
        "trap $'rm -rf /tmp/example' EXIT",
        "trap 'echo ok && rm -rf /tmp/example' EXIT ERR",
        "command trap 'rm -rf /tmp/example' EXIT",
        "builtin trap 'rm -rf /tmp/example' EXIT",
        "git -c alias.cleanup='!rm -rf /tmp/example' cleanup",
        "git -C repo -c alias.cleanup='!echo ok && rm -rf /tmp/example' cleanup",
        "/usr/bin/git -c alias.cleanup='!f() { rm -rf /tmp/example; }; f' cleanup",
        "/usr/local/bin/git -c alias.cleanup='!rm -rf /tmp/example' cleanup",
        "/opt/homebrew/bin/git -C repo -c alias.cleanup='!echo ok && rm -rf /tmp/example' cleanup",
        "git difftool --extcmd='rm -rf /tmp/example' HEAD",
        "git -C repo difftool -x 'echo ok && rm -rf /tmp/example' main",
        "/usr/bin/git difftool --extcmd 'rm -rf /tmp/example' HEAD~1",
        "/usr/local/bin/git difftool --extcmd 'rm -rf /tmp/example' HEAD~1",
        "/opt/homebrew/bin/git -C repo difftool -x 'echo ok && rm -rf /tmp/example' main",
        "git submodule foreach 'rm -rf /tmp/example'",
        "git -C repo submodule --quiet foreach --recursive 'rm -rf /tmp/example'",
        "/usr/bin/git -c protocol.file.allow=always submodule foreach 'echo ok && rm -rf /tmp/example'",
        "/opt/homebrew/bin/git -c protocol.file.allow=always submodule foreach 'echo ok && rm -rf /tmp/example'",
        "git filter-branch --tree-filter 'rm -rf /tmp/example' HEAD",
        "git filter-branch --index-filter='rm -rf /tmp/example' -- --all",
        "/usr/bin/git -C repo filter-branch --setup 'echo ok' --msg-filter 'rm -rf /tmp/example' HEAD",
        "/usr/local/bin/git -C repo filter-branch --setup 'echo ok' --msg-filter 'rm -rf /tmp/example' HEAD",
        "git rebase --exec 'rm -rf /tmp/example' main",
        "git rebase -i -x 'rm -rf /tmp/example' main",
        "git rebase --exec 'echo ok' --exec 'rm -rf /tmp/example' main",
        "/usr/bin/git -C repo rebase --onto main --exec='echo ok && rm -rf /tmp/example' topic",
        "/opt/homebrew/bin/git rebase --exec 'rm -rf /tmp/example' main",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_some(),
            "{command}"
        );
    }

    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"bash -- -c 'rm -rf /tmp/example'"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"npm install rm -rf /tmp/example"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"screen -ls rm -rf /tmp/example"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/screen -ls rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/npm exec --help rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"screen -r rm -rf /tmp/example"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"docker ps rm -rf /tmp/example"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/docker ps rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"docker exec --help app rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"docker run --rm rm"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"docker compose ps rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/docker compose ps rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"docker compose exec --help app rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"docker compose run --rm app"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"kubectl get pods rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/kubectl get pods rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"kubectl exec pod/app rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/oc exec pod/app rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"kubectl exec --help pod/app -- rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"tmux list-sessions rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"tmux new-session rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"npm exec --help rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"pnpm install rm -rf /tmp/example"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"pnpm exec --help rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"pnpm dlx"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"/opt/homebrew/bin/pnpm dlx"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"yarn install rm -rf /tmp/example"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"yarn exec --help rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"yarn dlx"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"/opt/homebrew/bin/yarn dlx"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"bun install rm -rf /tmp/example"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"bun x --help rm -rf /tmp/example"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"bun x"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_denied("bash", r#"{"command":"/opt/homebrew/bin/bun x"}"#)
            .is_none()
    );
    for command in [
        "poetry install rm -rf /tmp/example",
        "poetry run --help rm -rf /tmp/example",
        "poetry run",
        "/opt/homebrew/bin/poetry run --help rm -rf /tmp/example",
        "pipenv install rm -rf /tmp/example",
        "pipenv run --help rm -rf /tmp/example",
        "pipenv run",
        "/usr/local/bin/pipenv run",
        "uv pip install rm -rf /tmp/example",
        "uv run --help rm -rf /tmp/example",
        "uv run",
        "/opt/homebrew/bin/uv run",
        "pdm install rm -rf /tmp/example",
        "pdm --help run rm -rf /tmp/example",
        "pdm run --help rm -rf /tmp/example",
        "pdm run",
        "/opt/homebrew/bin/pdm run --help rm -rf /tmp/example",
        "hatch env show rm -rf /tmp/example",
        "hatch --version run rm -rf /tmp/example",
        "hatch run --help rm -rf /tmp/example",
        "hatch run",
        "/opt/homebrew/bin/hatch run",
        "conda install rm -rf /tmp/example",
        "/opt/homebrew/bin/env --help rm -rf /tmp/example",
        "/opt/homebrew/bin/xargs --help rm -rf /tmp/example",
        "conda --help run rm -rf /tmp/example",
        "conda --version run rm -rf /tmp/example",
        "conda run --help rm -rf /tmp/example",
        "conda run -n env",
        "/opt/homebrew/bin/conda run --help rm -rf /tmp/example",
        "mamba search rm -rf /tmp/example",
        "mamba run --unknown rm -rf /tmp/example",
        "/usr/local/bin/mamba run --unknown rm -rf /tmp/example",
        "micromamba run",
        "/opt/homebrew/bin/micromamba run",
        "bundle install rm -rf /tmp/example",
        "bundle exec --help rm -rf /tmp/example",
        "bundle exec",
        "/usr/local/bin/bundle exec --help rm -rf /tmp/example",
        "rbenv versions rm -rf /tmp/example",
        "rbenv exec --help rm -rf /tmp/example",
        "rbenv exec",
        "/opt/homebrew/bin/rbenv exec",
        "pyenv versions rm -rf /tmp/example",
        "pyenv exec --help rm -rf /tmp/example",
        "/usr/local/bin/pyenv exec --help rm -rf /tmp/example",
        "/opt/homebrew/bin/nodenv exec",
        "asdf list rm -rf /tmp/example",
        "asdf exec",
        "/opt/homebrew/bin/asdf exec",
        "mise list rm -rf /tmp/example",
        "/usr/local/bin/mise list rm -rf /tmp/example",
        "direnv allow rm -rf /tmp/example",
        "direnv exec --help rm -rf /tmp/example",
        "direnv exec .",
        "/usr/local/bin/direnv exec --help rm -rf /tmp/example",
        "/opt/homebrew/bin/direnv exec .",
        "/opt/homebrew/bin/flock -c",
        "/opt/homebrew/bin/flock --help /tmp/lock rm -rf /tmp/example",
        "/opt/homebrew/bin/watch --help rm -rf /tmp/example",
        "/opt/homebrew/bin/watch --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/setsid --help rm -rf /tmp/example",
        "/opt/homebrew/bin/setsid --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/doas -u",
        "/opt/homebrew/bin/doas --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/pkexec --user",
        "/opt/homebrew/bin/pkexec --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/chroot /tmp/root",
        "/opt/homebrew/bin/chroot --unknown /tmp/root rm -rf /tmp/example",
        "/opt/homebrew/bin/setpriv --dump rm -rf /tmp/example",
        "/opt/homebrew/bin/prlimit --pid 123 rm -rf /tmp/example",
        "/opt/homebrew/bin/numactl --hardware rm -rf /tmp/example",
        "/opt/homebrew/bin/setarch --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/linux64 --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/ionice --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/ionice -c 2 -n 7",
        "/opt/homebrew/bin/chrt --pid 123 rm -rf /tmp/example",
        "/opt/homebrew/bin/taskset --pid 123 rm -rf /tmp/example",
        "/opt/homebrew/bin/taskset --cpu-list=0",
        "/opt/homebrew/bin/unshare --help rm -rf /tmp/example",
        "/opt/homebrew/bin/unshare --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/nsenter --help rm -rf /tmp/example",
        "/opt/homebrew/bin/nsenter --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/bwrap --help rm -rf /tmp/example",
        "/opt/homebrew/bin/bwrap --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/firejail --help rm -rf /tmp/example",
        "/opt/homebrew/bin/firejail --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/strace --attach 123 rm -rf /tmp/example",
        "/opt/homebrew/bin/strace --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/systemd-run --help rm -rf /tmp/example",
        "/opt/homebrew/bin/systemd-run --unknown rm -rf /tmp/example",
        "nix-shell -p hello rm -rf /tmp/example",
        "nix-shell --help --run 'rm -rf /tmp/example'",
        "nix-shell -- --run 'rm -rf /tmp/example'",
        "nix develop . rm -rf /tmp/example",
        "nix develop --help --command rm -rf /tmp/example",
        "nix develop . -- --command rm -rf /tmp/example",
        "nix build . --command rm -rf /tmp/example",
        "/opt/homebrew/bin/nix-shell --help --run 'rm -rf /tmp/example'",
        "/usr/local/bin/nix develop --help --command rm -rf /tmp/example",
        "/opt/homebrew/bin/nix shell nixpkgs#coreutils -- --command rm -rf /tmp/example",
        "guix shell rm -rf /tmp/example",
        "guix shell --help -- rm -rf /tmp/example",
        "guix build -- rm -rf /tmp/example",
        "/usr/local/bin/guix shell --help -- rm -rf /tmp/example",
        "/opt/homebrew/bin/guix shell coreutils",
        "watchexec --help rm -rf /tmp/example",
        "watchexec -w src",
        "watchexec --unknown rm -rf /tmp/example",
        "/usr/local/bin/watchexec --help rm -rf /tmp/example",
        "/opt/homebrew/bin/watchexec -w src",
        "entr -h rm -rf /tmp/example",
        "entr -r",
        "entr --unknown rm -rf /tmp/example",
        "/usr/local/bin/entr -h rm -rf /tmp/example",
        "/opt/homebrew/bin/entr -r",
        "ssh-agent -k rm -rf /tmp/example",
        "/opt/homebrew/bin/ssh-agent -k rm -rf /tmp/example",
        "ssh-agent --help rm -rf /tmp/example",
        "ssh-agent",
        "gpg-agent rm -rf /tmp/example",
        "gpg-agent --daemon",
        "/usr/local/bin/gpg-agent --daemon",
        "gpg-agent --help --daemon rm -rf /tmp/example",
        "dbus-run-session --help rm -rf /tmp/example",
        "dbus-run-session --unknown rm -rf /tmp/example",
        "/opt/homebrew/bin/dbus-run-session --unknown rm -rf /tmp/example",
        "dbus-run-session",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"parallel --dry-run 'rm -rf /tmp/example' ::: target"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/parallel --dry-run 'rm -rf /tmp/example' ::: target"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/find . -name rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"git -c alias.cleanup='status' cleanup rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/git -c alias.cleanup='status' cleanup rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"git filter-branch --subdirectory-filter docs -- --all"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/usr/local/bin/git filter-branch --subdirectory-filter docs -- --all"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"git rebase main -- --exec 'rm -rf /tmp/example'"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/git rebase main -- --exec 'rm -rf /tmp/example'"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"git bisect start rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/git bisect start rm -rf /tmp/example"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"git difftool -- --extcmd='rm -rf /tmp/example'"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_denied(
                "bash",
                r#"{"command":"/opt/homebrew/bin/git difftool -- --extcmd='rm -rf /tmp/example'"}"#
            )
            .is_none()
    );
    for command in [
        "ssh example.com",
        "ssh -N example.com rm -rf /tmp/example",
        "ssh -G example.com rm -rf /tmp/example",
        "ssh -Q cipher",
        "ssh -W example.com:22 bastion rm -rf /tmp/example",
        "ssh -o Compression='rm -rf /tmp/example' example.com",
        "/opt/homebrew/bin/bash -- -c 'rm -rf /tmp/example'",
        "/opt/homebrew/bin/ssh -o Compression='rm -rf /tmp/example' example.com",
        "SSH_ASKPASS='rm -rf /tmp/example' ssh -G example.com",
        "SSH_ASKPASS='rm -rf /tmp/example' /opt/homebrew/bin/ssh -G example.com",
        "SSH_ASKPASS='rm -rf /tmp/example' ssh -Q cipher",
        "GIT_SSH_COMMAND='rm -rf /tmp/example' echo ok",
        "SUDO_ASKPASS='rm -rf /tmp/example' sudo echo ok",
        "SUDO_ASKPASS='rm -rf /tmp/example' /usr/local/bin/sudo echo ok",
        "SUDO_ASKPASS='rm -rf /tmp/example' /opt/homebrew/bin/sudo echo ok",
        "SUDO_ASKPASS='rm -rf /tmp/example' sudo -A",
        "SUDO_ASKPASS='rm -rf /tmp/example' /usr/local/bin/sudo -A",
        "SUDO_ASKPASS='rm -rf /tmp/example' /opt/homebrew/bin/sudo -A",
        "SUDO_ASKPASS='rm -rf /tmp/example' echo ok",
        "GIT_ASKPASS='rm -rf /tmp/example' git status",
        "SSH_ASKPASS='rm -rf /tmp/example' echo ok",
        "GIT_EXTERNAL_DIFF='rm -rf /tmp/example' echo ok",
        "GIT_EXTERNAL_DIFF='rm -rf /tmp/example' git status",
        "git -c core.sshCommand='rm -rf /tmp/example' status",
        "git -- -c core.sshCommand='rm -rf /tmp/example' fetch origin",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.sshCommand GIT_CONFIG_VALUE_0='rm -rf /tmp/example' git status",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.sshCommand GIT_CONFIG_VALUE_0='rm -rf /tmp/example' /usr/local/bin/git status",
        "GIT_CONFIG_COUNT=bad GIT_CONFIG_KEY_0=core.sshCommand GIT_CONFIG_VALUE_0='rm -rf /tmp/example' git fetch origin",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.cleanup GIT_CONFIG_VALUE_0=status git cleanup rm -rf /tmp/example",
        "GIT_SSH_CMD='rm -rf /tmp/example' git --config-env=core.sshCommand=GIT_SSH_CMD status",
        "GIT_SSH_CMD='rm -rf /tmp/example' /opt/homebrew/bin/git --config-env=core.sshCommand=GIT_SSH_CMD status",
        "git --config-env=core.sshCommand=GIT_SSH_CMD fetch origin",
        "GIT_SSH_CMD='rm -rf /tmp/example' git -- --config-env=core.sshCommand=GIT_SSH_CMD fetch origin",
        "git -c core.askPass='rm -rf /tmp/example' status",
        "git -- -c core.askPass='rm -rf /tmp/example' fetch origin",
        "git -c diff.external='rm -rf /tmp/example' status",
        "git -c diff.external='rm -rf /tmp/example' diff --no-ext-diff",
        "/opt/homebrew/bin/git -c diff.external='rm -rf /tmp/example' status",
        "git -c core.editor='rm -rf /tmp/example' commit -m ok",
        "git -c core.editor='rm -rf /tmp/example' commit --file /tmp/msg",
        "git -c core.editor='rm -rf /tmp/example' status",
        "/opt/homebrew/bin/git -c core.editor='rm -rf /tmp/example' commit -m ok",
        "git -c sequence.editor='rm -rf /tmp/example' rebase main",
        "GIT_EDITOR='rm -rf /tmp/example' git commit -m ok",
        "GIT_SEQUENCE_EDITOR='rm -rf /tmp/example' git rebase main",
        "GIT_SEQUENCE_EDITOR='rm -rf /tmp/example' git commit",
        "VISUAL='rm -rf /tmp/example' git status",
        "VISUAL='rm -rf /tmp/example' crontab -l",
        "EDITOR='rm -rf /tmp/example' crontab -",
        "EDITOR='rm -rf /tmp/example' /opt/homebrew/bin/crontab -l",
        "EDITOR='rm -rf /tmp/example' /usr/local/bin/crontab -r",
        "EDITOR='rm -rf /tmp/example' echo ok",
        "SUDO_EDITOR='rm -rf /tmp/example' sudo echo ok",
        "SUDO_EDITOR='rm -rf /tmp/example' sudo -e",
        "SUDO_EDITOR='rm -rf /tmp/example' /usr/local/bin/sudo -e",
        "SUDO_EDITOR='rm -rf /tmp/example' /opt/homebrew/bin/sudo -e",
        "SUDO_EDITOR='rm -rf /tmp/example' sudo --help -e /etc/hosts",
        "SUDO_EDITOR='rm -rf /tmp/example' /usr/local/bin/sudo --help -e /etc/hosts",
        "SUDO_EDITOR='rm -rf /tmp/example' /opt/homebrew/bin/sudo --help -e /etc/hosts",
        "/opt/homebrew/bin/su -l root rm -rf /tmp/example",
        "/opt/homebrew/bin/runuser --command= root",
        "/opt/homebrew/bin/runuser root rm -rf /tmp/example",
        "/opt/homebrew/bin/sg -x adm -c 'rm -rf /tmp/example'",
        "SYSTEMD_EDITOR='rm -rf /tmp/example' systemctl status ssh.service",
        "SYSTEMD_EDITOR='rm -rf /tmp/example' /opt/homebrew/bin/systemctl status ssh.service",
        "SYSTEMD_EDITOR='rm -rf /tmp/example' systemctl edit --help ssh.service",
        "SYSTEMD_EDITOR='rm -rf /tmp/example' /opt/homebrew/bin/systemctl edit --help ssh.service",
        "SYSTEMD_EDITOR='rm -rf /tmp/example' systemctl edit",
        "KUBE_EDITOR='rm -rf /tmp/example' kubectl get pods",
        "KUBE_EDITOR='rm -rf /tmp/example' kubectl edit --help deploy/app",
        "KUBE_EDITOR='rm -rf /tmp/example' kubectl edit",
        "EDITOR='rm -rf /tmp/example' kubectl diff -f app.yaml",
        "SVN_EDITOR='rm -rf /tmp/example' svn status",
        "SVN_EDITOR='rm -rf /tmp/example' /usr/local/bin/svn status",
        "SVN_EDITOR='rm -rf /tmp/example' svn commit -m ok src",
        "SVN_EDITOR='rm -rf /tmp/example' svn commit --file /tmp/msg src",
        "SVN_EDITOR='rm -rf /tmp/example' svn propedit --help svn:ignore .",
        "SVN_EDITOR='rm -rf /tmp/example' /opt/homebrew/bin/svn propedit --help svn:ignore .",
        "SVN_EDITOR='rm -rf /tmp/example' svn propedit",
        "HGEDITOR='rm -rf /tmp/example' hg status",
        "HGEDITOR='rm -rf /tmp/example' /opt/homebrew/bin/hg status",
        "HGEDITOR='rm -rf /tmp/example' hg commit -m ok",
        "HGEDITOR='rm -rf /tmp/example' hg commit --logfile /tmp/msg",
        "HGEDITOR='rm -rf /tmp/example' hg histedit --abort",
        "HGEDITOR='rm -rf /tmp/example' hg histedit",
        "hg --config ui.editor='rm -rf /tmp/example' status",
        "/usr/local/bin/hg --config ui.editor='rm -rf /tmp/example' status",
        "hg --config ui.editor='rm -rf /tmp/example' commit -m ok",
        "hg --config=ui.editor='rm -rf /tmp/example' histedit --abort",
        "hg --config ui.ssh='rm -rf /tmp/example' status",
        "hg --config ui.ssh='rm -rf /tmp/example' pull --help",
        "/opt/homebrew/bin/hg --config ui.ssh='rm -rf /tmp/example' pull --help",
        "hg -- --config ui.ssh='rm -rf /tmp/example' pull",
        "hg clone ssh://example/repo",
        "/usr/local/bin/hg clone ssh://example/repo",
        "GIT_PAGER='rm -rf /tmp/example' git status",
        "PAGER='rm -rf /tmp/example' echo ok",
        "MANPAGER='rm -rf /tmp/example' man -w ls",
        "MANPAGER='rm -rf /tmp/example' /opt/homebrew/bin/man -w ls",
        "MANPAGER='rm -rf /tmp/example' man -f printf",
        "man -P 'rm -rf /tmp/example'",
        "/opt/homebrew/bin/man -P 'rm -rf /tmp/example'",
        "man -w -P 'rm -rf /tmp/example' ls",
        "BROWSER='rm -rf /tmp/example' echo ok",
        "BROWSER='rm -rf /tmp/example' xdg-open --help",
        "BROWSER='rm -rf /tmp/example' /opt/homebrew/bin/xdg-open --help",
        "BROWSER='rm -rf /tmp/example' xdg-open",
        "BROWSER='rm -rf /tmp/example' /usr/local/bin/sensible-browser",
        "BROWSER='rm -rf /tmp/example' /opt/homebrew/bin/x-www-browser --help",
        "BROWSER='rm -rf /tmp/example' /usr/local/bin/gnome-open",
        "BROWSER='rm -rf /tmp/example' xdg-open --",
        "LESSOPEN='rm -rf /tmp/example' less README.md",
        "LESSOPEN='|rm -rf /tmp/example' less -",
        "LESSOPEN='|rm -rf /tmp/example' /usr/local/bin/less -",
        "LESSOPEN='|rm -rf /tmp/example' /opt/homebrew/bin/less -",
        "LESSOPEN='|rm -rf /tmp/example' echo ok",
        "LESSCLOSE='rm -rf /tmp/example' less -",
        "LESSCLOSE='rm -rf /tmp/example' /usr/local/bin/less -",
        "LESSCLOSE='rm -rf /tmp/example' /opt/homebrew/bin/less -",
        "LESSCLOSE='rm -rf /tmp/example' echo ok",
        "git --no-pager -c core.pager='rm -rf /tmp/example' log",
        "/opt/homebrew/bin/git --no-pager -c core.pager='rm -rf /tmp/example' log",
        "git -c core.pager='rm -rf /tmp/example' status",
        "git -c credential.helper='cache --timeout=1' credential fill",
        "git -c credential.helper='!rm -rf /tmp/example' status",
        "/usr/local/bin/git -c credential.helper='!rm -rf /tmp/example' status",
        "git -- -c credential.helper='!rm -rf /tmp/example' credential fill",
        "git -c difftool.cleanup.cmd='rm -rf /tmp/example' difftool --tool other HEAD",
        "/opt/homebrew/bin/git -c difftool.cleanup.cmd='rm -rf /tmp/example' difftool --tool other HEAD",
        "git -c mergetool.cleanup.cmd='rm -rf /tmp/example' status",
        "RSYNC_RSH='rm -rf /tmp/example' git status",
        "rsync -av src host:dst",
        "rsync -- --rsh='rm -rf /tmp/example' src host:dst",
        "/opt/homebrew/bin/rsync -- --rsh='rm -rf /tmp/example' src host:dst",
        "sshpass -h ssh example.com rm -rf /tmp/example",
        "/opt/homebrew/bin/sshpass -h ssh example.com rm -rf /tmp/example",
        "sshpass -V ssh example.com rm -rf /tmp/example",
        "sshpass --unknown ssh example.com rm -rf /tmp/example",
        "/usr/local/bin/sshpass --unknown ssh example.com rm -rf /tmp/example",
        "sshpass -p secret",
        "/opt/homebrew/bin/sshpass -p secret",
        "scp src host:dst",
        "scp -- -S 'rm -rf /tmp/example' src host:dst",
        "/opt/homebrew/bin/scp -- -S 'rm -rf /tmp/example' src host:dst",
        "scp -S",
        "sftp host",
        "sftp -- -oProxyCommand='rm -rf /tmp/example' host",
        "/usr/local/bin/sftp -- -oProxyCommand='rm -rf /tmp/example' host",
        "KUBECTL_EXTERNAL_DIFF='rm -rf /tmp/example' kubectl get pods",
        "KUBECTL_EXTERNAL_DIFF='rm -rf /tmp/example' kubectl diff --help",
        "KUBECTL_EXTERNAL_DIFF='rm -rf /tmp/example' oc diff -f app.yaml",
        "tar -cf out.tar .",
        "tar -- --checkpoint-action=exec='rm -rf /tmp/example'",
        "tar -- --to-command='rm -rf /tmp/example'",
        "tar -- --use-compress-program='rm -rf /tmp/example'",
        "/opt/homebrew/bin/gtar -- --to-command='rm -rf /tmp/example'",
        "socat - TCP-CONNECT:example.com:80",
        "socat - OPEN:/tmp/rm-rf-example",
        "/opt/homebrew/bin/socat -- - TCP-CONNECT:example.com:80",
        "/opt/homebrew/bin/gtime --help rm -rf /tmp/example",
        "/opt/homebrew/bin/timeout --help 5 rm -rf /tmp/example",
        "/opt/homebrew/bin/stdbuf --help rm -rf /tmp/example",
        "/opt/homebrew/bin/script -q /tmp/typescript rm -rf /tmp/example",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
}

#[test]
fn bash_deny_rules_match_commands_after_leading_redirections() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: Vec::new(),
        denied_rules: vec![PermissionRule::parse("Bash(rm:*)")],
        ..PermissionContext::default()
    };

    for command in [
        ">/tmp/out rm -rf /tmp/example",
        "2>/tmp/err rm -rf /tmp/example",
        "< /tmp/input rm -rf /tmp/example",
        ">> /tmp/out 2>&1 rm -rf /tmp/example",
        "FOO=bar >/tmp/out rm -rf /tmp/example",
        "FOO=bar 2>/tmp/err sudo -n rm -rf /tmp/example",
        "FOO=$'bar baz' >/tmp/out rm -rf /tmp/example",
        ">/tmp/out FOO=bar 2>/tmp/err rm -rf /tmp/example",
    ] {
        assert!(
            permissions
                .tool_denied(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_some(),
            "{command}"
        );
    }
}

#[test]
fn bash_allow_rules_strip_safe_env_vars_and_wrappers() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::parse("Bash(cargo test:*)")],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    for command in [
        "RUST_BACKTRACE=1 timeout --foreground 30 cargo test -p orbcode-core",
        "timeout 30 -- cargo test -p orbcode-core",
        "time -p cargo test -p orbcode-core",
        "/usr/bin/time -p cargo test -p orbcode-core",
        "/usr/bin/time --format=%E --output=/tmp/time.log cargo test -p orbcode-core",
        "/usr/local/bin/time --format=%E cargo test -p orbcode-core",
        "/opt/homebrew/bin/gtime -o /tmp/time.log cargo test -p orbcode-core",
        "nohup cargo test -p orbcode-core",
        "nice -n1 cargo test -p orbcode-core",
        "stdbuf -oL cargo test -p orbcode-core",
        "/usr/bin/timeout 30 /usr/bin/nice -n1 /usr/bin/nohup /usr/bin/stdbuf -oL cargo test -p orbcode-core",
        "/opt/homebrew/bin/timeout 30 /usr/local/bin/nice -n1 /opt/homebrew/bin/nohup /usr/local/bin/stdbuf -oL cargo test -p orbcode-core",
        "/usr/bin/xargs -0 -r cargo test -p orbcode-core",
        "/opt/homebrew/bin/xargs -0 -r cargo test -p orbcode-core",
    ] {
        assert!(
            permissions
                .tool_allowed(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_some(),
            "{command}"
        );
    }
}

#[test]
fn bash_allow_rules_do_not_strip_unsafe_env_vars() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::parse("Bash(cargo test:*)")],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"PATH=/tmp/fake cargo test -p orbcode-core"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"env PATH=/tmp/fake cargo test -p orbcode-core"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"/opt/homebrew/bin/env PATH=/tmp/fake cargo test -p orbcode-core"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"env -C /tmp cargo test -p orbcode-core"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"/opt/homebrew/bin/env -S 'cargo test -p orbcode-core'"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"env -S 'cargo test -p orbcode-core'"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"env --argv0=cargo cargo test -p orbcode-core"}"#
            )
            .is_none()
    );
    for command in [
        "command cargo test -p orbcode-core",
        "command -p cargo test -p orbcode-core",
        "exec cargo test -p orbcode-core",
        "sudo cargo test -p orbcode-core",
        "sudo -uroot cargo test -p orbcode-core",
        "sudo --set-home cargo test -p orbcode-core",
        "sudo --login cargo test -p orbcode-core",
        "/opt/homebrew/bin/sudo cargo test -p orbcode-core",
        "su -c 'cargo test -p orbcode-core'",
        "su --command='cargo test -p orbcode-core' root",
        "/opt/homebrew/bin/su -c 'cargo test -p orbcode-core'",
        "runuser -u root cargo test -p orbcode-core",
        "runuser -u root -c 'cargo test -p orbcode-core'",
        "/opt/homebrew/bin/runuser -u root cargo test -p orbcode-core",
        "setpriv --reuid 0 --regid 0 --clear-groups cargo test -p orbcode-core",
        "/opt/homebrew/bin/setpriv --no-new-privs cargo test -p orbcode-core",
        "prlimit --nofile=1024:2048 cargo test -p orbcode-core",
        "/opt/homebrew/bin/prlimit --nofile=1024:2048 cargo test -p orbcode-core",
        "numactl --cpunodebind=0 --membind=0 cargo test -p orbcode-core",
        "/opt/homebrew/bin/numactl --cpunodebind=0 --membind=0 cargo test -p orbcode-core",
        "setarch x86_64 -R cargo test -p orbcode-core",
        "/opt/homebrew/bin/setarch x86_64 -R cargo test -p orbcode-core",
        "linux32 -B cargo test -p orbcode-core",
        "/opt/homebrew/bin/linux64 --uname-2.6 -- cargo test -p orbcode-core",
        "doas cargo test -p orbcode-core",
        "doas -uroot cargo test -p orbcode-core",
        "/opt/homebrew/bin/doas cargo test -p orbcode-core",
        "pkexec cargo test -p orbcode-core",
        "/opt/homebrew/bin/pkexec cargo test -p orbcode-core",
        "chroot /tmp/root cargo test -p orbcode-core",
        "/opt/homebrew/bin/chroot /tmp/root cargo test -p orbcode-core",
        "flock /tmp/lock cargo test -p orbcode-core",
        "flock -c 'cargo test -p orbcode-core' /tmp/lock",
        "/opt/homebrew/bin/flock /tmp/lock cargo test -p orbcode-core",
        "/opt/homebrew/bin/flock -c 'cargo test -p orbcode-core' /tmp/lock",
        "watch cargo test -p orbcode-core",
        "/opt/homebrew/bin/watch cargo test -p orbcode-core",
        "setsid cargo test -p orbcode-core",
        "/opt/homebrew/bin/setsid cargo test -p orbcode-core",
        "ssh-agent cargo test -p orbcode-core",
        "ssh-agent -t 1h -- cargo test -p orbcode-core",
        "/opt/homebrew/bin/ssh-agent -t 1h -- cargo test -p orbcode-core",
        "gpg-agent --daemon cargo test -p orbcode-core",
        "/usr/local/bin/gpg-agent --daemon cargo test -p orbcode-core",
        "dbus-run-session -- cargo test -p orbcode-core",
        "/opt/homebrew/bin/dbus-run-session -- cargo test -p orbcode-core",
        "ionice cargo test -p orbcode-core",
        "/opt/homebrew/bin/ionice cargo test -p orbcode-core",
        "chrt -f 99 cargo test -p orbcode-core",
        "/opt/homebrew/bin/chrt -f 99 cargo test -p orbcode-core",
        "taskset 0x1 cargo test -p orbcode-core",
        "/opt/homebrew/bin/taskset 0x1 cargo test -p orbcode-core",
        "unshare -Ur cargo test -p orbcode-core",
        "/opt/homebrew/bin/unshare -Ur cargo test -p orbcode-core",
        "nsenter --target 1 --mount -- cargo test -p orbcode-core",
        "/opt/homebrew/bin/nsenter --target 1 --mount -- cargo test -p orbcode-core",
        "strace -f -o /tmp/trace.log cargo test -p orbcode-core",
        "/opt/homebrew/bin/strace -f -o /tmp/trace.log cargo test -p orbcode-core",
        "bwrap --ro-bind / / cargo test -p orbcode-core",
        "/opt/homebrew/bin/bwrap --ro-bind / / cargo test -p orbcode-core",
        "firejail --quiet --noprofile cargo test -p orbcode-core",
        "/opt/homebrew/bin/firejail --quiet --noprofile cargo test -p orbcode-core",
        "systemd-run --user --scope cargo test -p orbcode-core",
        "/opt/homebrew/bin/systemd-run --user --scope cargo test -p orbcode-core",
        "script -c 'cargo test -p orbcode-core' /tmp/typescript",
        "bash -c 'cargo test -p orbcode-core'",
        "bash -c $'cargo test -p orbcode-core'",
        "bash -o pipefail -c 'cargo test -p orbcode-core'",
        "bash -O extglob -c 'cargo test -p orbcode-core'",
        "sh -c 'cargo test -p orbcode-core'",
        "/opt/homebrew/bin/bash -c 'cargo test -p orbcode-core'",
        "eval 'cargo test -p orbcode-core'",
        "eval $'cargo test -p orbcode-core'",
        "eval -- 'cargo test -p orbcode-core'",
        "builtin eval 'cargo test -p orbcode-core'",
        "trap 'cargo test -p orbcode-core' EXIT",
        "trap $'cargo test -p orbcode-core' EXIT",
        "builtin trap 'cargo test -p orbcode-core' EXIT",
    ] {
        assert!(
            permissions
                .tool_allowed(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
}

#[test]
fn bash_allow_rules_reject_commands_with_substitution() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::parse("Bash(echo:*)")],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    assert!(
        permissions
            .tool_allowed("bash", r#"{"command":"echo $(rm -rf /tmp/example)"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_allowed("bash", r#"{"command":"echo \"$(rm -rf /tmp/example)\""}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_allowed("bash", r#"{"command":"echo `rm -rf /tmp/example`"}"#)
            .is_none()
    );
    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"echo $(( $(rm -rf /tmp/example) + 1 ))"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"echo $[ $(rm -rf /tmp/example) + 1 ]"}"#
            )
            .is_none()
    );
    assert!(
        permissions
            .tool_allowed(
                "bash",
                r#"{"command":"echo ${missing:-$(rm -rf /tmp/example)}"}"#
            )
            .is_none()
    );
}

#[test]
fn bash_allow_rules_reject_argv_execution_prefix_matches() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![
            PermissionRule::parse("Bash(find:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/find:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/find:*)"),
            PermissionRule::parse("Bash(parallel:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/parallel:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/parallel:*)"),
            PermissionRule::parse("Bash(ssh:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/ssh:*)"),
            PermissionRule::parse("Bash(sshpass:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/sshpass:*)"),
            PermissionRule::parse("Bash(git:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/git:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/git:*)"),
            PermissionRule::parse("Bash(npm:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/npm:*)"),
            PermissionRule::parse("Bash(npx:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/npx:*)"),
            PermissionRule::parse("Bash(pnpm:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/pnpm:*)"),
            PermissionRule::parse("Bash(pnpx:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/pnpx:*)"),
            PermissionRule::parse("Bash(yarn:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/yarn:*)"),
            PermissionRule::parse("Bash(bun:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/bun:*)"),
            PermissionRule::parse("Bash(bunx:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/bunx:*)"),
            PermissionRule::parse("Bash(poetry:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/poetry:*)"),
            PermissionRule::parse("Bash(pipenv:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/pipenv:*)"),
            PermissionRule::parse("Bash(uv:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/uv:*)"),
            PermissionRule::parse("Bash(uvx:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/uvx:*)"),
            PermissionRule::parse("Bash(pdm:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/pdm:*)"),
            PermissionRule::parse("Bash(hatch:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/hatch:*)"),
            PermissionRule::parse("Bash(conda:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/conda:*)"),
            PermissionRule::parse("Bash(mamba:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/mamba:*)"),
            PermissionRule::parse("Bash(micromamba:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/micromamba:*)"),
            PermissionRule::parse("Bash(bundle:*)"),
            PermissionRule::parse("Bash(bundler:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/bundle:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/bundler:*)"),
            PermissionRule::parse("Bash(rbenv:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/rbenv:*)"),
            PermissionRule::parse("Bash(pyenv:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/pyenv:*)"),
            PermissionRule::parse("Bash(nodenv:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/nodenv:*)"),
            PermissionRule::parse("Bash(asdf:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/asdf:*)"),
            PermissionRule::parse("Bash(mise:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/mise:*)"),
            PermissionRule::parse("Bash(direnv:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/direnv:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/direnv:*)"),
            PermissionRule::parse("Bash(nix:*)"),
            PermissionRule::parse("Bash(nix-shell:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/nix:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/nix-shell:*)"),
            PermissionRule::parse("Bash(guix:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/guix:*)"),
            PermissionRule::parse("Bash(watchexec:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/watchexec:*)"),
            PermissionRule::parse("Bash(entr:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/entr:*)"),
            PermissionRule::parse("Bash(tmux:*)"),
            PermissionRule::parse("Bash(screen:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/screen:*)"),
            PermissionRule::parse("Bash(docker:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/docker:*)"),
            PermissionRule::parse("Bash(podman:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/podman:*)"),
            PermissionRule::parse("Bash(kubectl:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/kubectl:*)"),
            PermissionRule::parse("Bash(oc:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/oc:*)"),
            PermissionRule::parse("Bash(socat:*)"),
        ],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    for command in ["find . -exec echo ok \\;", "find . -execdir echo ok +"] {
        assert!(
            permissions
                .tool_allowed(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
    for command in [
        "/usr/local/bin/find . -exec echo ok \\;",
        "/opt/homebrew/bin/find . -okdir echo ok +",
    ] {
        assert!(
            permissions
                .tool_allowed(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
    for command in ["find . -ok echo ok \\;", "find . -okdir echo ok +"] {
        assert!(
            permissions
                .tool_allowed(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
    for command in [
        "parallel 'echo ok' ::: target",
        "parallel ::: 'echo ok'",
        "parallel --will-cite -j 2 echo ok ::: target",
        "/usr/local/bin/parallel 'echo ok' ::: target",
        "/opt/homebrew/bin/parallel ::: 'echo ok'",
        "ssh example.com echo ok",
        "ssh -p 2222 example.com echo ok",
        "/usr/local/bin/ssh example.com echo ok",
        "sshpass -p secret ssh example.com echo ok",
        "sshpass -p secret echo ok",
        "/opt/homebrew/bin/sshpass -p secret ssh example.com echo ok",
        "/opt/homebrew/bin/sshpass -p secret echo ok",
        "git bisect run echo ok",
        "/usr/local/bin/git bisect run echo ok",
        "/opt/homebrew/bin/git -C repo bisect run echo ok",
        "npm exec -- echo ok",
        "npm exec --package=foo -- echo ok",
        "/opt/homebrew/bin/npm exec -- echo ok",
        "npx echo ok",
        "/usr/local/bin/npx echo ok",
        "pnpm exec echo ok",
        "pnpm dlx echo ok",
        "/opt/homebrew/bin/pnpm dlx echo ok",
        "pnpx echo ok",
        "/usr/local/bin/pnpx echo ok",
        "yarn exec echo ok",
        "yarn dlx echo ok",
        "/opt/homebrew/bin/yarn dlx echo ok",
        "bunx echo ok",
        "/usr/local/bin/bunx echo ok",
        "bun x echo ok",
        "/opt/homebrew/bin/bun x echo ok",
        "poetry run echo ok",
        "pipenv run echo ok",
        "uv run echo ok",
        "uvx echo ok",
        "pdm run echo ok",
        "hatch run echo ok",
        "/opt/homebrew/bin/poetry run echo ok",
        "/usr/local/bin/pipenv run echo ok",
        "/opt/homebrew/bin/uv run echo ok",
        "/usr/local/bin/uvx echo ok",
        "/opt/homebrew/bin/pdm run echo ok",
        "/opt/homebrew/bin/hatch run echo ok",
        "conda run -n env echo ok",
        "mamba run -n env echo ok",
        "micromamba run -n env echo ok",
        "/usr/local/bin/conda run -n env echo ok",
        "/opt/homebrew/bin/mamba run -n env echo ok",
        "/usr/local/bin/micromamba run -n env echo ok",
        "bundle exec echo ok",
        "bundler exec echo ok",
        "rbenv exec echo ok",
        "pyenv exec echo ok",
        "nodenv exec echo ok",
        "asdf exec echo ok",
        "mise exec echo ok",
        "/opt/homebrew/bin/bundle exec echo ok",
        "/usr/local/bin/bundler exec echo ok",
        "/opt/homebrew/bin/rbenv exec echo ok",
        "/usr/local/bin/pyenv exec echo ok",
        "/opt/homebrew/bin/nodenv exec echo ok",
        "/usr/local/bin/asdf exec echo ok",
        "/opt/homebrew/bin/mise exec echo ok",
        "direnv exec . echo ok",
        "/usr/local/bin/direnv exec . echo ok",
        "/opt/homebrew/bin/direnv exec /tmp/project echo ok",
        "nix develop . --command echo ok",
        "nix shell nixpkgs#coreutils -c echo ok",
        "nix-shell --run 'echo ok'",
        "guix shell -- echo ok",
        "watchexec -- echo ok",
        "entr echo ok",
        "/opt/homebrew/bin/nix develop . --command echo ok",
        "/usr/local/bin/nix-shell --run 'echo ok'",
        "/opt/homebrew/bin/guix shell -- echo ok",
        "/usr/local/bin/watchexec -- echo ok",
        "/opt/homebrew/bin/entr echo ok",
        "screen -dmS cleanup echo ok",
        "/usr/local/bin/screen -dmS cleanup echo ok",
        "docker exec app echo ok",
        "/opt/homebrew/bin/docker exec app echo ok",
        "docker run --rm alpine echo ok",
        "docker compose exec app echo ok",
        "/opt/homebrew/bin/docker compose exec app echo ok",
        "docker compose run --rm app echo ok",
        "podman exec app echo ok",
        "/usr/local/bin/podman exec app echo ok",
        "kubectl exec pod/app -- echo ok",
        "kubectl -n prod exec pod/app -- echo ok",
        "/usr/local/bin/kubectl exec pod/app -- echo ok",
        "oc exec pod/app -- echo ok",
        "/opt/homebrew/bin/oc exec pod/app -- echo ok",
        "socat - 'EXEC:echo ok'",
    ] {
        assert!(
            permissions
                .tool_allowed(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
}

#[test]
fn bash_allow_rules_reject_command_string_prefix_matches() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![
            PermissionRule::parse("Bash(bash:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/bash:*)"),
            PermissionRule::parse("Bash(eval:*)"),
            PermissionRule::parse("Bash(trap:*)"),
            PermissionRule::parse("Bash(sudo:*)"),
            PermissionRule::parse("Bash(sudoedit:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/sudo:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/sudoedit:*)"),
            PermissionRule::parse("Bash(systemctl:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/systemctl:*)"),
            PermissionRule::parse("Bash(kubectl:*)"),
            PermissionRule::parse("Bash(oc:*)"),
            PermissionRule::parse("Bash(sg:*)"),
            PermissionRule::parse("Bash(crontab:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/crontab:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/crontab:*)"),
            PermissionRule::parse("Bash(ssh:*)"),
            PermissionRule::parse("Bash(scp:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/scp:*)"),
            PermissionRule::parse("Bash(sftp:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/sftp:*)"),
            PermissionRule::parse("Bash(git:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/git:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/git:*)"),
            PermissionRule::parse("Bash(kubectl:*)"),
            PermissionRule::parse("Bash(rsync:*)"),
            PermissionRule::parse("Bash(svn:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/svn:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/svn:*)"),
            PermissionRule::parse("Bash(hg:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/hg:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/hg:*)"),
            PermissionRule::parse("Bash(tar:*)"),
            PermissionRule::parse("Bash(man:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/man:*)"),
            PermissionRule::parse("Bash(less:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/less:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/less:*)"),
            PermissionRule::parse("Bash(xdg-open:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/xdg-open:*)"),
            PermissionRule::parse("Bash(sensible-browser:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/sensible-browser:*)"),
            PermissionRule::parse("Bash(/usr/local/bin/x-www-browser:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/gnome-open:*)"),
            PermissionRule::parse("Bash(npm:*)"),
            PermissionRule::parse("Bash(npx:*)"),
            PermissionRule::parse("Bash(nix-shell:*)"),
            PermissionRule::parse("Bash(socat:*)"),
            PermissionRule::parse("Bash(/opt/homebrew/bin/script:*)"),
        ],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    for command in [
        "bash -c 'echo ok'",
        "/opt/homebrew/bin/bash -c 'echo ok'",
        "eval 'echo ok'",
        "trap 'echo ok' EXIT",
        "SUDO_ASKPASS='echo ok' sudo -A true",
        "SUDO_ASKPASS='echo ok' /usr/bin/sudo --askpass true",
        "SUDO_ASKPASS='echo ok' /usr/local/bin/sudo --askpass true",
        "SUDO_ASKPASS='echo ok' /opt/homebrew/bin/sudo -A true",
        "sg adm -c 'echo ok'",
        "/opt/homebrew/bin/sg adm -c 'echo ok'",
        "ssh -o ProxyCommand='echo ok' example.com",
        "ssh -oLocalCommand='echo ok' example.com",
        "/opt/homebrew/bin/ssh -o ProxyCommand='echo ok' example.com",
        "SSH_ASKPASS='echo ok' ssh example.com",
        "SSH_ASKPASS='echo ok' ssh -N example.com",
        "SSH_ASKPASS='echo ok' /opt/homebrew/bin/ssh example.com",
        "scp -S 'echo ok' src host:dst",
        "/opt/homebrew/bin/scp -S 'echo ok' src host:dst",
        "sftp -oProxyCommand='echo ok' host",
        "/usr/local/bin/sftp -oProxyCommand='echo ok' host",
        "GIT_SSH_COMMAND='ssh -i /tmp/key' git fetch origin",
        "GIT_SSH_COMMAND='ssh -i /tmp/key' /opt/homebrew/bin/git fetch origin",
        "env GIT_PROXY_COMMAND='nc proxy 22' git ls-remote origin",
        "GIT_ASKPASS='echo ok' git fetch origin",
        "SSH_ASKPASS='echo ok' git ls-remote origin",
        "GIT_EXTERNAL_DIFF='echo ok' git diff",
        "env GIT_EXTERNAL_DIFF='echo ok' git show HEAD",
        "git -c core.sshCommand='ssh -i /tmp/key' fetch origin",
        "git -C repo -c core.sshCommand='ssh -i /tmp/key' pull",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.sshCommand GIT_CONFIG_VALUE_0='ssh -i /tmp/key' git fetch origin",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.sshCommand GIT_CONFIG_VALUE_0='ssh -i /tmp/key' /usr/local/bin/git fetch origin",
        "GIT_SSH_CMD='ssh -i /tmp/key' git --config-env=core.sshCommand=GIT_SSH_CMD fetch origin",
        "GIT_SSH_CMD='ssh -i /tmp/key' /opt/homebrew/bin/git --config-env=core.sshCommand=GIT_SSH_CMD fetch origin",
        "git -c core.askPass='echo ok' clone https://example/repo",
        "git -c core.askPass='echo ok' credential fill",
        "git -c diff.external='echo ok' diff HEAD",
        "git -c diff.external='echo ok' show HEAD",
        "KUBECTL_EXTERNAL_DIFF='echo ok' kubectl diff -f app.yaml",
        "git -c credential.helper='!echo ok' credential fill",
        "git -c credential.helper='!echo ok' fetch origin",
        "git -c difftool.cleanup.cmd='echo ok' difftool --tool cleanup HEAD",
        "git -c mergetool.cleanup.cmd='echo ok' mergetool -t cleanup",
        "/opt/homebrew/bin/git -c credential.helper='!echo ok' fetch origin",
        "/usr/local/bin/git -c difftool.cleanup.cmd='echo ok' difftool --tool cleanup HEAD",
        "/opt/homebrew/bin/git -c mergetool.cleanup.cmd='echo ok' mergetool -t cleanup",
        "git -c core.editor='vim -n' commit",
        "git -c sequence.editor='vim -n' rebase -i main",
        "/opt/homebrew/bin/git -c core.editor='vim -n' commit",
        "GIT_EDITOR='vim -n' git commit",
        "GIT_SEQUENCE_EDITOR='vim -n' git rebase -i main",
        "VISUAL='vim -n' git rebase -i main",
        "EDITOR='vim -n' git tag -a v1.0",
        "EDITOR='vim -n' crontab -e",
        "VISUAL='vim -n' /usr/bin/crontab -u root -e",
        "VISUAL='vim -n' /usr/local/bin/crontab -u root -e",
        "EDITOR='vim -n' /opt/homebrew/bin/crontab -e",
        "SUDO_EDITOR='vim -n' sudoedit /etc/hosts",
        "SUDO_EDITOR='vim -n' /usr/local/bin/sudoedit /etc/hosts",
        "SUDO_EDITOR='vim -n' sudo -e /etc/hosts",
        "SUDO_EDITOR='vim -n' /usr/local/bin/sudo -e /etc/hosts",
        "SUDO_EDITOR='vim -n' /opt/homebrew/bin/sudo -e /etc/hosts",
        "VISUAL='vim -n' sudoedit /etc/hosts",
        "SYSTEMD_EDITOR='vim -n' systemctl edit ssh.service",
        "EDITOR='vim -n' systemctl --user edit app.service",
        "SYSTEMD_EDITOR='vim -n' /opt/homebrew/bin/systemctl --user edit app.service",
        "KUBE_EDITOR='vim -n' kubectl edit deploy/app",
        "EDITOR='vim -n' oc edit route/app",
        "SVN_EDITOR='vim -n' svn commit src",
        "EDITOR='vim -n' svn propedit svn:ignore .",
        "SVN_EDITOR='vim -n' /usr/local/bin/svn commit src",
        "EDITOR='vim -n' /opt/homebrew/bin/svn propedit svn:ignore .",
        "HGEDITOR='vim -n' hg commit",
        "HGEDITOR='vim -n' /usr/local/bin/hg commit",
        "EDITOR='vim -n' hg histedit tip",
        "HGEDITOR='vim -n' hg histedit --rev tip",
        "hg --config ui.editor='vim -n' commit",
        "hg --config=ui.editor='vim -n' histedit tip",
        "/opt/homebrew/bin/hg --config=ui.editor='vim -n' histedit tip",
        "hg --config ui.ssh='ssh -i /tmp/key' pull ssh://example/repo",
        "/usr/local/bin/hg --config ui.ssh='ssh -i /tmp/key' pull ssh://example/repo",
        "hg clone --ssh 'ssh -i /tmp/key' ssh://example/repo",
        "/opt/homebrew/bin/hg clone --ssh 'ssh -i /tmp/key' ssh://example/repo",
        "GIT_PAGER='less -R' git log",
        "GIT_PAGER='less -R' git --paginate status",
        "PAGER='less -R' git show HEAD",
        "git -c core.pager='less -R' diff HEAD",
        "git -c core.pager='less -R' --paginate status",
        "/opt/homebrew/bin/git -c core.pager='less -R' --paginate status",
        "MANPAGER='less -R' man ls",
        "PAGER='less -R' /usr/bin/man 1 printf",
        "MANPAGER='less -R' /opt/homebrew/bin/man 1 printf",
        "man -P 'less -R' ls",
        "/opt/homebrew/bin/man -P 'less -R' ls",
        "BROWSER='firefox' xdg-open https://example.com",
        "BROWSER='firefox' /opt/homebrew/bin/xdg-open https://example.com",
        "BROWSER='firefox' sensible-browser https://example.com",
        "BROWSER='firefox' /usr/local/bin/sensible-browser https://example.com",
        "BROWSER='firefox' /usr/local/bin/x-www-browser https://example.com",
        "BROWSER='firefox' /opt/homebrew/bin/gnome-open https://example.com",
        "LESSOPEN='|echo ok' less README.md",
        "LESSOPEN='|echo ok' /usr/local/bin/less README.md",
        "LESSOPEN='|echo ok' /opt/homebrew/bin/less README.md",
        "LESSCLOSE='echo ok' less README.md",
        "LESSCLOSE='echo ok' /usr/local/bin/less README.md",
        "LESSCLOSE='echo ok' /opt/homebrew/bin/less README.md",
        "RSYNC_RSH='ssh -i /tmp/key' rsync -av src host:dst",
        "RSYNC_RSH='ssh -i /tmp/key' /opt/homebrew/bin/rsync -av src host:dst",
        "rsync -e 'ssh -i /tmp/key' src host:dst",
        "rsync --rsh='ssh -i /tmp/key' src host:dst",
        "/opt/homebrew/bin/rsync --rsh='ssh -i /tmp/key' src host:dst",
        "SVN_SSH='ssh -i /tmp/key' svn checkout svn+ssh://example/repo",
        "SVN_SSH='ssh -i /tmp/key' /opt/homebrew/bin/svn checkout svn+ssh://example/repo",
        "SVN_SSH='ssh -i /tmp/key' /usr/local/bin/svn checkout svn+ssh://example/repo",
        "tar --checkpoint-action=exec='echo ok' -cf out.tar .",
        "tar --to-command='echo ok' -xf in.tar",
        "/opt/homebrew/bin/tar --to-command='echo ok' -xf in.tar",
        "tar -I 'echo ok' -cf out.tar .",
        "tar --use-compress-program='echo ok' -cf out.tar .",
        "/usr/local/bin/gtar --use-compress-program='echo ok' -cf out.tar .",
        "git -c alias.cleanup='!echo ok' cleanup",
        "git difftool --extcmd='echo ok' HEAD",
        "/usr/local/bin/git difftool --extcmd='echo ok' HEAD",
        "/opt/homebrew/bin/git -C repo difftool -x 'echo ok' main",
        "git submodule foreach 'echo ok'",
        "git filter-branch --tree-filter 'echo ok' HEAD",
        "git rebase --exec 'echo ok' main",
        "/usr/local/bin/git -c alias.cleanup='!echo ok' cleanup",
        "/opt/homebrew/bin/git submodule foreach 'echo ok'",
        "/usr/local/bin/git filter-branch --tree-filter 'echo ok' HEAD",
        "/opt/homebrew/bin/git rebase --exec 'echo ok' main",
        "npm exec -c 'echo ok'",
        "npx --call='echo ok'",
        "nix-shell --run 'echo ok'",
        "socat - 'SYSTEM:echo ok'",
        "socat - 'SHELL:echo ok'",
        "/usr/local/bin/socat - 'SYSTEM:echo ok'",
        "/opt/homebrew/bin/socat - 'SHELL:echo ok'",
        "/opt/homebrew/bin/script -q -c 'echo ok' /tmp/typescript",
        "tmux new-session -d -s cleanup 'echo ok'",
        "tmux split-window -h 'echo ok'",
        "/opt/homebrew/bin/tmux split-window -h 'echo ok'",
    ] {
        assert!(
            permissions
                .tool_allowed(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
}

#[test]
fn bash_allow_rules_reject_heredoc_prefix_matches() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::parse("Bash(cat:*)")],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    assert!(
        permissions
            .tool_allowed(
                "bash",
                &serde_json::json!({ "command": "cat <<EOF\nhello\nEOF" }).to_string()
            )
            .is_none()
    );
}

#[test]
fn bash_allow_rules_reject_shell_control_keyword_compounds() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::parse("Bash(echo:*)")],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    for command in [
        "if true; then echo ok; fi",
        "while true; do echo ok; break; done",
        "for path in src tests; do echo \"$path\"; done",
        "select choice in yes no; do echo \"$choice\"; break; done",
        "case \"$target\" in safe) echo ok ;; esac",
        "cleanup(){ echo ok; }; cleanup",
        "function cleanup { echo ok; }",
        "time { echo ok; }",
        "time (echo ok)",
    ] {
        assert!(
            permissions
                .tool_allowed(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
}

#[test]
fn bash_allow_rules_reject_prefix_matches_with_redirection() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::parse("Bash(echo:*)")],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    for command in [
        "echo ok > /tmp/example",
        "echo ok 2>/tmp/example",
        "echo ok &>/tmp/example",
        "echo ok < /tmp/example",
        "> /tmp/example echo ok",
    ] {
        assert!(
            permissions
                .tool_allowed(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
}

#[test]
fn bash_allow_rules_reject_prefix_matches_with_grouping() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![
            PermissionRule::parse("Bash(echo:*)"),
            PermissionRule::parse("Bash(rm:*)"),
        ],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    for command in [
        "echo ok && (rm -rf /tmp/example)",
        "echo ok && { rm -rf /tmp/example; }",
    ] {
        assert!(
            permissions
                .tool_allowed(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .is_none(),
            "{command}"
        );
    }
}

#[test]
fn exact_bash_rules_can_still_allow_redirection_after_explicit_approval() {
    let command = "printf ok > /tmp/example";
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::for_tool("bash", command)],
        denied_rules: Vec::new(),
        ..PermissionContext::default()
    };

    assert!(
        permissions
            .tool_allowed(
                "bash",
                &serde_json::json!({ "command": command }).to_string()
            )
            .is_some()
    );
}

#[test]
fn matches_mcp_server_level_tool_rules() {
    let rule = PermissionRule::parse("mcp__server__*");
    assert!(rule.matches_tool_wide("mcp__server__read"));
    assert!(!rule.matches_tool_wide("mcp__other__read"));
}

#[test]
fn matches_mcp_adapter_inputs_against_mcp_rules() {
    let exact_rule = PermissionRule::parse("mcp__docs__inspect");
    let wildcard_rule = PermissionRule::parse("mcp__docs__*");
    let server_rule = PermissionRule::parse("mcp__docs");
    let input = r#"{"server_id":"docs","tool_name":"inspect","input":{"mode":"brief"}}"#;

    assert!(exact_rule.matches_tool_call("call-mcp-tool", input));
    assert!(wildcard_rule.matches_tool_call("call-mcp-tool", input));
    assert!(server_rule.matches_tool_call("call-mcp-tool", input));
    assert!(!exact_rule.matches_tool_call(
        "call-mcp-tool",
        r#"{"server_id":"docs","tool_name":"other"}"#
    ));

    let resource_rule = PermissionRule::parse("mcp__docs__resources");
    assert!(resource_rule.matches_tool_call(
        "read-mcp-resource",
        r#"{"server_id":"docs","uri":"workspace://cwd"}"#
    ));
    assert!(server_rule.matches_tool_call(
        "read-mcp-resource",
        r#"{"server_id":"docs","uri":"workspace://cwd"}"#
    ));
}

#[test]
fn mcp_permission_context_uses_derived_target_for_allow_and_deny() {
    let permissions = PermissionContext {
        allow_network: true,
        provider_allow_network: true,
        allow_tools: false,
        allowed_rules: vec![PermissionRule::parse("mcp__docs__inspect")],
        denied_rules: vec![PermissionRule::parse("mcp__docs__delete")],
        ..PermissionContext::default()
    };

    assert!(permissions.tool_allowed_without_prompt(
        "call-mcp-tool",
        r#"{"server_id":"docs","tool_name":"inspect"}"#
    ));
    assert!(
        permissions
            .tool_denied(
                "call-mcp-tool",
                r#"{"server_id":"docs","tool_name":"delete"}"#
            )
            .is_some()
    );
}

#[test]
fn matches_file_group_directory_rules_against_file_tools() {
    let rule = PermissionRule::parse("File(/tmp/project/src/**)");

    assert!(rule.matches_tool_call(
        "Write",
        r#"{"file_path":"/tmp/project/src/lib.rs","content":"fn main() {}"}"#
    ));
    assert!(rule.matches_tool_call("Read", r#"{"file_path":"/tmp/project/src/lib.rs"}"#));
    assert!(rule.matches_tool_call(
        "Edit",
        r#"{"file_path":"/tmp/project/src/lib.rs","old_string":"a","new_string":"b"}"#
    ));
    assert!(!rule.matches_tool_call(
        "Write",
        r#"{"file_path":"/tmp/project/tests/lib.rs","content":"fn main() {}"}"#
    ));
    assert!(!rule.matches_tool_call("Bash", r#"{"command":"cat /tmp/project/src/lib.rs"}"#));
}

#[test]
fn matches_relative_directory_rules_only_against_relative_paths() {
    let rule = PermissionRule::parse("File(./**)");

    assert!(rule.matches_tool_call("Read", r#"{"file_path":"src/lib.rs"}"#));
    assert!(!rule.matches_tool_call("Read", r#"{"file_path":"/tmp/src/lib.rs"}"#));
    assert!(!rule.matches_tool_call("Read", r#"{"file_path":"../src/lib.rs"}"#));
}

#[test]
fn matches_search_tool_path_rules() {
    let glob = PermissionRule::parse("Glob(orbcode/**)");
    assert!(glob.matches_tool_call("Glob", r#"{"pattern":"**/*.rs","path":"orbcode"}"#));
    assert!(!glob.matches_tool_call("Glob", r#"{"pattern":"**/*.rs","path":"src"}"#));

    let grep = PermissionRule::parse("Grep(orbcode/**)");
    assert!(grep.matches_tool_call("Grep", r#"{"pattern":"Permission","path":"orbcode"}"#));
}

#[test]
fn additional_directories_allow_file_and_search_paths() {
    let root = tempfile::tempdir().expect("root");
    let cwd = root.path().join("workspace");
    let extra = root.path().join("extra");
    std::fs::create_dir_all(&cwd).expect("cwd");
    std::fs::create_dir_all(&extra).expect("extra");
    let extra = std::fs::canonicalize(extra).expect("canonical extra");
    let file = extra.join("src/lib.rs");
    std::fs::create_dir_all(file.parent().expect("file parent")).expect("parent");

    let permissions = PermissionContext {
        cwd,
        additional_directories: vec![extra.clone()],
        ..PermissionContext::default()
    };

    assert!(
        permissions.tool_allowed_without_prompt(
            "Write",
            &serde_json::json!({
                "file_path": file,
                "content": "fn main() {}"
            })
            .to_string()
        )
    );
    assert!(
        permissions.tool_allowed_without_prompt(
            "Grep",
            &serde_json::json!({
                "pattern": "fn main",
                "path": extra.join("src")
            })
            .to_string()
        )
    );
    assert!(!permissions.tool_allowed_without_prompt(
        "Write",
        r#"{"file_path":"/tmp/outside.rs","content":"fn main() {}"}"#
    ));
}
