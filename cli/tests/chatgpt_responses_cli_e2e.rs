//! Shipped-product coverage for ChatGPT Responses tool continuation.
//!
//! The scenario intentionally crosses real CLI process, persisted OAuth,
//! provider HTTP, streamed tool execution, transcript, and restart boundaries.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::chatgpt_auth::{
    CliProcess, ExpectedRequest, OpenAiTestEnv, RecordedRequest, ScriptedServer, TokenCanaries,
    callback_get, inspect_authorization_url,
};

const DEADLINE: Duration = Duration::from_secs(10);
const RESPONSES_PATH: &str = "/backend-api/codex/responses";
const MODEL: &str = "gpt-5.6-sol";
const INITIAL_PROMPT: &str = "Read the continuation fixture, then report its value.";
const FOLLOW_UP_PROMPT: &str = "Confirm that the persisted tool result is still available.";
const FIXTURE_FILE: &str = "continuation-fixture.txt";
const TOOL_RESULT: &str = "responses-tool-result-canary\n";
const TOOL_CALL_ID: &str = "call-responses-read";
const TOOL_ITEM_ID: &str = "item-responses-read";
const REASONING_SUMMARY: &str = "I should read the deterministic fixture.";
const ENCRYPTED_REASONING: &str = "encrypted-reasoning-continuation-canary";
const FINAL_ANSWER: &str = "RESPONSES_TOOL_CONTINUATION_OK";
const RESUME_ANSWER: &str = "RESPONSES_RESUME_OK";
const SAFE_CUSTOM_HEADER: &str = "responses-continuation";
const OVERRIDE_BEARER: &str = "custom-header-bearer-must-not-win";
const OVERRIDE_ACCOUNT: &str = "custom-header-account-must-not-win";

const SETTINGS: &str = r#"{
    "model": "gpt-5.6-sol",
    "env": {
        "ANTHROPIC_CUSTOM_HEADERS": "Authorization: Bearer custom-header-bearer-must-not-win\nChatGPT-Account-ID: custom-header-account-must-not-win\nOriginator: custom-originator-must-not-win\nSession-ID: custom-session-must-not-win\nX-E2E-Marker: responses-continuation"
    }
}"#;

#[test]
fn persisted_chatgpt_responses_executes_tool_and_continues_after_restart() {
    let canaries = TokenCanaries::new("responses-continuation");
    let server = ScriptedServer::start([
        ExpectedRequest::browser_token_exchange(&canaries),
        ExpectedRequest::responses_sse(RESPONSES_PATH, &tool_call_events()),
        ExpectedRequest::responses_sse(RESPONSES_PATH, &final_text_events()),
        ExpectedRequest::responses_sse(RESPONSES_PATH, &resume_text_events()),
    ]);
    let env = OpenAiTestEnv::for_server(&server);
    let anchor = complete_browser_login(&server, &env, &canaries);
    fs::write(anchor.cwd().join(FIXTURE_FILE), TOOL_RESULT).expect("write tool fixture");

    let mut prompt = anchor.spawn_again(prompt_args(INITIAL_PROMPT, None), &env);
    prompt.assert_success();
    let first_output = prompt.output();
    let first_records = stdout_json_records(&first_output);
    let session_id = record_session_id(&first_records);

    assert_first_cli_result(&first_records);
    assert_tool_lifecycle(&first_records);

    let transcript_path = transcript_path(anchor.home(), &session_id);
    let first_transcript = fs::read_to_string(&transcript_path).expect("read first transcript");
    let first_entries = jsonl_records(&first_transcript);
    assert_transcript_tool_round(&first_entries);
    assert_no_auth_material(&first_transcript, &canaries);

    let first_requests = server.requests();
    let first_responses = response_requests(&first_requests);
    assert_eq!(
        first_responses.len(),
        2,
        "tool round must use two Responses requests"
    );
    assert_initial_request(first_responses[0], &canaries, &session_id);
    assert_continuation_request(first_responses[1], &canaries, &session_id);
    assert_no_chat_completions(&first_requests);

    let mut resume = anchor.spawn_again(prompt_args(FOLLOW_UP_PROMPT, Some(&session_id)), &env);
    resume.assert_success();
    let resume_output = resume.output();
    let resume_records = stdout_json_records(&resume_output);
    assert_eq!(record_session_id(&resume_records), session_id);
    assert_success_result(&resume_records, RESUME_ANSWER, 26);

    let all_requests = server.requests();
    let responses = response_requests(&all_requests);
    assert_eq!(
        responses.len(),
        3,
        "two tool-round requests must be followed by one resume request"
    );
    assert_resume_request(responses[2], &canaries, &session_id);
    assert_no_chat_completions(&all_requests);

    let resumed_transcript = fs::read_to_string(&transcript_path).expect("read resumed transcript");
    assert!(resumed_transcript.contains(FINAL_ANSWER));
    assert!(resumed_transcript.contains(RESUME_ANSWER));
    assert_no_auth_material(&resumed_transcript, &canaries);
    assert_non_auth_files_secret_free(anchor.root(), &canaries);

    let combined_output = format!("{first_output}{resume_output}");
    canaries.assert_secrets_absent(&combined_output, all_requests.iter().cloned());
    assert!(!combined_output.contains("Authorization: Bearer"));
    assert!(!combined_output.contains(OVERRIDE_BEARER));
    assert!(!combined_output.contains(OVERRIDE_ACCOUNT));

    server.assert_finished();
}

fn complete_browser_login(
    server: &ScriptedServer,
    env: &OpenAiTestEnv,
    canaries: &TokenCanaries,
) -> CliProcess {
    let mut cli = CliProcess::spawn(
        [
            "auth",
            "login",
            "--provider",
            "openai",
            "--method",
            "chatgpt",
        ],
        env,
    );
    let authorization_url = cli.wait_for_stdout_prefix("open ", DEADLINE);
    let redirect_uri = cli.wait_for_stdout_prefix("listening ", DEADLINE);
    let authorization = inspect_authorization_url(&authorization_url);
    let callback = callback_get(
        &redirect_uri,
        "responses-continuation-authorization-code",
        &authorization.state,
    );
    assert!(callback.starts_with("HTTP/1.1 200 OK"), "{callback}");
    let exchange = server.wait_for_request(DEADLINE);
    assert_eq!(exchange.path, "/oauth/token");
    assert!(!exchange.authorization_header_present);
    cli.assert_success();

    let auth = fs::read_to_string(cli.home().join("auth.json")).expect("read persisted login");
    assert!(auth.contains(&canaries.access_token));
    assert!(!auth.contains("\"api_key\""));
    cli
}

fn prompt_args(prompt: &str, resume: Option<&str>) -> Vec<String> {
    let mut args = vec!["-p".to_string(), prompt.to_string()];
    if let Some(session_id) = resume {
        args.extend(["--resume".to_string(), session_id.to_string()]);
    }
    args.extend([
        "--provider".to_string(),
        "openai".to_string(),
        "--settings".to_string(),
        SETTINGS.to_string(),
        "--allow-tools".to_string(),
        "true".to_string(),
        "--allowed-tools".to_string(),
        "Read".to_string(),
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
    ]);
    args
}

fn tool_call_events() -> Vec<Value> {
    let arguments = json!({ "file_path": FIXTURE_FILE }).to_string();
    let split = arguments.find("path").expect("argument split marker");
    vec![
        json!({
            "type": "response.created",
            "response": { "id": "resp-tool", "status": "in_progress" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "delta": REASONING_SUMMARY
        }),
        json!({
            "type": "response.output_item.done",
            "item": {
                "id": "reasoning-tool",
                "type": "reasoning",
                "encrypted_content": ENCRYPTED_REASONING
            }
        }),
        json!({
            "type": "response.output_item.added",
            "item": {
                "id": TOOL_ITEM_ID,
                "type": "function_call",
                "call_id": TOOL_CALL_ID,
                "name": "Read"
            }
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": TOOL_ITEM_ID,
            "delta": &arguments[..split]
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": TOOL_ITEM_ID,
            "delta": &arguments[split..]
        }),
        json!({
            "type": "response.output_item.done",
            "item": {
                "id": TOOL_ITEM_ID,
                "type": "function_call",
                "call_id": TOOL_CALL_ID,
                "name": "Read",
                "arguments": arguments
            }
        }),
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp-tool",
                "status": "completed",
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 3,
                    "total_tokens": 14
                }
            }
        }),
    ]
}

fn final_text_events() -> Vec<Value> {
    vec![
        json!({
            "type": "response.created",
            "response": { "id": "resp-final", "status": "in_progress" }
        }),
        json!({ "type": "response.output_text.delta", "delta": FINAL_ANSWER }),
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp-final",
                "status": "completed",
                "usage": {
                    "input_tokens": 17,
                    "output_tokens": 4,
                    "total_tokens": 21
                }
            }
        }),
    ]
}

fn resume_text_events() -> Vec<Value> {
    vec![
        json!({
            "type": "response.created",
            "response": { "id": "resp-resume", "status": "in_progress" }
        }),
        json!({ "type": "response.output_text.delta", "delta": RESUME_ANSWER }),
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp-resume",
                "status": "completed",
                "usage": {
                    "input_tokens": 23,
                    "output_tokens": 3,
                    "total_tokens": 26
                }
            }
        }),
    ]
}

fn stdout_json_records(output: &str) -> Vec<Value> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("[stdout] "))
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("stdout line is stream-json"))
        .collect()
}

fn record_session_id(records: &[Value]) -> String {
    records
        .first()
        .and_then(|record| record.get("session_id"))
        .and_then(Value::as_str)
        .expect("system init session id")
        .to_string()
}

fn assert_first_cli_result(records: &[Value]) {
    // The terminal CLI result reports the final provider round; both rounds'
    // usage is preserved on their individual assistant transcript entries.
    assert_success_result(records, FINAL_ANSWER, 21);
    let init = records.first().expect("system init");
    assert_eq!(init["type"], "system");
    assert_eq!(init["model"], MODEL);
    assert!(
        init["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool == "file-read"))
    );
}

fn assert_success_result(records: &[Value], answer: &str, total_tokens: u64) {
    let result = records.last().expect("result record");
    assert_eq!(result["type"], "result");
    assert_eq!(result["subtype"], "success");
    assert_eq!(result["is_error"], false);
    assert_eq!(result["result"], answer);
    assert_eq!(result["usage"]["total_tokens"], total_tokens);
    assert_eq!(result["total_cost_usd"], 0.0);
    assert_eq!(
        result["pricing_known"], false,
        "subscription usage must not be represented as API-priced"
    );
    assert_eq!(result["modelUsage"][MODEL]["costUSD"], 0.0);
}

fn assert_tool_lifecycle(records: &[Value]) {
    let events = records
        .iter()
        .filter(|record| record["type"] == "stream_event")
        .map(|record| &record["event"])
        .collect::<Vec<_>>();
    let starts = events
        .iter()
        .filter(|event| event["type"] == "tool_use_started")
        .collect::<Vec<_>>();
    let completions = events
        .iter()
        .filter(|event| event["type"] == "tool_use_completed")
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 1, "one tool start expected: {events:?}");
    assert_eq!(completions.len(), 1, "one terminal tool event expected");
    assert_eq!(starts[0]["tool_use_id"], TOOL_CALL_ID);
    assert_eq!(starts[0]["tool_name"], "Read");
    assert_eq!(completions[0]["tool_use_id"], TOOL_CALL_ID);
    assert_eq!(completions[0]["tool_name"], "Read");
    assert_eq!(completions[0]["kind"], "success");

    let assistant_tool = records
        .iter()
        .position(|record| message_has_block(record, "tool_use", TOOL_CALL_ID))
        .expect("assistant tool_use record");
    let user_result = records
        .iter()
        .position(|record| message_has_block(record, "tool_result", TOOL_CALL_ID))
        .expect("user tool_result record");
    assert!(assistant_tool < user_result);
}

fn message_has_block(record: &Value, block_type: &str, tool_use_id: &str) -> bool {
    record["message"]["content"]
        .as_array()
        .is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block["type"] == block_type
                    && (block["id"] == tool_use_id || block["tool_use_id"] == tool_use_id)
            })
        })
}

fn response_requests(requests: &[RecordedRequest]) -> Vec<&RecordedRequest> {
    requests
        .iter()
        .filter(|request| request.path == RESPONSES_PATH)
        .collect()
}

fn assert_initial_request(request: &RecordedRequest, canaries: &TokenCanaries, session_id: &str) {
    assert_common_request(request, canaries, session_id);
    let body = request_body(request);
    assert_eq!(body["model"], MODEL);
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert!(body.get("messages").is_none());
    let read = body["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "Read"))
        .expect("Read tool definition");
    assert_eq!(read["type"], "function");
    assert!(read["parameters"]["properties"].get("file_path").is_some());
    assert!(
        input(&body)
            .iter()
            .all(|item| item["type"] != "function_call" && item["type"] != "function_call_output")
    );
}

fn assert_continuation_request(
    request: &RecordedRequest,
    canaries: &TokenCanaries,
    session_id: &str,
) {
    assert_common_request(request, canaries, session_id);
    let body = request_body(request);
    let input = input(&body);
    assert_tool_pairing(input);
    assert_reasoning_replay(input);
    let reasoning = index_of(input, |item| item["type"] == "reasoning");
    let call = index_of(input, |item| item["type"] == "function_call");
    let output = index_of(input, |item| item["type"] == "function_call_output");
    assert!(
        reasoning < call && call < output,
        "source order changed: {input:?}"
    );
}

fn assert_resume_request(request: &RecordedRequest, canaries: &TokenCanaries, session_id: &str) {
    assert_common_request(request, canaries, session_id);
    let body = request_body(request);
    let input = input(&body);
    assert_tool_pairing(input);
    assert_reasoning_replay(input);

    let reasoning = index_of(input, |item| item["type"] == "reasoning");
    let call = index_of(input, |item| item["type"] == "function_call");
    let output = index_of(input, |item| item["type"] == "function_call_output");
    let final_answer = index_of(input, |item| message_text(item) == Some(FINAL_ANSWER));
    let follow_up = index_of(input, |item| message_text(item) == Some(FOLLOW_UP_PROMPT));
    assert!(
        reasoning < call && call < output && output < final_answer && final_answer < follow_up,
        "persisted source order changed after restart: {input:?}"
    );
}

fn assert_common_request(request: &RecordedRequest, canaries: &TokenCanaries, session_id: &str) {
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, RESPONSES_PATH);
    assert!(request.authorization_header_present);
    assert_eq!(request.header_count("authorization"), 1);
    assert_eq!(
        request.authorization_bearer_sha256(),
        Some(secret_sha256(&canaries.access_token).as_str())
    );
    assert_eq!(
        request.header_value("chatgpt-account-id"),
        Some(canaries.account_id.as_str())
    );
    assert_eq!(request.header_value("originator"), Some("orbcode"));
    assert_eq!(request.header_value("session-id"), Some(session_id));
    assert_eq!(
        request.header_value("x-e2e-marker"),
        Some(SAFE_CUSTOM_HEADER)
    );
    assert_eq!(request.header_count("x-api-key"), 0);
    assert_eq!(request.header_count("api-key"), 0);
    assert_ne!(
        request.header_value("chatgpt-account-id"),
        Some(OVERRIDE_ACCOUNT)
    );
    assert_ne!(
        request.authorization_bearer_sha256(),
        Some(secret_sha256(OVERRIDE_BEARER).as_str())
    );
}

fn request_body(request: &RecordedRequest) -> Value {
    serde_json::from_str(&request.sanitized_body).expect("captured Responses JSON")
}

fn input(body: &Value) -> &[Value] {
    body["input"].as_array().expect("Responses input")
}

fn assert_tool_pairing(input: &[Value]) {
    let calls = input
        .iter()
        .filter(|item| item["type"] == "function_call" && item["call_id"] == TOOL_CALL_ID)
        .collect::<Vec<_>>();
    let outputs = input
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == TOOL_CALL_ID)
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1, "function call must appear once: {input:?}");
    assert_eq!(outputs.len(), 1, "tool output must appear once: {input:?}");
    assert_eq!(calls[0]["name"], "Read");
    assert_eq!(
        serde_json::from_str::<Value>(calls[0]["arguments"].as_str().expect("arguments JSON"))
            .expect("arguments parse"),
        json!({ "file_path": FIXTURE_FILE })
    );
    assert_eq!(outputs[0]["output"], TOOL_RESULT);
}

fn assert_reasoning_replay(input: &[Value]) {
    let reasoning = input
        .iter()
        .filter(|item| item["type"] == "reasoning")
        .collect::<Vec<_>>();
    assert_eq!(reasoning.len(), 1, "encrypted reasoning must appear once");
    assert_eq!(reasoning[0]["encrypted_content"], ENCRYPTED_REASONING);
    assert_eq!(reasoning[0]["summary"][0]["type"], "summary_text");
    assert_eq!(reasoning[0]["summary"][0]["text"], REASONING_SUMMARY);
    for item in input {
        assert_ne!(item["type"], "thinking");
        assert_ne!(item["type"], "redacted_thinking");
        assert!(item.get("signature").is_none());
        assert!(item.get("data").is_none());
    }
}

fn index_of(input: &[Value], predicate: impl Fn(&Value) -> bool) -> usize {
    input
        .iter()
        .position(predicate)
        .expect("expected input item")
}

fn message_text(item: &Value) -> Option<&str> {
    item.get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
}

fn assert_no_chat_completions(requests: &[RecordedRequest]) {
    assert!(
        requests
            .iter()
            .all(|request| !request.path.ends_with("/chat/completions")),
        "ChatGPT login must never route a prompt through Chat Completions"
    );
}

fn transcript_path(home: &Path, session_id: &str) -> PathBuf {
    let expected = format!("{session_id}.jsonl");
    regular_files(home)
        .into_iter()
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name == expected.as_str())
        })
        .expect("session transcript")
}

fn jsonl_records(contents: &str) -> Vec<Value> {
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid transcript JSONL"))
        .collect()
}

fn assert_transcript_tool_round(entries: &[Value]) {
    let assistant_index = entries
        .iter()
        .position(|entry| transcript_block(entry, "tool_use", TOOL_CALL_ID).is_some())
        .expect("assistant tool_use transcript entry");
    let result_index = entries
        .iter()
        .position(|entry| transcript_block(entry, "tool_result", TOOL_CALL_ID).is_some())
        .expect("user tool_result transcript entry");
    let final_index = entries
        .iter()
        .position(|entry| transcript_text(entry) == Some(FINAL_ANSWER))
        .expect("persisted final answer");
    assert!(assistant_index < result_index && result_index < final_index);

    let assistant = &entries[assistant_index];
    let blocks = assistant["message"]["content"]
        .as_array()
        .expect("assistant content blocks");
    assert_eq!(blocks[0]["type"], "thinking");
    assert_eq!(blocks[0]["thinking"], REASONING_SUMMARY);
    assert_eq!(blocks[0]["signature"], ENCRYPTED_REASONING);
    assert_eq!(blocks[1]["type"], "tool_use");
    assert_eq!(blocks[1]["id"], TOOL_CALL_ID);
    assert_eq!(blocks[1]["name"], "Read");
    assert_eq!(blocks[1]["input"], json!({ "file_path": FIXTURE_FILE }));
    assert_eq!(assistant["provider"], "openai");
    assert_eq!(assistant["billingBasis"], "subscription");
    assert_eq!(assistant["message"]["model"], MODEL);
    assert_eq!(assistant["message"]["usage"]["total_tokens"], 14);

    let result = transcript_block(&entries[result_index], "tool_result", TOOL_CALL_ID)
        .expect("tool result block");
    assert_eq!(result["content"], TOOL_RESULT);
    assert_eq!(result["is_error"], false);

    let final_entry = &entries[final_index];
    assert_eq!(final_entry["provider"], "openai");
    assert_eq!(final_entry["billingBasis"], "subscription");
    assert_eq!(final_entry["message"]["usage"]["total_tokens"], 21);
    for entry in entries {
        if let Some(blocks) = entry["message"]["content"].as_array() {
            for block in blocks {
                if matches!(block["type"].as_str(), Some("text" | "thinking")) {
                    let visible = block["text"]
                        .as_str()
                        .or_else(|| block["thinking"].as_str())
                        .unwrap_or_default();
                    assert!(!visible.contains(ENCRYPTED_REASONING));
                }
            }
        }
    }
}

fn transcript_block<'a>(entry: &'a Value, kind: &str, id: &str) -> Option<&'a Value> {
    entry["message"]["content"]
        .as_array()?
        .iter()
        .find(|block| block["type"] == kind && (block["id"] == id || block["tool_use_id"] == id))
}

fn transcript_text(entry: &Value) -> Option<&str> {
    entry["message"]["content"]
        .as_array()?
        .iter()
        .find(|block| block["type"] == "text")?
        .get("text")?
        .as_str()
}

fn assert_no_auth_material(contents: &str, canaries: &TokenCanaries) {
    for secret in [
        canaries.id_token.as_str(),
        canaries.access_token.as_str(),
        canaries.refresh_token.as_str(),
    ] {
        assert!(
            !contents.contains(secret),
            "transcript persisted an auth token"
        );
    }
    assert!(!contents.contains("Authorization: Bearer"));
}

fn assert_non_auth_files_secret_free(root: &Path, canaries: &TokenCanaries) {
    for path in regular_files(root) {
        if path.file_name().is_some_and(|name| name == "auth.json") {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        assert_no_auth_material(&contents, canaries);
    }
}

fn regular_files(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(path) = pending.pop() {
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.is_file() {
            files.push(path);
            continue;
        }
        let Ok(entries) = fs::read_dir(path) else {
            continue;
        };
        pending.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
    }
    files
}

fn secret_sha256(value: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}
