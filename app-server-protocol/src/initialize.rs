use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters sent by the client in the `initialize` request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct InitializeParams {
    pub protocol_version: String,
    pub client_info: ClientInfo,
    #[serde(default)]
    pub capabilities: ClientCapabilities,
}

/// Identifies the connecting client.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Capabilities declared by the client during the handshake.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub experimental_methods: bool,
    /// Opts this connection into the experimental persistent-goal method
    /// family. `experimental_methods` must also be enabled.
    #[serde(default)]
    pub persistent_goals: bool,
    /// Interactive question behaviors this connection can complete. Missing
    /// means the capability is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive_questions: Option<InteractiveQuestionsCapability>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct InteractiveQuestionsCapability {
    #[serde(default)]
    pub single_select: bool,
    #[serde(default)]
    pub multi_select: bool,
    #[serde(default)]
    pub free_text: bool,
    #[serde(default)]
    pub previews: bool,
    #[serde(default)]
    pub annotations: bool,
    #[serde(default)]
    pub special_outcomes: bool,
}

impl InteractiveQuestionsCapability {
    pub fn full() -> Self {
        Self {
            single_select: true,
            multi_select: true,
            free_text: true,
            previews: true,
            annotations: true,
            special_outcomes: true,
        }
    }

    pub fn fully_supported(&self) -> bool {
        self.single_select
            && self.multi_select
            && self.free_text
            && self.previews
            && self.annotations
            && self.special_outcomes
    }

    /// ACP's stable `session/request_permission` mapping: one option may be
    /// selected, but rich canonical interactions remain unsupported.
    pub fn option_only() -> Self {
        Self {
            single_select: true,
            ..Self::default()
        }
    }
}

/// Result returned by the server for the `initialize` request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct InitializeResult {
    pub protocol_version: String,
    pub server_info: ServerInfo,
    pub capabilities: ServerCapabilities,
}

/// Identifies the server.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// Capabilities advertised by the server.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ServerCapabilities {
    pub streaming: bool,
    pub stable_methods: Vec<String>,
    pub experimental_methods: Vec<String>,
    pub server_notification_methods: Vec<String>,
    pub server_request_methods: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initialize_params_roundtrip() {
        let params = InitializeParams {
            protocol_version: "1.0".into(),
            client_info: ClientInfo {
                name: "test-client".into(),
                version: "0.1.0".into(),
            },
            capabilities: ClientCapabilities {
                streaming: true,
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&params).unwrap();
        let back: InitializeParams = serde_json::from_str(&json).unwrap();
        assert_eq!(params, back);
    }

    #[test]
    fn initialize_params_default_capabilities() {
        let value = json!({
            "protocol_version": "1.0",
            "client_info": { "name": "c", "version": "0.1" }
        });
        let params: InitializeParams = serde_json::from_value(value).unwrap();
        assert!(!params.capabilities.streaming);
        assert!(!params.capabilities.experimental_methods);
        assert!(!params.capabilities.persistent_goals);
        assert!(params.capabilities.interactive_questions.is_none());
    }

    #[test]
    fn client_capabilities_default() {
        let caps = ClientCapabilities::default();
        assert!(!caps.streaming);
        assert!(!caps.experimental_methods);
        assert!(!caps.persistent_goals);
        assert!(caps.interactive_questions.is_none());
    }

    #[test]
    fn client_capabilities_experimental_opt_in() {
        let value = json!({
            "streaming": true,
            "experimental_methods": true
        });
        let caps: ClientCapabilities = serde_json::from_value(value).unwrap();
        assert!(caps.streaming);
        assert!(caps.experimental_methods);
    }

    #[test]
    fn client_capabilities_experimental_defaults_false() {
        let value = json!({"streaming": true});
        let caps: ClientCapabilities = serde_json::from_value(value).unwrap();
        assert!(!caps.experimental_methods);
        assert!(!caps.persistent_goals);
    }

    #[test]
    fn persistent_goals_requires_explicit_opt_in() {
        let caps: ClientCapabilities = serde_json::from_value(json!({
            "streaming": true,
            "experimental_methods": true,
            "persistent_goals": true
        }))
        .unwrap();
        assert!(caps.experimental_methods);
        assert!(caps.persistent_goals);
    }

    #[test]
    fn interactive_questions_requires_explicit_complete_capability() {
        let full = InteractiveQuestionsCapability::full();
        assert!(full.fully_supported());
        let partial: InteractiveQuestionsCapability = serde_json::from_value(json!({
            "single_select": true
        }))
        .unwrap();
        assert!(!partial.fully_supported());
    }

    #[test]
    fn initialize_result_roundtrip() {
        let result = InitializeResult {
            protocol_version: "1.0".into(),
            server_info: ServerInfo {
                name: "orbcode".into(),
                version: "0.1.0".into(),
            },
            capabilities: ServerCapabilities {
                streaming: true,
                stable_methods: vec!["turn/submit".into(), "session/list".into()],
                experimental_methods: vec!["background/create".into()],
                server_notification_methods: vec!["stream/event".into()],
                server_request_methods: vec!["permission/request".into()],
            },
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: InitializeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, back);
    }

    #[test]
    fn server_capabilities_empty_methods() {
        let caps = ServerCapabilities {
            streaming: false,
            stable_methods: vec![],
            experimental_methods: vec![],
            server_notification_methods: vec![],
            server_request_methods: vec![],
        };
        let json = serde_json::to_string(&caps).unwrap();
        let back: ServerCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps, back);
        assert!(back.stable_methods.is_empty());
        assert!(back.experimental_methods.is_empty());
        assert!(back.server_notification_methods.is_empty());
        assert!(back.server_request_methods.is_empty());
    }

    #[test]
    fn client_info_roundtrip() {
        let info = ClientInfo {
            name: "my-tui".into(),
            version: "2.0.0".into(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: ClientInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn server_info_roundtrip() {
        let info = ServerInfo {
            name: "orbcode-server".into(),
            version: "0.5.0".into(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: ServerInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn initialize_params_missing_protocol_version_fails() {
        let raw = json!({"client_info": {"name": "c", "version": "0.1"}});
        let result = serde_json::from_value::<InitializeParams>(raw);
        assert!(result.is_err(), "missing protocol_version should fail");
    }

    #[test]
    fn initialize_params_missing_client_info_fails() {
        let raw = json!({"protocol_version": "1.0"});
        let result = serde_json::from_value::<InitializeParams>(raw);
        assert!(result.is_err(), "missing client_info should fail");
    }
}
