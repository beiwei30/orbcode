use std::collections::{BTreeMap, BTreeSet};

use orbcode_app_server_protocol::method;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct InventoryEntry {
    method: String,
    stability: String,
    params: String,
    result: String,
    consumers: Vec<String>,
    opaque_fields: Vec<String>,
    sensitive_fields: Vec<String>,
}

fn inventory() -> Vec<InventoryEntry> {
    serde_json::from_str(include_str!("fixtures/method_contract_inventory.json"))
        .expect("method contract inventory must be valid JSON")
}

#[test]
fn inventory_exactly_covers_advertised_client_methods() {
    let entries = inventory();
    let actual = entries
        .iter()
        .map(|entry| entry.method.as_str())
        .collect::<BTreeSet<_>>();
    let expected = method::client_request_methods()
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "inventory method coverage drifted");
    assert_eq!(entries.len(), actual.len(), "inventory contains duplicates");

    let stable = entries
        .iter()
        .filter(|entry| entry.stability == "stable")
        .map(|entry| entry.method.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        stable,
        method::stable_client_request_methods()
            .into_iter()
            .collect(),
        "stable inventory classification drifted"
    );

    let experimental = entries
        .iter()
        .filter(|entry| entry.stability == "experimental")
        .map(|entry| entry.method.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        experimental,
        method::experimental_client_request_methods()
            .into_iter()
            .collect(),
        "experimental inventory classification drifted"
    );
}

#[test]
fn every_method_has_named_contracts_consumers_and_explicit_field_classification() {
    for entry in inventory() {
        assert!(
            !entry.params.is_empty(),
            "{} has no params type",
            entry.method
        );
        assert!(
            !entry.result.is_empty(),
            "{} has no result type",
            entry.method
        );
        assert!(
            !entry.consumers.is_empty(),
            "{} has no documented consumer",
            entry.method
        );
        assert_ne!(
            entry.params, "Value",
            "{} has raw Value params",
            entry.method
        );
        assert_ne!(
            entry.result, "Value",
            "{} has raw Value result",
            entry.method
        );

        let opaque = entry.opaque_fields.iter().collect::<BTreeSet<_>>();
        assert_eq!(
            opaque.len(),
            entry.opaque_fields.len(),
            "{} repeats opaque fields",
            entry.method
        );
        let sensitive = entry.sensitive_fields.iter().collect::<BTreeSet<_>>();
        assert_eq!(
            sensitive.len(),
            entry.sensitive_fields.len(),
            "{} repeats sensitive fields",
            entry.method
        );
    }
}

#[test]
fn mcp_read_contracts_never_classify_secret_bearing_configuration_as_output() {
    let entries = inventory()
        .into_iter()
        .map(|entry| (entry.method.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    for method_name in [
        method::MCP_LIST_SERVERS,
        method::MCP_SERVER_TRUST,
        method::MCP_LIST_TOOLS,
        method::MCP_LIST_RESOURCES,
        method::MCP_READ_RESOURCE,
        method::MCP_LIST_PROMPTS,
        method::MCP_GET_PROMPT,
        method::MCP_DIAGNOSE,
        method::MCP_CAPABILITIES,
        method::MCP_SLASH_SUGGESTIONS,
        method::MCP_OAUTH_OVERVIEW,
    ] {
        let entry = &entries[method_name];
        assert!(
            entry.sensitive_fields.is_empty(),
            "{method_name} must not expose sensitive output fields"
        );
    }
}
