//! Golden tests for search tool behavior: gitignore parity between grep
//! and glob, and hidden-file (dotfile) inclusion.

mod common;

use orbcode_tools::ToolRegistry;

fn registry() -> ToolRegistry {
    ToolRegistry::foundation()
}

/// Set up a minimal git repo with a `.gitignore` that excludes `ignored.rs`,
/// a hidden file `.hidden.rs`, and a normal file `visible.rs`.
fn seed_search_fixture(cwd: &std::path::Path) {
    common::init_git_repo(cwd);

    std::fs::write(cwd.join(".gitignore"), "ignored.rs\n").expect("write gitignore");
    std::fs::write(cwd.join("visible.rs"), "fn visible() {}\n").expect("write visible");
    std::fs::write(cwd.join(".hidden.rs"), "fn hidden() {}\n").expect("write hidden");
    std::fs::write(cwd.join("ignored.rs"), "fn ignored() {}\n").expect("write ignored");

    // Stage all files so git is aware of the repo structure.
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(cwd)
        .output()
        .expect("git add");
}

#[tokio::test]
async fn grep_excludes_gitignored_files() {
    let reg = registry();
    let ctx = common::test_context("grep-gitignore").await;
    seed_search_fixture(&ctx.cwd);

    let result = reg
        .invoke(
            "grep",
            &format!(
                r#"{{"pattern":"fn","path":"{}","include":"*.rs"}}"#,
                ctx.cwd.display()
            ),
            &ctx,
        )
        .await
        .expect("grep should succeed");

    assert!(
        result.output.contains("visible.rs"),
        "grep should include visible.rs"
    );
    assert!(
        result.output.contains(".hidden.rs"),
        "grep should include .hidden.rs (dotfiles are shown)"
    );
    assert!(
        !result.output.contains("ignored.rs"),
        "grep should exclude gitignored files, but output contained ignored.rs:\n{}",
        result.output
    );
}

#[tokio::test]
async fn glob_includes_gitignored_files() {
    let reg = registry();
    let ctx = common::test_context("glob-gitignore").await;
    seed_search_fixture(&ctx.cwd);

    let result = reg
        .invoke(
            "glob",
            &format!(r#"{{"pattern":"*.rs","path":"{}"}}"#, ctx.cwd.display()),
            &ctx,
        )
        .await
        .expect("glob should succeed");

    assert!(
        result.output.contains("visible.rs"),
        "glob should include visible.rs"
    );
    assert!(
        result.output.contains("ignored.rs"),
        "glob should include gitignored files (--no-ignore), output:\n{}",
        result.output
    );
}

#[tokio::test]
async fn grep_includes_hidden_dotfiles() {
    let reg = registry();
    let ctx = common::test_context("grep-hidden").await;
    seed_search_fixture(&ctx.cwd);

    let result = reg
        .invoke(
            "grep",
            &format!(
                r#"{{"pattern":"fn","path":"{}","include":"*.rs"}}"#,
                ctx.cwd.display()
            ),
            &ctx,
        )
        .await
        .expect("grep should succeed");

    assert!(
        result.output.contains(".hidden.rs"),
        "grep should show hidden/dotfiles, output:\n{}",
        result.output
    );
}

#[tokio::test]
async fn glob_includes_hidden_dotfiles() {
    let reg = registry();
    let ctx = common::test_context("glob-hidden").await;
    seed_search_fixture(&ctx.cwd);

    let result = reg
        .invoke(
            "glob",
            &format!(r#"{{"pattern":"*.rs","path":"{}"}}"#, ctx.cwd.display()),
            &ctx,
        )
        .await
        .expect("glob should succeed");

    assert!(
        result.output.contains(".hidden.rs"),
        "glob should show hidden/dotfiles, output:\n{}",
        result.output
    );
}

#[tokio::test]
async fn grep_metadata_contains_match_count_engine_duration() {
    let reg = registry();
    let ctx = common::test_context("grep-metadata-golden").await;
    seed_search_fixture(&ctx.cwd);

    let result = reg
        .invoke(
            "grep",
            &format!(r#"{{"pattern":"fn","path":"{}"}}"#, ctx.cwd.display()),
            &ctx,
        )
        .await
        .expect("grep should succeed");

    let meta = result
        .metadata
        .as_ref()
        .expect("grep metadata should be present");
    let grep = meta.get("grep").expect("nested grep block should exist");

    assert!(
        grep.get("matchCount").is_some(),
        "matchCount should be present in grep metadata"
    );
    assert!(
        grep["matchCount"].as_u64().unwrap() > 0,
        "matchCount should be positive for matching files"
    );
    assert!(
        grep.get("engine").is_some(),
        "engine should be present in grep metadata"
    );
    let engine = grep["engine"].as_str().unwrap();
    assert!(
        engine == "ripgrep" || engine == "fallback",
        "engine should be ripgrep or fallback, got: {engine}"
    );
    assert!(
        grep.get("durationMs").is_some(),
        "durationMs should be present in grep metadata"
    );
    assert!(
        meta.get("durationMs").is_some(),
        "top-level durationMs should be present"
    );
}

#[tokio::test]
async fn glob_metadata_contains_match_count_engine_duration() {
    let reg = registry();
    let ctx = common::test_context("glob-metadata-golden").await;
    seed_search_fixture(&ctx.cwd);

    let result = reg
        .invoke(
            "glob",
            &format!(r#"{{"pattern":"*.rs","path":"{}"}}"#, ctx.cwd.display()),
            &ctx,
        )
        .await
        .expect("glob should succeed");

    let meta = result
        .metadata
        .as_ref()
        .expect("glob metadata should be present");
    let glob = meta.get("glob").expect("nested glob block should exist");

    assert!(
        glob.get("matchCount").is_some(),
        "matchCount should be present in glob metadata"
    );
    assert!(
        glob["matchCount"].as_u64().unwrap() > 0,
        "matchCount should be positive for matching files"
    );
    assert!(
        glob.get("engine").is_some(),
        "engine should be present in glob metadata"
    );
    let engine = glob["engine"].as_str().unwrap();
    assert!(
        engine == "ripgrep" || engine == "fallback",
        "engine should be ripgrep or fallback, got: {engine}"
    );
    assert!(
        glob.get("durationMs").is_some(),
        "durationMs should be present in glob metadata"
    );
    assert!(
        meta.get("durationMs").is_some(),
        "top-level durationMs should be present"
    );
}
