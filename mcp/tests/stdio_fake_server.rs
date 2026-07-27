use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[allow(unused_imports)]
use orbcode_mcp::McpToolDescriptor;
use orbcode_mcp::{
    McpCancellationToken, McpError, McpRegistry, McpServerStatus, McpServerTrust, StdioMcpClient,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command as TokioCommand};

#[tokio::test]
async fn stdio_fake_server_success_fixture_round_trips_json_rpc() {
    let mut server = spawn_fake_stdio_server().await;

    let response = server
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "echo",
                "arguments": { "text": "hello" }
            }
        }))
        .await;

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["content"][0]["type"], "text");
    assert_eq!(response["result"]["content"][0]["text"], "echo: hello");
    assert!(response.get("error").is_none(), "{response}");
}

#[tokio::test]
async fn stdio_fake_server_error_fixture_returns_json_rpc_error() {
    let mut server = spawn_fake_stdio_server().await;

    let response = server
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "fail",
                "arguments": {}
            }
        }))
        .await;

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 2);
    assert_eq!(response["error"]["code"], -32000);
    assert_eq!(response["error"]["message"], "fake tool failed");
    assert!(response.get("result").is_none(), "{response}");
}

#[tokio::test]
async fn stdio_client_initializes_fake_server() {
    let mut client = StdioMcpClient::spawn(
        fake_stdio_server_binary(),
        std::iter::empty::<&str>(),
        Duration::from_secs(30),
    )
    .await
    .expect("spawn stdio MCP client");

    let result = client.initialize().await.expect("initialize fake server");

    assert_eq!(result.protocol_version, "2024-11-05");
    assert_eq!(result.server_info.name, "orbcode-fake-stdio");
    assert_eq!(result.server_info.version, "0.1.0");
    assert!(result.capabilities.get("tools").is_some());
}

#[tokio::test]
async fn stdio_client_lists_fake_server_tools_with_schema() {
    let mut client = StdioMcpClient::spawn(
        fake_stdio_server_binary(),
        std::iter::empty::<&str>(),
        Duration::from_secs(30),
    )
    .await
    .expect("spawn stdio MCP client");

    client.initialize().await.expect("initialize fake server");
    let result = client.list_tools().await.expect("list fake tools");

    let echo = result
        .tools
        .iter()
        .find(|tool| tool.name == "echo")
        .expect("echo tool");
    assert_eq!(echo.description, "Echo test input.");
    assert_eq!(echo.input_schema["type"], "object");
    assert_eq!(echo.input_schema["properties"]["text"]["type"], "string");

    assert!(result.tools.iter().any(|tool| tool.name == "fail"));
}

#[tokio::test]
async fn stdio_client_calls_fake_server_tool_and_parses_text_content() {
    let mut client = StdioMcpClient::spawn(
        fake_stdio_server_binary(),
        std::iter::empty::<&str>(),
        Duration::from_secs(30),
    )
    .await
    .expect("spawn stdio MCP client");

    client.initialize().await.expect("initialize fake server");
    let result = client
        .call_tool("echo", json!({ "text": "hello" }))
        .await
        .expect("call fake echo tool");

    assert!(!result.is_error);
    assert_eq!(result.content.len(), 1);
    assert_eq!(result.content[0].kind, "text");
    assert_eq!(result.content[0].text.as_deref(), Some("echo: hello"));
}

#[tokio::test]
async fn stdio_client_returns_json_rpc_error_from_fake_tool_call() {
    let mut client = StdioMcpClient::spawn(
        fake_stdio_server_binary(),
        std::iter::empty::<&str>(),
        Duration::from_secs(30),
    )
    .await
    .expect("spawn stdio MCP client");

    client.initialize().await.expect("initialize fake server");
    let error = client
        .call_tool("fail", json!({}))
        .await
        .expect_err("fake fail tool returns JSON-RPC error");

    match error {
        orbcode_mcp::McpError::JsonRpc { code, message } => {
            assert_eq!(code, -32000);
            assert_eq!(message, "fake tool failed");
        }
        other => panic!("expected JSON-RPC error, got {other}"),
    }
}

#[tokio::test]
async fn stdio_client_reports_schema_errors_for_malformed_tools_list() {
    let mut env = BTreeMap::new();
    env.insert("FAKE_MCP_BAD_TOOLS_LIST".to_string(), "1".to_string());
    let mut client = StdioMcpClient::spawn_configured(
        fake_stdio_server_binary(),
        &[],
        &env,
        None,
        Duration::from_secs(30),
    )
    .await
    .expect("spawn fake MCP server with malformed tools/list");

    client
        .initialize()
        .await
        .expect("initialize fake server before malformed tools/list");
    let error = client
        .list_tools()
        .await
        .expect_err("malformed tools/list result should fail");

    assert!(matches!(error, McpError::Json(_)), "{error}");
    let _ = client.shutdown().await;
}

#[tokio::test]
async fn stdio_client_times_out_slow_startup() {
    let mut env = BTreeMap::new();
    env.insert("FAKE_MCP_INIT_DELAY_MS".to_string(), "250".to_string());
    let mut client = StdioMcpClient::spawn_configured(
        fake_stdio_server_binary(),
        &[],
        &env,
        None,
        Duration::from_millis(50),
    )
    .await
    .expect("spawn slow fake server");

    let error = client
        .initialize()
        .await
        .expect_err("slow initialize should time out");
    assert!(matches!(error, McpError::Timeout(method) if method == "initialize"));
    let _ = client.shutdown().await;
}

#[tokio::test]
async fn stdio_client_captures_stderr_when_server_exits() {
    let mut env = BTreeMap::new();
    env.insert("FAKE_MCP_EXIT_ON_START".to_string(), "1".to_string());
    let mut client = StdioMcpClient::spawn_configured(
        fake_stdio_server_binary(),
        &[],
        &env,
        None,
        Duration::from_secs(30),
    )
    .await
    .expect("spawn failing fake server");

    let error = client
        .initialize()
        .await
        .expect_err("server exits before initialize response");
    assert!(error.to_string().contains("fake startup failed"), "{error}");
    let _ = client.shutdown().await;
}

#[tokio::test]
async fn registry_lists_and_calls_configured_stdio_server_tools() {
    let (home, cwd) = temp_paths("registry-stdio");
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                    "args": [],
                    "env": {"FAKE_MCP_ENV": "registry-env"},
                    "cwd": cwd.display().to_string(),
                }
            }
        }),
    );

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .set_server_trust("fake", McpServerTrust::Trusted)
        .await
        .expect("trust fake server");
    let tools = registry.list_tools("fake").await.expect("list fake tools");

    let echo = tools
        .iter()
        .find(|tool| tool.name == "echo")
        .expect("echo tool from real tools/list");
    assert_eq!(echo.summary, "Echo test input.");
    assert!(
        tools.iter().all(|tool| tool.name != "inspect"),
        "configured stdio tools should come from real tools/list, not modeled stored tools"
    );

    let result = registry
        .invoke_tool("fake", "echo", r#"{"text":"registry"}"#)
        .await
        .expect("call fake echo tool");
    assert_eq!(result.output, "echo: registry");
    let context_result = registry
        .invoke_tool("fake", "context", "{}")
        .await
        .expect("call fake context tool");
    let expected_cwd = std::fs::canonicalize(&cwd).expect("canonical cwd");
    assert!(
        context_result.output.contains("env=registry-env"),
        "{}",
        context_result.output
    );
    assert!(
        context_result
            .output
            .contains(&format!("cwd={}", expected_cwd.display())),
        "{}",
        context_result.output
    );

    let fake = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "fake")
        .expect("fake server");
    assert_eq!(fake.status, McpServerStatus::Ready);
    assert_eq!(fake.error, None);
}

#[tokio::test]
async fn registry_marks_configured_stdio_server_stopped_before_first_probe() {
    let (home, cwd) = temp_paths("registry-stdio-initial-state");
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                }
            }
        }),
    );

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    let fake = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "fake")
        .expect("fake server");
    assert_eq!(fake.status, McpServerStatus::Stopped);
}

#[tokio::test]
async fn registry_reports_starting_while_stdio_initialize_is_in_flight() {
    let (home, cwd) = temp_paths("registry-stdio-starting-state");
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                    "env": {"FAKE_MCP_INIT_DELAY_MS": "250"},
                }
            }
        }),
    );

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .set_server_trust("fake", McpServerTrust::Trusted)
        .await
        .expect("trust fake server");
    let probe_registry = registry.clone();
    let probe = tokio::spawn(async move { probe_registry.list_tools("fake").await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let fake = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "fake")
        .expect("fake server");
    assert_eq!(fake.status, McpServerStatus::Starting);

    probe.await.expect("probe task").expect("list fake tools");
}

#[tokio::test]
async fn registry_reuses_stdio_connection_between_calls() {
    let (home, cwd) = temp_paths("registry-stdio-reuse");
    let start_log = cwd.join("starts.log");
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                    "env": {"FAKE_MCP_START_LOG": start_log.display().to_string()},
                }
            }
        }),
    );

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .set_server_trust("fake", McpServerTrust::Trusted)
        .await
        .expect("trust fake server");
    registry.list_tools("fake").await.expect("list fake tools");
    registry
        .invoke_tool("fake", "echo", r#"{"text":"reuse"}"#)
        .await
        .expect("call fake echo tool");

    assert_eq!(read_start_count(&start_log), 1);
    let fake = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "fake")
        .expect("fake server");
    assert_eq!(fake.status, McpServerStatus::Ready);
}

#[tokio::test]
async fn registry_gracefully_stops_cached_stdio_connection_when_untrusted() {
    let (home, cwd) = temp_paths("registry-stdio-graceful-stop");
    let exit_log = cwd.join("exits.log");
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                    "env": {"FAKE_MCP_EXIT_LOG": exit_log.display().to_string()},
                }
            }
        }),
    );

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .set_server_trust("fake", McpServerTrust::Trusted)
        .await
        .expect("trust fake server");
    registry.list_tools("fake").await.expect("list fake tools");
    registry
        .set_server_trust("fake", McpServerTrust::Unknown)
        .await
        .expect("untrust fake server");

    assert_eq!(read_start_count(&exit_log), 1);
    let fake = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "fake")
        .expect("fake server");
    assert_eq!(fake.status, McpServerStatus::Stopped);
}

#[tokio::test]
async fn registry_restarts_stdio_connection_after_process_exit() {
    let (home, cwd) = temp_paths("registry-stdio-restart");
    let start_log = cwd.join("starts.log");
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                    "env": {
                        "FAKE_MCP_START_LOG": start_log.display().to_string(),
                        "FAKE_MCP_EXIT_AFTER_LIST": "1"
                    },
                }
            }
        }),
    );

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .set_server_trust("fake", McpServerTrust::Trusted)
        .await
        .expect("trust fake server");
    registry.list_tools("fake").await.expect("list fake tools");
    let result = registry
        .invoke_tool("fake", "echo", r#"{"text":"restart"}"#)
        .await
        .expect("call fake echo after restart");

    assert_eq!(result.output, "echo: restart");
    assert_eq!(read_start_count(&start_log), 2);
    let fake = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "fake")
        .expect("fake server");
    assert_eq!(fake.status, McpServerStatus::Ready);
}

#[tokio::test]
async fn registry_reports_disabled_and_failed_stdio_server_state() {
    let (home, cwd) = temp_paths("registry-stdio-state");
    let missing_command = cwd.join("missing-fake-server");
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "disabled": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                    "disabled": true,
                },
                "broken": {
                    "type": "stdio",
                    "command": missing_command.display().to_string(),
                }
            }
        }),
    );

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    let disabled_error = registry
        .list_tools("disabled")
        .await
        .expect_err("disabled server should not list tools");
    assert!(matches!(disabled_error, McpError::DisabledServer(server) if server == "disabled"));
    registry
        .set_server_trust("broken", McpServerTrust::Trusted)
        .await
        .expect("trust broken server");
    let broken_error = registry
        .list_tools("broken")
        .await
        .expect_err("broken server should fail");
    assert!(
        broken_error.to_string().contains("io error"),
        "{broken_error}"
    );

    let servers = registry.list_servers().await;
    let disabled = servers
        .iter()
        .find(|server| server.id == "disabled")
        .expect("disabled server");
    assert_eq!(disabled.status, McpServerStatus::Disabled);
    assert_eq!(disabled.error, None);
    let broken = servers
        .iter()
        .find(|server| server.id == "broken")
        .expect("broken server");
    assert_eq!(broken.status, McpServerStatus::Failed);
    assert!(
        broken
            .error
            .as_deref()
            .is_some_and(|error| error.contains("io error")),
        "{broken:?}"
    );
}

#[tokio::test]
async fn list_provider_tools_exposes_trusted_stdio_tools_with_schema() {
    let (home, cwd) = temp_paths("provider-tools-discovery");
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                    "args": [],
                }
            }
        }),
    );

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    let untrusted = registry.list_provider_tools().await;
    assert!(
        untrusted.iter().all(|tool| tool.server_id != "fake"),
        "untrusted servers must not be exposed to the model: {untrusted:?}"
    );

    registry
        .set_server_trust("fake", McpServerTrust::Trusted)
        .await
        .expect("trust fake server");
    let trusted = registry.list_provider_tools().await;
    let echo = trusted
        .iter()
        .find(|tool| tool.tool_name == "echo")
        .expect("echo tool exposed");
    assert_eq!(echo.server_id, "fake");
    assert_eq!(echo.description, "Echo test input.");
    assert_eq!(echo.input_schema["type"], "object");
    assert_eq!(echo.input_schema["properties"]["text"]["type"], "string");
}

#[tokio::test]
async fn list_provider_tools_skips_disabled_and_marks_failed_servers() {
    let (home, cwd) = temp_paths("provider-tools-state");
    let missing = cwd.join("missing-fake-server");
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                    "disabled": true,
                },
                "broken": {
                    "type": "stdio",
                    "command": missing.display().to_string(),
                }
            }
        }),
    );

    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .set_server_trust("broken", McpServerTrust::Trusted)
        .await
        .expect("trust broken server");

    let tools = registry.list_provider_tools().await;
    assert!(
        tools.is_empty(),
        "disabled + broken servers should contribute no tools: {tools:?}"
    );
    let servers = registry.list_servers().await;
    let broken = servers
        .iter()
        .find(|server| server.id == "broken")
        .expect("broken server");
    assert_eq!(broken.status, McpServerStatus::Failed);
    assert!(
        broken
            .error
            .as_deref()
            .is_some_and(|message| message.contains("io error")),
        "{broken:?}"
    );
}

#[tokio::test]
async fn invoke_tool_returns_is_error_when_mcp_tool_signals_failure() {
    let (home, cwd) = temp_paths("invoke-tool-is-error");
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                }
            }
        }),
    );
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .set_server_trust("fake", McpServerTrust::Trusted)
        .await
        .expect("trust fake server");

    // `fail` returns a JSON-RPC error which surfaces as McpError::JsonRpc.
    let jsonrpc_error = registry
        .invoke_tool("fake", "fail", "{}")
        .await
        .expect_err("fail tool should bubble JSON-RPC error");
    assert!(
        matches!(jsonrpc_error, McpError::JsonRpc { code: -32000, .. }),
        "{jsonrpc_error}"
    );

    // `echo` returns success content with isError=false.
    let ok = registry
        .invoke_tool("fake", "echo", r#"{"text":"alpha"}"#)
        .await
        .expect("echo succeeds");
    assert_eq!(ok.output, "echo: alpha");
    assert!(!ok.is_error);
}

async fn discovery_registry(label: &str, env: Value) -> McpRegistry {
    let (home, cwd) = temp_paths(label);
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                    "env": env,
                }
            }
        }),
    );
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .set_server_trust("fake", McpServerTrust::Trusted)
        .await
        .expect("trust fake server");
    registry
}

#[tokio::test]
async fn registry_discovers_stdio_resources_with_annotations() {
    let registry = discovery_registry("stdio-discover-resources", json!({})).await;

    let resources = registry
        .discover_resources("fake")
        .await
        .expect("discover resources");
    assert_eq!(resources.len(), 2);

    let text = resources
        .iter()
        .find(|resource| resource.uri == "res://text")
        .expect("text resource");
    assert_eq!(text.name, "Text Resource");
    assert_eq!(text.mime_type, "text/plain");
    let annotations = text.annotations.as_ref().expect("text annotations");
    assert_eq!(annotations.audience, vec!["user", "assistant"]);
    assert_eq!(annotations.priority, Some(0.8));

    let binary = resources
        .iter()
        .find(|resource| resource.uri == "res://binary")
        .expect("binary resource");
    assert!(binary.annotations.is_none());

    let fake = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "fake")
        .expect("fake server");
    assert_eq!(fake.status, McpServerStatus::Ready);
}

#[tokio::test]
async fn registry_discovers_stdio_resource_templates() {
    let registry = discovery_registry("stdio-discover-templates", json!({})).await;

    let templates = registry
        .discover_resource_templates("fake")
        .await
        .expect("discover resource templates");
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0].uri_template, "res://items/{itemId}");
    assert_eq!(templates[0].mime_type, "application/json");
    assert_eq!(
        templates[0]
            .annotations
            .as_ref()
            .expect("priority")
            .priority,
        Some(0.5)
    );
}

#[tokio::test]
async fn registry_reads_stdio_text_and_binary_resource_content() {
    let registry = discovery_registry("stdio-read-content", json!({})).await;

    let text = registry
        .read_resource_content("fake", "res://text")
        .await
        .expect("read text resource");
    assert!(!text.is_binary);
    assert_eq!(text.contents, "hello text");
    assert!(text.blob.is_none());
    assert_eq!(
        text.annotations.as_ref().expect("annotations").audience,
        vec!["user"]
    );

    let binary = registry
        .read_resource_content("fake", "res://binary")
        .await
        .expect("read binary resource");
    assert!(binary.is_binary);
    assert!(binary.contents.is_empty(), "binary must not pollute text");
    assert_eq!(binary.blob.as_deref(), Some("aGVsbG8="));
}

#[tokio::test]
async fn registry_lists_stdio_prompts_and_gets_prompt_with_binary_message() {
    let registry = discovery_registry("stdio-prompts", json!({})).await;

    let prompts = registry.list_prompts("fake").await.expect("list prompts");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "greet");
    assert_eq!(prompts[0].arguments.len(), 1);
    assert_eq!(prompts[0].arguments[0].name, "name");
    assert!(prompts[0].arguments[0].required);

    let result = registry
        .get_prompt("fake", "greet", json!({ "name": "world" }))
        .await
        .expect("get prompt");
    assert_eq!(result.description, "A greeting.");
    assert_eq!(result.messages.len(), 2);

    let user = &result.messages[0];
    assert_eq!(user.role, "user");
    assert_eq!(user.content.kind, "text");
    assert_eq!(user.content.text.as_deref(), Some("Hello there"));
    assert!(!user.content.is_binary);

    let assistant = &result.messages[1];
    assert_eq!(assistant.role, "assistant");
    assert!(assistant.content.is_binary);
    assert_eq!(assistant.content.binary.as_deref(), Some("aW1hZ2U="));
    assert_eq!(assistant.content.mime_type, "image/png");
    assert!(assistant.content.text.is_none());
}

#[tokio::test]
async fn registry_handles_empty_stdio_discovery_lists() {
    let registry =
        discovery_registry("stdio-empty-lists", json!({ "FAKE_MCP_EMPTY_LISTS": "1" })).await;

    assert!(
        registry
            .discover_resources("fake")
            .await
            .expect("discover resources")
            .is_empty()
    );
    assert!(
        registry
            .discover_resource_templates("fake")
            .await
            .expect("discover templates")
            .is_empty()
    );
    assert!(
        registry
            .list_prompts("fake")
            .await
            .expect("list prompts")
            .is_empty()
    );
}

#[tokio::test]
async fn registry_reports_schema_mismatch_for_stdio_resources_list() {
    let registry = discovery_registry(
        "stdio-bad-resources",
        json!({ "FAKE_MCP_BAD_RESOURCES_LIST": "1" }),
    )
    .await;

    let error = registry
        .discover_resources("fake")
        .await
        .expect_err("malformed resources/list should fail");
    assert!(matches!(error, McpError::Json(_)), "{error}");

    let fake = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "fake")
        .expect("fake server");
    assert_eq!(fake.status, McpServerStatus::Failed);
}

#[tokio::test]
async fn registry_surfaces_stdio_resource_read_error() {
    let registry = discovery_registry("stdio-resource-error", json!({})).await;

    let error = registry
        .read_resource_content("fake", "res://missing")
        .await
        .expect_err("unknown resource should error");
    assert!(
        matches!(error, McpError::JsonRpc { code: -32002, .. }),
        "{error}"
    );

    let fake = registry
        .list_servers()
        .await
        .into_iter()
        .find(|server| server.id == "fake")
        .expect("fake server");
    assert_eq!(fake.status, McpServerStatus::Failed);
}

#[tokio::test]
async fn cancel_token_aborts_stdio_invoke_tool_hang() {
    let (home, cwd) = temp_paths("cancel-stdio-hang");
    write_mcp_json(
        &cwd,
        json!({
            "mcpServers": {
                "fake": {
                    "type": "stdio",
                    "command": fake_stdio_server_binary().display().to_string(),
                }
            }
        }),
    );
    let registry = McpRegistry::load(&home, &cwd).await.expect("load registry");
    registry
        .set_server_trust("fake", McpServerTrust::Trusted)
        .await
        .expect("trust fake server");
    registry.list_tools("fake").await.expect("list tools");

    let flag = Arc::new(AtomicBool::new(false));
    let token = McpCancellationToken::from_flag(flag.clone());

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        flag.store(true, Ordering::Relaxed);
    });

    let start = std::time::Instant::now();
    let result = registry
        .invoke_tool_cancellable("fake", "hang", "{}", Some(token))
        .await;
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(McpError::Cancelled)),
        "expected Cancelled, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "stdio hang tool should be cancelled within 1s, took {elapsed:?}"
    );
}

struct FakeStdioServerProcess {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
}

fn temp_paths(label: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("orbcode-mcp-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let cwd = root.join("cwd");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&cwd).expect("create cwd");
    (home, cwd)
}

fn write_mcp_json(cwd: &Path, value: Value) {
    std::fs::write(
        cwd.join(".mcp.json"),
        serde_json::to_string_pretty(&value).expect("serialize mcp json"),
    )
    .expect("write .mcp.json");
}

fn read_start_count(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .count()
}

impl FakeStdioServerProcess {
    async fn request(&mut self, request: Value) -> Value {
        let payload = serde_json::to_string(&request).expect("serialize fake MCP request");
        self.stdin
            .write_all(payload.as_bytes())
            .await
            .expect("write fake MCP request");
        self.stdin
            .write_all(b"\n")
            .await
            .expect("write fake MCP request newline");
        self.stdin.flush().await.expect("flush fake MCP request");

        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .await
            .expect("read fake MCP response");
        assert!(!line.trim().is_empty(), "fake MCP server closed stdout");
        serde_json::from_str(&line).expect("parse fake MCP response")
    }
}

impl Drop for FakeStdioServerProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn spawn_fake_stdio_server() -> FakeStdioServerProcess {
    let mut child = TokioCommand::new(fake_stdio_server_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fake MCP stdio server");

    let stdin = child.stdin.take().expect("fake MCP server stdin");
    let stdout = child.stdout.take().expect("fake MCP server stdout");

    FakeStdioServerProcess {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    }
}

fn fake_stdio_server_binary() -> &'static Path {
    static SERVER_BINARY: OnceLock<PathBuf> = OnceLock::new();
    SERVER_BINARY
        .get_or_init(compile_fake_stdio_server)
        .as_path()
}

fn compile_fake_stdio_server() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "orbcode-mcp-fake-stdio-server-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create fake MCP server temp dir");

    let source = dir.join("fake_stdio_server.rs");
    let binary = dir.join(if cfg!(windows) {
        "fake_stdio_server.exe"
    } else {
        "fake_stdio_server"
    });
    std::fs::write(&source, FAKE_STDIO_SERVER_SOURCE).expect("write fake MCP server source");

    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = StdCommand::new(rustc)
        .arg("--edition=2021")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("compile fake MCP stdio server");

    assert!(
        output.status.success(),
        "compile fake MCP stdio server\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    binary
}

const FAKE_STDIO_SERVER_SOURCE: &str = r##"
use std::io::{self, BufRead, Write};
use std::time::Duration;

fn main() {
    if std::env::var("FAKE_MCP_EXIT_ON_START").ok().as_deref() == Some("1") {
        eprintln!("fake startup failed");
        std::process::exit(42);
    }
    append_line_from_env("FAKE_MCP_START_LOG", "start");

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }

        let response = fake_stdio_response(&line);
        writeln!(stdout, "{response}").expect("write fake MCP response");
        stdout.flush().expect("flush fake MCP response");
        if std::env::var("FAKE_MCP_EXIT_AFTER_LIST").ok().as_deref() == Some("1")
            && line.contains(r#""method":"tools/list""#)
        {
            break;
        }
    }
    append_line_from_env("FAKE_MCP_EXIT_LOG", "exit");
}

fn append_line_from_env(env_var: &str, line: &str) {
    let Ok(path) = std::env::var(env_var) else {
        return;
    };
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{line}"));
}

fn fake_stdio_response(request: &str) -> String {
    let id = extract_id(request);

    if request.contains(r#""method":"initialize""#) {
        if let Ok(delay) = std::env::var("FAKE_MCP_INIT_DELAY_MS") {
            if let Ok(delay) = delay.parse::<u64>() {
                std::thread::sleep(Duration::from_millis(delay));
            }
        }
        return success_response(
            &id,
            r#"{"protocolVersion":"2024-11-05","capabilities":{"tools":{},"resources":{},"prompts":{}},"serverInfo":{"name":"orbcode-fake-stdio","version":"0.1.0"}}"#,
        );
    }

    if request.contains(r#""method":"resources/list""#) {
        if std::env::var("FAKE_MCP_BAD_RESOURCES_LIST").ok().as_deref() == Some("1") {
            return success_response(&id, r#"{"resources":"not-an-array"}"#);
        }
        if std::env::var("FAKE_MCP_EMPTY_LISTS").ok().as_deref() == Some("1") {
            return success_response(&id, r#"{"resources":[]}"#);
        }
        return success_response(
            &id,
            r#"{"resources":[{"uri":"res://text","name":"Text Resource","mimeType":"text/plain","description":"A text resource.","annotations":{"audience":["user","assistant"],"priority":0.8}},{"uri":"res://binary","name":"Binary Resource","mimeType":"application/octet-stream","description":"A binary resource."}]}"#,
        );
    }

    if request.contains(r#""method":"resources/templates/list""#) {
        if std::env::var("FAKE_MCP_EMPTY_LISTS").ok().as_deref() == Some("1") {
            return success_response(&id, r#"{"resourceTemplates":[]}"#);
        }
        return success_response(
            &id,
            r#"{"resourceTemplates":[{"uriTemplate":"res://items/{itemId}","name":"Item","mimeType":"application/json","description":"An item by id.","annotations":{"priority":0.5}}]}"#,
        );
    }

    if request.contains(r#""method":"resources/read""#) {
        if request.contains(r#""uri":"res://text""#) {
            return success_response(
                &id,
                r#"{"contents":[{"uri":"res://text","mimeType":"text/plain","text":"hello text","annotations":{"audience":["user"]}}]}"#,
            );
        }
        if request.contains(r#""uri":"res://binary""#) {
            return success_response(
                &id,
                r#"{"contents":[{"uri":"res://binary","mimeType":"application/octet-stream","blob":"aGVsbG8="}]}"#,
            );
        }
        return error_response(&id, -32002, "resource not found");
    }

    if request.contains(r#""method":"prompts/list""#) {
        if std::env::var("FAKE_MCP_BAD_PROMPTS_LIST").ok().as_deref() == Some("1") {
            return success_response(&id, r#"{"prompts":"not-an-array"}"#);
        }
        if std::env::var("FAKE_MCP_EMPTY_LISTS").ok().as_deref() == Some("1") {
            return success_response(&id, r#"{"prompts":[]}"#);
        }
        return success_response(
            &id,
            r#"{"prompts":[{"name":"greet","description":"Greet someone.","arguments":[{"name":"name","description":"Who to greet.","required":true}]}]}"#,
        );
    }

    if request.contains(r#""method":"prompts/get""#) {
        if request.contains(r#""name":"greet""#) {
            return success_response(
                &id,
                r#"{"description":"A greeting.","messages":[{"role":"user","content":{"type":"text","text":"Hello there"}},{"role":"assistant","content":{"type":"image","data":"aW1hZ2U=","mimeType":"image/png"}}]}"#,
            );
        }
        return error_response(&id, -32602, "unknown prompt");
    }

    if request.contains(r#""method":"tools/list""#) {
        if std::env::var("FAKE_MCP_BAD_TOOLS_LIST").ok().as_deref() == Some("1") {
            return success_response(&id, r#"{"tools":"not-an-array"}"#);
        }
        return success_response(
            &id,
            r#"{"tools":[{"name":"echo","description":"Echo test input.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}},{"name":"context","description":"Return process env and cwd.","inputSchema":{"type":"object"}},{"name":"fail","description":"Return a deterministic fake failure.","inputSchema":{"type":"object"}},{"name":"hang","description":"Never responds (sleeps forever).","inputSchema":{"type":"object"}}]}"#,
        );
    }

    if request.contains(r#""method":"tools/call""#) && request.contains(r#""name":"echo""#) {
        let text = extract_text_argument(request);
        let echoed = escape_json_string(&format!("echo: {text}"));
        return success_response(
            &id,
            &format!(r#"{{"content":[{{"type":"text","text":"{echoed}"}}],"isError":false}}"#),
        );
    }

    if request.contains(r#""method":"tools/call""#) && request.contains(r#""name":"hang""#) {
        std::thread::sleep(Duration::from_secs(3600));
        return error_response(&id, -32000, "hang tool woke up unexpectedly");
    }

    if request.contains(r#""method":"tools/call""#) && request.contains(r#""name":"fail""#) {
        return error_response(&id, -32000, "fake tool failed");
    }

    if request.contains(r#""method":"tools/call""#) && request.contains(r#""name":"context""#) {
        let env = std::env::var("FAKE_MCP_ENV").unwrap_or_default();
        let cwd = std::env::current_dir()
            .map(|cwd| cwd.display().to_string())
            .unwrap_or_default();
        let text = escape_json_string(&format!("env={env}\ncwd={cwd}"));
        return success_response(
            &id,
            &format!(r#"{{"content":[{{"type":"text","text":"{text}"}}],"isError":false}}"#),
        );
    }

    if request.contains(r#""method":"tools/call""#) {
        return error_response(&id, -32602, "unknown fake tool");
    }

    error_response(&id, -32601, "unknown fake method")
}

fn extract_id(request: &str) -> String {
    let Some(index) = request.find(r#""id":"#) else {
        return "null".to_string();
    };
    let rest = request[index + r#""id":"#.len()..].trim_start();

    if rest.starts_with("null") {
        return "null".to_string();
    }

    if rest.starts_with('"') {
        return read_json_string_literal(rest).unwrap_or_else(|| "null".to_string());
    }

    let id: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
        .collect();
    if id.is_empty() { "null".to_string() } else { id }
}

fn extract_text_argument(request: &str) -> String {
    let pattern = "\"text\":\"";
    let Some(index) = request.find(pattern) else {
        return String::new();
    };
    let rest = &request[index + pattern.len()..];
    read_json_string_value(rest).unwrap_or_default()
}

fn read_json_string_literal(input: &str) -> Option<String> {
    let mut escaped = false;
    for (index, ch) in input.char_indices().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            return Some(input[..=index].to_string());
        }
    }
    None
}

fn read_json_string_value(input: &str) -> Option<String> {
    let mut value = String::new();
    let mut chars = input.chars();

    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                '"' => value.push('"'),
                '\\' => value.push('\\'),
                '/' => value.push('/'),
                'b' => value.push('\u{0008}'),
                'f' => value.push('\u{000c}'),
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                other => value.push(other),
            },
            other => value.push(other),
        }
    }

    None
}

fn success_response(id: &str, result: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#)
}

fn error_response(id: &str, code: i64, message: &str) -> String {
    let message = escape_json_string(message);
    format!(r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":{code},"message":"{message}"}}}}"#)
}

fn escape_json_string(input: &str) -> String {
    let mut output = String::new();
    for ch in input.chars() {
        match ch {
            '"' => output.push_str(r#"\""#),
            '\\' => output.push_str(r#"\\"#),
            '\n' => output.push_str(r#"\n"#),
            '\r' => output.push_str(r#"\r"#),
            '\t' => output.push_str(r#"\t"#),
            other => output.push(other),
        }
    }
    output
}
"##;
