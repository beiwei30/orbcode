//! TypeScript type export — generates a `protocol.ts` from Rust DTOs.
//!
//! Run with `UPDATE_SCHEMA=1 cargo test` to regenerate after intentional changes.
//! Without the env var, the test asserts the generated output matches the
//! checked-in file (drift detection).

use schemars::generate::SchemaSettings;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

const TS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/generated/typescript");

fn schema_for<T: schemars::JsonSchema>() -> Value {
    let settings = SchemaSettings::draft2020_12();
    let generator = settings.into_generator();
    serde_json::to_value(generator.into_root_schema_for::<T>()).unwrap()
}

fn is_pure_object(schema: &Value) -> bool {
    schema.get("type").and_then(|v| v.as_str()) == Some("object")
        && schema.get("oneOf").is_none()
        && schema.get("anyOf").is_none()
        && schema.get("$ref").is_none()
}

fn primitive_type(t: &str) -> &str {
    match t {
        "string" => "string",
        "number" | "integer" => "number",
        "boolean" => "boolean",
        "null" => "null",
        _ => "unknown",
    }
}

fn json_schema_to_ts(schema: &Value, depth: usize) -> String {
    // const literal
    if let Some(c) = schema.get("const") {
        return match c {
            Value::String(s) => format!("\"{s}\""),
            other => other.to_string(),
        };
    }

    // $ref — just emit the type name. If there are also properties (tagged
    // union variant with $ref + discriminant), merge them with intersection.
    if let Some(r) = schema.get("$ref").and_then(|v| v.as_str()) {
        let name = r.rsplit('/').next().unwrap_or(r);
        if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
            let required: Vec<&str> = schema
                .get("required")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let indent = "  ".repeat(depth + 1);
            let close_indent = "  ".repeat(depth);
            let fields: Vec<String> = props
                .iter()
                .map(|(k, v)| {
                    let opt = if required.contains(&k.as_str()) {
                        ""
                    } else {
                        "?"
                    };
                    let ty = json_schema_to_ts(v, depth + 1);
                    format!("{indent}{k}{opt}: {ty};")
                })
                .collect();
            return format!("{name} & {{\n{}\n{close_indent}}}", fields.join("\n"));
        }
        return name.to_string();
    }

    // oneOf / anyOf — union
    if let Some(one_of) = schema.get("oneOf").and_then(|v| v.as_array()) {
        let variants: Vec<String> = one_of.iter().map(|v| json_schema_to_ts(v, depth)).collect();
        return variants.join(" | ");
    }
    if let Some(any_of) = schema.get("anyOf").and_then(|v| v.as_array()) {
        let variants: Vec<String> = any_of.iter().map(|v| json_schema_to_ts(v, depth)).collect();
        return variants.join(" | ");
    }

    // enum — literal union
    if let Some(enums) = schema.get("enum").and_then(|v| v.as_array()) {
        let variants: Vec<String> = enums
            .iter()
            .map(|v| match v {
                Value::String(s) => format!("\"{s}\""),
                other => other.to_string(),
            })
            .collect();
        return variants.join(" | ");
    }

    // type field — can be a string or an array (multi-type)
    let type_val = schema.get("type");

    // type: ["string", "null"] → string | null
    if let Some(arr) = type_val.and_then(|v| v.as_array()) {
        let parts: Vec<&str> = arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(primitive_type)
            .collect();
        return parts.join(" | ");
    }

    // true schema (any value)
    if schema.as_bool() == Some(true) {
        return "unknown".to_string();
    }

    match type_val.and_then(|v| v.as_str()) {
        Some("string") => "string".to_string(),
        Some("number" | "integer") => "number".to_string(),
        Some("boolean") => "boolean".to_string(),
        Some("null") => "null".to_string(),
        Some("array") => {
            let items = schema
                .get("items")
                .map_or_else(|| "unknown".to_string(), |i| json_schema_to_ts(i, depth));
            format!("{items}[]")
        }
        Some("object") => {
            let props = schema.get("properties").and_then(|v| v.as_object());
            let required: Vec<&str> = schema
                .get("required")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            if let Some(props) = props {
                let indent = "  ".repeat(depth + 1);
                let close_indent = "  ".repeat(depth);
                let fields: Vec<String> = props
                    .iter()
                    .map(|(k, v)| {
                        let opt = if required.contains(&k.as_str()) {
                            ""
                        } else {
                            "?"
                        };
                        let ty = json_schema_to_ts(v, depth + 1);
                        format!("{indent}{k}{opt}: {ty};")
                    })
                    .collect();
                format!("{{\n{}\n{close_indent}}}", fields.join("\n"))
            } else {
                "Record<string, unknown>".to_string()
            }
        }
        _ => "unknown".to_string(),
    }
}

fn emit_named_type(name: &str, schema: &Value) -> String {
    let mut out = String::new();
    if let Some(desc) = schema.get("description").and_then(|v| v.as_str()) {
        writeln!(out, "/** {desc} */").expect("writing to String cannot fail");
    }
    let body = json_schema_to_ts(schema, 0);
    if is_pure_object(schema) && body.starts_with('{') && !body.contains('|') {
        writeln!(out, "export interface {name} {body}").expect("writing to String cannot fail");
    } else {
        writeln!(out, "export type {name} = {body};").expect("writing to String cannot fail");
    }
    out
}

fn generate_protocol_ts() -> String {
    let mut output = String::from(
        "// Auto-generated from Rust protocol DTOs — do not edit manually.\n\
         // Regenerate with: UPDATE_SCHEMA=1 cargo test -p orbcode-app-server-protocol\n\n",
    );

    let mut types: Vec<(&str, Value)> = vec![
        (
            "ClientMessage",
            schema_for::<orbcode_app_server_protocol::ClientMessage>(),
        ),
        (
            "ClientRequestEnvelope",
            schema_for::<orbcode_app_server_protocol::ClientRequestEnvelope>(),
        ),
        (
            "ServerMessage",
            schema_for::<orbcode_app_server_protocol::ServerMessage>(),
        ),
        (
            "ServerResponseEnvelope",
            schema_for::<orbcode_app_server_protocol::ServerResponseEnvelope>(),
        ),
        (
            "ResponseResult",
            schema_for::<orbcode_app_server_protocol::ResponseResult>(),
        ),
        (
            "ServerNotificationEnvelope",
            schema_for::<orbcode_app_server_protocol::ServerNotificationEnvelope>(),
        ),
        (
            "ServerRequestEnvelope",
            schema_for::<orbcode_app_server_protocol::ServerRequestEnvelope>(),
        ),
        (
            "InitializeParams",
            schema_for::<orbcode_app_server_protocol::InitializeParams>(),
        ),
        (
            "InitializeResult",
            schema_for::<orbcode_app_server_protocol::InitializeResult>(),
        ),
        (
            "ClientCapabilities",
            schema_for::<orbcode_app_server_protocol::ClientCapabilities>(),
        ),
        (
            "ServerCapabilities",
            schema_for::<orbcode_app_server_protocol::ServerCapabilities>(),
        ),
        (
            "BootstrapParams",
            schema_for::<orbcode_app_server_protocol::BootstrapParams>(),
        ),
        (
            "BootstrapState",
            schema_for::<orbcode_app_server_protocol::BootstrapState>(),
        ),
        (
            "SessionListResult",
            schema_for::<orbcode_app_server_protocol::SessionListResult>(),
        ),
        (
            "SessionForkResult",
            schema_for::<orbcode_app_server_protocol::SessionForkResult>(),
        ),
        (
            "SessionControlState",
            schema_for::<orbcode_app_server_protocol::SessionControlState>(),
        ),
        (
            "SetSessionPermissionModeParams",
            schema_for::<orbcode_app_server_protocol::SetSessionPermissionModeParams>(),
        ),
        (
            "SetSessionModelParams",
            schema_for::<orbcode_app_server_protocol::SetSessionModelParams>(),
        ),
        (
            "SetSessionEffortParams",
            schema_for::<orbcode_app_server_protocol::SetSessionEffortParams>(),
        ),
        (
            "AcpDeleteSessionParams",
            schema_for::<orbcode_app_server_protocol::AcpDeleteSessionParams>(),
        ),
        (
            "StreamEvent",
            schema_for::<orbcode_app_server_protocol::StreamEvent>(),
        ),
        (
            "StreamEventNotification",
            schema_for::<orbcode_app_server_protocol::StreamEventNotification>(),
        ),
        (
            "McpServerInput",
            schema_for::<orbcode_app_server_protocol::McpServerInput>(),
        ),
        (
            "McpServerOverview",
            schema_for::<orbcode_app_server_protocol::McpServerOverview>(),
        ),
        (
            "McpListServersResult",
            schema_for::<orbcode_app_server_protocol::McpListServersResult>(),
        ),
        (
            "ProtocolError",
            schema_for::<orbcode_app_server_protocol::ProtocolError>(),
        ),
        (
            "ErrorCode",
            schema_for::<orbcode_app_server_protocol::ErrorCode>(),
        ),
        (
            "PermissionDecisionWire",
            schema_for::<orbcode_app_server_protocol::PermissionDecisionWire>(),
        ),
        (
            "PermissionResponseParams",
            schema_for::<orbcode_app_server_protocol::PermissionResponseParams>(),
        ),
        (
            "McpTrustDecisionWire",
            schema_for::<orbcode_app_server_protocol::McpTrustDecisionWire>(),
        ),
        (
            "McpTrustResponseParams",
            schema_for::<orbcode_app_server_protocol::McpTrustResponseParams>(),
        ),
        (
            "AskUserQuestionRequest",
            schema_for::<orbcode_app_server_protocol::AskUserQuestionRequest>(),
        ),
        (
            "AskUserQuestionResponse",
            schema_for::<orbcode_app_server_protocol::AskUserQuestionResponse>(),
        ),
        (
            "AskUserQuestionSpec",
            schema_for::<orbcode_app_server_protocol::AskUserQuestionSpec>(),
        ),
        (
            "AskUserResponseOutcome",
            schema_for::<orbcode_app_server_protocol::AskUserResponseOutcome>(),
        ),
        (
            "InteractiveQuestionsCapability",
            schema_for::<orbcode_app_server_protocol::InteractiveQuestionsCapability>(),
        ),
    ];

    macro_rules! extend_types {
        ($($name:ident),+ $(,)?) => {
            $(types.push((
                stringify!($name),
                schema_for::<orbcode_app_server_protocol::$name>(),
            ));)+
        };
    }
    extend_types!(
        EmptyParams,
        NoData,
        SessionIdParams,
        SessionGoal,
        SessionGoalStatus,
        SessionGoalGetResult,
        SessionGoalSetParams,
        SessionGoalSetResult,
        SessionGoalClearResult,
        SessionGoalContinueParams,
        SessionGoalNotStartedReason,
        SessionGoalContinueResult,
        SessionModelOption,
        PermissionPresetOption,
        PermissionPresetsResult,
        SetSessionPermissionPresetParams,
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
        PermissionOverview,
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

    let mut all_defs: BTreeMap<String, Value> = BTreeMap::new();
    let mut top_level_names: HashSet<String> = HashSet::new();

    for (name, schema) in &types {
        top_level_names.insert(name.to_string());
        if let Some(defs) = schema.get("$defs").and_then(|v| v.as_object()) {
            for (def_name, def_schema) in defs {
                all_defs
                    .entry(def_name.clone())
                    .or_insert(def_schema.clone());
            }
        }
    }

    for (name, schema) in &types {
        output.push_str(&emit_named_type(name, schema));
        output.push('\n');
    }

    for (def_name, def_schema) in &all_defs {
        if !top_level_names.contains(def_name) {
            output.push_str(&emit_named_type(def_name, def_schema));
            output.push('\n');
        }
    }

    output.push_str("// Method constants\n");
    output.push_str("export const STABLE_METHODS = [\n");
    for m in orbcode_app_server_protocol::method::stable_client_request_methods() {
        writeln!(output, "  \"{m}\",").expect("writing to String cannot fail");
    }
    output.push_str("] as const;\n\n");

    output.push_str("export const EXPERIMENTAL_METHODS = [\n");
    for m in orbcode_app_server_protocol::method::experimental_client_request_methods() {
        writeln!(output, "  \"{m}\",").expect("writing to String cannot fail");
    }
    output.push_str("] as const;\n\n");

    output.push_str("export const SERVER_NOTIFICATION_METHODS = [\n");
    for m in orbcode_app_server_protocol::method::server_notification_methods() {
        writeln!(output, "  \"{m}\",").expect("writing to String cannot fail");
    }
    output.push_str("] as const;\n\n");

    output.push_str("export const SERVER_REQUEST_METHODS = [\n");
    for m in orbcode_app_server_protocol::method::server_request_methods() {
        writeln!(output, "  \"{m}\",").expect("writing to String cannot fail");
    }
    output.push_str("] as const;\n");

    output
}

#[test]
fn typescript_protocol_ts() {
    let generated = generate_protocol_ts();
    let path = format!("{TS_DIR}/protocol.ts");

    if std::env::var("UPDATE_SCHEMA").is_ok() {
        std::fs::write(&path, &generated).expect("write protocol.ts");
        // Also write to the client's stable import location.
        let client_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("clients/typescript/src/generated");
        let _ = std::fs::create_dir_all(&client_dir);
        std::fs::write(client_dir.join("protocol.ts"), &generated)
            .expect("write client protocol.ts");
        return;
    }

    let golden = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!("golden TypeScript types not found at {path}. Run with UPDATE_SCHEMA=1 to generate.")
    });

    assert_eq!(
        generated, golden,
        "TypeScript type drift detected. Run with UPDATE_SCHEMA=1 to update."
    );

    let client_copy_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("clients/typescript/src/generated/protocol.ts");
    if let Ok(client_copy) = std::fs::read_to_string(&client_copy_path) {
        assert_eq!(
            generated, client_copy,
            "client protocol.ts copy drifted from canonical. Run with UPDATE_SCHEMA=1 to sync."
        );
    } else {
        panic!(
            "client protocol.ts not found at {}. Run with UPDATE_SCHEMA=1 to generate.",
            client_copy_path.display()
        );
    }
}

#[test]
fn empty_object_schema_emits_type_alias() {
    let schema = serde_json::json!({ "type": "object" });

    assert_eq!(
        emit_named_type("EmptyParams", &schema),
        "export type EmptyParams = Record<string, unknown>;\n"
    );
}
