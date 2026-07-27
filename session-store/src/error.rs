use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error("{0}")]
    Config(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{operation} failed for {}: {source}{}", path.display(), hint.as_deref().map(|h| format!(" ({h})")).unwrap_or_default())]
    TranscriptIo {
        operation: &'static str,
        path: PathBuf,
        hint: Option<String>,
        #[source]
        source: std::io::Error,
    },
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

impl SessionStoreError {
    /// Wraps a raw [`std::io::Error`] with the path and operation that
    /// produced it. The wrapper attaches a short remediation hint for the
    /// common transcript-storage failures so callers (doctor, CLI) can
    /// surface actionable diagnostics.
    pub fn transcript_io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        let hint = io_recovery_hint(&source);
        Self::TranscriptIo {
            operation,
            path: path.to_path_buf(),
            hint,
            source,
        }
    }

    /// Returns true when the error likely reflects a transient filesystem
    /// condition (EINTR/EMFILE/disk hiccup, a permission blip) rather than
    /// genuine transcript corruption.
    ///
    /// Callers use this to avoid caching a `Corrupt` verdict or deleting a
    /// transcript on a one-time read failure. A `SessionNotFound`/`Json`
    /// error means the bytes on disk are actually empty or unparseable, so
    /// those are treated as genuine corruption (not transient).
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Io(_) | Self::TranscriptIo { .. })
    }
}

/// Returns a short remediation hint for common filesystem failures that
/// can corrupt or block transcript writes. Surfaces actionable advice
/// (e.g. "disk full") instead of forcing the user to interpret an
/// `os error` code on their own.
pub fn io_recovery_hint(error: &std::io::Error) -> Option<String> {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::PermissionDenied => {
            Some("permission denied — check file mode/ownership of ~/.claude".to_string())
        }
        ErrorKind::NotFound => {
            Some("path missing — parent directory may have been deleted".to_string())
        }
        ErrorKind::AlreadyExists => Some("path already exists".to_string()),
        ErrorKind::StorageFull => {
            Some("disk full — free space in the transcript directory".to_string())
        }
        ErrorKind::ReadOnlyFilesystem => Some("read-only filesystem".to_string()),
        ErrorKind::QuotaExceeded => Some("filesystem quota exceeded".to_string()),
        _ => match error.raw_os_error() {
            // ENOSPC fallback when the platform reports a numeric code that
            // does not yet round-trip through `ErrorKind::StorageFull`.
            Some(28) => Some("disk full — free space in the transcript directory".to_string()),
            // EDQUOT
            Some(122) => Some("filesystem quota exceeded".to_string()),
            _ => None,
        },
    }
}
