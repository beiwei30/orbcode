//! Real-subprocess integration coverage for `LocalShellTaskRegistry`.
//!
//! These tests spawn an actual child process and drive the registry exactly
//! the way the Bash tool does: open a task, `mark_running` with the live PID,
//! stream the child's stdout into the durable log, request cancellation, kill
//! the process, then transition to a terminal state. A fresh registry rooted at
//! the same home directory must recover the status, PID, exit/signal, kill
//! metadata, and byte offsets — proving the record survives process exit.

#![cfg(unix)]

use std::process::Stdio;
use std::time::Duration;

use orbcode_tools::{CreateLocalShellTask, LocalShellTaskRegistry, LocalShellTaskStatus};
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::{sleep, timeout};

/// Drive a real child through stream + cancel + kill, then confirm a brand-new
/// registry instance recovers the durable terminal record from disk.
#[tokio::test]
async fn subprocess_stream_cancel_and_reload_is_durable() {
    let home = TempDir::new().expect("home tempdir");
    let registry = LocalShellTaskRegistry::new(home.path());

    let task = registry
        .create(CreateLocalShellTask {
            session_id: "subprocess-session".to_string(),
            command: "printf 'streamed-line\\n'; sleep 30".to_string(),
            cwd: std::env::temp_dir(),
            label: None,
        })
        .await
        .expect("create task");
    let task_id = task.task_id.clone();

    // Spawn a child that emits a known line and then blocks, so the pipe stays
    // open while we observe live state and then interrupt it.
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("printf 'streamed-line\\n'; sleep 30")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn child");
    let pid = child.id();
    assert!(pid.is_some(), "child should expose a pid");

    registry
        .mark_running(&task_id, pid)
        .await
        .expect("mark running");

    // Stream the first line of stdout into the durable log.
    let mut stdout = child.stdout.take().expect("child stdout");
    let streamed = timeout(Duration::from_secs(5), read_line(&mut stdout))
        .await
        .expect("read child line within timeout");
    registry
        .append_stdout(&task_id, &streamed)
        .await
        .expect("append stdout");

    // Live snapshot reflects running status and the streamed bytes.
    let snapshot = registry.snapshot(&task_id).await.expect("snapshot");
    assert_eq!(snapshot.status, LocalShellTaskStatus::Running);
    assert_eq!(snapshot.stdout, b"streamed-line\n");
    assert_eq!(snapshot.output_bytes, streamed.len() as u64);

    // Esc / TaskStop equivalent: record intent, then perform a real kill.
    registry
        .request_cancel(&task_id)
        .await
        .expect("request cancel");
    assert!(registry.is_cancel_requested(&task_id));
    registry
        .mark_interrupting(&task_id, "SIGTERM", Some("interrupted by user"))
        .await
        .expect("mark interrupting");
    child.start_kill().expect("send kill");
    let status = timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("child exits after kill")
        .expect("wait status");
    let signal = exit_signal(&status);
    registry
        .mark_interrupted(
            &task_id,
            status.code(),
            signal,
            Some("interrupted by user".to_string()),
        )
        .await
        .expect("mark interrupted");

    // Rehydrate from disk with a fresh registry: durable truth must survive.
    drop(registry);
    let resumed = LocalShellTaskRegistry::new(home.path());
    let loaded = resumed.load(&task_id).await.expect("reload record");
    assert_eq!(loaded.status, LocalShellTaskStatus::Interrupted);
    assert_eq!(loaded.kill_signal_sent.as_deref(), Some("SIGTERM"));
    assert_eq!(loaded.output_bytes, streamed.len() as u64);

    let attempt = loaded.current_attempt.as_ref().expect("attempt recorded");
    assert_eq!(attempt.pid, pid);
    assert_eq!(attempt.signal, signal);
    assert_eq!(attempt.kill_reason.as_deref(), Some("interrupted by user"));
    assert_eq!(
        attempt.terminal_status,
        Some(LocalShellTaskStatus::Interrupted)
    );
    assert_eq!(attempt.log_byte_end, streamed.len() as u64);

    // The durable on-disk log replays the streamed bytes from offset zero even
    // though the in-memory scrollback was lost with the original registry.
    let (replayed, _) = resumed
        .read_output_from(&task_id, 0, 1024)
        .await
        .expect("replay log");
    assert_eq!(replayed, b"streamed-line\n");
}

/// A child that exits cleanly drives the registry to `Succeeded`, and the exit
/// code persists for a follower that loads the record fresh.
#[tokio::test]
async fn subprocess_success_records_exit_code() {
    let home = TempDir::new().expect("home tempdir");
    let registry = LocalShellTaskRegistry::new(home.path());

    let task = registry
        .create(CreateLocalShellTask {
            session_id: "subprocess-success".to_string(),
            command: "printf done".to_string(),
            cwd: std::env::temp_dir(),
            label: None,
        })
        .await
        .expect("create task");
    let task_id = task.task_id.clone();

    let mut child = Command::new("sh")
        .arg("-c")
        .arg("printf done")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn child");
    registry
        .mark_running(&task_id, child.id())
        .await
        .expect("mark running");

    let mut stdout = child.stdout.take().expect("child stdout");
    let mut captured = Vec::new();
    stdout
        .read_to_end(&mut captured)
        .await
        .expect("drain stdout");
    registry
        .append_stdout(&task_id, &captured)
        .await
        .expect("append stdout");
    let status = child.wait().await.expect("wait status");
    registry
        .mark_succeeded(&task_id, status.code().unwrap_or_default())
        .await
        .expect("mark succeeded");

    let resumed = LocalShellTaskRegistry::new(home.path());
    let loaded = resumed.load(&task_id).await.expect("reload record");
    assert_eq!(loaded.status, LocalShellTaskStatus::Succeeded);
    assert_eq!(loaded.output_bytes, captured.len() as u64);
    assert_eq!(
        loaded
            .current_attempt
            .as_ref()
            .and_then(|attempt| attempt.exit_code),
        Some(0)
    );
}

/// Read from a child pipe until a newline arrives (or the pipe closes).
async fn read_line<R>(reader: &mut R) -> Vec<u8>
where
    R: AsyncReadExt + Unpin,
{
    let mut out = Vec::new();
    let mut buf = [0u8; 64];
    loop {
        let read = reader.read(&mut buf).await.expect("read child stdout");
        if read == 0 {
            break;
        }
        out.extend_from_slice(&buf[..read]);
        if out.contains(&b'\n') {
            break;
        }
        // Yield briefly so a slow writer can flush the rest of the line.
        sleep(Duration::from_millis(5)).await;
    }
    out
}

fn exit_signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}
