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
    }

    #[test]
    fn client_capabilities_default() {
        let caps = ClientCapabilities::default();
        assert!(!caps.streaming);
        assert!(!caps.experimental_methods);
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
