mod support;

use std::fs;
#[cfg(unix)]
use std::net::TcpStream;
use std::time::Duration;

use support::chatgpt_auth::{
    CliProcess, ExpectedRequest, ISSUER_ENV, OpenAiTestEnv, ScriptedServer, TokenCanaries,
    callback_get, inspect_authorization_url,
};

const DEADLINE: Duration = Duration::from_secs(5);

#[test]
fn real_cli_browser_smoke_is_live_before_completion_and_stays_loopback() {
    let canaries = TokenCanaries::new("browser-smoke");
    let server = ScriptedServer::start([ExpectedRequest::browser_token_exchange(&canaries)]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut cli = CliProcess::spawn(
        [
            "auth",
            "login",
            "--provider",
            "openai",
            "--method",
            "chatgpt",
        ],
        &env,
    );

    // These lines are emitted and captured while the real binary is blocked on
    // its callback listener, proving the harness can drive an in-flight child.
    let authorization_url = cli.wait_for_stdout_prefix("open ", DEADLINE);
    let listening_uri = cli.wait_for_stdout_prefix("listening ", DEADLINE);
    let metadata = inspect_authorization_url(&authorization_url);
    assert_eq!(
        metadata.endpoint,
        format!("{}/oauth/authorize", server.base_url())
    );
    assert_eq!(metadata.redirect_uri, listening_uri);
    assert_eq!(metadata.response_type, "code");
    assert_eq!(metadata.originator, "orbcode-harness");
    assert_eq!(metadata.code_challenge_method, "S256");
    assert!(metadata.scope.contains("offline_access"));
    assert!(!metadata.client_id.is_empty());
    assert!(!metadata.code_challenge.is_empty());

    let callback = callback_get(&listening_uri, "browser-smoke-code", &metadata.state);
    assert!(callback.starts_with("HTTP/1.1 200 OK"), "{callback}");

    let request = server.wait_for_request(DEADLINE);
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/oauth/token");
    assert!(
        request
            .sanitized_body
            .contains("grant_type=authorization_code"),
        "{}",
        request.sanitized_body
    );
    cli.assert_success();
    assert!(
        cli.output().contains("signed in to openai via chatgpt"),
        "{}",
        cli.output()
    );
    assert!(
        cli.home().join("auth.json").is_file(),
        "successful smoke must persist the fake credentials"
    );
    assert!(cli.cwd().is_dir());
    assert!(cli.root().is_dir());

    let requests = server.requests();
    assert_eq!(
        requests.len(),
        1,
        "smoke must make one fake-service request"
    );
    canaries.assert_secrets_absent(&cli.output(), requests);
    server.assert_finished();
}

#[test]
fn malformed_override_fails_before_request_or_auth_file_mutation() {
    let server = ScriptedServer::start([]);
    let rejected = "http://example.com:4444/path?secret=override-canary";
    let env = OpenAiTestEnv::for_server(&server).set(ISSUER_ENV, rejected);
    let mut cli = CliProcess::spawn(
        [
            "auth",
            "login",
            "--provider",
            "openai",
            "--method",
            "chatgpt",
        ],
        &env,
    );

    let status = cli
        .wait_for_exit(DEADLINE)
        .unwrap_or_else(|| panic!("invalid override did not fail promptly\n{}", cli.output()));
    assert!(!status.success(), "invalid override must fail closed");
    let output = cli.output();
    assert!(output.contains(ISSUER_ENV), "{output}");
    assert!(
        !output.contains(rejected),
        "must not echo injected URL: {output}"
    );
    assert!(
        !output.contains("override-canary"),
        "must not leak query: {output}"
    );
    assert!(
        server.requests().is_empty(),
        "invalid override made a request"
    );
    assert!(
        !cli.home().join("auth.json").exists(),
        "invalid override must not mutate the auth store"
    );
    let home_entries = fs::read_dir(cli.home())
        .expect("read isolated home")
        .map(|entry| entry.expect("home entry").file_name())
        .collect::<Vec<_>>();
    assert!(
        !home_entries.iter().any(|name| name == "auth.json.tmp"),
        "invalid override left a temporary auth file"
    );
    server.assert_finished();
}

#[cfg(unix)]
#[test]
fn dropping_blocked_child_reaps_process_and_callback_listener() {
    let server = ScriptedServer::start([]);
    let env = OpenAiTestEnv::for_server(&server);
    let (pid, callback_port) = {
        let cli = CliProcess::spawn(
            [
                "auth",
                "login",
                "--provider",
                "openai",
                "--method",
                "chatgpt",
            ],
            &env,
        );
        let _ = cli.wait_for_stdout_prefix("open ", DEADLINE);
        let listening_uri = cli.wait_for_stdout_prefix("listening ", DEADLINE);
        let callback_port = url::Url::parse(&listening_uri)
            .expect("parse callback URI")
            .port()
            .expect("callback port");
        (cli.id(), callback_port)
        // `cli` drops here while its child is still waiting for a callback.
    };

    // SAFETY: signal 0 only probes the PID captured from the fixture child.
    let process_probe = unsafe { libc::kill(pid as libc::pid_t, 0) };
    assert_eq!(process_probe, -1, "Drop must reap the child process");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH),
        "child PID should no longer exist"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", callback_port)).is_err(),
        "Drop must close the child's callback listener"
    );
    server.assert_finished();
}
