use super::*;
use orbcode_mcp::{McpAuth, McpServerConfig, McpTransport};
use std::sync::atomic::AtomicBool;

#[tokio::test]
async fn todo_and_mcp_adapters_work() {
    let registry = ToolRegistry::foundation();
    let context = test_context("mcp").await;
    context
        .mcp
        .upsert_server(McpServerConfig {
            id: "docs".to_string(),
            transport: McpTransport::WebSocket,
            endpoint: "modeled://example.com".to_string(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            cwd: None,
            headers: std::collections::BTreeMap::new(),
            enabled: true,
            status: orbcode_mcp::McpServerStatus::Ready,
            error: None,
            summary: "Docs".to_string(),
            auth: McpAuth::None,
            trust: orbcode_mcp::McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("seed docs server");

    let todo = registry
        .invoke(
            "todo-write",
            r#"{"list":"phase3","items":["tool registry","mcp registry"]}"#,
            &context,
        )
        .await
        .expect("todo write");
    assert!(todo.output.contains("phase3.json"));

    let tool_call = registry
        .invoke("mcp__docs__inspect", r#"{"mode":"brief"}"#, &context)
        .await
        .expect("call mcp provider tool");
    assert!(tool_call.output.contains("server=docs"));
}

#[tokio::test]
async fn untrusted_mcp_server_refuses_model_initiated_tool_calls() {
    let registry = ToolRegistry::foundation();
    let context = test_context("mcp-untrusted").await;
    context
        .mcp
        .upsert_server(McpServerConfig {
            id: "pending".to_string(),
            transport: McpTransport::WebSocket,
            endpoint: "modeled://example.com".to_string(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            cwd: None,
            headers: std::collections::BTreeMap::new(),
            enabled: true,
            status: orbcode_mcp::McpServerStatus::Ready,
            error: None,
            summary: "Pending trust".to_string(),
            auth: McpAuth::None,
            trust: orbcode_mcp::McpServerTrust::Unknown,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("seed pending server");
    context
        .mcp
        .set_server_trust("pending", orbcode_mcp::McpServerTrust::Unknown)
        .await
        .expect("reset pending server to unknown trust");

    let error = registry
        .invoke("mcp__pending__inspect", "", &context)
        .await
        .expect_err("untrusted server should refuse model-initiated calls");
    let message = error.to_string();
    assert!(
        message.contains("not trusted") && message.contains("pending"),
        "{message}"
    );
}

#[tokio::test]
async fn network_tools_require_network_permission() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("network").await;
    context.allow_network = false;

    let error = registry
        .invoke("web-fetch", "https://example.com", &context)
        .await
        .expect_err("web fetch should fail");
    assert!(matches!(error, ToolError::NetworkDenied));
}

#[tokio::test]
async fn tool_search_returns_matching_tool_schemas() {
    let registry = ToolRegistry::foundation();
    let context = test_context("tool-search").await;
    let result = registry
        .invoke(
            "ToolSearch",
            r#"{"query":"plan mode","max_results":3}"#,
            &context,
        )
        .await
        .expect("invoke tool search");
    let payload: Value = serde_json::from_str(&result.output).expect("parse tool search");
    assert!(
        payload["matches"]
            .as_array()
            .expect("matches array")
            .iter()
            .any(|item| item == "EnterPlanMode")
    );
    assert!(
        payload["functions"]
            .as_array()
            .expect("functions array")
            .iter()
            .any(|item| item["name"] == "ExitPlanMode")
    );
}

#[tokio::test]
async fn tool_search_returns_mcp_tools_and_resources() {
    let registry = ToolRegistry::foundation();
    let context = test_context("tool-search-mcp").await;
    context
        .mcp
        .upsert_server(McpServerConfig {
            id: "docs".to_string(),
            transport: McpTransport::WebSocket,
            endpoint: "modeled://example.com".to_string(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            cwd: None,
            headers: std::collections::BTreeMap::new(),
            enabled: true,
            status: orbcode_mcp::McpServerStatus::Ready,
            error: None,
            summary: "Docs".to_string(),
            auth: McpAuth::None,
            trust: orbcode_mcp::McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        })
        .await
        .expect("seed docs server");

    let result = registry
        .invoke(
            "ToolSearch",
            r#"{"query":"docs inspect","max_results":5}"#,
            &context,
        )
        .await
        .expect("invoke tool search");
    let payload: Value = serde_json::from_str(&result.output).expect("parse tool search");
    assert!(
        payload["functions"]
            .as_array()
            .expect("functions array")
            .iter()
            .any(|item| item["name"] == "mcp__docs__inspect")
    );
    assert!(
        payload["mcp_resources"]
            .as_array()
            .expect("resources array")
            .iter()
            .any(|item| item["uri"] == "mcp://docs/info")
    );
}

#[tokio::test]
async fn invoke_accepts_typescript_style_tool_aliases() {
    let registry = ToolRegistry::foundation();
    let context = test_context("provider-aliases").await;
    std::fs::write(context.cwd.join("notes.txt"), "alias line\n").expect("write notes");

    let bash = registry
        .invoke("Bash", r#"{"command":"printf alias-ok"}"#, &context)
        .await
        .expect("Bash alias should execute");
    assert_eq!(bash.output, "alias-ok");

    let read = registry
        .invoke("Read", r#"{"file_path":"notes.txt"}"#, &context)
        .await
        .expect("Read alias should execute");
    assert!(read.output.contains("alias line"));
}

#[test]
fn provider_definitions_bias_interactive_use_toward_task_tools() {
    let registry = ToolRegistry::foundation();
    let tools = registry.provider_definitions(true, true);

    let task_create = tools
        .iter()
        .find(|tool| tool.name == "TaskCreate")
        .expect("TaskCreate exposed to provider");
    assert!(
        task_create
            .description
            .contains("Prefer this over TodoWrite for interactive multi-step work"),
        "TaskCreate description should bias toward Task* tools: `{}`",
        task_create.description
    );

    let task_update = tools
        .iter()
        .find(|tool| tool.name == "TaskUpdate")
        .expect("TaskUpdate exposed to provider");
    assert!(
        task_update
            .description
            .contains("Move a task to `in_progress`"),
        "TaskUpdate description should remind the model to bump status"
    );

    let todo_write = tools
        .iter()
        .find(|tool| tool.name == "TodoWrite")
        .expect("TodoWrite still exposed for compatibility");
    assert!(
        todo_write
            .description
            .contains("Legacy/compatibility tool: prefer TaskCreate"),
        "TodoWrite should remain available but mark itself legacy"
    );
}

#[test]
fn provider_definitions_respect_permission_visibility() {
    let registry = ToolRegistry::foundation();

    let restricted = registry.provider_definitions(false, false);
    assert!(
        !restricted.iter().any(|tool| tool.name == "AskUserQuestion"),
        "AskUserQuestion is provider_hidden and must not appear in provider definitions"
    );
    assert!(restricted.iter().any(|tool| tool.name == "EnterPlanMode"));
    assert!(restricted.iter().any(|tool| tool.name == "ExitPlanMode"));
    assert!(restricted.iter().any(|tool| tool.name == "Skill"));
    assert!(restricted.iter().any(|tool| tool.name == "ToolSearch"));
    assert!(
        restricted
            .iter()
            .any(|tool| tool.name == "VerifyPlanExecution")
    );
    assert!(!restricted.iter().any(|tool| tool.name == "Agent"));
    assert!(!restricted.iter().any(|tool| tool.name == "Bash"));
    assert!(!restricted.iter().any(|tool| tool.name == "LSP"));
    assert!(!restricted.iter().any(|tool| tool.name == "WebFetch"));

    let tools_only = registry.provider_definitions(true, false);
    assert!(tools_only.iter().any(|tool| tool.name == "Agent"));
    assert!(tools_only.iter().any(|tool| tool.name == "Bash"));
    assert!(tools_only.iter().any(|tool| tool.name == "LSP"));
    assert!(!tools_only.iter().any(|tool| tool.name == "WebFetch"));

    let full = registry.provider_definitions(true, true);
    assert!(full.iter().any(|tool| tool.name == "Bash"));
    assert!(full.iter().any(|tool| tool.name == "Read"));
    assert!(full.iter().any(|tool| tool.name == "TaskCreate"));
    assert!(full.iter().any(|tool| tool.name == "TaskOutput"));
    assert!(full.iter().any(|tool| tool.name == "TaskStop"));
    assert!(full.iter().any(|tool| tool.name == "TaskUpdate"));
    assert!(full.iter().any(|tool| tool.name == "EnterPlanMode"));
    assert!(full.iter().any(|tool| tool.name == "ExitPlanMode"));
    assert!(full.iter().any(|tool| tool.name == "Skill"));
    assert!(full.iter().any(|tool| tool.name == "ToolSearch"));
    assert!(full.iter().any(|tool| tool.name == "LSP"));
    assert!(full.iter().any(|tool| tool.name == "VerifyPlanExecution"));
    assert!(full.iter().any(|tool| tool.name == "WebFetch"));
}

#[test]
fn parse_mcp_provider_tool_name_round_trips_server_and_tool() {
    use crate::{mcp_provider_tool_name, parse_mcp_provider_tool_name};

    assert_eq!(
        parse_mcp_provider_tool_name("mcp__docs__search"),
        Some(("docs", "search"))
    );
    assert_eq!(
        parse_mcp_provider_tool_name("mcp__docs__deep__search"),
        Some(("docs", "deep__search"))
    );
    assert_eq!(parse_mcp_provider_tool_name("bash"), None);
    assert_eq!(parse_mcp_provider_tool_name("mcp__docs"), None);
    assert_eq!(parse_mcp_provider_tool_name("mcp____search"), None);

    let name = mcp_provider_tool_name("docs", "search");
    assert_eq!(name, "mcp__docs__search");
    assert_eq!(
        parse_mcp_provider_tool_name(&name),
        Some(("docs", "search"))
    );
}

#[tokio::test]
async fn every_tool_honors_precancel_before_input_validation() {
    let registry = ToolRegistry::foundation();
    let mut context = test_context("precancel-all").await;
    let cancel_flag = Arc::new(AtomicBool::new(true));
    context.cancellation = ToolCancellationToken::from_flag(cancel_flag);

    for spec in registry.planned() {
        let result = registry
            .invoke(spec.name, "not valid input", &context)
            .await;
        assert!(
            matches!(
                result,
                Err(ToolError::Interrupted | ToolError::InterruptedWithMetadata { .. })
            ),
            "tool `{}` should stop on cancellation before validating input, got {result:?}",
            spec.name
        );
    }
}
