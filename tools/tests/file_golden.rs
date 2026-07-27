//! Golden tests for file tool behavior: binary detection/rejection,
//! CRLF round-trip, BOM preservation, and staleness error messages.

use std::fmt::Write as _;

mod common;

use orbcode_tools::ToolRegistry;

fn registry() -> ToolRegistry {
    ToolRegistry::foundation()
}

// ---------------------------------------------------------------------------
// Binary file detection / rejection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_read_rejects_known_binary_extension() {
    let reg = registry();
    let ctx = common::test_context("bin-ext-read").await;

    let path = ctx.cwd.join("image.png");
    std::fs::write(&path, [0x89, 0x50, 0x4E, 0x47]).expect("write png");

    let err = reg
        .invoke(
            "file-read",
            &format!(r#"{{"file_path":"{}"}}"#, path.display()),
            &ctx,
        )
        .await
        .expect_err("binary read should fail");

    let msg = err.to_string();
    assert!(
        msg.contains("cannot read binary files"),
        "error should say 'cannot read binary files', got: {msg}"
    );
    assert!(
        msg.contains(".png"),
        "error should name the .png extension, got: {msg}"
    );
}

#[tokio::test]
async fn file_read_rejects_binary_by_content_heuristic() {
    let reg = registry();
    let ctx = common::test_context("bin-content-read").await;

    let path = ctx.cwd.join("data.dat");
    // Write a file with null bytes — triggers the content heuristic.
    let mut content = vec![0u8; 1024];
    content[0] = b'H';
    content[1] = 0x00; // null byte
    std::fs::write(&path, &content).expect("write binary content");

    let err = reg
        .invoke(
            "file-read",
            &format!(r#"{{"file_path":"{}"}}"#, path.display()),
            &ctx,
        )
        .await
        .expect_err("binary content should fail");

    let msg = err.to_string();
    assert!(
        msg.contains("binary"),
        "error should mention binary, got: {msg}"
    );
}

#[tokio::test]
async fn file_edit_rejects_known_binary_extension() {
    let reg = registry();
    let ctx = common::test_context("bin-ext-edit").await;

    let path = ctx.cwd.join("program.exe");
    std::fs::write(&path, [0x4D, 0x5A, 0x90, 0x00]).expect("write exe");

    let err = reg
        .invoke(
            "file-edit",
            &format!(
                r#"{{"file_path":"{}","old_string":"MZ","new_string":"XX"}}"#,
                path.display()
            ),
            &ctx,
        )
        .await
        .expect_err("binary edit should fail");

    let msg = err.to_string();
    assert!(
        msg.contains("cannot edit binary files"),
        "error should say 'cannot edit binary files', got: {msg}"
    );
    assert!(
        msg.contains(".exe"),
        "error should name the .exe extension, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// CRLF round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_write_preserves_crlf_on_overwrite() {
    let reg = registry();
    let ctx = common::test_context("crlf-write").await;

    let path = ctx.cwd.join("crlf.txt");
    // Seed with CRLF line endings.
    std::fs::write(&path, "line1\r\nline2\r\n").expect("write crlf seed");

    // Overwrite with LF content — the tool should convert to CRLF.
    reg.invoke(
        "file-write",
        &format!(
            r#"{{"file_path":"{}","content":"alpha\nbeta\n"}}"#,
            path.display()
        ),
        &ctx,
    )
    .await
    .expect("write should succeed");

    let on_disk = std::fs::read(&path).expect("read back");
    let text = String::from_utf8(on_disk).expect("utf8");
    assert!(
        text.contains("\r\n"),
        "overwritten content should have CRLF, got bytes: {:?}",
        text.as_bytes()
    );
    assert!(
        !text.replace("\r\n", "").contains('\r'),
        "should not have stray CR outside CRLF pairs"
    );
}

#[tokio::test]
async fn file_edit_preserves_crlf_through_edit() {
    let reg = registry();
    let ctx = common::context_with_read_state("crlf-edit").await;

    let path = ctx.cwd.join("crlf_edit.txt");
    std::fs::write(&path, "hello\r\nworld\r\n").expect("write crlf");

    // Read first (required for edit freshness).
    reg.invoke(
        "file-read",
        &format!(r#"{{"file_path":"{}"}}"#, path.display()),
        &ctx,
    )
    .await
    .expect("read");

    // Edit using LF-normalized strings.
    reg.invoke(
        "file-edit",
        &format!(
            r#"{{"file_path":"{}","old_string":"hello\nworld","new_string":"goodbye\nearth"}}"#,
            path.display()
        ),
        &ctx,
    )
    .await
    .expect("edit should succeed");

    let on_disk = std::fs::read_to_string(&path).expect("read back");
    assert!(
        on_disk.contains("goodbye\r\nearth"),
        "edited content should preserve CRLF, got: {on_disk:?}"
    );
}

// ---------------------------------------------------------------------------
// BOM preservation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_write_preserves_bom_on_overwrite() {
    let reg = registry();
    let ctx = common::test_context("bom-write").await;

    let path = ctx.cwd.join("bom.txt");
    // Seed with BOM.
    std::fs::write(&path, "\u{FEFF}original content\n").expect("write bom file");

    reg.invoke(
        "file-write",
        &format!(
            r#"{{"file_path":"{}","content":"replaced content\n"}}"#,
            path.display()
        ),
        &ctx,
    )
    .await
    .expect("write should succeed");

    let on_disk = std::fs::read_to_string(&path).expect("read back");
    assert!(
        on_disk.starts_with('\u{FEFF}'),
        "BOM should be preserved on overwrite"
    );
    assert!(
        on_disk.contains("replaced content"),
        "new content should be written"
    );
}

#[tokio::test]
async fn file_edit_preserves_bom_through_edit() {
    let reg = registry();
    let ctx = common::context_with_read_state("bom-edit").await;

    let path = ctx.cwd.join("bom_edit.txt");
    std::fs::write(&path, "\u{FEFF}alpha beta\n").expect("write bom");

    reg.invoke(
        "file-read",
        &format!(r#"{{"file_path":"{}"}}"#, path.display()),
        &ctx,
    )
    .await
    .expect("read");

    reg.invoke(
        "file-edit",
        &format!(
            r#"{{"file_path":"{}","old_string":"alpha beta","new_string":"gamma delta"}}"#,
            path.display()
        ),
        &ctx,
    )
    .await
    .expect("edit should succeed");

    let on_disk = std::fs::read_to_string(&path).expect("read back");
    assert!(
        on_disk.starts_with('\u{FEFF}'),
        "BOM should be preserved through edit"
    );
    assert!(on_disk.contains("gamma delta"));
}

#[tokio::test]
async fn file_read_reports_bom_in_metadata() {
    let reg = registry();
    let ctx = common::test_context("bom-meta").await;

    // File with BOM.
    let with_bom = ctx.cwd.join("with_bom.txt");
    std::fs::write(&with_bom, "\u{FEFF}content\n").expect("write");

    let result = reg
        .invoke(
            "file-read",
            &format!(r#"{{"file_path":"{}"}}"#, with_bom.display()),
            &ctx,
        )
        .await
        .expect("read bom file");

    let meta = result.metadata.as_ref().expect("metadata");
    assert_eq!(meta["hasBom"], true, "hasBom should be true for BOM file");
    assert_eq!(meta["encoding"], "utf-8");

    // File without BOM.
    let no_bom = ctx.cwd.join("no_bom.txt");
    std::fs::write(&no_bom, "content\n").expect("write");

    let result2 = reg
        .invoke(
            "file-read",
            &format!(r#"{{"file_path":"{}"}}"#, no_bom.display()),
            &ctx,
        )
        .await
        .expect("read non-bom file");

    let meta2 = result2.metadata.as_ref().expect("metadata");
    assert_eq!(
        meta2["hasBom"], false,
        "hasBom should be false for non-BOM file"
    );
}

// ---------------------------------------------------------------------------
// Staleness error messages (golden text)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_edit_stale_without_prior_read_error_text() {
    let reg = registry();
    let ctx = common::context_with_read_state("stale-edit").await;

    let path = ctx.cwd.join("stale.txt");
    std::fs::write(&path, "original\n").expect("write");

    let err = reg
        .invoke(
            "file-edit",
            &format!(
                r#"{{"file_path":"{}","old_string":"original","new_string":"changed"}}"#,
                path.display()
            ),
            &ctx,
        )
        .await
        .expect_err("edit without prior read should fail");

    let msg = err.to_string();
    assert!(
        msg.contains("File has been unexpectedly modified"),
        "stale edit error should contain exact message, got: {msg}"
    );
    assert!(
        msg.contains("Read it again"),
        "stale edit error should advise re-reading, got: {msg}"
    );
}

#[tokio::test]
async fn file_write_stale_after_external_modification_error_text() {
    let reg = registry();
    let ctx = common::context_with_read_state("stale-write").await;

    let path = ctx.cwd.join("stale_write.txt");
    std::fs::write(&path, "original\n").expect("write");

    // Read the file to record its state.
    reg.invoke(
        "file-read",
        &format!(r#"{{"file_path":"{}"}}"#, path.display()),
        &ctx,
    )
    .await
    .expect("read");

    // Simulate external modification.
    common::advance_mtime(&path, 10);

    let err = reg
        .invoke(
            "file-write",
            &format!(
                r#"{{"file_path":"{}","content":"new content\n"}}"#,
                path.display()
            ),
            &ctx,
        )
        .await
        .expect_err("write after external mod should fail");

    let msg = err.to_string();
    assert!(
        msg.contains("File has been modified since read"),
        "stale write error should contain exact message, got: {msg}"
    );
    assert!(
        msg.contains("by the user or by a linter"),
        "stale write error should mention user/linter cause, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Diff metadata in file-edit results
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_edit_produces_diff_metadata() {
    let reg = registry();
    let ctx = common::context_with_read_state("diff-meta").await;

    let path = ctx.cwd.join("diff_test.txt");
    std::fs::write(&path, "line1\nline2\nline3\nline4\nline5\nline6\nline7\n").expect("write");

    reg.invoke(
        "file-read",
        &format!(r#"{{"file_path":"{}"}}"#, path.display()),
        &ctx,
    )
    .await
    .expect("read");

    let result = reg
        .invoke(
            "file-edit",
            &format!(
                r#"{{"file_path":"{}","old_string":"line4","new_string":"LINE_FOUR"}}"#,
                path.display()
            ),
            &ctx,
        )
        .await
        .expect("edit should succeed");

    let meta = result.metadata.as_ref().expect("metadata present");
    assert!(meta["diff"].is_string(), "diff field should be a string");
    let diff = meta["diff"].as_str().unwrap();
    assert!(diff.contains("@@"), "diff should contain hunk header");
    assert!(diff.contains("-line4"), "diff should show removed line");
    assert!(diff.contains("+LINE_FOUR"), "diff should show added line");
    assert!(
        diff.contains(" line3"),
        "diff should have context before change"
    );
    assert!(
        diff.contains(" line5"),
        "diff should have context after change"
    );

    assert_eq!(meta["lineRange"]["start"], 4, "lineRange.start");
    assert_eq!(meta["lineRange"]["end"], 4, "lineRange.end");
    assert_eq!(meta["linesAdded"], 1);
    assert_eq!(meta["linesRemoved"], 1);
    assert!(
        meta.get("diffTruncated").is_none(),
        "diffTruncated should not be present for short diffs"
    );
}

#[tokio::test]
async fn file_edit_diff_truncated_for_large_change() {
    let reg = registry();
    let ctx = common::context_with_read_state("diff-trunc").await;

    let path = ctx.cwd.join("big_edit.txt");
    let mut content = String::new();
    for i in 1..=50 {
        writeln!(content, "line{i}").expect("writing to String cannot fail");
    }
    std::fs::write(&path, &content).expect("write");

    reg.invoke(
        "file-read",
        &format!(r#"{{"file_path":"{}"}}"#, path.display()),
        &ctx,
    )
    .await
    .expect("read");

    // Replace a large block (lines 5-30) to generate > 20 diff lines.
    let old_string: String = (5..=30).map(|i| format!("line{i}\n")).collect();
    let new_string: String = (5..=30).map(|i| format!("CHANGED{i}\n")).collect();
    let old_trimmed = old_string.trim_end();
    let new_trimmed = new_string.trim_end();

    let result = reg
        .invoke(
            "file-edit",
            &format!(
                r#"{{"file_path":"{}","old_string":"{}","new_string":"{}"}}"#,
                path.display(),
                old_trimmed.replace('\n', "\\n"),
                new_trimmed.replace('\n', "\\n"),
            ),
            &ctx,
        )
        .await
        .expect("edit should succeed");

    let meta = result.metadata.as_ref().expect("metadata present");
    assert_eq!(meta["diffTruncated"], true, "diffTruncated should be true");
    let diff = meta["diff"].as_str().unwrap();
    let diff_line_count = diff.lines().count();
    assert!(
        diff_line_count <= 20,
        "diff should be capped at 20 lines, got {diff_line_count}"
    );
}

#[tokio::test]
async fn file_edit_replace_all_diff_shows_first_and_last() {
    let reg = registry();
    let ctx = common::context_with_read_state("diff-replall").await;

    let path = ctx.cwd.join("replace_all.txt");
    let content = "aaa\nFOO\nbbb\nFOO\nccc\nFOO\nddd\nFOO\neee\n";
    std::fs::write(&path, content).expect("write");

    reg.invoke(
        "file-read",
        &format!(r#"{{"file_path":"{}"}}"#, path.display()),
        &ctx,
    )
    .await
    .expect("read");

    let result = reg
        .invoke(
            "file-edit",
            &format!(
                r#"{{"file_path":"{}","old_string":"FOO","new_string":"BAR","replace_all":true}}"#,
                path.display()
            ),
            &ctx,
        )
        .await
        .expect("replace_all edit should succeed");

    let meta = result.metadata.as_ref().expect("metadata present");
    assert_eq!(meta["linesAdded"], 4);
    assert_eq!(meta["linesRemoved"], 4);

    let diff = meta["diff"].as_str().unwrap();
    assert!(
        diff.contains("more replacement(s)"),
        "diff should summarize middle occurrences, got: {diff}"
    );
    assert!(diff.contains("-FOO"), "diff should show removed FOO");
    assert!(diff.contains("+BAR"), "diff should show added BAR");
}

// ---------------------------------------------------------------------------
// linesWritten in file-write results
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_write_reports_lines_written_metadata() {
    let reg = registry();
    let ctx = common::test_context("lines-written").await;

    let path = ctx.cwd.join("write_meta.txt");

    let result = reg
        .invoke(
            "file-write",
            &format!(
                r#"{{"file_path":"{}","content":"alpha\nbeta\ngamma\n"}}"#,
                path.display()
            ),
            &ctx,
        )
        .await
        .expect("write should succeed");

    let meta = result.metadata.as_ref().expect("metadata present");
    assert_eq!(meta["linesWritten"], 3, "linesWritten should be 3");
}

#[tokio::test]
async fn file_write_reports_lines_written_for_single_line() {
    let reg = registry();
    let ctx = common::test_context("lines-written-1").await;

    let path = ctx.cwd.join("one_line.txt");

    let result = reg
        .invoke(
            "file-write",
            &format!(
                r#"{{"file_path":"{}","content":"single line"}}"#,
                path.display()
            ),
            &ctx,
        )
        .await
        .expect("write should succeed");

    let meta = result.metadata.as_ref().expect("metadata present");
    // "single line\n" (trailing newline added) → 1 line
    assert_eq!(meta["linesWritten"], 1, "linesWritten should be 1");
}
