use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvancedCapabilityStatus {
    Implemented,
    Deferred,
}

impl fmt::Display for AdvancedCapabilityStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Implemented => f.write_str("implemented"),
            Self::Deferred => f.write_str("deferred"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdvancedCapability {
    pub name: &'static str,
    pub status: AdvancedCapabilityStatus,
    pub summary: &'static str,
}

pub fn advanced_capabilities() -> Vec<AdvancedCapability> {
    vec![
        AdvancedCapability {
            name: "background_sessions",
            status: AdvancedCapabilityStatus::Implemented,
            summary: "Queued background prompt execution with list/log/kill/attach flows.",
        },
        AdvancedCapability {
            name: "remote_control_bridge",
            status: AdvancedCapabilityStatus::Deferred,
            summary: "Bridge and remote-control transport stay out of scope for this Rust shell.",
        },
        AdvancedCapability {
            name: "voice",
            status: AdvancedCapabilityStatus::Deferred,
            summary: "Voice capture and streaming STT stay deferred in Phase 5.",
        },
        AdvancedCapability {
            name: "computer_use",
            status: AdvancedCapabilityStatus::Deferred,
            summary: "Computer-use and platform automation stay deferred in Phase 5.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advanced_capabilities_report_background_sessions_as_active() {
        let capabilities = advanced_capabilities();

        assert!(capabilities.iter().any(|capability| {
            capability.name == "background_sessions"
                && capability.status == AdvancedCapabilityStatus::Implemented
        }));
        assert!(capabilities.iter().any(|capability| {
            capability.name == "voice" && capability.status == AdvancedCapabilityStatus::Deferred
        }));
    }
}
