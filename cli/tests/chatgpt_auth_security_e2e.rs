//! Terminal security and release-boundary evidence for ChatGPT auth E2Es.

mod support;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::chatgpt_auth::{
    CliProcess, ExpectedRequest, OpenAiTestEnv, RecordedRequest, ScriptedServer, TokenCanaries,
    inspect_authorization_url,
};

const DEADLINE: Duration = Duration::from_secs(10);
const RESPONSES_PATH: &str = "/backend-api/codex/responses";
const AUTHORIZATION_CODE: &str = "security-browser-authorization-code-canary";
const CALLBACK_QUERY: &str = "security-callback-query-canary";
const STORED_API_KEY: &str = "security-stored-api-key-canary";
const ENV_API_KEY: &str = "security-environment-api-key-canary";
const DEVICE_USER_CODE: &str = "SECR-E2E";
const DEVICE_AUTH_ID: &str = "security-device-auth-id-canary";
const DEVICE_AUTHORIZATION_CODE: &str = "security-device-authorization-code-canary";
const DEVICE_CODE_VERIFIER: &str = "security-device-code-verifier-canary";
const DEVICE_CODE_CHALLENGE: &str = "security-device-code-challenge-canary";
const SECURITY_PROMPT: &str = "Return the security fixture acknowledgement.";
const SECURITY_ANSWER: &str = "CHATGPT_AUTH_SECURITY_OK";

#[test]
fn browser_login_and_responses_keep_canaries_in_the_intentional_store_only() {
    let canaries = TokenCanaries::new("security-browser");
    let server = ScriptedServer::start([
        ExpectedRequest::browser_token_exchange(&canaries)
            .requiring(&format!("code={AUTHORIZATION_CODE}")),
        ExpectedRequest::responses_sse(RESPONSES_PATH, &text_events()),
    ]);
    let env = OpenAiTestEnv::for_server(&server).set("ANTHROPIC_API_KEY", ENV_API_KEY);
    let seed = stored_api_key_seed();
    let mut login = CliProcess::spawn_with_auth(
        [
            "auth",
            "login",
            "--provider",
            "openai",
            "--method",
            "chatgpt",
        ],
        &env,
        &seed,
    );

    let authorization_url = login.wait_for_stdout_prefix("open ", DEADLINE);
    let redirect_uri = login.wait_for_stdout_prefix("listening ", DEADLINE);
    let metadata = inspect_authorization_url(&authorization_url);
    let callback_response = callback_with_probe(
        &redirect_uri,
        AUTHORIZATION_CODE,
        &metadata.state,
        CALLBACK_QUERY,
    );
    assert!(
        callback_response.starts_with("HTTP/1.1 200 OK"),
        "{callback_response}"
    );
    assert_absent(
        &callback_response,
        &[AUTHORIZATION_CODE, &metadata.state, CALLBACK_QUERY],
    );

    let token_exchange = server.wait_for_request(DEADLINE);
    assert_eq!(token_exchange.path, "/oauth/token");
    assert_eq!(
        token_exchange.secret_body_sha256("code"),
        Some(secret_sha256(AUTHORIZATION_CODE).as_str())
    );
    assert_eq!(
        token_exchange.secret_body_sha256("code_verifier"),
        Some(metadata.code_challenge.as_str())
    );
    login.assert_success();

    let mut prompt = login.spawn_again(
        [
            "-p",
            SECURITY_PROMPT,
            "--provider",
            "openai",
            "--settings",
            r#"{"model":"gpt-5.6-sol"}"#,
            "--output-format",
            "stream-json",
            "--verbose",
        ],
        &env,
    );
    prompt.assert_success();
    let prompt_output = prompt.output();
    assert!(prompt_output.contains(SECURITY_ANSWER), "{prompt_output}");

    let output = format!("{}{}", login.output(), prompt_output);
    let requests = server.requests();
    assert_eq!(requests.len(), 2, "scripted request count changed");
    canaries.assert_secrets_absent(&output, requests.iter().cloned());
    assert_absent(
        &output,
        &[
            AUTHORIZATION_CODE,
            CALLBACK_QUERY,
            STORED_API_KEY,
            ENV_API_KEY,
            &format!("Authorization: Bearer {}", canaries.access_token),
        ],
    );
    assert_eq!(
        output.matches(&metadata.state).count(),
        1,
        "state may appear only in the required authorization URL"
    );
    assert_eq!(
        output.matches(&metadata.code_challenge).count(),
        1,
        "PKCE challenge may appear only in the required authorization URL"
    );
    assert!(!output.contains("code_verifier="), "{output}");

    let responses_request = requests
        .iter()
        .find(|request| request.path == RESPONSES_PATH)
        .expect("one Responses request");
    assert_eq!(
        responses_request.authorization_bearer_sha256(),
        Some(secret_sha256(&canaries.access_token).as_str())
    );
    assert_eq!(
        responses_request.chatgpt_account_id_sha256(),
        Some(secret_sha256(&canaries.account_id).as_str())
    );
    let diagnostics = format!("{requests:?}");
    assert_absent(
        &diagnostics,
        &[
            AUTHORIZATION_CODE,
            CALLBACK_QUERY,
            STORED_API_KEY,
            ENV_API_KEY,
            &canaries.id_token,
            &canaries.access_token,
            &canaries.refresh_token,
            &canaries.account_id,
            &canaries.email,
            &canaries.plan,
        ],
    );

    let auth_path = login.home().join("auth.json");
    assert_intentional_auth_store(&auth_path, &canaries, &metadata);
    let persistent_secrets = [
        AUTHORIZATION_CODE,
        CALLBACK_QUERY,
        STORED_API_KEY,
        ENV_API_KEY,
        &canaries.id_token,
        &canaries.access_token,
        &canaries.refresh_token,
        &canaries.account_id,
        &canaries.email,
        &canaries.plan,
        &metadata.state,
        &metadata.code_challenge,
    ];
    assert_non_auth_files_secret_free(login.root(), &persistent_secrets);
    env.assert_no_public_network();
    server.assert_finished();
}

#[test]
fn device_ephemeral_values_are_visible_only_where_the_flow_requires_them() {
    let canaries = TokenCanaries::new("security-device");
    let server = ScriptedServer::start([
        ExpectedRequest::device_user_code(DEVICE_USER_CODE, DEVICE_AUTH_ID),
        ExpectedRequest::device_poll(
            DEVICE_USER_CODE,
            DEVICE_AUTH_ID,
            DEVICE_AUTHORIZATION_CODE,
            DEVICE_CODE_VERIFIER,
            DEVICE_CODE_CHALLENGE,
        ),
        ExpectedRequest::device_token_exchange(&canaries),
    ]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut login = CliProcess::spawn(
        [
            "auth",
            "login",
            "--provider",
            "openai",
            "--method",
            "chatgpt",
            "--device-code",
        ],
        &env,
    );

    let verification_uri = login.wait_for_stdout_prefix("open ", DEADLINE);
    assert_eq!(
        login.wait_for_stdout_prefix("code ", DEADLINE),
        DEVICE_USER_CODE
    );
    let _ = login.wait_for_stdout_prefix("waiting for authorization", DEADLINE);
    assert_eq!(
        verification_uri,
        format!("{}/codex/device", server.base_url())
    );
    let requests = server.wait_for_request_count(3, DEADLINE);
    login.assert_success();

    let poll = request_at(&requests, "/api/accounts/deviceauth/token");
    assert_eq!(
        poll.secret_body_sha256("user_code"),
        Some(secret_sha256(DEVICE_USER_CODE).as_str())
    );
    assert_eq!(
        poll.secret_body_sha256("device_auth_id"),
        Some(secret_sha256(DEVICE_AUTH_ID).as_str())
    );
    let exchange = request_at(&requests, "/oauth/token");
    assert_eq!(
        exchange.secret_body_sha256("code"),
        Some(secret_sha256(DEVICE_AUTHORIZATION_CODE).as_str())
    );
    assert_eq!(
        exchange.secret_body_sha256("code_verifier"),
        Some(secret_sha256(DEVICE_CODE_VERIFIER).as_str())
    );

    let output = login.output();
    assert_eq!(
        output.matches(DEVICE_USER_CODE).count(),
        1,
        "the user code must appear only on its required instruction line"
    );
    assert_absent(
        &output,
        &[
            DEVICE_AUTH_ID,
            DEVICE_AUTHORIZATION_CODE,
            DEVICE_CODE_VERIFIER,
            DEVICE_CODE_CHALLENGE,
        ],
    );
    canaries.assert_secrets_absent(&output, requests.iter().cloned());
    let diagnostics = format!("{requests:?}");
    assert_absent(
        &diagnostics,
        &[
            DEVICE_USER_CODE,
            DEVICE_AUTH_ID,
            DEVICE_AUTHORIZATION_CODE,
            DEVICE_CODE_VERIFIER,
            DEVICE_CODE_CHALLENGE,
        ],
    );
    let auth = fs::read_to_string(login.home().join("auth.json")).expect("read device auth");
    assert_absent(
        &auth,
        &[
            DEVICE_USER_CODE,
            DEVICE_AUTH_ID,
            DEVICE_AUTHORIZATION_CODE,
            DEVICE_CODE_VERIFIER,
            DEVICE_CODE_CHALLENGE,
        ],
    );
    assert_non_auth_files_secret_free(
        login.root(),
        &[
            DEVICE_USER_CODE,
            DEVICE_AUTH_ID,
            DEVICE_AUTHORIZATION_CODE,
            DEVICE_CODE_VERIFIER,
            DEVICE_CODE_CHALLENGE,
            &canaries.id_token,
            &canaries.access_token,
            &canaries.refresh_token,
            &canaries.account_id,
            &canaries.email,
            &canaries.plan,
        ],
    );
    env.assert_no_public_network();
    server.assert_finished();
}

#[test]
fn responses_and_logout_failures_do_not_echo_auth_material() {
    let canaries = TokenCanaries::new("security-failure");
    let provider_description = format!(
        "request rejected Authorization: Bearer {} \
         https://{}:{}@example.invalid/failure?access_token={}&refresh_token={}&id_token={}&code={}&state={}&code_verifier={}&code_challenge={}&device_auth_id={}&user_code={}&account_id={}&email={}&plan={}&api_key={}#{}",
        canaries.access_token,
        canaries.email,
        ENV_API_KEY,
        canaries.access_token,
        canaries.refresh_token,
        canaries.id_token,
        AUTHORIZATION_CODE,
        CALLBACK_QUERY,
        DEVICE_CODE_VERIFIER,
        DEVICE_CODE_CHALLENGE,
        DEVICE_AUTH_ID,
        DEVICE_USER_CODE,
        canaries.account_id,
        canaries.email,
        canaries.plan,
        STORED_API_KEY,
        CALLBACK_QUERY,
    );
    let server = ScriptedServer::start([ExpectedRequest::json(
        "POST",
        RESPONSES_PATH,
        json!({
            "error": {
                "type": "invalid_request_error",
                "message": provider_description
            }
        }),
    )
    .responding_with_status(400)]);
    let env = OpenAiTestEnv::for_server(&server);
    let auth = chatgpt_auth_seed(&canaries);
    let mut prompt = CliProcess::spawn_with_auth(
        [
            "-p",
            SECURITY_PROMPT,
            "--provider",
            "openai",
            "--settings",
            r#"{"model":"gpt-5.6-sol"}"#,
        ],
        &env,
        &auth,
    );
    let status = prompt
        .wait_for_exit(DEADLINE)
        .unwrap_or_else(|| panic!("Responses failure did not exit\n{}", prompt.output()));
    assert!(!status.success(), "Responses failure reported success");
    let output = prompt.output();
    assert!(output.contains("provider openai failed"), "{output}");
    assert_failure_canaries_absent(&output, &canaries);
    assert_non_auth_files_secret_free(prompt.root(), &failure_canaries(&canaries));
    env.assert_no_public_network();
    server.assert_finished();

    let logout_server = ScriptedServer::start([]);
    let logout_env = OpenAiTestEnv::for_server(&logout_server);
    let malformed = format!(
        "{{not-json {} {} {} }}",
        canaries.access_token, STORED_API_KEY, CALLBACK_QUERY
    );
    let mut logout = CliProcess::spawn_with_auth(
        ["auth", "logout", "--provider", "openai"],
        &logout_env,
        malformed.as_bytes(),
    );
    let status = logout
        .wait_for_exit(DEADLINE)
        .unwrap_or_else(|| panic!("logout failure did not exit\n{}", logout.output()));
    assert!(!status.success(), "malformed auth store logout succeeded");
    assert_failure_canaries_absent(&logout.output(), &canaries);
    assert!(!logout.home().join("auth.json.tmp").exists());
    assert_non_auth_files_secret_free(logout.root(), &failure_canaries(&canaries));
    logout_env.assert_no_public_network();
    logout_server.assert_finished();
}

#[test]
fn test_support_is_confined_to_non_public_feature_gated_surfaces() {
    let repo = repo_root();
    let config_manifest = read(repo.join("config/Cargo.toml"));
    let cli_manifest = read(repo.join("cli/Cargo.toml"));
    assert!(
        config_manifest.contains("default = []")
            && config_manifest.contains("oauth-test-support = []")
    );
    let normal_cli = cli_manifest
        .split("[dev-dependencies]")
        .next()
        .expect("normal dependency section");
    assert!(!normal_cli.contains("oauth-test-support"));
    assert!(
        cli_manifest
            .split_once("[dev-dependencies]")
            .expect("dev dependency section")
            .1
            .contains("features = [\"oauth-test-support\"]")
    );

    let prefix = ["ORBCODE", "_TEST_OPENAI_"].concat();
    for path in [
        repo.join("README.md"),
        repo.join("AGENTS.md"),
        repo.join("CLAUDE.md"),
        repo.join("config/src/env_compat.rs"),
        repo.join("config/src/layers.rs"),
    ] {
        assert!(
            !read(&path).contains(&prefix),
            "public surface: {}",
            path.display()
        );
    }
    for root in [
        repo.join("docs"),
        repo.join("protocol/src"),
        repo.join("cli/src"),
    ] {
        for path in regular_files(&root) {
            assert!(
                !read(&path).contains(&prefix),
                "public surface: {}",
                path.display()
            );
        }
    }
    let loader = read(repo.join("config/src/oauth_test_support.rs"));
    assert!(loader.contains("std::env::var(key)"));
    for path in regular_files(&repo.join("config/src")) {
        if path.ends_with("oauth_test_support.rs") {
            continue;
        }
        let contents = read(&path);
        assert!(
            !(contents.contains(&prefix) && contents.contains("std::env::var")),
            "test endpoint variables may be read only by oauth_test_support.rs: {}",
            path.display()
        );
    }

    let help = Command::new(env!("CARGO_BIN_EXE_orbcode"))
        .arg("--help")
        .output()
        .expect("run public help");
    assert!(help.status.success());
    let help_text = format!(
        "{}{}",
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr)
    );
    assert!(!help_text.contains(&prefix), "{help_text}");

    let server = ScriptedServer::start([]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut advanced = CliProcess::spawn(["advanced"], &env);
    advanced.assert_success();
    assert!(
        !advanced.output().contains(&prefix),
        "{}",
        advanced.output()
    );
    env.assert_no_public_network();
    server.assert_finished();
}

#[test]
#[ignore = "run by scripts/audit-chatgpt-auth-release.sh after building --release"]
fn release_binary_ignores_test_endpoint_overrides() {
    let binary = std::env::var_os("ORBCODE_RELEASE_BIN")
        .map(PathBuf::from)
        .expect("ORBCODE_RELEASE_BIN must name the freshly built release binary");
    assert!(
        binary.is_file(),
        "release binary missing: {}",
        binary.display()
    );
    let server = ScriptedServer::start([]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut cli = CliProcess::spawn_binary(
        &binary,
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

    let authorization_url = cli.wait_for_stdout_prefix("open ", DEADLINE);
    let metadata = inspect_authorization_url(&authorization_url);
    assert_eq!(metadata.endpoint, "https://auth.openai.com/oauth/authorize");
    assert_eq!(metadata.originator, "orbcode");
    let listening_uri = cli.wait_for_stdout_prefix("listening ", DEADLINE);
    let callback_port = url::Url::parse(&listening_uri)
        .expect("parse production callback URI")
        .port()
        .expect("production callback port");
    assert!(matches!(callback_port, 1455 | 1457), "{listening_uri}");
    assert!(!authorization_url.contains(server.base_url()));
    env.assert_no_public_network();
    cli.terminate();
    server.assert_finished();
}

fn text_events() -> Vec<Value> {
    vec![
        json!({
            "type": "response.created",
            "response": { "id": "security-response", "status": "in_progress" }
        }),
        json!({ "type": "response.output_text.delta", "delta": SECURITY_ANSWER }),
        json!({
            "type": "response.completed",
            "response": {
                "id": "security-response",
                "status": "completed",
                "usage": { "input_tokens": 3, "output_tokens": 2, "total_tokens": 5 }
            }
        }),
    ]
}

fn stored_api_key_seed() -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({
        "entries": [{
            "provider": "anthropic",
            "method": "api_key",
            "source": { "kind": "stored_api_key", "api_key": STORED_API_KEY },
            "updated_at": "2026-08-18T00:00:00Z"
        }]
    }))
    .expect("serialize API-key seed")
}

fn chatgpt_auth_seed(canaries: &TokenCanaries) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "entries": [{
            "provider": "openai",
            "method": "chatgpt",
            "source": {
                "kind": "chatgpt_oauth",
                "credentials": {
                    "id_token": canaries.id_token,
                    "access_token": canaries.access_token,
                    "refresh_token": canaries.refresh_token,
                    "expires_at": 4_102_444_800_000_i64,
                    "account_id": canaries.account_id,
                    "email": canaries.email,
                    "plan_type": canaries.plan
                }
            },
            "updated_at": "2026-08-18T00:00:00Z"
        }]
    }))
    .expect("serialize ChatGPT auth seed")
}

fn callback_with_probe(redirect_uri: &str, code: &str, state: &str, query_canary: &str) -> String {
    let mut url = url::Url::parse(redirect_uri).expect("parse callback URI");
    url.query_pairs_mut()
        .append_pair("code", code)
        .append_pair("state", state)
        .append_pair("security_probe", query_canary);
    let port = url.port().expect("callback port");
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect callback");
    stream
        .set_read_timeout(Some(DEADLINE))
        .expect("bound callback response");
    stream
        .set_write_timeout(Some(DEADLINE))
        .expect("bound callback request");
    let target = format!("{}?{}", url.path(), url.query().expect("callback query"));
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

fn request_at<'a>(requests: &'a [RecordedRequest], path: &str) -> &'a RecordedRequest {
    requests
        .iter()
        .find(|request| request.path == path)
        .unwrap_or_else(|| panic!("missing request {path}: {requests:?}"))
}

fn assert_intentional_auth_store(
    path: &Path,
    canaries: &TokenCanaries,
    metadata: &support::chatgpt_auth::AuthorizationMetadata,
) {
    let contents = fs::read_to_string(path).expect("read intentional auth store");
    let auth: Value = serde_json::from_str(&contents).expect("parse intentional auth store");
    let entries = auth["entries"].as_array().expect("auth entries");
    assert_eq!(entries.len(), 2, "{contents}");
    let stored = entries
        .iter()
        .find(|entry| entry["provider"] == "anthropic")
        .expect("preserved stored API key");
    assert_eq!(stored["source"]["api_key"], STORED_API_KEY);
    let chatgpt = entries
        .iter()
        .find(|entry| entry["provider"] == "openai")
        .expect("persisted ChatGPT entry");
    assert_eq!(chatgpt["method"], "chatgpt");
    assert_eq!(chatgpt["source"]["kind"], "chatgpt_oauth");
    let credentials = chatgpt["source"]["credentials"]
        .as_object()
        .expect("credential object");
    assert_eq!(
        credentials
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        [
            "access_token",
            "account_id",
            "email",
            "expires_at",
            "id_token",
            "plan_type",
            "refresh_token",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
    );
    assert_eq!(credentials["id_token"], canaries.id_token);
    assert_eq!(credentials["access_token"], canaries.access_token);
    assert_eq!(credentials["refresh_token"], canaries.refresh_token);
    assert_eq!(credentials["account_id"], canaries.account_id);
    assert_eq!(credentials["email"], canaries.email);
    assert_eq!(credentials["plan_type"], canaries.plan);
    for secret in [
        canaries.id_token.as_str(),
        canaries.access_token.as_str(),
        canaries.refresh_token.as_str(),
        canaries.account_id.as_str(),
        canaries.email.as_str(),
        canaries.plan.as_str(),
        STORED_API_KEY,
    ] {
        assert_eq!(contents.matches(secret).count(), 1, "duplicate {secret:?}");
    }
    assert_absent(
        &contents,
        &[
            ENV_API_KEY,
            AUTHORIZATION_CODE,
            CALLBACK_QUERY,
            &metadata.state,
            &metadata.code_challenge,
        ],
    );
    assert!(!path.with_extension("json.tmp").exists());
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(path)
            .expect("auth metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

fn assert_failure_canaries_absent(output: &str, canaries: &TokenCanaries) {
    assert_absent(output, &failure_canaries(canaries));
    assert!(output.chars().count() < 2_000, "unbounded error: {output}");
}

fn failure_canaries(canaries: &TokenCanaries) -> Vec<&str> {
    vec![
        &canaries.id_token,
        &canaries.access_token,
        &canaries.refresh_token,
        &canaries.account_id,
        &canaries.email,
        &canaries.plan,
        AUTHORIZATION_CODE,
        CALLBACK_QUERY,
        STORED_API_KEY,
        ENV_API_KEY,
        DEVICE_USER_CODE,
        DEVICE_AUTH_ID,
        DEVICE_AUTHORIZATION_CODE,
        DEVICE_CODE_VERIFIER,
        DEVICE_CODE_CHALLENGE,
    ]
}

fn assert_non_auth_files_secret_free(root: &Path, secrets: &[&str]) {
    for path in regular_files(root) {
        if path.file_name() == Some(OsStr::new("auth.json")) {
            continue;
        }
        let contents = fs::read(&path)
            .unwrap_or_else(|error| panic!("read artifact {}: {error}", path.display()));
        let contents = String::from_utf8_lossy(&contents);
        for secret in secrets {
            assert!(
                !contents.contains(secret),
                "artifact {} leaked {secret:?}",
                path.display()
            );
        }
    }
}

fn regular_files(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)
            .unwrap_or_else(|error| panic!("inspect {}: {error}", path.display()));
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_file() {
            files.push(path);
            continue;
        }
        let entries =
            fs::read_dir(&path).unwrap_or_else(|error| panic!("list {}: {error}", path.display()));
        pending.extend(entries.map(|entry| entry.expect("directory entry").path()));
    }
    files
}

fn assert_absent(contents: &str, secrets: &[&str]) {
    for secret in secrets {
        assert!(!contents.contains(secret), "leaked {secret:?}: {contents}");
    }
}

fn secret_sha256(value: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

fn read(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}
