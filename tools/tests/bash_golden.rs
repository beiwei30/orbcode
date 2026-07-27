//! Golden tests for Bash tool behavior: output truncation, exit codes,
//! timeout messages, and stderr fallback.

mod common;

use orbcode_tools::ToolRegistry;

fn registry() -> ToolRegistry {
    ToolRegistry::foundation()
}

#[tokio::test]
async fn bash_large_output_truncated_at_30k_chars() {
    let reg = registry();
    let ctx = common::test_context("bash-trunc-30k").await;

    // Generate ~40,000 'x' characters (well over the 30,000-char limit).
    let result = reg
        .invoke(
            "bash",
            r#"{"command":"python3 -c \"print('x' * 40000)\"","timeout":10000}"#,
            &ctx,
        )
        .await
        .expect("bash should succeed even when output is truncated");

    assert!(
        result
            .output
            .contains("[Bash output truncated for transcript safety."),
        "truncation diagnostic missing from output"
    );
    assert!(
        result.output.contains("Omitted"),
        "omission count missing from output"
    );
    // Output (including the diagnostic) should not exceed ~30,000 chars.
    assert!(
        result.output.chars().count() <= 31_000,
        "truncated output should be at most ~30k chars, got {}",
        result.output.chars().count()
    );
}

#[tokio::test]
async fn bash_truncation_metadata_shape() {
    let reg = registry();
    let ctx = common::test_context("bash-trunc-meta").await;

    let result = reg
        .invoke(
            "bash",
            r#"{"command":"python3 -c \"print('A' * 35000)\"","timeout":10000}"#,
            &ctx,
        )
        .await
        .expect("bash should succeed");

    let meta = result
        .metadata
        .as_ref()
        .expect("metadata should be present");
    let bash = &meta["bash"];
    assert_eq!(
        bash["outputTruncated"], true,
        "outputTruncated should be true"
    );
    assert!(
        bash["omittedChars"].as_u64().unwrap() > 0,
        "omittedChars should be positive"
    );
    assert!(
        bash["outputChars"].as_u64().unwrap() >= 35_000,
        "outputChars should reflect original length"
    );

    let trunc = &meta["truncation"];
    assert_eq!(trunc["truncated"], true);
    assert!(trunc["originalChars"].as_u64().unwrap() >= 35_000);
    assert!(trunc["omittedChars"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn bash_truncation_preserves_head_of_output() {
    let reg = registry();
    let ctx = common::test_context("bash-trunc-head").await;

    // Print a known prefix followed by filler to exceed the limit.
    let result = reg
        .invoke(
            "bash",
            r#"{"command":"printf 'GOLDEN_PREFIX_MARKER'; python3 -c \"print('z' * 40000)\"","timeout":10000}"#,
            &ctx,
        )
        .await
        .expect("bash should succeed");

    assert!(
        result.output.starts_with("GOLDEN_PREFIX_MARKER"),
        "truncation should preserve the head of the output"
    );
}

#[tokio::test]
async fn bash_success_exit_code_zero_metadata() {
    let reg = registry();
    let ctx = common::test_context("bash-exit-0").await;

    let result = reg
        .invoke("bash", r#"{"command":"true","timeout":5000}"#, &ctx)
        .await
        .expect("true should succeed");

    let meta = result.metadata.as_ref().expect("metadata");
    let bash = &meta["bash"];
    assert_eq!(bash["exitCode"], 0);
    assert_eq!(bash["interrupted"], false);
    assert_eq!(bash["timedOut"], false);
    assert_eq!(bash["outputTruncated"], false);
    assert!(
        bash["durationMs"].as_u64().is_some(),
        "durationMs should be present"
    );
}

#[tokio::test]
async fn bash_nonzero_exit_code_in_error_metadata() {
    let reg = registry();
    let ctx = common::test_context("bash-exit-42").await;

    let err = reg
        .invoke("bash", r#"{"command":"exit 42","timeout":5000}"#, &ctx)
        .await
        .expect_err("exit 42 should fail");

    let meta = err.metadata().expect("error should carry metadata");
    let bash = &meta["bash"];
    assert_eq!(bash["exitCode"], 42);
    assert_eq!(bash["timedOut"], false);
    assert_eq!(bash["interrupted"], false);
}

#[tokio::test]
async fn bash_timeout_error_message_format() {
    let reg = registry();
    let ctx = common::test_context("bash-timeout-fmt").await;

    let err = reg
        .invoke("bash", r#"{"command":"sleep 30","timeout":50}"#, &ctx)
        .await
        .expect_err("should time out");

    let msg = err.to_string();
    assert!(
        msg.contains("timed out"),
        "timeout error should mention 'timed out', got: {msg}"
    );
    assert!(
        msg.contains("50ms"),
        "timeout error should include the timeout value in ms, got: {msg}"
    );

    let meta = err.metadata().expect("timeout should carry metadata");
    assert_eq!(meta["bash"]["timedOut"], true);
    assert_eq!(meta["bash"]["timeoutMs"], 50);
}

#[tokio::test]
async fn bash_stderr_shown_when_stdout_empty() {
    let reg = registry();
    let ctx = common::test_context("bash-stderr-fallback").await;

    let err = reg
        .invoke(
            "bash",
            r#"{"command":"echo >&2 'stderr_golden_marker'; exit 1","timeout":5000}"#,
            &ctx,
        )
        .await
        .expect_err("nonzero exit should fail");

    let msg = err.to_string();
    assert!(
        msg.contains("stderr_golden_marker"),
        "when stdout is empty, stderr should appear in the error message, got: {msg}"
    );
}
