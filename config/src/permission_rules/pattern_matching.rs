//! Pattern matching for bash command rules and path rules.
//!
//! Handles wildcard patterns (`*`), prefix patterns (`cmd:*`), literal
//! matching with escape support (`\*`), and path-specific globbing for
//! file permission rules.

use super::tokenize_bash_words;
use super::wrappers::scheduling::{is_xargs_command_token, xargs_wrapper_prefix_width};

pub(super) fn bash_command_matches_pattern(command: &str, pattern: &str) -> bool {
    let command = command.trim();
    let pattern = pattern.trim();

    if pattern == "*" {
        return true;
    }

    if let Some(prefix) = pattern.strip_suffix(":*") {
        return command_matches_prefix(command, prefix.trim());
    }

    if has_unescaped_wildcard(pattern) {
        return wildcard_match(command, pattern);
    }

    command == unescape_rule_literal(pattern)
}

pub(super) fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == "*" {
        return true;
    }

    let relative_only = pattern.starts_with("./");
    let pattern = normalize_path_pattern(pattern);
    let path = normalize_path_pattern(path);

    if relative_only && is_absolute_path_like(&path) {
        return false;
    }
    if relative_only && path.split('/').any(|part| part == "..") {
        return false;
    }

    if pattern == "**" {
        return !relative_only || !is_absolute_path_like(&path);
    }

    if let Some(base) = pattern.strip_suffix("/**") {
        let base = if base.is_empty() { "." } else { base };
        return path == base || path.starts_with(&format!("{base}/"));
    }

    if let Some(base) = pattern.strip_suffix("/*") {
        let base = if base.is_empty() { "." } else { base };
        if base == "." {
            return !path.contains('/') && !is_absolute_path_like(&path);
        }
        let Some(child) = path.strip_prefix(&format!("{base}/")) else {
            return false;
        };
        return !child.is_empty() && !child.contains('/');
    }

    if pattern.contains('*') {
        return wildcard_match(&path, &pattern);
    }

    if pattern == "." {
        return !path.contains('/') && !is_absolute_path_like(&path);
    }

    path == pattern
}

fn normalize_path_pattern(value: &str) -> String {
    let mut normalized = value.trim().replace('\\', "/");
    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    while normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    if normalized.is_empty() {
        ".".to_string()
    } else {
        collapse_parent_segments(&normalized)
    }
}

/// Lexically resolve `.` and `..` segments in an already-slash-normalized path
/// so a traversal escape like `src/../../../etc/passwd` cannot match a base
/// glob pattern (`src/**`) via `starts_with`. A `..` pops the previous concrete
/// segment; leading `..` that cannot be popped are preserved (they represent an
/// escape above the root and therefore never match an in-scope base pattern).
fn collapse_parent_segments(value: &str) -> String {
    let is_absolute = value.starts_with('/');
    // A leading Windows drive designator (`C:`) is a ROOT, like `/`. It must
    // never be popped by a `..`, and it must be preserved through collapsing —
    // otherwise `C:/../Windows` would pop the drive and collapse to the RELATIVE
    // `Windows`, defeating the absolute-path check and letting `Read(./Windows/**)`
    // match the absolute `C:\Windows\System32`. The designator may stand on its
    // own segment (`C:/Windows`) or be fused with the first path component in a
    // *drive-relative* path (`C:secret.txt`, `C:..\Windows`); either way the
    // `C:` is the root and only what follows is subject to `.`/`..` collapse.
    let (drive_root, rest) = match drive_prefix_len(value) {
        Some(len) if !is_absolute => (Some(&value[..len]), &value[len..]),
        _ => (None, value),
    };
    let rooted = is_absolute || drive_root.is_some();
    let mut segments: Vec<&str> = Vec::new();
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|last| *last != "..") {
                    segments.pop();
                } else if !rooted {
                    // Only a relative path preserves a leading `..` (it escapes
                    // above the cwd). At a root (`/` or a drive) `..` is a no-op.
                    segments.push("..");
                }
            }
            other => segments.push(other),
        }
    }
    let joined = segments.join("/");
    if let Some(drive) = drive_root {
        if joined.is_empty() {
            drive.to_string()
        } else {
            format!("{drive}/{joined}")
        }
    } else if is_absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

fn is_absolute_path_like(value: &str) -> bool {
    value.starts_with('/') || is_drive_letter_prefix(value)
}

/// If `value` begins with a Windows drive designator (`[A-Za-z]:`), the byte
/// length of that prefix (always 2). Covers the absolute (`C:/Windows`),
/// bare-root (`C:`), and *drive-relative* (`C:secret.txt`) forms alike.
fn drive_prefix_len(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':').then_some(2)
}

/// Whether `value` begins with a Windows drive designator. Any `[A-Za-z]:`
/// prefix qualifies — including a drive-relative path like `C:secret.txt`, which
/// Windows resolves against the *current directory of drive C*, a location not
/// guaranteed to be inside the workspace. Such a path must never be treated as
/// an ordinary cwd-relative path (which `Read(./**)` would otherwise accept).
fn is_drive_letter_prefix(value: &str) -> bool {
    drive_prefix_len(value).is_some()
}

fn command_matches_prefix(command: &str, prefix: &str) -> bool {
    if command_matches_direct_prefix(command, prefix) {
        return true;
    }

    let Some(tokens) = tokenize_bash_words(command) else {
        return false;
    };
    if !tokens
        .first()
        .is_some_and(|token| is_xargs_command_token(token))
    {
        return false;
    }
    let Some(width) = xargs_wrapper_prefix_width(&tokens) else {
        return false;
    };
    let xargs_command = tokens[width..].join(" ");
    command_matches_direct_prefix(&xargs_command, prefix)
}

fn command_matches_direct_prefix(command: &str, prefix: &str) -> bool {
    command == prefix || command.starts_with(&format!("{prefix} "))
}

pub(super) fn is_bash_prefix_token_like(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|character| character.is_ascii_lowercase())
        && chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

pub(super) fn is_bash_subcommand_like(value: &str) -> bool {
    is_bash_prefix_token_like(value)
}

pub(super) fn is_bare_shell_prefix(value: &str) -> bool {
    matches!(
        value,
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "csh"
            | "tcsh"
            | "ksh"
            | "dash"
            | "cmd"
            | "powershell"
            | "pwsh"
            | "env"
            | "xargs"
            | "parallel"
            | "ssh"
            | "npm"
            | "npx"
            | "tmux"
            | "screen"
            | "docker"
            | "podman"
            | "kubectl"
            | "oc"
            | "socat"
            | "at"
            | "batch"
            | "crontab"
            | "nice"
            | "stdbuf"
            | "nohup"
            | "timeout"
            | "time"
            | "git"
            | "sudo"
            | "doas"
            | "sg"
            | "runuser"
            | "setpriv"
            | "prlimit"
            | "numactl"
            | "setarch"
            | "linux32"
            | "linux64"
            | "pkexec"
            | "chrt"
            | "unshare"
            | "nsenter"
            | "strace"
            | "bwrap"
            | "bubblewrap"
            | "firejail"
            | "systemd-run"
    )
}

pub(super) fn has_unescaped_wildcard(pattern: &str) -> bool {
    let mut escaped = false;
    for character in pattern.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == '*' {
            return true;
        }
    }
    false
}

pub(super) fn unescape_rule_literal(pattern: &str) -> String {
    let mut result = String::new();
    let mut escaped = false;
    for character in pattern.chars() {
        if escaped {
            if character == '*' || character == '\\' {
                result.push(character);
            } else {
                result.push('\\');
                result.push(character);
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            result.push(character);
        }
    }
    if escaped {
        result.push('\\');
    }
    result
}

fn wildcard_match(value: &str, pattern: &str) -> bool {
    let mut remaining = value;
    let mut first = true;
    let (parts, starts_with_wildcard, ends_with_wildcard, star_count) =
        split_wildcard_pattern(pattern);

    if star_count == 1
        && pattern.ends_with(" *")
        && value == unescape_rule_literal(pattern.trim_end_matches(" *"))
    {
        return true;
    }

    for part in &parts {
        if part.is_empty() {
            first = false;
            continue;
        }

        if first && !starts_with_wildcard {
            let Some(stripped) = remaining.strip_prefix(part) else {
                return false;
            };
            remaining = stripped;
        } else {
            let Some(index) = remaining.find(part) else {
                return false;
            };
            remaining = &remaining[index + part.len()..];
        }
        first = false;
    }

    ends_with_wildcard || remaining.is_empty()
}

fn split_wildcard_pattern(pattern: &str) -> (Vec<String>, bool, bool, usize) {
    let mut parts = vec![String::new()];
    let mut escaped = false;
    let mut starts_with_wildcard = false;
    let mut ends_with_wildcard = false;
    let mut star_count = 0usize;

    for (index, character) in pattern.chars().enumerate() {
        if escaped {
            if character == '*' || character == '\\' {
                parts.last_mut().expect("part exists").push(character);
            } else {
                parts.last_mut().expect("part exists").push('\\');
                parts.last_mut().expect("part exists").push(character);
            }
            escaped = false;
            ends_with_wildcard = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == '*' {
            if index == 0 {
                starts_with_wildcard = true;
            }
            ends_with_wildcard = true;
            star_count += 1;
            parts.push(String::new());
        } else {
            parts.last_mut().expect("part exists").push(character);
            ends_with_wildcard = false;
        }
    }
    if escaped {
        parts.last_mut().expect("part exists").push('\\');
    }

    (parts, starts_with_wildcard, ends_with_wildcard, star_count)
}
