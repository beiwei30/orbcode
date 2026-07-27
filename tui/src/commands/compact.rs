use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use orbcode_app_server_client::{AppClient, CompactSessionResult};
use orbcode_protocol::{TranscriptBlock, TranscriptMessage};
use tokio::sync::mpsc;

use crate::commands::async_local::{LocalCommandEnvelope, LocalCommandEvent};
use crate::history_cell::local_note::{
    LocalTranscriptNote, local_context_compacted_message, local_slash_command_output_message,
    parse_local_transcript_note,
};
use crate::history_cell::state::TranscriptUiState;
use crate::slash_commands::SlashCommandDeferredFeedback;
use crate::state::TuiState;
use crate::tool_cell::summary::is_file_read_like_tool;
use crate::tool_cell::utils::{display_tool_path, first_string_field, parse_tool_input};

#[derive(Clone, Debug)]
struct CompactRestoredFile {
    display_path: String,
    line_count: Option<usize>,
    sequence: usize,
}

impl TuiState {
    pub(crate) fn start_compact_slash_command(
        &mut self,
        line: &str,
        force: bool,
        _app_server: &AppClient,
        local_command_tx: &mpsc::UnboundedSender<LocalCommandEnvelope>,
    ) {
        if self
            .messages
            .iter()
            .all(|message| parse_local_transcript_note(message).is_some())
        {
            self.push_local_slash_command_output(
                line,
                "Nothing to compact.",
                Some("No model-visible conversation history has been recorded yet.".to_string()),
            );
            self.set_status_line("Nothing to compact.");
            return;
        }

        let command = line.to_string();
        let client = self.app_client();
        let session_id = self.session_id.clone();
        let local_command_tx = local_command_tx.clone();
        self.compact_started_at = Some(Instant::now());
        self.spinner_frame = 0;
        self.request_count = self.request_count.saturating_add(1);
        self.set_status_line("Compacting conversation...");
        // Detached compact command; completion is routed back through
        // LocalCommandEvent.
        let _compact_command_handle = tokio::spawn(async move {
            if !force
                && let Ok(value) = client.compact_decision(&session_id).await
                && let Some(needs) = value.get("NeedsConfirmation")
            {
                let context_percent_used =
                    needs["context_percent_used"].as_u64().unwrap_or(0) as u32;
                let threshold_percent = needs["threshold_percent"].as_u64().unwrap_or(0) as u32;
                let _ = local_command_tx.send(LocalCommandEnvelope::new(
                    session_id.clone(),
                    LocalCommandEvent::CompactNeedsConfirmation {
                        command,
                        context_percent_used,
                        threshold_percent,
                    },
                ));
                return;
            }
            let result = client
                .compact_session(&session_id)
                .await
                .map_err(|error| error.to_string())
                .and_then(parse_compact_session_result);
            let _ = local_command_tx.send(LocalCommandEnvelope::new(
                session_id,
                LocalCommandEvent::CompactFinished { command, result },
            ));
        });
    }

    pub(crate) fn apply_compact_result(&mut self, line: &str, result: CompactSessionResult) {
        let duration_ms = self
            .compact_started_at
            .take()
            .map(|started_at| started_at.elapsed().as_millis().min(u64::MAX as u128) as u64);
        self.remove_pending_compact_output();
        let restored_file_lines = compact_restored_file_detail_lines(&self.messages, &self.cwd);
        let removed = result
            .original_message_count
            .saturating_sub(result.compacted_message_count);
        let summary = if removed == 0 {
            "Conversation already compact.".to_string()
        } else if result.provider_generated {
            "Compacted (ctrl+o to see full summary)".to_string()
        } else {
            "Compacted with local fallback (ctrl+o to see full summary)".to_string()
        };
        let full_summary = result
            .session
            .messages
            .first()
            .map(|message| message.content.trim().to_string())
            .filter(|content| !content.is_empty());
        let mut detail = String::new();
        if !result.provider_generated {
            detail.push_str("Summary source: local fallback.");
        }
        if let Some(reason) = result.fallback_reason.as_deref() {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str("Fallback reason: ");
            detail.push_str(reason);
        }
        for line in restored_file_lines {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str(&line);
        }
        let detail = (!detail.is_empty()).then_some(detail);
        self.messages
            .push(local_context_compacted_message(duration_ms, full_summary));
        self.messages.push(local_slash_command_output_message(
            line.to_string(),
            summary.clone(),
            detail,
            SlashCommandDeferredFeedback::Quoted,
        ));
        self.transcript_ui = TranscriptUiState::from_messages(&self.messages, &self.cwd);
        self.pending_assistant.clear();
        self.history_flushed_message_count = 0;
        self.pending_history_flush = false;
        self.clear_live_tool_activities();
        self.set_status_line(if removed == 0 {
            summary
        } else {
            "Conversation compacted (ctrl+o for history).".to_string()
        });
    }

    pub(crate) fn remove_pending_compact_output(&mut self) {
        let Some(index) = self.messages.iter().rposition(|message| {
            matches!(
                parse_local_transcript_note(message),
                Some(LocalTranscriptNote::SlashCommandOutput { command, summary, .. })
                    if command == "/compact" && summary == "Compacting conversation..."
            )
        }) else {
            return;
        };
        self.messages.remove(index);
    }
}

fn parse_compact_session_result(value: serde_json::Value) -> Result<CompactSessionResult, String> {
    use orbcode_protocol::{SessionRecord, TokenUsage};

    let session: SessionRecord = serde_json::from_value(value["session"].clone())
        .map_err(|e| format!("protocol deserialization error: {e}"))?;
    let usage: Option<TokenUsage> = value
        .get("usage")
        .filter(|v| !v.is_null())
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(|e| format!("protocol deserialization error: {e}"))?;
    Ok(CompactSessionResult {
        session,
        original_message_count: value["original_message_count"].as_u64().unwrap_or(0) as usize,
        compacted_message_count: value["compacted_message_count"].as_u64().unwrap_or(0) as usize,
        provider_generated: value["provider_generated"].as_bool().unwrap_or(false),
        fallback_reason: value["fallback_reason"].as_str().map(String::from),
        usage,
    })
}

pub(crate) fn compact_restored_file_detail_lines(
    messages: &[TranscriptMessage],
    cwd: &Path,
) -> Vec<String> {
    const MAX_POST_COMPACT_FILES: usize = 5;

    let mut read_paths_by_id = HashMap::new();
    let mut files_by_path: HashMap<String, CompactRestoredFile> = HashMap::new();
    let mut sequence = 0;

    for message in messages {
        for block in &message.blocks {
            sequence += 1;
            match block {
                TranscriptBlock::ToolUse { id, name, input } if is_file_read_like_tool(name) => {
                    let parsed_input = parse_tool_input(input);
                    if let Some(file_path) = first_string_field(
                        parsed_input.as_ref(),
                        &["file_path", "filePath", "path"],
                    ) {
                        let display_path = display_tool_path(&file_path, cwd);
                        read_paths_by_id.insert(id.clone(), display_path.clone());
                        files_by_path
                            .entry(display_path.clone())
                            .and_modify(|file| file.sequence = sequence)
                            .or_insert(CompactRestoredFile {
                                display_path,
                                line_count: None,
                                sequence,
                            });
                    }
                }
                TranscriptBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    ..
                } if !is_error => {
                    if let Some(display_path) = read_paths_by_id.get(tool_use_id) {
                        let line_count = content.lines().count();
                        files_by_path
                            .entry(display_path.clone())
                            .and_modify(|file| {
                                file.line_count = Some(line_count);
                                file.sequence = sequence;
                            })
                            .or_insert(CompactRestoredFile {
                                display_path: display_path.clone(),
                                line_count: Some(line_count),
                                sequence,
                            });
                    }
                }
                _ => {}
            }
        }
    }

    let mut files = files_by_path.into_values().collect::<Vec<_>>();
    files.sort_by_key(|file| std::cmp::Reverse(file.sequence));
    files
        .into_iter()
        .take(MAX_POST_COMPACT_FILES)
        .map(|file| {
            if let Some(line_count) = file.line_count {
                format!("Read {} ({} lines)", file.display_path, line_count)
            } else {
                format!("Referenced file {}", file.display_path)
            }
        })
        .collect()
}
