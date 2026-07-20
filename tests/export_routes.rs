//! Integration coverage for the export HTTP routes (#67): the public
//! `/{*path}?format=pdf|slides` route (`src/routes/public/export.rs`,
//! subsuming `catch_all`'s fallback for every non-root path) and the admin
//! `/api/export/pages/{id}?format=pdf|slides` route
//! (`src/routes/api/export.rs`). Exercises the real `export::render_page`
//! pipeline end to end — directive bridge (#66) -> mdcast split/classify ->
//! `Registry::render_to_bytes` against the `EmbeddedAssets`-fallback
//! provider (#67) — so a real render, not a stub.
//!
//! Gated on `DATABASE_URL` per this repo's convention (see
//! `tests/api_pages.rs`'s module doc): skipped with a message, not a
//! failure, when unset. Every test creates its own throwaway `users`/
//! `tokens`/`pages`/`menus` rows (unique path via uuid) and cleans up.

// Shared across many sibling test binaries; not every one uses every
// helper (this file skips `common::send`, which decodes bodies as JSON —
// unsuitable for the binary PDF/HTML bodies exercised here), same
// convention as `tests/oauth_authorize.rs`.
#[allow(dead_code)]
mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use bytes::Bytes;
use common::test_db_url;
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{menu, page, token, user};
use site::state::{self, AppState};
use tower::ServiceExt;

struct Fixture {
    state: AppState,
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

    let username = format!("export-routes-{tag}-{}", uuid::Uuid::new_v4());
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

    Fixture {
        state,
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

fn public_app(state: &AppState) -> Router {
    Router::new()
        .merge(site::routes::public::export::router())
        .fallback(axum::routing::get(site::routes::public::catch_all))
        .with_state(state.clone())
}

fn admin_app(state: &AppState) -> Router {
    site::routes::api::router(state.clone()).with_state(state.clone())
}

async fn raw_get(app: &Router, uri: &str, cookie: Option<&str>) -> (StatusCode, HeaderMap, Bytes) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    let req = builder.body(Body::empty()).expect("build request");
    let resp = app.clone().oneshot(req).await.expect("request failed");
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, bytes)
}

async fn insert_page(
    db: &DatabaseConnection,
    path: &str,
    markdown: &str,
    private: bool,
    user_id: i32,
) -> page::Model {
    let now = chrono::Utc::now().fixed_offset();
    page::ActiveModel {
        path: Set(path.to_string()),
        summary: Set(None),
        markdown: Set(markdown.to_string()),
        tag_ids: Set(vec![]),
        private: Set(private),
        created_at: Set(now),
        created_by: Set(user_id),
        modified_at: Set(now),
        modified_by: Set(user_id),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway page")
}

async fn delete_page(db: &DatabaseConnection, id: i32) {
    page::Entity::delete_by_id(id)
        .exec(db)
        .await
        .expect("delete throwaway page");
}

#[tokio::test]
async fn public_pdf_export_returns_a_real_pdf() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "pdf").await;
    let path = format!("export-routes-test/pdf-{}", uuid::Uuid::new_v4());
    let pg = insert_page(
        &fx.db,
        &path,
        "# Hello export\n\nSome body text.",
        false,
        fx.user_id,
    )
    .await;

    let app = public_app(&fx.state);
    let (status, headers, body) = raw_get(&app, &format!("/{path}?format=pdf"), None).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(headers.get("content-type").unwrap(), "application/pdf");
    assert!(
        headers
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    assert!(!body.is_empty());
    assert!(
        body.starts_with(b"%PDF"),
        "not a real PDF: {:?}",
        &body[..body.len().min(32)]
    );

    delete_page(&fx.db, pg.id).await;
    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn public_slides_export_succeeds_when_pandoc_available_and_503s_when_not() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "slides").await;
    let path = format!("export-routes-test/slides-{}", uuid::Uuid::new_v4());
    let pg = insert_page(
        &fx.db,
        &path,
        "# Slide deck\n\nSome content.",
        false,
        fx.user_id,
    )
    .await;

    if fx.state.pandoc_available {
        let app = public_app(&fx.state);
        let (status, headers, body) = raw_get(&app, &format!("/{path}?format=slides"), None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "body: {}",
            String::from_utf8_lossy(&body)
        );
        assert_eq!(
            headers.get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
        assert!(!body.is_empty());
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("<html"), "got: {text}");
    } else {
        eprintln!("pandoc unavailable in this environment; skipping the 200 branch");
    }

    // Deterministic 503 path regardless of whether pandoc happens to be
    // installed in this environment: force `pandoc_available` off.
    let mut state2 = fx.state.clone();
    state2.pandoc_available = false;
    let app2 = public_app(&state2);
    let (status, _headers, body) = raw_get(&app2, &format!("/{path}?format=slides"), None).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "body: {}",
        String::from_utf8_lossy(&body)
    );

    delete_page(&fx.db, pg.id).await;
    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn private_page_export_requires_login() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "private").await;
    let path = format!("export-routes-test/private-{}", uuid::Uuid::new_v4());
    let pg = insert_page(
        &fx.db,
        &path,
        "# Secret\n\nOnly for logged-in users.",
        true,
        fx.user_id,
    )
    .await;

    let app = public_app(&fx.state);

    let (status, _headers, _body) = raw_get(&app, &format!("/{path}?format=pdf"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, headers, body) =
        raw_get(&app, &format!("/{path}?format=pdf"), Some(&fx.cookie)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(headers.get("content-type").unwrap(), "application/pdf");
    assert!(body.starts_with(b"%PDF"));

    delete_page(&fx.db, pg.id).await;
    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn unknown_export_format_is_a_bad_request() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "badfmt").await;
    let path = format!("export-routes-test/badfmt-{}", uuid::Uuid::new_v4());
    let pg = insert_page(&fx.db, &path, "Body.", false, fx.user_id).await;

    let app = public_app(&fx.state);
    let (status, _headers, _body) = raw_get(&app, &format!("/{path}?format=nonsense"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    delete_page(&fx.db, pg.id).await;
    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn plain_request_with_no_format_param_still_renders_the_normal_html_page() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "passthrough").await;
    let path = format!("export-routes-test/passthrough-{}", uuid::Uuid::new_v4());
    let marker = format!("passthrough-marker-{}", uuid::Uuid::new_v4());
    let pg = insert_page(
        &fx.db,
        &path,
        &format!("# Title\n\n{marker}"),
        false,
        fx.user_id,
    )
    .await;

    let app = public_app(&fx.state);
    let (status, headers, body) = raw_get(&app, &format!("/{path}"), None).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
    let content_type = headers.get("content-type").unwrap().to_str().unwrap();
    assert!(content_type.contains("text/html"), "got: {content_type}");
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains(&marker), "got: {text}");

    delete_page(&fx.db, pg.id).await;
    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn menu_item_export_and_passthrough_both_work_through_lookup_content() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "menu").await;
    let path = format!("export-routes-test/menu-{}", uuid::Uuid::new_v4());
    let saved_menu = menu::ActiveModel {
        title: Set("Export Menu Item".to_string()),
        path: Set(path.clone()),
        markdown: Set("# Menu page\n\nMenu body.".to_string()),
        order_index: Set(0),
        private: Set(false),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("insert throwaway menu item");

    let app = public_app(&fx.state);
    let (status, headers, body) = raw_get(&app, &format!("/{path}?format=pdf"), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(headers.get("content-type").unwrap(), "application/pdf");
    assert!(body.starts_with(b"%PDF"));

    menu::Entity::delete_by_id(saved_menu.id)
        .exec(&fx.db)
        .await
        .expect("delete throwaway menu item");
    cleanup_user(&fx.db, fx.user_id).await;
}

#[tokio::test]
async fn admin_pdf_export_is_gated_by_login_and_validates_input() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "admin").await;
    let path = format!("export-routes-test/admin-{}", uuid::Uuid::new_v4());
    let pg = insert_page(
        &fx.db,
        &path,
        "# Admin export\n\nBody text.",
        false,
        fx.user_id,
    )
    .await;

    let app = admin_app(&fx.state);
    let uri = format!("/export/pages/{}?format=pdf", pg.id);

    // No cookie -> require_login_api rejects with 401.
    let (status, _headers, _body) = raw_get(&app, &uri, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Valid cookie -> real PDF.
    let (status, headers, body) = raw_get(&app, &uri, Some(&fx.cookie)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(headers.get("content-type").unwrap(), "application/pdf");
    assert!(body.starts_with(b"%PDF"));

    // Bad format -> 400.
    let (status, _headers, _body) = raw_get(
        &app,
        &format!("/export/pages/{}?format=nonsense", pg.id),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Unknown page id -> 404.
    let (status, _headers, _body) = raw_get(
        &app,
        "/export/pages/2147483647?format=pdf",
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    delete_page(&fx.db, pg.id).await;
    cleanup_user(&fx.db, fx.user_id).await;
}
