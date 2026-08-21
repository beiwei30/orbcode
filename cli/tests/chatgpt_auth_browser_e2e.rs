mod support;

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::net::TcpListener;
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};
use support::chatgpt_auth::{
    AuthorizationMetadata, CALLBACK_PORTS_ENV, CliProcess, ExpectedRequest, ORIGINATOR_ENV,
    OpenAiTestEnv, RecordedRequest, ScriptedServer, TokenCanaries, callback_get,
    inspect_authorization_url,
};

const DEADLINE: Duration = Duration::from_secs(10);
const OPENAI_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const BROWSER_GUIDANCE: &str =
    "browser auto-launch disabled (ORBCODE_NO_BROWSER); open the URL above";
const SUCCESS_CODE: &str = "browser-success-code-canary";
const FALLBACK_CODE: &str = "browser-fallback-code-canary";

#[test]
fn browser_login_correlates_pkce_and_persists_across_processes() {
    let canaries = TokenCanaries::new("browser-login");
    let server = ScriptedServer::start([ExpectedRequest::browser_token_exchange(&canaries)
        .requiring(&format!("code={SUCCESS_CODE}"))
        .requiring(&format!("client_id={OPENAI_OAUTH_CLIENT_ID}"))]);
    let env = OpenAiTestEnv::for_server(&server).set(ORIGINATOR_ENV, "orbcode");
    let mut cli = spawn_browser_login(&env);

    let authorization_url = cli.wait_for_stdout_prefix("open ", DEADLINE);
    let listening_uri = cli.wait_for_stdout_prefix("listening ", DEADLINE);
    let _ = cli.wait_for_stdout_prefix(BROWSER_GUIDANCE, DEADLINE);
    assert_instruction_order(&cli.output(), &authorization_url, &listening_uri);
    assert!(
        cli.wait_for_exit(Duration::ZERO).is_none(),
        "login must still be waiting after its instructions are visible\n{}",
        cli.output()
    );

    let metadata = inspect_authorization_url(&authorization_url);
    assert_authorization_request(&metadata, &listening_uri, &server);
    let callback = callback_get(&listening_uri, SUCCESS_CODE, &metadata.state);
    assert!(callback.starts_with("HTTP/1.1 200 OK"), "{callback}");

    let request = server.wait_for_request(DEADLINE);
    assert_browser_exchange(&request, &metadata, SUCCESS_CODE);
    cli.assert_success();

    let login_output = cli.output();
    assert_signed_in_line(&login_output, &canaries);
    assert_persisted_credentials(cli.home(), &canaries);

    // A fresh process must discover the persisted login without refreshing it.
    let mut status = cli.spawn_again(["auth", "status"], &env);
    status.assert_success();
    let status_output = status.output();
    assert!(status_output.contains("openai"), "{status_output}");
    assert!(status_output.contains("chatgpt"), "{status_output}");
    assert!(status_output.contains("stored"), "{status_output}");
    assert!(status_output.contains("ready"), "{status_output}");

    let all_output = format!("{login_output}{status_output}");
    assert_secret_canaries_absent(&all_output, &canaries, SUCCESS_CODE, &metadata, &request);
    assert_eq!(
        server.requests().len(),
        1,
        "auth status must not refresh a newly issued token"
    );
    server.assert_finished();
}

#[test]
fn occupied_first_callback_port_falls_back_without_touching_it() {
    let occupied = TcpListener::bind("127.0.0.1:0").expect("reserve first callback port");
    occupied
        .set_nonblocking(true)
        .expect("make occupied callback listener nonblocking");
    let occupied_port = occupied
        .local_addr()
        .expect("occupied listener addr")
        .port();

    let canaries = TokenCanaries::new("browser-port-fallback");
    let server = ScriptedServer::start([ExpectedRequest::browser_token_exchange(&canaries)
        .requiring(&format!("code={FALLBACK_CODE}"))
        .requiring(&format!("client_id={OPENAI_OAUTH_CLIENT_ID}"))]);
    let env = OpenAiTestEnv::for_server(&server)
        .set(CALLBACK_PORTS_ENV, format!("{occupied_port},0"))
        .set(ORIGINATOR_ENV, "orbcode");
    let mut cli = spawn_browser_login(&env);

    let authorization_url = cli.wait_for_stdout_prefix("open ", DEADLINE);
    let listening_uri = cli.wait_for_stdout_prefix("listening ", DEADLINE);
    let _ = cli.wait_for_stdout_prefix(BROWSER_GUIDANCE, DEADLINE);
    assert_instruction_order(&cli.output(), &authorization_url, &listening_uri);
    let metadata = inspect_authorization_url(&authorization_url);
    assert_authorization_request(&metadata, &listening_uri, &server);

    let fallback_port = url::Url::parse(&listening_uri)
        .expect("parse fallback listener URI")
        .port()
        .expect("fallback listener port");
    assert_ne!(fallback_port, occupied_port);
    assert_ne!(fallback_port, 0);

    let callback = callback_get(&listening_uri, FALLBACK_CODE, &metadata.state);
    assert!(callback.starts_with("HTTP/1.1 200 OK"), "{callback}");
    let request = server.wait_for_request(DEADLINE);
    assert_browser_exchange(&request, &metadata, FALLBACK_CODE);
    cli.assert_success();

    let output = cli.output();
    assert_signed_in_line(&output, &canaries);
    assert_secret_canaries_absent(&output, &canaries, FALLBACK_CODE, &metadata, &request);
    assert!(cli.home().join("auth.json").is_file());
    assert!(
        matches!(occupied.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "the CLI must not connect to or replace the occupied first listener"
    );
    drop(occupied);
    let rebound = TcpListener::bind(("127.0.0.1", occupied_port))
        .expect("the occupied port must be released independently of the CLI");
    drop(rebound);

    assert_eq!(server.requests().len(), 1);
    server.assert_finished();
}

fn spawn_browser_login(env: &OpenAiTestEnv) -> CliProcess {
    CliProcess::spawn(
        [
            "auth",
            "login",
            "--provider",
            "openai",
            "--method",
            "chatgpt",
        ],
        env,
    )
}

fn assert_instruction_order(output: &str, authorization_url: &str, listening_uri: &str) {
    let open = output
        .find(&format!("open {authorization_url}"))
        .expect("authorization URL instruction");
    let listening = output
        .find(&format!("listening {listening_uri}"))
        .expect("callback listener instruction");
    let guidance = output.find(BROWSER_GUIDANCE).expect("browser guidance");
    assert!(open < listening && listening < guidance, "{output}");
}

fn assert_authorization_request(
    metadata: &AuthorizationMetadata,
    listening_uri: &str,
    server: &ScriptedServer,
) {
    assert_eq!(
        metadata.endpoint,
        format!("{}/oauth/authorize", server.base_url())
    );
    assert_eq!(metadata.client_id, OPENAI_OAUTH_CLIENT_ID);
    assert_eq!(metadata.response_type, "code");
    assert_eq!(metadata.code_challenge_method, "S256");
    assert!(!metadata.code_challenge.is_empty());
    assert!(!metadata.state.is_empty());
    assert_eq!(metadata.redirect_uri, listening_uri);
    assert_eq!(metadata.originator, "orbcode");
    assert_eq!(
        metadata.scope.split_whitespace().collect::<BTreeSet<_>>(),
        [
            "api.connectors.invoke",
            "api.connectors.read",
            "email",
            "offline_access",
            "openid",
            "profile",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
    );

    let redirect = url::Url::parse(listening_uri).expect("parse printed callback listener");
    assert_eq!(redirect.scheme(), "http");
    assert_eq!(redirect.host_str(), Some("localhost"));
    assert_eq!(redirect.path(), "/auth/callback");
}

fn assert_browser_exchange(
    request: &RecordedRequest,
    metadata: &AuthorizationMetadata,
    authorization_code: &str,
) {
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/oauth/token");
    assert!(!request.authorization_header_present);

    let fields = url::form_urlencoded::parse(request.sanitized_body.as_bytes())
        .into_owned()
        .collect::<HashMap<_, _>>();
    assert_eq!(
        fields.get("grant_type").map(String::as_str),
        Some("authorization_code")
    );
    assert_eq!(fields.get("code").map(String::as_str), Some("[REDACTED]"));
    assert_eq!(
        fields.get("redirect_uri").map(String::as_str),
        Some(metadata.redirect_uri.as_str())
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
        Some(sha256_url_safe(authorization_code).as_str())
    );
    let verifier_len = request
        .secret_body_len("code_verifier")
        .expect("token exchange must include a verifier");
    assert!(verifier_len > 0, "code verifier must be non-empty");
    assert_eq!(
        request.secret_body_sha256("code_verifier"),
        Some(metadata.code_challenge.as_str()),
        "the exchanged verifier must hash to the printed PKCE challenge"
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
    let expires_at = credentials["expires_at"]
        .as_i64()
        .expect("credential expiry milliseconds");
    let remaining_ms = expires_at - chrono::Utc::now().timestamp_millis();
    assert!(
        (3_500_000..=3_600_000).contains(&remaining_ms),
        "unexpected credential expiry delta: {remaining_ms}ms"
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
    authorization_code: &str,
    metadata: &AuthorizationMetadata,
    request: &RecordedRequest,
) {
    canaries.assert_secrets_absent(output, [request.clone()]);
    assert!(
        !output.contains(authorization_code),
        "CLI output leaked the authorization code"
    );
    assert!(
        !output.contains(&canaries.account_id),
        "CLI output leaked the account id"
    );
    assert!(!output.contains(&canaries.email), "CLI output leaked email");
    assert!(
        !output.contains(&canaries.plan),
        "CLI output leaked the plan claim"
    );
    let verifier_len = request
        .secret_body_len("code_verifier")
        .expect("captured verifier length");
    assert!(
        !contains_sha256_preimage(output, verifier_len, &metadata.code_challenge),
        "CLI output leaked the code verifier"
    );
}

fn contains_sha256_preimage(output: &str, value_len: usize, expected_digest: &str) -> bool {
    value_len > 0
        && output
            .as_bytes()
            .windows(value_len)
            .any(|candidate| sha256_url_safe(candidate) == expected_digest)
}

fn sha256_url_safe(value: impl AsRef<[u8]>) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_ref()))
}
