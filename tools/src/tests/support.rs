#[cfg(all(test, target_os = "linux"))]
use crate::bash::{LINUX_BUBBLEWRAP, executable_in_path};
use crate::task_tools::{
    BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus, write_background_task_record,
};
use crate::*;

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::Utc;
use orbcode_mcp::McpRegistry;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[derive(Default)]
pub(super) struct RecordingProgressReporter {
    pub(super) records: Mutex<Vec<Value>>,
}

#[async_trait]
impl ToolProgressReporter for RecordingProgressReporter {
    async fn report(&self, progress: Value) -> Result<(), ToolError> {
        self.records.lock().await.push(progress);
        Ok(())
    }
}

pub(super) fn test_root(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("current time")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "orbcode-tools-{label}-{}-{unique}",
        std::process::id()
    ))
}

pub(super) fn init_test_git_repo(cwd: &std::path::Path) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["init", "-b", "main"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("invoke git init");
    assert!(status.success(), "git init failed in {}", cwd.display());
}

pub(super) fn os_args_contain(args: &[OsString], needle: &[&str]) -> bool {
    args.windows(needle.len()).any(|window| {
        window
            .iter()
            .map(|value| value.to_string_lossy())
            .eq(needle.iter().copied())
    })
}

pub(super) async fn test_context(label: &str) -> ToolContext {
    let root = test_root(label);
    let cwd = root.join("cwd");
    let home = root.join("home");
    std::fs::create_dir_all(&cwd).expect("create cwd");
    std::fs::create_dir_all(&home).expect("create home");
    let mcp = McpRegistry::load(&home, &cwd).await.expect("load mcp");
    ToolContext {
        cwd,
        additional_directories: Vec::new(),
        home_dir: home,
        sandbox_mode: SandboxMode::DangerFullAccess,
        sandbox_allow_network: true,
        allow_network: true,
        allow_tools: true,
        mcp,
        progress: None,
        cancellation: ToolCancellationToken::default(),
        read_state: None,
        session_id: Some(label.to_string()),
        local_shell_tasks: None,
        on_cwd_change: None,
        plans_directory_override: None,
        ask_user_tx: None,
        settings_env: std::collections::BTreeMap::new(),
        skill_definitions: None,
    }
}

pub(super) async fn context_with_read_state(label: &str) -> ToolContext {
    let mut context = test_context(label).await;
    context.read_state = Some(Arc::new(FileReadState::new()));
    context
}

pub(super) fn advance_mtime(path: &std::path::Path, secs: u64) {
    let future = SystemTime::now() + Duration::from_secs(secs);
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open for mtime bump");
    file.set_modified(future).expect("set mtime");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn loopback_acceptor() -> (u16, std::sync::mpsc::Receiver<bool>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    listener
        .set_nonblocking(true)
        .expect("set listener nonblocking");
    let port = listener.local_addr().expect("listener address").port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            match listener.accept() {
                Ok((_stream, _addr)) => {
                    let _ = tx.send(true);
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        let _ = tx.send(false);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => {
                    let _ = tx.send(false);
                    return;
                }
            }
        }
    });
    (port, rx)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(super) fn sandbox_host_validation_forced(env_var: &str) -> bool {
    std::env::var(env_var)
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(super) fn skip_or_fail_sandbox_host_validation(
    env_var: &str,
    label: &str,
    reason: impl AsRef<str>,
) {
    let reason = reason.as_ref();
    if sandbox_host_validation_forced(env_var) {
        panic!("{reason}");
    }
    eprintln!("skipping {label} sandbox host validation: {reason}");
}

#[cfg(target_os = "linux")]
pub(super) const LINUX_SANDBOX_HOST_TESTS_ENV: &str = "ORBCODE_RUN_LINUX_SANDBOX_HOST_TESTS";

#[cfg(target_os = "linux")]
pub(super) fn skip_or_fail_linux_host_validation(reason: impl AsRef<str>) {
    skip_or_fail_sandbox_host_validation(LINUX_SANDBOX_HOST_TESTS_ENV, "Linux bubblewrap", reason);
}

#[cfg(target_os = "linux")]
pub(super) async fn linux_bubblewrap_host_context(
    label: &str,
    needs_python: bool,
) -> Option<ToolContext> {
    if executable_in_path(LINUX_BUBBLEWRAP).await.is_none() {
        skip_or_fail_linux_host_validation("`bwrap` was not found in PATH");
        return None;
    }
    if needs_python && executable_in_path("python3").await.is_none() {
        skip_or_fail_linux_host_validation("`python3` was not found in PATH");
        return None;
    }

    let mut context = test_context(label).await;
    context.sandbox_mode = SandboxMode::ReadOnly;
    context.sandbox_allow_network = false;
    let registry = ToolRegistry::foundation();
    match registry
        .invoke("bash", r#"{"command":"printf probe"}"#, &context)
        .await
    {
        Ok(result) if result.output == "probe" => Some(context),
        Ok(result) => {
            skip_or_fail_linux_host_validation(format!(
                "bubblewrap probe returned unexpected output `{}`",
                result.output
            ));
            None
        }
        Err(error) => {
            skip_or_fail_linux_host_validation(format!("bubblewrap probe failed: {error}"));
            None
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) const MACOS_SANDBOX_HOST_TESTS_ENV: &str = "ORBCODE_RUN_MACOS_SANDBOX_HOST_TESTS";

#[cfg(target_os = "macos")]
pub(super) const MACOS_SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

#[cfg(target_os = "macos")]
pub(super) fn skip_or_fail_macos_host_validation(reason: impl AsRef<str>) {
    skip_or_fail_sandbox_host_validation(MACOS_SANDBOX_HOST_TESTS_ENV, "macOS seatbelt", reason);
}

#[cfg(target_os = "macos")]
pub(super) async fn macos_seatbelt_host_context(label: &str) -> Option<ToolContext> {
    if !std::path::Path::new(MACOS_SANDBOX_EXEC_PATH).exists() {
        skip_or_fail_macos_host_validation(format!(
            "`{MACOS_SANDBOX_EXEC_PATH}` is not available on this host"
        ));
        return None;
    }
    let mut context = test_context(label).await;
    context.sandbox_mode = SandboxMode::ReadOnly;
    context.sandbox_allow_network = false;
    match ToolRegistry::foundation()
        .invoke("bash", r#"{"command":"printf probe"}"#, &context)
        .await
    {
        Ok(result) if result.output == "probe" => Some(context),
        Ok(result) => {
            skip_or_fail_macos_host_validation(format!(
                "seatbelt probe returned unexpected output `{}`",
                result.output
            ));
            None
        }
        Err(error) => {
            skip_or_fail_macos_host_validation(format!("seatbelt probe failed: {error}"));
            None
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) const WINDOWS_SANDBOX_HOST_TESTS_ENV: &str = "ORBCODE_RUN_WINDOWS_SANDBOX_HOST_TESTS";

#[cfg(target_os = "windows")]
pub(super) fn skip_or_fail_windows_host_validation(reason: impl AsRef<str>) {
    skip_or_fail_sandbox_host_validation(WINDOWS_SANDBOX_HOST_TESTS_ENV, "Windows sandbox", reason);
}

#[cfg(target_os = "windows")]
pub(super) async fn windows_sandbox_host_context(label: &str) -> Option<ToolContext> {
    if crate::bash::windows_sandbox_runner_path().is_none() {
        skip_or_fail_windows_host_validation(
            "`orbcode-windows-sandbox-runner` was not found on PATH and \
             ORBCODE_WINDOWS_SANDBOX_RUNNER does not point to an executable",
        );
        return None;
    }
    let mut context = test_context(label).await;
    context.sandbox_mode = SandboxMode::ReadOnly;
    context.sandbox_allow_network = false;
    match ToolRegistry::foundation()
        .invoke("bash", r#"{"command":"Write-Output probe"}"#, &context)
        .await
    {
        Ok(result) if result.output.trim() == "probe" => Some(context),
        Ok(result) => {
            skip_or_fail_windows_host_validation(format!(
                "Windows sandbox runner probe returned unexpected output `{}`",
                result.output
            ));
            None
        }
        Err(error) => {
            skip_or_fail_windows_host_validation(format!(
                "Windows sandbox runner probe failed: {error}"
            ));
            None
        }
    }
}

pub(super) async fn seed_background_job(
    context: &ToolContext,
    task_id: &str,
    status: BackgroundTaskStatus,
    pid: Option<u32>,
    log: &str,
) -> BackgroundTaskRecord {
    let logs_dir = context.home_dir.join("background").join("logs");
    std::fs::create_dir_all(&logs_dir).expect("create background logs");
    let log_path = logs_dir.join(format!("{task_id}.log"));
    std::fs::write(&log_path, log).expect("write background log");
    let now = Utc::now().to_rfc3339();
    let record = BackgroundTaskRecord {
        job_id: task_id.to_string(),
        session_id: "session-1".to_string(),
        prompt: format!("Background job {task_id}"),
        cwd: context.cwd.display().to_string(),
        status,
        created_at: now.clone(),
        updated_at: now,
        started_at: None,
        finished_at: None,
        pid,
        log_path: log_path.display().to_string(),
        error: None,
        task_kind: BackgroundTaskKind::BackgroundJob,
        tool_use_id: None,
        child_session_id: None,
        agent_type: None,
        model: None,
        permission_mode: None,
        result: None,
        exit_code: None,
        signal: None,
        extra: serde_json::Map::new(),
    };
    write_background_task_record(&context.home_dir, &record)
        .await
        .expect("write background record");
    record
}

// ---------------------------------------------------------------------------
// SearchEngineLock — serializes tests that toggle the process-global
// ripgrep fallback / simulated-rg state.
// ---------------------------------------------------------------------------

static SEARCH_ENGINE_GUARD: StdMutex<()> = StdMutex::new(());

pub(super) struct SearchEngineLock<'a> {
    _guard: MutexGuard<'a, ()>,
    fallback: bool,
    simulated: bool,
}

impl<'a> SearchEngineLock<'a> {
    fn acquire() -> MutexGuard<'static, ()> {
        SEARCH_ENGINE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn ripgrep() -> SearchEngineLock<'static> {
        let guard = Self::acquire();
        crate::grep_tool::set_force_fallback_for_tests(false);
        crate::grep_tool::set_simulated_ripgrep_for_tests(None);
        SearchEngineLock {
            _guard: guard,
            fallback: false,
            simulated: false,
        }
    }

    pub(super) fn fallback() -> SearchEngineLock<'static> {
        let guard = Self::acquire();
        crate::grep_tool::set_force_fallback_for_tests(true);
        crate::grep_tool::set_simulated_ripgrep_for_tests(None);
        SearchEngineLock {
            _guard: guard,
            fallback: true,
            simulated: false,
        }
    }

    pub(super) fn simulate_rg_timeout() -> SearchEngineLock<'static> {
        let guard = Self::acquire();
        crate::grep_tool::set_force_fallback_for_tests(false);
        crate::grep_tool::set_simulated_ripgrep_for_tests(Some(
            crate::grep_tool::SimulatedRipgrep::Timeout,
        ));
        SearchEngineLock {
            _guard: guard,
            fallback: false,
            simulated: true,
        }
    }

    pub(super) fn simulate_rg_success(stdout: &'static str) -> SearchEngineLock<'static> {
        let guard = Self::acquire();
        crate::grep_tool::set_force_fallback_for_tests(false);
        crate::grep_tool::set_simulated_ripgrep_for_tests(Some(
            crate::grep_tool::SimulatedRipgrep::Success(stdout),
        ));
        SearchEngineLock {
            _guard: guard,
            fallback: false,
            simulated: true,
        }
    }

    pub(super) fn simulate_rg_exit(code: i32) -> SearchEngineLock<'static> {
        let guard = Self::acquire();
        crate::grep_tool::set_force_fallback_for_tests(false);
        crate::grep_tool::set_simulated_ripgrep_for_tests(Some(
            crate::grep_tool::SimulatedRipgrep::Exit(code),
        ));
        SearchEngineLock {
            _guard: guard,
            fallback: false,
            simulated: true,
        }
    }
}

impl<'a> Drop for SearchEngineLock<'a> {
    fn drop(&mut self) {
        if self.fallback {
            crate::grep_tool::set_force_fallback_for_tests(false);
        }
        if self.simulated {
            crate::grep_tool::set_simulated_ripgrep_for_tests(None);
        }
    }
}

// ---------------------------------------------------------------------------
// WebCacheLock — serializes the tests that touch the process-global web
// cache / domain policy / network counter.
// ---------------------------------------------------------------------------

pub(super) struct WebCacheLock {
    _guard: MutexGuard<'static, ()>,
}

impl WebCacheLock {
    pub(super) fn acquire() -> WebCacheLock {
        let guard = crate::web_cache::TEST_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::web_cache::reset_for_tests();
        crate::web_fetch::WEB_FETCH_NETWORK_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        WebCacheLock { _guard: guard }
    }

    pub(super) fn network_calls(&self) -> usize {
        crate::web_fetch::WEB_FETCH_NETWORK_CALLS.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for WebCacheLock {
    fn drop(&mut self) {
        crate::web_cache::reset_for_tests();
    }
}

pub(super) fn seed_cache(url: &str, content: &str) {
    crate::web_cache::store(
        url,
        crate::web_cache::CachedContent {
            content: content.to_string(),
            final_url: url.to_string(),
            status_code: 200,
            content_type: "text/html".to_string(),
            converted_to_markdown: true,
            redirected: false,
            redirect_count: 0,
            truncated: false,
            response_bytes: content.len(),
        },
        // Seed under the active (global) domain policy, matching what the web
        // fetch path uses at lookup time in these single-policy tests.
        None,
    );
}

// ---------------------------------------------------------------------------
// http_fixture_server — lightweight local HTTP server for web tool tests.
// ---------------------------------------------------------------------------

pub(super) async fn http_fixture_server(
    responses: Vec<(&'static str, &'static str)>,
) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture server");
    let port = listener.local_addr().expect("listener address").port();

    let handle = tokio::spawn(async move {
        for (expected_path_prefix, response) in responses {
            let Ok((stream, _addr)) = listener.accept().await else {
                break;
            };
            let (reader, mut writer) = stream.into_split();
            let mut buf_reader = BufReader::new(reader);
            let mut request_line = String::new();
            let _ = buf_reader.read_line(&mut request_line).await;

            if !expected_path_prefix.is_empty() {
                assert!(
                    request_line.contains(expected_path_prefix),
                    "expected path `{expected_path_prefix}` in request, got `{request_line}`"
                );
            }

            let mut headers_done = false;
            while !headers_done {
                let mut line = String::new();
                let _ = buf_reader.read_line(&mut line).await;
                if line == "\r\n" || line == "\n" || line.is_empty() {
                    headers_done = true;
                }
            }

            let _ = writer.write_all(response.as_bytes()).await;
            let _ = writer.shutdown().await;
        }
    });

    (port, handle)
}

pub(super) fn http_response(status: u16, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} OK\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    )
}
