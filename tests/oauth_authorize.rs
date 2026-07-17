//! Integration coverage for `GET`/`POST /oauth/authorize`
//! (`src/routes/oauth/handlers.rs`'s `authorize_form`/`authorize_submit`):
//! parameter validation and the login-submit -> authorization-code redirect.
//! Token-endpoint scenarios live in the sibling `tests/oauth_token.rs` and
//! `tests/oauth_refresh.rs`; shared fixture/HTTP helpers live in
//! `tests/common/oauth.rs` (see its module doc for the full setup/cleanup
//! convention).
//!
//! Gated on `DATABASE_URL` per the repo convention (see `tests/policy_db.rs`)
//! — skips with a message, not a failure, when unset.

// `common`/`oauth_helpers` are shared across three sibling binaries
// (`oauth_authorize`, `oauth_token`, `oauth_refresh`); no single one of them
// uses every helper, so each compiles with some intentionally-unused ones.
#[allow(dead_code)]
mod common;
#[allow(dead_code)]
#[path = "common/oauth.rs"]
mod oauth_helpers;

use axum::http::StatusCode;
use common::test_db_url;
use oauth_helpers::{authorize_query, cleanup, get_uri, issue_code, pkce_pair, post_form, setup};

#[tokio::test]
async fn authorize_get_valid_params_renders_login_form() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "get-valid").await;
    let (_, challenge) = pkce_pair();

    let uri = authorize_query(&f.client_id, &f.redirect_uri, &challenge, "S256");
    let (status, _headers, body) = get_uri(&f.app, &uri).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<form"), "expected a login form: {body}");

    cleanup(&f).await;
}

#[tokio::test]
async fn authorize_get_rejects_invalid_params() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "get-invalid").await;
    let (_, challenge) = pkce_pair();

    // Unknown client_id.
    let uri = authorize_query("no-such-client", &f.redirect_uri, &challenge, "S256");
    let (status, _, _) = get_uri(&f.app, &uri).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown client_id");

    // Unregistered redirect_uri.
    let uri = authorize_query(
        &f.client_id,
        "https://evil.example/callback",
        &challenge,
        "S256",
    );
    let (status, _, _) = get_uri(&f.app, &uri).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unregistered redirect_uri");

    // Unsupported code_challenge_method.
    let uri = authorize_query(&f.client_id, &f.redirect_uri, &challenge, "plain");
    let (status, _, _) = get_uri(&f.app, &uri).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unsupported code_challenge_method"
    );

    cleanup(&f).await;
}

#[tokio::test]
async fn authorize_submit_correct_credentials_redirects_with_code() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "submit-ok").await;
    let (_, challenge) = pkce_pair();

    let code = issue_code(&f, &challenge).await;
    assert!(!code.is_empty());

    cleanup(&f).await;
}

#[tokio::test]
async fn authorize_submit_wrong_password_redirects_with_error() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "submit-bad-pw").await;
    let (_, challenge) = pkce_pair();

    let (status, headers, body) = post_form(
        &f.app,
        "/oauth/authorize",
        &[
            ("client_id", f.client_id.as_str()),
            ("redirect_uri", f.redirect_uri.as_str()),
            ("response_type", "code"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("username", f.username.as_str()),
            ("password", "wrong password"),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::SEE_OTHER, "still redirects: {body}");
    let location = headers
        .get("location")
        .expect("Location header on redirect")
        .to_str()
        .expect("Location header is valid utf8");
    let url = url::Url::parse(location).expect("Location is a valid absolute URL");
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert!(
        pairs
            .iter()
            .any(|(k, v)| k == "error" && v == "access_denied"),
        "expected error=access_denied in {location}"
    );
    assert!(
        !pairs.iter().any(|(k, _)| k == "code"),
        "no code should be issued on failed auth: {location}"
    );

    cleanup(&f).await;
}
