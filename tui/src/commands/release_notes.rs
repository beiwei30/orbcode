use anyhow::Result;
use orbcode_app_server_client::{AppClient, StatusOverview};

use crate::render::slash_output::{format_release_notes, parse_changelog_release_notes};

pub(crate) const CHANGELOG_URL: &str =
    "https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md";

pub(crate) async fn run_release_notes_slash_command(
    ui_version: &str,
    app_server: &AppClient,
    session_id: &str,
) -> Result<(String, Option<String>)> {
    let value = app_server
        .status_overview(session_id)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let overview: StatusOverview = serde_json::from_value(value)?;
    let cache_path = overview.home_dir.join("cache").join("changelog.md");
    let changelog = tokio::fs::read_to_string(&cache_path)
        .await
        .unwrap_or_default();
    let notes = parse_changelog_release_notes(&changelog);
    if notes.is_empty() {
        return Ok((
            "Release notes unavailable locally.".to_string(),
            Some(format!(
                "Current version: {ui_version}\nCached changelog: {}\nSee the full changelog at: {CHANGELOG_URL}",
                cache_path.display()
            )),
        ));
    }

    // Deliberately does NOT interpolate `ui_version`: the cached changelog is
    // Anthropic's (see CHANGELOG_URL), so pairing it with Orb Code's own version
    // would read as a claim about a Claude Code release that does not exist.
    Ok((
        "Release notes loaded from the Claude Code changelog.".to_string(),
        Some(format_release_notes(&notes)),
    ))
}
