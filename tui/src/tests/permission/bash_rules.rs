use crate::tests::support::*;

#[test]
fn permission_panel_suggests_grouped_rules_for_compound_bash_commands() {
    let command = r#"echo "=== app-server ===" && find orbcode/app-server/src -name "*.rs" | xargs wc -l 2>/dev/null | tail -1"#;
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({
            "command": command,
            "description": "统计 app-server 模块代码行数"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    assert_eq!(
        permission.always_allow_rules,
        vec![
            "echo:*".to_string(),
            "find:*".to_string(),
            "xargs wc -l 2>/dev/null".to_string(),
            "tail:*".to_string(),
        ]
    );

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);
    assert!(permission.option_lines().iter().any(|line| {
        line.contains("Yes, and don't ask again for: echo, find, and 2 more commands")
    }));
    assert!(!rendered.iter().any(|line| line.contains(r"find:*")));
}

#[test]
fn bash_permission_suggestions_group_shell_control_syntax() {
    let cases = [
        (
            r#"grep -B 2 -A 10 "match tool_name.as_str()" orbcode/tools/src/lib.rs | head -100"#,
            vec!["grep:*", "head:*"],
        ),
        (
            "rg ToolSpec orbcode/tools/src; wc -l orbcode/tools/src/lib.rs",
            vec!["rg:*", "wc:*"],
        ),
        (
            "echo one\nwc -l orbcode/Cargo.toml",
            vec!["echo one\nwc -l orbcode/Cargo.toml"],
        ),
        ("cat <<EOF\nhello\nEOF", vec!["cat <<EOF\nhello\nEOF"]),
        ("echo $(date)", vec!["echo $(date)"]),
        (
            r#"grep "ToolSpec\s*\{" orbcode/tools/src/lib.rs | head -100"#,
            vec!["grep:*", "head:*"],
        ),
        (
            "if true; then echo ok; fi",
            vec!["if true; then echo ok; fi"],
        ),
        (
            "while true; do echo ok; break; done",
            vec!["while true; do echo ok; break; done"],
        ),
        (
            "for path in src tests; do echo \"$path\"; done",
            vec!["for path in src tests; do echo \"$path\"; done"],
        ),
        (
            "select choice in yes no; do echo \"$choice\"; break; done",
            vec!["select choice in yes no; do echo \"$choice\"; break; done"],
        ),
        (
            "case \"$target\" in safe) echo ok ;; esac",
            vec!["case \"$target\" in safe) echo ok ;; esac"],
        ),
        ("bash -c 'echo ok'", vec!["bash -c 'echo ok'"]),
        (
            "bash -o pipefail -c 'echo ok'",
            vec!["bash -o pipefail -c 'echo ok'"],
        ),
        (
            "bash -O extglob -c 'echo ok'",
            vec!["bash -O extglob -c 'echo ok'"],
        ),
        ("bash --norc -c 'echo ok'", vec!["bash --norc -c 'echo ok'"]),
        (
            "bash --rcfile=/tmp/bashrc -c 'echo ok'",
            vec!["bash --rcfile=/tmp/bashrc -c 'echo ok'"],
        ),
        ("sh -lc 'echo ok'", vec!["sh -lc 'echo ok'"]),
        (
            "cleanup(){ echo ok; }; cleanup",
            vec!["cleanup(){ echo ok; }; cleanup"],
        ),
        (
            "function cleanup { echo ok; }",
            vec!["function cleanup { echo ok; }"],
        ),
        ("eval 'echo ok'", vec!["eval 'echo ok'"]),
        ("eval -- 'echo ok'", vec!["eval -- 'echo ok'"]),
        (
            "coproc cleanup { echo ok; }",
            vec!["coproc cleanup { echo ok; }"],
        ),
        ("trap 'echo ok' EXIT", vec!["trap 'echo ok' EXIT"]),
        ("time { echo ok; }", vec!["time { echo ok; }"]),
        ("time (echo ok)", vec!["time (echo ok)"]),
    ];

    for (command, expected_rules) in cases {
        assert_eq!(
            suggested_bash_permission_rules(command),
            expected_rules
                .into_iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn bash_permission_exact_rule_labels_show_verbatim_commands() {
    let cases = [
        (
            r#"grep -E "impl.*Tool.*for" orbcode/tools/src/lib.rs | sed 's/.*impl.*for /|/' | sed 's/ .*//' | sort"#,
            r#"grep -E "impl.*Tool.*for" orbcode/tools/src/lib.rs | sed 's/.*impl.*for /|/' | sed 's/ .*//' | sort"#,
        ),
        ("cmd1 && cmd2 && cmd3", "cmd1 && cmd2 && cmd3"),
        ("cmd1; cmd2; cmd3", "cmd1; cmd2; cmd3"),
        ("cmd1 || cmd2", "cmd1 || cmd2"),
        (
            "cmd1 && cmd2 || cmd3; cmd4 | cmd5",
            "cmd1 && cmd2 || cmd3; cmd4 | cmd5",
        ),
        (
            r#"grep "ToolSpec\\s\*\\{" orbcode/tools/src/lib.rs | head -100"#,
            r#"grep "ToolSpec\s*\{" orbcode/tools/src/lib.rs | head -100"#,
        ),
        ("echo $(date)", "echo $(date)"),
    ];

    for (rule, expected_label) in cases {
        assert_eq!(friendly_bash_permission_rule_label(rule), expected_label);
    }
}

#[test]
fn bash_destructive_command_warnings_match_common_risks() {
    let cases = [
        ("git reset --hard HEAD~1", "discard uncommitted changes"),
        ("git push --force origin main", "overwrite remote history"),
        ("git clean -fd", "delete untracked files"),
        ("git checkout -- .", "discard all working tree changes"),
        ("git restore .", "discard all working tree changes"),
        ("git stash drop", "remove stashed changes"),
        ("git branch -D feature", "force-delete a branch"),
        ("git commit --no-verify -m x", "skip safety hooks"),
        ("git commit --amend", "rewrite the last commit"),
        ("rm -rf /tmp/example", "recursively force-remove"),
        ("rm -r dir", "recursively remove"),
        ("rm -f file.txt", "force-remove"),
        ("chmod -R 777 target", "recursively change file permissions"),
        ("chown -R root target", "recursively change file ownership"),
        ("psql -c 'DROP TABLE users'", "drop or truncate"),
        ("DELETE FROM users;", "delete all rows"),
        ("kubectl delete pod example", "delete Kubernetes"),
        ("terraform destroy", "destroy Terraform"),
        ("echo hi > output.txt", "overwrite a file"),
    ];

    for (command, warning) in cases {
        assert!(
            destructive_bash_command_warning(command).is_some_and(|value| value.contains(warning)),
            "{command} should warn about {warning}"
        );
    }
}

#[test]
fn bash_destructive_command_warnings_ignore_common_safe_forms() {
    for command in [
        "ls -la",
        "git status",
        "git clean -fdn",
        "git clean --dry-run -fd",
        "grep pattern file 2>/dev/null",
        "echo hi >> output.txt",
        "DELETE FROM users WHERE id = 1;",
    ] {
        assert_eq!(destructive_bash_command_warning(command), None, "{command}");
    }
}

#[test]
fn permission_panel_suggests_grouped_rules_for_piped_grep_commands() {
    let command =
        r#"grep -B 2 -A 10 "match tool_name.as_str()" orbcode/tools/src/lib.rs | head -100"#;
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({
            "command": command,
            "description": "查看工具匹配逻辑"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    assert_eq!(
        permission.always_allow_rules,
        vec!["grep:*".to_string(), "head:*".to_string()]
    );

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);
    assert!(
        permission
            .option_lines()
            .iter()
            .any(|line| line.contains("Yes, and don't ask again for: grep and head commands"))
    );
    assert!(!rendered.iter().any(|line| line.contains("grep ... | head")));
    assert!(!rendered.iter().any(|line| line.contains("head:*")));
}

#[test]
fn permission_panel_shows_destructive_bash_warning() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({
            "command": "git reset --hard HEAD~1",
            "description": "Discard local changes"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert!(
        rendered
            .iter()
            .any(|line| { line.contains("Warning Note: may discard uncommitted changes") })
    );
}

#[test]
fn permission_panel_shows_sandbox_escalation_request() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({
            "command": "printf escalated > outside.txt",
            "sandbox_permissions": "require_escalated"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    let panel = permission_panel_content(&permission, 100);
    let rendered = plain_text_lines(&panel.body);

    assert!(
        rendered.iter().any(|line| {
            line.contains("Sandbox Requests unsandboxed execution for this command")
        })
    );
    assert!(rendered.iter().any(|line| {
        line.contains(
            "Effect Bypasses configured filesystem and network sandbox restrictions for this run",
        )
    }));
}

fn bash_permission_state() -> PermissionOverlayState {
    PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({ "command": "rm -rf /tmp/x" }).to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    })
}

#[test]
fn y_shortcut_does_not_override_selected_deny_option() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use orbcode_app_server_client::PermissionDecision;

    let mut permission = bash_permission_state();
    // Navigate to the deny ("No") option, then press the `y` shortcut.
    permission.selected_option = permission.deny_index();
    let action = apply_permission_request_key(
        &mut permission,
        &KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
    );
    match action {
        PermissionRequestKeyAction::Permission { decision, .. } => {
            assert_eq!(
                decision,
                PermissionDecision::Deny,
                "`y` must not override an explicit deny selection into an approve"
            );
        }
        other => panic!("expected a permission decision, got {other:?}"),
    }
}

#[test]
fn enter_while_editing_empty_always_allow_rule_keeps_dialog_open() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut permission = bash_permission_state();
    // Simulate selecting the always-allow slot, entering edit mode, and clearing
    // the rule — which previously shifted `deny_index()` onto the selection and
    // turned Enter into a silent Deny.
    permission.selected_option = 1;
    permission.editing_rule = true;
    permission.always_allow_rule.clear();
    permission.always_allow_rules.clear();
    let action = apply_permission_request_key(
        &mut permission,
        &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(
        matches!(action, PermissionRequestKeyAction::None),
        "Enter on an empty always-allow rule must keep the dialog open, not decide"
    );
}

#[test]
fn permission_panel_keeps_prefix_suggestion_for_simple_bash_commands() {
    let permission = PermissionOverlayState::new(PermissionRequest {
        request_id: "req-1".to_string(),
        session_id: "session".to_string(),
        tool_use_id: "tool-1".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({
            "command": "cargo test -p orbcode-core"
        })
        .to_string(),
        requires_tools_permission: true,
        requires_network_permission: false,
    });

    assert_eq!(permission.always_allow_rule, "cargo test:*");
}
