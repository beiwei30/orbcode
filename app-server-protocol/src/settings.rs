use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
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
pub enum EditorModeSetting {
    #[default]
    Normal,
    Vim,
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
