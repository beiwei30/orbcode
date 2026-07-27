use super::*;
#[cfg(all(test, target_os = "macos"))]
use crate::bash::macos_seatbelt_profile;
use crate::bash::{
    bash_failure_likely_sandbox_denial, linux_bubblewrap_args, shell_program,
    windows_sandbox_runner_args,
};
use crate::output::truncate_tool_output_with_metadata;
use std::sync::atomic::{AtomicBool, Ordering};

#[tokio::test]
async fn bash_accepts_cmd_alias() {
    let registry = ToolRegistry::foundation();
    let context = test_context("bash-cmd-alias").await;

    let result = registry
        .invoke("bash", r#"{"cmd":"printf alias-ok"}"#, &context)
        .await
        .expect("bash should accept cmd alias");

    assert_eq!(result.output, "alias-ok");
}

#[tokio::test]
async fn bash_preserves_leading_stdout_whitespace() {
    let registry = ToolRegistry::foundation();
    let context = test_context("bash-leading-stdout-whitespace").await;

    let result = registry
        .invoke(
            "bash",
            r#"{"command":"printf '      48 orbcode/core/src/advanced.rs\\n      74 orbcode/core/src/lib.rs\\n'"}"#,
            &context,
        )
        .await
        .expect("bash should preserve aligned stdout");

    assert_eq!(
        result.output,
        "      48 orbcode/core/src/advanced.rs\n      74 orbcode/core/src/lib.rs"
    );
}

#[tokio::test]
async fn bash_records_runtime_metadata_on_success() {
    let registry = ToolRegistry::foundation();
    let context = test_context("bash-runtime-metadata-success").await;

    let result = registry
        .invoke(
            "bash",
            r#"{"command":"printf ok","timeout":5000}"#,
            &context,
        )
        .await
        .expect("bash should succeed");

    let metadata = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("bash"))
        .expect("bash metadata should be present");
    assert_eq!(metadata["command"], "printf ok");
    assert_eq!(metadata["cwd"], context.cwd.display().to_string());
    assert_eq!(metadata["timeoutMs"], 5000);
    assert_eq!(metadata["exitCode"], 0);
    assert_eq!(metadata["interrupted"], false);
    assert_eq!(metadata["timedOut"], false);
    assert_eq!(metadata["outputTruncated"], false);
    assert!(metadata["durationMs"].as_u64().is_some());
}

#[tokio::test]
async fn bash_persists_durable_task_record_through_registry() {
    let registry = ToolRegistry::foundation();
    let context = test_context("bash-durable-success").await;

    let result = registry
        .invoke("bash", r#"{"command":"printf durable-ok"}"#, &context)
        .await
        .expect("bash should succeed");

    let bash = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("bash"))
        .expect("bash metadata should be present");
    let task_id = bash["taskId"].as_str().expect("taskId present").to_string();
    assert_eq!(bash["taskStatus"], "succeeded");
    assert!(bash["outputBytes"].as_u64().expect("outputBytes") >= "durable-ok".len() as u64);
    let log_path = bash["logPath"].as_str().expect("logPath present");
    assert!(PathBuf::from(log_path).exists(), "durable log file exists");
    let record_path = bash["recordPath"].as_str().expect("recordPath present");
    assert!(
        PathBuf::from(record_path).exists(),
        "durable record file exists"
    );

    let resumed = LocalShellTaskRegistry::new(&context.home_dir);
    let loaded = resumed.load(&task_id).await.expect("reload durable record");
    assert_eq!(loaded.status, LocalShellTaskStatus::Succeeded);
    assert_eq!(loaded.command, "printf durable-ok");
    assert_eq!(loaded.session_id, "bash-durable-success");
    assert_eq!(
        loaded
            .current_attempt
            .as_ref()
            .and_then(|attempt| attempt.exit_code),
        Some(0)
    );

    let listed = resumed
        .list_for_session("bash-durable-success")
        .await
        .expect("list for session");
    assert!(listed.iter().any(|record| record.task_id == task_id));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn bash_cancellation_persists_interrupted_record() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("bash-durable-cancel").await;
    let cancel_flag = Arc::new(AtomicBool::new(false));
    context.cancellation = ToolCancellationToken::from_flag(cancel_flag.clone());

    let cancel_flag_for_task = cancel_flag.clone();
    tokio::spawn(async move {
        sleep(Duration::from_millis(50)).await;
        cancel_flag_for_task.store(true, Ordering::SeqCst);
    });

    let result = timeout(
        Duration::from_secs(2),
        registry.invoke("bash", r#"{"command":"sleep 10"}"#, &context),
    )
    .await
    .expect("bash cancellation should not wait for the command timeout");

    let ToolError::InterruptedWithMetadata { metadata } =
        result.expect_err("cancelled bash should return interrupted metadata")
    else {
        panic!("expected interrupted-with-metadata error");
    };
    let bash = metadata
        .get("bash")
        .expect("bash metadata should be present");
    let task_id = bash["taskId"].as_str().expect("taskId present").to_string();
    assert_eq!(bash["taskStatus"], "interrupted");
    assert_eq!(bash["killSignal"], "SIGTERM");
    assert_eq!(bash["killReason"], "interrupted by user");

    let resumed = LocalShellTaskRegistry::new(&context.home_dir);
    let loaded = resumed.load(&task_id).await.expect("reload durable record");
    assert_eq!(loaded.status, LocalShellTaskStatus::Interrupted);
    assert_eq!(loaded.kill_signal_sent.as_deref(), Some("SIGTERM"));
    let attempt = loaded.current_attempt.as_ref().expect("attempt recorded");
    assert_eq!(attempt.kill_reason.as_deref(), Some("interrupted by user"));
    assert_eq!(
        attempt.terminal_status,
        Some(LocalShellTaskStatus::Interrupted)
    );
}

#[tokio::test]
async fn bash_timeout_returns_failed_metadata() {
    let registry = ToolRegistry::foundation();
    let context = test_context("bash-runtime-metadata-timeout").await;

    let error = registry
        .invoke("bash", r#"{"command":"sleep 5","timeout":50}"#, &context)
        .await
        .expect_err("bash should time out");

    let ToolError::ExecutionFailedWithMetadata { metadata, .. } = error else {
        panic!("expected metadata-bearing timeout error");
    };
    let bash = metadata
        .get("bash")
        .expect("bash metadata should be present");
    assert_eq!(bash["timeoutMs"], 50);
    assert_eq!(bash["timedOut"], true);
    assert_eq!(bash["interrupted"], false);
    assert!(bash["durationMs"].as_u64().unwrap_or_default() >= 50);
}

#[tokio::test]
async fn bash_omits_workspace_impact_outside_git_repo() {
    let registry = ToolRegistry::foundation();
    let context = test_context("bash-workspace-impact-no-git").await;

    let result = registry
        .invoke("bash", r#"{"command":"printf ok"}"#, &context)
        .await
        .expect("bash should succeed");
    let bash = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("bash"))
        .expect("bash metadata present");

    assert!(bash.get("workspaceImpact").is_none());
}

#[tokio::test]
async fn bash_records_git_working_tree_change() {
    let registry = ToolRegistry::foundation();
    let context = test_context("bash-workspace-impact-dirty").await;
    init_test_git_repo(&context.cwd);

    let result = registry
        .invoke("bash", r#"{"command":"touch new_file.txt"}"#, &context)
        .await
        .expect("bash should succeed");
    let bash = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("bash"))
        .expect("bash metadata present");
    let git = bash
        .get("workspaceImpact")
        .and_then(|impact| impact.get("git"))
        .expect("git impact recorded");

    assert_eq!(git["workingTreeChanged"], true);
    assert_eq!(git["preDirtyFiles"], 0);
    assert_eq!(git["postDirtyFiles"], 1);
    assert_eq!(git["dirtyDelta"], 1);
    assert_eq!(git["branchChanged"], false);
    assert_eq!(git["headChanged"], false);
}

#[tokio::test]
async fn bash_records_git_branch_change() {
    let registry = ToolRegistry::foundation();
    let context = test_context("bash-workspace-impact-branch").await;
    init_test_git_repo(&context.cwd);

    let result = registry
        .invoke(
            "bash",
            r#"{"command":"git checkout -b feature-x 2>/dev/null"}"#,
            &context,
        )
        .await
        .expect("bash should succeed");
    let bash = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("bash"))
        .expect("bash metadata present");
    let git = bash
        .get("workspaceImpact")
        .and_then(|impact| impact.get("git"))
        .expect("git impact recorded");

    assert_eq!(git["branchChanged"], true);
    assert_eq!(git["postBranch"], "feature-x");
}

#[test]
fn bash_truncation_metadata_tracks_omitted_chars() {
    let output = truncate_tool_output_with_metadata("a".repeat(40), 20, "Bash output truncated.");

    assert!(output.truncated);
    assert_eq!(output.original_chars, 40);
    assert!(output.omitted_chars > 0);
    assert!(output.output.contains("Omitted"));
}

#[test]
fn parses_sandbox_modes() {
    assert_eq!(
        SandboxMode::parse("danger-full-access"),
        Some(SandboxMode::DangerFullAccess)
    );
    assert_eq!(
        SandboxMode::parse("workspace_write"),
        Some(SandboxMode::WorkspaceWrite)
    );
    assert_eq!(SandboxMode::parse("readonly"), Some(SandboxMode::ReadOnly));
    assert_eq!(SandboxMode::parse("unknown"), None);
}

#[tokio::test]
async fn linux_bubblewrap_args_model_read_only_network_blocked() {
    let root = test_root("linux-bwrap-read-only");
    std::fs::create_dir_all(&root).expect("create temp root");

    let args = linux_bubblewrap_args("printf ok", SandboxMode::ReadOnly, &root, &[], false)
        .await
        .expect("build bwrap args");

    assert!(os_args_contain(&args, &["--ro-bind", "/", "/"]));
    assert!(os_args_contain(&args, &["--unshare-all"]));
    assert!(!os_args_contain(&args, &["--share-net"]));
    assert!(os_args_contain(
        &args,
        &["--setenv", "ORBCODE_SANDBOX_NETWORK_DISABLED", "1"]
    ));
    assert!(!os_args_contain(&args, &["--bind"]));
    let shell = shell_program();
    assert!(os_args_contain(&args, &["--", &shell, "-lc", "printf ok"]));
}

#[tokio::test]
async fn linux_bubblewrap_args_model_workspace_write_network_allowed() {
    let root = test_root("linux-bwrap-workspace-write");
    let extra = test_root("linux-bwrap-workspace-write-extra");
    std::fs::create_dir_all(&root).expect("create temp root");
    std::fs::create_dir_all(&extra).expect("create extra root");
    let canonical = std::fs::canonicalize(&root).expect("canonical root");
    let canonical = canonical.to_string_lossy().into_owned();
    let canonical_extra = std::fs::canonicalize(&extra).expect("canonical extra");
    let canonical_extra = canonical_extra.to_string_lossy().into_owned();

    let args = linux_bubblewrap_args(
        "printf ok",
        SandboxMode::WorkspaceWrite,
        &root,
        &[extra],
        true,
    )
    .await
    .expect("build bwrap args");

    assert!(os_args_contain(&args, &["--share-net"]));
    assert!(os_args_contain(
        &args,
        &["--bind", canonical.as_str(), canonical.as_str()]
    ));
    assert!(os_args_contain(
        &args,
        &["--bind", canonical_extra.as_str(), canonical_extra.as_str()]
    ));
    assert!(os_args_contain(&args, &["--chdir", canonical.as_str()]));
    assert!(!os_args_contain(
        &args,
        &["--setenv", "ORBCODE_SANDBOX_NETWORK_DISABLED", "1"]
    ));
}

#[tokio::test]
async fn windows_sandbox_runner_args_model_read_only_network_blocked() {
    let root = test_root("windows-runner-read-only");
    std::fs::create_dir_all(&root).expect("create temp root");
    let canonical = std::fs::canonicalize(&root).expect("canonical root");
    let canonical = canonical.to_string_lossy().into_owned();

    let args =
        windows_sandbox_runner_args("Write-Output ok", SandboxMode::ReadOnly, &root, &[], false)
            .await
            .expect("build Windows sandbox runner args");

    assert!(os_args_contain(&args, &["--mode", "read-only", "--cwd"]));
    assert!(args.iter().any(|arg| arg.to_string_lossy() == canonical));
    assert!(os_args_contain(&args, &["--network", "deny", "--"]));
    let shell = shell_program();
    #[cfg(not(target_os = "windows"))]
    assert!(os_args_contain(
        &args,
        &["--", &shell, "-lc", "Write-Output ok"]
    ));
    #[cfg(target_os = "windows")]
    assert!(os_args_contain(
        &args,
        &[
            "--",
            &shell,
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Write-Output ok"
        ]
    ));
}

#[tokio::test]
async fn windows_sandbox_runner_args_model_workspace_write_network_allowed() {
    let root = test_root("windows-runner-workspace-write");
    let extra = test_root("windows-runner-workspace-write-extra");
    std::fs::create_dir_all(&root).expect("create temp root");
    std::fs::create_dir_all(&extra).expect("create extra root");
    let canonical_extra = std::fs::canonicalize(&extra)
        .expect("canonical extra")
        .to_string_lossy()
        .into_owned();

    let args = windows_sandbox_runner_args(
        "Write-Output ok",
        SandboxMode::WorkspaceWrite,
        &root,
        &[extra],
        true,
    )
    .await
    .expect("build Windows sandbox runner args");

    assert!(os_args_contain(
        &args,
        &["--mode", "workspace-write", "--cwd"]
    ));
    assert!(os_args_contain(
        &args,
        &["--add-dir", canonical_extra.as_str()]
    ));
    assert!(os_args_contain(&args, &["--network", "allow", "--"]));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_seatbelt_profile_model_read_only_network_blocked() {
    let root = test_root("macos-seatbelt-read-only");
    std::fs::create_dir_all(&root).expect("create temp root");

    let (profile, params) = macos_seatbelt_profile(SandboxMode::ReadOnly, &root, &[], false)
        .await
        .expect("build profile");

    assert!(profile.contains("(deny default)"));
    assert!(profile.contains("(allow file-read*)"));
    assert!(profile.contains("(allow file-write-data"));
    assert!(profile.contains(r#"(literal "/dev/null")"#));
    assert!(!profile.contains("(allow network*)"));
    assert!(!profile.contains("ORBCODE_WORKSPACE_WRITE_ROOT"));
    assert!(params.is_empty());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_seatbelt_profile_model_workspace_write_network_allowed() {
    let root = test_root("macos-seatbelt-workspace-write");
    let extra = test_root("macos-seatbelt-workspace-write-extra");
    std::fs::create_dir_all(&root).expect("create temp root");
    std::fs::create_dir_all(&extra).expect("create extra root");
    let canonical = std::fs::canonicalize(&root).expect("canonical root");
    let canonical_extra = std::fs::canonicalize(&extra).expect("canonical extra");

    let (profile, params) =
        macos_seatbelt_profile(SandboxMode::WorkspaceWrite, &root, &[extra], true)
            .await
            .expect("build profile");

    assert!(profile.contains("(allow network*)"));
    assert!(
        profile.contains(r#"(allow file-write* (subpath (param "ORBCODE_WORKSPACE_WRITE_ROOT")))"#)
    );
    assert!(
        profile
            .contains(r#"(allow file-write* (subpath (param "ORBCODE_WORKSPACE_WRITE_DIR_0")))"#)
    );
    assert_eq!(
        params,
        vec![
            ("ORBCODE_WORKSPACE_WRITE_ROOT".to_string(), canonical),
            ("ORBCODE_WORKSPACE_WRITE_DIR_0".to_string(), canonical_extra),
        ]
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn read_only_sandbox_allows_read_commands() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("bash-read-only-sandbox-read").await;
    context.sandbox_mode = SandboxMode::ReadOnly;

    let result = registry
        .invoke("bash", r#"{"command":"printf sandboxed"}"#, &context)
        .await
        .expect("read-only sandbox should run read-only commands");

    assert_eq!(result.output, "sandboxed");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn read_only_sandbox_blocks_file_writes() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("bash-read-only-sandbox-write").await;
    context.sandbox_mode = SandboxMode::ReadOnly;
    let target = context.cwd.join("blocked.txt");

    let error = registry
        .invoke(
            "bash",
            &format!(r#"{{"command":"printf blocked > '{}'"}}"#, target.display()),
            &context,
        )
        .await
        .expect_err("read-only sandbox should block file writes");

    assert!(matches!(
        error,
        ToolError::ExecutionFailed(_) | ToolError::ExecutionFailedWithMetadata { .. }
    ));
    assert!(
        error
            .to_string()
            .contains("Retry affordance: call Bash again with the same command")
    );
    assert!(!target.exists());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn read_only_sandbox_allows_network_when_enabled() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("bash-read-only-sandbox-network-allow").await;
    context.sandbox_mode = SandboxMode::ReadOnly;
    context.sandbox_allow_network = true;
    let (port, accepted) = loopback_acceptor();

    registry
        .invoke(
            "bash",
            &format!(r#"{{"command":"nc -z 127.0.0.1 {port}"}}"#),
            &context,
        )
        .await
        .expect("read-only sandbox should allow network when enabled");

    assert!(
        accepted
            .recv_timeout(Duration::from_secs(3))
            .expect("receive accept result")
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn read_only_sandbox_blocks_network_when_disabled() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("bash-read-only-sandbox-network-deny").await;
    context.sandbox_mode = SandboxMode::ReadOnly;
    context.sandbox_allow_network = false;
    let (port, accepted) = loopback_acceptor();

    let error = registry
        .invoke(
            "bash",
            &format!(r#"{{"command":"nc -z 127.0.0.1 {port}"}}"#),
            &context,
        )
        .await
        .expect_err("read-only sandbox should block network when disabled");

    assert!(matches!(
        error,
        ToolError::ExecutionFailed(_) | ToolError::ExecutionFailedWithMetadata { .. }
    ));
    assert!(
        error
            .to_string()
            .contains("Retry affordance: call Bash again with the same command")
    );
    assert!(
        !accepted
            .recv_timeout(Duration::from_secs(4))
            .expect("receive accept result")
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn workspace_write_sandbox_allows_cwd_and_blocks_outside() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("bash-workspace-write-sandbox").await;
    context.sandbox_mode = SandboxMode::WorkspaceWrite;
    let inside = context.cwd.join("allowed.txt");
    let outside = context.home_dir.join("blocked.txt");

    let error = registry
        .invoke(
            "bash",
            &format!(
                r#"{{"command":"printf allowed > '{}'; printf blocked > '{}'"}}"#,
                inside.display(),
                outside.display()
            ),
            &context,
        )
        .await
        .expect_err("workspace-write sandbox should block writes outside cwd");

    assert!(matches!(
        error,
        ToolError::ExecutionFailed(_) | ToolError::ExecutionFailedWithMetadata { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(&inside).expect("read inside"),
        "allowed"
    );
    assert!(!outside.exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires Linux with bubblewrap; set ORBCODE_RUN_LINUX_SANDBOX_HOST_TESTS=1 to fail on missing prerequisites"]
async fn linux_bubblewrap_host_read_only_blocks_file_writes() {
    let Some(mut context) =
        linux_bubblewrap_host_context("linux-host-read-only-write", false).await
    else {
        return;
    };
    context.sandbox_mode = SandboxMode::ReadOnly;
    let target = context.cwd.join("blocked.txt");

    let error = ToolRegistry::foundation()
        .invoke(
            "bash",
            &json!({ "command": format!("printf blocked > '{}'", target.display()) }).to_string(),
            &context,
        )
        .await
        .expect_err("read-only bubblewrap sandbox should block file writes");

    assert!(matches!(
        error,
        ToolError::ExecutionFailed(_) | ToolError::ExecutionFailedWithMetadata { .. }
    ));
    assert!(
        error
            .to_string()
            .contains("Retry affordance: call Bash again with the same command")
    );
    assert!(!target.exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires Linux with bubblewrap; set ORBCODE_RUN_LINUX_SANDBOX_HOST_TESTS=1 to fail on missing prerequisites"]
async fn linux_bubblewrap_host_workspace_write_scope_is_enforced() {
    let Some(mut context) =
        linux_bubblewrap_host_context("linux-host-workspace-write", false).await
    else {
        return;
    };
    context.sandbox_mode = SandboxMode::WorkspaceWrite;
    let extra = test_root("linux-host-workspace-write-extra");
    std::fs::create_dir_all(&extra).expect("create extra writable directory");
    context.additional_directories.push(extra.clone());

    let inside = context.cwd.join("allowed.txt");
    let extra_file = extra.join("allowed-extra.txt");
    let outside_temp = context.home_dir.join("outside-temp");
    std::fs::create_dir_all(&outside_temp).expect("create outside temp dir");
    let outside_temp_file = outside_temp.join("blocked.txt");
    let outside_target = context.home_dir.join("blocked-symlink-target.txt");
    let symlink = context.cwd.join("blocked-symlink.txt");
    std::os::unix::fs::symlink(&outside_target, &symlink).expect("create symlink");

    let registry = ToolRegistry::foundation();
    registry
        .invoke(
            "bash",
            &json!({
                "command": format!(
                    "printf cwd > '{}' && printf extra > '{}'",
                    inside.display(),
                    extra_file.display()
                )
            })
            .to_string(),
            &context,
        )
        .await
        .expect("workspace-write bubblewrap sandbox should allow cwd and additional dirs");

    assert_eq!(
        std::fs::read_to_string(&inside).expect("read cwd output"),
        "cwd"
    );
    assert_eq!(
        std::fs::read_to_string(&extra_file).expect("read extra output"),
        "extra"
    );

    registry
        .invoke(
            "bash",
            &json!({ "command": format!("printf blocked > '{}'", outside_temp_file.display()) })
                .to_string(),
            &context,
        )
        .await
        .expect_err("workspace-write bubblewrap sandbox should block tempdir outside workspace");
    assert!(!outside_temp_file.exists());

    registry
        .invoke(
            "bash",
            &json!({ "command": format!("printf blocked > '{}'", symlink.display()) }).to_string(),
            &context,
        )
        .await
        .expect_err("workspace-write bubblewrap sandbox should block symlink escape writes");
    assert!(!outside_target.exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires Linux with bubblewrap and python3; set ORBCODE_RUN_LINUX_SANDBOX_HOST_TESTS=1 to fail on missing prerequisites"]
async fn linux_bubblewrap_host_network_policy_is_enforced() {
    let Some(mut context) = linux_bubblewrap_host_context("linux-host-network-policy", true).await
    else {
        return;
    };
    let registry = ToolRegistry::foundation();

    context.sandbox_mode = SandboxMode::ReadOnly;
    context.sandbox_allow_network = true;
    let (allowed_port, allowed_accepted) = loopback_acceptor();
    registry
        .invoke(
            "bash",
            &json!({
                "command": format!(
                    "python3 -c \"import socket; s=socket.create_connection(('127.0.0.1', {allowed_port}), 1); s.close()\""
                )
            })
            .to_string(),
            &context,
        )
        .await
        .expect("bubblewrap sandbox should allow network when enabled");
    assert!(
        allowed_accepted
            .recv_timeout(Duration::from_secs(3))
            .expect("receive allowed accept result")
    );

    context.sandbox_allow_network = false;
    let (blocked_port, blocked_accepted) = loopback_acceptor();
    registry
        .invoke(
            "bash",
            &json!({
                "command": format!(
                    "python3 -c \"import socket; s=socket.create_connection(('127.0.0.1', {blocked_port}), 1); s.close()\""
                )
            })
            .to_string(),
            &context,
        )
        .await
        .expect_err("bubblewrap sandbox should block network when disabled");
    assert!(
        !blocked_accepted
            .recv_timeout(Duration::from_secs(4))
            .expect("receive blocked accept result")
    );
}

#[cfg(not(target_os = "linux"))]
#[test]
#[ignore = "Linux bubblewrap host validation only runs on Linux"]
fn linux_bubblewrap_host_validation_is_linux_only() {}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn sandbox_escalation_runs_unsandboxed_after_approval() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("bash-sandbox-escalation").await;
    context.sandbox_mode = SandboxMode::ReadOnly;
    let target = context.cwd.join("escalated.txt");

    let result = registry
        .invoke(
            "bash",
            &format!(
                r#"{{"command":"printf escalated > '{}'","sandbox_permissions":"require_escalated"}}"#,
                target.display()
            ),
            &context,
        )
        .await
        .expect("escalated sandbox request should run outside read-only sandbox");

    assert!(result.summary.contains("printf escalated"));
    assert_eq!(
        std::fs::read_to_string(&target).expect("read escalated output"),
        "escalated"
    );
}

#[test]
fn detects_bash_sandbox_escalation_inputs() {
    assert!(bash_input_requests_sandbox_escalation(
        r#"{"command":"echo hi","sandbox_permissions":"require_escalated"}"#
    ));
    assert!(bash_input_requests_sandbox_escalation(
        r#"{"command":"echo hi","dangerouslyDisableSandbox":true}"#
    ));
    assert!(!bash_input_requests_sandbox_escalation(
        r#"{"command":"echo hi","sandbox_permissions":"use_default"}"#
    ));
}

#[tokio::test]
async fn bash_sandbox_denial_hint_requires_restrictive_non_escalated_sandbox() {
    let mut context = test_context("bash-sandbox-hint").await;
    context.sandbox_mode = SandboxMode::ReadOnly;

    assert!(bash_failure_likely_sandbox_denial(
        "Operation not permitted",
        &context,
        false
    ));
    assert!(!bash_failure_likely_sandbox_denial(
        "Operation not permitted",
        &context,
        true
    ));
    context.sandbox_mode = SandboxMode::DangerFullAccess;
    assert!(!bash_failure_likely_sandbox_denial(
        "Operation not permitted",
        &context,
        false
    ));
}

#[tokio::test]
async fn bash_emits_live_progress_updates() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("bash-progress").await;
    let reporter = Arc::new(RecordingProgressReporter::default());
    context.progress = Some(reporter.clone());

    let result = registry
        .invoke(
            "bash",
            r#"{"command":"printf alpha && sleep 0.05 && printf beta >&2"}"#,
            &context,
        )
        .await
        .expect("bash should succeed");

    assert_eq!(result.output, "alpha");

    let records = reporter.records.lock().await.clone();
    assert!(records.iter().any(|progress| {
        progress
            .get("data")
            .and_then(|data| data.get("status"))
            .and_then(Value::as_str)
            == Some("Running bash command")
    }));
    assert!(records.iter().any(|progress| {
        progress
            .get("data")
            .and_then(|data| data.get("status"))
            .and_then(Value::as_str)
            == Some("Streaming stdout")
    }));
    assert!(records.iter().any(|progress| {
        progress
            .get("data")
            .and_then(|data| data.get("status"))
            .and_then(Value::as_str)
            == Some("Streaming stderr")
    }));
    assert!(records.iter().any(|progress| {
        progress
            .get("data")
            .and_then(|data| data.get("status"))
            .and_then(Value::as_str)
            == Some("Bash command completed")
    }));
}

#[tokio::test]
async fn bash_cancellation_interrupts_long_running_command() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("bash-cancel").await;
    let cancel_flag = Arc::new(AtomicBool::new(false));
    context.cancellation = ToolCancellationToken::from_flag(cancel_flag.clone());
    let reporter = Arc::new(RecordingProgressReporter::default());
    context.progress = Some(reporter.clone());

    let cancel_flag_for_task = cancel_flag.clone();
    tokio::spawn(async move {
        sleep(Duration::from_millis(50)).await;
        cancel_flag_for_task.store(true, Ordering::SeqCst);
    });

    let result = timeout(
        Duration::from_secs(2),
        registry.invoke("Bash", r#"{"command":"sleep 10"}"#, &context),
    )
    .await
    .expect("bash cancellation should not wait for the command timeout");

    assert!(matches!(
        result,
        Err(ToolError::Interrupted | ToolError::InterruptedWithMetadata { .. })
    ));
    let records = reporter.records.lock().await.clone();
    assert!(records.iter().any(|progress| {
        progress
            .get("data")
            .and_then(|data| data.get("status"))
            .and_then(Value::as_str)
            == Some("Bash command interrupted")
    }));
}

// ---------------------------------------------------------------------------
// Cross-platform sandbox / tool validation harness
// ---------------------------------------------------------------------------

#[test]
fn shell_program_resolves_to_non_empty_executable_name() {
    let shell = shell_program();
    assert!(
        !shell.is_empty(),
        "shell_program must return a non-empty name"
    );
    #[cfg(target_os = "windows")]
    assert!(
        shell.to_ascii_lowercase().contains("powershell")
            || shell.to_ascii_lowercase().contains("pwsh"),
        "Windows shell_program should resolve to a PowerShell variant, got `{shell}`"
    );
}

#[tokio::test]
async fn linux_bubblewrap_args_use_posix_shell_invocation() {
    let root = test_root("linux-bubblewrap-shell-shape");
    std::fs::create_dir_all(&root).expect("create temp root");

    let args = linux_bubblewrap_args(
        "echo cross-platform",
        SandboxMode::ReadOnly,
        &root,
        &[],
        false,
    )
    .await
    .expect("build bubblewrap args");

    let shell = shell_program();
    assert!(
        os_args_contain(&args, &["--", &shell, "-lc", "echo cross-platform"]),
        "bubblewrap arg list should pass POSIX `-lc` to the shell"
    );
}

#[tokio::test]
async fn windows_sandbox_runner_args_use_powershell_invocation() {
    let root = test_root("windows-runner-shell-shape");
    std::fs::create_dir_all(&root).expect("create temp root");

    let args = windows_sandbox_runner_args(
        "Write-Output cross-platform",
        SandboxMode::ReadOnly,
        &root,
        &[],
        false,
    )
    .await
    .expect("build windows runner args");

    let shell = shell_program();
    #[cfg(target_os = "windows")]
    let invocation: &[&str] = &[
        "--",
        &shell,
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Write-Output cross-platform",
    ];
    #[cfg(not(target_os = "windows"))]
    let invocation: &[&str] = &["--", &shell, "-lc", "Write-Output cross-platform"];
    assert!(
        os_args_contain(&args, invocation),
        "windows runner arg list should match the host shell invocation shape"
    );
}

#[test]
fn resolve_path_returns_absolute_paths_unchanged() {
    let cwd = test_root("resolve-path-absolute");
    std::fs::create_dir_all(&cwd).expect("create cwd");

    #[cfg(unix)]
    let absolute_input = "/tmp/orbcode-resolve-path-test/file.txt";
    #[cfg(windows)]
    let absolute_input = r"C:\orbcode-resolve-path-test\file.txt";

    let resolved =
        crate::fs_text::resolve_path(&cwd, absolute_input).expect("resolve absolute path");
    assert!(
        resolved.is_absolute(),
        "absolute input should stay absolute on this platform"
    );
    assert_eq!(resolved, PathBuf::from(absolute_input));
}

#[test]
fn resolve_path_joins_relative_paths_against_cwd() {
    let cwd = test_root("resolve-path-relative");
    std::fs::create_dir_all(&cwd).expect("create cwd");

    let resolved =
        crate::fs_text::resolve_path(&cwd, "nested/file.txt").expect("resolve relative path");
    assert_eq!(resolved, cwd.join("nested").join("file.txt"));
    assert!(resolved.is_absolute());
}

#[cfg(target_os = "windows")]
#[test]
fn resolve_path_treats_unix_root_as_drive_relative_on_windows() {
    let cwd = test_root("resolve-path-drive-relative");
    std::fs::create_dir_all(&cwd).expect("create cwd");

    let resolved =
        crate::fs_text::resolve_path(&cwd, "/foo/bar.txt").expect("resolve drive-relative path");
    assert!(
        resolved.starts_with(&cwd),
        "drive-relative path should be joined onto cwd"
    );
}

#[tokio::test]
async fn shell_invocation_passes_command_verbatim() {
    let root = test_root("shell-invocation-verbatim");
    std::fs::create_dir_all(&root).expect("create temp root");
    let command = "printf 'a b c'";
    let args = windows_sandbox_runner_args(command, SandboxMode::ReadOnly, &root, &[], false)
        .await
        .expect("build runner args");
    let last = args
        .last()
        .expect("runner args have a trailing command")
        .to_string_lossy()
        .into_owned();
    assert_eq!(last, command);
}

// ---- host validation skip-or-fail semantics --------------------------------

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires Linux with bubblewrap; set ORBCODE_RUN_LINUX_SANDBOX_HOST_TESTS=1 to fail on missing prerequisites"]
async fn linux_bubblewrap_host_baseline_smoke() {
    let Some(context) = linux_bubblewrap_host_context("linux-host-baseline", false).await else {
        return;
    };
    let result = ToolRegistry::foundation()
        .invoke("bash", r#"{"command":"printf cross-platform"}"#, &context)
        .await
        .expect("read-only bubblewrap sandbox should run trivial commands");
    assert_eq!(result.output, "cross-platform");
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "requires macOS sandbox-exec; set ORBCODE_RUN_MACOS_SANDBOX_HOST_TESTS=1 to fail on missing prerequisites"]
async fn macos_seatbelt_host_baseline_smoke() {
    let Some(context) = macos_seatbelt_host_context("macos-host-baseline").await else {
        return;
    };
    let result = ToolRegistry::foundation()
        .invoke("bash", r#"{"command":"printf cross-platform"}"#, &context)
        .await
        .expect("read-only seatbelt sandbox should run trivial commands");
    assert_eq!(result.output, "cross-platform");
}

#[cfg(not(target_os = "macos"))]
#[test]
#[ignore = "macOS seatbelt host validation only runs on macOS"]
fn macos_seatbelt_host_validation_is_macos_only() {}

#[cfg(target_os = "windows")]
#[tokio::test]
#[ignore = "requires Windows with orbcode-windows-sandbox-runner; set ORBCODE_RUN_WINDOWS_SANDBOX_HOST_TESTS=1 to fail on missing prerequisites"]
async fn windows_sandbox_host_baseline_smoke() {
    let Some(context) = windows_sandbox_host_context("windows-host-baseline").await else {
        return;
    };
    let result = ToolRegistry::foundation()
        .invoke(
            "bash",
            r#"{"command":"Write-Output cross-platform"}"#,
            &context,
        )
        .await
        .expect("read-only windows sandbox runner should run trivial commands");
    assert_eq!(result.output.trim(), "cross-platform");
}

#[cfg(not(target_os = "windows"))]
#[test]
#[ignore = "Windows sandbox host validation only runs on Windows"]
fn windows_sandbox_host_validation_is_windows_only() {}

#[test]
fn sandbox_host_validation_forced_reads_truthy_env_values() {
    fn parse(value: &str) -> bool {
        matches!(value, "1" | "true" | "TRUE" | "yes" | "YES")
    }
    assert!(parse("1"));
    assert!(parse("true"));
    assert!(parse("TRUE"));
    assert!(parse("yes"));
    assert!(parse("YES"));
    assert!(!parse("0"));
    assert!(!parse(""));
    assert!(!parse("on"));
}
