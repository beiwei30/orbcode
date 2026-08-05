use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::{MessageRole, TranscriptBlock, TranscriptMessage};

const ROUGH_PROVIDER_TOOL_RESULT_MAX_CHARS: usize = 100_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ServerToolUseUsage {
    #[serde(default, deserialize_with = "deserialize_u32_or_zero")]
    pub web_search_requests: u32,
    #[serde(default, deserialize_with = "deserialize_u32_or_zero")]
    pub web_fetch_requests: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CacheCreationUsage {
    #[serde(default, deserialize_with = "deserialize_u32_or_zero")]
    pub ephemeral_1h_input_tokens: u32,
    #[serde(default, deserialize_with = "deserialize_u32_or_zero")]
    pub ephemeral_5m_input_tokens: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UsageIteration {
    #[serde(default, deserialize_with = "deserialize_u32_or_zero")]
    pub input_tokens: u32,
    #[serde(default, deserialize_with = "deserialize_u32_or_zero")]
    pub output_tokens: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TokenUsage {
    #[serde(default, deserialize_with = "deserialize_u32_or_zero")]
    pub input_tokens: u32,
    #[serde(default, deserialize_with = "deserialize_u32_or_zero")]
    pub cache_creation_input_tokens: u32,
    #[serde(default, deserialize_with = "deserialize_u32_or_zero")]
    pub cache_read_input_tokens: u32,
    #[serde(default, deserialize_with = "deserialize_u32_or_zero")]
    pub output_tokens: u32,
    #[serde(default, deserialize_with = "deserialize_server_tool_use_usage")]
    pub server_tool_use: ServerToolUseUsage,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default, deserialize_with = "deserialize_cache_creation_usage")]
    pub cache_creation: CacheCreationUsage,
    #[serde(default, deserialize_with = "deserialize_usage_iterations")]
    pub iterations: Vec<UsageIteration>,
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default, deserialize_with = "deserialize_u32_or_zero")]
    pub total_tokens: u32,
}

impl TokenUsage {
    pub fn from_text(input: &str, output: &str) -> Self {
        let input_tokens = token_count(input);
        let output_tokens = token_count(output);
        Self {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
            ..Self::default()
        }
    }

    pub fn component_total_tokens(&self) -> u32 {
        self.input_tokens
            .saturating_add(self.cache_creation_input_tokens)
            .saturating_add(self.cache_read_input_tokens)
            .saturating_add(self.output_tokens)
    }

    pub fn refresh_total_from_components(&mut self) {
        self.total_tokens = self.component_total_tokens();
    }
}

fn deserialize_u32_or_zero<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<u64>::deserialize(deserializer)?.unwrap_or(0) as u32)
}

fn deserialize_usage_iterations<'de, D>(deserializer: D) -> Result<Vec<UsageIteration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(Vec::new());
    };
    match value {
        Value::Array(values) => Ok(values
            .into_iter()
            .filter(serde_json::Value::is_object)
            .map(UsageIteration::deserialize)
            .filter_map(Result::ok)
            .collect()),
        _ => Ok(Vec::new()),
    }
}

fn deserialize_server_tool_use_usage<'de, D>(
    deserializer: D,
) -> Result<ServerToolUseUsage, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<ServerToolUseUsage>::deserialize(deserializer)?.unwrap_or_default())
}

fn deserialize_cache_creation_usage<'de, D>(deserializer: D) -> Result<CacheCreationUsage, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<CacheCreationUsage>::deserialize(deserializer)?.unwrap_or_default())
}

pub fn get_token_count_from_usage(usage: &TokenUsage) -> u32 {
    usage.component_total_tokens()
}

pub fn token_count_from_last_api_response(messages: &[TranscriptMessage]) -> u32 {
    messages
        .iter()
        .rev()
        .find_map(get_token_usage)
        .map_or(0, get_token_count_from_usage)
}

pub fn final_context_tokens_from_last_response(messages: &[TranscriptMessage]) -> u32 {
    messages
        .iter()
        .rev()
        .find_map(get_token_usage)
        .map_or(0, final_context_tokens_from_usage)
}

pub fn message_token_count_from_last_api_response(messages: &[TranscriptMessage]) -> u32 {
    messages
        .iter()
        .rev()
        .find_map(get_token_usage)
        .map_or(0, |usage| usage.output_tokens)
}

pub fn get_current_usage(messages: &[TranscriptMessage]) -> Option<TokenUsage> {
    messages.iter().rev().find_map(get_token_usage).cloned()
}

pub fn token_count_with_estimation(messages: &[TranscriptMessage]) -> u32 {
    for (index, message) in messages.iter().enumerate().rev() {
        if let Some(usage) = get_token_usage(message) {
            let mut anchor = index;
            let response_id = message.id.as_str();
            let mut prior_index = index;
            while prior_index > 0 {
                prior_index -= 1;
                let prior = &messages[prior_index];
                if matches!(prior.role, MessageRole::Assistant) {
                    if prior.id == response_id {
                        anchor = prior_index;
                    } else {
                        break;
                    }
                }
            }
            return get_token_count_from_usage(usage).saturating_add(
                rough_token_count_estimation_for_messages(&messages[anchor.saturating_add(1)..]),
            );
        }
    }
    rough_token_count_estimation_for_messages(messages)
}

pub fn rough_token_count_estimation_for_messages(messages: &[TranscriptMessage]) -> u32 {
    messages
        .iter()
        .map(rough_token_count_estimation_for_message)
        .fold(0_u32, u32::saturating_add)
}

fn get_token_usage(message: &TranscriptMessage) -> Option<&TokenUsage> {
    if matches!(message.role, MessageRole::Assistant) && !message.is_synthetic {
        message.usage.as_ref()
    } else {
        None
    }
}

fn final_context_tokens_from_usage(usage: &TokenUsage) -> u32 {
    usage
        .iterations
        .last()
        .map(|iteration| {
            iteration
                .input_tokens
                .saturating_add(iteration.output_tokens)
        })
        .unwrap_or_else(|| usage.input_tokens.saturating_add(usage.output_tokens))
}

fn rough_token_count_estimation_for_message(message: &TranscriptMessage) -> u32 {
    if message.blocks.is_empty() {
        return rough_token_count_estimate(&message.content);
    }
    message
        .blocks
        .iter()
        .map(rough_token_count_estimation_for_block)
        .fold(0_u32, u32::saturating_add)
}

fn rough_token_count_estimation_for_block(block: &TranscriptBlock) -> u32 {
    match block {
        TranscriptBlock::Text { text } | TranscriptBlock::Thinking { text, .. } => {
            rough_token_count_estimate(text)
        }
        TranscriptBlock::ToolUse { name, input, .. } => {
            rough_token_count_estimate(&provider_visible_tool_use_for_estimation(name, input))
        }
        TranscriptBlock::ToolResult { content, .. } => {
            rough_token_count_estimate(&provider_visible_tool_result_for_estimation(content))
        }
    }
}

fn provider_visible_tool_use_for_estimation(name: &str, input: &str) -> String {
    let rendered_input = serde_json::from_str::<Value>(input)
        .map_or_else(|_| input.to_string(), |value| value.to_string());
    format!("{name}{rendered_input}")
}

fn provider_visible_tool_result_for_estimation(content: &str) -> String {
    if content.chars().count() <= ROUGH_PROVIDER_TOOL_RESULT_MAX_CHARS {
        return content.to_string();
    }
    content
        .chars()
        .take(ROUGH_PROVIDER_TOOL_RESULT_MAX_CHARS)
        .collect()
}

fn rough_token_count_estimate(text: &str) -> u32 {
    (text.chars().count() as u32).saturating_add(2) / 4
}

fn token_count(text: &str) -> u32 {
    rough_token_count_estimate(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{MessageRole, TranscriptBlock, TranscriptMessage};

    #[test]
    fn token_usage_deserializes_legacy_nullable_fields() {
        let usage: TokenUsage = serde_json::from_value(serde_json::json!({
            "input_tokens": 1,
            "output_tokens": 2,
            "total_tokens": 3,
            "cache_creation_input_tokens": null,
            "cache_read_input_tokens": null,
            "server_tool_use": null,
            "service_tier": null,
            "cache_creation": null,
            "iterations": [
                { "input_tokens": 10, "output_tokens": 2 },
                { "input_tokens": 12, "output_tokens": 3 }
            ],
            "speed": null
        }))
        .expect("legacy usage deserializes");

        assert_eq!(usage.cache_creation_input_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 0);
        assert_eq!(usage.server_tool_use.web_search_requests, 0);
        assert_eq!(usage.cache_creation.ephemeral_1h_input_tokens, 0);
        assert_eq!(usage.iterations.len(), 2);
        assert_eq!(usage.iterations[1].input_tokens, 12);
    }

    #[test]
    fn usage_utilities_use_last_assistant_usage_and_estimate_later_messages() {
        let first_usage = TokenUsage {
            input_tokens: 10,
            cache_creation_input_tokens: 5,
            cache_read_input_tokens: 7,
            output_tokens: 3,
            ..TokenUsage::default()
        };
        let second_usage = TokenUsage {
            input_tokens: 20,
            output_tokens: 4,
            iterations: vec![UsageIteration {
                input_tokens: 18,
                output_tokens: 2,
            }],
            ..TokenUsage::default()
        };
        let messages = vec![
            TranscriptMessage::new(MessageRole::User, "hello"),
            TranscriptMessage::new(MessageRole::Assistant, "first").with_usage(first_usage),
            TranscriptMessage::new(MessageRole::Assistant, "second").with_usage(second_usage),
            TranscriptMessage::new(MessageRole::User, "abcdefghijkl"),
        ];

        assert_eq!(token_count_from_last_api_response(&messages), 24);
        assert_eq!(final_context_tokens_from_last_response(&messages), 20);
        assert_eq!(message_token_count_from_last_api_response(&messages), 4);
        assert_eq!(
            get_current_usage(&messages)
                .expect("current usage")
                .input_tokens,
            20
        );
        assert_eq!(token_count_with_estimation(&messages), 27);
    }

    #[test]
    fn token_count_with_estimation_falls_back_when_usage_is_absent() {
        let messages = vec![
            TranscriptMessage::new(MessageRole::User, "abcdefgh"),
            TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "Read".to_string(),
                    input: r#"{"file_path":"/tmp/example.rs"}"#.to_string(),
                }],
            ),
        ];

        assert_eq!(token_count_from_last_api_response(&messages), 0);
        assert!(token_count_with_estimation(&messages) > 2);
    }

    #[test]
    fn token_count_with_estimation_counts_tool_results_after_last_usage() {
        let usage = TokenUsage {
            input_tokens: 40,
            output_tokens: 5,
            ..TokenUsage::default()
        };
        let messages = vec![
            TranscriptMessage::new(MessageRole::Assistant, "call tool").with_usage(usage),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "abcdefghijklmnop".into(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        ];

        assert_eq!(token_count_from_last_api_response(&messages), 45);
        assert_eq!(token_count_with_estimation(&messages), 49);
    }

    #[test]
    fn token_count_with_estimation_anchors_at_first_split_assistant_response() {
        let usage = TokenUsage {
            input_tokens: 40,
            output_tokens: 5,
            ..TokenUsage::default()
        };
        let mut first_assistant = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"one"}"#.to_string(),
            }],
        )
        .with_usage(usage.clone());
        first_assistant.id = "api-response-1".to_string();
        let first_result = TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "abcdefghijklmnop".into(),
                is_error: false,
                metadata: None,
            }],
        );
        let mut second_assistant = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::ToolUse {
                id: "tool-2".to_string(),
                name: "Read".to_string(),
                input: r#"{"file_path":"a"}"#.to_string(),
            }],
        )
        .with_usage(usage);
        second_assistant.id = "api-response-1".to_string();
        let second_result = TranscriptMessage::from_blocks(
            MessageRole::User,
            vec![TranscriptBlock::ToolResult {
                tool_use_id: "tool-2".to_string(),
                content: "qrstuvwxyzabcdef".into(),
                is_error: false,
                metadata: None,
            }],
        );
        let messages = vec![
            first_assistant,
            first_result,
            second_assistant,
            second_result,
        ];

        assert_eq!(
            token_count_with_estimation(&messages),
            45 + rough_token_count_estimation_for_messages(&messages[1..])
        );
    }

    #[test]
    fn token_count_with_estimation_ignores_tool_result_metadata() {
        let usage = TokenUsage {
            input_tokens: 40,
            output_tokens: 5,
            ..TokenUsage::default()
        };
        let messages = vec![
            TranscriptMessage::new(MessageRole::Assistant, "call tool").with_usage(usage),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "abcdefghijklmnop".into(),
                    is_error: false,
                    metadata: Some("x".repeat(100_000)),
                }],
            ),
        ];

        assert_eq!(token_count_with_estimation(&messages), 49);
    }

    #[test]
    fn token_count_with_estimation_caps_oversized_tool_results() {
        let usage = TokenUsage {
            input_tokens: 40,
            output_tokens: 5,
            ..TokenUsage::default()
        };
        let messages = vec![
            TranscriptMessage::new(MessageRole::Assistant, "call tool").with_usage(usage),
            TranscriptMessage::from_blocks(
                MessageRole::User,
                vec![TranscriptBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "a"
                        .repeat(ROUGH_PROVIDER_TOOL_RESULT_MAX_CHARS + 50_000)
                        .into(),
                    is_error: false,
                    metadata: None,
                }],
            ),
        ];

        assert_eq!(
            token_count_with_estimation(&messages),
            45 + (ROUGH_PROVIDER_TOOL_RESULT_MAX_CHARS as u32 / 4)
        );
    }

    #[test]
    fn token_count_with_estimation_skips_synthetic_assistant_messages() {
        let real_usage = TokenUsage {
            input_tokens: 50,
            output_tokens: 10,
            ..TokenUsage::default()
        };
        let synthetic_usage = TokenUsage {
            input_tokens: 200,
            output_tokens: 40,
            ..TokenUsage::default()
        };
        let messages = vec![
            TranscriptMessage::new(MessageRole::User, "hello"),
            TranscriptMessage::new(MessageRole::Assistant, "real response")
                .with_usage(real_usage.clone()),
            TranscriptMessage::new(MessageRole::Assistant, "compact summary")
                .with_usage(synthetic_usage)
                .with_synthetic(true),
            TranscriptMessage::new(MessageRole::User, "follow up"),
        ];

        let estimation = token_count_with_estimation(&messages);
        let expected_base = get_token_count_from_usage(&real_usage);
        assert!(
            estimation >= expected_base,
            "estimation {estimation} should be at least the real usage {expected_base}"
        );
        assert!(estimation < 100);
    }

    #[test]
    fn get_current_usage_skips_synthetic_assistant_messages() {
        let real_usage = TokenUsage {
            input_tokens: 50,
            output_tokens: 10,
            ..TokenUsage::default()
        };
        let synthetic_usage = TokenUsage {
            input_tokens: 200,
            output_tokens: 40,
            ..TokenUsage::default()
        };
        let messages = vec![
            TranscriptMessage::new(MessageRole::Assistant, "real response")
                .with_usage(real_usage.clone()),
            TranscriptMessage::new(MessageRole::Assistant, "synthetic")
                .with_usage(synthetic_usage)
                .with_synthetic(true),
        ];

        let usage = get_current_usage(&messages).expect("should find real usage");
        assert_eq!(usage.input_tokens, 50);
        assert_eq!(usage.output_tokens, 10);
    }

    #[test]
    fn synthetic_system_and_user_messages_never_anchor() {
        let usage = TokenUsage {
            input_tokens: 30,
            output_tokens: 5,
            ..TokenUsage::default()
        };
        let messages = vec![
            TranscriptMessage::new(MessageRole::System, "compacted history").with_synthetic(true),
            TranscriptMessage::new(MessageRole::User, "hook context: approved")
                .with_synthetic(true),
            TranscriptMessage::new(MessageRole::Assistant, "real").with_usage(usage.clone()),
            TranscriptMessage::new(MessageRole::User, "please continue").with_synthetic(true),
        ];

        let estimation = token_count_with_estimation(&messages);
        let base = get_token_count_from_usage(&usage);
        assert!(estimation >= base);
        assert!(estimation < 100);
    }

    #[test]
    fn rough_estimation_matches_typescript_character_estimate() {
        let prose = "aaaaaaaaaaaaaaaa";
        let json = r#"{"a":1,"b":2}"#;
        let code = "fn main() {\nlet value = 1;\nprintln!(\"{value}\");\n}\n";

        assert_eq!(rough_token_count_estimate(prose), 4);
        assert_eq!(rough_token_count_estimate(json), 3);
        assert_eq!(
            rough_token_count_estimate(code),
            (code.chars().count() as u32).saturating_add(2) / 4
        );
    }
}
