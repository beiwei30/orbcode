mod anthropic;
mod openai;
mod openai_responses;

pub use anthropic::{AnthropicStreamReader, provider_stream_event_from_sse_frame};
pub use openai::OpenAiStreamReader;
pub use openai_responses::OpenAiResponsesStreamReader;

use orbcode_protocol::{ProviderId, StreamErrorCategory};

use crate::{ProviderError, ProviderErrorKind};

pub fn decode_stream_line(raw_line: &[u8]) -> Result<String, ProviderError> {
    decode_provider_stream_line(raw_line, ProviderId::Anthropic, "Anthropic")
}

pub(crate) fn decode_provider_stream_line(
    raw_line: &[u8],
    provider: ProviderId,
    stream_label: &str,
) -> Result<String, ProviderError> {
    let line = raw_line.strip_suffix(b"\n").unwrap_or(raw_line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8(line.to_vec()).map_err(|error| ProviderError {
        kind: ProviderErrorKind::Fatal,
        category: StreamErrorCategory::Other,
        provider: Some(provider),
        status: None,
        message: format!("{stream_label} stream contained invalid UTF-8: {error}"),
        suggestion: None,
        rate_limit: None,
    })
}
