use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::{CostSummary, ProviderId, TokenUsage};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextTokenSource {
    ProviderCountTokens,
    RoughEstimateFallback,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCategoryBreakdown {
    pub system_prompt: u32,
    pub system_tools: u32,
    pub mcp_tools: u32,
    pub memory: u32,
    pub skills: u32,
    pub conversation: u32,
    pub attachments: u32,
    pub uncategorized: u32,
}

impl ContextCategoryBreakdown {
    pub fn total(&self) -> u32 {
        self.system_prompt
            .saturating_add(self.system_tools)
            .saturating_add(self.mcp_tools)
            .saturating_add(self.memory)
            .saturating_add(self.skills)
            .saturating_add(self.conversation)
            .saturating_add(self.attachments)
            .saturating_add(self.uncategorized)
    }

    pub fn system_overhead(&self) -> u32 {
        self.system_prompt
            .saturating_add(self.system_tools)
            .saturating_add(self.mcp_tools)
            .saturating_add(self.memory)
            .saturating_add(self.skills)
            .saturating_add(self.uncategorized)
    }

    pub fn conversation_overhead(&self) -> u32 {
        self.conversation.saturating_add(self.attachments)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsageOverview {
    pub model: String,
    pub estimated_tokens: u32,
    pub token_source: ContextTokenSource,
    pub categories: ContextCategoryBreakdown,
    pub system_tools_tokens: u32,
    pub message_tokens: u32,
    pub context_window: u32,
    pub reserved_output_tokens: u32,
    pub reserved_buffer_tokens: u32,
    pub reserved_context_tokens: u32,
    pub free_space_tokens: u32,
    pub effective_context_window: u32,
    pub auto_compact_threshold: u32,
    pub warning_threshold: u32,
    pub error_threshold: u32,
    pub blocking_limit: u32,
    pub percent_left: u32,
    pub is_above_warning_threshold: bool,
    pub is_above_error_threshold: bool,
    pub is_above_auto_compact_threshold: bool,
    pub is_at_blocking_limit: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDiagnosticsReport {
    pub sections: Vec<ContextDiagnosticSection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDiagnosticSection {
    pub category: ContextDiagnosticCategory,
    pub status: ContextDiagnosticStatus,
    pub summary: String,
    pub details: Vec<String>,
    pub token_estimate: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextDiagnosticCategory {
    SystemPrompt,
    Settings,
    Tools,
    Mcp,
    Git,
    AddDir,
    Exclusions,
    Memory,
}

impl ContextDiagnosticCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::SystemPrompt => "System prompt",
            Self::Settings => "Settings",
            Self::Tools => "Tools",
            Self::Mcp => "MCP",
            Self::Git => "Git",
            Self::AddDir => "Add-dir",
            Self::Exclusions => "Exclusions",
            Self::Memory => "Memory",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextDiagnosticStatus {
    Loaded,
    Configured,
    Empty,
    Skipped,
}

impl ContextDiagnosticStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Configured => "configured",
            Self::Empty => "empty",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageOverview {
    pub session_id: String,
    pub model: String,
    pub provider: ProviderId,
    pub message_count: usize,
    pub assistant_message_count: usize,
    pub usage_message_count: usize,
    pub total_usage: TokenUsage,
    pub cost: CostSummary,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostOverview {
    pub session_id: String,
    pub model: String,
    pub provider: ProviderId,
    pub cost: CostSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsOverview {
    pub window_days: usize,
    pub message_count: usize,
    pub activity_days: Vec<StatsActivityDay>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsActivityDay {
    pub date: NaiveDate,
    pub message_count: usize,
}
