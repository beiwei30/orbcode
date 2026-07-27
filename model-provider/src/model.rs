use async_trait::async_trait;
use orbcode_protocol::ProviderId;

use crate::{
    ProviderCancellationToken, ProviderCompletion, ProviderError, ProviderRequest,
    ProviderResponse, ProviderStreamAccumulator, ProviderStreamEvent,
};

#[async_trait]
pub trait ProviderStreamSink: Send {
    async fn emit(&mut self, event: ProviderStreamEvent) -> Result<(), ProviderError>;

    async fn discard_attempt(
        &mut self,
        _provider: ProviderId,
        _fallback_provider: ProviderId,
        _reason: &str,
    ) -> Result<(), ProviderError> {
        Ok(())
    }
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn id(&self) -> ProviderId;

    async fn stream(
        &self,
        request: &ProviderRequest,
        sink: &mut dyn ProviderStreamSink,
        cancellation: ProviderCancellationToken,
    ) -> Result<ProviderCompletion, ProviderError>;

    /// Count tokens for a set of messages and tools using the provider's count-tokens API.
    /// Returns the estimated input token count, or None if count-tokens is not available.
    async fn count_tokens(&self, request: &ProviderRequest)
    -> Result<Option<usize>, ProviderError>;

    async fn generate(&self, request: &ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        let mut sink = CollectingProviderStreamSink::new(self.id(), None);
        self.stream(request, &mut sink, ProviderCancellationToken::default())
            .await?;
        Ok(sink.into_response())
    }
}

struct CollectingProviderStreamSink {
    accumulator: ProviderStreamAccumulator,
}

impl CollectingProviderStreamSink {
    fn new(provider: ProviderId, fallback_from: Option<ProviderId>) -> Self {
        Self {
            accumulator: ProviderStreamAccumulator::new(provider, fallback_from),
        }
    }

    fn into_response(self) -> ProviderResponse {
        self.accumulator.into_response()
    }
}

#[async_trait]
impl ProviderStreamSink for CollectingProviderStreamSink {
    async fn emit(&mut self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        self.accumulator.apply(&event);
        Ok(())
    }

    async fn discard_attempt(
        &mut self,
        provider: ProviderId,
        fallback_provider: ProviderId,
        _reason: &str,
    ) -> Result<(), ProviderError> {
        self.accumulator = ProviderStreamAccumulator::new(fallback_provider, Some(provider));
        Ok(())
    }
}
