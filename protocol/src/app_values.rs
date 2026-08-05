use serde::{Deserialize, Serialize};

use crate::{ProviderId, SessionRecord, TokenUsage};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactSessionResult {
    pub session: SessionRecord,
    pub original_message_count: usize,
    pub compacted_message_count: usize,
    pub provider_generated: bool,
    pub fallback_reason: Option<String>,
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactDecision {
    Proceed,
    NeedsConfirmation {
        context_percent_used: u32,
        threshold_percent: u32,
    },
    SkippedRecentManual {
        turns_since_compact: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowSource {
    Project,
    User,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowCommand {
    pub name: String,
    pub description: String,
    pub source: WorkflowSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRequestDebugSnapshot {
    pub provider: ProviderId,
    pub source: String,
    pub session_id: String,
    pub model: String,
    pub base_url: String,
    pub captured_at: String,
    pub recent_activity_json: String,
    pub previous_turn_json: String,
    pub body_json: String,
}
