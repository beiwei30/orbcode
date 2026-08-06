use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use orbcode_protocol::ProviderId;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ContextWindowOptions {
    pub disable_1m_context: bool,
    pub max_context_tokens_override: Option<u32>,
    pub auto_compact_window_override: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct MaxOutputTokenOptions {
    pub max_output_tokens_override: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TokenWarningOptions {
    pub auto_compact_enabled: bool,
    pub auto_compact_percent_override: Option<u32>,
    pub blocking_limit_override: Option<u32>,
}

impl Default for TokenWarningOptions {
    fn default() -> Self {
        Self {
            auto_compact_enabled: true,
            auto_compact_percent_override: None,
            blocking_limit_override: None,
        }
    }
}

/// Resolved statusline configuration. The server never executes `command`;
/// execution remains a client responsibility.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct StatuslineConfig {
    #[serde(rename = "statusline_command")]
    pub command: Option<String>,
    #[serde(rename = "statusline_refresh_interval_secs")]
    pub refresh_interval_secs: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingSource {
    Defaults,
    User,
    Project,
    Local,
    Managed,
    Cli,
    Environment,
    Session,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PersistedModelSetting {
    pub value: Option<String>,
    pub source: Option<SettingSource>,
    pub locked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", content = "model", rename_all = "snake_case")]
pub enum RuntimeModelOverride {
    Inherit,
    Default,
    Model(String),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelSelectionSource {
    Runtime,
    Environment,
    Persisted,
    ProviderDefault,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProviderModelSelection {
    pub requested_setting: Option<String>,
    pub family: Option<String>,
    pub model: String,
    pub request_model: String,
    pub display_label: String,
    pub display_name: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct EffectiveModelSelection {
    pub persisted: PersistedModelSetting,
    pub runtime_override: RuntimeModelOverride,
    pub requested_model: Option<String>,
    pub source: ModelSelectionSource,
    #[schemars(with = "String")]
    pub provider: ProviderId,
    pub resolution: ProviderModelSelection,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeSetting {
    #[default]
    Auto,
    Dark,
    Light,
    DarkDaltonized,
    LightDaltonized,
    DarkAnsi,
    LightAnsi,
}

impl ThemeSetting {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "dark-daltonized" => Some(Self::DarkDaltonized),
            "light-daltonized" => Some(Self::LightDaltonized),
            "dark-ansi" => Some(Self::DarkAnsi),
            "light-ansi" => Some(Self::LightAnsi),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::DarkDaltonized => "dark-daltonized",
            Self::LightDaltonized => "light-daltonized",
            Self::DarkAnsi => "dark-ansi",
            Self::LightAnsi => "light-ansi",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EditorModeSetting {
    #[default]
    Normal,
    Vim,
}

/// Client-owned appearance and editing preferences. Bootstrap uses the legacy
/// PascalCase wire spelling through field adapters while typed settings
/// methods use each enum's canonical settings spelling.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ClientPreferences {
    #[serde(with = "legacy_theme_wire")]
    #[schemars(with = "String")]
    pub theme: ThemeSetting,
    #[serde(with = "legacy_editor_mode_wire")]
    #[schemars(with = "String")]
    pub editor_mode: EditorModeSetting,
}

mod legacy_theme_wire {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::ThemeSetting;

    pub fn serialize<S>(value: &ThemeSetting, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match value {
            ThemeSetting::Auto => "Auto",
            ThemeSetting::Dark => "Dark",
            ThemeSetting::Light => "Light",
            ThemeSetting::DarkDaltonized => "DarkDaltonized",
            ThemeSetting::LightDaltonized => "LightDaltonized",
            ThemeSetting::DarkAnsi => "DarkAnsi",
            ThemeSetting::LightAnsi => "LightAnsi",
        })
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<ThemeSetting, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "Auto" => Ok(ThemeSetting::Auto),
            "Dark" => Ok(ThemeSetting::Dark),
            "Light" => Ok(ThemeSetting::Light),
            "DarkDaltonized" => Ok(ThemeSetting::DarkDaltonized),
            "LightDaltonized" => Ok(ThemeSetting::LightDaltonized),
            "DarkAnsi" => Ok(ThemeSetting::DarkAnsi),
            "LightAnsi" => Ok(ThemeSetting::LightAnsi),
            other => ThemeSetting::parse(other)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown theme: {other}"))),
        }
    }
}

mod legacy_editor_mode_wire {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::EditorModeSetting;

    pub fn serialize<S>(value: &EditorModeSetting, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match value {
            EditorModeSetting::Normal => "Normal",
            EditorModeSetting::Vim => "Vim",
        })
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<EditorModeSetting, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "Normal" => Ok(EditorModeSetting::Normal),
            "Vim" => Ok(EditorModeSetting::Vim),
            other => EditorModeSetting::parse(other)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown editor mode: {other}"))),
        }
    }
}

impl EditorModeSetting {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "normal" | "emacs" => Some(Self::Normal),
            "vim" => Some(Self::Vim),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Vim => "vim",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SandboxLocalSettings {
    pub enabled: bool,
    pub auto_allow_bash_if_sandboxed: bool,
    pub allow_unsandboxed_commands: bool,
    pub excluded_commands: Vec<String>,
    pub filesystem: SandboxFilesystemLocalSettings,
    pub network: SandboxNetworkLocalSettings,
}

impl Default for SandboxLocalSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_allow_bash_if_sandboxed: true,
            allow_unsandboxed_commands: true,
            excluded_commands: Vec::new(),
            filesystem: SandboxFilesystemLocalSettings::default(),
            network: SandboxNetworkLocalSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SandboxFilesystemLocalSettings {
    pub allow_write: Vec<String>,
    pub deny_write: Vec<String>,
    pub deny_read: Vec<String>,
    pub allow_read: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SandboxNetworkLocalSettings {
    pub allowed_domains: Vec<String>,
    pub allow_unix_sockets: Vec<String>,
    pub allow_all_unix_sockets: Option<bool>,
    pub allow_local_binding: Option<bool>,
    pub http_proxy_port: Option<u64>,
    pub socks_proxy_port: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SandboxSettingsUpdate {
    pub enabled: Option<bool>,
    pub auto_allow_bash_if_sandboxed: Option<bool>,
    pub allow_unsandboxed_commands: Option<bool>,
}
