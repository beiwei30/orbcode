//! PTY-level smoke tests for embedded and remote TUI startup.
//!
//! These drive a real pseudo-terminal child (an embedded TUI, or a `serve`
//! process plus a remote TUI) and assert on first-frame render timing and clean
//! teardown. They pass reliably when run on their own, but under the CPU
//! saturation of a full `cargo test --workspace` the harness's blocking PTY I/O
//! and child reaping can stall past the per-step deadlines and hang the run.
//! They are therefore `#[ignore]`d by default and excluded from the standard
//! workspace run; execute them explicitly (ideally single-threaded, uncontended):
//!
//! ```text
//! cargo test -p orbcode --test tui_remote_pty_e2e -- --ignored --test-threads=1
//! ```

#![cfg(unix)]

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use orbcode_app_server_client::AppClient;
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

/// Serializes the PTY smoke tests against each other. Each test spawns a real
/// pseudo-terminal child (an embedded TUI, or a `serve` process plus a remote
/// TUI) and asserts on first-frame render timing. Running them concurrently —
/// with each other and with the rest of `cargo test --workspace` — saturates the
/// CPU and pushes first render past its deadline, producing flaky timeouts.
/// Holding this async lock for the whole test keeps only one PTY child alive at
/// a time from this binary, so the timing assertions stay deterministic.
fn pty_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

struct TestEnv {
    home: TempDir,
    cwd: TempDir,
}

impl TestEnv {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("home tempdir");
        let cwd = tempfile::tempdir().expect("cwd tempdir");

        std::fs::write(
            home.path().join("settings.json"),
            r#"{"env":{"ANTHROPIC_API_KEY":"stub-key"}}"#,
        )
        .expect("write settings");

        Self { home, cwd }
    }
}

struct PtyTui {
    child: Child,
    master: File,
    output: Vec<u8>,
    /// How many Device Status Report (cursor-position, `ESC[6n`) queries from the
    /// child we have already answered. `setup_terminal` calls
    /// `crossterm::cursor::position()` before the first render, which emits
    /// `ESC[6n` and blocks reading the reply; a real terminal answers with
    /// `ESC[<row>;<col>R`. This harness must do the same or the child hangs
    /// forever waiting for a cursor-position report.
    dsr_replies: usize,
}

impl PtyTui {
    fn spawn(args: &[String], env: &TestEnv) -> Self {
        let mut master: RawFd = -1;
        let mut slave: RawFd = -1;
        let mut winsize = libc::winsize {
            ws_row: 30,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut winsize,
            )
        };
        assert_eq!(result, 0, "openpty: {}", io::Error::last_os_error());
        set_nonblocking(master);

        let stdin_fd = dup_fd(slave);
        let stdout_fd = dup_fd(slave);
        let stderr_fd = dup_fd(slave);

        let mut command = Command::new(ORBCODE_BIN);
        command
            .args(args)
            .current_dir(env.cwd.path())
            .env("ORBCODE_HOME", env.home.path())
            .env("HOME", env.home.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("RUST_LOG", "warn")
            .env("TERM", "xterm-256color")
            .env("ORBCODE_TUI_PTY_SMOKE_EXIT_AFTER_FIRST_FRAME", "1")
            .stdin(unsafe { Stdio::from(File::from_raw_fd(stdin_fd)) })
            .stdout(unsafe { Stdio::from(File::from_raw_fd(stdout_fd)) })
            .stderr(unsafe { Stdio::from(File::from_raw_fd(stderr_fd)) });

        let slave_for_ctty = slave;
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(slave_for_ctty, libc::TIOCSCTTY.into(), 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let child = command.spawn().expect("spawn orbcode under pty");

        unsafe {
            libc::close(slave);
        }

        Self {
            child,
            master: unsafe { File::from_raw_fd(master) },
            output: Vec::new(),
            dsr_replies: 0,
        }
    }

    fn wait_for_initial_render(&mut self) -> String {
        // Generous deadline: the child renders the first frame in a couple of
        // seconds unloaded, but a full `--workspace` run can slow startup by an
        // order of magnitude. Fail slow rather than flaky.
        let output = self.read_until(Duration::from_secs(60), |text| {
            text.contains("\u{1b}[J")
                && text.contains("\u{1b}[?7l")
                && text.contains("\u{1b}[?7h")
                && text.contains("Claude Code")
        });
        assert!(
            output.contains("\u{1b}[J") && output.contains("Claude Code"),
            "TUI did not clear and paint visible content. Output:\n{output}"
        );
        output
    }

    fn wait_for_clean_exit(mut self) {
        let status = self
            .wait_timeout(Duration::from_secs(30))
            .expect("TUI smoke exits after first frame");
        let output = self.read_available_for(Duration::from_millis(250));
        assert!(
            status.success(),
            "TUI exited with {status}. Output:\n{output}"
        );
    }

    fn read_until<F>(&mut self, timeout: Duration, predicate: F) -> String
    where
        F: Fn(&str) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            self.read_available_once();
            let text = String::from_utf8_lossy(&self.output).into_owned();
            if predicate(&text) {
                return text;
            }
            if let Some(status) = self.child.try_wait().expect("poll child") {
                panic!("TUI exited before expected PTY output: {status}\n{text}");
            }
            if let Some(signal) = stopped_signal(self.child.id()) {
                panic!("TUI stopped by signal {signal} before expected PTY output\n{text}");
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for PTY output. Output:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn read_available_for(&mut self, duration: Duration) -> String {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.read_available_once();
            std::thread::sleep(Duration::from_millis(10));
        }
        String::from_utf8_lossy(&self.output).into_owned()
    }

    fn read_available_once(&mut self) {
        let mut buf = [0_u8; 4096];
        loop {
            match self.master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => self.output.extend_from_slice(&buf[..n]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                Err(error) => panic!("read pty output: {error}"),
            }
        }
        self.answer_cursor_position_queries();
    }

    /// Reply to any not-yet-answered `ESC[6n` cursor-position queries the child
    /// has emitted, mimicking a real terminal's `ESC[<row>;<col>R` report. Row 1
    /// / col 1 is a valid answer for a freshly cleared screen and is all
    /// `setup_terminal` needs to proceed past `crossterm::cursor::position()`.
    fn answer_cursor_position_queries(&mut self) {
        let query_count = self.output.windows(4).filter(|w| w == b"\x1b[6n").count();
        while self.dsr_replies < query_count {
            if self.master.write_all(b"\x1b[1;1R").is_err() {
                break;
            }
            let _ = self.master.flush();
            self.dsr_replies += 1;
        }
    }

    fn wait_timeout(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll child") {
                return Some(status);
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for PtyTui {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn dup_fd(fd: RawFd) -> RawFd {
    let dup = unsafe { libc::dup(fd) };
    assert!(dup >= 0, "dup: {}", io::Error::last_os_error());
    dup
}

fn set_nonblocking(fd: RawFd) {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert!(flags >= 0, "fcntl F_GETFL: {}", io::Error::last_os_error());
    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    assert_eq!(
        result,
        0,
        "fcntl F_SETFL O_NONBLOCK: {}",
        io::Error::last_os_error()
    );
}

fn stopped_signal(pid: u32) -> Option<i32> {
    let mut status = 0;
    let result = unsafe {
        libc::waitpid(
            pid as libc::pid_t,
            &mut status,
            libc::WNOHANG | libc::WUNTRACED,
        )
    };
    if result == pid as libc::pid_t && libc::WIFSTOPPED(status) {
        Some(libc::WSTOPSIG(status))
    } else {
        None
    }
}

async fn read_connection_info_from<R>(mut reader: R, stream_name: &str) -> Value
where
    R: AsyncBufRead + Unpin,
{
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await.expect("read stream");
            assert!(n != 0, "{stream_name} EOF before connection info");
            if let Ok(value) = serde_json::from_str::<Value>(line.trim())
                && value.get("transport").is_some()
            {
                return value;
            }
        }
    })
    .await
    .expect("connection info JSON within 30s")
}

fn spawn_serve(args: &[&str], env: &TestEnv) -> tokio::process::Child {
    tokio::process::Command::new(ORBCODE_BIN)
        .arg("serve")
        .args(args)
        .current_dir(env.cwd.path())
        .env_clear()
        .env("ORBCODE_HOME", env.home.path())
        .env("HOME", env.home.path())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("RUST_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Reap the server if the test panics before its explicit `start_kill`,
        // so a failed run can't leak a process still holding the listener.
        .kill_on_drop(true)
        .spawn()
        .expect("spawn orbcode serve")
}

async fn read_connection_info(child: &mut tokio::process::Child) -> Value {
    let stdout = child.stdout.take().expect("serve stdout");
    read_connection_info_from(BufReader::new(stdout), "stdout").await
}

async fn assert_websocket_server_accepts_next_client(addr: &str, token: &str) {
    // Bound the reconnect so a server that fails to keep its listener open
    // surfaces as a test failure rather than an unbounded hang of the run.
    let sessions = tokio::time::timeout(Duration::from_secs(30), async {
        let client = AppClient::connect_websocket(&format!("ws://{addr}"), token)
            .await
            .expect("connect websocket after TUI disconnect");
        client.list_sessions().await.expect("list sessions")
    })
    .await
    .expect("reconnect to preserved websocket listener within 30s");
    assert!(
        sessions.is_empty(),
        "fresh server should return an empty session list after remote TUI disconnect: {sessions:?}"
    );
}

async fn assert_socket_server_accepts_next_client(path: &Path, token: &str) {
    // Bound the reconnect so a server that fails to keep its listener open
    // surfaces as a test failure rather than an unbounded hang of the run.
    let sessions = tokio::time::timeout(Duration::from_secs(30), async {
        let client = AppClient::connect_socket(path, token)
            .await
            .expect("connect socket after TUI disconnect");
        client.list_sessions().await.expect("list sessions")
    })
    .await
    .expect("reconnect to preserved socket listener within 30s");
    assert!(
        sessions.is_empty(),
        "fresh server should return an empty session list after remote TUI disconnect: {sessions:?}"
    );
}

#[tokio::test]
#[ignore = "load-sensitive PTY e2e; run explicitly with --ignored (see module docs)"]
async fn embedded_tui_renders_full_screen_and_exits_cleanly() {
    let _serial = pty_test_lock().lock().await;
    let env = TestEnv::new();
    let mut tui = PtyTui::spawn(&["tui".to_string()], &env);

    tui.wait_for_initial_render();
    tui.wait_for_clean_exit();
}

#[tokio::test]
#[ignore = "load-sensitive PTY e2e; run explicitly with --ignored (see module docs)"]
async fn remote_tui_websocket_renders_full_screen_and_preserves_listener() {
    let _serial = pty_test_lock().lock().await;
    let env = TestEnv::new();
    let mut server = spawn_serve(&["--websocket", "127.0.0.1:0"], &env);
    let info = read_connection_info(&mut server).await;
    assert_eq!(info["transport"], "websocket");
    let addr = info["addr"].as_str().expect("addr");
    let token = info["auth_token"].as_str().expect("auth_token");

    let mut tui = PtyTui::spawn(
        &[
            "remote".to_string(),
            format!("ws://{addr}"),
            "--token".to_string(),
            token.to_string(),
        ],
        &env,
    );

    tui.wait_for_initial_render();
    tui.wait_for_clean_exit();
    assert_websocket_server_accepts_next_client(addr, token).await;

    let _ = server.start_kill();
    let _ = server.wait().await;
}

#[tokio::test]
#[ignore = "load-sensitive PTY e2e; run explicitly with --ignored (see module docs)"]
async fn remote_tui_socket_renders_full_screen_and_preserves_listener() {
    let _serial = pty_test_lock().lock().await;
    let env = TestEnv::new();
    let sock_dir = tempfile::tempdir().expect("socket dir");
    let socket_path = sock_dir.path().join("orbcode-tui-remote.sock");
    let mut server = spawn_serve(&["--socket", socket_path.to_str().unwrap()], &env);
    let info = read_connection_info(&mut server).await;
    assert_eq!(info["transport"], "socket");
    let path = PathBuf::from(info["path"].as_str().expect("socket path"));
    let token = info["auth_token"].as_str().expect("auth_token");

    let mut tui = PtyTui::spawn(
        &[
            "remote".to_string(),
            path.display().to_string(),
            "--token".to_string(),
            token.to_string(),
        ],
        &env,
    );

    tui.wait_for_initial_render();
    tui.wait_for_clean_exit();
    assert_socket_server_accepts_next_client(&path, token).await;

    let _ = server.start_kill();
    let _ = server.wait().await;
}
