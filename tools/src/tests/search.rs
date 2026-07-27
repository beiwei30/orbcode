use super::*;

#[tokio::test]
async fn glob_accepts_typescript_path_field() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("glob-path").await;
    std::fs::create_dir_all(context.cwd.join("src")).expect("create src");
    std::fs::write(context.cwd.join("src/lib.rs"), "pub fn demo() {}\n").expect("write lib");

    let result = registry
        .invoke("glob", r#"{"pattern":"*.rs","path":"src"}"#, &context)
        .await
        .expect("glob with TS path field");

    assert!(result.output.contains("src/lib.rs"));
}

#[tokio::test]
async fn glob_includes_hidden_and_gitignored_files() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("glob-hidden-ignored").await;
    std::fs::write(context.cwd.join(".gitignore"), "ignored.rs\n").expect("write gitignore");
    std::fs::write(context.cwd.join("visible.rs"), "pub fn visible() {}\n")
        .expect("write visible file");
    std::fs::write(context.cwd.join(".hidden.rs"), "pub fn hidden() {}\n")
        .expect("write hidden file");
    std::fs::write(context.cwd.join("ignored.rs"), "pub fn ignored() {}\n")
        .expect("write ignored file");

    let result = registry
        .invoke("glob", r#"{"pattern":"*.rs","path":"."}"#, &context)
        .await
        .expect("glob should include hidden and gitignored files");

    assert!(result.output.contains("visible.rs"));
    assert!(result.output.contains(".hidden.rs"));
    assert!(result.output.contains("ignored.rs"));
}

#[tokio::test]
async fn glob_sorts_by_modified_time_oldest_first() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("glob-sort").await;
    std::fs::create_dir_all(context.cwd.join("src")).expect("create src");
    std::fs::write(context.cwd.join("src/old.rs"), "pub fn old() {}\n").expect("write old");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(context.cwd.join("src/new.rs"), "pub fn new() {}\n").expect("write new");

    let result = registry
        .invoke("glob", r#"{"pattern":"*.rs","path":"src"}"#, &context)
        .await
        .expect("glob should succeed");

    let paths = result.output.lines().collect::<Vec<_>>();
    assert_eq!(paths.first().copied(), Some("src/old.rs"));
    assert_eq!(paths.get(1).copied(), Some("src/new.rs"));
}

#[tokio::test]
async fn grep_supports_query_alias_and_glob_filter() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("grep-query").await;
    std::fs::create_dir_all(context.cwd.join("src")).expect("create src");
    std::fs::create_dir_all(context.cwd.join("docs")).expect("create docs");
    std::fs::write(context.cwd.join("src/lib.rs"), "alpha\n").expect("write src file");
    std::fs::write(context.cwd.join("docs/guide.md"), "alpha\n").expect("write docs file");

    let result = registry
        .invoke(
            "grep",
            r#"{"query":"alpha","path":".","glob":"src/*.rs","output_mode":"files_with_matches"}"#,
            &context,
        )
        .await
        .expect("grep with query alias");

    assert!(result.output.contains("src/lib.rs"));
    assert!(!result.output.contains("docs/guide.md"));
}

#[tokio::test]
async fn grep_includes_hidden_respects_gitignore_and_sorts_newest_first() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("grep-hidden-ignored-sort").await;
    std::fs::create_dir_all(context.cwd.join(".git")).expect("create git dir");
    std::fs::write(context.cwd.join(".gitignore"), "ignored.txt\n").expect("write gitignore");
    std::fs::write(context.cwd.join(".git/config"), "alpha\n").expect("write git metadata");
    std::fs::write(context.cwd.join("old.txt"), "alpha\n").expect("write old");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(context.cwd.join(".hidden.txt"), "alpha\n").expect("write hidden");
    std::fs::write(context.cwd.join("ignored.txt"), "alpha\n").expect("write ignored");

    let result = registry
        .invoke(
            "grep",
            r#"{"pattern":"alpha","path":".","output_mode":"files_with_matches","head_limit":0}"#,
            &context,
        )
        .await
        .expect("grep should include hidden files and respect gitignore");

    assert!(result.output.contains("Found 2 files"));
    assert!(result.output.contains(".hidden.txt"));
    assert!(result.output.contains("old.txt"));
    assert!(!result.output.contains("ignored.txt"));
    assert!(!result.output.contains(".git/config"));
    let paths = result
        .output
        .lines()
        .filter(|line| line.ends_with(".txt"))
        .collect::<Vec<_>>();
    assert_eq!(paths, vec![".hidden.txt", "old.txt"]);
}

#[tokio::test]
async fn glob_defaults_to_all_files_when_only_path_is_provided() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("missing-pattern").await;
    std::fs::create_dir_all(context.cwd.join("src")).expect("create src");
    std::fs::write(context.cwd.join("src/lib.rs"), "pub fn demo() {}\n").expect("write lib");

    let result = registry
        .invoke("glob", r#"{"path":"src"}"#, &context)
        .await
        .expect("glob should default to matching files under the provided path");

    assert!(result.output.contains("lib.rs"));
}

#[tokio::test]
async fn glob_defaults_to_workspace_files_when_input_is_empty() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("empty-glob").await;
    std::fs::write(context.cwd.join("README.md"), "demo\n").expect("write readme");

    let result = registry
        .invoke("glob", "", &context)
        .await
        .expect("glob should default to matching workspace files");

    assert!(result.output.contains("README.md"));
}

#[tokio::test]
async fn glob_truncates_large_result_sets() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("glob-truncate").await;
    std::fs::create_dir_all(context.cwd.join("src")).expect("create src");
    for index in 0..105 {
        std::fs::write(
            context.cwd.join("src").join(format!("file-{index}.rs")),
            "pub fn demo() {}\n",
        )
        .expect("write glob fixture");
    }

    let result = registry
        .invoke("glob", r#"{"pattern":"*.rs","path":"src"}"#, &context)
        .await
        .expect("glob should succeed");

    assert!(result.summary.contains("105"));
    assert!(
        result
            .output
            .contains("Results are truncated. Consider using a more specific path or pattern.")
    );
    assert_eq!(
        result
            .output
            .lines()
            .filter(|line| line.ends_with(".rs"))
            .count(),
        100
    );
}

#[tokio::test]
async fn grep_defaults_to_head_limit_for_file_results() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("grep-file-limit").await;
    for index in 0..260 {
        std::fs::write(context.cwd.join(format!("match-{index}.txt")), "alpha\n")
            .expect("write grep fixture");
    }

    let result = registry
        .invoke(
            "grep",
            r#"{"pattern":"alpha","path":".","output_mode":"files_with_matches"}"#,
            &context,
        )
        .await
        .expect("grep should succeed");

    assert!(result.output.contains("Found 250 files limit: 250"));
    assert_eq!(
        result
            .output
            .lines()
            .filter(|line| line.ends_with(".txt"))
            .count(),
        250
    );
}

#[tokio::test]
async fn grep_defaults_to_head_limit_when_unspecified() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("grep-default-limit").await;
    std::fs::write(
        context.cwd.join("many.txt"),
        (0..300)
            .map(|index| format!("alpha-{index}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("write grep fixture");

    let result = registry
        .invoke(
            "grep",
            r#"{"pattern":"alpha","path":"many.txt","output_mode":"content"}"#,
            &context,
        )
        .await
        .expect("grep should succeed");

    assert!(
        result
            .output
            .contains("[Showing results with pagination = limit: 250]")
    );
    assert!(result.output.contains("alpha-0"));
    assert!(!result.output.contains("alpha-299"));
}

#[tokio::test]
async fn glob_falls_back_when_ripgrep_unavailable() {
    let _engine = SearchEngineLock::fallback();
    let registry = ToolRegistry::foundation();
    let context = test_context("glob-fallback").await;
    std::fs::create_dir_all(context.cwd.join("src/inner")).expect("create dirs");
    std::fs::create_dir_all(context.cwd.join(".git")).expect("create vcs dir");
    std::fs::write(context.cwd.join("src/visible.rs"), "pub fn v() {}\n").expect("write visible");
    std::fs::write(context.cwd.join("src/inner/nested.rs"), "pub fn n() {}\n")
        .expect("write nested");
    std::fs::write(context.cwd.join(".git/config"), "ignore me\n").expect("write git config");

    let result = registry
        .invoke("glob", r#"{"pattern":"**/*.rs","path":"."}"#, &context)
        .await
        .expect("glob fallback should succeed");

    let metadata = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("glob"))
        .expect("glob metadata should be present");
    assert_eq!(
        metadata.get("engine").and_then(Value::as_str),
        Some("fallback")
    );
    assert!(metadata.get("diagnostic").is_some());
    assert!(metadata.get("ripgrepVersion").is_none());
    let filenames: Vec<&str> = metadata
        .get("filenames")
        .and_then(Value::as_array)
        .expect("filenames")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(filenames.iter().any(|name| name.ends_with("visible.rs")));
    assert!(filenames.iter().any(|name| name.ends_with("nested.rs")));
    assert!(!filenames.iter().any(|name| name.contains(".git")));
}

#[tokio::test]
async fn grep_falls_back_when_ripgrep_unavailable() {
    let _engine = SearchEngineLock::fallback();
    let registry = ToolRegistry::foundation();
    let context = test_context("grep-fallback").await;
    std::fs::create_dir_all(context.cwd.join(".git")).expect("create vcs dir");
    std::fs::write(context.cwd.join("alpha.txt"), "hello alpha world\n").expect("write alpha");
    std::fs::write(context.cwd.join("bravo.txt"), "no match here\n").expect("write bravo");
    std::fs::write(context.cwd.join(".git/config"), "alpha vcs\n").expect("write git config");

    let result = registry
        .invoke(
            "grep",
            r#"{"pattern":"alpha","path":".","output_mode":"files_with_matches","head_limit":0}"#,
            &context,
        )
        .await
        .expect("grep fallback should succeed");

    assert!(result.output.contains("alpha.txt"));
    assert!(!result.output.contains("bravo.txt"));
    assert!(!result.output.contains(".git/config"));
    assert!(result.output.contains("Grep fallback engaged"));

    let metadata = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("grep"))
        .expect("grep metadata should be present");
    assert_eq!(
        metadata.get("engine").and_then(Value::as_str),
        Some("fallback")
    );
    assert_eq!(
        metadata.get("outputMode").and_then(Value::as_str),
        Some("files_with_matches")
    );
    assert_eq!(metadata.get("numFiles").and_then(Value::as_u64), Some(1));
}

#[tokio::test]
async fn grep_fallback_supports_content_mode_with_line_numbers() {
    let _engine = SearchEngineLock::fallback();
    let registry = ToolRegistry::foundation();
    let context = test_context("grep-fallback-content").await;
    std::fs::write(
        context.cwd.join("notes.txt"),
        "first line\nsecond line ALPHA\nthird line\n",
    )
    .expect("write notes");

    let result = registry
        .invoke(
            "grep",
            r#"{"pattern":"ALPHA","path":"notes.txt","output_mode":"content"}"#,
            &context,
        )
        .await
        .expect("grep fallback should produce content");

    assert!(result.output.contains("notes.txt:2:second line ALPHA"));
}

#[tokio::test]
async fn grep_returns_error_for_missing_path() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("grep-missing-path").await;

    let error = registry
        .invoke(
            "grep",
            r#"{"pattern":"alpha","path":"does/not/exist"}"#,
            &context,
        )
        .await
        .expect_err("grep should fail when path does not exist");

    assert!(error.to_string().contains("Path does not exist"));
}

async fn assert_invalid_regex_rejected(label: &str) {
    let registry = ToolRegistry::foundation();
    let context = test_context(label).await;
    std::fs::write(context.cwd.join("a.txt"), "anything\n").expect("write");

    let error = registry
        .invoke(
            "grep",
            r#"{"pattern":"[unterminated","path":"a.txt","output_mode":"content"}"#,
            &context,
        )
        .await
        .expect_err("invalid regex should be rejected explicitly");

    let message = error.to_string();
    assert!(
        message.contains("Invalid regex pattern"),
        "expected explicit rejection, got: {message}"
    );
    assert!(message.contains("[unterminated"));
}

#[tokio::test]
async fn grep_rejects_invalid_regex_before_engine_selection() {
    {
        let _engine = SearchEngineLock::ripgrep();
        assert_invalid_regex_rejected("grep-invalid-regex-rg").await;
    }
    {
        let _engine = SearchEngineLock::fallback();
        assert_invalid_regex_rejected("grep-invalid-regex-fallback").await;
    }
}

#[tokio::test]
async fn grep_fallback_respects_gitignore() {
    let _engine = SearchEngineLock::fallback();
    let registry = ToolRegistry::foundation();
    let context = test_context("grep-fallback-gitignore").await;
    std::fs::create_dir_all(context.cwd.join(".git")).expect("create git dir");
    std::fs::write(context.cwd.join(".gitignore"), "ignored.txt\n").expect("write gitignore");
    std::fs::write(context.cwd.join("visible.txt"), "alpha\n").expect("write visible");
    std::fs::write(context.cwd.join(".hidden.txt"), "alpha\n").expect("write hidden");
    std::fs::write(context.cwd.join("ignored.txt"), "alpha\n").expect("write ignored");

    let result = registry
        .invoke(
            "grep",
            r#"{"pattern":"alpha","path":".","output_mode":"files_with_matches","head_limit":0}"#,
            &context,
        )
        .await
        .expect("grep fallback should succeed");

    assert!(result.output.contains("visible.txt"));
    assert!(result.output.contains(".hidden.txt"));
    assert!(
        !result.output.contains("ignored.txt"),
        "fallback should honor .gitignore: {}",
        result.output
    );
    let metadata = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("grep"))
        .expect("grep metadata should be present");
    assert_eq!(
        metadata.get("engine").and_then(Value::as_str),
        Some("fallback")
    );
    assert_eq!(metadata.get("numFiles").and_then(Value::as_u64), Some(2));
}

#[tokio::test]
async fn glob_falls_back_when_ripgrep_times_out() {
    let _engine = SearchEngineLock::simulate_rg_timeout();
    let registry = ToolRegistry::foundation();
    let context = test_context("glob-timeout-fallback").await;
    std::fs::create_dir_all(context.cwd.join("src")).expect("create src");
    std::fs::write(context.cwd.join("src/lib.rs"), "pub fn demo() {}\n").expect("write lib");

    let result = registry
        .invoke("glob", r#"{"pattern":"*.rs","path":"src"}"#, &context)
        .await
        .expect("glob should fall back when rg times out");

    assert!(result.output.contains("src/lib.rs"));
    let metadata = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("glob"))
        .expect("glob metadata should be present");
    assert_eq!(
        metadata.get("engine").and_then(Value::as_str),
        Some("fallback")
    );
    assert!(
        metadata
            .get("diagnostic")
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("timed out")),
        "diagnostic should explain the timeout: {metadata:?}"
    );
}

#[tokio::test]
async fn grep_falls_back_when_ripgrep_times_out() {
    let _engine = SearchEngineLock::simulate_rg_timeout();
    let registry = ToolRegistry::foundation();
    let context = test_context("grep-timeout-fallback").await;
    std::fs::write(context.cwd.join("alpha.txt"), "hello alpha\n").expect("write alpha");

    let result = registry
        .invoke(
            "grep",
            r#"{"pattern":"alpha","path":".","output_mode":"files_with_matches","head_limit":0}"#,
            &context,
        )
        .await
        .expect("grep should fall back when rg times out");

    assert!(result.output.contains("alpha.txt"));
    assert!(result.output.contains("Grep fallback engaged"));
    let metadata = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("grep"))
        .expect("grep metadata should be present");
    assert_eq!(
        metadata.get("engine").and_then(Value::as_str),
        Some("fallback")
    );
    assert!(
        metadata
            .get("diagnostic")
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("timed out")),
        "diagnostic should explain the timeout: {metadata:?}"
    );
}

#[tokio::test]
async fn glob_falls_back_when_ripgrep_exits_nonzero() {
    let _engine = SearchEngineLock::simulate_rg_exit(2);
    let registry = ToolRegistry::foundation();
    let context = test_context("glob-exit-fallback").await;
    std::fs::create_dir_all(context.cwd.join("src")).expect("create src");
    std::fs::write(context.cwd.join("src/lib.rs"), "pub fn demo() {}\n").expect("write lib");

    let result = registry
        .invoke("glob", r#"{"pattern":"*.rs","path":"src"}"#, &context)
        .await
        .expect("glob should fall back when rg exits non-1");

    assert!(result.output.contains("src/lib.rs"));
    let metadata = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("glob"))
        .expect("glob metadata should be present");
    assert_eq!(
        metadata.get("engine").and_then(Value::as_str),
        Some("fallback")
    );
    assert!(
        metadata
            .get("diagnostic")
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("exited 2")),
        "diagnostic should report the non-1 exit code: {metadata:?}"
    );
}

#[tokio::test]
async fn grep_falls_back_when_ripgrep_exits_nonzero() {
    let _engine = SearchEngineLock::simulate_rg_exit(2);
    let registry = ToolRegistry::foundation();
    let context = test_context("grep-exit-fallback").await;
    std::fs::write(context.cwd.join("alpha.txt"), "hello alpha\n").expect("write alpha");

    let result = registry
        .invoke(
            "grep",
            r#"{"pattern":"alpha","path":".","output_mode":"files_with_matches","head_limit":0}"#,
            &context,
        )
        .await
        .expect("grep should fall back when rg exits non-1");

    assert!(result.output.contains("alpha.txt"));
    let metadata = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("grep"))
        .expect("grep metadata should be present");
    assert_eq!(
        metadata.get("engine").and_then(Value::as_str),
        Some("fallback")
    );
    assert!(
        metadata
            .get("diagnostic")
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("exited 2")),
        "diagnostic should report the non-1 exit code: {metadata:?}"
    );
}

#[tokio::test]
async fn glob_records_ripgrep_metadata_for_results() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("glob-metadata").await;
    std::fs::create_dir_all(context.cwd.join("src")).expect("create src");
    std::fs::write(context.cwd.join("src/lib.rs"), "pub fn demo() {}\n").expect("write lib");

    let result = registry
        .invoke("glob", r#"{"pattern":"*.rs","path":"src"}"#, &context)
        .await
        .expect("glob should succeed");

    let metadata = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("glob"))
        .expect("glob metadata should be present");
    assert_eq!(
        metadata.get("engine").and_then(Value::as_str),
        Some("ripgrep")
    );
    assert_eq!(
        metadata.get("pattern").and_then(Value::as_str),
        Some("*.rs")
    );
    assert_eq!(metadata.get("path").and_then(Value::as_str), Some("src"));
    assert_eq!(metadata.get("numFiles").and_then(Value::as_u64), Some(1));
    assert_eq!(
        metadata.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    assert!(metadata.get("durationMs").and_then(Value::as_u64).is_some());
    let filenames: Vec<&str> = metadata
        .get("filenames")
        .and_then(Value::as_array)
        .expect("filenames")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(filenames.iter().any(|name| name.ends_with("src/lib.rs")));
}

#[tokio::test]
async fn grep_records_metadata_for_count_mode() {
    let _engine = SearchEngineLock::ripgrep();
    let registry = ToolRegistry::foundation();
    let context = test_context("grep-metadata-count").await;
    std::fs::write(context.cwd.join("a.txt"), "alpha\nalpha line two\nbeta\n").expect("write a");
    std::fs::write(context.cwd.join("b.txt"), "alpha solitary\n").expect("write b");

    let result = registry
        .invoke(
            "grep",
            r#"{"pattern":"alpha","path":".","output_mode":"count"}"#,
            &context,
        )
        .await
        .expect("grep count should succeed");

    let metadata = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("grep"))
        .expect("grep metadata should be present");
    assert_eq!(
        metadata.get("outputMode").and_then(Value::as_str),
        Some("count")
    );
    assert_eq!(metadata.get("numFiles").and_then(Value::as_u64), Some(2));
    assert_eq!(metadata.get("numMatches").and_then(Value::as_u64), Some(3));
}

#[tokio::test]
async fn search_tools_ignore_nullish_optional_paths() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nullish-paths").await;
    std::fs::write(context.cwd.join("README.md"), "alpha\n").expect("write readme");

    let grep = registry
        .invoke(
            "grep",
            r#"{"pattern":"alpha","path":"undefined","glob":"null"}"#,
            &context,
        )
        .await
        .expect("grep should ignore nullish optional paths");
    assert!(grep.output.contains("README.md"));

    let glob = registry
        .invoke("glob", r#"{"path":"."}"#, &context)
        .await
        .expect("glob should still succeed with default pattern");
    assert!(glob.output.contains("README.md"));
}
