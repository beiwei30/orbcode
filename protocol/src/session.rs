use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::provider::{EffortLevel, ProviderId};
use crate::usage::TokenUsage;

pub type SessionId = String;

fn is_false(v: &bool) -> bool {
    !v
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TranscriptBlock {
    Text {
        text: String,
    },
    Thinking {
        text: String,
        #[serde(default)]
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
        #[serde(default)]
        metadata: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    #[serde(default)]
    pub blocks: Vec<TranscriptBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_synthetic: bool,
}

impl TranscriptMessage {
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        let content = content.into();
        let blocks = if content.is_empty() {
            Vec::new()
        } else {
            vec![TranscriptBlock::Text {
                text: content.clone(),
            }]
        };
        Self::from_parts(role, content, blocks)
    }

    pub fn from_blocks(role: MessageRole, blocks: Vec<TranscriptBlock>) -> Self {
        let content = visible_content_from_blocks(&blocks);
        Self::from_parts(role, content, blocks)
    }

    pub fn from_parts(
        role: MessageRole,
        content: impl Into<String>,
        blocks: Vec<TranscriptBlock>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role,
            content: content.into(),
            blocks,
            stop_reason: None,
            usage: None,
            created_at: Utc::now(),
            is_synthetic: false,
        }
    }

    pub fn with_stop_reason(mut self, stop_reason: impl Into<String>) -> Self {
        self.stop_reason = Some(stop_reason.into());
        self
    }

    pub fn with_usage(mut self, usage: TokenUsage) -> Self {
        self.usage = Some(usage);
        self
    }

    pub fn with_synthetic(mut self, synthetic: bool) -> Self {
        self.is_synthetic = synthetic;
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRecord {
    pub session_id: SessionId,
    pub title: Option<String>,
    #[serde(default)]
    pub custom_title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<ProviderId>,
    #[serde(default)]
    pub additional_directories: Vec<String>,
    #[serde(default)]
    pub session_allowed_tools: Vec<String>,
    #[serde(default)]
    pub session_disallowed_tools: Vec<String>,
    #[serde(default)]
    pub session_effort: Option<EffortLevel>,
    pub messages: Vec<TranscriptMessage>,
}

impl Default for SessionRecord {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRecord {
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            session_id: Uuid::new_v4().to_string(),
            title: None,
            custom_title: None,
            created_at: now,
            updated_at: now,
            cwd: None,
            git_branch: None,
            model: None,
            provider: None,
            additional_directories: Vec::new(),
            session_allowed_tools: Vec::new(),
            session_disallowed_tools: Vec::new(),
            session_effort: None,
            messages: Vec::new(),
        }
    }

    pub fn push_message(&mut self, message: TranscriptMessage) {
        if self.title.is_none() && matches!(message.role, MessageRole::User) {
            self.title = Some(truncate_title(&message.content));
        }
        self.updated_at = message.created_at;
        self.messages.push(message);
    }

    /// The user-visible title: the custom title if set, otherwise the auto title.
    pub fn display_title(&self) -> Option<&str> {
        self.custom_title.as_deref().or(self.title.as_deref())
    }

    pub fn summary(&self) -> SessionSummary {
        let (total_input_tokens, total_output_tokens) = self.aggregate_token_usage();
        let duration_secs = self.duration_secs();
        SessionSummary {
            session_id: self.session_id.clone(),
            title: self.display_title().map(str::to_string),
            custom_title: self.custom_title.clone(),
            message_count: self.messages.len(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            cwd: self.cwd.clone(),
            git_branch: self.git_branch.clone(),
            model: self.model.clone(),
            provider: self.provider,
            transcript_path: None,
            status: SessionStatus::Available,
            total_input_tokens,
            total_output_tokens,
            duration_secs,
        }
    }

    fn aggregate_token_usage(&self) -> (u64, u64) {
        let mut input: u64 = 0;
        let mut output: u64 = 0;
        for message in &self.messages {
            if let Some(usage) = &message.usage {
                input = input.saturating_add(u64::from(usage.input_tokens));
                output = output.saturating_add(u64::from(usage.output_tokens));
            }
        }
        (input, output)
    }

    fn duration_secs(&self) -> Option<u64> {
        if self.messages.is_empty() {
            return None;
        }
        let duration = self.updated_at - self.created_at;
        let secs = duration.num_seconds();
        if secs >= 0 { Some(secs as u64) } else { None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub title: Option<String>,
    #[serde(default)]
    pub custom_title: Option<String>,
    pub message_count: usize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<ProviderId>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub status: SessionStatus,
    #[serde(default)]
    pub total_input_tokens: u64,
    #[serde(default)]
    pub total_output_tokens: u64,
    #[serde(default)]
    pub duration_secs: Option<u64>,
}

impl Default for SessionSummary {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            session_id: String::new(),
            title: None,
            custom_title: None,
            message_count: 0,
            created_at: now,
            updated_at: now,
            cwd: None,
            git_branch: None,
            model: None,
            provider: None,
            transcript_path: None,
            status: SessionStatus::Available,
            total_input_tokens: 0,
            total_output_tokens: 0,
            duration_secs: None,
        }
    }
}

impl SessionSummary {
    pub fn display_title(&self) -> Option<&str> {
        self.custom_title.as_deref().or(self.title.as_deref())
    }
}

/// Assigns unique display titles to a list of session summaries by appending
/// a short session-id suffix (`(abc1)`) to duplicates. The first occurrence
/// (by list position) keeps the bare title; subsequent duplicates get the
/// suffix. Summaries without a title are left untouched.
pub fn unique_display_titles(summaries: &[SessionSummary]) -> Vec<Option<String>> {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for summary in summaries {
        if let Some(title) = summary.display_title() {
            *counts.entry(title).or_insert(0) += 1;
        }
    }
    let mut seen: HashMap<&str, bool> = HashMap::new();
    summaries
        .iter()
        .map(|summary| {
            let title = summary.display_title()?;
            let has_duplicates = counts.get(title).copied().unwrap_or(0) > 1;
            if !has_duplicates {
                return Some(title.to_string());
            }
            let first = seen.entry(title).or_insert(true);
            if *first {
                *first = false;
                Some(title.to_string())
            } else {
                let short_id = &summary.session_id[..summary.session_id.len().min(8)];
                Some(format!("{title} ({short_id})"))
            }
        })
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionStatus {
    #[default]
    Available,
    /// The transcript exists on disk but could not be loaded.
    /// `reason` is a short human-readable diagnostic.
    Corrupt { reason: String },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnContext {
    pub cwd: String,
    #[serde(default)]
    pub additional_directories: Vec<String>,
    #[serde(default)]
    pub additional_directory_details: Vec<AdditionalDirectoryInfo>,
    #[serde(default)]
    pub repo_root: Option<String>,
    #[serde(default)]
    pub cwd_relative_to_repo: Option<String>,
    pub current_date: String,
    pub git_branch: Option<String>,
    #[serde(default)]
    pub git_default_branch: Option<String>,
    #[serde(default)]
    pub git_user: Option<String>,
    pub git_status: Option<String>,
    #[serde(default)]
    pub git_recent_commits: Option<String>,
    #[serde(default)]
    pub git_remote: Option<String>,
    #[serde(default)]
    pub git_worktree_state: Option<WorktreeState>,
    #[serde(default)]
    pub trusted_project: Option<bool>,
    #[serde(default)]
    pub memory_sources: Vec<MemorySource>,
    #[serde(default)]
    pub claude_md: Option<String>,
}

impl TurnContext {
    pub fn compact_summary(&self) -> String {
        let branch = self.git_branch.as_deref().unwrap_or("no-git");
        let mut parts = vec![
            format!("cwd={}", self.cwd),
            format!("date={}", self.current_date),
            format!("branch={branch}"),
        ];
        if let Some(default_branch) = self.git_default_branch.as_deref()
            && Some(default_branch) != self.git_branch.as_deref()
        {
            parts.push(format!("default_branch={default_branch}"));
        }
        if !self.additional_directories.is_empty() {
            parts.push(format!(
                "additional_dirs={}",
                self.additional_directories.len()
            ));
        }
        if let Some(repo_root) = self.repo_root.as_deref() {
            parts.push(format!("repo_root={repo_root}"));
        }
        if let Some(relative) = self.cwd_relative_to_repo.as_deref() {
            parts.push(format!("repo_subdir={relative}"));
        }
        if let Some(state) = self.git_worktree_state
            && !matches!(state, WorktreeState::Normal)
        {
            parts.push(format!("worktree={}", state.as_label()));
        }
        if let Some(trusted) = self.trusted_project {
            parts.push(format!("trusted={trusted}"));
        }
        parts.join(" ")
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemorySourceKind {
    Managed,
    User,
    Project,
    Local,
    Team,
    Agent,
    Skill,
}

impl MemorySourceKind {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Managed => "Managed memory",
            Self::User => "User memory",
            Self::Project => "Project memory",
            Self::Local => "Local memory",
            Self::Team => "Team memory",
            Self::Agent => "Agent memory",
            Self::Skill => "Skill memory",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemorySourceStatus {
    Loaded,
    Empty,
    Missing,
    Skipped,
}

impl MemorySourceStatus {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Empty => "empty",
            Self::Missing => "missing",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySource {
    pub kind: MemorySourceKind,
    pub label: String,
    pub path: Option<String>,
    pub status: MemorySourceStatus,
    pub writable: bool,
    #[serde(default)]
    pub trust_boundary: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub skipped_reason: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdditionalDirectoryInfo {
    pub path: String,
    #[serde(default)]
    pub has_claude_md: bool,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub repo_root: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorktreeState {
    Normal,
    Detached,
    Linked,
}

impl WorktreeState {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Detached => "detached HEAD",
            Self::Linked => "linked worktree",
        }
    }
}

pub fn visible_content_from_blocks(blocks: &[TranscriptBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::Text { text } => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then_some(trimmed.to_string())
            }
            TranscriptBlock::Thinking { .. }
            | TranscriptBlock::ToolUse { .. }
            | TranscriptBlock::ToolResult { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn blocks_have_renderable_content(blocks: &[TranscriptBlock]) -> bool {
    blocks.iter().any(|block| match block {
        TranscriptBlock::Text { text } => !text.trim().is_empty(),
        TranscriptBlock::Thinking { .. } => false,
        TranscriptBlock::ToolUse { .. } | TranscriptBlock::ToolResult { .. } => true,
    })
}

fn truncate_title(input: &str) -> String {
    const MAX_TITLE_LEN: usize = 48;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "Untitled Session".to_string();
    }

    let mut title = String::new();
    for ch in trimmed.chars().take(MAX_TITLE_LEN) {
        title.push(ch);
    }
    if trimmed.chars().count() > MAX_TITLE_LEN {
        title.push_str("...");
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::TokenUsage;

    #[test]
    fn transcript_message_content_omits_thinking_blocks() {
        let message = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::Thinking {
                    text: "private chain of thought".to_string(),
                    signature: None,
                },
                TranscriptBlock::Text {
                    text: "public answer".to_string(),
                },
            ],
        );

        assert_eq!(message.content, "public answer");
        assert!(matches!(
            message.blocks.as_slice(),
            [
                TranscriptBlock::Thinking { text, .. },
                TranscriptBlock::Text { text: visible }
            ] if text == "private chain of thought" && visible == "public answer"
        ));
    }

    #[test]
    fn transcript_message_content_omits_tool_blocks() {
        let message = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![
                TranscriptBlock::Text {
                    text: "Let me inspect the workspace.".to_string(),
                },
                TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "glob".to_string(),
                    input: r#"{"pattern":"src/**/*"}"#.to_string(),
                },
            ],
        );

        assert_eq!(message.content, "Let me inspect the workspace.");
    }

    #[test]
    fn tool_only_blocks_are_still_renderable_even_without_visible_content() {
        assert!(blocks_have_renderable_content(&[
            TranscriptBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "glob".to_string(),
                input: r#"{"pattern":"src/**/*"}"#.to_string(),
            }
        ]));
        assert_eq!(
            visible_content_from_blocks(&[TranscriptBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "glob".to_string(),
                input: r#"{"pattern":"src/**/*"}"#.to_string(),
            }]),
            ""
        );
    }

    #[test]
    fn session_summary_display_title_prefers_custom() {
        let summary = SessionSummary {
            title: Some("auto title".to_string()),
            custom_title: Some("custom title".to_string()),
            ..Default::default()
        };
        assert_eq!(summary.display_title(), Some("custom title"));
    }

    #[test]
    fn session_summary_display_title_falls_back_to_auto() {
        let summary = SessionSummary {
            title: Some("auto title".to_string()),
            custom_title: None,
            ..Default::default()
        };
        assert_eq!(summary.display_title(), Some("auto title"));
    }

    #[test]
    fn session_summary_display_title_none_when_no_title() {
        let summary = SessionSummary::default();
        assert_eq!(summary.display_title(), None);
    }

    #[test]
    fn unique_display_titles_adds_suffix_to_duplicates() {
        let summaries = vec![
            SessionSummary {
                session_id: "aaaa1111-0000".to_string(),
                title: Some("Fix bug".to_string()),
                ..Default::default()
            },
            SessionSummary {
                session_id: "bbbb2222-0000".to_string(),
                title: Some("Fix bug".to_string()),
                ..Default::default()
            },
            SessionSummary {
                session_id: "cccc3333-0000".to_string(),
                title: Some("Unique title".to_string()),
                ..Default::default()
            },
        ];
        let titles = unique_display_titles(&summaries);
        assert_eq!(titles[0], Some("Fix bug".to_string()));
        assert_eq!(titles[1], Some("Fix bug (bbbb2222)".to_string()));
        assert_eq!(titles[2], Some("Unique title".to_string()));
    }

    #[test]
    fn unique_display_titles_handles_no_title() {
        let summaries = vec![
            SessionSummary {
                session_id: "aaaa".to_string(),
                title: None,
                ..Default::default()
            },
            SessionSummary {
                session_id: "bbbb".to_string(),
                title: Some("Has title".to_string()),
                ..Default::default()
            },
        ];
        let titles = unique_display_titles(&summaries);
        assert_eq!(titles[0], None);
        assert_eq!(titles[1], Some("Has title".to_string()));
    }

    #[test]
    fn unique_display_titles_three_way_collision() {
        let summaries = vec![
            SessionSummary {
                session_id: "aaa".to_string(),
                title: Some("Same".to_string()),
                ..Default::default()
            },
            SessionSummary {
                session_id: "bbb".to_string(),
                title: Some("Same".to_string()),
                ..Default::default()
            },
            SessionSummary {
                session_id: "ccc".to_string(),
                title: Some("Same".to_string()),
                ..Default::default()
            },
        ];
        let titles = unique_display_titles(&summaries);
        assert_eq!(titles[0], Some("Same".to_string()));
        assert_eq!(titles[1], Some("Same (bbb)".to_string()));
        assert_eq!(titles[2], Some("Same (ccc)".to_string()));
    }

    #[test]
    fn session_record_summary_populates_enriched_fields() {
        let mut record = SessionRecord::new();
        record.session_id = "test".to_string();
        record.created_at = chrono::Utc::now() - chrono::Duration::seconds(120);
        let mut msg1 = TranscriptMessage::new(MessageRole::User, "hello");
        msg1.created_at = record.created_at;
        record.push_message(msg1);
        let mut msg2 = TranscriptMessage::new(MessageRole::Assistant, "hi");
        msg2.created_at = record.created_at + chrono::Duration::seconds(120);
        msg2.usage = Some(TokenUsage {
            input_tokens: 200,
            output_tokens: 80,
            ..Default::default()
        });
        record.push_message(msg2);

        let summary = record.summary();
        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.total_input_tokens, 200);
        assert_eq!(summary.total_output_tokens, 80);
        assert_eq!(summary.duration_secs, Some(120));
    }

    #[test]
    fn is_synthetic_defaults_to_false_in_deserialization() {
        let json = serde_json::json!({
            "id": "test-id",
            "role": "assistant",
            "content": "hello",
            "created_at": "2026-01-01T00:00:00Z"
        });
        let message: TranscriptMessage =
            serde_json::from_value(json).expect("deserializes without is_synthetic");
        assert!(!message.is_synthetic);
    }

    #[test]
    fn is_synthetic_round_trips_through_serde() {
        let message =
            TranscriptMessage::new(MessageRole::Assistant, "synthetic").with_synthetic(true);
        let json = serde_json::to_value(&message).expect("serializes");
        assert_eq!(json["is_synthetic"], serde_json::json!(true));
        let restored: TranscriptMessage = serde_json::from_value(json).expect("deserializes");
        assert!(restored.is_synthetic);
    }

    #[test]
    fn is_synthetic_omitted_when_false() {
        let message = TranscriptMessage::new(MessageRole::Assistant, "normal");
        let json = serde_json::to_value(&message).expect("serializes");
        assert!(json.get("is_synthetic").is_none());
    }
}
