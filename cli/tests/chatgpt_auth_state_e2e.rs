mod support;

use std::fs;
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;
use support::chatgpt_auth::{
    CliProcess, ExpectedRequest, OpenAiTestEnv, ScriptedServer, TokenCanaries, callback_get,
    inspect_authorization_url,
};

const DEADLINE: Duration = Duration::from_secs(10);
const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const STORED_OPENAI_KEY: &str = "sk-status-stored-openai-canary";
const ENV_OPENAI_KEY: &str = "sk-status-env-openai-canary";
const STORED_ANTHROPIC_KEY: &str = "sk-status-stored-anthropic-canary";

#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusLine {
    provider: String,
    method: String,
    storage: String,
    state: String,
    summary: String,
}

#[test]
fn status_projects_every_openai_precedence_combination_without_secrets() {
    let chatgpt_only = seed_chatgpt("status-chatgpt-only");
    let auth_path = chatgpt_only.anchor.home().join("auth.json");
    let bytes_before = fs::read(&auth_path).expect("read ChatGPT-only auth store");
    let modified_before = fs::metadata(&auth_path)
        .expect("ChatGPT-only auth metadata")
        .modified()
        .expect("ChatGPT-only auth mtime");

    let first_output = run(&chatgpt_only.anchor, ["auth", "status"], &chatgpt_only.env);
    let first = parse_status(&first_output);
    assert_eq!(
        first,
        [status_line(
            "openai",
            "chatgpt",
            "stored",
            "active",
            "chatgpt oauth (ready)",
        )]
    );
    assert_secret_free(&first_output, &chatgpt_only.canaries, &[]);

    let second_output = run(&chatgpt_only.anchor, ["auth", "status"], &chatgpt_only.env);
    assert_eq!(parse_status(&second_output), first);
    assert_store_unchanged(&auth_path, &bytes_before, modified_before);
    chatgpt_only.server.assert_finished();

    let (stored_only, stored_env, stored_server) = seed_api_key("openai", STORED_OPENAI_KEY);
    let stored_output = run(&stored_only, ["auth", "status"], &stored_env);
    assert_eq!(
        parse_status(&stored_output),
        [status_line(
            "openai",
            "api_key",
            "stored",
            "active",
            "stored:sk-s***ry",
        )]
    );
    assert_secret_free(
        &stored_output,
        &TokenCanaries::new("unused"),
        &[STORED_OPENAI_KEY],
    );
    stored_server.assert_finished();

    let mixed = seed_chatgpt("status-mixed-persisted");
    seed_additional_api_key(&mixed.anchor, &mixed.env, "openai", STORED_OPENAI_KEY);
    seed_additional_api_key(&mixed.anchor, &mixed.env, "anthropic", STORED_ANTHROPIC_KEY);
    let mixed_output = run(&mixed.anchor, ["auth", "status"], &mixed.env);
    assert_eq!(
        parse_status(&mixed_output),
        [
            status_line(
                "anthropic",
                "api_key",
                "stored",
                "active",
                "stored:sk-s***ry",
            ),
            status_line(
                "openai",
                "chatgpt",
                "stored",
                "ready",
                "chatgpt oauth (ready)",
            ),
            status_line("openai", "api_key", "stored", "active", "stored:sk-s***ry",),
        ]
    );
    assert_secret_free(
        &mixed_output,
        &mixed.canaries,
        &[STORED_OPENAI_KEY, STORED_ANTHROPIC_KEY],
    );
    mixed.server.assert_finished();

    let env_precedence = seed_chatgpt("status-env-precedence");
    let status_env = env_precedence
        .env
        .clone()
        .set(OPENAI_API_KEY_ENV, ENV_OPENAI_KEY);
    let env_output = run(&env_precedence.anchor, ["auth", "status"], &status_env);
    assert_eq!(
        parse_status(&env_output),
        [
            status_line(
                "openai",
                "chatgpt",
                "stored",
                "ready",
                "chatgpt oauth (ready)",
            ),
            status_line("openai", "api_key", "env", "active", "env:OPENAI_API_KEY",),
        ]
    );
    assert_secret_free(&env_output, &env_precedence.canaries, &[ENV_OPENAI_KEY]);
    env_precedence.server.assert_finished();
}

#[test]
fn status_blocks_expired_and_incomplete_chatgpt_without_rewriting_store() {
    let expired = seed_chatgpt("status-expired");
    mutate_chatgpt_credentials(expired.anchor.home(), |credentials| {
        credentials["expires_at"] = Value::from(Utc::now().timestamp_millis() - 60_000);
    });
    let expired_path = expired.anchor.home().join("auth.json");
    let expired_bytes = fs::read(&expired_path).expect("read expired auth store");
    let expired_mtime = fs::metadata(&expired_path)
        .expect("expired auth metadata")
        .modified()
        .expect("expired auth mtime");
    let expired_output = run(&expired.anchor, ["auth", "status"], &expired.env);
    assert_eq!(
        parse_status(&expired_output),
        [status_line(
            "openai",
            "chatgpt",
            "stored",
            "blocked",
            "chatgpt oauth (expired)",
        )]
    );
    assert_secret_free(&expired_output, &expired.canaries, &[]);
    assert_store_unchanged(&expired_path, &expired_bytes, expired_mtime);
    expired.server.assert_finished();

    let incomplete = seed_chatgpt("status-incomplete");
    mutate_chatgpt_credentials(incomplete.anchor.home(), |credentials| {
        credentials["refresh_token"] = Value::String(String::new());
    });
    let incomplete_path = incomplete.anchor.home().join("auth.json");
    let incomplete_bytes = fs::read(&incomplete_path).expect("read incomplete auth store");
    let incomplete_mtime = fs::metadata(&incomplete_path)
        .expect("incomplete auth metadata")
        .modified()
        .expect("incomplete auth mtime");
    let incomplete_output = run(&incomplete.anchor, ["auth", "status"], &incomplete.env);
    assert_eq!(
        parse_status(&incomplete_output),
        [status_line(
            "openai",
            "chatgpt",
            "stored",
            "blocked",
            "chatgpt oauth (incomplete)",
        )]
    );
    assert_secret_free(&incomplete_output, &incomplete.canaries, &[]);
    assert_store_unchanged(&incomplete_path, &incomplete_bytes, incomplete_mtime);
    incomplete.server.assert_finished();
}

#[test]
fn provider_logout_removes_only_persisted_openai_entries_and_is_idempotent() {
    let fixture = seed_chatgpt("provider-logout");
    seed_additional_api_key(&fixture.anchor, &fixture.env, "openai", STORED_OPENAI_KEY);
    seed_additional_api_key(
        &fixture.anchor,
        &fixture.env,
        "anthropic",
        STORED_ANTHROPIC_KEY,
    );
    let auth_path = fixture.anchor.home().join("auth.json");
    let before = fs::read_to_string(&auth_path).expect("read provider logout seed");
    let anthropic_before = provider_entry_bytes(&before, "anthropic");
    let logout_env = fixture.env.clone().set(OPENAI_API_KEY_ENV, ENV_OPENAI_KEY);

    let logout_output = run(
        &fixture.anchor,
        ["auth", "logout", "--provider", "openai"],
        &logout_env,
    );
    assert!(
        logout_output.contains("removed 2 persisted auth entry(s)"),
        "{logout_output}"
    );
    assert_secret_free(
        &logout_output,
        &fixture.canaries,
        &[STORED_OPENAI_KEY, STORED_ANTHROPIC_KEY, ENV_OPENAI_KEY],
    );

    let after = fs::read_to_string(&auth_path).expect("read provider logout result");
    assert_eq!(provider_entry_bytes(&after, "anthropic"), anthropic_before);
    let entries = auth_entries(&after);
    assert_eq!(entries.len(), 1, "{after}");
    assert_eq!(entries[0]["provider"], "anthropic");
    assert_eq!(entries[0]["source"]["api_key"], STORED_ANTHROPIC_KEY);
    assert_secure_atomic_store(&auth_path);

    let status_output = run(&fixture.anchor, ["auth", "status"], &logout_env);
    assert_eq!(
        parse_status(&status_output),
        [
            status_line(
                "anthropic",
                "api_key",
                "stored",
                "active",
                "stored:sk-s***ry",
            ),
            status_line("openai", "api_key", "env", "active", "env:OPENAI_API_KEY",),
        ]
    );
    assert_secret_free(
        &status_output,
        &fixture.canaries,
        &[STORED_OPENAI_KEY, STORED_ANTHROPIC_KEY, ENV_OPENAI_KEY],
    );

    let repeated = run(
        &fixture.anchor,
        ["auth", "logout", "--provider", "openai"],
        &logout_env,
    );
    assert!(
        repeated.contains("removed 0 persisted auth entry(s)"),
        "{repeated}"
    );
    let repeated_bytes = fs::read_to_string(&auth_path).expect("read repeated logout result");
    assert_eq!(
        provider_entry_bytes(&repeated_bytes, "anthropic"),
        anthropic_before
    );
    assert_secure_atomic_store(&auth_path);
    fixture.server.assert_finished();
}

#[test]
fn global_logout_removes_all_persisted_entries_but_not_environment_state() {
    let fixture = seed_chatgpt("global-logout");
    seed_additional_api_key(&fixture.anchor, &fixture.env, "openai", STORED_OPENAI_KEY);
    seed_additional_api_key(
        &fixture.anchor,
        &fixture.env,
        "anthropic",
        STORED_ANTHROPIC_KEY,
    );
    let logout_env = fixture.env.clone().set(OPENAI_API_KEY_ENV, ENV_OPENAI_KEY);

    let logout_output = run(&fixture.anchor, ["auth", "logout"], &logout_env);
    assert!(
        logout_output.contains("removed 3 persisted auth entry(s)"),
        "{logout_output}"
    );
    assert_secret_free(
        &logout_output,
        &fixture.canaries,
        &[STORED_OPENAI_KEY, STORED_ANTHROPIC_KEY, ENV_OPENAI_KEY],
    );
    let auth_path = fixture.anchor.home().join("auth.json");
    let after = fs::read_to_string(&auth_path).expect("read global logout result");
    assert!(auth_entries(&after).is_empty(), "{after}");
    assert_secure_atomic_store(&auth_path);

    let status_output = run(&fixture.anchor, ["auth", "status"], &logout_env);
    assert_eq!(
        parse_status(&status_output),
        [status_line(
            "openai",
            "api_key",
            "env",
            "active",
            "env:OPENAI_API_KEY",
        )]
    );
    assert_secret_free(&status_output, &fixture.canaries, &[ENV_OPENAI_KEY]);

    let repeated = run(&fixture.anchor, ["auth", "logout"], &logout_env);
    assert!(
        repeated.contains("removed 0 persisted auth entry(s)"),
        "{repeated}"
    );
    assert_secure_atomic_store(&auth_path);
    fixture.server.assert_finished();
}

struct ChatGptFixture {
    anchor: CliProcess,
    env: OpenAiTestEnv,
    server: ScriptedServer,
    canaries: TokenCanaries,
}

fn seed_chatgpt(label: &str) -> ChatGptFixture {
    let canaries = TokenCanaries::new(label);
    let server = ScriptedServer::start([ExpectedRequest::browser_token_exchange(&canaries)]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut anchor = CliProcess::spawn(
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
    let authorization_url = anchor.wait_for_stdout_prefix("open ", DEADLINE);
    let redirect_uri = anchor.wait_for_stdout_prefix("listening ", DEADLINE);
    let metadata = inspect_authorization_url(&authorization_url);
    let callback = callback_get(
        &redirect_uri,
        &format!("{label}-authorization-code"),
        &metadata.state,
    );
    assert!(callback.starts_with("HTTP/1.1 200 OK"), "{callback}");
    let _ = server.wait_for_request(DEADLINE);
    anchor.assert_success();
    assert!(anchor.home().join("auth.json").is_file());

    ChatGptFixture {
        anchor,
        env,
        server,
        canaries,
    }
}

fn seed_api_key(provider: &str, api_key: &str) -> (CliProcess, OpenAiTestEnv, ScriptedServer) {
    let server = ScriptedServer::start([]);
    let env = OpenAiTestEnv::for_server(&server);
    let mut anchor = CliProcess::spawn(
        [
            "auth",
            "login",
            "--provider",
            provider,
            "--method",
            "api-key",
            "--token",
            api_key,
        ],
        &env,
    );
    anchor.assert_success();
    (anchor, env, server)
}

fn seed_additional_api_key(
    anchor: &CliProcess,
    env: &OpenAiTestEnv,
    provider: &str,
    api_key: &str,
) {
    let mut child = anchor.spawn_again(
        [
            "auth",
            "login",
            "--provider",
            provider,
            "--method",
            "api-key",
            "--token",
            api_key,
        ],
        env,
    );
    child.assert_success();
}

fn run<const N: usize>(anchor: &CliProcess, args: [&str; N], env: &OpenAiTestEnv) -> String {
    let mut child = anchor.spawn_again(args, env);
    child.assert_success();
    child.output()
}

fn parse_status(output: &str) -> Vec<StatusLine> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("[stdout] "))
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let provider = fields.next()?;
            if !matches!(provider, "anthropic" | "openai" | "gemini" | "grok") {
                return None;
            }
            Some(StatusLine {
                provider: provider.to_string(),
                method: fields.next()?.to_string(),
                storage: fields.next()?.to_string(),
                state: fields.next()?.to_string(),
                summary: fields.collect::<Vec<_>>().join(" "),
            })
        })
        .collect()
}

fn status_line(
    provider: &str,
    method: &str,
    storage: &str,
    state: &str,
    summary: &str,
) -> StatusLine {
    StatusLine {
        provider: provider.to_string(),
        method: method.to_string(),
        storage: storage.to_string(),
        state: state.to_string(),
        summary: summary.to_string(),
    }
}

fn assert_secret_free(output: &str, canaries: &TokenCanaries, extra: &[&str]) {
    for secret in [
        canaries.id_token.as_str(),
        canaries.access_token.as_str(),
        canaries.refresh_token.as_str(),
        canaries.account_id.as_str(),
        canaries.email.as_str(),
        canaries.plan.as_str(),
    ]
    .into_iter()
    .chain(extra.iter().copied())
    {
        assert!(
            !output.contains(secret),
            "CLI output leaked {secret:?}\n{output}"
        );
    }
}

fn mutate_chatgpt_credentials(home: &Path, mutate: impl FnOnce(&mut Value)) {
    let path = home.join("auth.json");
    let mut auth: Value = serde_json::from_slice(&fs::read(&path).expect("read auth store"))
        .expect("parse auth store");
    let credentials = auth["entries"]
        .as_array_mut()
        .expect("auth entries")
        .iter_mut()
        .find(|entry| entry["provider"] == "openai" && entry["method"] == "chatgpt")
        .map(|entry| &mut entry["source"]["credentials"])
        .expect("ChatGPT credentials");
    assert!(credentials.is_object(), "ChatGPT credentials object");
    mutate(credentials);
    fs::write(
        path,
        serde_json::to_vec_pretty(&auth).expect("serialize mutated auth store"),
    )
    .expect("write mutated auth store");
}

fn assert_store_unchanged(
    path: &Path,
    expected_bytes: &[u8],
    expected_modified: std::time::SystemTime,
) {
    assert_eq!(
        fs::read(path).expect("read auth store after status"),
        expected_bytes,
        "auth status rewrote auth.json"
    );
    assert_eq!(
        fs::metadata(path)
            .expect("auth metadata after status")
            .modified()
            .expect("auth mtime after status"),
        expected_modified,
        "auth status changed auth.json mtime"
    );
    assert!(!path.with_extension("json.tmp").exists());
}

fn auth_entries(contents: &str) -> Vec<Value> {
    serde_json::from_str::<Value>(contents)
        .expect("parse auth store")
        .get("entries")
        .and_then(Value::as_array)
        .expect("auth entries")
        .clone()
}

fn provider_entry_bytes(contents: &str, provider: &str) -> Vec<u8> {
    let marker = format!("\"provider\": \"{provider}\"");
    let marker_index = contents.find(&marker).expect("provider entry marker");
    let start = contents[..marker_index]
        .rfind('{')
        .expect("provider entry object start");
    let bytes = contents.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes[start..].iter().copied().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return bytes[start..=start + offset].to_vec();
                }
            }
            _ => {}
        }
    }
    panic!("unterminated provider entry object")
}

fn assert_secure_atomic_store(path: &Path) {
    assert!(!path.with_extension("json.tmp").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mode = fs::metadata(path)
            .expect("auth store metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
