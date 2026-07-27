use std::path::{Path, PathBuf};

const CWD_PROBE_MARKER: &str = "__ORBCODE_CWD_PROBE__";

pub(crate) struct CwdExtraction {
    pub stdout: String,
    pub new_cwd: Option<PathBuf>,
}

pub(crate) fn wrap_command_with_cwd_probe(command: &str, shell: &str) -> String {
    if shell.contains("fish") {
        format!(
            "{command}\nset -l __orbcode_ec $status; printf '\\n{CWD_PROBE_MARKER}\\n'; pwd; exit $__orbcode_ec"
        )
    } else if cfg!(target_os = "windows") {
        format!(
            "{command}\n$__orbcode_ec = $LASTEXITCODE; Write-Host ''; Write-Host '{CWD_PROBE_MARKER}'; Write-Host (Get-Location).Path; exit $__orbcode_ec"
        )
    } else {
        format!(
            "{command}\n__orbcode_ec=$?; printf '\\n{CWD_PROBE_MARKER}\\n'; pwd; exit $__orbcode_ec"
        )
    }
}

pub(crate) async fn extract_cwd_from_output(stdout: &str, original_cwd: &Path) -> CwdExtraction {
    let Some(marker_pos) = stdout.rfind(CWD_PROBE_MARKER) else {
        return CwdExtraction {
            stdout: stdout.to_string(),
            new_cwd: None,
        };
    };

    let cleaned = stdout[..marker_pos].trim_end().to_string();
    let after_marker = &stdout[marker_pos + CWD_PROBE_MARKER.len()..];
    let cwd_str = after_marker.trim();

    if cwd_str.is_empty() {
        return CwdExtraction {
            stdout: cleaned,
            new_cwd: None,
        };
    }

    let detected = PathBuf::from(cwd_str);
    let changed = cwd_actually_changed(&detected, original_cwd).await;
    let is_dir = tokio::fs::metadata(&detected)
        .await
        .is_ok_and(|m| m.is_dir());
    CwdExtraction {
        stdout: cleaned,
        new_cwd: if changed && is_dir {
            Some(detected)
        } else {
            None
        },
    }
}

async fn cwd_actually_changed(detected: &Path, original: &Path) -> bool {
    if let (Ok(canon_detected), Ok(canon_original)) = (
        tokio::fs::canonicalize(detected).await,
        tokio::fs::canonicalize(original).await,
    ) {
        canon_detected != canon_original
    } else {
        detected != original
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn wrap_adds_cwd_probe_for_bash() {
        let wrapped = wrap_command_with_cwd_probe("ls -la", "/bin/bash");
        assert!(wrapped.starts_with("ls -la\n"));
        assert!(wrapped.contains("__orbcode_ec=$?"));
        assert!(wrapped.contains(CWD_PROBE_MARKER));
        assert!(wrapped.contains("pwd"));
        assert!(wrapped.ends_with("exit $__orbcode_ec"));
    }

    #[test]
    fn wrap_adds_cwd_probe_for_zsh() {
        let wrapped = wrap_command_with_cwd_probe("echo hi", "/bin/zsh");
        assert!(wrapped.contains("__orbcode_ec=$?"));
        assert!(wrapped.contains(CWD_PROBE_MARKER));
    }

    #[test]
    fn wrap_adds_cwd_probe_for_fish() {
        let wrapped = wrap_command_with_cwd_probe("echo hi", "/usr/bin/fish");
        assert!(wrapped.contains("set -l __orbcode_ec $status"));
        assert!(wrapped.contains(CWD_PROBE_MARKER));
        assert!(wrapped.contains("pwd"));
    }

    #[tokio::test]
    async fn extract_detects_cwd_change() {
        let original = env::temp_dir();
        let new_dir = PathBuf::from("/");
        let stdout = format!("some output\n{CWD_PROBE_MARKER}\n{}", new_dir.display());

        let result = extract_cwd_from_output(&stdout, &original).await;
        assert_eq!(result.stdout, "some output");
        assert!(result.new_cwd.is_some());
    }

    #[tokio::test]
    async fn extract_no_change_when_same_cwd() {
        let cwd = env::current_dir().expect("current dir");
        let stdout = format!("output\n{CWD_PROBE_MARKER}\n{}", cwd.display());

        let result = extract_cwd_from_output(&stdout, &cwd).await;
        assert_eq!(result.stdout, "output");
        assert!(result.new_cwd.is_none());
    }

    #[tokio::test]
    async fn extract_no_marker_returns_original() {
        let stdout = "plain output\nno marker here";
        let result = extract_cwd_from_output(stdout, Path::new("/tmp")).await;
        assert_eq!(result.stdout, stdout);
        assert!(result.new_cwd.is_none());
    }

    #[tokio::test]
    async fn extract_empty_stdout_with_marker() {
        let new_dir = PathBuf::from("/");
        let stdout = format!("\n{CWD_PROBE_MARKER}\n{}", new_dir.display());

        let result = extract_cwd_from_output(&stdout, &PathBuf::from("/tmp")).await;
        assert_eq!(result.stdout, "");
        assert!(result.new_cwd.is_some());
    }

    #[tokio::test]
    async fn extract_nonexistent_dir_not_reported() {
        let stdout =
            format!("output\n{CWD_PROBE_MARKER}\n/this/path/definitely/does/not/exist/at/all");
        let result = extract_cwd_from_output(&stdout, Path::new("/tmp")).await;
        assert_eq!(result.stdout, "output");
        assert!(result.new_cwd.is_none());
    }

    #[tokio::test]
    async fn extract_empty_path_after_marker() {
        let stdout = format!("output\n{CWD_PROBE_MARKER}\n");
        let result = extract_cwd_from_output(&stdout, Path::new("/tmp")).await;
        assert_eq!(result.stdout, "output");
        assert!(result.new_cwd.is_none());
    }
}
