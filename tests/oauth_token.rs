//! Integration coverage for `POST /oauth/token` with `grant_type=
//! authorization_code` (`src/routes/oauth/security.rs`'s `exchange_code`):
//! happy path + expiry, PKCE mismatch, single-use enforcement, and
//! `redirect_uri`/parameter validation. The `refresh_token` grant lives in
//! the sibling `tests/oauth_refresh.rs`; authorize-endpoint scenarios in
//! `tests/oauth_authorize.rs`; shared fixture/HTTP helpers in
//! `tests/common/oauth.rs`.
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
use oauth_helpers::{ACCESS_TOKEN_HOURS, cleanup, issue_code, pkce_pair, post_form_json, setup};
use sea_orm::{ActiveModelTrait, Set};
use site::entity::oauth_code;

#[tokio::test]
async fn token_exchange_issues_correct_expiry() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "exchange-ok").await;
    let (verifier, challenge) = pkce_pair();
    let code = issue_code(&f, &challenge).await;

    let (status, value) = post_form_json(
        &f.app,
        "/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", f.redirect_uri.as_str()),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {value}");
    assert!(
        value["access_token"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(
        value["refresh_token"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert_eq!(value["token_type"], "Bearer");
    assert_eq!(value["expires_in"], ACCESS_TOKEN_HOURS * 3600);

    cleanup(&f).await;
}

#[tokio::test]
async fn token_exchange_rejects_wrong_verifier() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "exchange-badv").await;
    let (_correct_verifier, challenge) = pkce_pair();
    let code = issue_code(&f, &challenge).await;

    let (status, value) = post_form_json(
        &f.app,
        "/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("code_verifier", "not-the-right-verifier"),
            ("redirect_uri", f.redirect_uri.as_str()),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"], "invalid_grant");

    cleanup(&f).await;
}

#[tokio::test]
async fn token_exchange_rejects_reused_code() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "exchange-reuse").await;
    let (verifier, challenge) = pkce_pair();
    let code = issue_code(&f, &challenge).await;

    let pairs = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("code_verifier", verifier.as_str()),
        ("redirect_uri", f.redirect_uri.as_str()),
    ];
    let (first_status, first_value) = post_form_json(&f.app, "/oauth/token", &pairs).await;
    assert_eq!(first_status, StatusCode::OK, "first use: {first_value}");

    let (second_status, second_value) = post_form_json(&f.app, "/oauth/token", &pairs).await;
    assert_eq!(second_status, StatusCode::BAD_REQUEST, "reuse must fail");
    assert_eq!(second_value["error"], "invalid_grant");

    cleanup(&f).await;
}

#[tokio::test]
async fn token_exchange_rejects_expired_code() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "exchange-expired").await;
    let (verifier, challenge) = pkce_pair();

    // Insert the code directly, already expired, instead of waiting out the
    // real 10-minute TTL (`CODE_MINUTES` in `handlers.rs`).
    let code = format!("expired-code-{}", uuid::Uuid::new_v4());
    let past = chrono::Utc::now().fixed_offset() - chrono::Duration::minutes(5);
    oauth_code::ActiveModel {
        code: Set(code.clone()),
        client_id: Set(f.client_id.clone()),
        user_id: Set(f.user_id),
        redirect_uri: Set(f.redirect_uri.clone()),
        code_challenge: Set(challenge),
        expires_at: Set(past),
        used: Set(false),
        ..Default::default()
    }
    .insert(&f.db)
    .await
    .expect("insert expired oauth_code");

    let (status, value) = post_form_json(
        &f.app,
        "/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", f.redirect_uri.as_str()),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"], "invalid_grant");

    cleanup(&f).await;
}

#[tokio::test]
async fn token_exchange_rejects_mismatched_redirect_uri() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "exchange-badredirect").await;
    let (verifier, challenge) = pkce_pair();
    let code = issue_code(&f, &challenge).await;

    let (status, value) = post_form_json(
        &f.app,
        "/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", "https://different.example/callback"),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"], "invalid_grant");

    cleanup(&f).await;
}

#[tokio::test]
async fn token_exchange_rejects_missing_code_or_verifier() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "exchange-missing").await;
    let (verifier, challenge) = pkce_pair();
    let code = issue_code(&f, &challenge).await;

    let (status, value) = post_form_json(
        &f.app,
        "/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", f.redirect_uri.as_str()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "missing code");
    assert_eq!(value["error"], "invalid_request");

    let (status, value) = post_form_json(
        &f.app,
        "/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", f.redirect_uri.as_str()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "missing code_verifier");
    assert_eq!(value["error"], "invalid_request");

    cleanup(&f).await;
}
