//! Integration coverage for the session-cookie-protected `/api/tags/*` routes
//! (`src/routes/api/tags.rs`) and `src/repo/tags.rs` beneath them, plus the
//! `/api/pages` tag round-trip (pages carry tags as a plain `tag_ids: Vec<i32>`
//! column, see `src/entity/page.rs` — no join table) — real HTTP through
//! `tower::ServiceExt::oneshot`, real Postgres, no mocking (issue #27).
//!
//! Gated on `DATABASE_URL` per the repo convention (see `tests/policy_db.rs`'s
//! module doc): skipped, not failed, when unset. `site_test` isn't reset
//! between runs, so each test creates its own throwaway `users` row and
//! cleans up everything it inserted. Neither `tags` nor `pages` has an FK
//! between them (`tag_ids` is just an integer array), so a page referencing a
//! deleted tag would simply keep a dangling id — this test deletes the page
//! before the tag either way, since both were created by the test.

mod common;

use axum::Router;
use common::{send, test_db_url};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::{Value, json};
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
    };
    let state: AppState = state::create_state(&config).await;
    let db = state.db.clone();

    let username = format!("api-tags-{tag}-{}", uuid::Uuid::new_v4());
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
async fn create_list_read_update_and_delete_a_tag() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tag_name = format!("api-tag-{}", uuid::Uuid::new_v4());
    let fx = setup(&db_url, "lifecycle").await;

    let (status, created) = send(
        &fx.app,
        "POST",
        "/tags",
        &fx.cookie,
        Some(json!({ "name": tag_name, "description": "created by integration test" })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{created:?}");
    let tag_id = created["id"].as_i64().expect("id") as i32;
    assert_eq!(created["name"], tag_name);

    // Read.
    let (status, read_back) =
        send(&fx.app, "GET", &format!("/tags/{tag_id}"), &fx.cookie, None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(read_back["name"], tag_name);

    // List must include it.
    let (status, listed) = send(&fx.app, "GET", "/tags", &fx.cookie, None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let listed = listed.as_array().expect("tags list");
    assert!(
        listed
            .iter()
            .any(|t| t["id"].as_i64() == Some(tag_id as i64)),
        "created tag not present in list: {listed:?}"
    );

    // Update.
    let renamed = format!("{tag_name}-renamed");
    let (status, updated) = send(
        &fx.app,
        "PUT",
        &format!("/tags/{tag_id}"),
        &fx.cookie,
        Some(json!({ "name": renamed, "description": "renamed" })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{updated:?}");
    assert_eq!(updated["name"], renamed);

    // Delete.
    let (status, _) = send(
        &fx.app,
        "DELETE",
        &format!("/tags/{tag_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT);

    let (status, _) = send(&fx.app, "GET", &format!("/tags/{tag_id}"), &fx.cookie, None).await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn assigning_a_tag_to_a_page_round_trips_through_tag_ids() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let unique = uuid::Uuid::new_v4();
    let tag_name = format!("api-tag-page-{unique}");
    let fx = setup(&db_url, "page-assign").await;

    let (status, created_tag) = send(
        &fx.app,
        "POST",
        "/tags",
        &fx.cookie,
        Some(json!({ "name": tag_name })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{created_tag:?}");
    let tag_id = created_tag["id"].as_i64().expect("id") as i32;

    let page_path = format!("api-test/tags/page/{unique}");
    let (status, created_page) = send(
        &fx.app,
        "POST",
        "/pages",
        &fx.cookie,
        Some(json!({
            "path": page_path,
            "markdown": "tagged page",
            "tag_ids": [tag_id],
            "private": true,
        })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{created_page:?}");
    let page_id = created_page["id"].as_i64().expect("id") as i32;
    assert_eq!(
        created_page["tag_ids"].as_array().expect("tag_ids"),
        &vec![Value::from(tag_id)]
    );

    // Round-trip through a fresh read.
    let (status, read_back) = send(
        &fx.app,
        "GET",
        &format!("/pages/{page_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(
        read_back["tag_ids"].as_array().expect("tag_ids"),
        &vec![Value::from(tag_id)]
    );

    // Clean up: page first (it references the tag id, though there's no FK
    // enforcing it), then the tag itself.
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
        "DELETE",
        &format!("/tags/{tag_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT);

    cleanup_user(&fx.db, fx.user_id).await;
}
