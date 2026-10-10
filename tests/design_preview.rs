//! Draft preview mode (#116) over a full `AppState` on fs storage: the
//! `design_preview` cookie switches public pages and `/assets/*` to the draft
//! only together with a valid session; preview responses carry the banner and
//! `no-store`; a broken draft template shows its name and line; preview off
//! restores the published design.
//!
//! Gated on `DATABASE_URL` like every DB test.

// Shared with tests/storage.rs; not every helper is used here.
#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::routing::get;
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set};
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{page, token, user};
use site::routes::public;
use storage_fixture::TestStorage;
use tower::ServiceExt;

const BANNER: &str = "NÁHLED DRAFTU";
const PREVIEW: &str = "design_preview=1";

struct Response {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Response {
    fn cache_control(&self) -> &str {
        self.headers
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }

    fn is_preview(&self) -> bool {
        self.body.contains(BANNER) || self.cache_control() == "no-store"
    }
}

async fn call(app: &Router, method: &str, uri: &str, cookie: &str, body: &str) -> Response {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::REFERER, "http://localhost/back/here?x=1")
        .body(Body::from(body.to_string()))
        .expect("request");
    let resp = app.clone().oneshot(req).await.expect("response");
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    Response {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

#[tokio::test]
async fn draft_preview_is_admin_only_and_marked() {
    let Ok(db_url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let db = Database::connect(&db_url).await.expect("connect");
    let ts = TestStorage::fs(&db);
    let config = Config {
        database_url: db_url,
        design_dir: None,
        serper_api_key: None,
        mdcast_url: None,
        mdcast_token: None,
        storage: ts.fs_config(),
    };
    let state = site::state::create_state(&config).await;
    let app = Router::new()
        .nest("/api", site::routes::api::router(state.clone()))
        .route("/assets/{*path}", get(public::assets::serve))
        .merge(public::export::router())
        .fallback(get(public::catch_all))
        .with_state(state);

    let admin = user::ActiveModel {
        username: Set(format!("preview-{}", uuid::Uuid::new_v4())),
        password_hash: Set("unused".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert user");
    let nonce = site::auth::generate_token();
    token::ActiveModel {
        nonce: Set(nonce.clone()),
        user_id: Set(admin.id),
        expires_at: Set(None),
        label: Set(Some("test".into())),
        is_service: Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert session token");
    let session = format!("{SESSION_COOKIE}={nonce}");
    let both = format!("{session}; {PREVIEW}");
    let now = chrono::Utc::now().fixed_offset();
    let page_path = format!("preview-test-{}", uuid::Uuid::new_v4());
    let pg = page::ActiveModel {
        path: Set(page_path.clone()),
        summary: Set(None),
        markdown: Set("page body".into()),
        tag_ids: Set(vec![]),
        private: Set(false),
        created_at: Set(now),
        created_by: Set(admin.id),
        modified_at: Set(now),
        modified_by: Set(admin.id),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert page");
    let url = format!("/{page_path}");
    let css = "/assets/css/style.css";

    let published = call(&app, "GET", &url, &session, "").await;
    let anon_published = call(&app, "GET", &url, "", "").await;
    let published_css = call(&app, "GET", css, "", "").await;
    assert_eq!(published.status, StatusCode::OK);
    assert!(published.body.contains("page body") && !published.is_preview());
    assert_eq!(published_css.headers[header::VARY], "Cookie");

    // Preview on: the cookie is set as the issue specifies.
    let on = call(
        &app,
        "POST",
        "/api/design/preview",
        &session,
        r#"{"on":true}"#,
    )
    .await;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    let set = on.headers[header::SET_COOKIE].to_str().expect("ascii");
    for part in [PREVIEW, "HttpOnly", "SameSite=Lax", "Path=/"] {
        assert!(set.contains(part), "{set}");
    }
    // Anonymous requests cannot switch preview on.
    let anon = call(&app, "POST", "/api/design/preview", "", r#"{"on":true}"#).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);

    // An uninitialized draft previews as the published design.
    let untouched = call(&app, "GET", &url, &both, "").await;
    assert!(untouched.body.contains("page body") && untouched.body.contains(BANNER));

    let draft_page = "<html><body class=\"d\">DRAFT-PAGE {{ body_html }}</body></html>";
    for (path, bytes) in [
        ("templates/path_page.html", draft_page),
        ("assets/css/style.css", "DRAFT-CSS"),
    ] {
        let put = call(
            &app,
            "PUT",
            &format!("/api/design/draft/{path}"),
            &session,
            bytes,
        )
        .await;
        assert_eq!(put.status, StatusCode::OK, "{}", put.body);
    }

    // Anonymous with the cookie (no or a bogus session) never sees the draft.
    for cookie in [
        PREVIEW.to_string(),
        format!("{SESSION_COOKIE}=bogus; {PREVIEW}"),
    ] {
        let page = call(&app, "GET", &url, &cookie, "").await;
        assert_eq!(page.body, anon_published.body, "{cookie}");
        assert!(!page.is_preview(), "{cookie}");
        let asset = call(&app, "GET", css, &cookie, "").await;
        assert_eq!(asset.body, published_css.body);
        assert!(asset.cache_control().starts_with("public"), "{cookie}");
    }
    // The admin without the cookie still sees the published design.
    let admin_published = call(&app, "GET", &url, &session, "").await;
    assert!(!admin_published.is_preview() && !admin_published.body.contains("DRAFT-PAGE"));

    // The admin in preview sees the draft page and asset, marked.
    let preview = call(&app, "GET", &url, &both, "").await;
    assert_eq!(preview.status, StatusCode::OK);
    assert!(preview.body.contains("DRAFT-PAGE"), "{}", preview.body);
    assert!(preview.body.contains("page body"));
    let banner_at = preview.body.find(BANNER).expect("banner");
    assert!(banner_at > preview.body.find("<body class=\"d\">").expect("body tag"));
    assert_eq!(preview.cache_control(), "no-store");
    let asset = call(&app, "GET", css, &both, "").await;
    assert_eq!(asset.body, "DRAFT-CSS");
    assert_eq!(asset.cache_control(), "no-store");
    assert!(!asset.body.contains(BANNER), "no banner outside HTML");
    // Export errors in preview are marked too, and stay JSON on the API.
    let export = format!("/api/export/pages/{}?format=pdf", pg.id);
    let failed = call(&app, "GET", &export, &both, "").await;
    assert_eq!(
        failed.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "no MDCAST_URL"
    );
    assert_eq!(failed.cache_control(), "no-store");
    assert!(failed.body.starts_with('{') && !failed.body.contains(BANNER));
    let public_export = call(&app, "GET", &format!("{url}?format=nope"), &both, "").await;
    assert_eq!(public_export.status, StatusCode::BAD_REQUEST);
    assert_eq!(public_export.cache_control(), "no-store");

    // An edit shows on the next page load; assets reuse the draft that page
    // load built, so they never queue on storage.
    let put = call(
        &app,
        "PUT",
        "/api/design/draft/assets/css/style.css",
        &session,
        "V2",
    )
    .await;
    assert_eq!(put.status, StatusCode::OK);
    assert_eq!(call(&app, "GET", css, &both, "").await.body, "DRAFT-CSS");
    assert!(
        call(&app, "GET", &url, &both, "")
            .await
            .body
            .contains(BANNER)
    );
    assert_eq!(call(&app, "GET", css, &both, "").await.body, "V2");

    // A broken draft template shows its name and line, not the generic page.
    let broken = "<html><body>\n{% if %}</body></html>";
    let put = call(
        &app,
        "PUT",
        "/api/design/draft/templates/path_page.html",
        &session,
        broken,
    )
    .await;
    assert_eq!(put.status, StatusCode::OK);
    let error = call(&app, "GET", &url, &both, "").await;
    assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        error.body.contains("path_page.html, line 2"),
        "{}",
        error.body
    );
    assert!(error.body.contains(BANNER) && error.cache_control() == "no-store");
    assert!(!error.body.contains("Something went wrong"));
    // The published site is unaffected.
    assert_eq!(
        call(&app, "GET", &url, PREVIEW, "").await.body,
        anon_published.body
    );

    // Preview off (the banner's exit link) restores the published design.
    let exit = call(&app, "GET", "/api/design/preview/exit", &both, "").await;
    assert_eq!(exit.status, StatusCode::SEE_OTHER);
    assert_eq!(exit.headers[header::LOCATION], "/back/here?x=1");
    let cleared = exit.headers[header::SET_COOKIE].to_str().expect("ascii");
    assert!(cleared.starts_with("design_preview=;"), "{cleared}");
    let off = call(
        &app,
        "POST",
        "/api/design/preview",
        &session,
        r#"{"on":false}"#,
    )
    .await;
    assert_eq!(off.status, StatusCode::OK);
    assert!(
        off.headers[header::SET_COOKIE]
            .to_str()
            .expect("ascii")
            .starts_with("design_preview=;")
    );
    let after = call(&app, "GET", &url, &session, "").await;
    assert_eq!(after.body, published.body);

    page::Entity::delete_by_id(pg.id)
        .exec(&db)
        .await
        .expect("delete page");
    user::Entity::delete_by_id(admin.id)
        .exec(&db)
        .await
        .expect("delete user");
}
