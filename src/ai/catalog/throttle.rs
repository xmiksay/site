//! Per-provider throttle status (#89), split out of `catalog.rs` to keep
//! that file under the 400-line cap. A provider's dedicated `HttpClient`
//! (built in `refresh()`) is what makes a throttle reading addressable per
//! provider at all — a single client shared across every provider could only
//! ever report one global worst-offender endpoint, unattributable to a
//! `provider_id`.

use entanglement_provider::{GEMINI_BASE, HttpClient};

use crate::entity::llm_provider;

use super::{SiteCatalog, ollama_base_url};

/// Mirrors `entanglement_provider::client`'s own private `DEFAULT_CONCURRENCY`
/// (currently `3`), purely for display when a provider has no configured
/// `concurrency` and has never made a live request yet. The live client still
/// independently resolves its own real default the first time a request
/// lands — this constant never feeds back into request behavior.
pub(super) const DEFAULT_CONCURRENCY_FALLBACK: usize = 3;

/// One provider row's dedicated `HttpClient` (see `refresh()`), plus the
/// idle-state label/cap to show before any request has ever landed on it —
/// the live pool only learns an endpoint's real URL lazily, on first use.
pub(super) struct ProviderHandle {
    pub(super) endpoint: String,
    pub(super) cap: usize,
    pub(super) http: HttpClient,
}

/// Live throttle posture for one provider (#89) — the wire shape returned by
/// `GET /api/assistant/providers/status`, serialized directly. Unlike
/// `ProviderView` (which wraps an `llm_provider::Model` DB row for read
/// paths), there's no DB row to separate this from — it's a live snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderThrottleStatus {
    pub provider_id: i32,
    pub endpoint: String,
    pub in_flight: usize,
    pub cap: usize,
    pub backoff_remaining_ms: Option<u64>,
    pub penalized: bool,
}

/// A human label for `provider`'s endpoint before any request has landed —
/// `throttle_statuses()`'s idle fallback, since the live pool only learns an
/// endpoint's URL lazily on first use.
pub(super) fn provider_endpoint_label(provider: &llm_provider::Model) -> String {
    match provider.kind.as_str() {
        "ollama" => ollama_base_url(provider),
        "gemini" => GEMINI_BASE.to_string(),
        "anthropic" => "https://api.anthropic.com".to_string(),
        _ => provider.base_url.clone().unwrap_or_default(),
    }
}

impl SiteCatalog {
    /// Live throttle posture for every provider (#89) — `GET
    /// /api/assistant/providers/status` polls this so the admin providers view
    /// can show *why* the assistant is slow (429 cool-down, permit exhaustion)
    /// instead of guessing. Each provider's dedicated `HttpClient` (see
    /// `refresh()`) is what makes this addressable per provider at all.
    pub fn throttle_statuses(&self) -> Vec<ProviderThrottleStatus> {
        let inner = self.inner.read();
        let mut statuses: Vec<ProviderThrottleStatus> = inner
            .by_provider_id
            .iter()
            .map(
                |(&provider_id, handle)| match handle.http.throttle_status() {
                    Some(live) => ProviderThrottleStatus {
                        provider_id,
                        endpoint: live.endpoint,
                        in_flight: live.in_flight,
                        cap: live.cap,
                        backoff_remaining_ms: live.backoff_remaining.map(|d| d.as_millis() as u64),
                        penalized: live.penalized,
                    },
                    None => ProviderThrottleStatus {
                        provider_id,
                        endpoint: handle.endpoint.clone(),
                        in_flight: 0,
                        cap: handle.cap,
                        backoff_remaining_ms: None,
                        penalized: false,
                    },
                },
            )
            .collect();
        statuses.sort_by_key(|s| s.provider_id);
        statuses
    }
}
