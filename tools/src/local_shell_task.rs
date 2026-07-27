//! Durable registry for long-running local shell / Bash tasks.
//!
//! Phase 1 focus is the **registry**: state transitions persist across
//! restarts, cancellation and process-exit are recorded on disk, and output
//! is streamed into a byte-addressable log so a follower can resume from an
//! offset after detach. TUI / headless surfaces are deliberately out of scope
//! here; they consume this registry rather than extending it.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::ToolError;

const LOCAL_SHELL_DIR: &str = "local_shell_tasks";
const LOCAL_SHELL_LOGS_DIR: &str = "logs";

/// Default upper bound (per stream) for the in-memory live log buffer that
/// backs `snapshot`. The durable on-disk log is unbounded and
/// byte-addressable via `read_output_from`; this buffer only retains the most
/// recent bytes so a live view can be produced in O(1) memory regardless of
/// how chatty a task is.
const DEFAULT_LIVE_BUFFER_BYTES: usize = 64 * 1024;

/// Which standard stream a chunk of output belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalShellStream {
    Stdout,
    Stderr,
}

/// Lifecycle states recognised by the local-shell registry. The set is closed
/// on purpose: every transition is persisted, so adding a state means deciding
/// how already-written records decode.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalShellTaskStatus {
    Queued,
    PermissionPending,
    Running,
    Interrupting,
    Succeeded,
    Failed,
    Interrupted,
}

impl LocalShellTaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::PermissionPending => "permission_pending",
            Self::Running => "running",
            Self::Interrupting => "interrupting",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Interrupted)
    }

    pub fn is_active(self) -> bool {
        !self.is_terminal()
    }

    fn can_transition_to(self, next: Self) -> bool {
        use LocalShellTaskStatus::*;
        match (self, next) {
            // Idempotent self transitions are allowed so callers can re-emit
            // the same lifecycle event without tripping the validator.
            (a, b) if a == b => true,
            (Queued, PermissionPending) => true,
            (Queued, Running) => true,
            (Queued, Interrupted) => true,
            (Queued, Failed) => true,
            // No PID exists during PermissionPending so we skip Interrupting
            // and jump straight to a terminal state when the user declines or
            // cancels the gate.
            (PermissionPending, Running) => true,
            (PermissionPending, Interrupted) => true,
            (PermissionPending, Failed) => true,
            (Running, Interrupting) => true,
            (Running, Succeeded) => true,
            (Running, Failed) => true,
            (Running, Interrupted) => true,
            (Interrupting, Interrupted) => true,
            (Interrupting, Succeeded) => true,
            (Interrupting, Failed) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalShellAttempt {
    pub attempt: u32,
    pub pid: Option<u32>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    /// Byte offset into the persistent log file where this attempt began
    /// writing. Together with `log_byte_end` this lets a follower replay or
    /// skip output produced by a single attempt.
    pub log_byte_start: u64,
    pub log_byte_end: u64,
    pub kill_reason: Option<String>,
    pub terminal_status: Option<LocalShellTaskStatus>,
}

impl LocalShellAttempt {
    fn new(attempt: u32, log_byte_start: u64) -> Self {
        Self {
            attempt,
            pid: None,
            started_at: None,
            finished_at: None,
            exit_code: None,
            signal: None,
            log_byte_start,
            log_byte_end: log_byte_start,
            kill_reason: None,
            terminal_status: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CreateLocalShellTask {
    pub session_id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalShellTaskRecord {
    pub task_id: String,
    pub session_id: String,
    pub command: String,
    pub cwd: String,
    pub label: Option<String>,
    pub status: LocalShellTaskStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The attempt currently in flight (or the most recent one if we are in a
    /// terminal state). `None` only between create and the first transition
    /// that opens an attempt.
    pub current_attempt: Option<LocalShellAttempt>,
    /// Completed attempts in chronological order, oldest first. A restart
    /// archives `current_attempt` here before allocating a fresh attempt.
    pub history: Vec<LocalShellAttempt>,
    pub restart_count: u32,
    /// Total bytes written to the log file across every attempt. Followers
    /// poll `(output_bytes, read_output_from)` to stream output without races.
    pub output_bytes: u64,
    pub log_path: String,
    pub last_error: Option<String>,
    pub kill_signal_sent: Option<String>,
}

/// A fixed-capacity byte buffer that keeps only the most recent `cap` bytes.
/// When more bytes are appended than fit, the oldest bytes are dropped from
/// the front and counted in `dropped` so callers can flag truncation and tell
/// the user how much scrollback was lost.
#[derive(Clone, Debug, Default)]
struct BoundedLog {
    buf: VecDeque<u8>,
    dropped: u64,
}

impl BoundedLog {
    fn append(&mut self, chunk: &[u8], cap: usize) {
        if cap == 0 {
            self.dropped = self
                .dropped
                .saturating_add(u64::try_from(chunk.len()).unwrap_or(0));
            self.buf.clear();
            return;
        }
        self.buf.extend(chunk.iter().copied());
        if self.buf.len() > cap {
            let overflow = self.buf.len() - cap;
            self.buf.drain(0..overflow);
            self.dropped = self
                .dropped
                .saturating_add(u64::try_from(overflow).unwrap_or(0));
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    fn truncated(&self) -> bool {
        self.dropped > 0
    }
}

/// In-memory, process-local runtime state for a live task. This is *not*
/// persisted: it holds the bounded scrollback buffers and the cancellation
/// request flag, both of which only matter while the owning process is alive.
/// After a restart the durable record (status, full log) is recovered from
/// disk; these buffers simply start empty again.
#[derive(Clone, Debug, Default)]
struct LiveTaskState {
    stdout: BoundedLog,
    stderr: BoundedLog,
    cancel_requested: bool,
}

/// A point-in-time view of a task's status plus the most recent buffered
/// stdout/stderr. Returned by [`LocalShellTaskRegistry::snapshot`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalShellTaskSnapshot {
    pub task_id: String,
    pub status: LocalShellTaskStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// True when older stdout bytes were dropped to honour the buffer cap.
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub stdout_dropped_bytes: u64,
    pub stderr_dropped_bytes: u64,
    pub cancel_requested: bool,
    /// Total bytes written to the durable on-disk log across all attempts.
    pub output_bytes: u64,
}

/// Durable on-disk registry for local shell tasks.
///
/// **Concurrency.** The high-level stream API (`append_stdout` /
/// `append_stderr`) serialises all writes for a given `task_id` behind a
/// per-task async lock, so concurrent producers cannot lose record-field
/// updates and the in-memory scrollback stays consistent. The lower-level
/// `transition` / `append_output` primitives still follow an unguarded
/// load-mutate-save pattern; callers driving those directly must own the
/// `task_id` (the append-only log file itself is OS-atomic, so byte content is
/// always safe). The live scrollback buffers and cancellation flags are kept
/// in a process-local map guarded by a `std::sync::Mutex`.
#[derive(Clone)]
pub struct LocalShellTaskRegistry {
    tasks_dir: PathBuf,
    logs_dir: PathBuf,
    buffer_cap: usize,
    live: Arc<Mutex<HashMap<String, LiveTaskState>>>,
    task_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
}

impl fmt::Debug for LocalShellTaskRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalShellTaskRegistry")
            .field("tasks_dir", &self.tasks_dir)
            .field("logs_dir", &self.logs_dir)
            .field("buffer_cap", &self.buffer_cap)
            .finish_non_exhaustive()
    }
}

impl LocalShellTaskRegistry {
    pub fn new(home_dir: &Path) -> Self {
        Self::with_buffer_cap(home_dir, DEFAULT_LIVE_BUFFER_BYTES)
    }

    /// Construct a registry with a custom per-stream live-buffer cap. Used by
    /// callers that want tighter memory bounds and by tests exercising
    /// truncation.
    pub fn with_buffer_cap(home_dir: &Path, buffer_cap: usize) -> Self {
        let tasks_dir = home_dir.join(LOCAL_SHELL_DIR);
        let logs_dir = tasks_dir.join(LOCAL_SHELL_LOGS_DIR);
        Self {
            tasks_dir,
            logs_dir,
            buffer_cap,
            live: Arc::new(Mutex::new(HashMap::new())),
            task_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn tasks_dir(&self) -> &Path {
        &self.tasks_dir
    }

    pub fn log_path_for(&self, task_id: &str) -> PathBuf {
        self.logs_dir.join(format!("{task_id}.log"))
    }

    pub fn record_path_for(&self, task_id: &str) -> PathBuf {
        self.tasks_dir.join(format!("{task_id}.json"))
    }

    pub async fn create(
        &self,
        request: CreateLocalShellTask,
    ) -> Result<LocalShellTaskRecord, ToolError> {
        self.ensure_dirs().await?;
        let task_id = Uuid::new_v4().to_string();
        let log_path = self.log_path_for(&task_id);
        tokio::fs::write(&log_path, b"").await?;
        let now = Utc::now();
        let record = LocalShellTaskRecord {
            task_id: task_id.clone(),
            session_id: request.session_id,
            command: request.command,
            cwd: request.cwd.display().to_string(),
            label: request.label,
            status: LocalShellTaskStatus::Queued,
            created_at: now,
            updated_at: now,
            current_attempt: None,
            history: Vec::new(),
            restart_count: 0,
            output_bytes: 0,
            log_path: log_path.display().to_string(),
            last_error: None,
            kill_signal_sent: None,
        };
        self.save(&record).await?;
        self.live
            .lock()
            .expect("live map poisoned")
            .insert(task_id.clone(), LiveTaskState::default());
        Ok(record)
    }

    pub async fn load(&self, task_id: &str) -> Result<LocalShellTaskRecord, ToolError> {
        let path = self.record_path_for(task_id);
        if !tokio::fs::try_exists(&path).await? {
            return Err(ToolError::InvalidInput(format!(
                "local shell task not found: {task_id}"
            )));
        }
        let contents = tokio::fs::read_to_string(&path).await?;
        Ok(serde_json::from_str(&contents)?)
    }

    /// Reconcile a record's `output_bytes` (and current attempt's
    /// `log_byte_end`) against the on-disk log length. Useful after a crash
    /// between `append_output`'s file write and the record save — bytes are
    /// already on disk and the record can be brought in sync without losing
    /// them. Returns the reconciled record.
    pub async fn reconcile_log_offset(
        &self,
        task_id: &str,
    ) -> Result<LocalShellTaskRecord, ToolError> {
        let record = self.load(task_id).await?;
        let log_path = PathBuf::from(&record.log_path);
        let actual_len = if tokio::fs::try_exists(&log_path).await? {
            tokio::fs::metadata(&log_path).await?.len()
        } else {
            0
        };
        if actual_len == record.output_bytes {
            return Ok(record);
        }
        self.transition(task_id, |record| {
            record.output_bytes = actual_len;
            if let Some(attempt) = record.current_attempt.as_mut() {
                attempt.log_byte_end = actual_len;
            }
            Ok(())
        })
        .await
    }

    pub async fn list_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<LocalShellTaskRecord>, ToolError> {
        let mut out = Vec::new();
        if !tokio::fs::try_exists(&self.tasks_dir).await? {
            return Ok(out);
        }
        let mut entries = tokio::fs::read_dir(&self.tasks_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(contents) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            let Ok(record) = serde_json::from_str::<LocalShellTaskRecord>(&contents) else {
                continue;
            };
            if record.session_id == session_id {
                out.push(record);
            }
        }
        out.sort_by_key(|record| record.created_at);
        Ok(out)
    }

    pub async fn mark_permission_pending(
        &self,
        task_id: &str,
    ) -> Result<LocalShellTaskRecord, ToolError> {
        self.transition(task_id, |record| {
            ensure_transition(record.status, LocalShellTaskStatus::PermissionPending)?;
            record.status = LocalShellTaskStatus::PermissionPending;
            Ok(())
        })
        .await
    }

    pub async fn mark_running(
        &self,
        task_id: &str,
        pid: Option<u32>,
    ) -> Result<LocalShellTaskRecord, ToolError> {
        self.transition(task_id, |record| {
            ensure_transition(record.status, LocalShellTaskStatus::Running)?;
            let attempt = open_attempt(record);
            attempt.pid = pid;
            attempt.started_at.get_or_insert_with(Utc::now);
            record.status = LocalShellTaskStatus::Running;
            record.last_error = None;
            Ok(())
        })
        .await
    }

    pub async fn mark_interrupting(
        &self,
        task_id: &str,
        signal: &str,
        reason: Option<&str>,
    ) -> Result<LocalShellTaskRecord, ToolError> {
        self.transition(task_id, |record| {
            ensure_transition(record.status, LocalShellTaskStatus::Interrupting)?;
            record.status = LocalShellTaskStatus::Interrupting;
            record.kill_signal_sent = Some(signal.to_string());
            if let Some(attempt) = record.current_attempt.as_mut()
                && let Some(reason) = reason
            {
                attempt.kill_reason = Some(reason.to_string());
            }
            Ok(())
        })
        .await
    }

    pub async fn mark_succeeded(
        &self,
        task_id: &str,
        exit_code: i32,
    ) -> Result<LocalShellTaskRecord, ToolError> {
        self.finish(
            task_id,
            LocalShellTaskStatus::Succeeded,
            Some(exit_code),
            None,
            None,
        )
        .await
    }

    pub async fn mark_failed(
        &self,
        task_id: &str,
        exit_code: Option<i32>,
        signal: Option<i32>,
        error: Option<String>,
    ) -> Result<LocalShellTaskRecord, ToolError> {
        self.finish(
            task_id,
            LocalShellTaskStatus::Failed,
            exit_code,
            signal,
            error,
        )
        .await
    }

    pub async fn mark_interrupted(
        &self,
        task_id: &str,
        exit_code: Option<i32>,
        signal: Option<i32>,
        reason: Option<String>,
    ) -> Result<LocalShellTaskRecord, ToolError> {
        self.finish(
            task_id,
            LocalShellTaskStatus::Interrupted,
            exit_code,
            signal,
            reason,
        )
        .await
    }

    async fn finish(
        &self,
        task_id: &str,
        next: LocalShellTaskStatus,
        exit_code: Option<i32>,
        signal: Option<i32>,
        reason: Option<String>,
    ) -> Result<LocalShellTaskRecord, ToolError> {
        self.transition(task_id, |record| {
            ensure_transition(record.status, next)?;
            let now = Utc::now();
            let log_byte_end = record.output_bytes;
            // Open an attempt on the fly if the task terminated before
            // mark_running was called (e.g. failed during permission gate).
            // Stamp `started_at = now` so consumers always see start <= finish.
            let attempt = open_attempt(record);
            attempt.started_at.get_or_insert(now);
            attempt.finished_at = Some(now);
            attempt.exit_code = exit_code;
            attempt.signal = signal;
            attempt.log_byte_end = log_byte_end;
            attempt.terminal_status = Some(next);
            if next == LocalShellTaskStatus::Interrupted && attempt.kill_reason.is_none() {
                attempt.kill_reason = reason.clone();
            }
            record.status = next;
            record.last_error = match next {
                LocalShellTaskStatus::Failed => reason.clone(),
                LocalShellTaskStatus::Interrupted => reason,
                _ => None,
            };
            Ok(())
        })
        .await
    }

    /// Append `chunk` to the durable log and update `output_bytes` atomically.
    /// Returns the new total byte count.
    pub async fn append_output(&self, task_id: &str, chunk: &[u8]) -> Result<u64, ToolError> {
        let record = self.load(task_id).await?;
        let log_path = PathBuf::from(&record.log_path);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await?;
        file.write_all(chunk).await?;
        file.flush().await?;
        let metadata = tokio::fs::metadata(&log_path).await?;
        let new_total = metadata.len();
        let written = u64::try_from(chunk.len()).unwrap_or(0);
        self.transition(task_id, |record| {
            record.output_bytes = new_total;
            if record.current_attempt.is_some() || written > 0 {
                // Either an attempt is already open, or we just produced
                // output before mark_running (rare). Either way we want a
                // place to anchor the byte range — open_attempt will reuse
                // the current attempt if there is one.
                let start_before = new_total.saturating_sub(written);
                let attempt = open_attempt(record);
                if attempt.log_byte_start > start_before {
                    attempt.log_byte_start = start_before;
                }
                attempt.log_byte_end = new_total;
            }
            Ok(())
        })
        .await?;
        Ok(new_total)
    }

    /// Read at most `limit` bytes from the log starting at `offset`.
    /// Returns `(bytes, new_offset)`. When `offset` is past the end the
    /// returned buffer is empty and `new_offset == offset`.
    pub async fn read_output_from(
        &self,
        task_id: &str,
        offset: u64,
        limit: usize,
    ) -> Result<(Vec<u8>, u64), ToolError> {
        let record = self.load(task_id).await?;
        let log_path = PathBuf::from(&record.log_path);
        if !tokio::fs::try_exists(&log_path).await? {
            return Ok((Vec::new(), offset));
        }
        let mut file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(&log_path)
            .await?;
        let file_len = file.metadata().await?.len();
        if offset >= file_len {
            return Ok((Vec::new(), offset));
        }
        file.seek(SeekFrom::Start(offset)).await?;
        let remaining = usize::try_from(file_len - offset).unwrap_or(usize::MAX);
        let take = remaining.min(limit);
        let mut buf = vec![0u8; take];
        file.read_exact(&mut buf).await?;
        let new_offset = offset.saturating_add(u64::try_from(take).unwrap_or(0));
        Ok((buf, new_offset))
    }

    /// Archive the current attempt and put the task back in `Queued` so a
    /// subsequent `mark_running` opens a fresh attempt. Restart is only
    /// allowed from a terminal state — that's what makes the data model
    /// safe to recover after a crash: there's no in-flight attempt to lose.
    pub async fn request_restart(&self, task_id: &str) -> Result<LocalShellTaskRecord, ToolError> {
        self.transition(task_id, |record| {
            if !record.status.is_terminal() {
                return Err(ToolError::InvalidInput(format!(
                    "cannot restart local shell task {} from state {}",
                    record.task_id,
                    record.status.as_str()
                )));
            }
            if let Some(attempt) = record.current_attempt.take() {
                record.history.push(attempt);
            }
            record.status = LocalShellTaskStatus::Queued;
            record.restart_count = record.restart_count.saturating_add(1);
            record.last_error = None;
            record.kill_signal_sent = None;
            Ok(())
        })
        .await
    }

    /// Append a stdout chunk: durably to the on-disk log and to the bounded
    /// in-memory scrollback. Thread-safe for concurrent callers on the same
    /// `task_id`. Returns the new total on-disk byte count.
    pub async fn append_stdout(&self, task_id: &str, chunk: &[u8]) -> Result<u64, ToolError> {
        self.append_stream(task_id, LocalShellStream::Stdout, chunk)
            .await
    }

    /// Append a stderr chunk. See [`append_stdout`](Self::append_stdout).
    pub async fn append_stderr(&self, task_id: &str, chunk: &[u8]) -> Result<u64, ToolError> {
        self.append_stream(task_id, LocalShellStream::Stderr, chunk)
            .await
    }

    async fn append_stream(
        &self,
        task_id: &str,
        stream: LocalShellStream,
        chunk: &[u8],
    ) -> Result<u64, ToolError> {
        // Serialise every write for this task so concurrent producers cannot
        // race the load-mutate-save of the durable record.
        let lock = self.task_lock(task_id);
        let _guard = lock.lock().await;
        let total = self.append_output(task_id, chunk).await?;
        let mut live = self.live.lock().expect("live map poisoned");
        let state = live.entry(task_id.to_string()).or_default();
        match stream {
            LocalShellStream::Stdout => state.stdout.append(chunk, self.buffer_cap),
            LocalShellStream::Stderr => state.stderr.append(chunk, self.buffer_cap),
        }
        Ok(total)
    }

    /// Return a point-in-time view of the task's status plus its most recent
    /// buffered stdout/stderr. Errors for an unknown `task_id`.
    pub async fn snapshot(&self, task_id: &str) -> Result<LocalShellTaskSnapshot, ToolError> {
        let record = self.load(task_id).await?;
        let live = self.live.lock().expect("live map poisoned");
        let state = live.get(task_id).cloned().unwrap_or_default();
        Ok(LocalShellTaskSnapshot {
            task_id: record.task_id,
            status: record.status,
            stdout: state.stdout.snapshot(),
            stderr: state.stderr.snapshot(),
            stdout_truncated: state.stdout.truncated(),
            stderr_truncated: state.stderr.truncated(),
            stdout_dropped_bytes: state.stdout.dropped,
            stderr_dropped_bytes: state.stderr.dropped,
            cancel_requested: state.cancel_requested,
            output_bytes: record.output_bytes,
        })
    }

    /// Record a cancellation request for the task. Idempotent and safe to call
    /// after the task has already reached a terminal state — it only sets an
    /// in-memory intent flag that a running worker observes via
    /// [`is_cancel_requested`](Self::is_cancel_requested) to drive the actual
    /// interrupt. Errors only for an unknown `task_id`.
    pub async fn request_cancel(&self, task_id: &str) -> Result<(), ToolError> {
        self.load(task_id).await?;
        self.live
            .lock()
            .expect("live map poisoned")
            .entry(task_id.to_string())
            .or_default()
            .cancel_requested = true;
        Ok(())
    }

    /// Whether a cancellation has been requested for the task. Returns `false`
    /// for unknown tasks or tasks with no live state (e.g. after a restart,
    /// since the flag is process-local).
    pub fn is_cancel_requested(&self, task_id: &str) -> bool {
        self.live
            .lock()
            .expect("live map poisoned")
            .get(task_id)
            .is_some_and(|state| state.cancel_requested)
    }

    fn task_lock(&self, task_id: &str) -> Arc<AsyncMutex<()>> {
        self.task_locks
            .lock()
            .expect("task lock map poisoned")
            .entry(task_id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    async fn ensure_dirs(&self) -> Result<(), ToolError> {
        tokio::fs::create_dir_all(&self.tasks_dir).await?;
        tokio::fs::create_dir_all(&self.logs_dir).await?;
        Ok(())
    }

    async fn save(&self, record: &LocalShellTaskRecord) -> Result<(), ToolError> {
        self.ensure_dirs().await?;
        let path = self.record_path_for(&record.task_id);
        let tmp = self
            .tasks_dir
            .join(format!("{}.{}.tmp", record.task_id, Uuid::new_v4()));
        tokio::fs::write(&tmp, serde_json::to_vec_pretty(record)?).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }

    async fn transition<F>(
        &self,
        task_id: &str,
        mutate: F,
    ) -> Result<LocalShellTaskRecord, ToolError>
    where
        F: FnOnce(&mut LocalShellTaskRecord) -> Result<(), ToolError>,
    {
        let mut record = self.load(task_id).await?;
        mutate(&mut record)?;
        record.updated_at = Utc::now();
        self.save(&record).await?;
        Ok(record)
    }
}

fn ensure_transition(
    current: LocalShellTaskStatus,
    next: LocalShellTaskStatus,
) -> Result<(), ToolError> {
    if current.can_transition_to(next) {
        Ok(())
    } else {
        Err(ToolError::InvalidInput(format!(
            "invalid local shell task transition: {} -> {}",
            current.as_str(),
            next.as_str()
        )))
    }
}

fn next_attempt_number(record: &LocalShellTaskRecord) -> u32 {
    u32::try_from(record.history.len())
        .unwrap_or(u32::MAX)
        .saturating_add(1)
}

/// Return a mutable reference to the current attempt, opening a new one
/// anchored at the current `output_bytes` cursor if none exists. Callers may
/// further narrow `log_byte_start` after this returns if they know the
/// attempt began earlier than the current cursor (used by `append_output`
/// when bytes arrive before `mark_running`).
fn open_attempt(record: &mut LocalShellTaskRecord) -> &mut LocalShellAttempt {
    if record.current_attempt.is_none() {
        let attempt_number = next_attempt_number(record);
        record.current_attempt = Some(LocalShellAttempt::new(attempt_number, record.output_bytes));
    }
    record
        .current_attempt
        .as_mut()
        .expect("current_attempt just inserted")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn registry() -> (TempDir, LocalShellTaskRegistry) {
        let dir = TempDir::new().expect("tempdir");
        let registry = LocalShellTaskRegistry::new(dir.path());
        (dir, registry)
    }

    fn create_request(session: &str, cmd: &str) -> CreateLocalShellTask {
        CreateLocalShellTask {
            session_id: session.to_string(),
            command: cmd.to_string(),
            cwd: std::env::temp_dir(),
            label: None,
        }
    }

    #[tokio::test]
    async fn happy_path_run_to_success() {
        let (_dir, registry) = registry();
        let record = registry
            .create(create_request("s1", "echo hi"))
            .await
            .expect("create");
        assert_eq!(record.status, LocalShellTaskStatus::Queued);
        assert!(record.current_attempt.is_none());

        let pending = registry
            .mark_permission_pending(&record.task_id)
            .await
            .expect("permission pending");
        assert_eq!(pending.status, LocalShellTaskStatus::PermissionPending);

        let running = registry
            .mark_running(&record.task_id, Some(4242))
            .await
            .expect("running");
        assert_eq!(running.status, LocalShellTaskStatus::Running);
        let attempt = running.current_attempt.as_ref().expect("attempt opened");
        assert_eq!(attempt.attempt, 1);
        assert_eq!(attempt.pid, Some(4242));
        assert!(attempt.started_at.is_some());

        registry
            .append_output(&record.task_id, b"hello\n")
            .await
            .expect("append output");
        registry
            .append_output(&record.task_id, b"world\n")
            .await
            .expect("append more");

        let finished = registry
            .mark_succeeded(&record.task_id, 0)
            .await
            .expect("succeeded");
        assert_eq!(finished.status, LocalShellTaskStatus::Succeeded);
        assert_eq!(finished.output_bytes, 12);
        let attempt = finished.current_attempt.as_ref().expect("attempt closed");
        assert_eq!(attempt.exit_code, Some(0));
        assert_eq!(attempt.log_byte_start, 0);
        assert_eq!(attempt.log_byte_end, 12);
        assert_eq!(
            attempt.terminal_status,
            Some(LocalShellTaskStatus::Succeeded)
        );
    }

    #[tokio::test]
    async fn state_survives_new_registry_instance() {
        let dir = TempDir::new().expect("tempdir");
        let registry = LocalShellTaskRegistry::new(dir.path());
        let record = registry
            .create(create_request("s1", "sleep 1"))
            .await
            .expect("create");
        registry
            .mark_running(&record.task_id, Some(9))
            .await
            .expect("running");
        registry
            .append_output(&record.task_id, b"line1\n")
            .await
            .expect("append");
        registry
            .mark_failed(
                &record.task_id,
                Some(2),
                None,
                Some("nonzero exit".to_string()),
            )
            .await
            .expect("failed");

        // Drop the existing handle and rehydrate from disk.
        drop(registry);
        let resumed = LocalShellTaskRegistry::new(dir.path());
        let loaded = resumed.load(&record.task_id).await.expect("reload");
        assert_eq!(loaded.status, LocalShellTaskStatus::Failed);
        assert_eq!(loaded.last_error.as_deref(), Some("nonzero exit"));
        let attempt = loaded.current_attempt.as_ref().expect("attempt");
        assert_eq!(attempt.exit_code, Some(2));
        assert_eq!(attempt.log_byte_end, 6);
    }

    #[tokio::test]
    async fn read_output_resumes_from_offset() {
        let (_dir, registry) = registry();
        let record = registry
            .create(create_request("s1", "cat"))
            .await
            .expect("create");
        registry
            .mark_running(&record.task_id, None)
            .await
            .expect("running");
        registry
            .append_output(&record.task_id, b"aaaa")
            .await
            .expect("first chunk");
        let (first, offset_after_first) = registry
            .read_output_from(&record.task_id, 0, 1024)
            .await
            .expect("read first");
        assert_eq!(first, b"aaaa");
        assert_eq!(offset_after_first, 4);

        registry
            .append_output(&record.task_id, b"bbbbcc")
            .await
            .expect("second chunk");
        let (second, offset_after_second) = registry
            .read_output_from(&record.task_id, offset_after_first, 1024)
            .await
            .expect("read second");
        assert_eq!(second, b"bbbbcc");
        assert_eq!(offset_after_second, 10);

        // Reading past the end is a no-op.
        let (tail, tail_offset) = registry
            .read_output_from(&record.task_id, offset_after_second, 1024)
            .await
            .expect("read tail");
        assert!(tail.is_empty());
        assert_eq!(tail_offset, offset_after_second);
    }

    #[tokio::test]
    async fn interrupt_path_records_kill_metadata() {
        let (_dir, registry) = registry();
        let record = registry
            .create(create_request("s1", "yes"))
            .await
            .expect("create");
        registry
            .mark_running(&record.task_id, Some(1234))
            .await
            .expect("running");
        let interrupting = registry
            .mark_interrupting(&record.task_id, "SIGTERM", Some("user cancel"))
            .await
            .expect("interrupting");
        assert_eq!(interrupting.status, LocalShellTaskStatus::Interrupting);
        assert_eq!(interrupting.kill_signal_sent.as_deref(), Some("SIGTERM"));

        let interrupted = registry
            .mark_interrupted(&record.task_id, None, Some(15), None)
            .await
            .expect("interrupted");
        assert_eq!(interrupted.status, LocalShellTaskStatus::Interrupted);
        let attempt = interrupted.current_attempt.as_ref().expect("attempt");
        assert_eq!(attempt.signal, Some(15));
        assert_eq!(attempt.kill_reason.as_deref(), Some("user cancel"));
        assert_eq!(
            attempt.terminal_status,
            Some(LocalShellTaskStatus::Interrupted)
        );
    }

    #[tokio::test]
    async fn restart_archives_attempt_and_reopens_queued() {
        let (_dir, registry) = registry();
        let record = registry
            .create(create_request("s1", "make test"))
            .await
            .expect("create");
        registry
            .mark_running(&record.task_id, Some(1))
            .await
            .expect("running");
        registry
            .append_output(&record.task_id, b"first run\n")
            .await
            .expect("append");
        registry
            .mark_failed(&record.task_id, Some(1), None, Some("flaky".into()))
            .await
            .expect("failed");

        let restarted = registry
            .request_restart(&record.task_id)
            .await
            .expect("restart");
        assert_eq!(restarted.status, LocalShellTaskStatus::Queued);
        assert_eq!(restarted.restart_count, 1);
        assert_eq!(restarted.history.len(), 1);
        assert!(restarted.current_attempt.is_none());
        assert_eq!(restarted.history[0].attempt, 1);
        assert_eq!(restarted.history[0].exit_code, Some(1));

        // Second attempt picks up the next attempt number and offset
        // continues from where the log left off.
        registry
            .mark_running(&record.task_id, Some(2))
            .await
            .expect("running 2");
        registry
            .append_output(&record.task_id, b"second\n")
            .await
            .expect("append 2");
        let done = registry
            .mark_succeeded(&record.task_id, 0)
            .await
            .expect("success");
        let attempt = done.current_attempt.as_ref().expect("attempt 2");
        assert_eq!(attempt.attempt, 2);
        assert_eq!(attempt.log_byte_start, 10);
        assert_eq!(attempt.log_byte_end, 17);
    }

    #[tokio::test]
    async fn rejects_invalid_transitions() {
        let (_dir, registry) = registry();
        let record = registry
            .create(create_request("s1", "ls"))
            .await
            .expect("create");
        let err = registry
            .mark_succeeded(&record.task_id, 0)
            .await
            .expect_err("cannot succeed from queued");
        assert!(matches!(err, ToolError::InvalidInput(_)));

        registry
            .mark_running(&record.task_id, None)
            .await
            .expect("run");
        registry
            .mark_succeeded(&record.task_id, 0)
            .await
            .expect("succeed");
        let err = registry
            .mark_running(&record.task_id, None)
            .await
            .expect_err("cannot rerun from terminal without restart");
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn restart_rejected_from_running() {
        let (_dir, registry) = registry();
        let record = registry
            .create(create_request("s1", "sleep"))
            .await
            .expect("create");
        registry
            .mark_running(&record.task_id, None)
            .await
            .expect("running");
        let err = registry
            .request_restart(&record.task_id)
            .await
            .expect_err("restart from running");
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn finish_from_queued_stamps_started_at_for_symmetry() {
        let (_dir, registry) = registry();
        let record = registry
            .create(create_request("s1", "noop"))
            .await
            .expect("create");
        // Skip mark_running entirely — terminate straight from Queued.
        let failed = registry
            .mark_failed(
                &record.task_id,
                Some(1),
                None,
                Some("permission denied".into()),
            )
            .await
            .expect("failed");
        assert_eq!(failed.status, LocalShellTaskStatus::Failed);
        let attempt = failed
            .current_attempt
            .as_ref()
            .expect("synthetic attempt created");
        let start = attempt.started_at.expect("started_at stamped");
        let finish = attempt.finished_at.expect("finished_at stamped");
        assert!(start <= finish);
        assert_eq!(attempt.attempt, 1);
    }

    #[tokio::test]
    async fn reconcile_log_offset_recovers_unrecorded_bytes() {
        let (dir, registry) = registry();
        let record = registry
            .create(create_request("s1", "dev-server"))
            .await
            .expect("create");
        registry
            .mark_running(&record.task_id, Some(7))
            .await
            .expect("running");
        registry
            .append_output(&record.task_id, b"first\n")
            .await
            .expect("first chunk");

        // Simulate a crash between `append_output`'s file write and the
        // record save: write extra bytes to the log file directly.
        let log_path = dir
            .path()
            .join("local_shell_tasks/logs")
            .join(format!("{}.log", record.task_id));
        let mut handle = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .await
            .expect("open log");
        tokio::io::AsyncWriteExt::write_all(&mut handle, b"crashed-tail\n")
            .await
            .expect("append");
        tokio::io::AsyncWriteExt::flush(&mut handle)
            .await
            .expect("flush");
        drop(handle);

        let before = registry.load(&record.task_id).await.expect("reload");
        assert_eq!(before.output_bytes, 6);

        let reconciled = registry
            .reconcile_log_offset(&record.task_id)
            .await
            .expect("reconcile");
        assert_eq!(reconciled.output_bytes, 19);
        assert_eq!(
            reconciled
                .current_attempt
                .as_ref()
                .expect("attempt")
                .log_byte_end,
            19
        );
    }

    #[tokio::test]
    async fn list_for_session_filters_and_sorts() {
        let (_dir, registry) = registry();
        let one = registry
            .create(create_request("s1", "one"))
            .await
            .expect("one");
        let two = registry
            .create(create_request("s1", "two"))
            .await
            .expect("two");
        let other = registry
            .create(create_request("s2", "elsewhere"))
            .await
            .expect("other");

        let s1 = registry.list_for_session("s1").await.expect("list");
        let ids: Vec<_> = s1.iter().map(|r| r.task_id.clone()).collect();
        assert!(ids.contains(&one.task_id));
        assert!(ids.contains(&two.task_id));
        assert!(!ids.contains(&other.task_id));
        assert_eq!(ids.len(), 2);
    }

    #[tokio::test]
    async fn snapshot_separates_stdout_and_stderr() {
        let (_dir, registry) = registry();
        let record = registry
            .create(create_request("s1", "build"))
            .await
            .expect("create");
        registry
            .mark_running(&record.task_id, Some(1))
            .await
            .expect("running");
        registry
            .append_stdout(&record.task_id, b"out-1\n")
            .await
            .expect("stdout");
        registry
            .append_stderr(&record.task_id, b"err-1\n")
            .await
            .expect("stderr");
        registry
            .append_stdout(&record.task_id, b"out-2\n")
            .await
            .expect("stdout 2");

        let snap = registry.snapshot(&record.task_id).await.expect("snapshot");
        assert_eq!(snap.status, LocalShellTaskStatus::Running);
        assert_eq!(snap.stdout, b"out-1\nout-2\n");
        assert_eq!(snap.stderr, b"err-1\n");
        assert!(!snap.stdout_truncated);
        assert!(!snap.stderr_truncated);
        // Durable log captured every byte from both streams.
        assert_eq!(snap.output_bytes, 18);
    }

    #[tokio::test]
    async fn bounded_buffer_retains_tail_and_flags_truncation() {
        let dir = TempDir::new().expect("tempdir");
        // 8-byte cap per stream forces truncation.
        let registry = LocalShellTaskRegistry::with_buffer_cap(dir.path(), 8);
        let record = registry
            .create(create_request("s1", "chatty"))
            .await
            .expect("create");
        registry
            .mark_running(&record.task_id, None)
            .await
            .expect("running");
        registry
            .append_stdout(&record.task_id, b"0123456789ABCDEF")
            .await
            .expect("append");

        let snap = registry.snapshot(&record.task_id).await.expect("snapshot");
        // Only the trailing 8 bytes survive in memory.
        assert_eq!(snap.stdout, b"89ABCDEF");
        assert!(snap.stdout_truncated);
        assert_eq!(snap.stdout_dropped_bytes, 8);
        // The durable on-disk log keeps the full 16 bytes regardless.
        assert_eq!(snap.output_bytes, 16);
        let (full, _) = registry
            .read_output_from(&record.task_id, 0, 1024)
            .await
            .expect("read full log");
        assert_eq!(full, b"0123456789ABCDEF");
    }

    #[tokio::test]
    async fn concurrent_appends_are_lossless() {
        let (_dir, registry) = registry();
        let record = registry
            .create(create_request("s1", "parallel"))
            .await
            .expect("create");
        registry
            .mark_running(&record.task_id, None)
            .await
            .expect("running");

        let writers = 8u64;
        let per_writer = 50usize;
        let mut handles = Vec::new();
        for _ in 0..writers {
            let registry = registry.clone();
            let task_id = record.task_id.clone();
            handles.push(tokio::spawn(async move {
                for _ in 0..per_writer {
                    registry
                        .append_stdout(&task_id, b"x")
                        .await
                        .expect("append");
                }
            }));
        }
        for handle in handles {
            handle.await.expect("join");
        }

        let expected = writers * per_writer as u64;
        let snap = registry.snapshot(&record.task_id).await.expect("snapshot");
        // No bytes were lost on disk despite concurrent producers.
        assert_eq!(snap.output_bytes, expected);
        // The in-memory buffer (default cap) also holds every byte here.
        assert_eq!(snap.stdout.len() as u64, expected);
        assert!(snap.stdout.iter().all(|&b| b == b'x'));
    }

    #[tokio::test]
    async fn cancel_is_idempotent_and_safe_after_exit() {
        let (_dir, registry) = registry();
        let record = registry
            .create(create_request("s1", "long"))
            .await
            .expect("create");
        assert!(!registry.is_cancel_requested(&record.task_id));

        registry
            .mark_running(&record.task_id, Some(99))
            .await
            .expect("running");
        registry
            .request_cancel(&record.task_id)
            .await
            .expect("cancel");
        assert!(registry.is_cancel_requested(&record.task_id));
        // Idempotent: a second request is a no-op, not an error.
        registry
            .request_cancel(&record.task_id)
            .await
            .expect("cancel again");
        assert!(registry.is_cancel_requested(&record.task_id));

        // Drive the task to a terminal state, then cancel again: safe no-op.
        registry
            .mark_interrupted(&record.task_id, None, Some(9), Some("user".into()))
            .await
            .expect("interrupted");
        registry
            .request_cancel(&record.task_id)
            .await
            .expect("cancel after exit");
        assert!(registry.is_cancel_requested(&record.task_id));
    }

    #[tokio::test]
    async fn unknown_id_queries_error_or_default() {
        let (_dir, registry) = registry();
        let err = registry
            .snapshot("does-not-exist")
            .await
            .expect_err("snapshot unknown id");
        assert!(matches!(err, ToolError::InvalidInput(_)));

        let err = registry
            .request_cancel("does-not-exist")
            .await
            .expect_err("cancel unknown id");
        assert!(matches!(err, ToolError::InvalidInput(_)));

        let err = registry
            .append_stdout("does-not-exist", b"data")
            .await
            .expect_err("append unknown id");
        assert!(matches!(err, ToolError::InvalidInput(_)));

        // Pure in-memory query for an unknown id is a safe `false`.
        assert!(!registry.is_cancel_requested("does-not-exist"));
    }
}
