use std::collections::HashSet;
use std::fmt::Write as _;

use orbcode_protocol::{
    MessageRole, TranscriptBlock, TranscriptMessage, visible_content_from_blocks,
};
use orbcode_session_store::{
    DEFAULT_MAX_TOOL_RESULT_SIZE_CHARS, PERSISTED_OUTPUT_CLOSING_TAG, PERSISTED_OUTPUT_TAG,
    format_tool_result_size,
};

pub(crate) const MISSING_TOOL_RESULT: &str = "Tool result was not recorded before the next model turn; treating this tool call as interrupted.";

const TOOL_ROUND_SUMMARY_PREVIEW_CHARS: usize = 160;

#[derive(Clone, Debug)]
struct ToolRoundResultSummary {
    tool_use_id: String,
    tool_name: String,
    is_error: bool,
    size: usize,
    summary_worthy: bool,
    preview: String,
}

#[derive(Clone, Debug)]
struct ToolRoundSummaryBuilder {
    tool_uses: Vec<(String, String)>,
    tool_use_ids: HashSet<String>,
    results: Vec<ToolRoundResultSummary>,
}

pub(crate) fn repair_missing_tool_results(
    messages: Vec<TranscriptMessage>,
) -> Vec<TranscriptMessage> {
    let messages = strip_orphaned_tool_results(messages);
    let mut repaired = Vec::with_capacity(messages.len());

    for (index, message) in messages.iter().enumerate() {
        repaired.push(message.clone());
        if !matches!(message.role, MessageRole::Assistant) {
            continue;
        }

        let missing = missing_tool_results_after_assistant(&messages, index);
        if missing.is_empty() {
            continue;
        }

        repaired.push(TranscriptMessage::from_blocks(
            MessageRole::User,
            missing
                .into_iter()
                .map(|tool_use_id| TranscriptBlock::ToolResult {
                    tool_use_id,
                    content: MISSING_TOOL_RESULT.to_string(),
                    is_error: true,
                    metadata: None,
                })
                .collect(),
        ));
    }

    repaired
}

fn strip_orphaned_tool_results(messages: Vec<TranscriptMessage>) -> Vec<TranscriptMessage> {
    let mut stripped = Vec::with_capacity(messages.len());
    let mut active_tool_use_ids: Option<HashSet<String>> = None;

    for message in messages {
        if matches!(message.role, MessageRole::Assistant) {
            active_tool_use_ids = tool_use_ids_in_message(&message);
            stripped.push(message);
            continue;
        }

        if matches!(message.role, MessageRole::User) {
            if let Some(message) =
                strip_orphaned_tool_result_blocks(message, active_tool_use_ids.as_ref())
            {
                stripped.push(message);
            }
            continue;
        }

        stripped.push(message);
    }

    stripped
}

fn tool_use_ids_in_message(message: &TranscriptMessage) -> Option<HashSet<String>> {
    let ids = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    (!ids.is_empty()).then_some(ids)
}

fn strip_orphaned_tool_result_blocks(
    mut message: TranscriptMessage,
    active_tool_use_ids: Option<&HashSet<String>>,
) -> Option<TranscriptMessage> {
    if message.blocks.is_empty() {
        return Some(message);
    }

    let mut changed = false;
    let mut blocks = Vec::with_capacity(message.blocks.len());
    for block in std::mem::take(&mut message.blocks) {
        match &block {
            TranscriptBlock::ToolResult { tool_use_id, .. }
                if !active_tool_use_ids.is_some_and(|ids| ids.contains(tool_use_id)) =>
            {
                changed = true;
            }
            _ => blocks.push(block),
        }
    }

    if !changed {
        message.blocks = blocks;
        return Some(message);
    }
    if blocks.is_empty() {
        return None;
    }

    message.content = visible_content_from_blocks(&blocks);
    message.blocks = blocks;
    Some(message)
}

fn missing_tool_results_after_assistant(
    messages: &[TranscriptMessage],
    assistant_index: usize,
) -> Vec<String> {
    let mut tool_use_ids = Vec::new();
    let mut seen_tool_uses = HashSet::new();
    for block in &messages[assistant_index].blocks {
        let TranscriptBlock::ToolUse { id, .. } = block else {
            continue;
        };
        if seen_tool_uses.insert(id.clone()) {
            tool_use_ids.push(id.clone());
        }
    }
    if tool_use_ids.is_empty() {
        return Vec::new();
    }

    let mut answered = HashSet::new();
    for message in messages.iter().skip(assistant_index + 1) {
        if matches!(message.role, MessageRole::Assistant) {
            break;
        }
        for block in &message.blocks {
            if let TranscriptBlock::ToolResult { tool_use_id, .. } = block {
                answered.insert(tool_use_id.clone());
            }
        }
    }

    tool_use_ids
        .into_iter()
        .filter(|tool_use_id| !answered.contains(tool_use_id))
        .collect()
}

pub(crate) fn add_tool_round_summaries(messages: Vec<TranscriptMessage>) -> Vec<TranscriptMessage> {
    let mut summarized = Vec::with_capacity(messages.len());
    let mut pending: Option<ToolRoundSummaryBuilder> = None;

    for message in messages {
        if pending
            .as_ref()
            .is_some_and(|summary| !summary.message_has_matching_tool_result(&message))
            && let Some(summary) = pending
                .take()
                .and_then(ToolRoundSummaryBuilder::into_message)
        {
            summarized.push(summary);
        }

        if let Some(summary) = pending.as_mut() {
            summary.collect_results_from_message(&message);
        }

        if matches!(message.role, MessageRole::Assistant) {
            let tool_uses = tool_uses_for_summary(&message);
            summarized.push(message);
            pending = (!tool_uses.is_empty()).then(|| ToolRoundSummaryBuilder::new(tool_uses));
        } else {
            summarized.push(message);
        }
    }

    if let Some(summary) = pending.and_then(ToolRoundSummaryBuilder::into_message) {
        summarized.push(summary);
    }

    summarized
}

impl ToolRoundSummaryBuilder {
    fn new(tool_uses: Vec<(String, String)>) -> Self {
        Self {
            tool_use_ids: tool_uses
                .iter()
                .map(|(tool_use_id, _)| tool_use_id.clone())
                .collect(),
            tool_uses,
            results: Vec::new(),
        }
    }

    fn message_has_matching_tool_result(&self, message: &TranscriptMessage) -> bool {
        matches!(message.role, MessageRole::User)
            && message.blocks.iter().any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult { tool_use_id, .. }
                        if self.tool_use_ids.contains(tool_use_id)
                )
            })
    }

    fn collect_results_from_message(&mut self, message: &TranscriptMessage) {
        if !matches!(message.role, MessageRole::User) {
            return;
        }
        for block in &message.blocks {
            let TranscriptBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } = block
            else {
                continue;
            };
            if !self.tool_use_ids.contains(tool_use_id)
                || self
                    .results
                    .iter()
                    .any(|result| result.tool_use_id == *tool_use_id)
            {
                continue;
            }
            let tool_name = self
                .tool_uses
                .iter()
                .find_map(|(id, name)| (id == tool_use_id).then(|| name.clone()))
                .unwrap_or_else(|| "tool".to_string());
            let size = content.chars().count();
            self.results.push(ToolRoundResultSummary {
                tool_use_id: tool_use_id.clone(),
                tool_name,
                is_error: *is_error,
                size,
                summary_worthy: size > DEFAULT_MAX_TOOL_RESULT_SIZE_CHARS
                    || content.trim_start().starts_with(PERSISTED_OUTPUT_TAG),
                preview: tool_round_summary_preview(content),
            });
        }
    }

    fn into_message(self) -> Option<TranscriptMessage> {
        if self.results.is_empty() {
            return None;
        }
        let should_summarize =
            self.tool_uses.len() > 1 || self.results.iter().any(|result| result.summary_worthy);
        if !should_summarize {
            return None;
        }

        let mut lines = vec![format!(
            "Tool round summary: {} tool result{}.",
            self.results.len(),
            if self.results.len() == 1 { "" } else { "s" }
        )];
        for result in self.results {
            let status = if result.is_error {
                "failed"
            } else {
                "completed"
            };
            let mut line = format!(
                "- {} `{}`: {}, {}",
                result.tool_name,
                result.tool_use_id,
                status,
                format_tool_result_size(result.size)
            );
            if !result.preview.is_empty() {
                write!(line, "; preview: {}", result.preview)
                    .expect("writing to String cannot fail");
            }
            lines.push(line);
        }

        Some(TranscriptMessage::new(MessageRole::User, lines.join("\n")))
    }
}

fn tool_uses_for_summary(message: &TranscriptMessage) -> Vec<(String, String)> {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::ToolUse { id, name, .. } => Some((id.clone(), name.clone())),
            _ => None,
        })
        .collect()
}

fn tool_round_summary_preview(content: &str) -> String {
    let content = content.trim();
    let content = content
        .strip_prefix(PERSISTED_OUTPUT_TAG)
        .unwrap_or(content)
        .trim();
    let content = content
        .strip_suffix(PERSISTED_OUTPUT_CLOSING_TAG)
        .unwrap_or(content)
        .trim();
    let preview = content.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_tool_round_summary_preview(&preview, TOOL_ROUND_SUMMARY_PREVIEW_CHARS)
}

fn truncate_tool_round_summary_preview(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut preview = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    preview.push_str("...");
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tool_result_repair_inserts_synthetic_result() {
        let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "bash".to_string(),
                        input: "{}".to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "tool-2".to_string(),
                        name: "glob".to_string(),
                        input: "{}".to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-2".to_string(),
                    content: "ok".to_string(),
                    is_error: false,
                    metadata: None,
                }],
            ),
            TranscriptMessage::new(MessageRole::User, "next prompt"),
        ];

        let repaired = repair_missing_tool_results(messages);

        assert_eq!(repaired.len(), 4);
        assert!(matches!(
            repaired[1].blocks.as_slice(),
            [TranscriptBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            }] if tool_use_id == "tool-1" && content == MISSING_TOOL_RESULT && *is_error
        ));
        assert!(matches!(
            repaired[2].blocks.as_slice(),
            [TranscriptBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            }] if tool_use_id == "tool-2" && content == "ok" && !*is_error
        ));
        assert_eq!(repaired[3].content, "next prompt");
    }

    #[test]
    fn multi_tool_round_summary_is_inserted_before_next_assistant_turn() {
        let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "bash".to_string(),
                        input: "{}".to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "tool-2".to_string(),
                        name: "glob".to_string(),
                        input: "{}".to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![
                    TranscriptBlock::ToolResult {
                        tool_use_id: "tool-1".to_string(),
                        content: "listed files".to_string(),
                        is_error: false,
                        metadata: None,
                    },
                    TranscriptBlock::ToolResult {
                        tool_use_id: "tool-2".to_string(),
                        content: "glob failed".to_string(),
                        is_error: true,
                        metadata: None,
                    },
                ],
            ),
            TranscriptMessage::new(MessageRole::Assistant, "next response"),
        ];

        let summarized = add_tool_round_summaries(messages);

        assert_eq!(summarized.len(), 4);
        assert_eq!(summarized[2].role, MessageRole::User);
        assert!(
            summarized[2]
                .content
                .starts_with("Tool round summary: 2 tool results.")
        );
        assert!(summarized[2].content.contains("bash `tool-1`: completed"));
        assert!(summarized[2].content.contains("glob `tool-2`: failed"));
        assert_eq!(summarized[3].content, "next response");
    }

    #[test]
    fn single_small_tool_round_does_not_add_summary() {
        let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "bash".to_string(),
                    input: "{}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "ok".to_string(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        ];

        let summarized = add_tool_round_summaries(messages);

        assert_eq!(summarized.len(), 2);
    }

    #[test]
    fn single_persisted_large_tool_round_adds_summary() {
        let content = format!(
            "{PERSISTED_OUTPUT_TAG}\nOutput too large (58.6 KB). Full output saved to: /tmp/tool-result.txt\n\nPreview (first 2.0 KB):\n{}\n...\n{PERSISTED_OUTPUT_CLOSING_TAG}",
            "line\n".repeat(20)
        );
        assert!(content.chars().count() < DEFAULT_MAX_TOOL_RESULT_SIZE_CHARS);
        let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "bash".to_string(),
                    input: "{}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content,
                    is_error: false,
                    metadata: None,
                }],
            ),
            TranscriptMessage::new(MessageRole::Assistant, "next response"),
        ];

        let summarized = add_tool_round_summaries(messages);

        assert_eq!(summarized.len(), 4);
        assert_eq!(summarized[2].role, MessageRole::User);
        assert!(
            summarized[2]
                .content
                .starts_with("Tool round summary: 1 tool result.")
        );
        assert!(summarized[2].content.contains("bash `tool-1`: completed"));
        assert!(
            summarized[2]
                .content
                .contains("Output too large (58.6 KB).")
        );
        assert!(!summarized[2].content.contains(PERSISTED_OUTPUT_TAG));
        assert_eq!(summarized[3].content, "next response");
    }
}
