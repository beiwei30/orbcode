use orbcode_protocol::{ProviderId, TokenUsage, TranscriptBlock, visible_content_from_blocks};

use crate::{
    ProviderContentBlockDelta, ProviderContentBlockStart, ProviderResponse, ProviderStreamEvent,
    merge_usage,
};

#[derive(Clone, Debug)]
pub struct ProviderStreamAccumulator {
    provider: ProviderId,
    fallback_from: Option<ProviderId>,
    content: String,
    blocks: Vec<TranscriptBlock>,
    stop_reason: Option<String>,
    usage: TokenUsage,
    deltas: Vec<String>,
}

impl ProviderStreamAccumulator {
    pub fn new(provider: ProviderId, fallback_from: Option<ProviderId>) -> Self {
        Self {
            provider,
            fallback_from,
            content: String::new(),
            blocks: Vec::new(),
            stop_reason: None,
            usage: TokenUsage::default(),
            deltas: Vec::new(),
        }
    }

    pub fn from_parts(
        provider: ProviderId,
        fallback_from: Option<ProviderId>,
        content: String,
        blocks: Vec<TranscriptBlock>,
        stop_reason: Option<String>,
        usage: TokenUsage,
        deltas: Vec<String>,
    ) -> Self {
        Self {
            provider,
            fallback_from,
            content,
            blocks,
            stop_reason,
            usage,
            deltas,
        }
    }

    pub fn apply(&mut self, event: &ProviderStreamEvent) {
        match event {
            ProviderStreamEvent::MessageStart {
                provider,
                fallback_from,
                usage,
            } => {
                self.provider = *provider;
                self.fallback_from = *fallback_from;
                merge_usage(&mut self.usage, usage);
            }
            ProviderStreamEvent::ContentBlockStart { index, block } => match block {
                ProviderContentBlockStart::Text { text } => {
                    self.content.push_str(text);
                    if !text.is_empty() {
                        self.deltas.push(text.clone());
                    }
                    set_block_at(
                        &mut self.blocks,
                        *index,
                        TranscriptBlock::Text { text: text.clone() },
                    );
                }
                ProviderContentBlockStart::Thinking { text, signature } => {
                    set_block_at(
                        &mut self.blocks,
                        *index,
                        TranscriptBlock::Thinking {
                            text: text.clone(),
                            signature: signature.clone(),
                        },
                    );
                }
                ProviderContentBlockStart::ToolUse { id, name, input } => {
                    set_block_at(
                        &mut self.blocks,
                        *index,
                        TranscriptBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        },
                    );
                }
            },
            ProviderStreamEvent::ContentBlockDelta { index, delta } => match delta {
                ProviderContentBlockDelta::Text(text) => {
                    self.content.push_str(text);
                    if !text.is_empty() {
                        self.deltas.push(text.clone());
                    }
                    if let Some(TranscriptBlock::Text { text: block_text }) =
                        self.blocks.get_mut(*index)
                    {
                        block_text.push_str(text);
                    }
                }
                ProviderContentBlockDelta::Thinking(text) => {
                    if let Some(TranscriptBlock::Thinking {
                        text: block_text, ..
                    }) = self.blocks.get_mut(*index)
                    {
                        block_text.push_str(text);
                    }
                }
                ProviderContentBlockDelta::Signature(signature) => {
                    if let Some(TranscriptBlock::Thinking {
                        signature: block_signature,
                        ..
                    }) = self.blocks.get_mut(*index)
                    {
                        *block_signature = Some(signature.clone());
                    }
                }
                ProviderContentBlockDelta::InputJson(partial) => {
                    if let Some(TranscriptBlock::ToolUse { input, .. }) =
                        self.blocks.get_mut(*index)
                    {
                        append_tool_json_delta(input, partial);
                    }
                }
            },
            ProviderStreamEvent::ContentBlockStop { .. } => {}
            ProviderStreamEvent::MessageDelta { stop_reason, usage } => {
                if let Some(stop_reason) = stop_reason {
                    self.stop_reason = Some(stop_reason.clone());
                }
                merge_usage(&mut self.usage, usage);
            }
            ProviderStreamEvent::MessageStop => {}
        }
    }

    pub fn block(&self, index: usize) -> Option<&TranscriptBlock> {
        self.blocks.get(index)
    }

    #[cfg(test)]
    pub(crate) fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn usage(&self) -> TokenUsage {
        self.usage.clone()
    }

    pub fn stop_reason(&self) -> Option<&str> {
        self.stop_reason.as_deref()
    }

    pub fn into_parts(
        self,
    ) -> (
        String,
        Vec<TranscriptBlock>,
        Option<String>,
        TokenUsage,
        Vec<String>,
    ) {
        (
            self.content,
            self.blocks,
            self.stop_reason,
            self.usage,
            self.deltas,
        )
    }

    pub fn into_response(mut self) -> ProviderResponse {
        if !self.blocks.is_empty() {
            self.content = render_blocks_for_display(&self.blocks);
        } else if !self.content.is_empty() {
            self.blocks = vec![TranscriptBlock::Text {
                text: self.content.clone(),
            }];
        } else if self.usage.total_tokens == 0 {
            self.usage = TokenUsage::from_text("", &self.content);
        }

        ProviderResponse {
            provider: self.provider,
            fallback_from: self.fallback_from,
            content: self.content,
            blocks: self.blocks,
            stop_reason: self.stop_reason,
            usage: self.usage,
            deltas: self.deltas,
        }
    }
}

pub fn render_blocks_for_display(blocks: &[TranscriptBlock]) -> String {
    visible_content_from_blocks(blocks)
}

fn append_tool_json_delta(input: &mut String, partial: &str) {
    if input.trim().is_empty() || is_empty_json_object_literal(input) {
        input.clear();
    }
    input.push_str(partial);
}

fn set_block_at(blocks: &mut Vec<TranscriptBlock>, index: usize, block: TranscriptBlock) {
    while blocks.len() <= index {
        blocks.push(TranscriptBlock::Thinking {
            text: String::new(),
            signature: None,
        });
    }
    blocks[index] = block;
}

fn is_empty_json_object_literal(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed == "{}" || trimmed == "{ }"
}
