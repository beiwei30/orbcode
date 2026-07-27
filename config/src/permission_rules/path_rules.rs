//! Path-target extraction and additional-directory matching for permission
//! rules.
//!
//! Split out from the main `permission_rules` module to keep the bash rule
//! logic separate from filesystem-path rule logic.

use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::canonical_tool_name;

#[derive(Deserialize)]
struct PathToolInput {
    file_path: Option<String>,
    #[serde(rename = "filePath")]
    file_path_camel: Option<String>,
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
        "notebook-edit" => parsed.collect(&[|p| &p.path]),
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
        "notebook-edit" => parsed.collect(&[|p| &p.path]),
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

fn path_is_inside_directory(path: &Path, directory: &Path) -> bool {
    path == directory || path.starts_with(directory)
}
