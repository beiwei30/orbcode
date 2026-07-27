use super::*;

fn parse(metadata: &Value) -> orbcode_protocol::ToolResultMetadata {
    serde_json::from_value(metadata.clone()).expect("unified metadata parses")
}

#[tokio::test]
async fn bash_populates_unified_schema_and_keeps_legacy_block() {
    let registry = ToolRegistry::foundation();
    let context = test_context("meta-bash").await;
    let outcome = registry
        .invoke("bash", r#"{"command":"printf hi"}"#, &context)
        .await
        .expect("bash run");
    let metadata = outcome.metadata.expect("bash metadata");
    let unified = parse(&metadata);
    assert!(unified.duration_ms.is_some());
    assert!(unified.truncation.is_some());
    let permissions = unified.permissions.expect("permissions");
    assert_eq!(permissions.tools_allowed, Some(true));
    let sandbox = unified.sandbox.expect("sandbox");
    assert!(sandbox.mode.is_some());
    assert!(metadata.get("bash").is_some());
}

#[tokio::test]
async fn file_read_and_write_populate_file_change_metadata() {
    let registry = ToolRegistry::foundation();
    let context = test_context("meta-file").await;
    let written = registry
        .invoke(
            "file-write",
            r#"{"file_path":"out/data.txt","content":"alpha"}"#,
            &context,
        )
        .await
        .expect("file write");
    let write_changes = parse(&written.metadata.unwrap())
        .file_changes
        .expect("write file changes");
    assert_eq!(write_changes.operation.as_deref(), Some("write"));
    assert!(write_changes.paths[0].ends_with("data.txt"));

    let read = registry
        .invoke("file-read", r#"{"file_path":"out/data.txt"}"#, &context)
        .await
        .expect("file read");
    let read_changes = parse(&read.metadata.unwrap())
        .file_changes
        .expect("read file changes");
    assert_eq!(read_changes.operation.as_deref(), Some("read"));
}

#[tokio::test]
async fn glob_and_grep_populate_duration_and_keep_legacy_blocks() {
    let registry = ToolRegistry::foundation();
    let context = test_context("meta-search").await;
    tokio::fs::write(context.cwd.join("alpha.rs"), "fn alpha() {}\n")
        .await
        .expect("seed file");

    let glob = registry
        .invoke("glob", r#"{"pattern":"**/*.rs"}"#, &context)
        .await
        .expect("glob");
    let glob_meta = glob.metadata.expect("glob metadata");
    assert!(parse(&glob_meta).duration_ms.is_some());
    assert!(parse(&glob_meta).truncation.is_some());
    assert!(glob_meta.get("glob").is_some());

    let grep = registry
        .invoke(
            "grep",
            r#"{"pattern":"alpha","output_mode":"content"}"#,
            &context,
        )
        .await
        .expect("grep");
    let grep_meta = grep.metadata.expect("grep metadata");
    assert!(parse(&grep_meta).duration_ms.is_some());
    assert!(grep_meta.get("grep").is_some());
}

#[tokio::test]
async fn orphaned_status_serde_round_trip() {
    use crate::{
        BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus, background_log_path,
        read_background_task_record, write_background_task_record,
    };

    let context = test_context("orphaned-serde").await;
    let log_path = background_log_path(&context.home_dir, "agent-orphan-serde");
    std::fs::create_dir_all(log_path.parent().expect("logs dir")).expect("create logs dir");
    std::fs::write(&log_path, "").expect("write log");

    let mut record = BackgroundTaskRecord::new_local_agent(
        "agent-orphan-serde".to_string(),
        "session-1".to_string(),
        "session-1:agent-bbb".to_string(),
        "toolu-parent".to_string(),
        "general-purpose".to_string(),
        "orphan test".to_string(),
        context.cwd.display().to_string(),
        Some("claude-sonnet-4-20250514".to_string()),
        None,
        log_path.display().to_string(),
    );
    record.status = BackgroundTaskStatus::Orphaned;
    record.error = Some("process restarted; agent was orphaned".to_string());

    write_background_task_record(&context.home_dir, &record)
        .await
        .expect("write");
    let loaded = read_background_task_record(&context.home_dir, "agent-orphan-serde")
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(loaded.status, BackgroundTaskStatus::Orphaned);
    assert!(loaded.status.is_terminal());
    assert!(!loaded.status.is_active());
    assert_eq!(loaded.task_kind, BackgroundTaskKind::LocalAgent);
    assert_eq!(
        loaded.error.as_deref(),
        Some("process restarted; agent was orphaned")
    );
}
