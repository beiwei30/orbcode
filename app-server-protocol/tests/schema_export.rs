//! Schema export tests — generate JSON Schema from Rust DTOs and compare
//! against checked-in golden files. Any drift fails CI.
//!
//! Run with `UPDATE_SCHEMA=1 cargo test` to regenerate the golden files
//! after intentional changes.

use schemars::generate::SchemaSettings;
use serde_json::Value;
use std::collections::BTreeMap;

const SCHEMA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/generated/schema");

fn generate_schema<T: schemars::JsonSchema>() -> Value {
    let settings = SchemaSettings::draft2020_12();
    let generator = settings.into_generator();
    let schema = generator.into_root_schema_for::<T>();
    serde_json::to_value(schema).expect("schema to JSON")
}

fn assert_schema_matches<T: schemars::JsonSchema>(name: &str) {
    let schema = generate_schema::<T>();
    let pretty = serde_json::to_string_pretty(&schema).expect("pretty JSON");
    let path = format!("{SCHEMA_DIR}/{name}.json");

    if std::env::var("UPDATE_SCHEMA").is_ok() {
        std::fs::write(&path, format!("{pretty}\n")).expect("write schema");
        return;
    }

    let golden = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!("golden schema not found at {path}. Run with UPDATE_SCHEMA=1 to generate.")
    });
    let golden_value: Value = serde_json::from_str(&golden)
        .unwrap_or_else(|e| panic!("invalid JSON in golden schema {path}: {e}"));

    assert_eq!(
        schema, golden_value,
        "schema drift detected for {name}. Run with UPDATE_SCHEMA=1 to update."
    );
}

fn assert_method_contract_schema_bundle() {
    let mut contracts = BTreeMap::new();
    macro_rules! insert {
        ($($name:ident),+ $(,)?) => {
            $(
                contracts.insert(
                    stringify!($name).to_string(),
                    generate_schema::<orbcode_app_server_protocol::$name>(),
                );
            )+
        };
    }

    insert!(
        EmptyParams,
        NoData,
        SessionIdParams,
        SetSessionPermissionModeParams,
        SetSessionModelParams,
        SetSessionEffortParams,
        SessionControlState,
        SessionRenameParams,
        SessionForkParams,
        SessionRewindParams,
        SessionRecordMessageParams,
        SessionFindByTitleParams,
        SessionFindByTitleResult,
        TurnSubmitParams,
        TurnSubmitResult,
        TurnCancelResult,
        TurnInterruptResult,
        SentResult,
        PermissionModeResult,
        PermissionSetModeParams,
        PermissionRuleParams,
        SessionPermissionRuleParams,
        PermissionRuleUpdateResult,
        AddDirectoryParams,
        ValidateDirectoryParams,
        ModelNameResult,
        ModelOptionsResult,
        SetModelParams,
        SetModelResult,
        SetThinkingBudgetParams,
        ThinkingBudgetResult,
        ProvidersResult,
        ThemeParams,
        ThemeResult,
        EffortParams,
        EffortResult,
        OutputStyleParams,
        OutputStyleResult,
        PathResult,
        KeybindingsFileResult,
        KeybindingsLoadResult,
        EditorModeParams,
        EditorModeResult,
        OutputStyleOptionsResult,
        ActiveOutputStyleResult,
        SettingKeyParams,
        SettingLockedResult,
        EnabledParams,
        EnabledResult,
        StringPathParams,
        SandboxExcludedCommandParams,
        SandboxExcludedCommandResult,
        AllowAllParams,
        AllowAllResult,
        ToolsListResult,
        SeedReadStateParams,
        SeedReadStateResult,
        ToolInvokeParams,
        ToolInvokeResult,
        SkillDefinitionsParams,
        SkillDefinitionsResult,
        AgentDefinitionsResult,
        AgentDefinitionsWithWarningsResult,
        TaskListParams,
        TaskListResult,
        EnterPlanModeResult,
        AuthLoginParams,
        AuthLoginResult,
        AuthLogoutParams,
        AuthLogoutResult,
        DiagnosticsCleanupChildSessionsParams,
        ChildSessionOrphanCleanupResult,
        AdvancedCapabilitiesResult,
        LastProviderRequestResult,
        PreUserInstructionsResult,
        BackgroundCreateParams,
        BackgroundCreateResult,
        BackgroundJobParams,
        BackgroundCancelResult,
        CancelAsyncTaskParams,
        CancelAsyncTaskResult,
        BackgroundLogResult,
        BackgroundEventsResult,
        BackgroundSubscribeParams,
        BackgroundSubscribeResult,
        WorkflowStartParams,
        WorkflowStartDynamicParams,
        WorkflowResumeParams,
        WorkflowTaskResult,
        McpServerIdParams,
        McpServerStatusOverview,
        McpStatusResult,
        McpSessionServerParams,
        McpServerTrustResult,
        McpSetTrustParams,
        McpListToolsResult,
        McpListResourcesResult,
        McpReadResourceParams,
        McpResourceContent,
        McpListPromptsResult,
        McpGetPromptParams,
        McpPromptResult,
        McpInvokeToolParams,
        McpToolResult,
        McpDiagnoseResult,
        McpRemoveServerResult,
        McpCapabilitiesResult,
        McpOAuthOverviewParams,
        McpOAuthOverview,
        McpLogoutOAuthTokenResult,
        AuthOverview,
        PermissionContext,
        SandboxLocalSettings,
        SandboxSettingsUpdate,
        HookDiscovery,
    );

    let schema = serde_json::to_value(contracts).expect("contract schemas to JSON");
    let pretty = serde_json::to_string_pretty(&schema).expect("pretty JSON");
    let path = format!("{SCHEMA_DIR}/MethodContracts.json");
    if std::env::var("UPDATE_SCHEMA").is_ok() {
        std::fs::write(&path, format!("{pretty}\n")).expect("write schema bundle");
        return;
    }
    let golden = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!("golden schema not found at {path}. Run with UPDATE_SCHEMA=1 to generate.")
    });
    let golden_value: Value = serde_json::from_str(&golden)
        .unwrap_or_else(|e| panic!("invalid JSON in golden schema {path}: {e}"));
    assert_eq!(
        schema, golden_value,
        "method contract schema bundle drifted"
    );
}

#[test]
fn schema_method_contracts() {
    assert_method_contract_schema_bundle();
}

// ---------------------------------------------------------------------------
// Envelope types
// ---------------------------------------------------------------------------

#[test]
fn schema_client_message() {
    assert_schema_matches::<orbcode_app_server_protocol::ClientMessage>("ClientMessage");
}

#[test]
fn schema_server_message() {
    assert_schema_matches::<orbcode_app_server_protocol::ServerMessage>("ServerMessage");
}

#[test]
fn schema_client_request_envelope() {
    assert_schema_matches::<orbcode_app_server_protocol::ClientRequestEnvelope>(
        "ClientRequestEnvelope",
    );
}

#[test]
fn schema_server_response_envelope() {
    assert_schema_matches::<orbcode_app_server_protocol::ServerResponseEnvelope>(
        "ServerResponseEnvelope",
    );
}

#[test]
fn schema_response_result() {
    assert_schema_matches::<orbcode_app_server_protocol::ResponseResult>("ResponseResult");
}

#[test]
fn schema_server_notification_envelope() {
    assert_schema_matches::<orbcode_app_server_protocol::ServerNotificationEnvelope>(
        "ServerNotificationEnvelope",
    );
}

#[test]
fn schema_server_request_envelope() {
    assert_schema_matches::<orbcode_app_server_protocol::ServerRequestEnvelope>(
        "ServerRequestEnvelope",
    );
}

// ---------------------------------------------------------------------------
// Initialize types
// ---------------------------------------------------------------------------

#[test]
fn schema_initialize_params() {
    assert_schema_matches::<orbcode_app_server_protocol::InitializeParams>("InitializeParams");
}

#[test]
fn schema_initialize_result() {
    assert_schema_matches::<orbcode_app_server_protocol::InitializeResult>("InitializeResult");
}

#[test]
fn schema_client_capabilities() {
    assert_schema_matches::<orbcode_app_server_protocol::ClientCapabilities>("ClientCapabilities");
}

#[test]
fn schema_server_capabilities() {
    assert_schema_matches::<orbcode_app_server_protocol::ServerCapabilities>("ServerCapabilities");
}

// ---------------------------------------------------------------------------
// Request parameter types
// ---------------------------------------------------------------------------

#[test]
fn schema_bootstrap_params() {
    assert_schema_matches::<orbcode_app_server_protocol::BootstrapParams>("BootstrapParams");
}

#[test]
fn schema_mcp_server_input() {
    assert_schema_matches::<orbcode_app_server_protocol::McpServerInput>("McpServerInput");
}

#[test]
fn schema_mcp_server_overview() {
    assert_schema_matches::<orbcode_app_server_protocol::McpServerOverview>("McpServerOverview");
}

#[test]
fn schema_mcp_list_servers_result() {
    assert_schema_matches::<orbcode_app_server_protocol::McpListServersResult>(
        "McpListServersResult",
    );
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[test]
fn schema_protocol_error() {
    assert_schema_matches::<orbcode_app_server_protocol::ProtocolError>("ProtocolError");
}

#[test]
fn schema_error_code() {
    assert_schema_matches::<orbcode_app_server_protocol::ErrorCode>("ErrorCode");
}

// ---------------------------------------------------------------------------
// Server-request types
// ---------------------------------------------------------------------------

#[test]
fn schema_permission_decision_wire() {
    assert_schema_matches::<orbcode_app_server_protocol::PermissionDecisionWire>(
        "PermissionDecisionWire",
    );
}

#[test]
fn schema_permission_response_params() {
    assert_schema_matches::<orbcode_app_server_protocol::PermissionResponseParams>(
        "PermissionResponseParams",
    );
}

#[test]
fn schema_mcp_trust_decision_wire() {
    assert_schema_matches::<orbcode_app_server_protocol::McpTrustDecisionWire>(
        "McpTrustDecisionWire",
    );
}

#[test]
fn schema_mcp_trust_response_params() {
    assert_schema_matches::<orbcode_app_server_protocol::McpTrustResponseParams>(
        "McpTrustResponseParams",
    );
}

#[test]
fn schema_ask_user_question_request() {
    assert_schema_matches::<orbcode_app_server_protocol::AskUserQuestionRequest>(
        "AskUserQuestionRequest",
    );
}

#[test]
fn schema_ask_user_question_response() {
    assert_schema_matches::<orbcode_app_server_protocol::AskUserQuestionResponse>(
        "AskUserQuestionResponse",
    );
}

#[test]
fn schema_ask_user_question_spec() {
    assert_schema_matches::<orbcode_app_server_protocol::AskUserQuestionSpec>(
        "AskUserQuestionSpec",
    );
}

#[test]
fn schema_ask_user_response_outcome() {
    assert_schema_matches::<orbcode_app_server_protocol::AskUserResponseOutcome>(
        "AskUserResponseOutcome",
    );
}

#[test]
fn schema_interactive_questions_capability() {
    assert_schema_matches::<orbcode_app_server_protocol::InteractiveQuestionsCapability>(
        "InteractiveQuestionsCapability",
    );
}
