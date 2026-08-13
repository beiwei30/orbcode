//! Path-target extraction and additional-directory matching for permission
//! rules.
//!
//! Split out from the main `permission_rules` module to keep the bash rule
//! logic separate from filesystem-path rule logic.

use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::canonical_tool_name;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolPathBoundary {
    NotPathAware,
    InsideAllowedRoots,
    OutsideAllowedRoots { targets: Vec<PathBuf> },
    InvalidInput,
}

#[derive(Deserialize)]
struct PathToolInput {
    file_path: Option<String>,
    #[serde(rename = "filePath")]
    file_path_camel: Option<String>,
    notebook_path: Option<String>,
    #[serde(rename = "notebookPath")]
    notebook_path_camel: Option<String>,
    path: Option<String>,
    base: Option<String>,
    pattern: Option<String>,
    glob: Option<String>,
}

impl PathToolInput {
    fn collect(&self, fields: &[fn(&Self) -> &Option<String>]) -> Vec<String> {
        fields
            .iter()
            .filter_map(|f| f(self).as_deref())
            .filter(|v| !v.trim().is_empty())
            .map(ToString::to_string)
            .collect()
    }
}

pub(super) fn extract_path_targets_from_tool_input(
    tool_name: &str,
    tool_input: &str,
) -> Vec<String> {
    let Ok(parsed) = serde_json::from_str::<PathToolInput>(tool_input) else {
        return Vec::new();
    };

    match canonical_tool_name(tool_name).as_str() {
        "file-read" | "file-write" | "file-edit" => {
            parsed.collect(&[|p| &p.file_path, |p| &p.file_path_camel, |p| &p.path])
        }
        "notebook-edit" => parsed.collect(&[
            |p| &p.notebook_path,
            |p| &p.notebook_path_camel,
            |p| &p.path,
        ]),
        // For glob, the authoritative access target is the search root
        // (`path`/`base`). The `pattern`/`glob` fields are relative filters, so
        // matching them against a path rule while the root points elsewhere
        // (e.g. `{"path":"/etc","pattern":"src/x"}`) would authorize access
        // outside the intended scope. Only fall back to the filter fields when
        // no explicit root is given (they then encode the cwd-relative path).
        "glob" => {
            let roots = parsed.collect(&[|p| &p.path, |p| &p.base]);
            if roots.is_empty() {
                parsed.collect(&[|p| &p.pattern, |p| &p.glob])
            } else {
                roots
            }
        }
        // grep's `glob` is a filename filter, never a directory — matching it
        // against a path rule (`{"path":"/etc","glob":"src"}`) would authorize
        // grepping `/etc`. Only the search root (`path`) is a real target.
        "grep" => parsed.collect(&[|p| &p.path]),
        "lsp" => parsed.collect(&[|p| &p.file_path_camel, |p| &p.file_path, |p| &p.path]),
        _ => parsed.collect(&[|p| &p.file_path, |p| &p.file_path_camel, |p| &p.path]),
    }
}

fn extract_path_targets_for_additional_directories(
    tool_name: &str,
    tool_input: &str,
) -> Vec<String> {
    let Ok(parsed) = serde_json::from_str::<PathToolInput>(tool_input) else {
        return Vec::new();
    };

    match canonical_tool_name(tool_name).as_str() {
        "file-read" | "file-write" | "file-edit" => {
            parsed.collect(&[|p| &p.file_path, |p| &p.file_path_camel, |p| &p.path])
        }
        "notebook-edit" => parsed.collect(&[
            |p| &p.notebook_path,
            |p| &p.notebook_path_camel,
            |p| &p.path,
        ]),
        "glob" => parsed.collect(&[|p| &p.path, |p| &p.base]),
        "grep" => parsed.collect(&[|p| &p.path]),
        "lsp" => parsed.collect(&[|p| &p.file_path_camel, |p| &p.file_path, |p| &p.path]),
        _ => Vec::new(),
    }
}

/// Returns true when every path operand inside `tool_input` is inside one
/// of `additional_directories` (after canonicalisation). Used as the
/// "implicit allow" path for tools that operate outside the project root
/// but inside a directory the user has trusted.
pub fn tool_path_allowed_by_additional_directory(
    cwd: &Path,
    additional_directories: &[PathBuf],
    tool_name: &str,
    tool_input: &str,
) -> bool {
    if additional_directories.is_empty() {
        return false;
    }

    let targets = extract_path_targets_for_additional_directories(tool_name, tool_input);
    !targets.is_empty()
        && targets.iter().all(|target| {
            permission_target_is_inside_additional_directory(cwd, target, additional_directories)
        })
}

/// Classify every real filesystem target for a path-aware tool against the
/// current workspace and explicitly added directories. Existing targets are
/// canonicalized; for a target that does not exist, the nearest existing
/// ancestor is canonicalized before the remaining path is appended. This
/// prevents both lexical `..` and symlink escapes.
pub fn tool_path_boundary(
    cwd: &Path,
    additional_directories: &[PathBuf],
    tool_name: &str,
    tool_input: &str,
) -> ToolPathBoundary {
    let canonical_name = canonical_tool_name(tool_name);
    if !matches!(
        canonical_name.as_str(),
        "file-read" | "file-write" | "file-edit" | "notebook-edit" | "glob" | "grep" | "lsp"
    ) {
        return ToolPathBoundary::NotPathAware;
    }

    let mut targets = extract_path_targets_from_tool_input(tool_name, tool_input);
    if targets.is_empty() {
        if matches!(canonical_name.as_str(), "glob" | "grep")
            && serde_json::from_str::<serde_json::Value>(tool_input).is_ok()
        {
            targets.push(".".to_string());
        } else {
            return ToolPathBoundary::InvalidInput;
        }
    }

    let Some(workspace_root) = normalize_boundary_path(cwd) else {
        return ToolPathBoundary::InvalidInput;
    };
    let mut allowed_roots = vec![workspace_root];
    for directory in additional_directories {
        let absolute = if directory.is_absolute() {
            directory.clone()
        } else {
            cwd.join(directory)
        };
        let Some(root) = normalize_boundary_path(&absolute) else {
            return ToolPathBoundary::InvalidInput;
        };
        if !allowed_roots.iter().any(|existing| existing == &root) {
            allowed_roots.push(root);
        }
    }

    let mut outside = Vec::new();
    for target in targets {
        let target = PathBuf::from(target);
        let absolute = if target.is_absolute() {
            target
        } else {
            cwd.join(target)
        };
        let Some(normalized) = normalize_boundary_path(&absolute) else {
            return ToolPathBoundary::InvalidInput;
        };
        if !allowed_roots
            .iter()
            .any(|root| path_is_inside_directory(&normalized, root))
        {
            outside.push(normalized);
        }
    }

    if outside.is_empty() {
        ToolPathBoundary::InsideAllowedRoots
    } else {
        ToolPathBoundary::OutsideAllowedRoots { targets: outside }
    }
}

fn permission_target_is_inside_additional_directory(
    cwd: &Path,
    target: &str,
    additional_directories: &[PathBuf],
) -> bool {
    let target_path = PathBuf::from(target);
    let absolute_target = if target_path.is_absolute() {
        target_path
    } else {
        cwd.join(target_path)
    };
    let Some(normalized_target) = normalize_permission_target_path(&absolute_target) else {
        return false;
    };

    additional_directories.iter().any(|directory| {
        let normalized_directory =
            normalize_permission_target_path(directory).unwrap_or_else(|| directory.clone());
        path_is_inside_directory(&normalized_target, &normalized_directory)
    })
}

fn normalize_permission_target_path(path: &Path) -> Option<PathBuf> {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return Some(canonical);
    }

    let parent = path.parent()?;
    let canonical_parent = std::fs::canonicalize(parent).ok()?;
    Some(canonical_parent.join(path.file_name()?))
}

fn normalize_boundary_path(path: &Path) -> Option<PathBuf> {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return Some(canonical);
    }

    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(ancestor) {
            let mut resolved = canonical;
            for component in missing.iter().rev() {
                resolved.push(component);
            }
            return Some(resolved);
        }
        missing.push(ancestor.file_name()?.to_os_string());
        ancestor = ancestor.parent()?;
    }
}

fn path_is_inside_directory(path: &Path, directory: &Path) -> bool {
    path == directory || path.starts_with(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_handles_relative_absolute_missing_and_multiple_targets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&outside).expect("outside");

        assert_eq!(
            tool_path_boundary(
                &workspace,
                &[],
                "file-write",
                r#"{"file_path":"src/new/deep/file.rs"}"#,
            ),
            ToolPathBoundary::InsideAllowedRoots
        );
        assert!(matches!(
            tool_path_boundary(
                &workspace,
                &[],
                "file-write",
                &format!(r#"{{"file_path":"{}"}}"#, outside.join("new.txt").display()),
            ),
            ToolPathBoundary::OutsideAllowedRoots { .. }
        ));
        assert!(matches!(
            tool_path_boundary(
                &workspace,
                &[],
                "file-edit",
                r#"{"file_path":"../outside/new.txt","path":"src/lib.rs"}"#,
            ),
            ToolPathBoundary::OutsideAllowedRoots { .. }
        ));
        assert_eq!(
            tool_path_boundary(
                &workspace,
                std::slice::from_ref(&outside),
                "file-write",
                &format!(r#"{{"file_path":"{}"}}"#, outside.join("new.txt").display()),
            ),
            ToolPathBoundary::InsideAllowedRoots
        );
    }

    #[test]
    fn glob_and_grep_authorize_search_root_not_filter() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&outside).expect("outside");

        assert!(matches!(
            tool_path_boundary(&workspace, &[], "glob", r#"{"pattern":"../../outside/**"}"#),
            ToolPathBoundary::OutsideAllowedRoots { .. }
        ));
        assert!(matches!(
            tool_path_boundary(
                &workspace,
                &[],
                "grep",
                &format!(r#"{{"path":"{}","glob":"src/**"}}"#, outside.display()),
            ),
            ToolPathBoundary::OutsideAllowedRoots { .. }
        ));
    }

    #[test]
    fn notebook_boundary_uses_real_notebook_path_field() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&outside).expect("outside");

        assert_eq!(
            tool_path_boundary(
                &workspace,
                &[],
                "NotebookEdit",
                r#"{"notebook_path":"notebooks/demo.ipynb","new_source":"x"}"#,
            ),
            ToolPathBoundary::InsideAllowedRoots
        );
        assert!(matches!(
            tool_path_boundary(
                &workspace,
                &[],
                "NotebookEdit",
                &format!(
                    r#"{{"notebookPath":"{}","new_source":"x"}}"#,
                    outside.join("demo.ipynb").display()
                ),
            ),
            ToolPathBoundary::OutsideAllowedRoots { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn boundary_canonicalization_detects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&outside).expect("outside");
        symlink(&outside, workspace.join("escape")).expect("symlink");

        assert!(matches!(
            tool_path_boundary(
                &workspace,
                &[],
                "file-write",
                r#"{"file_path":"escape/not-created-yet.txt"}"#,
            ),
            ToolPathBoundary::OutsideAllowedRoots { .. }
        ));

        let nested = outside.join("nested");
        std::fs::create_dir_all(&nested).expect("nested outside directory");
        symlink(&nested, workspace.join("nested-escape")).expect("nested symlink");
        assert!(matches!(
            tool_path_boundary(
                &workspace,
                &[],
                "file-write",
                r#"{"file_path":"nested-escape/../missing.txt"}"#,
            ),
            ToolPathBoundary::OutsideAllowedRoots { .. }
        ));
    }
}
