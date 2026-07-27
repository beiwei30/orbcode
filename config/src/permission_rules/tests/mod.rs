use super::*;

fn bash_input(command: &str) -> String {
    serde_json::json!({ "command": command }).to_string()
}

fn file_path_input(path: &str) -> String {
    serde_json::json!({ "file_path": path }).to_string()
}

// Parser-owned edge cases covered here: rule precedence, bash AST/control-flow
// expansion, wrapper stripping, argv command-body extraction, stdin script
// bodies, path target/glob matching, and syntax normalization. Runtime prompt
// lifecycle and settings mutation remain outside this module.

// ---- deny-over-allow precedence ----

#[test]
fn deny_takes_precedence_over_allow_when_both_match() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    let allow = vec![PermissionRule::parse("Bash(rm:*)")];
    assert_eq!(
        resolve_bash_command_permission(&deny, &allow, "bash", &bash_input("rm -rf /tmp/x")),
        BashRuleDecision::Deny
    );
}

#[test]
fn allow_applies_when_no_deny_matches() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    let allow = vec![PermissionRule::parse("Bash(ls:*)")];
    assert_eq!(
        resolve_bash_command_permission(&deny, &allow, "bash", &bash_input("ls -la")),
        BashRuleDecision::Allow
    );
}

#[test]
fn ask_when_neither_allow_nor_deny_matches() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    let allow = vec![PermissionRule::parse("Bash(ls:*)")];
    assert_eq!(
        resolve_bash_command_permission(&deny, &allow, "bash", &bash_input("cat foo")),
        BashRuleDecision::Ask
    );
}

#[test]
fn deny_precedence_holds_even_when_every_segment_is_allowed() {
    // Each compound segment is covered by an allow rule, yet a deny rule on
    // one segment still wins.
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    let allow = vec![
        PermissionRule::parse("Bash(echo:*)"),
        PermissionRule::parse("Bash(rm:*)"),
    ];
    assert_eq!(
        resolve_bash_command_permission(
            &deny,
            &allow,
            "bash",
            &bash_input("echo hi && rm -rf /tmp")
        ),
        BashRuleDecision::Deny
    );
}

// ---- bash rule matching coverage ----

#[test]
fn allow_exact_rule_matches_only_exact_command() {
    let allow = vec![PermissionRule::parse("Bash(git status)")];
    assert!(bash_command_allowed_by_rules(&allow, "bash", &bash_input("git status")).is_some());
    assert!(bash_command_allowed_by_rules(&allow, "bash", &bash_input("git status -s")).is_none());
}

#[test]
fn allow_prefix_rule_matches_command_prefix() {
    let allow = vec![PermissionRule::parse("Bash(npm run test:*)")];
    assert!(bash_command_allowed_by_rules(&allow, "bash", &bash_input("npm run test")).is_some());
    assert!(
        bash_command_allowed_by_rules(&allow, "bash", &bash_input("npm run test -- --watch"))
            .is_some()
    );
    assert!(bash_command_allowed_by_rules(&allow, "bash", &bash_input("npm run build")).is_none());
}

#[test]
fn allow_prefix_rule_is_fail_closed_for_uncovered_compound_segment() {
    let allow = vec![PermissionRule::parse("Bash(echo:*)")];
    assert!(
        bash_command_allowed_by_rules(&allow, "bash", &bash_input("echo hi && rm -rf /tmp"))
            .is_none()
    );

    let allow_both = vec![
        PermissionRule::parse("Bash(echo:*)"),
        PermissionRule::parse("Bash(ls:*)"),
    ];
    assert!(
        bash_command_allowed_by_rules(&allow_both, "bash", &bash_input("echo hi && ls -la"))
            .is_some()
    );
}

#[test]
fn deny_rule_matches_any_compound_segment() {
    let rm = PermissionRule::parse("Bash(rm:*)");
    for command in [
        "echo hi && rm -rf /tmp",
        "echo hi; rm -rf /tmp",
        "cat foo | rm -rf /tmp",
    ] {
        assert!(
            rm.matches_tool_call_with_mode(
                "bash",
                &bash_input(command),
                PermissionRuleMatchMode::Deny
            ),
            "expected deny to match a segment in {command:?}"
        );
    }
}

#[test]
fn deny_rule_matches_commands_inside_control_structures() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    for command in [
        "if true; then rm -rf /tmp/cache; fi",
        "while true; do rm -rf /tmp/cache; break; done",
        r#"case "$target" in *) rm -rf /tmp/cache ;; esac"#,
    ] {
        assert_eq!(
            resolve_bash_command_permission(&deny, &[], "bash", &bash_input(command)),
            BashRuleDecision::Deny,
            "expected deny to scan control-flow body in {command:?}"
        );
    }
}

#[test]
fn deny_rule_matches_through_quote_and_whitespace_normalization() {
    let rm = PermissionRule::parse("Bash(rm -rf /tmp/cache)");
    for command in [
        r#"rm -rf "/tmp/cache""#,
        "rm -rf '/tmp/cache'",
        "rm   -rf   /tmp/cache",
    ] {
        assert!(
            rm.matches_tool_call_with_mode(
                "bash",
                &bash_input(command),
                PermissionRuleMatchMode::Deny
            ),
            "expected deny to match normalized form of {command:?}"
        );
    }
}

#[test]
fn escaped_wildcard_in_rule_matches_literal_star_only() {
    assert!(bash_command_matches_pattern("echo *", r"echo \*"));
    assert!(!bash_command_matches_pattern("echo foo", r"echo \*"));
    // An unescaped star is a wildcard.
    assert!(bash_command_matches_pattern("echo foo", "echo *"));
}

// ---- normalization to a consistent rule key ----

#[test]
fn normalization_collapses_whitespace_and_quotes_to_one_key() {
    for form in [
        "rm -rf /tmp/cache",
        "rm   -rf   /tmp/cache",
        r#"rm -rf "/tmp/cache""#,
        "rm -rf '/tmp/cache'",
    ] {
        assert_eq!(
            normalize_bash_command_for_rule(form, false).as_deref(),
            Some("rm -rf /tmp/cache"),
            "unexpected normalized key for {form:?}"
        );
    }
}

// ---- minimal usable allow-rule suggestions ----

#[test]
fn suggests_minimal_prefix_rule_for_simple_command() {
    assert_eq!(
        suggested_bash_permission_rules("ls -la"),
        vec!["ls:*".to_string()]
    );
}

#[test]
fn suggests_subcommand_scoped_prefix_rule() {
    assert_eq!(
        suggested_bash_permission_rules("cargo build --release"),
        vec!["cargo build:*".to_string()]
    );
}

#[test]
fn suggested_rule_allows_the_originating_command() {
    let command = "cargo build --release";
    let rules: Vec<_> = suggested_bash_permission_rules(command)
        .iter()
        .map(|suggestion| PermissionRule::parse(&format!("Bash({suggestion})")))
        .collect();
    assert!(bash_command_allowed_by_rules(&rules, "bash", &bash_input(command)).is_some());
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
fn suggests_grouped_bash_permission_rules_for_compound_commands() {
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

// ---- ANSI-C quoting ($'...') in allow/deny ----

#[test]
fn ansi_c_quoted_argument_matches_allow_rule() {
    let allow = vec![PermissionRule::parse("Bash(echo:*)")];
    assert!(
        bash_command_allowed_by_rules(&allow, "bash", &bash_input(r"echo $'hello\nworld'"))
            .is_some()
    );
}

#[test]
fn ansi_c_quoted_command_name_matches_allow_rule() {
    let allow = vec![PermissionRule::parse(r"Bash(echo $'hello\nworld')")];
    assert!(
        bash_command_allowed_by_rules(&allow, "bash", &bash_input(r"echo $'hello\nworld'"))
            .is_some()
    );
}

#[test]
fn deny_rule_matches_command_with_ansi_c_quoting() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    assert_eq!(
        resolve_bash_command_permission(&deny, &[], "bash", &bash_input(r"rm $'/tmp/evil\tpath'")),
        BashRuleDecision::Deny
    );
}

#[test]
fn ansi_c_quoting_normalized_for_deny_exact_match() {
    let deny = vec![PermissionRule::parse("Bash(rm -rf /tmp/evil path)")];
    assert_eq!(
        resolve_bash_command_permission(
            &deny,
            &[],
            "bash",
            &bash_input(r"rm -rf $'/tmp/evil path'")
        ),
        BashRuleDecision::Deny
    );
}

// ---- process substitution in deny ----

#[test]
fn deny_rule_detects_command_inside_process_substitution() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    assert_eq!(
        resolve_bash_command_permission(
            &deny,
            &[],
            "bash",
            &bash_input("diff <(rm -rf /tmp/secrets) /dev/null")
        ),
        BashRuleDecision::Deny
    );
}

#[test]
fn deny_rule_detects_command_substitution_in_argument() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    for command in [
        "echo $(rm -rf /tmp/secrets)",
        "echo `rm -rf /tmp/secrets`",
        r#"echo "prefix $(rm -rf /tmp/secrets)""#,
        r#"echo "prefix `rm -rf /tmp/secrets`""#,
    ] {
        assert_eq!(
            resolve_bash_command_permission(&deny, &[], "bash", &bash_input(command)),
            BashRuleDecision::Deny,
            "expected deny to scan nested substitution in {command:?}"
        );
    }
}

// ---- wrapper stripping in deny ----

#[test]
fn deny_rule_strips_privilege_wrappers_but_allow_rule_does_not() {
    let rule = PermissionRule::parse("Bash(rm:*)");
    let input = bash_input("sudo -n -- rm -rf /tmp/cache");

    assert!(rule.matches_tool_call_with_mode("bash", &input, PermissionRuleMatchMode::Deny));
    assert!(bash_command_allowed_by_rules(&[rule], "bash", &input).is_none());
}

#[test]
fn deny_rule_strips_nested_wrappers_but_allow_rule_does_not() {
    let rule = PermissionRule::parse("Bash(rm:*)");
    let input = bash_input("env -S 'sudo -n -- rm -rf /tmp/cache'");

    assert!(rule.matches_tool_call_with_mode("bash", &input, PermissionRuleMatchMode::Deny));
    assert!(bash_command_allowed_by_rules(&[rule], "bash", &input).is_none());
}

#[test]
fn deny_rule_strips_leading_env_assignments_and_redirections() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    for command in [
        "2>/tmp/permission.log rm -rf /tmp/cache",
        "TMPDIR=/tmp 2>/tmp/permission.log rm -rf /tmp/cache",
    ] {
        assert_eq!(
            resolve_bash_command_permission(&deny, &[], "bash", &bash_input(command)),
            BashRuleDecision::Deny,
            "expected deny to strip leading env/redirection in {command:?}"
        );
    }
}

#[test]
fn deny_rule_scans_shell_command_string_but_allow_rule_does_not() {
    let rule = PermissionRule::parse("Bash(rm:*)");
    let input = bash_input("bash -lc 'rm -rf /tmp/cache'");

    assert!(rule.matches_tool_call_with_mode("bash", &input, PermissionRuleMatchMode::Deny));
    assert!(bash_command_allowed_by_rules(&[rule], "bash", &input).is_none());
}

#[test]
fn deny_rule_scans_argv_command_bodies_but_allow_rule_does_not() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    let find_exec = bash_input(r"find . -name cache -exec rm -rf {} \;");
    assert_eq!(
        resolve_bash_command_permission(&deny, &[], "bash", &find_exec),
        BashRuleDecision::Deny
    );

    let broad_find_allow = vec![PermissionRule::parse("Bash(find:*)")];
    assert!(bash_command_allowed_by_rules(&broad_find_allow, "bash", &find_exec).is_none());
}

// ---- heredoc nested commands in deny ----

#[test]
fn deny_rule_detects_command_substitution_in_unquoted_heredoc() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    assert_eq!(
        resolve_bash_command_permission(
            &deny,
            &[],
            "bash",
            &bash_input("cat <<EOF\n$(rm -rf /critical)\nEOF")
        ),
        BashRuleDecision::Deny
    );
}

#[test]
fn deny_rule_does_not_scan_quoted_heredoc_body() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    assert_eq!(
        resolve_bash_command_permission(
            &deny,
            &[],
            "bash",
            &bash_input("cat <<'EOF'\n$(rm -rf /)\nEOF")
        ),
        BashRuleDecision::Ask
    );
}

#[test]
fn deny_rule_scans_shell_stdin_bodies() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    for command in [
        "bash <<'EOF'\nrm -rf /tmp/cache\nEOF\n",
        "bash <<< 'rm -rf /tmp/cache'",
        "bash /dev/fd/3 3<<< 'rm -rf /tmp/cache'",
    ] {
        assert_eq!(
            resolve_bash_command_permission(&deny, &[], "bash", &bash_input(command)),
            BashRuleDecision::Deny,
            "expected deny to scan shell stdin body in {command:?}"
        );
    }
}

#[test]
fn deny_rule_scans_crontab_stdin_bodies() {
    let deny = vec![PermissionRule::parse("Bash(rm:*)")];
    assert_eq!(
        resolve_bash_command_permission(
            &deny,
            &[],
            "bash",
            &bash_input("crontab <<EOF\n* * * * * rm -rf /tmp/cache\nEOF\n")
        ),
        BashRuleDecision::Deny
    );
}

// ---- path glob behavior ----

#[test]
fn path_globs_distinguish_recursive_and_direct_children() {
    let recursive = PermissionRule::parse("Read(src/**)");
    assert!(recursive.matches_tool_call("Read", &file_path_input("src")));
    assert!(recursive.matches_tool_call("Read", &file_path_input("src/lib.rs")));
    assert!(recursive.matches_tool_call("Read", &file_path_input("src/bin/main.rs")));

    let direct_child = PermissionRule::parse("Read(src/*)");
    assert!(!direct_child.matches_tool_call("Read", &file_path_input("src")));
    assert!(direct_child.matches_tool_call("Read", &file_path_input("src/lib.rs")));
    assert!(!direct_child.matches_tool_call("Read", &file_path_input("src/bin/main.rs")));
}

#[test]
fn relative_path_rules_reject_absolute_and_parent_targets() {
    let relative = PermissionRule::parse("Read(./src/**)");
    assert!(relative.matches_tool_call("Read", &file_path_input("src/lib.rs")));
    assert!(relative.matches_tool_call("Read", &file_path_input("./src/lib.rs")));
    assert!(!relative.matches_tool_call("Read", &file_path_input("/src/lib.rs")));
    assert!(!relative.matches_tool_call("Read", &file_path_input("C:/src/lib.rs")));
    assert!(!relative.matches_tool_call("Read", &file_path_input(r"C:\src\lib.rs")));
    assert!(!relative.matches_tool_call("Read", &file_path_input("../src/lib.rs")));
}

#[test]
fn relative_path_rules_reject_windows_drive_root_traversal_escape() {
    // A `..` at a Windows drive root must not pop the drive letter and collapse
    // to a relative path: `C:\..\Windows\System32` normalizes to `C:/Windows/
    // System32` (still absolute), so `Read(./Windows/**)` — a cwd-relative rule —
    // must NOT match the absolute system directory.
    let relative = PermissionRule::parse("Read(./Windows/**)");
    assert!(!relative.matches_tool_call("Read", &file_path_input(r"C:\..\Windows\System32")));
    assert!(!relative.matches_tool_call("Read", &file_path_input("C:/../Windows/System32")));
    assert!(!relative.matches_tool_call("Read", &file_path_input("C:/../../Windows/System32")));
    // The plain cwd-relative target still matches.
    assert!(relative.matches_tool_call("Read", &file_path_input("Windows/System32")));
}

#[test]
fn relative_path_rules_reject_windows_drive_relative_targets() {
    // A Windows *drive-relative* path (`C:secret.txt`, no separator after the
    // colon) resolves against the current directory of drive C — a location not
    // guaranteed to be inside the workspace — so a cwd-relative rule must NOT
    // match it. The colon prefix (any `[A-Za-z]:`) marks it drive-qualified,
    // exactly like an absolute drive path.
    let any = PermissionRule::parse("Read(./**)");
    assert!(!any.matches_tool_call("Read", &file_path_input("C:secret.txt")));
    assert!(!any.matches_tool_call("Read", &file_path_input(r"C:secret.txt")));
    assert!(!any.matches_tool_call("Read", &file_path_input("d:passwd")));
    // A drive-relative parent escape likewise stays drive-qualified.
    assert!(!any.matches_tool_call("Read", &file_path_input(r"C:..\Windows\System32")));
    // A subdir rule must not match a drive-relative target that would otherwise
    // collapse to a bare relative path.
    let windows = PermissionRule::parse("Read(./Windows/**)");
    assert!(!windows.matches_tool_call("Read", &file_path_input(r"C:..\Windows\System32")));
    // A genuine cwd-relative file is still allowed by the broad rule.
    assert!(any.matches_tool_call("Read", &file_path_input("notes.txt")));
}

#[test]
fn glob_and_grep_rules_match_parser_owned_path_targets() {
    let glob_rule = PermissionRule::parse("Glob(src/**)");
    assert!(glob_rule.matches_tool_call(
        "Glob",
        &serde_json::json!({ "pattern": "src/**/*.rs" }).to_string()
    ));
    assert!(glob_rule.matches_tool_call(
        "Glob",
        &serde_json::json!({ "path": "src/bin", "pattern": "*.rs" }).to_string()
    ));
    assert!(!glob_rule.matches_tool_call(
        "Glob",
        &serde_json::json!({ "path": "../src", "pattern": "*.rs" }).to_string()
    ));

    let grep_rule = PermissionRule::parse("Grep(src/**)");
    assert!(grep_rule.matches_tool_call(
        "Grep",
        &serde_json::json!({ "path": "src", "glob": "**/*.rs" }).to_string()
    ));
    assert!(!grep_rule.matches_tool_call(
        "Grep",
        &serde_json::json!({ "path": "tests", "glob": "**/*.rs" }).to_string()
    ));
}

#[test]
fn path_rules_reject_dot_dot_traversal_escape() {
    // A base glob allow rule must not authorize a `..` traversal that escapes
    // the intended scope, even though the pattern is not `./`-relative.
    let allow = PermissionRule::parse("Edit(src/**)");
    assert!(allow.matches_tool_call("Edit", &file_path_input("src/lib.rs")));
    assert!(!allow.matches_tool_call("Edit", &file_path_input("src/../../../etc/passwd")));
    assert!(!allow.matches_tool_call("Edit", &file_path_input("src/../secret")));

    // Interior `..` that stays in-scope resolves and still matches a deny rule.
    let deny = PermissionRule::parse("Edit(secret/**)");
    assert!(deny.matches_tool_call("Edit", &file_path_input("secret/../secret/file")));
}

#[test]
fn grep_glob_filter_cannot_authorize_out_of_scope_path() {
    // The `glob` field is a filename filter, not a directory: a rule scoped to
    // `src` must not authorize grepping `/etc` just because the filter matches.
    let rule = PermissionRule::parse("Grep(src)");
    assert!(!rule.matches_tool_call(
        "Grep",
        &serde_json::json!({ "path": "/etc", "glob": "src" }).to_string()
    ));
}

#[test]
fn glob_pattern_cannot_authorize_out_of_scope_root() {
    // A search root outside the rule scope must be rejected even when the
    // relative `pattern` coincidentally matches the rule.
    let rule = PermissionRule::parse("Glob(src/**)");
    assert!(!rule.matches_tool_call(
        "Glob",
        &serde_json::json!({ "path": "/etc", "pattern": "src/**/*.rs" }).to_string()
    ));
}
