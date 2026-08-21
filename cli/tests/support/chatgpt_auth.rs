//! Shared black-box ChatGPT auth fixtures.
//!
//! Scenario tests use this module to drive the real `orbcode` executable. The
//! helpers keep services loopback-only, bound every wait, redact recorded
//! credentials, and own child/listener cleanup through `Drop`.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use url::Url;

pub const ISSUER_ENV: &str = "ORBCODE_TEST_OPENAI_ISSUER";
pub const CODEX_BASE_URL_ENV: &str = "ORBCODE_TEST_OPENAI_CODEX_BASE_URL";
pub const CALLBACK_PORTS_ENV: &str = "ORBCODE_TEST_OPENAI_CALLBACK_PORTS";
pub const BROWSER_TIMEOUT_MS_ENV: &str = "ORBCODE_TEST_OPENAI_BROWSER_TIMEOUT_MS";
pub const DEVICE_TIMEOUT_MS_ENV: &str = "ORBCODE_TEST_OPENAI_DEVICE_TIMEOUT_MS";
pub const ORIGINATOR_ENV: &str = "ORBCODE_TEST_OPENAI_ORIGINATOR";

const DEFAULT_DEADLINE: Duration = Duration::from_secs(10);
const CHILD_ENV_TO_CLEAR: &[&str] = &[
    "ORBCODE_ANTHROPIC_API_KEY",
    "ANTHROPIC_API_KEY",
    "ORBCODE_ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_AUTH_TOKEN",
    "ORBCODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ORBCODE_OPENAI_API_KEY",
    "OPENAI_API_KEY",
    "ORBCODE_GEMINI_API_KEY",
    "GEMINI_API_KEY",
    "ORBCODE_XAI_API_KEY",
    "XAI_API_KEY",
    "ORBCODE_GROK_API_KEY",
    "GROK_API_KEY",
    "ORBCODE_ANTHROPIC_BASE_URL",
    "ANTHROPIC_BASE_URL",
    "ORBCODE_OPENAI_BASE_URL",
    "OPENAI_BASE_URL",
    "ORBCODE_PROXY",
    "CLAUDE_CODE_PROXY",
    "ANTHROPIC_PROXY_URL",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    ISSUER_ENV,
    CODEX_BASE_URL_ENV,
    CALLBACK_PORTS_ENV,
    BROWSER_TIMEOUT_MS_ENV,
    DEVICE_TIMEOUT_MS_ENV,
    ORIGINATOR_ENV,
];

#[derive(Clone, Debug)]
pub struct OpenAiTestEnv {
    values: Vec<(&'static str, String)>,
}

impl OpenAiTestEnv {
    pub fn for_server(server: &ScriptedServer) -> Self {
        Self {
            values: vec![
                (ISSUER_ENV, server.base_url().to_string()),
                (
                    CODEX_BASE_URL_ENV,
                    format!("{}/backend-api/codex", server.base_url()),
                ),
                (CALLBACK_PORTS_ENV, "0".to_string()),
                (BROWSER_TIMEOUT_MS_ENV, "3000".to_string()),
                (DEVICE_TIMEOUT_MS_ENV, "3000".to_string()),
                (ORIGINATOR_ENV, "orbcode-harness".to_string()),
            ],
        }
    }

    pub fn set(mut self, name: &'static str, value: impl Into<String>) -> Self {
        let value = value.into();
        if let Some((_, existing)) = self.values.iter_mut().find(|(key, _)| *key == name) {
            *existing = value;
        } else {
            self.values.push((name, value));
        }
        self
    }

    fn apply(&self, command: &mut Command) {
        for (name, value) in &self.values {
            command.env(name, value);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug)]
struct OutputLine {
    stream: OutputStream,
    line: String,
}

/// A real CLI child with live stdout/stderr capture and bounded teardown.
pub struct CliProcess {
    child: Option<Child>,
    root: Arc<TempDir>,
    home: PathBuf,
    cwd: PathBuf,
    output: Arc<Mutex<String>>,
    events: Receiver<OutputLine>,
    readers: Vec<JoinHandle<()>>,
}

impl CliProcess {
    pub fn spawn<I, S>(args: I, env: &OpenAiTestEnv) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let root = Arc::new(tempfile::tempdir().expect("create ChatGPT auth fixture root"));
        let home = root.path().join("home");
        let cwd = root.path().join("cwd");
        fs::create_dir_all(&home).expect("create isolated Orb Code home");
        fs::create_dir_all(&cwd).expect("create isolated child cwd");

        Self::spawn_with_layout(args, env, root, home, cwd)
    }

    /// Spawn against an auth store written before the child starts. Failure
    /// scenarios use this to prove unrelated credentials are byte-preserved.
    pub fn spawn_with_auth<I, S>(args: I, env: &OpenAiTestEnv, auth: &[u8]) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let root = Arc::new(tempfile::tempdir().expect("create ChatGPT auth fixture root"));
        let home = root.path().join("home");
        let cwd = root.path().join("cwd");
        fs::create_dir_all(&home).expect("create isolated Orb Code home");
        fs::create_dir_all(&cwd).expect("create isolated child cwd");
        fs::write(home.join("auth.json"), auth).expect("seed unrelated auth entry");

        Self::spawn_with_layout(args, env, root, home, cwd)
    }

    /// Start another real CLI process against this fixture's isolated home and
    /// cwd. The shared temporary root remains alive until both children drop.
    pub fn spawn_again<I, S>(&self, args: I, env: &OpenAiTestEnv) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        Self::spawn_with_layout(
            args,
            env,
            Arc::clone(&self.root),
            self.home.clone(),
            self.cwd.clone(),
        )
    }

    fn spawn_with_layout<I, S>(
        args: I,
        env: &OpenAiTestEnv,
        root: Arc<TempDir>,
        home: PathBuf,
        cwd: PathBuf,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut command = Command::new(env!("CARGO_BIN_EXE_orbcode"));
        command
            .args(args)
            .current_dir(&cwd)
            .env("ORBCODE_HOME", &home)
            .env("ORBCODE_NO_BROWSER", "1")
            .env_remove("CLAUDE_CONFIG_DIR")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for name in CHILD_ENV_TO_CLEAR {
            command.env_remove(name);
        }
        env.apply(&mut command);

        let mut child = command.spawn().expect("spawn real orbcode binary");
        let stdout = child.stdout.take().expect("capture orbcode stdout");
        let stderr = child.stderr.take().expect("capture orbcode stderr");
        let output = Arc::new(Mutex::new(String::new()));
        let (events_tx, events) = mpsc::channel();
        let readers = vec![
            spawn_output_reader(
                stdout,
                OutputStream::Stdout,
                Arc::clone(&output),
                events_tx.clone(),
            ),
            spawn_output_reader(stderr, OutputStream::Stderr, Arc::clone(&output), events_tx),
        ];

        Self {
            child: Some(child),
            root,
            home,
            cwd,
            output,
            events,
            readers,
        }
    }

    pub fn wait_for_stdout_prefix(&self, prefix: &str, deadline: Duration) -> String {
        self.wait_for_line(OutputStream::Stdout, deadline, |line| {
            line.strip_prefix(prefix).map(str::to_string)
        })
        .unwrap_or_else(|| {
            panic!(
                "orbcode did not print stdout prefix {prefix:?} before the deadline\n{}",
                self.output()
            )
        })
    }

    pub fn wait_for_line<T>(
        &self,
        stream: OutputStream,
        deadline: Duration,
        mut inspect: impl FnMut(&str) -> Option<T>,
    ) -> Option<T> {
        let started = Instant::now();
        loop {
            let remaining = deadline.checked_sub(started.elapsed())?;
            match self.events.recv_timeout(remaining) {
                Ok(event) if event.stream == stream => {
                    if let Some(value) = inspect(event.line.trim_end()) {
                        return Some(value);
                    }
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    pub fn wait_for_exit(&mut self, deadline: Duration) -> Option<ExitStatus> {
        let started = Instant::now();
        loop {
            let child = self.child.as_mut().expect("child still owned");
            match child.try_wait().expect("poll orbcode child") {
                Some(status) => {
                    self.join_readers();
                    return Some(status);
                }
                None if started.elapsed() < deadline => thread::sleep(Duration::from_millis(10)),
                None => return None,
            }
        }
    }

    pub fn assert_success(&mut self) -> ExitStatus {
        let status = self.wait_for_exit(DEFAULT_DEADLINE).unwrap_or_else(|| {
            panic!(
                "orbcode did not exit before the deadline\n{}",
                self.output()
            )
        });
        assert!(
            status.success(),
            "orbcode exited with {status}\n{}",
            self.output()
        );
        status
    }

    pub fn output(&self) -> String {
        self.output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn id(&self) -> u32 {
        self.child.as_ref().expect("child still owned").id()
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    #[cfg(unix)]
    pub fn interrupt(&mut self) {
        if let Some(child) = &mut self.child
            && child
                .try_wait()
                .expect("poll child before SIGINT")
                .is_none()
        {
            // SAFETY: the PID belongs to the live child owned by this fixture.
            let result = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) };
            assert_eq!(result, 0, "send SIGINT to orbcode child");
        }
    }

    fn join_readers(&mut self) {
        for reader in self.readers.drain(..) {
            reader.join().expect("join child output reader");
        }
    }

    fn cleanup(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            #[cfg(unix)]
            // SAFETY: the PID belongs to the live child owned by this fixture.
            unsafe {
                libc::kill(child.id() as libc::pid_t, libc::SIGINT);
            }
            let started = Instant::now();
            while started.elapsed() < Duration::from_millis(300) {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.join_readers();
    }
}

impl Drop for CliProcess {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn spawn_output_reader(
    stream: impl Read + Send + 'static,
    stream_kind: OutputStream,
    output: Arc<Mutex<String>>,
    events: mpsc::Sender<OutputLine>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) != 0 {
            let tagged = format!("[{}] {line}", stream_name(&stream_kind));
            output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push_str(&tagged);
            let _ = events.send(OutputLine {
                stream: stream_kind.clone(),
                line: line.clone(),
            });
            line.clear();
        }
    })
}

fn stream_name(stream: &OutputStream) -> &'static str {
    match stream {
        OutputStream::Stdout => "stdout",
        OutputStream::Stderr => "stderr",
    }
}

#[derive(Clone, Debug)]
pub struct ExpectedRequest {
    method: String,
    path: String,
    required_body_markers: Vec<String>,
    response_status: u16,
    response_content_type: String,
    response_headers: Vec<(String, String)>,
    response_body: String,
    close_without_response: bool,
}

impl ExpectedRequest {
    pub fn json(method: &str, path: &str, response_body: Value) -> Self {
        Self::raw(method, path, "application/json", response_body.to_string())
    }

    pub fn raw(
        method: &str,
        path: &str,
        content_type: &str,
        response_body: impl Into<String>,
    ) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            required_body_markers: Vec::new(),
            response_status: 200,
            response_content_type: content_type.to_string(),
            response_headers: Vec::new(),
            response_body: response_body.into(),
            close_without_response: false,
        }
    }

    pub fn requiring(mut self, marker: &str) -> Self {
        self.required_body_markers.push(marker.to_string());
        self
    }

    pub fn responding_with_status(mut self, status: u16) -> Self {
        self.response_status = status;
        self
    }

    pub fn closing_connection(mut self) -> Self {
        self.close_without_response = true;
        self
    }

    pub fn browser_token_exchange(canaries: &TokenCanaries) -> Self {
        Self::json("POST", "/oauth/token", canaries.token_response())
            .requiring("grant_type=authorization_code")
    }

    pub fn device_user_code(user_code: &str, device_auth_id: &str) -> Self {
        Self::json(
            "POST",
            "/api/accounts/deviceauth/usercode",
            json!({
                "device_auth_id": device_auth_id,
                "user_code": user_code,
                "interval": "1"
            }),
        )
        .requiring("client_id")
    }

    pub fn device_poll(
        user_code: &str,
        device_auth_id: &str,
        authorization_code: &str,
        code_verifier: &str,
        code_challenge: &str,
    ) -> Self {
        Self::json(
            "POST",
            "/api/accounts/deviceauth/token",
            json!({
                "authorization_code": authorization_code,
                "code_verifier": code_verifier,
                "code_challenge": code_challenge
            }),
        )
        .requiring(user_code)
        .requiring(device_auth_id)
    }

    pub fn device_poll_pending(user_code: &str, device_auth_id: &str) -> Self {
        Self::json(
            "POST",
            "/api/accounts/deviceauth/token",
            json!({ "error": "authorization_pending" }),
        )
        .requiring(user_code)
        .requiring(device_auth_id)
        .responding_with_status(403)
    }

    pub fn device_token_exchange(canaries: &TokenCanaries) -> Self {
        Self::browser_token_exchange(canaries)
    }

    pub fn refresh_exchange(canaries: &TokenCanaries) -> Self {
        Self::json("POST", "/oauth/token", canaries.token_response())
            .requiring("grant_type=refresh_token")
    }

    pub fn responses_sse(path: &str, events: &[Value]) -> Self {
        let mut body = events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        body.push_str("data: [DONE]\n\n");
        Self {
            method: "POST".to_string(),
            path: path.to_string(),
            required_body_markers: Vec::new(),
            response_status: 200,
            response_content_type: "text/event-stream".to_string(),
            response_headers: Vec::new(),
            response_body: body,
            close_without_response: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub authorization_header_present: bool,
    pub sanitized_body: String,
    pub received_at: Instant,
    header_names: Vec<String>,
    authorization_bearer_sha256: Option<String>,
    secret_body_fingerprints: HashMap<String, SecretBodyFingerprint>,
}

impl RecordedRequest {
    /// Return a URL-safe SHA-256 digest for a captured OAuth secret field.
    /// The fixture never retains the field's plaintext value.
    pub fn secret_body_sha256(&self, name: &str) -> Option<&str> {
        self.secret_body_fingerprints
            .get(name)
            .map(|fingerprint| fingerprint.sha256.as_str())
    }

    pub fn secret_body_len(&self, name: &str) -> Option<usize> {
        self.secret_body_fingerprints
            .get(name)
            .map(|fingerprint| fingerprint.len)
    }

    /// Return the digest of the bearer token without retaining the token or
    /// full Authorization header in fake-server diagnostics.
    pub fn authorization_bearer_sha256(&self) -> Option<&str> {
        self.authorization_bearer_sha256.as_deref()
    }

    pub fn header_count(&self, name: &str) -> usize {
        self.header_names
            .iter()
            .filter(|candidate| candidate.eq_ignore_ascii_case(name))
            .count()
    }

    /// Inspect a non-sensitive captured header. Sensitive header values are
    /// deliberately omitted from `headers` and can only be checked through a
    /// purpose-built digest or presence accessor.
    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn searchable_metadata(&self) -> String {
        format!(
            "{} {} {:?} {}",
            self.method, self.path, self.headers, self.sanitized_body
        )
    }
}

#[derive(Default)]
struct ServerState {
    remaining: VecDeque<ExpectedRequest>,
    received: Vec<RecordedRequest>,
    failure: Option<String>,
}

/// A loopback HTTP service that consumes requests in one exact scripted order.
pub struct ScriptedServer {
    base_url: String,
    state: Arc<Mutex<ServerState>>,
    recorded_requests: Receiver<RecordedRequest>,
    shutdown: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl ScriptedServer {
    pub fn start(steps: impl IntoIterator<Item = ExpectedRequest>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake OpenAI service");
        listener
            .set_nonblocking(true)
            .expect("make fake OpenAI service nonblocking");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("fake service addr")
        );
        let state = Arc::new(Mutex::new(ServerState {
            remaining: steps.into_iter().collect(),
            ..ServerState::default()
        }));
        let worker_state = Arc::clone(&state);
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let (recorded_tx, recorded_requests) = mpsc::channel();
        let worker = thread::spawn(move || {
            loop {
                if shutdown_rx.try_recv().is_ok() {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        serve_one_request(&mut stream, &worker_state, &recorded_tx)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => {
                        record_server_failure(&worker_state, format!("listener failed: {error}"));
                        break;
                    }
                }
            }
        });
        Self {
            base_url,
            state,
            recorded_requests,
            shutdown: Some(shutdown_tx),
            worker: Some(worker),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn wait_for_request(&self, deadline: Duration) -> RecordedRequest {
        self.wait_for_request_count(1, deadline)
            .into_iter()
            .last()
            .expect("one recorded request")
    }

    pub fn wait_for_request_count(&self, count: usize, deadline: Duration) -> Vec<RecordedRequest> {
        let started = Instant::now();
        let mut requests = Vec::with_capacity(count);
        while requests.len() < count {
            let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
                panic!("fake OpenAI service received no request before the deadline");
            };
            match self.recorded_requests.recv_timeout(remaining) {
                Ok(request) => requests.push(request),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    let state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(failure) = &state.failure {
                        panic!("fake OpenAI service failed: {failure}");
                    }
                    panic!("fake OpenAI service received no request before the deadline");
                }
            }
        }
        requests
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .received
            .clone()
    }

    pub fn assert_no_request(&self, deadline: Duration) {
        match self.recorded_requests.recv_timeout(deadline) {
            Ok(request) => panic!(
                "fake OpenAI service received an unexpected request after terminal failure: {} {}",
                request.method, request.path
            ),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                panic!("fake OpenAI service stopped before the quiet period completed")
            }
        }
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(failure) = &state.failure {
            panic!("fake OpenAI service failed: {failure}");
        }
    }

    pub fn assert_finished(mut self) {
        if let Err(error) = self.verify_finished(DEFAULT_DEADLINE) {
            panic!("fake OpenAI service failed: {error}");
        }
    }

    pub fn verify_finished(&mut self, deadline: Duration) -> Result<(), String> {
        let started = Instant::now();
        loop {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.remaining.is_empty() || state.failure.is_some() {
                break;
            }
            drop(state);
            if started.elapsed() >= deadline {
                self.stop();
                return Err(
                    "did not receive every scripted request before the deadline".to_string()
                );
            }
            thread::sleep(Duration::from_millis(5));
        }
        self.stop();
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(failure) = &state.failure {
            return Err(failure.clone());
        }
        if !state.remaining.is_empty() {
            return Err(format!(
                "still expected {} request(s)",
                state.remaining.len()
            ));
        }
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take() {
            worker.join().expect("join fake OpenAI service");
        }
    }
}

impl Drop for ScriptedServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve_one_request(
    stream: &mut TcpStream,
    state: &Arc<Mutex<ServerState>>,
    recorded_tx: &mpsc::Sender<RecordedRequest>,
) {
    stream
        .set_nonblocking(false)
        .expect("make accepted fake-service connection blocking");
    stream
        .set_read_timeout(Some(DEFAULT_DEADLINE))
        .expect("set fake service read timeout");
    stream
        .set_write_timeout(Some(DEFAULT_DEADLINE))
        .expect("set fake service write timeout");
    let parsed = match read_request(stream) {
        Ok(request) => request,
        Err(error) => {
            record_server_failure(state, error);
            return;
        }
    };
    let recorded = parsed.recorded();
    let step = {
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.received.push(recorded.clone());
        let _ = recorded_tx.send(recorded);
        let Some(step) = state.remaining.pop_front() else {
            state.failure = Some("received an unexpected repeated request".to_string());
            write_response(stream, 500, "text/plain", &[], "unexpected request");
            return;
        };
        if parsed.method != step.method || parsed.path != step.path {
            state.failure = Some(format!(
                "expected {} {} but received {} {}",
                step.method, step.path, parsed.method, parsed.path
            ));
        } else if step
            .required_body_markers
            .iter()
            .any(|marker| !parsed.body.contains(marker.as_str()))
        {
            state.failure = Some(format!(
                "{} {} body omitted a required request-shape marker",
                step.method, step.path
            ));
        }
        step
    };
    let failed = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .failure
        .is_some();
    if step.close_without_response {
        return;
    }
    if failed {
        write_response(stream, 500, "text/plain", &[], "script mismatch");
    } else {
        write_response(
            stream,
            step.response_status,
            &step.response_content_type,
            &step.response_headers,
            &step.response_body,
        );
    }
}

fn record_server_failure(state: &Arc<Mutex<ServerState>>, error: String) {
    let mut state = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.failure.get_or_insert(error);
}

struct ParsedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

#[derive(Clone, Debug)]
struct SecretBodyFingerprint {
    sha256: String,
    len: usize,
}

impl ParsedRequest {
    fn recorded(&self) -> RecordedRequest {
        let authorization_headers = self
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        let authorization_header_present = !authorization_headers.is_empty();
        let authorization_bearer_sha256 = authorization_headers
            .first()
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(secret_sha256);
        let header_names = self.headers.iter().map(|(name, _)| name.clone()).collect();
        let headers = self
            .headers
            .iter()
            .filter(|(name, _)| !sensitive_name(name))
            .cloned()
            .collect();
        RecordedRequest {
            method: self.method.clone(),
            path: self.path.clone(),
            headers,
            authorization_header_present,
            sanitized_body: sanitize_body(&self.body),
            received_at: Instant::now(),
            header_names,
            authorization_bearer_sha256,
            secret_body_fingerprints: secret_body_fingerprints(&self.body),
        }
    }
}

fn secret_sha256(value: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

fn secret_body_fingerprints(body: &str) -> HashMap<String, SecretBodyFingerprint> {
    url::form_urlencoded::parse(body.as_bytes())
        .filter(|(name, _)| matches!(name.as_ref(), "code" | "code_verifier"))
        .map(|(name, value)| {
            let fingerprint = SecretBodyFingerprint {
                sha256: secret_sha256(&value),
                len: value.len(),
            };
            (name.into_owned(), fingerprint)
        })
        .collect()
}

fn read_request(stream: &mut TcpStream) -> Result<ParsedRequest, String> {
    const MAX_REQUEST_BYTES: usize = 1024 * 1024;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream
            .read(&mut buffer)
            .map_err(|error| format!("failed to read request: {error}"))?;
        if read == 0 {
            return Err("request ended before its headers completed".to_string());
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err("request exceeded fixture size limit".to_string());
        }
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let header_text = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| "request headers were not UTF-8".to_string())?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "request line was missing".to_string())?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| "request method was missing".to_string())?
        .to_string();
    let target = request_parts
        .next()
        .ok_or_else(|| "request target was missing".to_string())?;
    let path = target.split('?').next().unwrap_or(target).to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<Vec<_>>();
    let content_length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let read = stream
            .read(&mut buffer)
            .map_err(|error| format!("failed to read request body: {error}"))?;
        if read == 0 {
            return Err("request body ended before Content-Length".to_string());
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err("request exceeded fixture size limit".to_string());
        }
    }
    let body =
        String::from_utf8_lossy(&bytes[header_end..header_end + content_length]).into_owned();
    Ok(ParsedRequest {
        method,
        path,
        headers,
        body,
    })
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    headers: &[(String, String)],
    body: &str,
) {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Internal Server Error",
    };
    let extra_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn sensitive_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "authorization",
        "cookie",
        "token",
        "secret",
        "api-key",
        "verifier",
    ]
    .iter()
    .any(|part| name.contains(part))
}

fn sanitize_body(body: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<Value>(body) {
        redact_json(&mut value);
        return value.to_string();
    }
    if body.contains('=') {
        return body
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((key, _)) if sensitive_name(key) || key == "code" => {
                    format!("{key}=[REDACTED]")
                }
                _ => pair.to_string(),
            })
            .collect::<Vec<_>>()
            .join("&");
    }
    if body.is_empty() {
        String::new()
    } else {
        format!("[{} request body bytes]", body.len())
    }
}

fn redact_json(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if sensitive_name(key) || key == "code" {
                    *value = Value::String("[REDACTED]".to_string());
                } else {
                    redact_json(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(redact_json),
        _ => {}
    }
}

#[derive(Clone, Debug)]
pub struct AuthorizationMetadata {
    pub endpoint: String,
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: String,
    pub state: String,
    pub originator: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
}

pub fn inspect_authorization_url(value: &str) -> AuthorizationMetadata {
    let mut url = Url::parse(value).expect("parse printed OAuth authorization URL");
    let query = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
    url.set_query(None);
    AuthorizationMetadata {
        endpoint: url.to_string(),
        response_type: required_query(&query, "response_type"),
        client_id: required_query(&query, "client_id"),
        redirect_uri: required_query(&query, "redirect_uri"),
        scope: required_query(&query, "scope"),
        state: required_query(&query, "state"),
        originator: required_query(&query, "originator"),
        code_challenge: required_query(&query, "code_challenge"),
        code_challenge_method: required_query(&query, "code_challenge_method"),
    }
}

fn required_query(query: &HashMap<String, String>, name: &str) -> String {
    query
        .get(name)
        .unwrap_or_else(|| panic!("authorization URL omitted {name}"))
        .clone()
}

pub fn callback_get(redirect_uri: &str, code: &str, state: &str) -> String {
    let mut url = Url::parse(redirect_uri).expect("parse loopback redirect URI");
    assert_eq!(url.scheme(), "http", "callback must be HTTP loopback");
    assert!(
        matches!(url.host_str(), Some("localhost" | "127.0.0.1")),
        "callback must stay loopback"
    );
    url.query_pairs_mut()
        .append_pair("code", code)
        .append_pair("state", state);
    let port = url.port().expect("callback has explicit port");
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect OAuth callback");
    stream
        .set_read_timeout(Some(DEFAULT_DEADLINE))
        .expect("set callback read timeout");
    stream
        .set_write_timeout(Some(DEFAULT_DEADLINE))
        .expect("set callback write timeout");
    let target = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    };
    let request =
        format!("GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("send OAuth callback");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read OAuth callback response");
    response
}

#[derive(Clone, Debug)]
pub struct TokenCanaries {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: String,
    pub email: String,
    pub plan: String,
}

impl TokenCanaries {
    pub fn new(label: &str) -> Self {
        let account_id = format!("acct-{label}-canary");
        let email = format!("{label}-canary@example.invalid");
        let plan = format!("{label}-plan-canary");
        let claims = json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": plan
            }
        });
        Self {
            id_token: jwt(&claims, &format!("{label}-id-signature")),
            access_token: jwt(&claims, &format!("{label}-access-signature")),
            refresh_token: format!("{label}-refresh-token-canary"),
            account_id,
            email,
            plan,
        }
    }

    pub fn token_response(&self) -> Value {
        json!({
            "id_token": self.id_token,
            "access_token": self.access_token,
            "refresh_token": self.refresh_token,
            "expires_in": 3600
        })
    }

    pub fn assert_secrets_absent(
        &self,
        output: &str,
        requests: impl IntoIterator<Item = RecordedRequest>,
    ) {
        let request_metadata = requests
            .into_iter()
            .map(|request| request.searchable_metadata())
            .collect::<Vec<_>>()
            .join("\n");
        for secret in [&self.id_token, &self.access_token, &self.refresh_token] {
            assert!(!output.contains(secret), "CLI output leaked a token canary");
            assert!(
                !request_metadata.contains(secret),
                "recorded request metadata leaked a token canary"
            );
        }
    }
}

fn jwt(claims: &Value, signature: &str) -> String {
    let encoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!(
        "{}.{}.{}",
        encoder.encode(br#"{"alg":"none","typ":"JWT"}"#),
        encoder.encode(claims.to_string()),
        encoder.encode(signature)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_server_reports_out_of_order_repeated_and_missing_requests() {
        let mut out_of_order = ScriptedServer::start([
            ExpectedRequest::json("POST", "/first", json!({ "ok": true })),
            ExpectedRequest::json("POST", "/second", json!({ "ok": true })),
        ]);
        raw_request(out_of_order.base_url(), "GET", "/second");
        let error = out_of_order
            .verify_finished(Duration::from_millis(100))
            .expect_err("out-of-order method/path must fail verification");
        assert!(error.contains("expected POST /first"), "{error}");
        assert!(error.contains("received GET /second"), "{error}");

        let mut repeated =
            ScriptedServer::start([ExpectedRequest::json("GET", "/once", json!({ "ok": true }))]);
        raw_request(repeated.base_url(), "GET", "/once");
        raw_request(repeated.base_url(), "GET", "/once");
        let error = repeated
            .verify_finished(Duration::from_millis(100))
            .expect_err("repeated request must fail verification");
        assert!(error.contains("unexpected repeated request"), "{error}");

        let mut missing = ScriptedServer::start([ExpectedRequest::json(
            "GET",
            "/never",
            json!({ "ok": true }),
        )]);
        let error = missing
            .verify_finished(Duration::from_millis(25))
            .expect_err("missing request must fail within a bounded deadline");
        assert!(error.contains("before the deadline"), "{error}");
    }

    fn raw_request(base_url: &str, method: &str, path: &str) {
        let url = Url::parse(base_url).expect("parse fixture base URL");
        let mut stream = TcpStream::connect((
            url.host_str().expect("fixture host"),
            url.port().expect("fixture port"),
        ))
        .expect("connect fixture server");
        stream
            .set_read_timeout(Some(DEFAULT_DEADLINE))
            .expect("set fixture response timeout");
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            url.host_str().expect("fixture host")
        );
        stream
            .write_all(request.as_bytes())
            .expect("write fixture request");
        let mut response = String::new();
        // Error-path responses may close with a reset on some platforms. The
        // server-state assertion below, not the transport shutdown style, is
        // authoritative for this helper.
        let _ = stream.read_to_string(&mut response);
    }
}
