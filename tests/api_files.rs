//! Integration coverage for the session-cookie-protected `/api/files/*`
//! routes (`src/routes/api/files.rs`) and `src/repo/files.rs` / `src/files.rs`
//! beneath them — real HTTP through `tower::ServiceExt::oneshot`, real
//! Postgres, no mocking (issue #27).
//!
//! Gated on `DATABASE_URL` per the repo convention (see `tests/policy_db.rs`'s
//! module doc): skipped, not failed, when unset. `site_test` isn't reset
//! between runs, so each test creates its own throwaway `users` row and
//! cleans up everything it inserted. Unlike pages, a deleted `files` row does
//! *not* cascade to `file_blobs` (content-addressed and deliberately shared
//! across files, see `src/files.rs`) or `file_thumbnails` (no FK at all, see
//! `src/entity/file_thumbnail.rs`), so tests delete those rows explicitly.
//!
//! `send` from `tests/common/mod.rs` only builds JSON request bodies, so file
//! upload (`multipart/form-data`) needs its own tiny request builder — kept
//! local to this file rather than added to the shared helper, per the task's
//! "build it locally" guidance: `reqwest`'s `multipart` feature isn't enabled
//! in `Cargo.toml`, and the upload handler's exact field names (`path`,
//! `description`, `file`) are simple enough to hand-encode.

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{send, test_db_url};
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::Value;
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{file_blob, file_thumbnail, token, user};
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
    };
    let state: AppState = state::create_state(&config).await;
    let db = state.db.clone();

    let username = format!("api-files-{tag}-{}", uuid::Uuid::new_v4());
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

/// Encode a minimal `multipart/form-data` body for the upload handler's three
/// possible fields (`path`, `description`, `file`) — hand-rolled since
/// `reqwest`'s `multipart` feature isn't enabled and pulling in a new
/// dev-dependency is out of scope for this change.
fn multipart_body(
    path: &str,
    description: Option<&str>,
    filename: &str,
    data: &[u8],
) -> (String, Vec<u8>) {
    let boundary = format!("testboundary-{}", uuid::Uuid::new_v4());
    let mut body = Vec::new();

    let push_text_field = |body: &mut Vec<u8>, name: &str, value: &str| {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    };

    push_text_field(&mut body, "path", path);
    if let Some(desc) = description {
        push_text_field(&mut body, "description", desc);
    }

    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: text/plain\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn upload(app: &Router, cookie: &str, path: &str, data: &[u8]) -> (StatusCode, Value) {
    let (content_type, body) = multipart_body(path, Some("test upload"), "test.txt", data);
    let req = Request::builder()
        .method("POST")
        .uri("/files")
        .header("cookie", cookie)
        .header("content-type", content_type)
        .body(Body::from(body))
        .expect("build multipart request");
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

async fn delete_blob_if_unreferenced(db: &DatabaseConnection, hash: &str) {
    // Best-effort cleanup: only delete the blob if nothing else in the shared
    // `site_test` DB still references this content-addressed hash.
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
async fn upload_read_and_delete_a_file() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tag = uuid::Uuid::new_v4();
    let fx = setup(&db_url, "upload").await;
    let path = format!("api-test/files/{tag}");
    let content = format!("hello from {tag}");

    let (status, created) = upload(&fx.app, &fx.cookie, &path, content.as_bytes()).await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let file_id = created["id"].as_i64().expect("id") as i32;
    let hash = created["hash"].as_str().expect("hash").to_string();
    assert_eq!(created["path"], path);
    assert_eq!(created["mimetype"], "text/plain");
    assert_eq!(created["size_bytes"].as_i64(), Some(content.len() as i64));

    let (status, read_back) = send(
        &fx.app,
        "GET",
        &format!("/files/{file_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read_back["hash"], hash);
    assert_eq!(read_back["path"], path);

    let (status, _) = send(
        &fx.app,
        "DELETE",
        &format!("/files/{file_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = send(
        &fx.app,
        "GET",
        &format!("/files/{file_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    delete_blob_if_unreferenced(&fx.db, &hash).await;
    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn identical_content_under_different_paths_dedups_to_the_same_blob() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tag = uuid::Uuid::new_v4();
    let fx = setup(&db_url, "dedup").await;
    let content = format!("shared content {tag}");

    let path_a = format!("api-test/files/dedup-a/{tag}");
    let path_b = format!("api-test/files/dedup-b/{tag}");

    let (status_a, created_a) = upload(&fx.app, &fx.cookie, &path_a, content.as_bytes()).await;
    assert_eq!(status_a, StatusCode::CREATED, "{created_a:?}");
    let (status_b, created_b) = upload(&fx.app, &fx.cookie, &path_b, content.as_bytes()).await;
    assert_eq!(status_b, StatusCode::CREATED, "{created_b:?}");

    let file_id_a = created_a["id"].as_i64().expect("id") as i32;
    let file_id_b = created_b["id"].as_i64().expect("id") as i32;
    let hash_a = created_a["hash"].as_str().expect("hash").to_string();
    let hash_b = created_b["hash"].as_str().expect("hash").to_string();
    assert_eq!(hash_a, hash_b, "identical content must hash identically");
    assert_ne!(
        file_id_a, file_id_b,
        "each upload is still its own file row"
    );

    // Exactly one `file_blobs` row backs both `files` rows (dedup by hash, not two blobs).
    let blob_count = file_blob::Entity::find_by_id(hash_a.clone())
        .all(&fx.db)
        .await
        .expect("query file_blobs")
        .len();
    assert_eq!(blob_count, 1, "expected a single deduped file_blobs row");

    let (status, _) = send(
        &fx.app,
        "DELETE",
        &format!("/files/{file_id_a}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &fx.app,
        "DELETE",
        &format!("/files/{file_id_b}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    delete_blob_if_unreferenced(&fx.db, &hash_a).await;
    cleanup_user(&fx.db, fx.user_id).await;
}

/// Sanity check that a plain-text upload (used by every test in this file, to
/// keep fixtures simple) never triggers thumbnail generation — so tests don't
/// need to clean up `file_thumbnails` rows they never created. Guards the
/// assumption other tests in this file rely on if `make_thumbnail`'s
/// image-only gate (`src/files.rs`) ever changes.
#[tokio::test]
async fn text_upload_does_not_create_a_thumbnail() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tag = uuid::Uuid::new_v4();
    let fx = setup(&db_url, "no-thumb").await;
    let path = format!("api-test/files/no-thumb/{tag}");

    let (status, created) = upload(&fx.app, &fx.cookie, &path, b"just text, not an image").await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let file_id = created["id"].as_i64().expect("id") as i32;
    let hash = created["hash"].as_str().expect("hash").to_string();
    assert_eq!(created["has_thumbnail"], false);

    let has_thumb_row = file_thumbnail::Entity::find_by_id(file_id)
        .one(&fx.db)
        .await
        .expect("query file_thumbnails")
        .is_some();
    assert!(!has_thumb_row);

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
