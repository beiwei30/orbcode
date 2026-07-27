use std::path::{Path, PathBuf};

use orbcode_protocol::WorktreeState;
use tokio::process::Command;

pub(super) const MAX_STATUS_CHARS: usize = 1000;
pub(super) const STATUS_TRUNCATION_SUFFIX: &str = "\n... (truncated because it exceeds 1k characters. Run `git status` via bash if you need more.)";
pub(super) const RECENT_COMMIT_COUNT: &str = "5";

pub(super) async fn git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

pub(super) fn truncate_status(status: &str) -> String {
    if status.chars().count() <= MAX_STATUS_CHARS {
        return status.to_string();
    }
    let mut truncated: String = status.chars().take(MAX_STATUS_CHARS).collect();
    truncated.push_str(STATUS_TRUNCATION_SUFFIX);
    truncated
}

pub(super) fn sanitize_git_remote(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            return format_remote_components(host, path);
        }
        return None;
    }

    let scheme_split = trimmed.split_once("://")?;
    let scheme = scheme_split.0.to_ascii_lowercase();
    let after_scheme = scheme_split.1;
    if !matches!(scheme.as_str(), "https" | "http" | "ssh" | "git") {
        return None;
    }
    let without_userinfo = match after_scheme.split_once('@') {
        Some((_, rest)) => rest,
        None => after_scheme,
    };
    let (host, path) = without_userinfo.split_once('/')?;
    format_remote_components(host, path)
}

fn format_remote_components(host: &str, path: &str) -> Option<String> {
    let host = host.split_once(':').map_or(host, |(h, _)| h).trim();
    if host.is_empty() {
        return None;
    }
    let mut cleaned_path = path.trim_start_matches('/');
    if let Some(stripped) = cleaned_path.strip_suffix(".git") {
        cleaned_path = stripped;
    }
    let cleaned_path = cleaned_path.trim_end_matches('/');
    if cleaned_path.is_empty() {
        return None;
    }
    if !cleaned_path.contains('/') {
        return None;
    }
    Some(format!("{host}/{cleaned_path}").to_ascii_lowercase())
}

pub(super) async fn resolve_repo_context(cwd: &Path, repo_root: &str) -> (String, Option<String>) {
    let repo_root_path = Path::new(repo_root);
    if let Ok(relative) = cwd.strip_prefix(repo_root_path) {
        return (
            repo_root.to_string(),
            normalize_relative_repo_subdir(relative),
        );
    }

    let canonical_cwd = tokio::fs::canonicalize(cwd).await.ok();
    let canonical_repo_root = tokio::fs::canonicalize(repo_root_path).await.ok();
    if let (Some(canonical_cwd), Some(canonical_repo_root)) =
        (canonical_cwd.as_deref(), canonical_repo_root.as_deref())
        && let Ok(relative) = canonical_cwd.strip_prefix(canonical_repo_root)
    {
        let relative_display = normalize_relative_repo_subdir(relative);
        let display_repo_root =
            derive_repo_root_display(cwd, relative).unwrap_or_else(|| repo_root.to_string());
        return (display_repo_root, relative_display);
    }

    (repo_root.to_string(), None)
}

fn normalize_relative_repo_subdir(relative: &Path) -> Option<String> {
    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative.display().to_string())
    }
}

fn derive_repo_root_display(cwd: &Path, relative: &Path) -> Option<String> {
    let mut display_root = cwd.to_path_buf();
    for _ in 0..relative.components().count() {
        if !display_root.pop() {
            return None;
        }
    }
    Some(display_root.display().to_string())
}

pub(super) fn classify_worktree(cwd: &Path, raw_git_dir: &str, detached: bool) -> WorktreeState {
    if detached {
        return WorktreeState::Detached;
    }
    let git_dir_path = if Path::new(raw_git_dir).is_absolute() {
        PathBuf::from(raw_git_dir)
    } else {
        cwd.join(raw_git_dir)
    };
    if git_dir_path
        .components()
        .any(|component| component.as_os_str() == "worktrees")
    {
        WorktreeState::Linked
    } else {
        WorktreeState::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_git_remote_normalizes_ssh_form() {
        assert_eq!(
            sanitize_git_remote("git@github.com:owner/repo.git").as_deref(),
            Some("github.com/owner/repo")
        );
    }

    #[test]
    fn sanitize_git_remote_strips_credentials_and_dot_git() {
        assert_eq!(
            sanitize_git_remote("https://user:token@github.com/Owner/Repo.git").as_deref(),
            Some("github.com/owner/repo")
        );
    }

    #[test]
    fn sanitize_git_remote_handles_ssh_url() {
        assert_eq!(
            sanitize_git_remote("ssh://git@github.com/owner/repo").as_deref(),
            Some("github.com/owner/repo")
        );
    }

    #[test]
    fn sanitize_git_remote_rejects_file_and_bare_paths() {
        assert!(sanitize_git_remote("file:///tmp/repo").is_none());
        assert!(sanitize_git_remote("/srv/git/repo").is_none());
        assert!(sanitize_git_remote("").is_none());
    }

    #[test]
    fn truncate_status_caps_and_appends_tail() {
        let long = "x".repeat(MAX_STATUS_CHARS * 2);
        let truncated = truncate_status(&long);
        assert!(truncated.len() <= MAX_STATUS_CHARS * 4 + STATUS_TRUNCATION_SUFFIX.len());
        assert!(truncated.ends_with(STATUS_TRUNCATION_SUFFIX));
        assert!(truncated.starts_with("xxxx"));
    }

    #[test]
    fn truncate_status_passthrough_short() {
        let short = " M file.rs\n?? other.rs";
        assert_eq!(truncate_status(short), short);
    }
}
