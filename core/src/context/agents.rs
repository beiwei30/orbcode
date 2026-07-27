use std::collections::HashSet;
use std::path::{Path, PathBuf};

use orbcode_protocol::{MemorySource, MemorySourceKind};
use orbcode_tools::{SkillSource, load_skill_definitions};

use super::memory::{push_memory_file, trust_label};

pub(super) async fn push_scoped_agent_sources(
    sources: &mut Vec<MemorySource>,
    seen: &mut HashSet<PathBuf>,
    home_dir: Option<&Path>,
    cwd: &Path,
    project_trusted: bool,
) {
    if let Some(home_dir) = home_dir {
        push_agent_dir(sources, seen, &home_dir.join("agents"), true, "user", None).await;
    }
    let skip = (!project_trusted)
        .then(|| "project agent memory skipped because project is not trusted".to_string());
    push_agent_dir(
        sources,
        seen,
        &cwd.join(".claude").join("agents"),
        true,
        "project",
        skip,
    )
    .await;
}

async fn push_agent_dir(
    sources: &mut Vec<MemorySource>,
    seen: &mut HashSet<PathBuf>,
    dir: &Path,
    writable: bool,
    boundary: &str,
    skip_reason: Option<String>,
) {
    let Ok(exists) = tokio::fs::try_exists(dir).await else {
        return;
    };
    if !exists {
        return;
    }
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    let mut paths = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("md") {
            paths.push(path);
        }
    }
    paths.sort();
    for path in paths {
        let scope = path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(|name| format!("agent:{name}"));
        push_memory_file(
            sources,
            seen,
            MemorySourceKind::Agent,
            path,
            writable,
            false,
            Some(boundary.to_string()),
            scope,
            skip_reason.clone(),
        )
        .await;
    }
}

pub(super) async fn push_scoped_skill_sources(
    sources: &mut Vec<MemorySource>,
    seen: &mut HashSet<PathBuf>,
    home_dir: Option<&Path>,
    cwd: &Path,
    project_trusted: bool,
) {
    let Some(home_dir) = home_dir else {
        return;
    };
    let Ok(skills) = load_skill_definitions(home_dir, cwd).await else {
        return;
    };
    for skill in skills {
        let (writable, boundary, skip_reason) = match skill.source {
            SkillSource::User => (true, "user".to_string(), None),
            SkillSource::Project => (
                true,
                trust_label(project_trusted),
                (!project_trusted).then(|| {
                    "project skill memory skipped because project is not trusted".to_string()
                }),
            ),
            SkillSource::Plugin { plugin_id } => (false, format!("plugin:{plugin_id}"), None),
            SkillSource::Bundled => (false, "bundled".to_string(), None),
            SkillSource::Mcp { server_id } => (false, format!("mcp:{server_id}"), None),
        };
        push_memory_file(
            sources,
            seen,
            MemorySourceKind::Skill,
            skill.path,
            writable,
            false,
            Some(boundary),
            Some(format!("skill:{}", skill.name)),
            skip_reason,
        )
        .await;
    }
}
