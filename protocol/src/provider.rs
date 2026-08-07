use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProviderId {
    Anthropic,
    #[serde(rename = "openai", alias = "open_ai")]
    OpenAi,
    Gemini,
    Grok,
}

impl ProviderId {
    pub const ALL: [ProviderId; 4] = [
        ProviderId::Anthropic,
        ProviderId::OpenAi,
        ProviderId::Gemini,
        ProviderId::Grok,
    ];

    pub const ACTIVE: [ProviderId; 2] = [ProviderId::Anthropic, ProviderId::OpenAi];

    pub const DISABLED: [ProviderId; 2] = [ProviderId::Gemini, ProviderId::Grok];

    pub fn is_active(self) -> bool {
        matches!(self, Self::Anthropic | Self::OpenAi)
    }

    pub fn is_disabled(self) -> bool {
        !self.is_active()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAi),
            "gemini" => Some(Self::Gemini),
            "grok" => Some(Self::Grok),
            _ => None,
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str((*self).as_str())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    Max,
}

impl EffortLevel {
    pub const ALL: [EffortLevel; 4] = [
        EffortLevel::Low,
        EffortLevel::Medium,
        EffortLevel::High,
        EffortLevel::Max,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Low => "Quick, straightforward implementation with minimal overhead",
            Self::Medium => "Balanced approach with standard implementation and testing",
            Self::High => "Comprehensive implementation with extensive testing and documentation",
            Self::Max => "Maximum capability with deepest reasoning",
        }
    }
}

impl fmt::Display for EffortLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str((*self).as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum SandboxMode {
    #[default]
    DangerFullAccess,
    WorkspaceWrite,
    ReadOnly,
}

impl SandboxMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "danger-full-access" | "danger_full_access" | "danger" | "none" => {
                Some(Self::DangerFullAccess)
            }
            "workspace-write" | "workspace_write" | "workspace" => Some(Self::WorkspaceWrite),
            "read-only" | "read_only" | "readonly" => Some(Self::ReadOnly),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DangerFullAccess => "danger-full-access",
            Self::WorkspaceWrite => "workspace-write",
            Self::ReadOnly => "read-only",
        }
    }

    pub fn is_restrictive(self) -> bool {
        !matches!(self, Self::DangerFullAccess)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_subset_excludes_disabled_providers() {
        assert_eq!(
            ProviderId::ACTIVE.as_slice(),
            &[ProviderId::Anthropic, ProviderId::OpenAi]
        );
        assert!(ProviderId::Anthropic.is_active());
        assert!(ProviderId::OpenAi.is_active());
        assert!(!ProviderId::Gemini.is_active());
        assert!(!ProviderId::Grok.is_active());
    }

    #[test]
    fn all_contains_both_active_and_disabled_variants() {
        assert_eq!(ProviderId::ALL.len(), 4);
        for active in ProviderId::ACTIVE {
            assert!(ProviderId::ALL.contains(&active));
        }
        assert!(ProviderId::ALL.contains(&ProviderId::Gemini));
        assert!(ProviderId::ALL.contains(&ProviderId::Grok));
    }

    #[test]
    fn serde_roundtrip_preserves_disabled_variants() {
        for id in ProviderId::ALL {
            let json = serde_json::to_string(&id).expect("serialize");
            let parsed: ProviderId = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, id);
        }
    }

    #[test]
    fn serde_deserializes_legacy_config_with_disabled_providers() {
        let gemini: ProviderId = serde_json::from_str(r#""gemini""#).expect("deserialize gemini");
        assert_eq!(gemini, ProviderId::Gemini);

        let grok: ProviderId = serde_json::from_str(r#""grok""#).expect("deserialize grok");
        assert_eq!(grok, ProviderId::Grok);
    }

    #[test]
    fn parse_handles_disabled_providers() {
        assert_eq!(ProviderId::parse("gemini"), Some(ProviderId::Gemini));
        assert_eq!(ProviderId::parse("grok"), Some(ProviderId::Grok));
        assert_eq!(ProviderId::parse("GEMINI"), Some(ProviderId::Gemini));
        assert_eq!(ProviderId::parse("Grok"), Some(ProviderId::Grok));
    }

    /// Invariant: `ACTIVE` and `DISABLED` must partition `ALL` exactly.
    /// If a new variant is added to `ProviderId` without updating these
    /// arrays, this test fails — forcing the author to decide whether the
    /// new provider is active (has a real adapter) or disabled (placeholder).
    #[test]
    fn provider_surface_guard_active_disabled_partition_all() {
        let mut combined: Vec<ProviderId> = Vec::new();
        combined.extend_from_slice(&ProviderId::ACTIVE);
        combined.extend_from_slice(&ProviderId::DISABLED);
        combined.sort_by_key(|id| id.as_str().to_string());

        let mut all_sorted = ProviderId::ALL.to_vec();
        all_sorted.sort_by_key(|id| id.as_str().to_string());

        assert_eq!(
            combined, all_sorted,
            "ACTIVE ∪ DISABLED must equal ALL — a new ProviderId variant \
             was added without updating the partition arrays"
        );

        let active_set: std::collections::HashSet<ProviderId> =
            ProviderId::ACTIVE.iter().copied().collect();
        let disabled_set: std::collections::HashSet<ProviderId> =
            ProviderId::DISABLED.iter().copied().collect();

        assert!(
            active_set.is_disjoint(&disabled_set),
            "ACTIVE and DISABLED must not overlap"
        );
    }

    /// Invariant: `is_active()` and `is_disabled()` must agree with the
    /// const arrays. A new variant that doesn't update the match arms will
    /// be caught here.
    #[test]
    fn provider_surface_guard_is_active_matches_const_arrays() {
        for id in ProviderId::ALL {
            let in_active = ProviderId::ACTIVE.contains(&id);
            let in_disabled = ProviderId::DISABLED.contains(&id);
            assert_eq!(
                id.is_active(),
                in_active,
                "is_active() disagrees with ACTIVE array for {id}"
            );
            assert_eq!(
                id.is_disabled(),
                in_disabled,
                "is_disabled() disagrees with DISABLED array for {id}"
            );
            assert_ne!(
                id.is_active(),
                id.is_disabled(),
                "is_active() and is_disabled() must be mutually exclusive for {id}"
            );
        }
    }

    /// Invariant: every variant in ALL must round-trip through `as_str()`
    /// and `parse()`. If a new variant is added without updating both
    /// methods, this catches it.
    #[test]
    fn provider_surface_guard_as_str_parse_roundtrip_for_all_variants() {
        for id in ProviderId::ALL {
            let s = id.as_str();
            let parsed = ProviderId::parse(s);
            assert_eq!(
                parsed,
                Some(id),
                "ProviderId::{id:?} round-trip failed: as_str()={s:?}, parse()={parsed:?}"
            );
        }
    }
}
