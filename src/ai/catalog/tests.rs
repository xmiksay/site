//! Unit tests for `src/ai/catalog.rs`, split out to keep that file under the
//! 400-line cap.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The exact lock type `SiteCatalog.inner` uses. A `std::sync::RwLock`
/// would poison here — this proves `parking_lot::RwLock` doesn't, so one
/// panicking `refresh()` call can't fail-closed every later
/// `model_by_id`/`default_model` lookup for every session (issue #28).
#[test]
fn panicking_while_holding_the_write_lock_does_not_poison_it() {
    let lock = Arc::new(RwLock::new(CatalogInner::default()));
    let panicking = lock.clone();

    let result = std::thread::spawn(move || {
        let _guard = panicking.write();
        panic!("simulated panic mid-refresh");
    })
    .join();
    assert!(result.is_err(), "the spawned thread should have panicked");

    // A `std::sync::RwLock` would return `Err(Poisoned)` here instead.
    let inner = lock.read();
    assert!(inner.by_model_id.is_empty());
    assert!(inner.default_model_id.is_none());
}

fn provider(kind: &str, api_key: Option<&str>, base_url: Option<&str>) -> llm_provider::Model {
    provider_with_limits(kind, api_key, base_url, None, None)
}

fn provider_with_limits(
    kind: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
    concurrency: Option<i32>,
    rpm: Option<i32>,
) -> llm_provider::Model {
    llm_provider::Model {
        id: 1,
        label: "test-provider".to_string(),
        kind: kind.to_string(),
        api_key: api_key.map(str::to_string),
        base_url: base_url.map(str::to_string),
        concurrency,
        rpm,
        created_at: chrono::Utc::now().fixed_offset(),
    }
}

#[test]
fn ollama_without_base_url_falls_back_to_default() {
    let p = provider("ollama", None, None);
    assert_eq!(ollama_base_url(&p), OLLAMA_BASE);
    assert!(build_factory(&p, "model", &HttpClient::new()).is_ok());
}

#[test]
fn ollama_with_blank_base_url_falls_back_to_default() {
    let p = provider("ollama", None, Some(""));
    assert_eq!(ollama_base_url(&p), OLLAMA_BASE);
}

#[test]
fn ollama_with_base_url_uses_it() {
    let p = provider("ollama", None, Some("http://example.internal:1234/v1"));
    assert_eq!(ollama_base_url(&p), "http://example.internal:1234/v1");
}

#[test]
fn anthropic_without_api_key_errs() {
    let p = provider("anthropic", None, None);
    let err = build_factory(&p, "model", &HttpClient::new())
        .err()
        .expect("expected build_factory to fail");
    assert!(err.to_string().contains("no api_key"));
}

#[test]
fn anthropic_with_api_key_builds_ok() {
    let p = provider("anthropic", Some("key"), None);
    assert!(build_factory(&p, "model", &HttpClient::new()).is_ok());
}

#[test]
fn gemini_without_api_key_errs() {
    let p = provider("gemini", None, None);
    let err = build_factory(&p, "model", &HttpClient::new())
        .err()
        .expect("expected build_factory to fail");
    assert!(err.to_string().contains("no api_key"));
}

#[test]
fn gemini_with_api_key_builds_ok() {
    let p = provider("gemini", Some("key"), None);
    assert!(build_factory(&p, "model", &HttpClient::new()).is_ok());
}

#[test]
fn unsupported_kind_errs_naming_it() {
    let p = provider("mystery", None, None);
    let err = build_factory(&p, "model", &HttpClient::new())
        .err()
        .expect("expected build_factory to fail");
    assert!(err.to_string().contains("mystery"));
}

#[test]
fn positive_u32_passes_through_a_positive_value() {
    assert_eq!(positive_u32(Some(30)), Some(30));
}

#[test]
fn positive_u32_treats_zero_or_negative_as_unset() {
    assert_eq!(positive_u32(Some(0)), None);
    assert_eq!(positive_u32(Some(-1)), None);
    assert_eq!(positive_u32(None), None);
}

#[test]
fn positive_usize_passes_through_a_positive_value() {
    assert_eq!(positive_usize(Some(2)), Some(2));
}

#[test]
fn positive_usize_treats_zero_or_negative_as_unset() {
    assert_eq!(positive_usize(Some(0)), None);
    assert_eq!(positive_usize(Some(-5)), None);
    assert_eq!(positive_usize(None), None);
}

/// A minimal OpenAI-compat SSE mock: accepts a connection, tracks how many
/// are open at once (updating `max_seen`), holds the connection for
/// `delay` before responding, then closes it. Good enough for
/// `openai_factory`'s stream parser — it doesn't validate the request, only
/// that a response arrived.
async fn spawn_concurrency_probe(delay: std::time::Duration) -> (String, Arc<AtomicUsize>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let (in_flight, max_seen_task) = (in_flight, max_seen.clone());

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let in_flight = in_flight.clone();
            let max_seen = max_seen_task.clone();
            tokio::spawn(async move {
                // Drain whatever the client already wrote; the mock never
                // inspects the request, so a best-effort read is enough.
                let mut buf = [0u8; 4096];
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    socket.read(&mut buf),
                )
                .await;

                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);

                let body = "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
                             data: [DONE]\n\n";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    (format!("http://{addr}"), max_seen)
}

/// ADR-0111: `llm_providers.concurrency` must cap **simultaneously
/// in-flight requests to that endpoint**, not just be plumbed through and
/// ignored. Three turns fired at once against a `concurrency: Some(1)`
/// ollama provider must serialize — without the fix (both `None`, the
/// library's default concurrency of 3) all three would race through
/// together and `max_seen` would hit 3. `rpm` is pinned to a very high
/// budget so the (separate) adaptive pacing gate can't itself space the
/// three dispatches out and produce `max_seen == 1` for the wrong reason —
/// the concurrency semaphore must be what's actually gating them.
#[tokio::test]
async fn concurrency_capped_provider_serializes_concurrent_turns() {
    let (base_url, max_seen) = spawn_concurrency_probe(std::time::Duration::from_millis(150)).await;
    let http = HttpClient::new();
    let p = provider_with_limits("ollama", None, Some(&base_url), Some(1), Some(1_000_000));
    let factory = build_factory(&p, "model", &http).expect("build factory");

    let mut handles = Vec::new();
    for _ in 0..3 {
        let factory = factory.clone();
        handles.push(tokio::spawn(async move {
            let mut llm = factory();
            let mut stream = llm
                .stream(entanglement_provider::LlmRequest {
                    system: "",
                    model: None,
                    messages: &[],
                    tools: &[],
                    generation: None,
                })
                .await
                .expect("stream should start");
            while futures_util::StreamExt::next(&mut stream).await.is_some() {}
        }));
    }
    for h in handles {
        h.await.expect("turn task should not panic");
    }

    assert_eq!(
        max_seen.load(Ordering::SeqCst),
        1,
        "concurrency: Some(1) must serialize every in-flight request to this endpoint"
    );
}
