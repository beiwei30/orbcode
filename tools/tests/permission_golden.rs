//! Golden tests for permission-denied and network-denied tool error shapes.

mod common;

use orbcode_tools::{ToolError, ToolRegistry};

fn registry() -> ToolRegistry {
    ToolRegistry::foundation()
}

#[tokio::test]
async fn tools_permission_denied_blocks_bash() {
    let reg = registry();
    let mut ctx = common::test_context("perm-bash").await;
    ctx.allow_tools = false;

    let err = reg
        .invoke("bash", r#"{"command":"echo hi","timeout":5000}"#, &ctx)
        .await
        .expect_err("bash should be denied");

    assert!(
        matches!(err, ToolError::PermissionDenied),
        "expected PermissionDenied, got: {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "tool permissions are disabled",
        "PermissionDenied display text mismatch"
    );
}

#[tokio::test]
async fn tools_permission_denied_blocks_file_read() {
    let reg = registry();
    let mut ctx = common::test_context("perm-fread").await;
    ctx.allow_tools = false;

    let path = ctx.cwd.join("test.txt");
    std::fs::write(&path, "content").expect("write");

    let err = reg
        .invoke(
            "file-read",
            &format!(r#"{{"file_path":"{}"}}"#, path.display()),
            &ctx,
        )
        .await
        .expect_err("file-read should be denied");

    assert!(matches!(err, ToolError::PermissionDenied));
}

#[tokio::test]
async fn tools_permission_denied_blocks_grep() {
    let reg = registry();
    let mut ctx = common::test_context("perm-grep").await;
    ctx.allow_tools = false;

    let err = reg
        .invoke(
            "grep",
            &format!(r#"{{"pattern":"test","path":"{}"}}"#, ctx.cwd.display()),
            &ctx,
        )
        .await
        .expect_err("grep should be denied");

    assert!(matches!(err, ToolError::PermissionDenied));
}

#[tokio::test]
async fn tools_permission_denied_blocks_glob() {
    let reg = registry();
    let mut ctx = common::test_context("perm-glob").await;
    ctx.allow_tools = false;

    let err = reg
        .invoke(
            "glob",
            &format!(r#"{{"pattern":"*.rs","path":"{}"}}"#, ctx.cwd.display()),
            &ctx,
        )
        .await
        .expect_err("glob should be denied");

    assert!(matches!(err, ToolError::PermissionDenied));
}

#[tokio::test]
async fn network_denied_blocks_web_fetch() {
    let reg = registry();
    let mut ctx = common::test_context("perm-net-fetch").await;
    ctx.allow_network = false;

    let err = reg
        .invoke(
            "web-fetch",
            r#"{"url":"https://example.com","prompt":"extract"}"#,
            &ctx,
        )
        .await
        .expect_err("web-fetch should be denied");

    assert!(
        matches!(err, ToolError::NetworkDenied),
        "expected NetworkDenied, got: {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "network permissions are disabled",
        "NetworkDenied display text mismatch"
    );
}

#[tokio::test]
async fn network_denied_blocks_web_search() {
    let reg = registry();
    let mut ctx = common::test_context("perm-net-search").await;
    ctx.allow_network = false;

    let err = reg
        .invoke("web-search", r#"{"query":"rust programming"}"#, &ctx)
        .await
        .expect_err("web-search should be denied");

    assert!(matches!(err, ToolError::NetworkDenied));
}
