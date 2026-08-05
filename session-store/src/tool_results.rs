use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use orbcode_protocol::{MessageRole, TranscriptBlock, TranscriptMessage};
use tokio::io::AsyncWriteExt;

use crate::SessionStoreError;

pub const DEFAULT_MAX_TOOL_RESULT_SIZE_CHARS: usize = 50_000;
pub const MAX_TOOL_RESULTS_PER_MESSAGE_CHARS: usize = 200_000;
pub const PERSISTED_OUTPUT_TAG: &str = "<persisted-output>";
pub const PERSISTED_OUTPUT_CLOSING_TAG: &str = "</persisted-output>";

const BASH_TRANSCRIPT_TRUNCATION_MARKER: &str = "Bash output truncated for transcript safety.";
const TOOL_RESULT_PREVIEW_SIZE_CHARS: usize = 2_000;

#[derive(Clone)]
pub struct ToolResultStore {
    current_project_dir: PathBuf,
}

#[derive(Clone, Debug)]
struct ToolResultBudgetCandidate {
    message_index: usize,
    block_index: usize,
    tool_use_id: String,
    content: String,
    size: usize,
}

pub fn tool_result_persistence_threshold(tool_name: &str) -> Option<usize> {
    let normalized = tool_name.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "read" | "file-read" | "fileread") {
        None
    } else {
        Some(DEFAULT_MAX_TOOL_RESULT_SIZE_CHARS)
    }
}

pub fn persisted_tool_result_preview_message(content: &str, path_display: &str) -> String {
    let (preview, has_more) = tool_result_preview(content, TOOL_RESULT_PREVIEW_SIZE_CHARS);
    let retained_truncation_note = trailing_bash_truncation_note(content)
        .filter(|note| !preview.contains(*note))
        .map(str::to_string);
    let mut message = format!(
        "{PERSISTED_OUTPUT_TAG}\nOutput too large ({}). Full output saved to: {path_display}\n\nPreview (first {}):\n{}",
        format_tool_result_size(content.chars().count()),
        format_tool_result_size(TOOL_RESULT_PREVIEW_SIZE_CHARS),
        preview
    );
    if has_more {
        message.push_str("\n...\n");
    } else {
        message.push('\n');
    }
    if let Some(note) = retained_truncation_note {
        message.push_str(&note);
        message.push('\n');
    }
    message.push_str(PERSISTED_OUTPUT_CLOSING_TAG);
    message
}

pub fn format_tool_result_size(chars: usize) -> String {
    if chars < 1024 {
        format!("{chars} B")
    } else if chars < 1024 * 1024 {
        format!("{:.1} KB", chars as f64 / 1024.0)
    } else {
        format!("{:.1} MB", chars as f64 / (1024.0 * 1024.0))
    }
}

fn tool_result_preview(content: &str, max_chars: usize) -> (String, bool) {
    let content_len = content.chars().count();
    if content_len <= max_chars {
        return (content.to_string(), false);
    }

    let truncated = content.chars().take(max_chars).collect::<String>();
    let last_newline = truncated.rfind('\n');
    let cut_point = last_newline
        .filter(|index| *index > max_chars / 2)
        .unwrap_or(truncated.len());
    (truncated[..cut_point].to_string(), true)
}

fn trailing_bash_truncation_note(content: &str) -> Option<&str> {
    content
        .lines()
        .rev()
        .find(|line| line.starts_with('[') && line.contains(BASH_TRANSCRIPT_TRUNCATION_MARKER))
}

impl ToolResultStore {
    pub fn new(current_project_dir: PathBuf) -> Self {
        Self {
            current_project_dir,
        }
    }

    pub async fn persist(
        &self,
        session_id: &str,
        tool_use_id: &str,
        content: &str,
    ) -> Result<String, SessionStoreError> {
        let dir = self
            .current_project_dir
            .join(session_id)
            .join("tool-results");
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join(format!("{tool_use_id}.txt"));
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                file.write_all(content.as_bytes()).await?;
                file.flush().await?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }

        Ok(path.display().to_string())
    }

    pub async fn apply_budget_replacements(
        &self,
        session_id: &str,
        messages: &mut [TranscriptMessage],
    ) -> Result<(), SessionStoreError> {
        let tool_names = tool_names_by_use_id(messages);
        let groups = collect_tool_result_budget_groups(messages, &tool_names);
        let mut replacements = HashMap::new();

        for candidates in groups {
            let total_size = candidates
                .iter()
                .map(|candidate| candidate.size)
                .sum::<usize>();
            if total_size <= MAX_TOOL_RESULTS_PER_MESSAGE_CHARS {
                continue;
            }

            let mut remaining = total_size;
            let mut sorted = candidates;
            sorted.sort_by(|left, right| {
                right
                    .size
                    .cmp(&left.size)
                    .then_with(|| left.message_index.cmp(&right.message_index))
                    .then_with(|| left.block_index.cmp(&right.block_index))
            });
            for candidate in sorted {
                if remaining <= MAX_TOOL_RESULTS_PER_MESSAGE_CHARS {
                    break;
                }
                remaining = remaining.saturating_sub(candidate.size);
                let path_display = self
                    .persist(session_id, &candidate.tool_use_id, &candidate.content)
                    .await?;
                replacements.insert(
                    candidate.tool_use_id,
                    persisted_tool_result_preview_message(&candidate.content, &path_display),
                );
            }
        }

        if replacements.is_empty() {
            return Ok(());
        }

        for message in messages {
            for block in &mut message.blocks {
                let TranscriptBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                else {
                    continue;
                };
                if let Some(replacement) = replacements.get(tool_use_id) {
                    content.replace_text(replacement.clone());
                }
            }
        }

        Ok(())
    }
}

fn tool_names_by_use_id(messages: &[TranscriptMessage]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for message in messages {
        if !matches!(message.role, MessageRole::Assistant) {
            continue;
        }
        for block in &message.blocks {
            if let TranscriptBlock::ToolUse { id, name, .. } = block {
                names.insert(id.clone(), name.clone());
            }
        }
    }
    names
}

fn collect_tool_result_budget_groups(
    messages: &[TranscriptMessage],
    tool_names: &HashMap<String, String>,
) -> Vec<Vec<ToolResultBudgetCandidate>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    let mut seen_assistant_ids = HashSet::new();

    for (message_index, message) in messages.iter().enumerate() {
        match message.role {
            MessageRole::User => {
                for (block_index, block) in message.blocks.iter().enumerate() {
                    let TranscriptBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = block
                    else {
                        continue;
                    };
                    let persisted_content = content.provider_string();
                    if persisted_content.trim().is_empty()
                        || persisted_content.starts_with(PERSISTED_OUTPUT_TAG)
                    {
                        continue;
                    }
                    if tool_names
                        .get(tool_use_id)
                        .is_some_and(|name| tool_result_persistence_threshold(name).is_none())
                    {
                        continue;
                    }
                    let size = persisted_content.chars().count();
                    current.push(ToolResultBudgetCandidate {
                        message_index,
                        block_index,
                        tool_use_id: tool_use_id.clone(),
                        content: persisted_content,
                        size,
                    });
                }
            }
            MessageRole::Assistant
                if seen_assistant_ids.insert(message.id.clone()) && !current.is_empty() =>
            {
                groups.push(std::mem::take(&mut current));
            }
            _ => {}
        }
    }

    if !current.is_empty() {
        groups.push(current);
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_protocol::{
        MessageRole, ToolResultContent, TranscriptBlock, TranscriptJsonField, TranscriptMessage,
    };

    #[test]
    fn persisted_tool_result_preview_message_retains_trailing_bash_truncation_note() {
        let bash_note = "[Bash output truncated for transcript safety. Re-run with a narrower command if you need the omitted portion. Omitted 70000 characters.]";
        let content = format!("{}\n\n{bash_note}", "line\n".repeat(12_000));

        let message = persisted_tool_result_preview_message(&content, "/tmp/tool-result.txt");

        assert!(message.starts_with(PERSISTED_OUTPUT_TAG));
        assert!(message.contains("Full output saved to: /tmp/tool-result.txt"));
        assert!(message.contains("Preview (first 2.0 KB):"));
        assert!(message.contains(bash_note), "{message}");
        assert_eq!(message.matches(bash_note).count(), 1, "{message}");
        assert!(message.ends_with(PERSISTED_OUTPUT_CLOSING_TAG));
    }

    #[test]
    fn tool_result_persistence_threshold_skips_file_reads() {
        assert_eq!(tool_result_persistence_threshold("Read"), None);
        assert_eq!(tool_result_persistence_threshold("file-read"), None);
        assert_eq!(
            tool_result_persistence_threshold("bash"),
            Some(DEFAULT_MAX_TOOL_RESULT_SIZE_CHARS)
        );
    }

    #[tokio::test]
    async fn persist_writes_tool_result_once_and_keeps_existing_file() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = ToolResultStore::new(temp.path().to_path_buf());

        let path = store
            .persist("session-1", "tool-1", "first")
            .await
            .expect("persist first result");
        store
            .persist("session-1", "tool-1", "second")
            .await
            .expect("existing result is accepted");

        assert!(path.ends_with("session-1/tool-results/tool-1.txt"));
        assert_eq!(
            tokio::fs::read_to_string(path)
                .await
                .expect("read persisted result"),
            "first"
        );
    }

    #[tokio::test]
    async fn apply_budget_replacements_persists_largest_group_members() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = ToolResultStore::new(temp.path().to_path_buf());
        let session_id = "aggregate-tool-result-session";
        let assistant = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            (0..5)
                .map(|index| TranscriptBlock::ToolUse {
                    id: format!("tool-{index}"),
                    name: "web-fetch".to_string(),
                    input: "{}".to_string(),
                })
                .collect(),
        );
        let mut messages = vec![assistant];
        for index in 0..5 {
            messages.push(TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: format!("tool-{index}"),
                    content: "x".repeat(45_000).into(),
                    is_error: false,
                    metadata: None,
                }],
            ));
        }

        store
            .apply_budget_replacements(session_id, &mut messages)
            .await
            .expect("apply budget");
        let replaced = messages
            .iter()
            .flat_map(|message| &message.blocks)
            .filter(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult { content, .. }
                        if content.starts_with(PERSISTED_OUTPUT_TAG)
                )
            })
            .count();

        assert_eq!(replaced, 1);
        assert!(
            temp.path()
                .join(session_id)
                .join("tool-results")
                .join("tool-0.txt")
                .exists()
        );
    }

    #[tokio::test]
    async fn apply_budget_replacements_preserves_loaded_structured_content() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = ToolResultStore::new(temp.path().to_path_buf());
        let session_id = "structured-tool-result-session";
        let original = serde_json::json!([
            {"type": "text", "text": "visible text"},
            {
                "type": "structured",
                "payload": "x".repeat(MAX_TOOL_RESULTS_PER_MESSAGE_CHARS)
            }
        ]);
        let mut messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "tool-structured".to_string(),
                    name: "web-fetch".to_string(),
                    input: "{}".to_string(),
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-structured".to_string(),
                    content: ToolResultContent::from_loaded(TranscriptJsonField::Value(
                        original.clone(),
                    )),
                    is_error: false,
                    metadata: None,
                }],
            ),
        ];

        store
            .apply_budget_replacements(session_id, &mut messages)
            .await
            .expect("apply budget");

        let persisted = tokio::fs::read_to_string(
            temp.path()
                .join(session_id)
                .join("tool-results")
                .join("tool-structured.txt"),
        )
        .await
        .expect("read persisted structured result");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&persisted)
                .expect("parse persisted structured result"),
            original
        );

        let [TranscriptBlock::ToolResult { content, .. }] = messages[1].blocks.as_slice() else {
            panic!("expected one tool result");
        };
        assert!(content.starts_with(PERSISTED_OUTPUT_TAG));
        assert!(content.loaded_field().is_none());
    }

    #[test]
    fn regression_budget_group_boundaries_align_with_assistant_messages() {
        let assistant_1 = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            (0..3)
                .map(|i| TranscriptBlock::ToolUse {
                    id: format!("tool-a-{i}"),
                    name: "Bash".to_string(),
                    input: "{}".to_string(),
                })
                .collect(),
        );
        let results_1: Vec<TranscriptMessage> = (0..3)
            .map(|i| {
                TranscriptMessage::from_blocks(
                    MessageRole::User,
                    vec![TranscriptBlock::ToolResult {
                        tool_use_id: format!("tool-a-{i}"),
                        content: "r".repeat(10_000).into(),
                        is_error: false,
                        metadata: None,
                    }],
                )
            })
            .collect();

        let assistant_2 = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            (0..4)
                .map(|i| TranscriptBlock::ToolUse {
                    id: format!("tool-b-{i}"),
                    name: "Bash".to_string(),
                    input: "{}".to_string(),
                })
                .collect(),
        );
        let results_2: Vec<TranscriptMessage> = (0..4)
            .map(|i| {
                TranscriptMessage::from_blocks(
                    MessageRole::User,
                    vec![TranscriptBlock::ToolResult {
                        tool_use_id: format!("tool-b-{i}"),
                        content: "s".repeat(10_000).into(),
                        is_error: false,
                        metadata: None,
                    }],
                )
            })
            .collect();

        let mut messages = vec![assistant_1];
        messages.extend(results_1);
        messages.push(assistant_2);
        messages.extend(results_2);

        let tool_names = tool_names_by_use_id(&messages);
        let groups = collect_tool_result_budget_groups(&messages, &tool_names);

        assert_eq!(groups.len(), 2, "should produce exactly 2 groups");
        assert_eq!(groups[0].len(), 3, "first group should have 3 candidates");
        assert_eq!(groups[1].len(), 4, "second group should have 4 candidates");
    }

    #[tokio::test]
    async fn regression_budget_replacement_count_is_minimal() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = ToolResultStore::new(temp.path().to_path_buf());
        let session_id = "budget-minimal-session";

        let assistant = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            (0..5)
                .map(|i| TranscriptBlock::ToolUse {
                    id: format!("tool-{i}"),
                    name: "web-fetch".to_string(),
                    input: "{}".to_string(),
                })
                .collect(),
        );
        let mut messages = vec![assistant];
        for i in 0..5 {
            messages.push(TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: format!("tool-{i}"),
                    content: "x".repeat(45_000).into(),
                    is_error: false,
                    metadata: None,
                }],
            ));
        }

        store
            .apply_budget_replacements(session_id, &mut messages)
            .await
            .expect("apply budget");

        let replaced = messages
            .iter()
            .flat_map(|m| &m.blocks)
            .filter(|b| {
                matches!(b, TranscriptBlock::ToolResult { content, .. }
                    if content.starts_with(PERSISTED_OUTPUT_TAG))
            })
            .count();
        let unreplaced = messages
            .iter()
            .flat_map(|m| &m.blocks)
            .filter(|b| {
                matches!(b, TranscriptBlock::ToolResult { content, .. }
                    if !content.starts_with(PERSISTED_OUTPUT_TAG))
            })
            .count();

        assert_eq!(
            replaced, 1,
            "only the minimum number of results should be replaced"
        );
        assert_eq!(unreplaced, 4, "remaining results should stay intact");
    }

    #[test]
    fn regression_budget_preview_message_length_bounded() {
        let content = "y".repeat(100_000);
        let preview = persisted_tool_result_preview_message(&content, "/tmp/result.txt");

        assert!(
            preview.starts_with(PERSISTED_OUTPUT_TAG),
            "preview should start with persisted output tag"
        );
        assert!(
            preview.ends_with(PERSISTED_OUTPUT_CLOSING_TAG),
            "preview should end with closing tag"
        );
        assert!(
            preview.len() < content.len() / 10,
            "preview ({} bytes) should be much smaller than input ({} bytes)",
            preview.len(),
            content.len()
        );
    }

    #[test]
    fn regression_budget_format_size_human_readable() {
        assert_eq!(format_tool_result_size(1), "1 B");
        assert_eq!(format_tool_result_size(999), "999 B");
        assert_eq!(format_tool_result_size(1024), "1.0 KB");
        assert_eq!(format_tool_result_size(50_000), "48.8 KB");
        assert_eq!(format_tool_result_size(1_100_000), "1.0 MB");
    }

    #[tokio::test]
    #[ignore = "manual stress test for tool result budget replacement at scale"]
    async fn tool_result_budget_stress_replaces_many_large_results() {
        use std::time::Instant;

        const TOOL_COUNT: usize = 50;
        const RESULT_CHARS: usize = 60_000;

        let temp = tempfile::tempdir().expect("create temp dir");
        let store = ToolResultStore::new(temp.path().to_path_buf());
        let session_id = "budget-stress-session";

        let assistant = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            (0..TOOL_COUNT)
                .map(|i| TranscriptBlock::ToolUse {
                    id: format!("tool-{i}"),
                    name: "web-fetch".to_string(),
                    input: "{}".to_string(),
                })
                .collect(),
        );
        let mut messages = vec![assistant];
        for i in 0..TOOL_COUNT {
            messages.push(TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: format!("tool-{i}"),
                    content: "z".repeat(RESULT_CHARS).into(),
                    is_error: false,
                    metadata: None,
                }],
            ));
        }

        let started = Instant::now();
        store
            .apply_budget_replacements(session_id, &mut messages)
            .await
            .expect("apply budget");
        let duration = started.elapsed();

        let replaced = messages
            .iter()
            .flat_map(|m| &m.blocks)
            .filter(|b| {
                matches!(b, TranscriptBlock::ToolResult { content, .. }
                    if content.starts_with(PERSISTED_OUTPUT_TAG))
            })
            .count();

        eprintln!(
            "tool_count={TOOL_COUNT} result_chars={RESULT_CHARS} \
             replaced={replaced} persist_us={}",
            duration.as_micros()
        );
    }

    #[tokio::test]
    async fn apply_budget_replacements_skips_read_results() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = ToolResultStore::new(temp.path().to_path_buf());
        let session_id = "aggregate-read-skip-session";
        let mut messages = vec![
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::ToolUse {
                        id: "read-tool".to_string(),
                        name: "Read".to_string(),
                        input: "{}".to_string(),
                    },
                    TranscriptBlock::ToolUse {
                        id: "bash-tool".to_string(),
                        name: "bash".to_string(),
                        input: "{}".to_string(),
                    },
                ],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "read-tool".to_string(),
                    content: "r".repeat(190_000).into(),
                    is_error: false,
                    metadata: None,
                }],
            ),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "bash-tool".to_string(),
                    content: "b".repeat(20_000).into(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        ];

        store
            .apply_budget_replacements(session_id, &mut messages)
            .await
            .expect("apply budget");
        let replaced = messages
            .iter()
            .flat_map(|message| &message.blocks)
            .any(|block| {
                matches!(
                    block,
                    TranscriptBlock::ToolResult { content, .. }
                        if content.starts_with(PERSISTED_OUTPUT_TAG)
                )
            });

        assert!(!replaced);
        assert!(!temp.path().join(session_id).join("tool-results").exists());
    }
}
