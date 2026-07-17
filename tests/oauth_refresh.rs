//! Integration coverage for `POST /oauth/token` with
//! `grant_type=refresh_token` (`src/routes/oauth/security.rs`'s `refresh`):
//! rotation (old refresh token revoked, new access/refresh pair issued) and
//! rejection of an unknown/already-revoked token. The `authorization_code`
//! grant and authorize-endpoint scenarios live in the sibling
//! `tests/oauth_token.rs`/`tests/oauth_authorize.rs`; shared fixture/HTTP
//! helpers in `tests/common/oauth.rs`.
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
use oauth_helpers::{cleanup, issue_code, pkce_pair, post_form_json, setup};

#[tokio::test]
async fn refresh_token_rotates_and_revokes_old_token() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "refresh-ok").await;
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
    let old_refresh = value["refresh_token"]
        .as_str()
        .expect("refresh_token present")
        .to_string();

    let (status, value) = post_form_json(
        &f.app,
        "/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", old_refresh.as_str()),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {value}");
    let new_access = value["access_token"]
        .as_str()
        .expect("access_token present");
    let new_refresh = value["refresh_token"]
        .as_str()
        .expect("refresh_token present");
    assert!(!new_access.is_empty());
    assert_ne!(
        new_refresh, old_refresh,
        "refresh must rotate to a new value"
    );

    // The rotated-out refresh token must no longer be usable.
    let (status, value) = post_form_json(
        &f.app,
        "/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", old_refresh.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "old refresh token reused: {value}"
    );
    assert_eq!(value["error"], "invalid_grant");

    cleanup(&f).await;
}

#[tokio::test]
async fn refresh_token_rejects_unknown_token() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let f = setup(&db_url, "refresh-unknown").await;

    let (status, value) = post_form_json(
        &f.app,
        "/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", "totally-made-up-refresh-token"),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"], "invalid_grant");

    cleanup(&f).await;
}
