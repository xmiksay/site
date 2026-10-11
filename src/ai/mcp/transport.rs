//! Transport plumbing for [`SiteMcp`](super::SiteMcp): the endpoint-pool HTTP
//! client every MCP connection rides, and the timeouts bounding one server's
//! handshake and tool listing. Split out of `mcp.rs` to keep both files under
//! the 400-line cap; the unit tests stay in `super::tests`.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use anyhow::Context;
use entanglement_provider::{HttpClient as PoolHttpClient, RetryConfig};
use entanglement_runtime::mcp::HttpClient;

/// Ceiling on one remote MCP server's `connect()` handshake (TCP connect +
/// `initialize` + `notifications/initialized`). Without this, a single
/// unresponsive server can stall `routes_for_user` for as long as the
/// underlying HTTP client's own per-request timeout (up to ~2 minutes across
/// the handshake's two round-trips). See issue #28.
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Ceiling on the `tools/list` that follows a successful handshake — issue
/// #28's bound applied to the *second* half of `routes_for_user`'s work.
///
/// Needed as of entanglement 0.6: ADR-0153/#559 moved the MCP transport onto
/// the provider's endpoint pool and dropped the transport's own flat 60s
/// whole-request timeout with it. The pool bounds the TCP+TLS connect and the
/// idle gap *within a streaming* body, but nothing bounds a non-streaming
/// response that simply never arrives. So a server that answers the handshake
/// and then goes silent — no longer a hypothetical, it's the same black-hole
/// failure mode #28 was filed for — would hang `routes_for_user` forever.
pub(super) const LIST_TOOLS_TIMEOUT: Duration = Duration::from_secs(10);

/// The `rpm` the MCP endpoint pool is built with — high enough that the pool's
/// pacing gate never fires, which is the point.
///
/// `RetryConfig::default()`'s 50 rpm is calibrated for a rate-limited LLM API
/// driven by a turn loop: `RateLimiter::new` turns it into `60_000 / rpm` ms
/// of forced spacing between two requests to the same endpoint, so a single
/// MCP server would cost ~1.2s for the handshake and ~2.4s to reach
/// `tools/list` — paid per server, sequentially, on every cold cache or 60s
/// TTL expiry. A user's own MCP servers are not that: they are their
/// endpoints, reached a handful of times per cache refresh, and pre-0.6 the
/// transport paced them not at all. Everything else in `RetryConfig` — the
/// retry schedule, the per-endpoint concurrency cap, the 429/`Retry-After`
/// cool-down — is genuine robustness, already bounded by [`CONNECT_TIMEOUT`]
/// and [`LIST_TOOLS_TIMEOUT`], and stays at its default. LLM traffic
/// (`ai::catalog`, one `HttpClient::new()` per provider row) keeps the
/// defaults wholesale.
///
/// There is no "unlimited" sentinel — `rpm` only ever divides, or bounds
/// `shared_store`'s per-minute request window — so this is a deliberately
/// unreachable value rather than a real limit: 1ms of spacing, and 1000 req/s
/// before the shared window would even notice. `u32::MAX` would work
/// arithmetically but reads like an accident.
pub(super) const MCP_ENDPOINT_RPM: u32 = 60_000;

/// Set (to `1`) to switch off `entanglement_provider`'s cross-process endpoint
/// gate (ADR-0144) for the whole process. Its admission and its release each
/// run a locked read-modify-write with two `fsync`s on
/// `$XDG_DATA_HOME/entanglement/endpoints/<sha256(url)>.state` — the release
/// synchronously in `Drop`, on a runtime worker — so one MCP handshake plus
/// `tools/list` cost twelve `fsync`s: tens of ms idle, past [`CONNECT_TIMEOUT`]
/// under heavy disk load, which silently dropped the server from the user's
/// tool list (#124). Its concurrency cap is also keyed by URL across every
/// process and connection, undoing the per-connect isolation
/// [`mcp_pool_client`] exists for. Every gate held inside one `HttpClient`
/// (pacing, concurrency, `Retry-After`) stays on for LLM and MCP traffic
/// alike. What is given up: the file also joined *separate* clients for the
/// same endpoint + key, and `ai::catalog`'s `refresh()` builds a fresh client
/// per provider on every refresh — so after an admin edit, sessions still on
/// the old client and those on the new one no longer share a 429 cool-down or
/// a concurrency cap until the old ones finish their turn. A narrow, brief
/// overlap, against a fixed fsync tax on every request. The provider reads it
/// per endpoint, so it must be set
/// before any thread exists: `site_server`'s `main`, and `.cargo/config.toml`
/// for `cargo test`/`cargo run`.
pub const NO_SHARED_ENDPOINT_STATE_ENV: &str = "ENTANGLEMENT_NO_SHARED_ENDPOINT_STATE";

/// [`RetryConfig::default`] with the pacing gate opened up — see
/// [`MCP_ENDPOINT_RPM`].
pub(super) fn mcp_retry_config() -> RetryConfig {
    RetryConfig {
        rpm: MCP_ENDPOINT_RPM,
        ..Default::default()
    }
}

/// The provider endpoint-pool client one MCP connection rides. One pool per
/// connect keeps each connection on its own endpoint bucket — the pre-0.6
/// isolation; the site has no cross-user endpoint budget to pool against.
pub(super) fn mcp_pool_client() -> anyhow::Result<PoolHttpClient> {
    PoolHttpClient::with_config(mcp_retry_config()).context("building the MCP endpoint HTTP client")
}

/// Run one MCP request on its own task, giving up after `limit` and reporting
/// an expiry as `"{what} timed out after Ns"`.
///
/// Spawned rather than awaited inline because since entanglement 0.6 an
/// abandoned MCP request is expensive to *drop*: the provider's per-endpoint
/// pool (ADR-0157) takes a cross-process lease per request and releases it
/// from `Drop` via two synchronous `fsync`s. Awaiting inline runs that
/// filesystem work on the caller's own thread *after* the deadline has already
/// passed — measured here at 2ms idle but 120ms+ under concurrent disk load —
/// which puts unbounded, I/O-load-sensitive time outside the very bound issue
/// #28 exists to enforce. Aborting a task hands that teardown to a runtime
/// worker instead, so the caller returns within `limit` regardless.
pub(super) async fn bounded<F, T>(limit: Duration, what: String, fut: F) -> anyhow::Result<T>
where
    F: Future<Output = anyhow::Result<T>> + Send + 'static,
    T: Send + 'static,
{
    let mut task = tokio::spawn(fut);
    // `&mut task` (not `task`) so the handle survives the timeout: dropping a
    // `JoinHandle` merely detaches, leaving a dead server's request running —
    // and renewing its endpoint lease — forever.
    match tokio::time::timeout(limit, &mut task).await {
        Ok(joined) => joined.with_context(|| format!("{what} panicked"))?,
        Err(_) => {
            task.abort();
            anyhow::bail!("{what} timed out after {}s", limit.as_secs_f64())
        }
    }
}

/// `HttpClient::connect`, bounded to `timeout_duration` — see [`CONNECT_TIMEOUT`]'s
/// doc. A parameter (not just the constant baked in) so a test can prove the
/// bound actually applies without waiting out the real production timeout.
pub(super) async fn connect_with_timeout(
    server: &str,
    url: &str,
    headers: &HashMap<String, String>,
    timeout_duration: Duration,
) -> anyhow::Result<HttpClient> {
    // Since entanglement 0.6 the MCP transport rides `entanglement_provider`'s
    // shared per-endpoint pool (#559/ADR-0153) rather than building its own
    // `reqwest::Client`. `api_key: None` marks these as user-declared servers,
    // not ones bundled with a provider key.
    let pool = mcp_pool_client()?;
    let (name, url, headers) = (server.to_string(), url.to_string(), headers.clone());
    let what = format!("MCP server `{name}` connect");
    bounded(timeout_duration, what, async move {
        HttpClient::connect(&name, &url, &headers, pool, None).await
    })
    .await
}
