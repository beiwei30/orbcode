use super::*;

#[tokio::test]
async fn file_tools_round_trip() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file").await;
    registry
        .invoke(
            "file-write",
            r#"{"file_path":"notes/test.txt","content":"alpha\nbeta"}"#,
            &context,
        )
        .await
        .expect("file write");
    let read = registry
        .invoke(
            "file-read",
            r#"{"file_path":"notes/test.txt","offset":1,"limit":20}"#,
            &context,
        )
        .await
        .expect("file read");
    assert!(read.output.contains("alpha"));

    registry
        .invoke(
            "file-edit",
            r#"{"file_path":"notes/test.txt","old_string":"beta","new_string":"gamma"}"#,
            &context,
        )
        .await
        .expect("file edit");
    let updated = registry
        .invoke("file-read", r#"{"file_path":"notes/test.txt"}"#, &context)
        .await
        .expect("read updated");
    assert!(updated.output.contains("gamma"));
}

#[tokio::test]
async fn file_read_matches_typescript_size_and_token_limits() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-limits").await;
    let oversized_path = context.cwd.join("large.txt");
    tokio::fs::write(&oversized_path, "a\n".repeat(140_000))
        .await
        .expect("write oversized file");

    let oversized = registry
        .invoke("file-read", r#"{"file_path":"large.txt"}"#, &context)
        .await
        .expect_err("oversized read should fail");
    assert!(
        oversized
            .to_string()
            .contains("File content (273.4KB) exceeds maximum allowed size (256KB)")
    );

    let limited = registry
        .invoke(
            "file-read",
            r#"{"file_path":"large.txt","offset":1,"limit":1}"#,
            &context,
        )
        .await
        .expect("explicit limit bypasses whole-file size cap");
    assert_eq!(limited.output, "a\n");

    tokio::fs::write(context.cwd.join("too-many-tokens.txt"), "a".repeat(120_000))
        .await
        .expect("write token-heavy file");
    let token_heavy = registry
        .invoke(
            "file-read",
            r#"{"file_path":"too-many-tokens.txt"}"#,
            &context,
        )
        .await
        .expect_err("token-heavy read should fail");
    assert!(
        token_heavy
            .to_string()
            .contains("exceeds maximum allowed tokens")
    );
}

#[tokio::test]
async fn file_read_uses_one_based_line_ranges() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-read-lines").await;
    tokio::fs::write(context.cwd.join("lines.txt"), "one\ntwo\nthree\nfour\n")
        .await
        .expect("write line fixture");

    let offset_limit = registry
        .invoke(
            "file-read",
            r#"{"file_path":"lines.txt","offset":2,"limit":2}"#,
            &context,
        )
        .await
        .expect("read offset/limit range");
    assert_eq!(offset_limit.output, "two\nthree\n");

    let start_end = registry
        .invoke(
            "file-read",
            r#"{"file_path":"lines.txt","start_line":2,"end_line":3}"#,
            &context,
        )
        .await
        .expect("read start/end range");
    assert_eq!(start_end.output, "two\nthree\n");
}

#[tokio::test]
async fn file_read_handles_zero_limit_and_out_of_range_lines() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-read-line-bounds").await;
    tokio::fs::write(context.cwd.join("lines.txt"), "one\ntwo\nthree\n")
        .await
        .expect("write line fixture");

    let zero_limit = registry
        .invoke(
            "file-read",
            r#"{"file_path":"lines.txt","offset":2,"limit":0}"#,
            &context,
        )
        .await
        .expect("read zero lines");
    assert_eq!(zero_limit.output, "");

    let beyond_start = registry
        .invoke(
            "file-read",
            r#"{"file_path":"lines.txt","offset":99,"limit":10}"#,
            &context,
        )
        .await
        .expect("read beyond eof");
    assert_eq!(beyond_start.output, "");

    let beyond_end = registry
        .invoke(
            "file-read",
            r#"{"file_path":"lines.txt","start_line":2,"end_line":99}"#,
            &context,
        )
        .await
        .expect("read through eof");
    assert_eq!(beyond_end.output, "two\nthree\n");
}

#[tokio::test]
async fn file_read_handles_end_before_start_and_missing_final_newline() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-read-no-final-newline").await;
    tokio::fs::write(context.cwd.join("lines.txt"), "one\ntwo\nthree")
        .await
        .expect("write line fixture");

    let reversed = registry
        .invoke(
            "file-read",
            r#"{"file_path":"lines.txt","start_line":3,"end_line":2}"#,
            &context,
        )
        .await
        .expect("read reversed range");
    assert_eq!(reversed.output, "");

    let final_line = registry
        .invoke(
            "file-read",
            r#"{"file_path":"lines.txt","start_line":3,"end_line":3}"#,
            &context,
        )
        .await
        .expect("read final line");
    assert_eq!(final_line.output, "three");

    let range_to_eof = registry
        .invoke(
            "file-read",
            r#"{"file_path":"lines.txt","offset":2,"limit":10}"#,
            &context,
        )
        .await
        .expect("read range to eof");
    assert_eq!(range_to_eof.output, "two\nthree");
}

#[tokio::test]
async fn file_read_rejects_binary_files_with_typescript_style_message() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-read-binary").await;
    tokio::fs::write(context.cwd.join("archive.zip"), [0x50, 0x4b, 0x03, 0x04])
        .await
        .expect("write binary fixture");

    let error = registry
        .invoke("file-read", r#"{"file_path":"archive.zip"}"#, &context)
        .await
        .expect_err("binary read should fail");

    assert!(error.to_string().contains(
        "This tool cannot read binary files. The file appears to be a binary .zip file. Please use appropriate tools for binary file analysis."
    ));
}

#[tokio::test]
async fn file_edit_rejects_binary_files_before_matching() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-edit-binary").await;
    let path = context.cwd.join("payload.bin");
    tokio::fs::write(&path, [0, 159, 146, 150])
        .await
        .expect("write binary fixture");

    let error = registry
        .invoke(
            "file-edit",
            r#"{"file_path":"payload.bin","old_string":"target","new_string":"changed"}"#,
            &context,
        )
        .await
        .expect_err("binary edit should fail");

    assert!(error.to_string().contains(
        "This tool cannot edit binary files. The file appears to be a binary .bin file. Please use appropriate tools for binary file analysis."
    ));
    let unchanged = tokio::fs::read(&path)
        .await
        .expect("read unchanged binary fixture");
    assert_eq!(unchanged, [0, 159, 146, 150]);
}

#[tokio::test]
async fn file_edit_rejects_files_above_typescript_size_limit() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-edit-large").await;
    let path = context.cwd.join("huge.txt");
    let file = tokio::fs::File::create(&path)
        .await
        .expect("create sparse fixture");
    file.set_len(1024 * 1024 * 1024 + 1)
        .await
        .expect("size sparse fixture");

    let error = registry
        .invoke(
            "file-edit",
            r#"{"file_path":"huge.txt","old_string":"target","new_string":"changed"}"#,
            &context,
        )
        .await
        .expect_err("oversized edit should fail");

    assert!(
        error
            .to_string()
            .contains("File is too large to edit (1GB). Maximum editable file size is 1GB.")
    );
}

#[tokio::test]
async fn file_edit_preserves_exact_replacement_strings() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-edit-exact").await;
    tokio::fs::write(
        context.cwd.join("notes.txt"),
        "alpha\n  beta  \ngamma\nremove me\n",
    )
    .await
    .expect("write fixture");

    registry
        .invoke(
            "file-edit",
            r#"{"file_path":"notes.txt","old_string":"  beta  ","new_string":"  delta  "}"#,
            &context,
        )
        .await
        .expect("replace padded line");
    registry
        .invoke(
            "file-edit",
            r#"{"file_path":"notes.txt","old_string":"remove me\n","new_string":""}"#,
            &context,
        )
        .await
        .expect("delete line");

    let updated = tokio::fs::read_to_string(context.cwd.join("notes.txt"))
        .await
        .expect("read updated file");
    assert_eq!(updated, "alpha\n  delta  \ngamma\n");
}

#[tokio::test]
async fn file_edit_rejects_ambiguous_single_replacement() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-edit-ambiguous").await;
    let path = context.cwd.join("notes.txt");
    tokio::fs::write(&path, "target\nmiddle\ntarget\n")
        .await
        .expect("write fixture");

    let error = registry
        .invoke(
            "file-edit",
            r#"{"file_path":"notes.txt","old_string":"target","new_string":"changed"}"#,
            &context,
        )
        .await
        .expect_err("ambiguous edit should fail");

    assert!(error.to_string().contains("Found 2 matches"));
    let unchanged = tokio::fs::read_to_string(path)
        .await
        .expect("read unchanged file");
    assert_eq!(unchanged, "target\nmiddle\ntarget\n");
}

#[tokio::test]
async fn file_edit_crlf_round_trip() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-edit-crlf").await;
    let path = context.cwd.join("crlf.txt");
    tokio::fs::write(&path, "alpha\r\nbeta\r\ngamma\r\n")
        .await
        .expect("write CRLF fixture");

    registry
        .invoke("file-read", r#"{"file_path":"crlf.txt"}"#, &context)
        .await
        .expect("read CRLF file");

    registry
        .invoke(
            "file-edit",
            r#"{"file_path":"crlf.txt","old_string":"beta\n","new_string":"BETA\n"}"#,
            &context,
        )
        .await
        .expect("edit CRLF file with LF old_string/new_string");

    let raw = tokio::fs::read(&path).await.expect("read result");
    let result = String::from_utf8(raw).expect("utf8");
    assert_eq!(result, "alpha\r\nBETA\r\ngamma\r\n");
}

#[tokio::test]
async fn file_edit_preserves_bom() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-edit-bom").await;
    let path = context.cwd.join("bom.txt");
    tokio::fs::write(&path, "\u{FEFF}hello\nworld\n")
        .await
        .expect("write BOM fixture");

    registry
        .invoke("file-read", r#"{"file_path":"bom.txt"}"#, &context)
        .await
        .expect("read BOM file");

    registry
        .invoke(
            "file-edit",
            r#"{"file_path":"bom.txt","old_string":"hello","new_string":"HELLO"}"#,
            &context,
        )
        .await
        .expect("edit BOM file");

    let raw = tokio::fs::read(&path).await.expect("read result");
    let result = String::from_utf8(raw).expect("utf8");
    assert!(result.starts_with("\u{FEFF}"), "BOM must be preserved");
    assert!(result.contains("HELLO"), "edit must apply");
}

#[tokio::test]
async fn file_write_preserves_bom_on_overwrite() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-write-bom").await;
    let path = context.cwd.join("bom.txt");
    tokio::fs::write(&path, "\u{FEFF}original\n")
        .await
        .expect("write BOM fixture");

    registry
        .invoke(
            "file-write",
            r#"{"file_path":"bom.txt","content":"replaced"}"#,
            &context,
        )
        .await
        .expect("overwrite BOM file");

    let raw = tokio::fs::read(&path).await.expect("read result");
    let result = String::from_utf8(raw).expect("utf8");
    assert!(
        result.starts_with("\u{FEFF}"),
        "BOM must be preserved on overwrite"
    );
    assert!(result.contains("replaced"));
}

#[tokio::test]
async fn file_write_preserves_crlf_on_overwrite() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-write-crlf").await;
    let path = context.cwd.join("crlf.txt");
    tokio::fs::write(&path, "line1\r\nline2\r\n")
        .await
        .expect("write CRLF fixture");

    registry
        .invoke(
            "file-write",
            r#"{"file_path":"crlf.txt","content":"new1\nnew2\n"}"#,
            &context,
        )
        .await
        .expect("overwrite CRLF file with LF content");

    let raw = tokio::fs::read(&path).await.expect("read result");
    let result = String::from_utf8(raw).expect("utf8");
    assert_eq!(
        result, "new1\r\nnew2\r\n",
        "line endings must be converted to CRLF"
    );
}

#[tokio::test]
async fn file_write_new_file_appends_final_newline() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-write-newfile-nl").await;

    registry
        .invoke(
            "file-write",
            r#"{"file_path":"new.txt","content":"no trailing newline"}"#,
            &context,
        )
        .await
        .expect("write new file without trailing newline");

    let raw = tokio::fs::read(context.cwd.join("new.txt"))
        .await
        .expect("read result");
    let result = String::from_utf8(raw).expect("utf8");
    assert_eq!(
        result, "no trailing newline\n",
        "new file must get trailing newline"
    );
}

#[tokio::test]
async fn file_write_preserves_no_final_newline() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-write-no-nl").await;
    let path = context.cwd.join("no-nl.txt");
    tokio::fs::write(&path, "no newline at end")
        .await
        .expect("write fixture without final newline");

    registry
        .invoke(
            "file-write",
            r#"{"file_path":"no-nl.txt","content":"replaced content"}"#,
            &context,
        )
        .await
        .expect("overwrite file without final newline");

    let raw = tokio::fs::read(&path).await.expect("read result");
    let result = String::from_utf8(raw).expect("utf8");
    assert_eq!(result, "replaced content", "must NOT add trailing newline");
}

#[tokio::test]
async fn file_write_existing_with_final_newline_appends_newline() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-write-existing-nl").await;
    let path = context.cwd.join("has-nl.txt");
    tokio::fs::write(&path, "original\n")
        .await
        .expect("write fixture with final newline");

    registry
        .invoke(
            "file-write",
            r#"{"file_path":"has-nl.txt","content":"replaced"}"#,
            &context,
        )
        .await
        .expect("overwrite file that has final newline");

    let raw = tokio::fs::read(&path).await.expect("read result");
    let result = String::from_utf8(raw).expect("utf8");
    assert_eq!(
        result, "replaced\n",
        "must add trailing newline matching original"
    );
}

#[tokio::test]
async fn file_read_metadata_includes_encoding_and_has_bom() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-read-encoding").await;
    tokio::fs::write(context.cwd.join("plain.txt"), "hello\n")
        .await
        .expect("write plain file");
    tokio::fs::write(context.cwd.join("bom.txt"), "\u{FEFF}hello\n")
        .await
        .expect("write BOM file");

    let plain = registry
        .invoke("file-read", r#"{"file_path":"plain.txt"}"#, &context)
        .await
        .expect("read plain");
    let plain_meta = plain.metadata.expect("plain metadata");
    assert_eq!(plain_meta["encoding"], "utf-8");
    assert_eq!(plain_meta["hasBom"], false);

    let bom = registry
        .invoke("file-read", r#"{"file_path":"bom.txt"}"#, &context)
        .await
        .expect("read BOM");
    let bom_meta = bom.metadata.expect("BOM metadata");
    assert_eq!(bom_meta["encoding"], "utf-8");
    assert_eq!(bom_meta["hasBom"], true);
}

#[tokio::test]
async fn file_write_empty_content_no_trailing_newline() {
    let registry = ToolRegistry::foundation();
    let context = test_context("file-write-empty").await;

    registry
        .invoke(
            "file-write",
            r#"{"file_path":"empty.txt","content":""}"#,
            &context,
        )
        .await
        .expect("write empty file");

    let raw = tokio::fs::read(context.cwd.join("empty.txt"))
        .await
        .expect("read result");
    assert!(raw.is_empty(), "empty content must stay empty");
}
