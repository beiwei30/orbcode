use async_trait::async_trait;
use orbcode_protocol::ProviderId;

use crate::{
    ModelProvider, ProviderCancellationToken, ProviderCompletion, ProviderDescriptor,
    ProviderError, ProviderRequest, ProviderStreamSink, stream_openai_request,
};

pub(super) struct OpenAiProvider;

impl OpenAiProvider {
    pub(super) fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::OpenAi,
            summary: "OpenAI Chat Completions and ChatGPT/Codex Responses streaming adapter.",
        }
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAi
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
        if should_use_openai_stub(request) {
            return super::stream_stub_response(
                self.id(),
                "OpenAI-compatible phase 2 response",
                request,
                sink,
                cancellation,
            )
            .await;
        }
        stream_openai_request(request, sink, cancellation).await
    }

    async fn count_tokens(
        &self,
        request: &ProviderRequest,
    ) -> Result<Option<usize>, ProviderError> {
        #[cfg(any(test, feature = "mock-provider"))]
        if crate::mock::is_mock_base_url(&request.base_url) {
            return crate::mock::count_tokens_mock(request);
        }
        if should_use_openai_stub(request) {
            return Ok(None);
        }
        Ok(None)
    }
}

fn should_use_openai_stub(request: &ProviderRequest) -> bool {
    request.base_url.starts_with("stub://")
        || (request.api_key.is_none()
            && request.auth_token.is_none()
            && !is_local_openai_base_url(&request.base_url))
}

fn is_local_openai_base_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
}
