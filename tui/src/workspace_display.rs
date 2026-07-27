use std::path::Path;

pub(crate) fn shorten_display_path(path: &Path) -> String {
    let display = path.display().to_string();
    if let Ok(home) = std::env::var("HOME") {
        if let Some(suffix) = display.strip_prefix(&home) {
            if suffix.is_empty() {
                "~".to_string()
            } else {
                format!("~{suffix}")
            }
        } else {
            display
        }
    } else {
        display
    }
}

// `detect_claude_code_version` lived here: it sniffed `scripts/defines.ts` from
// an adjacent claude-code checkout so the banner could mirror the TypeScript
// CLI's version. The banner now reports Orb Code's own version, so the sniffing
// (and its `find_repo_root` helper) has no callers and is gone.
