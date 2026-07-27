use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use orbcode_config::{managed_memory_file, managed_rules_dir, user_memory_file, user_rules_dir};
use orbcode_protocol::{MemorySource, MemorySourceKind, MemorySourceStatus};

use super::agents::{push_scoped_agent_sources, push_scoped_skill_sources};

const MEMORY_INSTRUCTION_PROMPT: &str = "Codebase and user instructions are shown below. Be sure to adhere to these instructions. IMPORTANT: These instructions OVERRIDE any default behavior and you MUST follow them exactly as written.";

pub(super) async fn load_memory_sources(
    cwd: &Path,
    additional_directories: &[PathBuf],
    home_dir: Option<&Path>,
    trusted_project: Option<bool>,
) -> Vec<MemorySource> {
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    let project_trusted = trusted_project.unwrap_or(true);

    push_memory_file(
        &mut sources,
        &mut seen,
        MemorySourceKind::Managed,
        managed_memory_file(),
        false,
        true,
        Some("managed policy".to_string()),
        None,
        None,
    )
    .await;
    push_rules_dir(
        &mut sources,
        &mut seen,
        MemorySourceKind::Managed,
        managed_rules_dir(),
        false,
        Some("managed policy".to_string()),
        None,
    )
    .await;

    if let Some(home_dir) = home_dir {
        push_memory_file(
            &mut sources,
            &mut seen,
            MemorySourceKind::User,
            user_memory_file(home_dir),
            true,
            true,
            Some("private user".to_string()),
            None,
            None,
        )
        .await;
        push_rules_dir(
            &mut sources,
            &mut seen,
            MemorySourceKind::User,
            user_rules_dir(home_dir),
            true,
            Some("private user".to_string()),
            None,
        )
        .await;
    }

    for dir in project_memory_directories(cwd) {
        push_project_memory_sources(&mut sources, &mut seen, dir, dir == cwd, project_trusted)
            .await;
    }
    for directory in additional_directories {
        push_project_memory_sources(&mut sources, &mut seen, directory, false, project_trusted)
            .await;
    }

    push_scoped_agent_sources(&mut sources, &mut seen, home_dir, cwd, project_trusted).await;
    push_scoped_skill_sources(&mut sources, &mut seen, home_dir, cwd, project_trusted).await;
    sources.push(MemorySource {
        kind: MemorySourceKind::Team,
        label: MemorySourceKind::Team.as_label().to_string(),
        path: None,
        status: MemorySourceStatus::Skipped,
        writable: false,
        trust_boundary: Some("managed/team".to_string()),
        scope: None,
        skipped_reason: Some("team memory is not configured in Orb Code".to_string()),
        content: None,
    });

    sources
}

pub(super) fn memory_source_paths(
    cwd: &Path,
    additional_directories: &[PathBuf],
    home_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    paths.push(managed_memory_file());
    paths.push(managed_rules_dir());
    if let Some(home_dir) = home_dir {
        paths.push(user_memory_file(home_dir));
        paths.push(user_rules_dir(home_dir));
    }
    for dir in project_memory_directories(cwd) {
        paths.push(dir.join("CLAUDE.md"));
        paths.push(dir.join(".claude").join("CLAUDE.md"));
        paths.push(dir.join(".claude").join("rules"));
        paths.push(dir.join("CLAUDE.local.md"));
    }
    for dir in additional_directories {
        paths.push(dir.join("CLAUDE.md"));
        paths.push(dir.join(".claude").join("CLAUDE.md"));
        paths.push(dir.join(".claude").join("rules"));
        paths.push(dir.join("CLAUDE.local.md"));
    }
    if let Some(home_dir) = home_dir {
        paths.push(home_dir.join("agents"));
        paths.push(home_dir.join("skills"));
    }
    paths.push(cwd.join(".claude").join("agents"));
    paths.push(cwd.join(".claude").join("skills"));
    paths
}

pub(super) fn project_memory_directories(cwd: &Path) -> Vec<&Path> {
    let mut directories = cwd
        .ancestors()
        .filter(|path| path.parent().is_some())
        .collect::<Vec<_>>();
    directories.reverse();
    directories
}

async fn push_project_memory_sources(
    sources: &mut Vec<MemorySource>,
    seen: &mut HashSet<PathBuf>,
    dir: &Path,
    include_missing: bool,
    project_trusted: bool,
) {
    let skip = (!project_trusted)
        .then(|| "project memory skipped because project is not trusted".to_string());
    push_memory_file(
        sources,
        seen,
        MemorySourceKind::Project,
        dir.join("CLAUDE.md"),
        true,
        include_missing,
        Some(trust_label(project_trusted)),
        None,
        skip.clone(),
    )
    .await;
    push_memory_file(
        sources,
        seen,
        MemorySourceKind::Project,
        dir.join(".claude").join("CLAUDE.md"),
        true,
        false,
        Some(trust_label(project_trusted)),
        None,
        skip.clone(),
    )
    .await;
    push_rules_dir(
        sources,
        seen,
        MemorySourceKind::Project,
        dir.join(".claude").join("rules"),
        true,
        Some(trust_label(project_trusted)),
        skip.clone(),
    )
    .await;
    push_memory_file(
        sources,
        seen,
        MemorySourceKind::Local,
        dir.join("CLAUDE.local.md"),
        true,
        include_missing,
        Some(trust_label(project_trusted)),
        None,
        skip,
    )
    .await;
}

async fn push_rules_dir(
    sources: &mut Vec<MemorySource>,
    seen: &mut HashSet<PathBuf>,
    kind: MemorySourceKind,
    rules_dir: PathBuf,
    writable: bool,
    trust_boundary: Option<String>,
    skip_reason: Option<String>,
) {
    let Ok(exists) = tokio::fs::try_exists(&rules_dir).await else {
        return;
    };
    if !exists {
        return;
    }
    let Ok(mut entries) = tokio::fs::read_dir(rules_dir).await else {
        return;
    };
    let mut paths = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "md") {
            paths.push(path);
        }
    }
    paths.sort();
    for path in paths {
        push_memory_file(
            sources,
            seen,
            kind,
            path,
            writable,
            false,
            trust_boundary.clone(),
            None,
            skip_reason.clone(),
        )
        .await;
    }
}

/// Cap on how much of a single memory file (`CLAUDE.md`, rules, agent/skill
/// docs) is pulled into a request. Without it, a very large — or deliberately
/// hostile — memory file would balloon every request's token count.
const MAX_MEMORY_FILE_BYTES: u64 = 256 * 1024;

/// Read at most [`MAX_MEMORY_FILE_BYTES`] of `path`, reporting whether the file
/// was longer and got truncated. The read itself is bounded, so an enormous
/// file never lands fully in memory.
async fn read_memory_file_capped(path: &Path) -> std::io::Result<(String, bool)> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path).await?;
    let mut buf = Vec::new();
    // Read one extra byte so we can detect (not just guess) truncation.
    file.take(MAX_MEMORY_FILE_BYTES + 1)
        .read_to_end(&mut buf)
        .await?;
    let truncated = buf.len() as u64 > MAX_MEMORY_FILE_BYTES;
    if truncated {
        buf.truncate(MAX_MEMORY_FILE_BYTES as usize);
    }
    Ok((String::from_utf8_lossy(&buf).into_owned(), truncated))
}

pub(super) async fn push_memory_file(
    sources: &mut Vec<MemorySource>,
    seen: &mut HashSet<PathBuf>,
    kind: MemorySourceKind,
    path: PathBuf,
    writable: bool,
    include_missing: bool,
    trust_boundary: Option<String>,
    scope: Option<String>,
    skip_reason: Option<String>,
) {
    if !seen.insert(memory_path_key(&path).await) {
        return;
    }
    let exists = tokio::fs::try_exists(&path).await.unwrap_or(false);
    if !exists && !include_missing {
        return;
    }

    let (status, content, skipped_reason) = if let Some(reason) = skip_reason {
        if !exists && !include_missing {
            return;
        }
        (MemorySourceStatus::Skipped, None, Some(reason))
    } else if !exists {
        (MemorySourceStatus::Missing, None, None)
    } else {
        match read_memory_file_capped(&path).await {
            Ok((contents, truncated)) => {
                let mut trimmed = contents.trim().to_string();
                if trimmed.is_empty() {
                    (MemorySourceStatus::Empty, None, None)
                } else {
                    if truncated {
                        trimmed.push_str(&format!(
                            "\n\n[memory file truncated: only the first {} KiB are included]",
                            MAX_MEMORY_FILE_BYTES / 1024
                        ));
                    }
                    (MemorySourceStatus::Loaded, Some(trimmed), None)
                }
            }
            Err(error) => (
                MemorySourceStatus::Skipped,
                None,
                Some(format!("failed to read: {error}")),
            ),
        }
    };

    sources.push(MemorySource {
        kind,
        label: kind.as_label().to_string(),
        path: Some(path.display().to_string()),
        status,
        writable,
        trust_boundary,
        scope,
        skipped_reason,
        content,
    });
}

pub(super) fn trust_label(project_trusted: bool) -> String {
    if project_trusted {
        "trusted project".to_string()
    } else {
        "untrusted project".to_string()
    }
}

async fn memory_path_key(path: &Path) -> PathBuf {
    tokio::fs::canonicalize(path)
        .await
        .unwrap_or_else(|_| normalize_path(path))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub(super) fn render_claude_md(sources: &[MemorySource]) -> Option<String> {
    let memories = sources
        .iter()
        .filter(|source| {
            matches!(
                source.kind,
                MemorySourceKind::Managed
                    | MemorySourceKind::User
                    | MemorySourceKind::Project
                    | MemorySourceKind::Local
                    | MemorySourceKind::Team
            ) && source.status == MemorySourceStatus::Loaded
        })
        .filter_map(|source| {
            let path = source.path.as_deref()?;
            let content = source.content.as_deref()?;
            let description = match source.kind {
                MemorySourceKind::Project => " (project instructions, checked into the codebase)",
                MemorySourceKind::Local => " (user's private project instructions, not checked in)",
                MemorySourceKind::Team => " (shared team memory, synced across the organization)",
                MemorySourceKind::Managed => " (managed global instructions for all users)",
                MemorySourceKind::User => " (user's private global instructions for all projects)",
                _ => return None,
            };
            Some(format!("Contents of {path}{description}:\n\n{content}"))
        })
        .collect::<Vec<_>>();
    if memories.is_empty() {
        None
    } else {
        Some(format!(
            "{MEMORY_INSTRUCTION_PROMPT}\n\n{}",
            memories.join("\n\n")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_memory_file_capped_truncates_oversized_file() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let file = dir.path().join("CLAUDE.md");
        let big = "x".repeat((MAX_MEMORY_FILE_BYTES as usize) + 4096);
        tokio::fs::write(&file, &big).await.expect("write fixture");

        let (contents, truncated) = read_memory_file_capped(&file).await.expect("read");
        assert!(truncated, "an oversized memory file must report truncation");
        assert_eq!(
            contents.len(),
            MAX_MEMORY_FILE_BYTES as usize,
            "content must be capped at the byte limit"
        );
    }

    #[tokio::test]
    async fn read_memory_file_capped_keeps_small_file_intact() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let file = dir.path().join("CLAUDE.md");
        tokio::fs::write(&file, "small memory file")
            .await
            .expect("write fixture");
        let (contents, truncated) = read_memory_file_capped(&file).await.expect("read");
        assert!(!truncated);
        assert_eq!(contents, "small memory file");
    }

    #[tokio::test]
    async fn memory_path_key_deduplicates_via_canonical_resolution() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let file = dir.path().join("CLAUDE.md");
        tokio::fs::write(&file, "instructions")
            .await
            .expect("write fixture");

        let symlink = dir.path().join("link.md");
        #[cfg(unix)]
        tokio::fs::symlink(&file, &symlink)
            .await
            .expect("create symlink");
        #[cfg(not(unix))]
        {
            let _ = symlink;
            return;
        }

        let key_real = memory_path_key(&file).await;
        let key_link = memory_path_key(&symlink).await;
        assert_eq!(
            key_real, key_link,
            "symlink and real path must resolve to the same dedupe key"
        );
    }

    #[tokio::test]
    async fn memory_path_key_falls_back_to_normalize_for_missing_files() {
        let missing = Path::new("/nonexistent/project/CLAUDE.md");
        let key = memory_path_key(missing).await;
        assert_eq!(
            key,
            PathBuf::from("/nonexistent/project/CLAUDE.md"),
            "missing path should fall back to normalize_path"
        );
    }

    #[test]
    fn project_memory_directories_skip_filesystem_root() {
        let cwd = std::path::Path::new("/workspace/project/nested");

        let directories = project_memory_directories(cwd)
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            directories,
            vec![
                "/workspace".to_string(),
                "/workspace/project".to_string(),
                "/workspace/project/nested".to_string(),
            ]
        );
    }
}
