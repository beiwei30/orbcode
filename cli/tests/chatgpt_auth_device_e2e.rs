mod support;

use std::collections::HashMap;
use std::fs;
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};
use support::chatgpt_auth::{
    CliProcess, DEVICE_TIMEOUT_MS_ENV, ExpectedRequest, OpenAiTestEnv, RecordedRequest,
    ScriptedServer, TokenCanaries,
};

const DEADLINE: Duration = Duration::from_secs(10);
const OPENAI_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const USER_CODE: &str = "ABCD-EFGH";
const DEVICE_AUTH_ID: &str = "device-auth-id-canary";
const AUTHORIZATION_CODE: &str = "device-authorization-code-canary";
const CODE_VERIFIER: &str = "device-code-verifier-canary";
const CODE_CHALLENGE: &str = "device-code-challenge-canary";
const WAITING_GUIDANCE: &str = "waiting for authorization; interval=1s";
// The Tokio timer must not fire before its one-second deadline. Allow 100 ms
// for timestamp capture and scheduler granularity on loaded CI hosts.
const POLL_INTERVAL_FLOOR: Duration = Duration::from_millis(900);

#[test]
fn device_login_handles_pending_and_persists_across_processes() {
    let canaries = TokenCanaries::new("device-login");
    let server = ScriptedServer::start([
        ExpectedRequest::device_user_code(USER_CODE, DEVICE_AUTH_ID),
        ExpectedRequest::device_poll_pending(USER_CODE, DEVICE_AUTH_ID).responding_with_status(404),
        ExpectedRequest::device_poll_pending(USER_CODE, DEVICE_AUTH_ID),
        ExpectedRequest::device_poll(
            USER_CODE,
            DEVICE_AUTH_ID,
            AUTHORIZATION_CODE,
            CODE_VERIFIER,
            CODE_CHALLENGE,
        ),
        ExpectedRequest::device_token_exchange(&canaries)
            .requiring(&format!("code={AUTHORIZATION_CODE}"))
            .requiring(&format!("client_id={OPENAI_OAUTH_CLIENT_ID}"))
            .requiring(&format!("code_verifier={CODE_VERIFIER}")),
    ]);
    let env = OpenAiTestEnv::for_server(&server).set(DEVICE_TIMEOUT_MS_ENV, "8000");
    let mut cli = spawn_device_login(&env);

    let user_code_request = server.wait_for_request(DEADLINE);
    assert_request(&user_code_request, "/api/accounts/deviceauth/usercode");
    let user_code_body: Value =
        serde_json::from_str(&user_code_request.sanitized_body).expect("device-code request JSON");
    assert_eq!(user_code_body["client_id"], OPENAI_OAUTH_CLIENT_ID);

    let verification_uri = cli.wait_for_stdout_prefix("open ", DEADLINE);
    let printed_user_code = cli.wait_for_stdout_prefix("code ", DEADLINE);
    let _ = cli.wait_for_stdout_prefix(WAITING_GUIDANCE, DEADLINE);
    assert_eq!(
        verification_uri,
        format!("{}/codex/device", server.base_url())
    );
    assert_eq!(printed_user_code, USER_CODE);
    assert_instruction_order(&cli.output(), &verification_uri);
    assert!(
        cli.wait_for_exit(Duration::ZERO).is_none(),
        "login must remain alive after flushing its instructions\n{}",
        cli.output()
    );
    assert!(
        server.requests().len() < 4,
        "the successful poll arrived before instructions were observed"
    );

    let polls = server.wait_for_request_count(3, DEADLINE);
    for poll in &polls {
        assert_request(poll, "/api/accounts/deviceauth/token");
    }
    assert_poll_intervals(&polls);

    let token_exchange = server.wait_for_request(DEADLINE);
    assert_token_exchange(&token_exchange, &server);
    cli.assert_success();

    let login_output = cli.output();
    assert_signed_in_line(&login_output, &canaries);
    assert_persisted_credentials(cli.home(), &canaries);

    let requests_before_status = server.requests();
    assert_eq!(requests_before_status.len(), 5);
    assert_eq!(
        requests_before_status
            .iter()
            .filter(|request| request.path == "/api/accounts/deviceauth/token")
            .count(),
        3,
        "expected two pending polls followed by one successful poll"
    );
    assert_eq!(
        requests_before_status
            .iter()
            .filter(|request| request.path == "/oauth/token")
            .count(),
        1,
        "device login must exchange its authorization code exactly once"
    );

    // A fresh process must select the only usable OpenAI credential without
    // polling, exchanging, or refreshing anything.
    let mut status = cli.spawn_again(["auth", "status"], &env);
    status.assert_success();
    let status_output = status.output();
    let chatgpt_status = status_output
        .lines()
        .find(|line| line.contains("openai") && line.contains("chatgpt"))
        .unwrap_or_else(|| panic!("missing ChatGPT status line\n{status_output}"));
    assert!(chatgpt_status.contains("stored"), "{chatgpt_status}");
    assert!(chatgpt_status.contains("active"), "{chatgpt_status}");
    assert_eq!(
        server.requests().len(),
        requests_before_status.len(),
        "auth status must not poll, exchange, or refresh a current login"
    );

    let all_output = format!("{login_output}{status_output}");
    assert_secret_canaries_absent(&all_output, &canaries, &requests_before_status);
    server.assert_finished();
}

fn spawn_device_login(env: &OpenAiTestEnv) -> CliProcess {
    CliProcess::spawn(
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
    )
}

fn assert_request(request: &RecordedRequest, path: &str) {
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, path);
    assert!(!request.authorization_header_present);
}

fn assert_instruction_order(output: &str, verification_uri: &str) {
    let open = output
        .find(&format!("open {verification_uri}"))
        .expect("verification URL instruction");
    let code = output
        .find(&format!("code {USER_CODE}"))
        .expect("user-code instruction");
    let waiting = output.find(WAITING_GUIDANCE).expect("waiting guidance");
    assert!(open < code && code < waiting, "{output}");
}

fn assert_poll_intervals(polls: &[RecordedRequest]) {
    assert_eq!(polls.len(), 3);
    for (index, pair) in polls.windows(2).enumerate() {
        let interval = pair[1]
            .received_at
            .saturating_duration_since(pair[0].received_at);
        assert!(
            interval >= POLL_INTERVAL_FLOOR,
            "device polls {} and {} were only {interval:?} apart",
            index + 1,
            index + 2
        );
    }
}

fn assert_token_exchange(request: &RecordedRequest, server: &ScriptedServer) {
    assert_request(request, "/oauth/token");
    let fields = url::form_urlencoded::parse(request.sanitized_body.as_bytes())
        .into_owned()
        .collect::<HashMap<_, _>>();
    let expected_redirect_uri = format!("{}/deviceauth/callback", server.base_url());
    assert_eq!(
        fields.get("grant_type").map(String::as_str),
        Some("authorization_code")
    );
    assert_eq!(fields.get("code").map(String::as_str), Some("[REDACTED]"));
    assert_eq!(
        fields.get("redirect_uri").map(String::as_str),
        Some(expected_redirect_uri.as_str())
    );
    assert_eq!(
        fields.get("client_id").map(String::as_str),
        Some(OPENAI_OAUTH_CLIENT_ID)
    );
    assert_eq!(
        fields.get("code_verifier").map(String::as_str),
        Some("[REDACTED]")
    );
    assert_eq!(
        request.secret_body_sha256("code"),
        Some(sha256_url_safe(AUTHORIZATION_CODE).as_str())
    );
    assert_eq!(
        request.secret_body_sha256("code_verifier"),
        Some(sha256_url_safe(CODE_VERIFIER).as_str())
    );
}

fn assert_signed_in_line(output: &str, canaries: &TokenCanaries) {
    let signed_in = output
        .lines()
        .find(|line| line.contains("signed in to openai via chatgpt"))
        .unwrap_or_else(|| panic!("missing signed-in line\n{output}"));
    assert!(signed_in.contains("chatgpt oauth (ready"), "{signed_in}");
    assert!(!signed_in.contains(&canaries.email), "{signed_in}");
    assert!(!signed_in.contains(&canaries.plan), "{signed_in}");
}

fn assert_persisted_credentials(home: &std::path::Path, canaries: &TokenCanaries) {
    let auth_path = home.join("auth.json");
    let contents = fs::read_to_string(&auth_path).expect("read persisted auth.json");
    let auth: Value = serde_json::from_str(&contents).expect("parse persisted auth.json");
    let entries = auth["entries"].as_array().expect("auth entries array");
    assert_eq!(entries.len(), 1, "one ChatGPT OAuth entry expected");
    let entry = &entries[0];
    assert_eq!(entry["provider"], "openai");
    assert_eq!(entry["method"], "chatgpt");
    assert_eq!(entry["source"]["kind"], "chatgpt_oauth");
    let credentials = &entry["source"]["credentials"];
    assert_eq!(credentials["id_token"], canaries.id_token);
    assert_eq!(credentials["access_token"], canaries.access_token);
    assert_eq!(credentials["refresh_token"], canaries.refresh_token);
    assert_eq!(credentials["account_id"], canaries.account_id);
    assert_eq!(credentials["email"], canaries.email);
    assert_eq!(credentials["plan_type"], canaries.plan);
    assert!(
        credentials["expires_at"]
            .as_i64()
            .is_some_and(|expires_at| {
                expires_at > chrono::Utc::now().timestamp_millis() + 3_500_000
            }),
        "persisted credentials must remain current"
    );
    assert!(!contents.contains("stored_api_key"));
    assert!(!contents.contains("\"api_key\""));
    assert!(
        !home.join("auth.json.tmp").exists(),
        "atomic save must not leave its temporary file behind"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mode = fs::metadata(auth_path)
            .expect("auth.json metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

fn assert_secret_canaries_absent(
    output: &str,
    canaries: &TokenCanaries,
    requests: &[RecordedRequest],
) {
    canaries.assert_secrets_absent(output, requests.iter().cloned());
    for secret in [
        AUTHORIZATION_CODE,
        CODE_VERIFIER,
        CODE_CHALLENGE,
        DEVICE_AUTH_ID,
        canaries.account_id.as_str(),
        canaries.email.as_str(),
        canaries.plan.as_str(),
    ] {
        assert!(!output.contains(secret), "CLI output leaked {secret}");
    }
}

fn sha256_url_safe(value: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}
