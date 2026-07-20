//! Integration coverage for the session-cookie-protected `/api/pages/*`
//! routes (`src/routes/api/pages.rs`) and the `src/repo/pages.rs` +
//! `src/repo/pages_revisions.rs` layers they call into — real HTTP through
//! `tower::ServiceExt::oneshot`, real Postgres, no mocking (issue #27).
//!
//! Gated on `DATABASE_URL` per the repo convention (see `tests/policy_db.rs`'s
//! module doc): skipped with a message, not a failure, when unset, so
//! `cargo test`/`make verify` stays green without a live test DB. `site_test`
//! isn't reset between runs, so each test creates its own throwaway `users`
//! row (unique tag/uuid) and cleans up everything it inserted — page deletes
//! cascade to `page_revisions` (`on_delete = "Cascade"` in
//! `src/entity/page_revision.rs`), so only the page and the user need
//! explicit cleanup.

mod common;

use axum::Router;
use common::{send, test_db_url};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::json;
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{token, user};
use site::state::{self, AppState};

struct Fixture {
    app: Router,
    db: DatabaseConnection,
    cookie: String,
    user_id: i32,
}

async fn setup(db_url: &str, tag: &str) -> Fixture {
    let config = Config {
        database_url: db_url.to_string(),
        design_dir: None,
        serper_api_key: None,
        mdcast_pandoc_path: "pandoc".to_string(),
    };
    let state: AppState = state::create_state(&config).await;
    let db = state.db.clone();

    let username = format!("api-pages-{tag}-{}", uuid::Uuid::new_v4());
    let saved_user = user::ActiveModel {
        username: Set(username),
        password_hash: Set("unused".to_string()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway user");

    let nonce = site::auth::generate_token();
    token::ActiveModel {
        nonce: Set(nonce.clone()),
        user_id: Set(saved_user.id),
        expires_at: Set(None),
        label: Set(Some("test".to_string())),
        is_service: Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert session token");

    let app = site::routes::api::router(state.clone()).with_state(state);

    Fixture {
        app,
        db,
        cookie: format!("{SESSION_COOKIE}={nonce}"),
        user_id: saved_user.id,
    }
}

async fn cleanup_user(db: &DatabaseConnection, user_id: i32) {
    user::Entity::delete_by_id(user_id)
        .exec(db)
        .await
        .expect("delete throwaway user"); // cascades to tokens
}

#[tokio::test]
async fn full_page_lifecycle_create_read_update_revision_restore_delete() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tag = uuid::Uuid::new_v4();
    let fx = setup(&db_url, "lifecycle").await;
    let path = format!("api-test/pages/{tag}");

    // Create.
    let (status, created) = send(
        &fx.app,
        "POST",
        "/pages",
        &fx.cookie,
        Some(json!({
            "path": path,
            "summary": "initial summary",
            "markdown": "# v1",
            "tag_ids": [],
            "private": true,
        })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{created:?}");
    let page_id = created["id"].as_i64().expect("id") as i32;
    assert_eq!(created["path"], path);
    assert_eq!(created["private"], true);

    // Read back.
    let (status, detail) = send(
        &fx.app,
        "GET",
        &format!("/pages/{page_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(detail["markdown"], "# v1");
    assert_eq!(detail["revisions"].as_array().expect("revisions").len(), 0);

    // Update markdown -> creates a revision recording the old ("# v1") content.
    let (status, updated) = send(
        &fx.app,
        "PUT",
        &format!("/pages/{page_id}"),
        &fx.cookie,
        Some(json!({
            "path": path,
            "summary": "updated summary",
            "markdown": "# v2",
            "tag_ids": [],
            "private": true,
        })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{updated:?}");

    let (status, detail) = send(
        &fx.app,
        "GET",
        &format!("/pages/{page_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(detail["markdown"], "# v2");
    let revisions = detail["revisions"].as_array().expect("revisions");
    assert_eq!(
        revisions.len(),
        1,
        "expected exactly one revision: {detail:?}"
    );
    let rev_id = revisions[0]["id"].as_i64().expect("revision id") as i32;

    // Read the revision directly: it must reconstruct back to the pre-update content.
    let (status, rev_detail) = send(
        &fx.app,
        "GET",
        &format!("/pages/{page_id}/revisions/{rev_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(rev_detail["markdown"], "# v1");

    // Restore that revision -> current content reverts to "# v1".
    let (status, restored) = send(
        &fx.app,
        "POST",
        &format!("/pages/{page_id}/revisions/{rev_id}/restore"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{restored:?}");

    let (status, detail) = send(
        &fx.app,
        "GET",
        &format!("/pages/{page_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(detail["markdown"], "# v1");

    // Delete.
    let (status, _) = send(
        &fx.app,
        "DELETE",
        &format!("/pages/{page_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT);

    let (status, _) = send(
        &fx.app,
        "GET",
        &format!("/pages/{page_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn unauthenticated_request_is_rejected() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "unauth").await;

    // Bogus/nonexistent session nonce -> no matching token row -> 401.
    let bogus_cookie = format!("{SESSION_COOKIE}=this-nonce-does-not-exist");
    let (status, body) = send(&fx.app, "GET", "/pages", &bogus_cookie, None).await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED, "{body:?}");

    // No cookie at all -> also 401.
    let (status, body) = send(&fx.app, "GET", "/pages", "", None).await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED, "{body:?}");

    cleanup_user(&fx.db, fx.user_id).await;
}
