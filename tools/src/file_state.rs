use std::collections::HashMap;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// TS-aligned error text for the `file-edit` runtime staleness guard
/// (`FILE_UNEXPECTEDLY_MODIFIED_ERROR` in the TypeScript CLI). Returned when an
/// edit is attempted without a prior read, or when the file changed since the
/// last read in a way that is not provably content-identical.
pub(crate) const FILE_UNEXPECTEDLY_MODIFIED_ERROR: &str =
    "File has been unexpectedly modified. Read it again before attempting to write it.";

/// TS-aligned error text for the `file-write` staleness guard. Returned when a
/// previously read file has been modified on disk since that read.
pub(crate) const FILE_MODIFIED_SINCE_READ_ERROR: &str = "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it.";

/// File modification time, floored to whole milliseconds to match the
/// TypeScript representation (`Math.floor(stat.mtimeMs)`). Files whose mtime
/// predates the Unix epoch report `0`, matching the TS clamp.
pub(crate) fn mtime_ms(metadata: &Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_millis())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReadRecord {
    timestamp_ms: u128,
    content: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

impl ReadRecord {
    fn is_full_read(&self) -> bool {
        self.offset.is_none() && self.limit.is_none()
    }
}

/// Per-session table mapping a resolved file path to the state of its most
/// recent read. Used to reject edits/writes that would clobber changes made
/// since the model last observed the file. Mirrors the TypeScript CLI
/// `readFileState` map; a shared handle is threaded through [`crate::ToolContext`]
/// so all file tools in one session see the same table.
#[derive(Debug, Default)]
pub struct FileReadState {
    entries: Mutex<HashMap<PathBuf, ReadRecord>>,
    /// When set, the table is loaded from this file on construction and rewritten
    /// after every mutation, so independent one-shot `orbcode tool` processes (Read
    /// then Edit) share read state across invocations.
    persist_path: Option<PathBuf>,
}

impl FileReadState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a disk-backed table. Existing state at `path` is loaded
    /// best-effort (a missing or corrupt file starts empty); every later
    /// mutation is flushed back to `path` before the mutating method returns,
    /// so cross-process readers always see the latest snapshot.
    pub fn with_persistence(path: PathBuf) -> Self {
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HashMap<PathBuf, ReadRecord>>(&bytes).ok())
            .unwrap_or_default();
        Self {
            entries: Mutex::new(entries),
            persist_path: Some(path),
        }
    }

    /// Serialize `entries` to disk via a tmp-file + rename so a concurrent
    /// reader never sees a half-written file. Uses `tokio::fs` so the I/O
    /// runs on the async runtime without blocking the executor; the caller
    /// `.await`s the result so ordering and completion are guaranteed.
    async fn persist(&self, entries: &HashMap<PathBuf, ReadRecord>) {
        let Some(path) = &self.persist_path else {
            return;
        };
        let Ok(serialized) = serde_json::to_vec(entries) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if tokio::fs::write(&tmp, &serialized).await.is_ok() {
            let _ = tokio::fs::rename(&tmp, path).await;
        }
    }

    /// Record a successful file read. `offset`/`limit` describe the requested
    /// range (both `None` for a whole-file read); only whole-file reads can
    /// later satisfy the content-identity fallback in [`Self::edit_is_stale`].
    pub async fn record_read(
        &self,
        path: &Path,
        timestamp_ms: u128,
        content: String,
        offset: Option<usize>,
        limit: Option<usize>,
    ) {
        let mut entries = self.entries.lock().await;
        entries.insert(
            path.to_path_buf(),
            ReadRecord {
                timestamp_ms,
                content,
                offset,
                limit,
            },
        );
        self.persist(&entries).await;
    }

    /// Record a successful whole-file write/edit, refreshing the read state so a
    /// subsequent edit in the same session is not considered stale. The
    /// recorded range is cleared (full read) so the content-identity fallback
    /// applies.
    pub async fn record_write(&self, path: &Path, timestamp_ms: u128, content: String) {
        self.record_read(path, timestamp_ms, content, None, None)
            .await;
    }

    /// Returns `true` when an edit must be rejected as stale.
    ///
    /// Mirrors the `file-edit` runtime guard: a missing prior read, or a current
    /// mtime newer than the recorded read, is stale — unless the read was a
    /// whole-file read whose recorded content is byte-identical to what is on
    /// disk now (which tolerates mtime-only churn from cloud sync/antivirus).
    pub async fn edit_is_stale(
        &self,
        path: &Path,
        current_mtime_ms: u128,
        current_content: &str,
    ) -> bool {
        let entries = self.entries.lock().await;
        match entries.get(path) {
            None => true,
            Some(record) => {
                if current_mtime_ms > record.timestamp_ms {
                    let content_unchanged =
                        record.is_full_read() && current_content == record.content;
                    !content_unchanged
                } else {
                    false
                }
            }
        }
    }

    /// Returns `true` when a write must be rejected as stale.
    ///
    /// Mirrors `file-write`: only a prior read that predates the current mtime is
    /// stale. A never-read existing file is allowed because a write replaces the
    /// whole file rather than patching it.
    pub async fn write_is_stale(&self, path: &Path, current_mtime_ms: u128) -> bool {
        let entries = self.entries.lock().await;
        match entries.get(path) {
            Some(record) => current_mtime_ms > record.timestamp_ms,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn path() -> PathBuf {
        PathBuf::from("/tmp/example.rs")
    }

    #[tokio::test]
    async fn edit_without_prior_read_is_stale() {
        let state = FileReadState::new();
        assert!(state.edit_is_stale(&path(), 1_000, "anything").await);
    }

    #[tokio::test]
    async fn edit_after_unchanged_read_is_fresh() {
        let state = FileReadState::new();
        state
            .record_read(&path(), 1_000, "body".to_string(), None, None)
            .await;
        // Same mtime as recorded read -> not modified since read.
        assert!(!state.edit_is_stale(&path(), 1_000, "body").await);
    }

    #[tokio::test]
    async fn edit_after_external_change_is_stale() {
        let state = FileReadState::new();
        state
            .record_read(&path(), 1_000, "body".to_string(), None, None)
            .await;
        // mtime advanced and content differs -> stale.
        assert!(state.edit_is_stale(&path(), 2_000, "tampered").await);
    }

    #[tokio::test]
    async fn edit_tolerates_mtime_only_churn_on_full_read() {
        let state = FileReadState::new();
        state
            .record_read(&path(), 1_000, "body".to_string(), None, None)
            .await;
        // mtime advanced but full-read content identical -> safe to proceed.
        assert!(!state.edit_is_stale(&path(), 2_000, "body").await);
    }

    #[tokio::test]
    async fn edit_partial_read_with_advanced_mtime_is_stale_even_if_content_matches() {
        let state = FileReadState::new();
        state
            .record_read(&path(), 1_000, "body".to_string(), Some(1), Some(10))
            .await;
        // Partial reads cannot use the content-identity fallback.
        assert!(state.edit_is_stale(&path(), 2_000, "body").await);
    }

    #[tokio::test]
    async fn write_without_prior_read_is_allowed() {
        let state = FileReadState::new();
        assert!(!state.write_is_stale(&path(), 5_000).await);
    }

    #[tokio::test]
    async fn write_after_external_change_is_stale() {
        let state = FileReadState::new();
        state
            .record_read(&path(), 1_000, "body".to_string(), None, None)
            .await;
        assert!(state.write_is_stale(&path(), 2_000).await);
        // Equal mtime is not stale.
        assert!(!state.write_is_stale(&path(), 1_000).await);
    }

    #[tokio::test]
    async fn record_write_refreshes_state() {
        let state = FileReadState::new();
        state
            .record_read(&path(), 1_000, "body".to_string(), None, None)
            .await;
        // A write bumps mtime and refreshes the recorded read.
        state
            .record_write(&path(), 3_000, "rewritten".to_string())
            .await;
        assert!(!state.edit_is_stale(&path(), 3_000, "rewritten").await);
        assert!(!state.write_is_stale(&path(), 3_000).await);
    }

    #[tokio::test]
    async fn disk_backed_persistence_survives_cross_process_reload() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let persist_path = dir.path().join("file-read-state.json");
        let file = PathBuf::from("/project/src/main.rs");

        let state1 = FileReadState::with_persistence(persist_path.clone());
        state1
            .record_read(&file, 5_000, "fn main() {}".to_string(), None, None)
            .await;

        let state2 = FileReadState::with_persistence(persist_path);
        assert!(
            !state2.edit_is_stale(&file, 5_000, "fn main() {}").await,
            "second process should see the read recorded by the first"
        );
    }

    #[test]
    fn error_text_matches_typescript() {
        assert_eq!(
            FILE_UNEXPECTEDLY_MODIFIED_ERROR,
            "File has been unexpectedly modified. Read it again before attempting to write it."
        );
        assert_eq!(
            FILE_MODIFIED_SINCE_READ_ERROR,
            "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it."
        );
    }
}
