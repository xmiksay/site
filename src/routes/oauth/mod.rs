//! OAuth2 authorization server (PKCE, RFC 7591 dynamic client registration).
//!
//! Split along the natural seam in this endpoint: [`handlers`] holds the
//! interactive-flow Axum handlers (authorize + token dispatch), [`metadata`]
//! holds the well-known discovery/registration endpoints, and [`security`]
//! holds the DB/PKCE/token-issuance logic those handlers delegate to (kept
//! free of Axum extractors so it can be unit tested directly).

use axum::Router;
use axum::http::HeaderMap;
use axum::routing::{get, post};

use crate::state::AppState;

mod handlers;
mod metadata;
mod security;

pub use security::authenticate_mcp;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/oauth/register", post(metadata::register))
        .route(
            "/oauth/authorize",
            get(handlers::authorize_form).post(handlers::authorize_submit),
        )
        .route("/oauth/token", post(handlers::token))
        .route(
            "/.well-known/oauth-protected-resource",
            get(metadata::protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(metadata::authorization_server_metadata),
        )
}

/// Return the base URL from the Host header or fall back to localhost.
pub(super) fn base_url(headers: &HeaderMap) -> String {
    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        let scheme = if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
            "http"
        } else {
            "https"
        };
        format!("{scheme}://{host}")
    } else {
        "http://localhost:3000".to_string()
    }
}
