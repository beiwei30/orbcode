use std::path::Path;

use async_trait::async_trait;
use orbcode_config::{AppConfig, auto_compact_threshold, effective_context_window_size};
use orbcode_protocol::{
    MessageRole, ProviderId, SessionRecord, TokenUsage, TranscriptBlock, TranscriptMessage,
    rough_token_count_estimation_for_messages, visible_content_from_blocks,
};

use orbcode_model_provider::{
    AttemptDiscardDisposition, ProviderResponse, ProviderStreamAccumulator, ProviderStreamEvent,
    ProviderStreamSink,
};

/// Visible text substituted for tool-result block content cleared by
/// microcompaction. Mirrors the lightweight TypeScript pass that reclaims
/// context by dropping bulky tool output while keeping the tool_use/tool_result
/// pairing (and therefore the provider request) structurally valid.
pub(crate) const MICROCOMPACT_TOOL_RESULT_PLACEHOLDER: &str =
    "[Tool result content cleared to reclaim context (microcompact).]";

/// Marker prefixed onto an oversized message whose body was truncated in place
/// by a snip pass. Truncating in place (rather than removing the message)
/// preserves user/assistant alternation and tool pairing for the remaining
/// history.
pub(crate) const SNIP_TRUNCATED_PREFIX: &str = "[snipped oversized message] ";

/// Default text for the synthetic snip-boundary system message. Matches the
/// session-store reader placeholder so a snipped boundary round-trips to the
/// same visible content on resume.
pub(crate) const SNIP_BOUNDARY_TEXT: &str =
    "[snip] Conversation history before this point has been snipped.";

/// Environment overrides (read through the settings/env resolver) that let
/// tests drive the snip and microcompact thresholds deterministically without
/// constructing context windows large enough to hit the production defaults.
const MICROCOMPACT_THRESHOLD_ENV: &str = "ORBCODE_MICROCOMPACT_TOKEN_THRESHOLD_OVERRIDE";
const MICROCOMPACT_KEEP_RECENT_ENV: &str = "ORBCODE_MICROCOMPACT_KEEP_RECENT_OVERRIDE";
const SNIP_THRESHOLD_ENV: &str = "ORBCODE_SNIP_MESSAGE_TOKEN_THRESHOLD_OVERRIDE";

/// Number of most-recent messages microcompaction leaves untouched so the
/// active tool round and current prompt keep their full content.
const MICROCOMPACT_DEFAULT_KEEP_RECENT: usize = 4;

/// Tool-result blocks shorter than this (in characters) are left alone — the
/// reclaimed context would not justify the placeholder churn.
pub(crate) const MICROCOMPACT_MIN_RESULT_CHARS: usize = 200;

/// Head-preview length retained when truncating an oversized message in place.
const SNIP_PREVIEW_CHARS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CompactSessionResult {
    pub session: SessionRecord,
    pub original_message_count: usize,
    pub compacted_message_count: usize,
    pub provider_generated: bool,
    pub fallback_reason: Option<String>,
    pub usage: Option<TokenUsage>,
}

pub(crate) struct CompactProviderStreamSink {
    accumulator: ProviderStreamAccumulator,
}

impl CompactProviderStreamSink {
    pub(crate) fn new(provider: ProviderId) -> Self {
        Self {
            accumulator: ProviderStreamAccumulator::new(provider, None),
        }
    }

    pub(crate) fn into_response(self) -> ProviderResponse {
        self.accumulator.into_response()
    }
}

#[async_trait]
impl ProviderStreamSink for CompactProviderStreamSink {
    async fn emit(
        &mut self,
        event: ProviderStreamEvent,
    ) -> Result<(), orbcode_model_provider::ProviderError> {
        self.accumulator.apply(&event);
        Ok(())
    }

    async fn discard_attempt(
        &mut self,
        provider: ProviderId,
        fallback_provider: ProviderId,
        _reason: &str,
    ) -> Result<AttemptDiscardDisposition, orbcode_model_provider::ProviderError> {
        self.accumulator = ProviderStreamAccumulator::new(fallback_provider, Some(provider));
        Ok(AttemptDiscardDisposition::SafeToFallback)
    }
}

pub(crate) fn modeled_compaction_summary(messages: &[TranscriptMessage]) -> String {
    let user_count = messages
        .iter()
        .filter(|message| matches!(message.role, MessageRole::User))
        .count();
    let assistant_count = messages
        .iter()
        .filter(|message| matches!(message.role, MessageRole::Assistant))
        .count();
    let system_count = messages
        .iter()
        .filter(|message| matches!(message.role, MessageRole::System))
        .count();
    let first_user = messages
        .iter()
        .filter(|message| matches!(message.role, MessageRole::User))
        .find_map(compaction_message_preview);
    let last_user = messages
        .iter()
        .rev()
        .filter(|message| matches!(message.role, MessageRole::User))
        .find_map(compaction_message_preview);
    let last_assistant = messages
        .iter()
        .rev()
        .filter(|message| matches!(message.role, MessageRole::Assistant))
        .find_map(compaction_message_preview);

    let mut lines = vec![
        "Compacted conversation summary:".to_string(),
        "This is a local modeled compaction placeholder. Full provider-generated summarization has not run yet.".to_string(),
        format!(
            "Original messages: {} total ({} user, {} assistant, {} system).",
            messages.len(),
            user_count,
            assistant_count,
            system_count
        ),
    ];
    if let Some(first_user) = first_user {
        lines.push(format!("First user message: {first_user}"));
    }
    if let Some(last_user) = last_user {
        lines.push(format!("Most recent user message: {last_user}"));
    }
    if let Some(last_assistant) = last_assistant {
        lines.push(format!("Most recent assistant message: {last_assistant}"));
    }
    lines.join("\n")
}

pub(crate) fn compaction_prompt() -> String {
    let no_tools_preamble = r"CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.

- Do NOT use Read, Bash, Grep, Glob, Edit, Write, or ANY other tool.
- You already have all the context you need in the conversation above.
- Tool calls will be REJECTED and will waste your only turn — you will fail the task.
- Your entire response must be plain text: an <analysis> block followed by a <summary> block.

";

    let detailed_analysis_instruction_base = r"Before providing your final summary, wrap your analysis in <analysis> tags to organize your thoughts and ensure you've covered all necessary points. In your analysis process:

1. Chronologically analyze each message and section of the conversation. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names
     - full code snippets
     - function signatures
     - file edits
   - Errors that you ran into and how you fixed them
   - Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
2. Double-check for technical accuracy and completeness, addressing each required element thoroughly.";

    let base_compact_prompt = format!(
        r"Your task is to create a detailed summary of the conversation so far, paying close attention to the user's explicit requests and your previous actions.
This summary should be thorough in capturing technical details, code patterns, and architectural decisions that would be essential for continuing development work without losing context.

{detailed_analysis_instruction_base}

Your summary should include the following sections:

1. Primary Request and Intent: Capture all of the user's explicit requests and intents in detail
2. Key Technical Concepts: List all important technical concepts, technologies, and frameworks discussed.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. Pay special attention to the most recent messages and include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Errors and fixes: List all errors that you ran into, and how you fixed them. Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages that are not tool results. These are critical for understanding the users' feedback and changing intent.
7. Pending Tasks: Outline any pending tasks that you have explicitly been asked to work on.
8. Current Work: Describe in detail precisely what was being worked on immediately before this summary request, paying special attention to the most recent messages from both user and assistant. Include file names and code snippets where applicable.
9. Optional Next Step: List the next step that you will take that is related to the most recent work you were doing. IMPORTANT: ensure that this step is DIRECTLY in line with the user's most recent explicit requests, and the task you were working on immediately before this summary request. If your last task was concluded, then only list next steps if they are explicitly in line with the users request. Do not start on tangential requests or really old requests that were already completed without confirming with the user first.
                       If there is a next step, include direct quotes from the most recent conversation showing exactly what task you were working on and where you left off. This should be verbatim to ensure there's no drift in task interpretation.

Here's an example of how your output should be structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]
   - [...]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Summary of the changes made to this file, if any]
      - [Important Code Snippet]
   - [File Name 2]
      - [Important Code Snippet]
   - [...]

4. Errors and fixes:
    - [Detailed description of error 1]:
      - [How you fixed the error]
      - [User feedback on the error if any]
    - [...]

5. Problem Solving:
   [Description of solved problems and ongoing troubleshooting]

6. All user messages:
    - [Detailed non tool use user message]
    - [...]

7. Pending Tasks:
   - [Task 1]
   - [Task 2]
   - [...]

8. Current Work:
   [Precise description of current work]

9. Optional Next Step:
   [Optional Next step to take]

</summary>
</example>

Please provide your summary based on the conversation so far, following this structure and ensuring precision and thoroughness in your response.

There may be additional summarization instructions provided in the included context. If so, remember to follow these instructions when creating the above summary. Examples of instructions include:
<example>
## Compact Instructions
When summarizing the conversation focus on typescript code changes and also remember the mistakes you made and how you fixed them.
</example>

<example>
# Summary instructions
When you are using compact - please focus on test output and code changes. Include file reads verbatim.
</example>
"
    );

    let no_tools_trailer = "\n\nREMINDER: Do NOT call any tools. Respond with plain text only — an <analysis> block followed by a <summary> block. Tool calls will be rejected and you will fail the task.";

    format!("{no_tools_preamble}{base_compact_prompt}{no_tools_trailer}")
}

pub(crate) fn compact_user_summary_message(summary: &str, transcript_path: &Path) -> String {
    let formatted_summary = format_compact_summary(summary);
    format!(
        "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n{formatted_summary}\n\nIf you need specific details from before compaction (like exact code snippets, error messages, or content you generated), read the full transcript at: {}",
        transcript_path.display()
    )
}

pub(crate) fn compact_provider_messages(messages: &[TranscriptMessage]) -> Vec<TranscriptMessage> {
    messages
        .iter()
        .filter_map(|message| {
            let blocks = message_blocks(message)
                .into_iter()
                .filter(|block| !matches!(block, TranscriptBlock::Thinking { .. }))
                .collect::<Vec<_>>();
            if blocks.is_empty() {
                return None;
            }
            Some(TranscriptMessage::from_blocks(message.role.clone(), blocks))
        })
        .collect()
}

pub(crate) fn compact_empty_response_detail(response: &ProviderResponse) -> String {
    let block_types = response
        .blocks
        .iter()
        .map(|block| match block {
            TranscriptBlock::Text { .. } => "text",
            TranscriptBlock::Thinking { .. } => "thinking",
            TranscriptBlock::ToolUse { .. } => "tool_use",
            TranscriptBlock::ToolResult { .. } => "tool_result",
            _ => "unknown",
        })
        .collect::<Vec<_>>();
    format!(
        "stop_reason={}, blocks=[{}], deltas={}, output_tokens={}",
        response.stop_reason.as_deref().unwrap_or("null"),
        block_types.join(","),
        response.deltas.len(),
        response.usage.output_tokens
    )
}

pub(crate) fn compact_result_summary(session: &SessionRecord) -> Option<String> {
    session
        .messages
        .first()
        .map(|message| message.content.trim().to_string())
        .filter(|content| !content.is_empty())
}

pub(crate) fn format_compact_summary(summary: &str) -> String {
    let without_analysis = strip_tagged_section(summary, "analysis");
    let formatted = if let Some(summary_body) = extract_tagged_section(&without_analysis, "summary")
    {
        replace_tagged_section(
            &without_analysis,
            "summary",
            &format!("Summary:\n{}", summary_body.trim()),
        )
    } else {
        without_analysis
    };
    collapse_extra_blank_lines(&formatted).trim().to_string()
}

fn strip_tagged_section(input: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = input.find(&open) else {
        return input.to_string();
    };
    let Some(end_relative) = input[start + open.len()..].find(&close) else {
        return input.to_string();
    };
    let end = start + open.len() + end_relative + close.len();
    let mut output = String::with_capacity(input.len().saturating_sub(end - start));
    output.push_str(&input[..start]);
    output.push_str(&input[end..]);
    output
}

fn replace_tagged_section(input: &str, tag: &str, replacement: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = input.find(&open) else {
        return input.to_string();
    };
    let Some(end_relative) = input[start + open.len()..].find(&close) else {
        return input.to_string();
    };
    let end = start + open.len() + end_relative + close.len();
    let mut output = String::with_capacity(input.len() + replacement.len());
    output.push_str(&input[..start]);
    output.push_str(replacement);
    output.push_str(&input[end..]);
    output
}

fn collapse_extra_blank_lines(input: &str) -> String {
    let mut output = String::new();
    let mut blank_count = 0;
    for line in input.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                output.push('\n');
            }
        } else {
            blank_count = 0;
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(line);
        }
    }
    output
}

fn extract_tagged_section(input: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = input.find(&open)? + open.len();
    let end = input[start..].find(&close)? + start;
    Some(input[start..end].to_string())
}

fn compaction_message_preview(message: &TranscriptMessage) -> Option<String> {
    let text = visible_content_from_blocks(&message_blocks(message));
    let preview = truncate_compaction_preview(&collapse_compaction_whitespace(&text), 220);
    (!preview.trim().is_empty()).then_some(preview)
}

fn collapse_compaction_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_compaction_preview(text: &str, max_chars: usize) -> String {
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

fn message_blocks(message: &TranscriptMessage) -> Vec<TranscriptBlock> {
    if message.blocks.is_empty() {
        if message.content.is_empty() {
            Vec::new()
        } else {
            vec![TranscriptBlock::Text {
                text: message.content.clone(),
            }]
        }
    } else {
        message.blocks.clone()
    }
}

/// Per-message token budget above which a standalone text message is snipped.
/// Defaults to half the effective context window so only a genuinely oversized
/// single message trips it; an env override lets tests force the boundary low.
pub(crate) fn snip_message_token_threshold(config: &AppConfig, model: &str) -> u32 {
    if let Some(override_tokens) = resolve_threshold_override(config, SNIP_THRESHOLD_ENV) {
        return override_tokens;
    }
    effective_context_window_size(
        model,
        &config.context_window_options(),
        &config.max_output_token_options(),
    ) / 2
}

/// Total provider-visible token estimate above which microcompaction clears
/// old tool-result content. Defaults to the autocompact threshold so the cheap
/// pass runs first, before the expensive provider-summarized autocompact.
pub(crate) fn microcompact_token_threshold(config: &AppConfig, model: &str) -> u32 {
    if let Some(override_tokens) = resolve_threshold_override(config, MICROCOMPACT_THRESHOLD_ENV) {
        return override_tokens;
    }
    auto_compact_threshold(
        model,
        &config.context_window_options(),
        &config.max_output_token_options(),
        &config.token_warning_options(),
    )
}

pub(crate) fn microcompact_keep_recent(config: &AppConfig) -> usize {
    config
        .resolve_env(MICROCOMPACT_KEEP_RECENT_ENV)
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(MICROCOMPACT_DEFAULT_KEEP_RECENT)
}

fn resolve_threshold_override(config: &AppConfig, key: &str) -> Option<u32> {
    config
        .resolve_env(key)
        .and_then(|value| value.trim().parse::<u32>().ok())
}

/// Estimated provider-visible token size of the history.
pub(crate) fn estimated_history_tokens(messages: &[TranscriptMessage]) -> u32 {
    rough_token_count_estimation_for_messages(messages)
}

fn estimated_message_tokens(message: &TranscriptMessage) -> u32 {
    rough_token_count_estimation_for_messages(std::slice::from_ref(message))
}

fn message_has_tool_blocks(message: &TranscriptMessage) -> bool {
    message.blocks.iter().any(|block| {
        matches!(
            block,
            TranscriptBlock::ToolUse { .. } | TranscriptBlock::ToolResult { .. }
        )
    })
}

fn snip_boundary_message() -> TranscriptMessage {
    TranscriptMessage::new(MessageRole::System, SNIP_BOUNDARY_TEXT.to_string())
}

/// Text a snipped message should preview from. Falls back to block text
/// (including thinking) when `content` is empty, so a block-only message — which
/// is judged oversized by its blocks — does not snip down to just the prefix.
fn snip_source_text(message: &TranscriptMessage) -> String {
    if !message.content.trim().is_empty() {
        return message.content.clone();
    }
    message_blocks(message)
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::Text { text } | TranscriptBlock::Thinking { text, .. } => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn truncate_message_for_snip(message: &TranscriptMessage) -> TranscriptMessage {
    let source = snip_source_text(message);
    let preview =
        truncate_compaction_preview(&collapse_compaction_whitespace(&source), SNIP_PREVIEW_CHARS);
    let mut truncated = TranscriptMessage::new(
        message.role.clone(),
        format!("{SNIP_TRUNCATED_PREFIX}{preview}"),
    );
    truncated.id = message.id.clone();
    truncated.created_at = message.created_at;
    truncated
}

/// Truncate standalone oversized text messages in place, inserting a single
/// snip-boundary marker before the first one. The trailing message (the
/// current prompt) and any tool_use/tool_result-bearing message are never
/// snipped, so request pairing and alternation stay intact. Returns the
/// rewritten history plus the ids of the messages that were truncated, or
/// `None` when nothing exceeded the threshold.
pub(crate) fn snip_oversized_messages(
    messages: Vec<TranscriptMessage>,
    per_message_token_threshold: u32,
) -> (Vec<TranscriptMessage>, Vec<String>) {
    if per_message_token_threshold == 0 || messages.len() < 2 {
        return (messages, Vec::new());
    }
    let last_index = messages.len() - 1;
    let mut snipped_ids = Vec::new();
    let mut rewritten = Vec::with_capacity(messages.len() + 1);
    for (index, message) in messages.into_iter().enumerate() {
        let is_candidate = index < last_index
            && !matches!(message.role, MessageRole::System)
            && !message_has_tool_blocks(&message)
            && estimated_message_tokens(&message) >= per_message_token_threshold;
        if is_candidate {
            if snipped_ids.is_empty() {
                rewritten.push(snip_boundary_message());
            }
            snipped_ids.push(message.id.clone());
            rewritten.push(truncate_message_for_snip(&message));
        } else {
            rewritten.push(message);
        }
    }
    (rewritten, snipped_ids)
}

/// Clear bulky tool-result content in messages older than the keep-recent
/// window, preserving each block's `tool_use_id`/`is_error` so pairing holds.
/// Returns the number of tool-result blocks cleared.
pub(crate) fn microcompact_tool_results(
    messages: &mut [TranscriptMessage],
    keep_recent_messages: usize,
    min_result_chars: usize,
) -> usize {
    if messages.len() <= keep_recent_messages {
        return 0;
    }
    let cutoff = messages.len() - keep_recent_messages;
    let mut cleared = 0;
    for message in messages[..cutoff].iter_mut() {
        let mut changed = false;
        for block in message.blocks.iter_mut() {
            let TranscriptBlock::ToolResult {
                content, metadata, ..
            } = block
            else {
                continue;
            };
            if content == MICROCOMPACT_TOOL_RESULT_PLACEHOLDER
                || content.chars().count() < min_result_chars
            {
                continue;
            }
            *content = MICROCOMPACT_TOOL_RESULT_PLACEHOLDER.to_string();
            *metadata = None;
            cleared += 1;
            changed = true;
        }
        if changed {
            message.content = visible_content_from_blocks(&message.blocks);
        }
    }
    cleared
}

pub(crate) fn lightweight_compaction_summary(
    snipped_message_count: usize,
    microcompacted_tool_results: usize,
) -> String {
    let mut parts = Vec::new();
    if snipped_message_count > 0 {
        parts.push(format!(
            "snipped {snipped_message_count} oversized message{}",
            if snipped_message_count == 1 { "" } else { "s" }
        ));
    }
    if microcompacted_tool_results > 0 {
        parts.push(format!(
            "microcompacted {microcompacted_tool_results} tool result{}",
            if microcompacted_tool_results == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    if parts.is_empty() {
        "Reclaimed conversation context.".to_string()
    } else {
        let detail = parts.join(" and ");
        let mut detail_chars = detail.chars();
        let first = detail_chars.next().map(|c| c.to_ascii_uppercase());
        let capitalized = first
            .map(|c| format!("{c}{}", detail_chars.as_str()))
            .unwrap_or(detail);
        format!("Reclaimed conversation context: {capitalized}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modeled_compaction_summary_skips_empty_previews() {
        let summary = modeled_compaction_summary(&[
            TranscriptMessage::new(MessageRole::User, "please inspect the repo"),
            TranscriptMessage::new(MessageRole::User, ""),
            TranscriptMessage::new(MessageRole::Assistant, "I found the TUI entrypoint."),
        ]);

        assert!(summary.contains("Most recent user message: please inspect the repo"));
        assert!(!summary.contains("Most recent user message: \n"));
    }

    #[test]
    fn format_compact_summary_strips_analysis_and_keeps_summary_body() {
        let formatted = format_compact_summary(
            "<analysis>scratch</analysis>\n<summary>\n1. Primary Request and Intent:\n   Build compact.\n</summary>",
        );

        assert_eq!(
            formatted,
            "Summary:\n1. Primary Request and Intent:\n   Build compact."
        );
    }

    #[test]
    fn format_compact_summary_collapses_extra_blank_lines() {
        let formatted = format_compact_summary(
            "<analysis>scratch</analysis>\n\n\n<summary>\nA\n\n\nB\n</summary>\n\n\n",
        );

        assert_eq!(formatted, "Summary:\nA\n\nB");
    }

    #[test]
    fn compaction_prompt_matches_typescript_no_tools_and_summary_shape() {
        let prompt = compaction_prompt();

        assert!(prompt.starts_with("CRITICAL: Respond with TEXT ONLY. Do NOT call any tools."));
        assert!(
            prompt.contains(
                "- Tool calls will be REJECTED and will waste your only turn — you will fail the task."
            ),
            "{prompt}"
        );
        assert!(prompt.contains("Your summary should include the following sections:"));
        assert!(
                prompt.contains(
                    "9. Optional Next Step: List the next step that you will take that is related to the most recent work you were doing."
                ),
                "{prompt}"
            );
        assert!(prompt.contains("Here's an example of how your output should be structured:"));
        assert!(prompt.contains(
            "There may be additional summarization instructions provided in the included context."
        ));
        assert!(prompt.ends_with("Tool calls will be rejected and you will fail the task."));
    }

    #[test]
    fn compact_empty_response_detail_reports_block_shape() {
        let response = ProviderResponse {
            provider: ProviderId::Anthropic,
            fallback_from: None,
            content: String::new(),
            blocks: vec![TranscriptBlock::Thinking {
                text: "reasoning only".to_string(),
                signature: None,
            }],
            stop_reason: Some("max_tokens".to_string()),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 4096,
                total_tokens: 4106,
                ..TokenUsage::default()
            },
            deltas: Vec::new(),
        };

        assert_eq!(
            compact_empty_response_detail(&response),
            "stop_reason=max_tokens, blocks=[thinking], deltas=0, output_tokens=4096"
        );
    }

    #[test]
    fn compact_provider_messages_strip_thinking_blocks() {
        let messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::Thinking {
                    text: "private reasoning".to_string(),
                    signature: None,
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::Thinking {
                        text: "private reasoning".to_string(),
                        signature: None,
                    },
                    TranscriptBlock::Text {
                        text: "visible answer".to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "Read".to_string(),
                    input: r#"{"file_path":"orbcode/tui/src/lib.rs"}"#.to_string(),
                }],
            ),
        ];

        let stripped = compact_provider_messages(&messages);

        assert_eq!(stripped.len(), 2);
        assert!(stripped.iter().all(|message| {
            !message
                .blocks
                .iter()
                .any(|block| matches!(block, TranscriptBlock::Thinking { .. }))
        }));
        assert_eq!(stripped[0].content, "visible answer");
        assert!(matches!(
            stripped[1].blocks.as_slice(),
            [TranscriptBlock::ToolUse { id, .. }] if id == "tool-1"
        ));
    }

    fn tool_result_message(tool_use_id: &str, content: &str) -> TranscriptMessage {
        TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error: false,
                metadata: Some("{\"status\":\"completed\"}".to_string()),
            }],
        )
    }

    #[test]
    fn snip_truncates_oversized_standalone_message_and_inserts_boundary() {
        let huge = "x".repeat(4_000);
        let messages = vec![
            TranscriptMessage::new(MessageRole::User, huge.clone()),
            TranscriptMessage::new(MessageRole::Assistant, "short answer"),
            TranscriptMessage::new(MessageRole::User, "current prompt"),
        ];

        let original_id = messages[0].id.clone();
        let (rewritten, snipped) = snip_oversized_messages(messages, 100);

        assert_eq!(snipped.len(), 1);
        assert_eq!(snipped[0], original_id);
        // boundary inserted ahead of the truncated message
        assert_eq!(rewritten.len(), 4);
        assert_eq!(rewritten[0].role, MessageRole::System);
        assert_eq!(rewritten[0].content, SNIP_BOUNDARY_TEXT);
        assert!(rewritten[1].content.starts_with(SNIP_TRUNCATED_PREFIX));
        assert!(!rewritten[1].content.contains(&huge));
        // remaining messages untouched
        assert_eq!(rewritten[2].content, "short answer");
        assert_eq!(rewritten[3].content, "current prompt");
    }

    #[test]
    fn snip_preserves_preview_for_block_only_thinking_message() {
        // A thinking-only message has empty `content` but is oversized by its
        // blocks. Truncating from `content` would leave only the snip prefix;
        // the preview must come from the block text instead.
        let huge_thought = "reasoning ".repeat(500);
        let thinking = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::Thinking {
                text: huge_thought.clone(),
                signature: None,
            }],
        );
        assert!(
            thinking.content.trim().is_empty(),
            "content should be empty"
        );
        let truncated = truncate_message_for_snip(&thinking);
        assert!(truncated.content.starts_with(SNIP_TRUNCATED_PREFIX));
        assert!(
            truncated.content.len() > SNIP_TRUNCATED_PREFIX.len() + 4,
            "block-only message must keep a text preview, got {:?}",
            truncated.content
        );
        assert!(truncated.content.contains("reasoning"));
    }

    #[test]
    fn snip_never_touches_trailing_prompt_or_tool_messages() {
        let huge = "y".repeat(4_000);
        let messages = vec![
            tool_result_message("tool-1", &huge),
            TranscriptMessage::new(MessageRole::User, huge.clone()),
        ];

        // The oversized tool message is skipped; the last message is the prompt
        // and is never snipped, so nothing qualifies.
        let (_, snipped) = snip_oversized_messages(messages, 100);
        assert!(snipped.is_empty());
    }

    #[test]
    fn microcompact_clears_old_tool_results_but_keeps_recent() {
        let big = "z".repeat(1_000);
        let mut messages = vec![
            tool_result_message("tool-old", &big),
            TranscriptMessage::new(MessageRole::Assistant, "older answer"),
            tool_result_message("tool-recent", &big),
            TranscriptMessage::new(MessageRole::User, "current prompt"),
        ];

        let cleared = microcompact_tool_results(&mut messages, 2, MICROCOMPACT_MIN_RESULT_CHARS);

        assert_eq!(cleared, 1);
        assert!(matches!(
            messages[0].blocks.as_slice(),
            [TranscriptBlock::ToolResult { content, .. }]
                if content == MICROCOMPACT_TOOL_RESULT_PLACEHOLDER
        ));
        // recent tool result preserved
        assert!(matches!(
            messages[2].blocks.as_slice(),
            [TranscriptBlock::ToolResult { content, .. }] if content == &big
        ));
    }

    #[test]
    fn microcompact_skips_small_results_and_is_idempotent() {
        let mut messages = vec![
            tool_result_message("tool-small", "ok"),
            tool_result_message("tool-big", &"q".repeat(1_000)),
            TranscriptMessage::new(MessageRole::User, "prompt"),
        ];

        let first = microcompact_tool_results(&mut messages, 0, MICROCOMPACT_MIN_RESULT_CHARS);
        assert_eq!(first, 1);
        let second = microcompact_tool_results(&mut messages, 0, MICROCOMPACT_MIN_RESULT_CHARS);
        assert_eq!(second, 0, "already-cleared results are not cleared again");
    }

    #[test]
    fn lightweight_summary_describes_applied_passes() {
        assert_eq!(
            lightweight_compaction_summary(1, 0),
            "Reclaimed conversation context: Snipped 1 oversized message."
        );
        assert_eq!(
            lightweight_compaction_summary(0, 3),
            "Reclaimed conversation context: Microcompacted 3 tool results."
        );
        assert_eq!(
            lightweight_compaction_summary(2, 1),
            "Reclaimed conversation context: Snipped 2 oversized messages and microcompacted 1 tool result."
        );
    }
}
