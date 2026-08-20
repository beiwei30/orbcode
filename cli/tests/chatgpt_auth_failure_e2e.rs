mod support;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt as _;
use std::time::Duration;

use serde_json::json;
use support::chatgpt_auth::{
    BROWSER_TIMEOUT_MS_ENV, CliProcess, DEVICE_TIMEOUT_MS_ENV, ExpectedRequest, OpenAiTestEnv,
    ScriptedServer, TokenCanaries, inspect_authorization_url,
};

const DEADLINE: Duration = Duration::from_secs(5);
const QUIET_PERIOD: Duration = Duration::from_millis(150);
const USER_CODE: &str = "FAIL-E2E";
const DEVICE_AUTH_ID: &str = "failure-device-auth-id-canary";
const AUTHORIZATION_CODE: &str = "failure-authorization-code-canary";
const CODE_VERIFIER: &str = "failure-code-verifier-canary";
const CODE_CHALLENGE: &str = "failure-code-challenge-canary";
const CALLBACK_CODE: &str = "failure-callback-code-canary";
const UNRELATED_AUTH_MARKER: &str = "unrelated-auth-entry-canary";
const UNRELATED_AUTH: &[u8] = br#"{
  "entries": [
    {
      "provider": "anthropic",
      "method": "api_key",
      "source": {
        "kind": "stored_hint",
        "hint": "unrelated-auth-entry-canary"
      },
      "updated_at": "2026-08-18T00:00:00Z"
    }
  ]
}
"#;

#[test]
fn browser_state_mismatch_returns_400_without_exchange_or_mutation() {
    let server = ScriptedServer::start([]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut cli = spawn_browser_login(&env);
    let (state, listening_uri) = browser_ready(&cli);
    assert_ne!(state, "wrong-state-canary");

    let response = callback_with_pairs(
        &listening_uri,
        &[("code", CALLBACK_CODE), ("state", "wrong-state-canary")],
    );
    assert_bad_callback(&response);
    let output = assert_browser_failure(&mut cli, &listening_uri, "callback");
    assert!(output.contains("state mismatch"), "{output}");
    assert_absent(&output, &[CALLBACK_CODE, "wrong-state-canary"]);
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

#[test]
fn browser_rejection_is_actionable_bounded_and_terminal_safe() {
    let server = ScriptedServer::start([]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut cli = spawn_browser_login(&env);
    let (state, listening_uri) = browser_ready(&cli);
    let description = format!(
        "access denied by organization\nINJECTED-LINE\u{1b}[31m \
         https://url-user-canary:url-password-canary@example.invalid/denied?code=url-code-canary&state=url-state-canary&code_verifier=url-verifier-canary&access_token=url-token-canary&refresh_token=url-refresh-canary {}",
        "x".repeat(1000)
    );

    let response = callback_with_pairs(
        &listening_uri,
        &[
            ("error", "access_denied"),
            ("error_description", &description),
            ("state", &state),
        ],
    );
    assert_bad_callback(&response);
    let output = assert_browser_failure(&mut cli, &listening_uri, "callback");
    assert!(output.contains("access denied by organization"), "{output}");
    assert!(
        output.contains("https://example.invalid/denied"),
        "{output}"
    );
    assert!(
        !output.contains("\u{1b}"),
        "terminal escape leaked: {output:?}"
    );
    assert!(
        !output
            .lines()
            .any(|line| line.starts_with("[stderr] INJECTED-LINE")),
        "multiline injection: {output:?}"
    );
    assert_absent(
        &output,
        &[
            "url-user-canary",
            "url-password-canary",
            "url-code-canary",
            "url-state-canary",
            "url-verifier-canary",
            "url-token-canary",
            "url-refresh-canary",
        ],
    );
    let rejection_line = output
        .lines()
        .find(|line| line.contains("callback was rejected"))
        .expect("actionable callback rejection line");
    assert!(rejection_line.chars().count() < 400, "{rejection_line}");
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

#[test]
fn browser_callback_without_code_or_error_returns_400_without_exchange() {
    let server = ScriptedServer::start([]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut cli = spawn_browser_login(&env);
    let (state, listening_uri) = browser_ready(&cli);

    let response = callback_with_pairs(&listening_uri, &[("state", &state)]);
    assert_bad_callback(&response);
    let output = assert_browser_failure(&mut cli, &listening_uri, "callback");
    assert!(output.contains("omitted the code"), "{output}");
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

#[test]
fn browser_malformed_callback_target_returns_400_without_exchange() {
    let server = ScriptedServer::start([]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut cli = spawn_browser_login(&env);
    let (state, listening_uri) = browser_ready(&cli);
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", CALLBACK_CODE)
        .append_pair("state", &state)
        .finish();

    let response = raw_callback(&listening_uri, &format!("/wrong/callback?{query}"));
    assert_bad_callback(&response);
    let output = assert_browser_failure(&mut cli, &listening_uri, "callback");
    assert!(output.contains("request target"), "{output}");
    assert_absent(&output, &[CALLBACK_CODE]);
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

#[test]
fn browser_callback_timeout_exits_and_releases_listener() {
    let server = ScriptedServer::start([]);
    let env = OpenAiTestEnv::for_server(&server).set(BROWSER_TIMEOUT_MS_ENV, "150");
    let mut cli = spawn_browser_login(&env);
    let (_, listening_uri) = browser_ready(&cli);

    let output = assert_browser_failure(&mut cli, &listening_uri, "timed out");
    assert!(output.contains("callback timed out"), "{output}");
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

#[test]
fn browser_token_endpoint_status_errors_are_bounded_and_redacted() {
    for status in [400, 500] {
        let description = if status == 400 {
            format!(
                "token endpoint unavailable \
                 https://body-user-canary:body-password-canary@example.invalid/token?access_token=body-token-canary&refresh_token=body-refresh-canary&code=body-code-canary&state=body-state-canary&code_verifier=body-verifier-canary {}",
                "y".repeat(1000)
            )
        } else {
            format!("oversized-provider-body-canary {}", "z".repeat(12 * 1024))
        };
        let server = ScriptedServer::start([ExpectedRequest::json(
            "POST",
            "/oauth/token",
            json!({ "error_description": description }),
        )
        .responding_with_status(status)]);
        let env = OpenAiTestEnv::for_server(&server);
        let mut cli = spawn_browser_login(&env);
        let (state, listening_uri) = browser_ready(&cli);

        let response = callback_with_pairs(
            &listening_uri,
            &[("code", CALLBACK_CODE), ("state", &state)],
        );
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        let _ = server.wait_for_request(DEADLINE);
        let output = assert_browser_failure(&mut cli, &listening_uri, "token exchange");
        assert!(output.contains(&status.to_string()), "{output}");
        assert_absent(
            &output,
            &[
                CALLBACK_CODE,
                "body-user-canary",
                "body-password-canary",
                "body-token-canary",
                "body-refresh-canary",
                "body-code-canary",
                "body-state-canary",
                "body-verifier-canary",
            ],
        );
        assert!(
            output.chars().count() < 1500,
            "unbounded error output: {output}"
        );
        server.assert_no_request(QUIET_PERIOD);
        server.assert_finished();
    }
}

#[test]
fn browser_malformed_or_incomplete_token_success_never_persists() {
    let canaries = TokenCanaries::new("browser-failed-token");
    let cases = [
        ExpectedRequest::raw("POST", "/oauth/token", "application/json", "{not-json"),
        ExpectedRequest::json(
            "POST",
            "/oauth/token",
            json!({
                "id_token": canaries.id_token,
                "access_token": canaries.access_token,
                "expires_in": 3600
            }),
        ),
    ];

    for step in cases {
        let server = ScriptedServer::start([step]);
        let env = OpenAiTestEnv::for_server(&server);
        let mut cli = spawn_browser_login(&env);
        let (state, listening_uri) = browser_ready(&cli);
        let response = callback_with_pairs(
            &listening_uri,
            &[("code", CALLBACK_CODE), ("state", &state)],
        );
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        let _ = server.wait_for_request(DEADLINE);

        let output = assert_browser_failure(&mut cli, &listening_uri, "token exchange");
        assert_absent(&output, &[CALLBACK_CODE]);
        canaries.assert_secrets_absent(&output, server.requests());
        server.assert_no_request(QUIET_PERIOD);
        server.assert_finished();
    }
}

#[cfg(unix)]
#[test]
fn browser_sigint_while_waiting_reaps_child_and_releases_listener() {
    let server = ScriptedServer::start([]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut cli = spawn_browser_login(&env);
    let (_, listening_uri) = browser_ready(&cli);
    let pid = cli.id();

    cli.interrupt();
    let status = cli
        .wait_for_exit(DEADLINE)
        .unwrap_or_else(|| panic!("SIGINT did not stop browser login\n{}", cli.output()));
    assert!(!status.success(), "SIGINT must not report successful login");
    assert!(status.signal() == Some(libc::SIGINT) || status.code().is_some());
    assert_process_reaped(pid);
    assert_seeded_auth_preserved(&cli);
    assert_listener_released(&listening_uri);
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

#[test]
fn malformed_device_authorization_start_payload_fails_before_polling() {
    let server = ScriptedServer::start([ExpectedRequest::json(
        "POST",
        "/api/accounts/deviceauth/usercode",
        json!({ "device_auth_id": "", "user_code": USER_CODE, "interval": "1" }),
    )]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut cli = spawn_device_login(&env);
    let _ = server.wait_for_request(DEADLINE);

    let output = assert_device_failure(&mut cli, "device authorization");
    assert!(output.contains("required fields were empty"), "{output}");
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

#[test]
fn non_pending_device_poll_statuses_are_terminal() {
    for status in [401, 429, 500] {
        let server = ScriptedServer::start([
            ExpectedRequest::device_user_code(USER_CODE, DEVICE_AUTH_ID),
            ExpectedRequest::json(
                "POST",
                "/api/accounts/deviceauth/token",
                json!({ "error_description": format!("terminal status {status}") }),
            )
            .requiring(USER_CODE)
            .requiring(DEVICE_AUTH_ID)
            .responding_with_status(status),
        ]);
        let env = OpenAiTestEnv::for_server(&server);
        let mut cli = spawn_device_login(&env);
        let requests = server.wait_for_request_count(2, DEADLINE);
        assert_eq!(requests.len(), 2);
        device_instructions_visible(&cli);

        let output = assert_device_failure(&mut cli, "device authorization");
        assert!(output.contains(&status.to_string()), "{output}");
        assert_eq!(server.requests().len(), 2, "status {status} was retried");
        server.assert_no_request(QUIET_PERIOD);
        server.assert_finished();
    }
}

#[test]
fn malformed_device_poll_success_does_not_exchange_tokens() {
    let server = ScriptedServer::start([
        ExpectedRequest::device_user_code(USER_CODE, DEVICE_AUTH_ID),
        ExpectedRequest::json(
            "POST",
            "/api/accounts/deviceauth/token",
            json!({
                "authorization_code": AUTHORIZATION_CODE,
                "code_challenge": CODE_CHALLENGE
            }),
        )
        .requiring(USER_CODE)
        .requiring(DEVICE_AUTH_ID),
    ]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut cli = spawn_device_login(&env);
    let _ = server.wait_for_request_count(2, DEADLINE);
    device_instructions_visible(&cli);

    let output = assert_device_failure(&mut cli, "device authorization");
    assert_absent(&output, &[AUTHORIZATION_CODE, CODE_CHALLENGE]);
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

#[test]
fn continuous_pending_device_polls_stop_at_short_timeout() {
    let server = ScriptedServer::start([
        ExpectedRequest::device_user_code(USER_CODE, DEVICE_AUTH_ID),
        ExpectedRequest::device_poll_pending(USER_CODE, DEVICE_AUTH_ID).responding_with_status(403),
        ExpectedRequest::device_poll_pending(USER_CODE, DEVICE_AUTH_ID).responding_with_status(404),
    ]);
    let env = OpenAiTestEnv::for_server(&server).set(DEVICE_TIMEOUT_MS_ENV, "1250");
    let mut cli = spawn_device_login(&env);
    let requests = server.wait_for_request_count(3, DEADLINE);
    assert_eq!(requests.len(), 3);
    device_instructions_visible(&cli);

    let output = assert_device_failure(&mut cli, "timed out");
    assert!(
        output.contains("device authorization timed out"),
        "{output}"
    );
    assert_eq!(server.requests().len(), 3);
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

#[test]
fn device_token_exchange_status_errors_are_terminal_and_redacted() {
    for status in [400, 500] {
        let description = format!(
            "device token rejected \
             https://device-user-canary:device-password-canary@example.invalid/token?code=device-body-code-canary&state=device-body-state-canary&code_verifier=device-body-verifier-canary&access_token=device-body-token-canary&refresh_token=device-body-refresh-canary {}",
            "q".repeat(1000)
        );
        let server = ScriptedServer::start([
            ExpectedRequest::device_user_code(USER_CODE, DEVICE_AUTH_ID),
            ExpectedRequest::device_poll(
                USER_CODE,
                DEVICE_AUTH_ID,
                AUTHORIZATION_CODE,
                CODE_VERIFIER,
                CODE_CHALLENGE,
            ),
            ExpectedRequest::json(
                "POST",
                "/oauth/token",
                json!({ "error_description": description }),
            )
            .responding_with_status(status),
        ]);
        let env = OpenAiTestEnv::for_server(&server);
        let mut cli = spawn_device_login(&env);
        let _ = server.wait_for_request_count(3, DEADLINE);
        device_instructions_visible(&cli);

        let output = assert_device_failure(&mut cli, "token exchange");
        assert!(output.contains(&status.to_string()), "{output}");
        assert_absent(
            &output,
            &[
                AUTHORIZATION_CODE,
                CODE_VERIFIER,
                CODE_CHALLENGE,
                "device-user-canary",
                "device-password-canary",
                "device-body-code-canary",
                "device-body-state-canary",
                "device-body-verifier-canary",
                "device-body-token-canary",
                "device-body-refresh-canary",
            ],
        );
        server.assert_no_request(QUIET_PERIOD);
        server.assert_finished();
    }
}

#[test]
fn incomplete_device_token_success_never_persists() {
    let canaries = TokenCanaries::new("device-failed-token");
    let server = ScriptedServer::start([
        ExpectedRequest::device_user_code(USER_CODE, DEVICE_AUTH_ID),
        ExpectedRequest::device_poll(
            USER_CODE,
            DEVICE_AUTH_ID,
            AUTHORIZATION_CODE,
            CODE_VERIFIER,
            CODE_CHALLENGE,
        ),
        ExpectedRequest::json(
            "POST",
            "/oauth/token",
            json!({
                "id_token": canaries.id_token,
                "access_token": canaries.access_token,
                "expires_in": 3600
            }),
        ),
    ]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut cli = spawn_device_login(&env);
    let _ = server.wait_for_request_count(3, DEADLINE);
    device_instructions_visible(&cli);

    let output = assert_device_failure(&mut cli, "token exchange");
    canaries.assert_secrets_absent(&output, server.requests());
    assert_absent(
        &output,
        &[AUTHORIZATION_CODE, CODE_VERIFIER, CODE_CHALLENGE],
    );
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

#[test]
fn issuer_connection_close_during_device_poll_is_terminal() {
    let server = ScriptedServer::start([
        ExpectedRequest::device_user_code(USER_CODE, DEVICE_AUTH_ID),
        ExpectedRequest::json(
            "POST",
            "/api/accounts/deviceauth/token",
            json!({ "unused": true }),
        )
        .requiring(USER_CODE)
        .requiring(DEVICE_AUTH_ID)
        .closing_connection(),
    ]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut cli = spawn_device_login(&env);
    let _ = server.wait_for_request_count(2, DEADLINE);
    device_instructions_visible(&cli);

    let output = assert_device_failure(&mut cli, "device authorization");
    assert!(
        output.contains("poll") && output.contains("failed"),
        "{output}"
    );
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

#[cfg(unix)]
#[test]
fn device_sigint_while_pending_reaps_child_without_another_poll() {
    let server = ScriptedServer::start([
        ExpectedRequest::device_user_code(USER_CODE, DEVICE_AUTH_ID),
        ExpectedRequest::device_poll_pending(USER_CODE, DEVICE_AUTH_ID),
    ]);
    let env = OpenAiTestEnv::for_server(&server).set(DEVICE_TIMEOUT_MS_ENV, "5000");
    let mut cli = spawn_device_login(&env);
    let _ = server.wait_for_request_count(2, DEADLINE);
    device_instructions_visible(&cli);
    let pid = cli.id();

    cli.interrupt();
    let status = cli
        .wait_for_exit(DEADLINE)
        .unwrap_or_else(|| panic!("SIGINT did not stop device login\n{}", cli.output()));
    assert!(!status.success(), "SIGINT must not report successful login");
    assert!(status.signal() == Some(libc::SIGINT) || status.code().is_some());
    assert_process_reaped(pid);
    assert_seeded_auth_preserved(&cli);
    server.assert_no_request(QUIET_PERIOD);
    server.assert_finished();
}

fn spawn_browser_login(env: &OpenAiTestEnv) -> CliProcess {
    CliProcess::spawn_with_auth(
        [
            "auth",
            "login",
            "--provider",
            "openai",
            "--method",
            "chatgpt",
        ],
        env,
        UNRELATED_AUTH,
    )
}

fn spawn_device_login(env: &OpenAiTestEnv) -> CliProcess {
    CliProcess::spawn_with_auth(
        [
            "auth",
            "login",
            "--provider",
            "openai",
            "--method",
            "chatgpt",
            "--device-code",
        ],
        env,
        UNRELATED_AUTH,
    )
}

fn browser_ready(cli: &CliProcess) -> (String, String) {
    let authorization_url = cli.wait_for_stdout_prefix("open ", DEADLINE);
    let listening_uri = cli.wait_for_stdout_prefix("listening ", DEADLINE);
    let metadata = inspect_authorization_url(&authorization_url);
    assert_eq!(metadata.redirect_uri, listening_uri);
    (metadata.state, listening_uri)
}

fn device_instructions_visible(cli: &CliProcess) {
    let _ = cli.wait_for_stdout_prefix("open ", DEADLINE);
    assert_eq!(cli.wait_for_stdout_prefix("code ", DEADLINE), USER_CODE);
    let _ = cli.wait_for_stdout_prefix("waiting for authorization", DEADLINE);
}

fn callback_with_pairs(redirect_uri: &str, pairs: &[(&str, &str)]) -> String {
    let url = url::Url::parse(redirect_uri).expect("parse callback URI");
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        query.append_pair(name, value);
    }
    raw_callback(redirect_uri, &format!("{}?{}", url.path(), query.finish()))
}

fn raw_callback(redirect_uri: &str, target: &str) -> String {
    let url = url::Url::parse(redirect_uri).expect("parse callback URI");
    let port = url.port().expect("callback port");
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect callback listener");
    stream
        .set_read_timeout(Some(DEADLINE))
        .expect("bound callback read");
    stream
        .set_write_timeout(Some(DEADLINE))
        .expect("bound callback write");
    let request =
        format!("GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("write callback request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read callback response");
    response
}

fn assert_bad_callback(response: &str) {
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
    assert!(response.contains("Return to the terminal"), "{response}");
}

fn assert_browser_failure(cli: &mut CliProcess, listening_uri: &str, phase: &str) -> String {
    let output = assert_failure(cli, phase);
    assert_listener_released(listening_uri);
    output
}

fn assert_device_failure(cli: &mut CliProcess, phase: &str) -> String {
    assert_failure(cli, phase)
}

fn assert_failure(cli: &mut CliProcess, phase: &str) -> String {
    let pid = cli.id();
    let status = cli.wait_for_exit(DEADLINE).unwrap_or_else(|| {
        panic!(
            "failure path did not exit before deadline\n{}",
            cli.output()
        )
    });
    assert!(!status.success(), "failure path exited successfully");
    assert_process_reaped(pid);
    let output = cli.output();
    assert!(
        output.to_ascii_lowercase().contains(phase),
        "error omitted phase {phase:?}: {output}"
    );
    assert_seeded_auth_preserved(cli);
    output
}

fn assert_seeded_auth_preserved(cli: &CliProcess) {
    let auth_path = cli.home().join("auth.json");
    let contents = fs::read(&auth_path).expect("read seeded auth store");
    assert_eq!(contents, UNRELATED_AUTH, "unrelated auth entry was mutated");
    assert!(
        !cli.home().join("auth.json.tmp").exists(),
        "failure left a temporary auth file"
    );
    assert!(
        !cli.output().contains(UNRELATED_AUTH_MARKER),
        "failure output printed auth file contents"
    );
}

fn assert_listener_released(listening_uri: &str) {
    let port = url::Url::parse(listening_uri)
        .expect("parse callback listener")
        .port()
        .expect("callback port");
    let listener = TcpListener::bind(("127.0.0.1", port))
        .unwrap_or_else(|error| panic!("callback listener {port} survived exit: {error}"));
    drop(listener);
}

#[cfg(unix)]
fn assert_process_reaped(pid: u32) {
    // SAFETY: signal 0 only probes the PID that wait_for_exit just reaped.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    assert_eq!(result, -1, "child process survived expected termination");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH),
        "child PID should no longer exist"
    );
}

#[cfg(not(unix))]
fn assert_process_reaped(_pid: u32) {}

fn assert_absent(output: &str, values: &[&str]) {
    for value in values {
        assert!(!output.contains(value), "output leaked {value:?}: {output}");
    }
}
