use std::env;
use std::path::PathBuf;

/// Parse a comma-delimited environment variable into a tool rule list.
pub(crate) fn parse_list_env(key: &str) -> Vec<String> {
    env::var(key)
        .ok()
        .map(|value| parse_tool_rule_list(&[value]))
        .unwrap_or_default()
}

/// Parse one or more rule-list values (as produced by repeated CLI flags or a
/// single comma/space-separated env var) into individual tool rules. Parenthesised
/// segments (e.g. `Bash(git commit:*)`) are kept intact.
pub fn parse_tool_rule_list(values: &[String]) -> Vec<String> {
    let mut rules = Vec::new();

    for value in values {
        let mut current = String::new();
        let mut paren_depth = 0usize;

        for character in value.chars() {
            match character {
                '(' => {
                    paren_depth = paren_depth.saturating_add(1);
                    current.push(character);
                }
                ')' => {
                    paren_depth = paren_depth.saturating_sub(1);
                    current.push(character);
                }
                ',' | ' ' if paren_depth == 0 => {
                    push_rule(&mut rules, &mut current);
                }
                _ => current.push(character),
            }
        }

        push_rule(&mut rules, &mut current);
    }

    rules
}

fn push_rule(rules: &mut Vec<String>, current: &mut String) {
    let rule = current.trim();
    if !rule.is_empty() {
        rules.push(rule.to_string());
    }
    current.clear();
}

/// Resolve the additional directories from settings and CLI overrides.
/// Relative paths are resolved against `cwd`. Non-existent, non-directory,
/// and duplicate paths are filtered out.
pub(crate) fn resolve_additional_directories(
    cwd: &std::path::Path,
    settings_dirs: &[String],
    override_dirs: Vec<PathBuf>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for raw in settings_dirs {
        push_resolved_additional_dir(cwd, PathBuf::from(raw), &mut dirs);
    }
    for raw in override_dirs {
        push_resolved_additional_dir(cwd, raw, &mut dirs);
    }
    dirs
}

fn push_resolved_additional_dir(cwd: &std::path::Path, raw: PathBuf, dirs: &mut Vec<PathBuf>) {
    let candidate = if raw.is_absolute() {
        raw
    } else {
        cwd.join(raw)
    };
    let canonical = match std::fs::canonicalize(&candidate) {
        Ok(path) => path,
        Err(_) => return,
    };
    if !canonical.is_dir() {
        return;
    }
    if dirs.iter().any(|existing| existing == &canonical) {
        return;
    }
    dirs.push(canonical);
}

#[cfg(test)]
mod tests {
    use super::parse_tool_rule_list;

    #[test]
    fn parse_tool_rule_list_splits_on_comma_and_space() {
        let input = vec!["Read,Write Bash".to_string()];
        assert_eq!(
            parse_tool_rule_list(&input),
            vec!["Read".to_string(), "Write".to_string(), "Bash".to_string()]
        );
    }

    #[test]
    fn parse_tool_rule_list_preserves_parenthesised_args() {
        let input = vec!["Bash(cargo test:*),Read(src/**)".to_string()];
        assert_eq!(
            parse_tool_rule_list(&input),
            vec!["Bash(cargo test:*)".to_string(), "Read(src/**)".to_string(),]
        );
    }

    #[test]
    fn parse_tool_rule_list_handles_multiple_values() {
        let input = vec!["Read".to_string(), "Write,Bash(rm:*)".to_string()];
        assert_eq!(
            parse_tool_rule_list(&input),
            vec![
                "Read".to_string(),
                "Write".to_string(),
                "Bash(rm:*)".to_string(),
            ]
        );
    }

    #[test]
    fn parse_tool_rule_list_handles_empty_entries() {
        let input = vec![",,  , Read , ,".to_string()];
        assert_eq!(parse_tool_rule_list(&input), vec!["Read".to_string()]);
    }
}
