//! Integration coverage for the session-cookie-protected `/api/galleries/*`
//! routes (`src/routes/api/galleries.rs`) and `src/repo/galleries.rs`
//! beneath them — real HTTP through `tower::ServiceExt::oneshot`, real
//! Postgres, no mocking (issue #27).
//!
//! Gated on `DATABASE_URL` per the repo convention (see `tests/policy_db.rs`'s
//! module doc): skipped, not failed, when unset. `site_test` isn't reset
//! between runs, so each test creates its own throwaway `users` row and
//! cleans up everything it inserted. `galleries.file_ids` (see
//! `src/entity/gallery.rs`) is a plain `Vec<i32>` column with no FK to
//! `files`, so associating a file is just passing its id in — deleting the
//! gallery doesn't touch the file row, and vice versa, so both are cleaned up
//! independently.

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{send, test_db_url};
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::{Value, json};
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{file_blob, token, user};
use site::state::{self, AppState};
use tower::ServiceExt;

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

    let username = format!("api-galleries-{tag}-{}", uuid::Uuid::new_v4());
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

/// Minimal hand-rolled `multipart/form-data` upload, same shape as
/// `tests/api_files.rs`'s helper (kept local rather than shared since
/// `tests/common/mod.rs` is out of scope for this change and this file only
/// needs one file to associate with a gallery).
async fn upload_file(app: &Router, cookie: &str, path: &str, data: &[u8]) -> (StatusCode, Value) {
    let boundary = format!("testboundary-{}", uuid::Uuid::new_v4());
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\n{path}\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method("POST")
        .uri("/files")
        .header("cookie", cookie)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("build multipart request");
    let resp = app.clone().oneshot(req).await.expect("request failed");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "response body was not JSON: {e} (body: {})",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, value)
}

async fn delete_blob_if_unreferenced(db: &DatabaseConnection, hash: &str) {
    use sea_orm::{ColumnTrait, QueryFilter};
    let still_referenced = site::entity::file::Entity::find()
        .filter(site::entity::file::Column::Hash.eq(hash))
        .one(db)
        .await
        .expect("query files by hash")
        .is_some();
    if !still_referenced {
        file_blob::Entity::delete_by_id(hash.to_string())
            .exec(db)
            .await
            .expect("delete file_blob");
    }
}

#[tokio::test]
async fn create_associate_file_read_and_delete_a_gallery() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tag = uuid::Uuid::new_v4();
    let fx = setup(&db_url, "lifecycle").await;

    let file_path = format!("api-test/galleries/file/{tag}");
    let (status, uploaded) =
        upload_file(&fx.app, &fx.cookie, &file_path, b"gallery image stand-in").await;
    assert_eq!(status, StatusCode::CREATED, "{uploaded:?}");
    let file_id = uploaded["id"].as_i64().expect("file id") as i32;
    let hash = uploaded["hash"].as_str().expect("hash").to_string();

    let gallery_path = format!("api-test/galleries/{tag}");
    let (status, created) = send(
        &fx.app,
        "POST",
        "/galleries",
        &fx.cookie,
        Some(json!({
            "path": gallery_path,
            "title": "Test Gallery",
            "description": "created by api_galleries integration test",
            "file_ids": [file_id],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let gallery_id = created["id"].as_i64().expect("id") as i32;
    assert_eq!(created["path"], gallery_path);
    assert_eq!(created["title"], "Test Gallery");
    assert_eq!(
        created["file_ids"].as_array().expect("file_ids"),
        &vec![Value::from(file_id)]
    );

    // Read back.
    let (status, read_back) = send(
        &fx.app,
        "GET",
        &format!("/galleries/{gallery_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        read_back["file_ids"].as_array().expect("file_ids"),
        &vec![Value::from(file_id)]
    );

    // Update: rename and drop the file association.
    let (status, updated) = send(
        &fx.app,
        "PUT",
        &format!("/galleries/{gallery_id}"),
        &fx.cookie,
        Some(json!({
            "path": gallery_path,
            "title": "Renamed Gallery",
            "file_ids": [],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated:?}");
    assert_eq!(updated["title"], "Renamed Gallery");
    assert_eq!(updated["file_ids"].as_array().expect("file_ids").len(), 0);

    // Delete.
    let (status, _) = send(
        &fx.app,
        "DELETE",
        &format!("/galleries/{gallery_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = send(
        &fx.app,
        "GET",
        &format!("/galleries/{gallery_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Clean up the file the gallery referenced (not cascaded).
    let (status, _) = send(
        &fx.app,
        "DELETE",
        &format!("/files/{file_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    delete_blob_if_unreferenced(&fx.db, &hash).await;

    cleanup_user(&fx.db, fx.user_id).await;
}
