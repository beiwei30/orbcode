mod anthropic;
mod openai;
mod openai_responses;

pub use anthropic::{AnthropicStreamReader, provider_stream_event_from_sse_frame};
pub use openai::OpenAiStreamReader;
pub use openai_responses::OpenAiResponsesStreamReader;

use orbcode_protocol::StreamErrorCategory;

use crate::{ProviderError, ProviderErrorKind};

pub fn decode_stream_line(raw_line: &[u8]) -> Result<String, ProviderError> {
    let line = raw_line.strip_suffix(b"\n").unwrap_or(raw_line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8(line.to_vec()).map_err(|error| ProviderError {
        kind: ProviderErrorKind::Fatal,
        category: StreamErrorCategory::Other,
        provider: None,
        status: None,
        message: format!("Anthropic stream contained invalid UTF-8: {error}"),
        suggestion: None,
        rate_limit: None,
    })
}
