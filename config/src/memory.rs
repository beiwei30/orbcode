use std::env;
use std::path::{Path, PathBuf};

pub fn managed_memory_dir() -> PathBuf {
    if let Some(path) =
        env::var_os("CLAUDE_CODE_MANAGED_SETTINGS_PATH").filter(|value| !value.is_empty())
    {
        return PathBuf::from(path);
    }

    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/ClaudeCode")
    }
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(r"C:\Program Files\ClaudeCode")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        PathBuf::from("/etc/claude-code")
    }
}

pub fn managed_memory_file() -> PathBuf {
    managed_memory_dir().join("CLAUDE.md")
}

pub fn managed_rules_dir() -> PathBuf {
    managed_memory_dir().join(".claude").join("rules")
}

pub fn user_memory_file(home_dir: &Path) -> PathBuf {
    home_dir.join("CLAUDE.md")
}

pub fn user_rules_dir(home_dir: &Path) -> PathBuf {
    home_dir.join("rules")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_paths_follow_typescript_claude_home_layout() {
        let home = PathBuf::from("/tmp/.claude");

        assert_eq!(
            user_memory_file(&home),
            PathBuf::from("/tmp/.claude/CLAUDE.md")
        );
        assert_eq!(user_rules_dir(&home), PathBuf::from("/tmp/.claude/rules"));
    }
}
