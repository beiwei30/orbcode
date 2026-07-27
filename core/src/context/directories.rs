use std::path::PathBuf;

use orbcode_protocol::AdditionalDirectoryInfo;

use super::git::git_output;

pub(super) async fn collect_additional_directory_details(
    additional_directories: &[PathBuf],
) -> Vec<AdditionalDirectoryInfo> {
    let mut details = Vec::with_capacity(additional_directories.len());
    for dir in additional_directories {
        let path_display = dir.display().to_string();
        let claude_md_path = dir.join("CLAUDE.md");
        let has_claude_md = tokio::fs::try_exists(&claude_md_path)
            .await
            .unwrap_or(false);
        let (repo_root, branch) = tokio::join!(
            git_output(dir, &["rev-parse", "--show-toplevel"]),
            git_output(dir, &["--no-optional-locks", "branch", "--show-current"]),
        );
        details.push(AdditionalDirectoryInfo {
            path: path_display,
            has_claude_md,
            git_branch: branch,
            repo_root,
        });
    }
    details
}
