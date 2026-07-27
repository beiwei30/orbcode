//! Live provider probe.
//!
//! Fires a single minimal streaming request at a configured provider and
//! reports whether it succeeded or surfaced a normalized [`ProviderError`]
//! (carrying `category` and `suggestion`). It runs the provider's real request
//! path, so it reuses the same error classifier the session loop relies on —
//! but without a full turn: there is no tool loop, no session persistence, and
//! the streamed response is discarded. Intended for `doctor`-style diagnostics
//! that want to surface auth/rate-limit/network issues ahead of a real turn.

use async_trait::async_trait;
use orbcode_protocol::ProviderId;

use crate::ProviderRequest;
use crate::adapters::provider_for;
use orbcode_protocol::StreamErrorCategory;

use crate::error::ProviderError;
use crate::model::ProviderStreamSink;
use crate::types::{ProviderCancellationToken, ProviderStreamEvent};

/// High-level classification of a probe outcome.
#[derive(Clone, Debug)]
pub enum ProbeResult {
    Ok,
    RateLimited { retry_after_seconds: Option<u64> },
    AccountSuspended,
    Failed(ProviderError),
}

/// Outcome of a single provider probe.
#[derive(Clone, Debug)]
pub struct ProviderProbeReport {
    pub provider: ProviderId,
    /// `None` on success; the normalized provider error otherwise.
    pub error: Option<ProviderError>,
}

impl ProviderProbeReport {
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }

    /// Classify the raw probe outcome into a high-level [`ProbeResult`].
    pub fn classify(&self) -> ProbeResult {
        match &self.error {
            None => ProbeResult::Ok,
            Some(error) => match error.category {
                StreamErrorCategory::RateLimit => ProbeResult::RateLimited {
                    retry_after_seconds: error.retry_after_secs(),
                },
                StreamErrorCategory::AccountSuspended => ProbeResult::AccountSuspended,
                _ => ProbeResult::Failed(error.clone()),
            },
        }
    }
}

/// Discards every stream event. The probe only cares about success-vs-error.
struct DiscardSink;

#[async_trait]
impl ProviderStreamSink for DiscardSink {
    async fn emit(&mut self, _event: ProviderStreamEvent) -> Result<(), ProviderError> {
        Ok(())
    }
}

/// Run one minimal streaming request against `provider_id` using `request`.
///
/// The caller is responsible for building `request` from config (base URL,
/// credentials, a short prompt). Returns a [`ProviderProbeReport`]; provider
/// attribution is guaranteed on any error so the suggestion lookup is populated.
pub async fn probe_provider(
    provider_id: ProviderId,
    request: &ProviderRequest,
    cancellation: ProviderCancellationToken,
) -> ProviderProbeReport {
    let provider = provider_for(provider_id);
    let mut sink = DiscardSink;
    match provider.stream(request, &mut sink, cancellation).await {
        Ok(_) => ProviderProbeReport {
            provider: provider_id,
            error: None,
        },
        Err(error) => ProviderProbeReport {
            provider: provider_id,
            error: Some(error.with_provider(provider_id)),
        },
    }
}
