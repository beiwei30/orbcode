use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn fixtures() -> Value {
    serde_json::from_str(include_str!("fixtures/method_family_contracts.json"))
        .expect("method family fixture must be valid JSON")
}

fn assert_round_trip<T>(fixtures: &Value, family: &str)
where
    T: DeserializeOwned + Serialize,
{
    let expected = fixtures
        .get(family)
        .unwrap_or_else(|| panic!("missing {family} fixture"));
    let typed: T = serde_json::from_value(expected.clone())
        .unwrap_or_else(|error| panic!("invalid {family} fixture: {error}"));
    assert_eq!(
        serde_json::to_value(typed).expect("serialize fixture"),
        *expected,
        "{family} wire shape drifted"
    );
}

#[test]
fn representative_method_families_round_trip_exactly() {
    let fixture = fixtures();
    assert_round_trip::<orbcode_app_server_protocol::NoData>(&fixture, "no_data");
    assert_round_trip::<orbcode_app_server_protocol::SessionRenameParams>(&fixture, "session");
    assert_round_trip::<orbcode_app_server_protocol::TurnSubmitResult>(&fixture, "turn");
    assert_round_trip::<orbcode_app_server_protocol::PermissionRuleUpdateResult>(
        &fixture,
        "permission",
    );
    assert_round_trip::<orbcode_app_server_protocol::SandboxSettingsUpdate>(&fixture, "settings");
    assert_round_trip::<orbcode_app_server_protocol::McpGetPromptParams>(&fixture, "mcp_prompt");
    assert_round_trip::<orbcode_app_server_protocol::McpToolResult>(&fixture, "mcp_tool");
    assert_round_trip::<orbcode_app_server_protocol::ToolInvokeResult>(&fixture, "tools");
    assert_round_trip::<orbcode_app_server_protocol::AuthLoginParams>(&fixture, "auth");
    assert_round_trip::<orbcode_app_server_protocol::DiagnosticsCleanupChildSessionsParams>(
        &fixture,
        "diagnostics",
    );
    assert_round_trip::<orbcode_app_server_protocol::BackgroundCreateResult>(
        &fixture,
        "background",
    );
    assert_round_trip::<orbcode_app_server_protocol::WorkflowStartDynamicParams>(
        &fixture, "workflow",
    );
}
