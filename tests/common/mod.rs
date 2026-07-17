//! Shared helpers for the assistant-session integration tests
//! (`tests/assistant_session_base.rs`,
//! `tests/assistant_session_subagent_researcher.rs`,
//! `tests/assistant_session_subagent_pagewriter.rs`). Named `common/mod.rs`
//! rather than `common.rs`: Cargo's integration-test discovery only picks up
//! files directly under `tests/*.rs` as their own test binaries, so a
//! `tests/common.rs` would itself become a (helper-only, pointless) test
//! target — the `mod.rs` subdirectory form is the standard way to share code
//! across integration tests without that happening.
//!
//! Each `tests/*.rs` file is compiled as its own crate, so `pub` here means
//! "visible to the one binary that includes this module", not crate-wide.
//! The scripted-`Llm` sub-agent fixture (only needed by the two
//! `assistant_session_subagent_*` binaries, not `assistant_session_base`)
//! lives in the sibling `scripted.rs`, included separately via `#[path]` so
//! `assistant_session_base` never compiles it and can't warn on it as unused.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

pub async fn test_db_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

pub async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let body = match body {
        Some(v) => Body::from(serde_json::to_vec(&v).unwrap()),
        None => Body::empty(),
    };
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("cookie", cookie)
        .body(body)
        .unwrap();
    let resp = app.clone().oneshot(req).await.expect("request failed");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body was not JSON: {e} (body: {})",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, value)
}
