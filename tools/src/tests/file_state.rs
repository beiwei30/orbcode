use super::*;

#[tokio::test]
async fn file_state_edit_without_prior_read_is_rejected() {
    let registry = ToolRegistry::foundation();
    let context = context_with_read_state("file-state-unread").await;
    let path = context.cwd.join("notes.txt");
    tokio::fs::write(&path, "alpha\nbeta\n")
        .await
        .expect("write fixture");

    let error = registry
        .invoke(
            "file-edit",
            r#"{"file_path":"notes.txt","old_string":"alpha","new_string":"omega"}"#,
            &context,
        )
        .await
        .expect_err("edit without read should be rejected");

    assert!(
        error
            .to_string()
            .ends_with(crate::file_state::FILE_UNEXPECTEDLY_MODIFIED_ERROR)
    );
    let unchanged = tokio::fs::read_to_string(&path).await.expect("read file");
    assert_eq!(unchanged, "alpha\nbeta\n");
}

#[tokio::test]
async fn file_state_edit_after_read_without_change_succeeds() {
    let registry = ToolRegistry::foundation();
    let context = context_with_read_state("file-state-fresh").await;
    let path = context.cwd.join("notes.txt");
    tokio::fs::write(&path, "alpha\nbeta\n")
        .await
        .expect("write fixture");

    registry
        .invoke("file-read", r#"{"file_path":"notes.txt"}"#, &context)
        .await
        .expect("read records state");
    registry
        .invoke(
            "file-edit",
            r#"{"file_path":"notes.txt","old_string":"alpha","new_string":"omega"}"#,
            &context,
        )
        .await
        .expect("edit after fresh read should succeed");

    let updated = tokio::fs::read_to_string(&path).await.expect("read file");
    assert_eq!(updated, "omega\nbeta\n");
}

#[tokio::test]
async fn file_state_edit_after_external_modification_is_rejected() {
    let registry = ToolRegistry::foundation();
    let context = context_with_read_state("file-state-stale").await;
    let path = context.cwd.join("notes.txt");
    tokio::fs::write(&path, "alpha\nbeta\n")
        .await
        .expect("write fixture");

    registry
        .invoke("file-read", r#"{"file_path":"notes.txt"}"#, &context)
        .await
        .expect("read records state");

    tokio::fs::write(&path, "alpha\nGAMMA\n")
        .await
        .expect("external modification");
    advance_mtime(&path, 10);

    let error = registry
        .invoke(
            "file-edit",
            r#"{"file_path":"notes.txt","old_string":"alpha","new_string":"omega"}"#,
            &context,
        )
        .await
        .expect_err("stale edit should be rejected");
    assert!(
        error
            .to_string()
            .ends_with(crate::file_state::FILE_UNEXPECTEDLY_MODIFIED_ERROR)
    );

    registry
        .invoke("file-read", r#"{"file_path":"notes.txt"}"#, &context)
        .await
        .expect("re-read refreshes state");
    registry
        .invoke(
            "file-edit",
            r#"{"file_path":"notes.txt","old_string":"alpha","new_string":"omega"}"#,
            &context,
        )
        .await
        .expect("edit after re-read should succeed");
    let updated = tokio::fs::read_to_string(&path).await.expect("read file");
    assert_eq!(updated, "omega\nGAMMA\n");
}

#[tokio::test]
async fn file_state_write_refreshes_read_state_for_later_edit() {
    let registry = ToolRegistry::foundation();
    let context = context_with_read_state("file-state-write-refresh").await;
    let path = context.cwd.join("notes.txt");
    tokio::fs::write(&path, "alpha\n")
        .await
        .expect("write fixture");

    registry
        .invoke("file-read", r#"{"file_path":"notes.txt"}"#, &context)
        .await
        .expect("read records state");
    registry
        .invoke(
            "file-write",
            r#"{"file_path":"notes.txt","content":"alpha\nbeta\n"}"#,
            &context,
        )
        .await
        .expect("write refreshes state");
    advance_mtime(&path, 5);

    registry
        .invoke(
            "file-edit",
            r#"{"file_path":"notes.txt","old_string":"beta","new_string":"delta"}"#,
            &context,
        )
        .await
        .expect("edit after write should succeed without re-read");
    let updated = tokio::fs::read_to_string(&path).await.expect("read file");
    assert_eq!(updated, "alpha\ndelta\n");
}

#[tokio::test]
async fn file_state_write_after_external_modification_is_rejected() {
    let registry = ToolRegistry::foundation();
    let context = context_with_read_state("file-state-write-stale").await;
    let path = context.cwd.join("notes.txt");
    tokio::fs::write(&path, "alpha\n")
        .await
        .expect("write fixture");

    registry
        .invoke("file-read", r#"{"file_path":"notes.txt"}"#, &context)
        .await
        .expect("read records state");
    tokio::fs::write(&path, "tampered\n")
        .await
        .expect("external modification");
    advance_mtime(&path, 10);

    let error = registry
        .invoke(
            "file-write",
            r#"{"file_path":"notes.txt","content":"fresh\n"}"#,
            &context,
        )
        .await
        .expect_err("stale write should be rejected");
    assert!(
        error
            .to_string()
            .ends_with(crate::file_state::FILE_MODIFIED_SINCE_READ_ERROR)
    );
}

#[tokio::test]
async fn file_state_write_to_new_file_needs_no_prior_read() {
    let registry = ToolRegistry::foundation();
    let context = context_with_read_state("file-state-new-write").await;

    registry
        .invoke(
            "file-write",
            r#"{"file_path":"fresh.txt","content":"hello\n"}"#,
            &context,
        )
        .await
        .expect("new file write should succeed");
    let written = tokio::fs::read_to_string(context.cwd.join("fresh.txt"))
        .await
        .expect("read file");
    assert_eq!(written, "hello\n");
}

#[tokio::test]
async fn file_state_disabled_context_skips_freshness_check() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-state-disabled").await;
    let path = context.cwd.join("notes.txt");
    tokio::fs::write(&path, "alpha\n")
        .await
        .expect("write fixture");

    registry
        .invoke(
            "file-edit",
            r#"{"file_path":"notes.txt","old_string":"alpha","new_string":"omega"}"#,
            &context,
        )
        .await
        .expect("edit without read state should succeed when disabled");
    let updated = tokio::fs::read_to_string(&path).await.expect("read file");
    assert_eq!(updated, "omega\n");
}

#[tokio::test]
async fn file_state_partial_read_does_not_satisfy_edit_freshness() {
    let registry = ToolRegistry::foundation();
    let context = context_with_read_state("file-state-partial").await;
    let path = context.cwd.join("notes.txt");
    tokio::fs::write(&path, "alpha\nbeta\ngamma\n")
        .await
        .expect("write fixture");

    registry
        .invoke(
            "file-read",
            r#"{"file_path":"notes.txt","offset":1,"limit":1}"#,
            &context,
        )
        .await
        .expect("partial read records state");
    advance_mtime(&path, 10);

    let error = registry
        .invoke(
            "file-edit",
            r#"{"file_path":"notes.txt","old_string":"alpha","new_string":"omega"}"#,
            &context,
        )
        .await
        .expect_err("partial read should not satisfy freshness after mtime bump");
    assert!(
        error
            .to_string()
            .ends_with(crate::file_state::FILE_UNEXPECTEDLY_MODIFIED_ERROR)
    );
}
