use std::collections::BTreeMap;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum SkillSource {
    User,
    Project,
    Plugin { plugin_id: String },
    Bundled,
    Mcp { server_id: String },
}

impl SkillSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Plugin { .. } => "plugin",
            Self::Bundled => "bundled",
            Self::Mcp { .. } => "mcp",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SkillDefinition {
    pub name: String,
    pub description: Option<String>,
    pub when_to_use: Option<String>,
    pub allowed_tools: Vec<String>,
    pub model: Option<String>,
    pub assets: Vec<PathBuf>,
    pub path: PathBuf,
    pub body: String,
    pub source: SkillSource,
}

impl Default for SkillDefinition {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: None,
            when_to_use: None,
            allowed_tools: Vec::new(),
            model: None,
            assets: Vec::new(),
            path: PathBuf::new(),
            body: String::new(),
            source: SkillSource::User,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSource {
    BuiltIn,
    UserSettings,
    ProjectSettings,
    Plugin { plugin_id: String },
}

impl AgentSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::UserSettings => "user",
            Self::ProjectSettings => "project",
            Self::Plugin { .. } => "plugin",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentPermissionMode {
    Default,
    #[serde(alias = "accept-edits")]
    AcceptEdits,
    #[serde(alias = "bypass-permissions")]
    BypassPermissions,
    #[serde(alias = "dont-ask")]
    DontAsk,
    Plan,
    Auto,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AgentHookMatcher {
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub hooks: Vec<AgentHookCommand>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type")]
pub enum AgentHookCommand {
    #[serde(rename = "command")]
    Command {
        command: String,
        #[serde(default)]
        r#if: Option<String>,
        #[serde(default)]
        timeout: Option<f64>,
    },
    #[serde(other)]
    Unsupported,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AgentDefinition {
    pub agent_type: String,
    pub description: String,
    pub prompt: String,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub model: Option<String>,
    pub permission_mode: Option<AgentPermissionMode>,
    pub skills: Vec<String>,
    pub mcp_server_names: Option<Vec<String>>,
    pub hooks: BTreeMap<String, Vec<AgentHookMatcher>>,
    pub source: AgentSource,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentWarningKind {
    MissingField,
    DuplicateName,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AgentLoadWarning {
    pub kind: AgentWarningKind,
    pub source: AgentSource,
    pub path: Option<PathBuf>,
    pub agent_type: Option<String>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookLayer {
    User,
    Project,
    Local,
    Managed,
}

impl HookLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
            Self::Managed => "managed",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookProvenance {
    BuiltIn,
    Settings(HookLayer),
    Skill { name: String },
    Agent { name: String },
    Plugin { plugin_id: String },
}

impl HookProvenance {
    pub fn label(&self) -> String {
        match self {
            Self::BuiltIn => "built-in".to_string(),
            Self::Settings(layer) => layer.as_str().to_string(),
            Self::Skill { name } => format!("skill:{name}"),
            Self::Agent { name } => format!("agent:{name}"),
            Self::Plugin { plugin_id } => format!("plugin:{plugin_id}"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookValidationStatus {
    Valid,
    Invalid(String),
}

impl HookValidationStatus {
    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DiscoveredHook {
    pub event: String,
    pub provenance: HookProvenance,
    pub matcher: Option<String>,
    pub command: String,
    pub trusted: bool,
    pub validation: HookValidationStatus,
}

impl DiscoveredHook {
    pub fn summary_line(&self) -> String {
        let matcher = self.matcher.as_deref().unwrap_or("*");
        let trust = if self.trusted { "trusted" } else { "untrusted" };
        let validity = match &self.validation {
            HookValidationStatus::Valid => "valid".to_string(),
            HookValidationStatus::Invalid(reason) => format!("invalid: {reason}"),
        };
        format!(
            "[{}] {} ({}) -> {} ({trust}, {validity})",
            self.provenance.label(),
            self.event,
            matcher,
            self.command
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct HookDiscoveryWarning {
    pub provenance: HookProvenance,
    pub event: String,
    pub message: String,
}

impl HookDiscoveryWarning {
    pub fn summary_line(&self) -> String {
        if self.event.is_empty() {
            format!("[{}] {}", self.provenance.label(), self.message)
        } else {
            format!(
                "[{}] {}: {}",
                self.provenance.label(),
                self.event,
                self.message
            )
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct HookDiscovery {
    pub hooks: Vec<DiscoveredHook>,
    pub warnings: Vec<HookDiscoveryWarning>,
}

impl HookDiscovery {
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn trusted_count(&self) -> usize {
        self.hooks.iter().filter(|hook| hook.trusted).count()
    }

    pub fn invalid_count(&self) -> usize {
        self.hooks
            .iter()
            .filter(|hook| !hook.validation.is_valid())
            .count()
    }
}
