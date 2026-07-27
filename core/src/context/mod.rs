mod agents;
mod directories;
pub(crate) mod fingerprint;
mod git;
mod memory;

use std::path::{Path, PathBuf};

use chrono::Utc;
use orbcode_protocol::TurnContext;

use directories::collect_additional_directory_details;
use git::{
    RECENT_COMMIT_COUNT, classify_worktree, git_output, resolve_repo_context, sanitize_git_remote,
    truncate_status,
};
use memory::{load_memory_sources, render_claude_md};

#[cfg(test)]
pub async fn build_turn_context(cwd: &Path) -> TurnContext {
    build_turn_context_with_additional_dirs(cwd, &[]).await
}

#[cfg(test)]
pub async fn build_turn_context_with_additional_dirs(
    cwd: &Path,
    additional_directories: &[PathBuf],
) -> TurnContext {
    build_turn_context_with_memory_options(cwd, additional_directories, None, None).await
}

pub async fn build_turn_context_with_memory_home(
    cwd: &Path,
    additional_directories: &[PathBuf],
    home_dir: &Path,
) -> TurnContext {
    build_turn_context_with_memory_options(cwd, additional_directories, Some(home_dir), None).await
}

pub async fn build_turn_context_with_memory_options(
    cwd: &Path,
    additional_directories: &[PathBuf],
    home_dir: Option<&Path>,
    trusted_project: Option<bool>,
) -> TurnContext {
    let cwd_display = cwd.display().to_string();
    let additional_directories_display = additional_directories
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let current_date = Utc::now().date_naive().to_string();

    let (raw_repo_root, memory_sources) = tokio::join!(
        git_output(cwd, &["rev-parse", "--show-toplevel"]),
        load_memory_sources(cwd, additional_directories, home_dir, trusted_project)
    );
    let claude_md = render_claude_md(&memory_sources);

    let (repo_root, cwd_relative_to_repo) = match raw_repo_root.as_deref() {
        Some(repo_root) => {
            let (root, relative) = resolve_repo_context(cwd, repo_root).await;
            (Some(root), relative)
        }
        None => (None, None),
    };

    let mut git_branch = None;
    let mut git_status = None;
    let mut git_default_branch = None;
    let mut git_user = None;
    let mut git_recent_commits = None;
    let mut git_remote = None;
    let mut git_worktree_state = None;

    if repo_root.is_some() {
        let (branch, status, default_branch, user, commits, remote, worktree) = tokio::join!(
            git_output(cwd, &["--no-optional-locks", "branch", "--show-current"]),
            async {
                git_output(cwd, &["--no-optional-locks", "status", "--short"])
                    .await
                    .map(|status| truncate_status(&status))
            },
            git_output(
                cwd,
                &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
            ),
            git_output(cwd, &["config", "user.name"]),
            git_output(
                cwd,
                &[
                    "--no-optional-locks",
                    "log",
                    "--oneline",
                    "--no-decorate",
                    "-n",
                    RECENT_COMMIT_COUNT,
                ],
            ),
            async {
                git_output(cwd, &["config", "--get", "remote.origin.url"])
                    .await
                    .and_then(|raw| sanitize_git_remote(&raw))
            },
            git_output(cwd, &["rev-parse", "--git-dir"]),
        );

        git_branch = branch.clone();
        git_status = status;
        git_default_branch = default_branch.and_then(|symbolic| {
            symbolic
                .split_once('/')
                .map(|(_, branch)| branch.to_string())
                .filter(|s| !s.is_empty())
        });
        git_user = user;
        git_recent_commits = commits;
        git_remote = remote;
        git_worktree_state =
            worktree.map(|raw_git_dir| classify_worktree(cwd, &raw_git_dir, branch.is_none()));
    }

    let additional_directory_details =
        collect_additional_directory_details(additional_directories).await;

    TurnContext {
        cwd: cwd_display,
        additional_directories: additional_directories_display,
        additional_directory_details,
        repo_root,
        cwd_relative_to_repo,
        current_date,
        git_branch,
        git_default_branch,
        git_user,
        git_status,
        git_recent_commits,
        git_remote,
        git_worktree_state,
        trusted_project,
        memory_sources,
        claude_md,
    }
}

#[cfg(test)]
mod tests {
    use orbcode_protocol::{MemorySourceKind, MemorySourceStatus, WorktreeState};
    use tempfile::tempdir;
    use tokio::process::Command;

    use super::{
        build_turn_context, build_turn_context_with_additional_dirs,
        build_turn_context_with_memory_home, build_turn_context_with_memory_options,
    };

    async fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .await
            .expect("git command spawn");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    async fn init_repo(path: &std::path::Path) {
        tokio::fs::create_dir_all(path).await.expect("mkdir");
        run_git(path, &["init", "-q", "-b", "main"]).await;
        run_git(path, &["config", "user.email", "ci@example.com"]).await;
        run_git(path, &["config", "user.name", "CI Tester"]).await;
        run_git(path, &["config", "commit.gpgsign", "false"]).await;
    }

    #[tokio::test]
    async fn loads_claude_md_from_parent_chain() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let nested = root.join("a/b");
        tokio::fs::create_dir_all(&nested)
            .await
            .expect("nested dir");
        tokio::fs::write(root.join("CLAUDE.md"), "root instructions")
            .await
            .expect("root claude md");
        tokio::fs::write(root.join("a").join("CLAUDE.md"), "nested instructions")
            .await
            .expect("child claude md");

        let context = build_turn_context(nested.as_path()).await;

        let claude_md = context.claude_md.expect("claude md context");
        assert!(claude_md.contains("root instructions"));
        assert!(claude_md.contains("nested instructions"));
        assert!(claude_md.contains("Contents of "));
    }

    #[tokio::test]
    async fn captures_repo_root_and_nested_relative_cwd() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path();
        let nested = root.join("orbcode/src");
        tokio::fs::create_dir_all(&nested)
            .await
            .expect("nested dir");

        let output = Command::new("git")
            .arg("init")
            .arg(root)
            .output()
            .await
            .expect("git init");
        assert!(
            output.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let context = build_turn_context(nested.as_path()).await;

        assert_eq!(
            context.repo_root.as_deref(),
            Some(root.to_str().expect("root path"))
        );
        assert_eq!(context.cwd_relative_to_repo.as_deref(), Some("orbcode/src"));
    }

    #[tokio::test]
    async fn includes_additional_working_directories_and_instructions() {
        let temp = tempdir().expect("tempdir");
        let cwd = temp.path().join("workspace");
        let extra = temp.path().join("extra");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::create_dir_all(&extra).await.expect("extra");
        tokio::fs::write(extra.join("CLAUDE.md"), "extra instructions")
            .await
            .expect("extra claude md");

        let context =
            build_turn_context_with_additional_dirs(&cwd, std::slice::from_ref(&extra)).await;

        assert_eq!(
            context.additional_directories,
            vec![extra.display().to_string()]
        );
        let claude_md = context.claude_md.expect("claude md");
        assert!(claude_md.contains("Contents of "));
        assert!(claude_md.contains("extra instructions"));
        assert_eq!(context.additional_directory_details.len(), 1);
        let detail = &context.additional_directory_details[0];
        assert_eq!(detail.path, extra.display().to_string());
        assert!(detail.has_claude_md);
        assert!(detail.git_branch.is_none());
        assert!(detail.repo_root.is_none());
    }

    #[tokio::test]
    async fn memory_sources_follow_typescript_order_and_local_priority() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let root = temp.path().join("repo");
        let nested = root.join("pkg");
        tokio::fs::create_dir_all(home.join("rules"))
            .await
            .expect("home rules");
        tokio::fs::create_dir_all(nested.join(".claude").join("rules"))
            .await
            .expect("project rules");
        tokio::fs::write(home.join("CLAUDE.md"), "user")
            .await
            .expect("user");
        tokio::fs::write(home.join("rules").join("a.md"), "user rule")
            .await
            .expect("user rule");
        tokio::fs::write(root.join("CLAUDE.md"), "root project")
            .await
            .expect("root project");
        tokio::fs::write(root.join("CLAUDE.local.md"), "root local")
            .await
            .expect("root local");
        tokio::fs::write(nested.join("CLAUDE.md"), "nested project")
            .await
            .expect("nested project");
        tokio::fs::write(nested.join(".claude").join("CLAUDE.md"), "dot project")
            .await
            .expect("dot project");
        tokio::fs::write(
            nested.join(".claude").join("rules").join("z.md"),
            "project rule",
        )
        .await
        .expect("project rule");
        tokio::fs::write(nested.join("CLAUDE.local.md"), "nested local")
            .await
            .expect("nested local");

        let context = build_turn_context_with_memory_home(&nested, &[], &home).await;
        let loaded = context
            .memory_sources
            .iter()
            .filter(|source| source.status == MemorySourceStatus::Loaded)
            .filter(|source| {
                matches!(
                    source.kind,
                    MemorySourceKind::User | MemorySourceKind::Project | MemorySourceKind::Local
                )
            })
            .map(|source| {
                (
                    source.kind,
                    source
                        .path
                        .as_deref()
                        .and_then(|path| std::path::Path::new(path).file_name())
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_string(),
                    source.content.clone().unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            loaded,
            vec![
                (
                    MemorySourceKind::User,
                    "CLAUDE.md".to_string(),
                    "user".to_string()
                ),
                (
                    MemorySourceKind::User,
                    "a.md".to_string(),
                    "user rule".to_string()
                ),
                (
                    MemorySourceKind::Project,
                    "CLAUDE.md".to_string(),
                    "root project".to_string()
                ),
                (
                    MemorySourceKind::Local,
                    "CLAUDE.local.md".to_string(),
                    "root local".to_string()
                ),
                (
                    MemorySourceKind::Project,
                    "CLAUDE.md".to_string(),
                    "nested project".to_string()
                ),
                (
                    MemorySourceKind::Project,
                    "CLAUDE.md".to_string(),
                    "dot project".to_string()
                ),
                (
                    MemorySourceKind::Project,
                    "z.md".to_string(),
                    "project rule".to_string()
                ),
                (
                    MemorySourceKind::Local,
                    "CLAUDE.local.md".to_string(),
                    "nested local".to_string()
                ),
            ]
        );

        let claude_md = context.claude_md.expect("claude md");
        assert!(
            claude_md.find("root project").expect("root project")
                < claude_md.find("nested local").expect("nested local")
        );
    }

    #[tokio::test]
    async fn untrusted_project_skips_project_local_agent_and_skill_memory() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("repo");
        tokio::fs::create_dir_all(home.join("agents"))
            .await
            .expect("home agents");
        tokio::fs::create_dir_all(cwd.join(".claude").join("agents"))
            .await
            .expect("project agents");
        tokio::fs::create_dir_all(cwd.join(".claude").join("skills").join("rust"))
            .await
            .expect("project skills");
        tokio::fs::write(home.join("CLAUDE.md"), "user")
            .await
            .expect("user");
        tokio::fs::write(cwd.join("CLAUDE.md"), "PROJECT_SECRET_MARKER")
            .await
            .expect("project");
        tokio::fs::write(cwd.join("CLAUDE.local.md"), "LOCAL_SECRET_MARKER")
            .await
            .expect("local");
        tokio::fs::write(
            cwd.join(".claude").join("agents").join("review.md"),
            "---\nname: review\ndescription: review\n---\nagent prompt",
        )
        .await
        .expect("agent");
        tokio::fs::write(
            cwd.join(".claude")
                .join("skills")
                .join("rust")
                .join("SKILL.md"),
            "---\nname: rust\n---\nskill body",
        )
        .await
        .expect("skill");

        let context =
            build_turn_context_with_memory_options(&cwd, &[], Some(&home), Some(false)).await;

        assert!(
            context
                .memory_sources
                .iter()
                .any(|source| source.kind == MemorySourceKind::User
                    && source.status == MemorySourceStatus::Loaded)
        );
        assert!(
            context
                .memory_sources
                .iter()
                .filter(|source| matches!(
                    source.kind,
                    MemorySourceKind::Project
                        | MemorySourceKind::Local
                        | MemorySourceKind::Agent
                        | MemorySourceKind::Skill
                ))
                .all(|source| source.status == MemorySourceStatus::Skipped),
            "{:?}",
            context.memory_sources
        );
        let claude_md = context.claude_md.unwrap_or_default();
        assert!(!claude_md.contains("PROJECT_SECRET_MARKER"));
        assert!(!claude_md.contains("LOCAL_SECRET_MARKER"));
    }

    #[tokio::test]
    async fn captures_default_branch_user_and_recent_commits() {
        let temp = tempdir().expect("tempdir");
        let nested = temp.path().join("crate/src");
        init_repo(temp.path()).await;
        tokio::fs::create_dir_all(&nested).await.expect("nested");
        tokio::fs::write(temp.path().join("README.md"), "hello")
            .await
            .expect("write readme");
        run_git(temp.path(), &["add", "README.md"]).await;
        run_git(temp.path(), &["commit", "-q", "-m", "first commit"]).await;
        tokio::fs::write(temp.path().join("README.md"), "hello world")
            .await
            .expect("update readme");
        run_git(temp.path(), &["commit", "-aq", "-m", "second commit"]).await;
        run_git(
            temp.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://example.com/owner/repo.git",
            ],
        )
        .await;
        run_git(
            temp.path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        )
        .await;

        let context = build_turn_context(nested.as_path()).await;

        assert_eq!(context.git_branch.as_deref(), Some("main"));
        assert_eq!(context.git_default_branch.as_deref(), Some("main"));
        assert_eq!(
            context.git_remote.as_deref(),
            Some("example.com/owner/repo")
        );
        assert_eq!(context.git_user.as_deref(), Some("CI Tester"));
        let commits = context
            .git_recent_commits
            .expect("recent commits populated");
        assert!(commits.contains("first commit"));
        assert!(commits.contains("second commit"));
        assert!(matches!(
            context.git_worktree_state,
            Some(WorktreeState::Normal)
        ));
    }

    #[tokio::test]
    async fn non_git_directory_has_no_git_fields() {
        let temp = tempdir().expect("tempdir");
        let context = build_turn_context(temp.path()).await;
        assert!(context.git_branch.is_none());
        assert!(context.git_default_branch.is_none());
        assert!(context.git_user.is_none());
        assert!(context.git_status.is_none());
        assert!(context.git_recent_commits.is_none());
        assert!(context.git_remote.is_none());
        assert!(context.git_worktree_state.is_none());
        assert!(context.repo_root.is_none());
    }

    #[tokio::test]
    async fn detached_head_marks_worktree_state() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path()).await;
        tokio::fs::write(temp.path().join("file.txt"), "a")
            .await
            .expect("write");
        run_git(temp.path(), &["add", "file.txt"]).await;
        run_git(temp.path(), &["commit", "-q", "-m", "first"]).await;
        tokio::fs::write(temp.path().join("file.txt"), "b")
            .await
            .expect("write");
        run_git(temp.path(), &["commit", "-aq", "-m", "second"]).await;
        run_git(temp.path(), &["checkout", "-q", "HEAD~1"]).await;

        let context = build_turn_context(temp.path()).await;

        assert!(context.git_branch.is_none());
        assert!(matches!(
            context.git_worktree_state,
            Some(WorktreeState::Detached)
        ));
    }

    #[tokio::test]
    async fn additional_directory_details_capture_git_branch_for_separate_repo() {
        let temp = tempdir().expect("tempdir");
        let cwd = temp.path().join("primary");
        let extra = temp.path().join("extra");
        init_repo(&cwd).await;
        init_repo(&extra).await;
        tokio::fs::write(extra.join("note.txt"), "n")
            .await
            .expect("write note");
        run_git(&extra, &["add", "note.txt"]).await;
        run_git(&extra, &["commit", "-q", "-m", "extra commit"]).await;

        let context =
            build_turn_context_with_additional_dirs(&cwd, std::slice::from_ref(&extra)).await;

        assert_eq!(context.additional_directory_details.len(), 1);
        let detail = &context.additional_directory_details[0];
        assert_eq!(detail.path, extra.display().to_string());
        assert_eq!(detail.git_branch.as_deref(), Some("main"));
        assert!(detail.repo_root.is_some());
    }
}
