//! Shared fixture + HTTP helpers for the OAuth2 PKCE flow integration tests
//! (`tests/oauth_authorize.rs`, `tests/oauth_token.rs`,
//! `tests/oauth_refresh.rs` — split out of a single ~600-line file to respect
//! the repo's 400-line cap, one file per endpoint's scenarios). Included via
//! `#[path = "common/oauth.rs"]` in each of those, the same way
//! `tests/common/scripted.rs` is included only where needed, rather than
//! being declared in `common/mod.rs` directly (so no other integration test
//! binary pays for or warns on unused oauth helpers).
//!
//! Drives the real router end to end via `tower::ServiceExt::oneshot`
//! against the real `site_test` Postgres — no `src/routes/oauth` handler
//! functions are called directly. `security.rs` already unit-tests
//! `verify_pkce`/`is_code_valid` as pure logic (no DB); these tests cover the
//! DB/HTTP-integration layer around them.
//!
//! Each test creates its own throwaway `users` row (cascades
//! `oauth_codes`/`oauth_tokens` on delete) and its own `oauth_clients` row,
//! which is *not* user-scoped, so [`cleanup`] deletes it explicitly — same
//! per-test-tag isolation convention as `tests/policy_db.rs` (`site_test`
//! isn't reset between runs).

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde_json::Value;
use sha2::{Digest, Sha256};
use site::config::Config;
use site::entity::{oauth_client, user};
use site::state::{self, AppState};
use tower::ServiceExt;

/// Mirrors the private `ACCESS_TOKEN_HOURS` constant in
/// `src/routes/oauth/security.rs` (1h). That module isn't `pub`, so it can't
/// be imported across the integration-test crate boundary — duplicated here
/// deliberately rather than hardcoding `3600` with no explanation; a drift
/// would show up as a failure in `oauth_token.rs`'s expiry assertion.
pub const ACCESS_TOKEN_HOURS: i64 = 1;

pub struct Fixture {
    pub app: Router,
    pub db: DatabaseConnection,
    pub user_id: i32,
    pub username: String,
    pub password: String,
    pub client_id: String,
    pub redirect_uri: String,
}

pub async fn setup(db_url: &str, tag: &str) -> Fixture {
    let config = Config {
        database_url: db_url.to_string(),
        design_dir: None,
        serper_api_key: None,
    };
    let state: AppState = state::create_state(&config).await;
    let db = state.db.clone();

    let username = format!("oauth-{tag}-{}", uuid::Uuid::new_v4());
    let password = "correct horse battery staple".to_string();
    let saved_user = user::ActiveModel {
        username: Set(username.clone()),
        password_hash: Set(site::auth::hash_password(&password)),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway user");

    let client_id = format!("oauth-client-{tag}-{}", uuid::Uuid::new_v4());
    let redirect_uri = "https://client.example/callback".to_string();
    oauth_client::ActiveModel {
        client_id: Set(client_id.clone()),
        client_secret: Set(None),
        client_name: Set(Some(format!("test client {tag}"))),
        redirect_uris: Set(serde_json::json!([redirect_uri])),
        created_at: Set(chrono::Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway oauth_client");

    let app = site::routes::oauth::router().with_state(state.clone());

    Fixture {
        app,
        db,
        user_id: saved_user.id,
        username,
        password,
        client_id,
        redirect_uri,
    }
}

/// `oauth_clients` rows aren't scoped to a user and don't cascade — clean
/// up explicitly. Deleting the user cascades its `oauth_codes`/`oauth_tokens`.
pub async fn cleanup(f: &Fixture) {
    oauth_client::Entity::delete_many()
        .filter(oauth_client::Column::ClientId.eq(f.client_id.as_str()))
        .exec(&f.db)
        .await
        .expect("delete throwaway oauth_client");
    user::Entity::delete_by_id(f.user_id)
        .exec(&f.db)
        .await
        .expect("delete throwaway user");
}

pub fn pkce_pair() -> (String, String) {
    let verifier = format!("verifier-{}", uuid::Uuid::new_v4());
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());
    (verifier, challenge)
}

// ---------------------------------------------------------------------------
// HTTP helpers (Form-encoded, not JSON — `tests/common::send` doesn't fit)
// ---------------------------------------------------------------------------

pub async fn raw(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let resp = app.clone().oneshot(req).await.expect("request failed");
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect response body")
        .to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

pub async fn get_uri(app: &Router, uri: &str) -> (StatusCode, HeaderMap, String) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .expect("build GET request");
    raw(app, req).await
}

pub async fn post_form(
    app: &Router,
    uri: &str,
    pairs: &[(&str, &str)],
) -> (StatusCode, HeaderMap, String) {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .expect("build POST request");
    raw(app, req).await
}

pub async fn post_form_json(
    app: &Router,
    uri: &str,
    pairs: &[(&str, &str)],
) -> (StatusCode, Value) {
    let (status, _headers, body) = post_form(app, uri, pairs).await;
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("response body was not JSON: {e} (body: {body})"))
    };
    (status, value)
}

pub fn authorize_query(
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    method: &str,
) -> String {
    let qs = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", method)
        .finish();
    format!("/oauth/authorize?{qs}")
}

/// Drive a full authorize round trip (GET form sanity-check + POST submit
/// with the fixture's real credentials) and pull the `code` out of the
/// redirect's `Location` header.
pub async fn issue_code(f: &Fixture, challenge: &str) -> String {
    let uri = authorize_query(&f.client_id, &f.redirect_uri, challenge, "S256");
    let (status, _headers, body) = get_uri(&f.app, &uri).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "authorize form should render: {body}"
    );

    let (status, headers, body) = post_form(
        &f.app,
        "/oauth/authorize",
        &[
            ("client_id", f.client_id.as_str()),
            ("redirect_uri", f.redirect_uri.as_str()),
            ("response_type", "code"),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("username", f.username.as_str()),
            ("password", f.password.as_str()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "expected redirect: {body}");
    let location = headers
        .get("location")
        .expect("Location header on redirect")
        .to_str()
        .expect("Location header is valid utf8");
    let url = url::Url::parse(location).expect("Location is a valid absolute URL");
    url.query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
        .expect("redirect carries a code query param")
}
