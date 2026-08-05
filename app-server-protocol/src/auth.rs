use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use orbcode_protocol::ProviderId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    ApiKey,
    #[serde(alias = "oauth_device")]
    OAuthDevice,
    #[serde(rename = "chatgpt")]
    ChatGpt,
}

impl fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ApiKey => "api_key",
            Self::OAuthDevice => "oauth_device",
            Self::ChatGpt => "chatgpt",
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AuthStatusEntry {
    #[schemars(with = "String")]
    pub provider: ProviderId,
    pub method: AuthMethod,
    pub source_summary: String,
    pub persisted: bool,
    pub usable: bool,
    pub active: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AuthOverview {
    pub store_path: PathBuf,
    pub entries: Vec<AuthStatusEntry>,
}

impl AuthOverview {
    pub fn has_provider(&self, provider: ProviderId) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.provider == provider && entry.usable)
    }
}

/// Authentication view embedded in diagnostics/status. Protocol 1.0 includes
/// credential metadata timestamps here, unlike the dedicated auth methods.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct StatusAuthStatusEntry {
    #[schemars(with = "String")]
    pub provider: ProviderId,
    pub method: AuthMethod,
    pub source_summary: String,
    pub persisted: bool,
    pub usable: bool,
    pub active: bool,
    #[schemars(with = "Option<String>")]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct StatusAuthOverview {
    pub store_path: PathBuf,
    pub entries: Vec<StatusAuthStatusEntry>,
}
