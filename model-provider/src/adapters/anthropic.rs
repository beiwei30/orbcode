use async_trait::async_trait;
use orbcode_protocol::ProviderId;

use crate::{
    ModelProvider, ProviderCancellationToken, ProviderCompletion, ProviderDescriptor,
    ProviderError, ProviderRequest, ProviderStreamSink, count_tokens_anthropic,
    stream_anthropic_request,
};

pub(super) struct AnthropicProvider;

impl AnthropicProvider {
    pub(super) fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::Anthropic,
            summary: "Anthropic-compatible streaming adapter driven by ~/.claude settings or process env.",
        }
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Anthropic
    }

    async fn stream(
        &self,
        request: &ProviderRequest,
        sink: &mut dyn ProviderStreamSink,
        cancellation: ProviderCancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        #[cfg(any(test, feature = "mock-provider"))]
        if crate::mock::is_mock_base_url(&request.base_url) {
            return crate::mock::stream_mock(self.id(), request, sink, cancellation).await;
        }
        if request.base_url.starts_with("stub://") {
            return super::stream_stub_response(
                self.id(),
                "Anthropic compatibility stub response",
                request,
                sink,
                cancellation,
            )
            .await;
        }
        stream_anthropic_request(request, sink, cancellation).await
    }

    async fn count_tokens(
        &self,
        request: &ProviderRequest,
    ) -> Result<Option<usize>, ProviderError> {
        #[cfg(any(test, feature = "mock-provider"))]
        if crate::mock::is_mock_base_url(&request.base_url) {
            return crate::mock::count_tokens_mock(request);
        }
        if request.base_url.starts_with("stub://") {
            return Ok(None);
        }
        count_tokens_anthropic(request).await
    }
}
