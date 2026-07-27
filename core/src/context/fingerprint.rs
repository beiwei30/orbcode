use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use orbcode_protocol::TurnContext;

use super::git::git_output;
use super::memory::memory_source_paths;

/// Filesystem mtime granularity on HFS+/APFS is 1 second. If the cache was
/// populated less than this long ago, we rebuild unconditionally to avoid
/// serving stale context when a tool round modified a CLAUDE.md or skill file
/// within the same mtime bucket.
const MIN_CACHE_AGE: std::time::Duration = std::time::Duration::from_secs(2);

/// Upper bound on how many files we stat when expanding a memory *directory*
/// (`.claude/rules`, `agents`, `skills`) into per-file mtimes. Bounds the cost
/// of the fingerprint on pathologically large memory trees.
const MAX_MEMORY_DIR_FILES: usize = 512;

#[derive(Debug)]
pub(crate) struct TurnContextCache {
    fingerprint: Fingerprint,
    stored_at: Instant,
    pub(crate) context: TurnContext,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Fingerprint {
    date: String,
    git_head: Option<String>,
    /// Current branch name (`git branch --show-current`). `git_head` alone
    /// misses a branch switch that lands on the same commit (e.g. two branches
    /// pointing at the same HEAD), which would reuse a cached context carrying a
    /// stale `git_branch`.
    git_branch: Option<String>,
    /// Hash of `git status --porcelain`. `git_head` alone misses uncommitted
    /// working-tree edits (staged or unstaged) that change `TurnContext`'s
    /// `git_status`; without this a tool round that edits tracked files without
    /// committing would reuse a cached context carrying a stale `git_status`.
    git_status: Option<u64>,
    file_mtimes: Vec<(PathBuf, Option<SystemTime>)>,
}

pub(crate) async fn compute_fingerprint(
    cwd: &Path,
    additional_directories: &[PathBuf],
    home_dir: Option<&Path>,
) -> Fingerprint {
    let date = chrono::Utc::now().date_naive().to_string();
    let git_head = git_output(cwd, &["rev-parse", "HEAD"]).await;
    let git_branch = git_output(cwd, &["--no-optional-locks", "branch", "--show-current"]).await;
    let git_status = git_output(cwd, &["--no-optional-locks", "status", "--porcelain"])
        .await
        .map(|status| hash_str(&status));

    let paths = memory_source_paths(cwd, additional_directories, home_dir);
    let mut file_mtimes = Vec::with_capacity(paths.len());
    for path in paths {
        collect_path_mtimes(&path, &mut file_mtimes).await;
    }

    Fingerprint {
        date,
        git_head,
        git_branch,
        git_status,
        file_mtimes,
    }
}

fn hash_str(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Records `(path, mtime)` entries for a memory source path.
///
/// A plain file contributes its own mtime. A **directory** (e.g.
/// `.claude/rules`, `agents`, `skills`) is expanded into the mtimes of the
/// files it contains (bounded, recursive), plus the directory node's own
/// mtime. Stating only the directory node would miss edits to the *contents*
/// of a file inside it, because a content edit does not change the parent
/// directory's mtime.
async fn collect_path_mtimes(path: &Path, out: &mut Vec<(PathBuf, Option<SystemTime>)>) {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        // Record the missing path so a later creation invalidates the cache.
        out.push((path.to_path_buf(), None));
        return;
    };
    if !metadata.is_dir() {
        out.push((path.to_path_buf(), metadata.modified().ok()));
        return;
    }

    // Directory node mtime captures add/remove of direct entries.
    out.push((path.to_path_buf(), metadata.modified().ok()));

    let mut files = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(child_metadata) = entry.metadata().await else {
                continue;
            };
            let child = entry.path();
            if child_metadata.is_dir() {
                stack.push(child);
            } else {
                files.push((child, child_metadata.modified().ok()));
                if files.len() >= MAX_MEMORY_DIR_FILES {
                    break;
                }
            }
        }
        if files.len() >= MAX_MEMORY_DIR_FILES {
            break;
        }
    }
    // Sort for a deterministic fingerprint regardless of readdir order.
    files.sort_by(|left, right| left.0.cmp(&right.0));
    out.extend(files);
}

pub(crate) fn is_cache_valid(cache: &TurnContextCache, current: &Fingerprint) -> bool {
    cache.stored_at.elapsed() >= MIN_CACHE_AGE && cache.fingerprint == *current
}

pub(crate) fn store_cache(fingerprint: Fingerprint, context: TurnContext) -> TurnContextCache {
    TurnContextCache {
        fingerprint,
        stored_at: Instant::now(),
        context,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_fingerprints_match() {
        let a = Fingerprint {
            date: "2026-01-01".into(),
            git_head: Some("abc123".into()),
            git_branch: Some("main".into()),
            git_status: Some(7),
            file_mtimes: vec![(PathBuf::from("/a"), Some(SystemTime::UNIX_EPOCH))],
        };
        let b = Fingerprint {
            date: "2026-01-01".into(),
            git_head: Some("abc123".into()),
            git_branch: Some("main".into()),
            git_status: Some(7),
            file_mtimes: vec![(PathBuf::from("/a"), Some(SystemTime::UNIX_EPOCH))],
        };
        assert_eq!(a, b);
    }

    #[test]
    fn different_git_head_invalidates() {
        let a = Fingerprint {
            date: "2026-01-01".into(),
            git_head: Some("abc".into()),
            git_branch: None,
            git_status: None,
            file_mtimes: vec![],
        };
        let b = Fingerprint {
            date: "2026-01-01".into(),
            git_head: Some("def".into()),
            git_branch: None,
            git_status: None,
            file_mtimes: vec![],
        };
        assert_ne!(a, b);
    }

    #[test]
    fn different_git_status_invalidates() {
        let a = Fingerprint {
            date: "2026-01-01".into(),
            git_head: Some("abc".into()),
            git_branch: Some("main".into()),
            git_status: Some(hash_str(" M src/lib.rs\n")),
            file_mtimes: vec![],
        };
        let b = Fingerprint {
            date: "2026-01-01".into(),
            git_head: Some("abc".into()),
            git_branch: Some("main".into()),
            git_status: Some(hash_str(" M src/lib.rs\n M src/main.rs\n")),
            file_mtimes: vec![],
        };
        assert_ne!(
            a, b,
            "an uncommitted working-tree change (same HEAD) must invalidate the cache"
        );
    }

    #[test]
    fn different_git_branch_invalidates() {
        // Two branches can point at the same commit; switching between them
        // keeps `git_head` identical but must still invalidate the cache so
        // `TurnContext.git_branch` is not stale.
        let a = Fingerprint {
            date: "2026-01-01".into(),
            git_head: Some("abc".into()),
            git_branch: Some("main".into()),
            git_status: Some(0),
            file_mtimes: vec![],
        };
        let b = Fingerprint {
            date: "2026-01-01".into(),
            git_head: Some("abc".into()),
            git_branch: Some("feature".into()),
            git_status: Some(0),
            file_mtimes: vec![],
        };
        assert_ne!(
            a, b,
            "a branch switch at the same commit must invalidate the cached context"
        );
    }

    #[test]
    fn different_date_invalidates() {
        let a = Fingerprint {
            date: "2026-01-01".into(),
            git_head: None,
            git_branch: None,
            git_status: None,
            file_mtimes: vec![],
        };
        let b = Fingerprint {
            date: "2026-01-02".into(),
            git_head: None,
            git_branch: None,
            git_status: None,
            file_mtimes: vec![],
        };
        assert_ne!(a, b);
    }

    #[test]
    fn different_file_mtime_invalidates() {
        let t1 = SystemTime::UNIX_EPOCH;
        let t2 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let a = Fingerprint {
            date: "d".into(),
            git_head: None,
            git_branch: None,
            git_status: None,
            file_mtimes: vec![(PathBuf::from("/x"), Some(t1))],
        };
        let b = Fingerprint {
            date: "d".into(),
            git_head: None,
            git_branch: None,
            git_status: None,
            file_mtimes: vec![(PathBuf::from("/x"), Some(t2))],
        };
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn editing_file_contents_inside_memory_dir_changes_fingerprint() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rules = dir.path().join("rules");
        tokio::fs::create_dir_all(&rules).await.expect("rules dir");
        let rule = rules.join("style.md");
        tokio::fs::write(&rule, "first").await.expect("write rule");

        let mut before = Vec::new();
        collect_path_mtimes(&rules, &mut before).await;

        // Rewrite the file's contents with a distinct mtime; the parent
        // directory node's mtime does not change on a content edit.
        let later = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000_000);
        tokio::fs::write(&rule, "second edited contents")
            .await
            .expect("edit rule");
        let times = std::fs::FileTimes::new().set_modified(later);
        let handle = std::fs::File::options()
            .write(true)
            .open(&rule)
            .expect("open rule for mtime");
        handle.set_times(times).expect("set mtime");

        let mut after = Vec::new();
        collect_path_mtimes(&rules, &mut after).await;

        assert_ne!(
            before, after,
            "editing a file inside a memory directory must change the fingerprint"
        );
    }
}
