use agent_client_protocol::schema::{ContentBlock, ContentChunk, SessionUpdate};
use orbcode_protocol::{MessageRole, SessionRecord, TranscriptBlock, TranscriptMessage};

use super::tool_updates::{tool_call_replay_started, tool_result_replay_update};

pub(super) fn replay_updates_for_session(session: &SessionRecord) -> Vec<SessionUpdate> {
    session
        .messages
        .iter()
        .flat_map(replay_updates_for_message)
        .collect()
}

fn replay_updates_for_message(message: &TranscriptMessage) -> Vec<SessionUpdate> {
    if message.blocks.is_empty() {
        return replay_text_update(&message.role, &message.content)
            .into_iter()
            .collect();
    }

    message
        .blocks
        .iter()
        .filter_map(|block| replay_update_for_block(&message.role, block))
        .collect()
}

fn replay_text_update(role: &MessageRole, text: &str) -> Option<SessionUpdate> {
    if text.is_empty() {
        return None;
    }
    let chunk = ContentChunk::new(ContentBlock::from(text.to_string()));
    match role {
        MessageRole::User => Some(SessionUpdate::UserMessageChunk(chunk)),
        MessageRole::Assistant => Some(SessionUpdate::AgentMessageChunk(chunk)),
        _ => None,
    }
}

fn replay_update_for_block(role: &MessageRole, block: &TranscriptBlock) -> Option<SessionUpdate> {
    match (role, block) {
        (_, TranscriptBlock::Text { text }) => replay_text_update(role, text),
        (MessageRole::Assistant, TranscriptBlock::Thinking { text, .. }) if !text.is_empty() => {
            Some(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                ContentBlock::from(text.clone()),
            )))
        }
        (MessageRole::Assistant, TranscriptBlock::ToolUse { id, name, input }) => Some(
            SessionUpdate::ToolCall(tool_call_replay_started(id, name, input)),
        ),
        (
            MessageRole::User,
            TranscriptBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            },
        ) => Some(SessionUpdate::ToolCallUpdate(tool_result_replay_update(
            tool_use_id,
            content,
            *is_error,
        ))),
        _ => None,
    }
}
