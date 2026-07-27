use async_trait::async_trait;
use orbcode_protocol::ProviderId;

use crate::{
    ModelProvider, ProviderCancellationToken, ProviderCompletion, ProviderError, ProviderRequest,
    ProviderStreamSink,
};

pub(super) struct DisabledProvider(pub(super) ProviderId);

#[async_trait]
impl ModelProvider for DisabledProvider {
    fn id(&self) -> ProviderId {
        self.0
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
        let _ = (request, sink, cancellation);
        Err(ProviderError::unsupported_provider(self.0))
    }

    async fn count_tokens(
        &self,
        request: &ProviderRequest,
    ) -> Result<Option<usize>, ProviderError> {
        #[cfg(any(test, feature = "mock-provider"))]
        if crate::mock::is_mock_base_url(&request.base_url) {
            return crate::mock::count_tokens_mock(request);
        }
        let _ = request;
        Ok(None)
    }
}
