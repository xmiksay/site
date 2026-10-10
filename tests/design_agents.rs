//! Design access for agents (#118): the `design_*` MCP tools on `POST /mcp`
//! (write → read → changes → render check catching an undefined variable,
//! never publishing) and the draft HTTP routes with the MCP Bearer token
//! (service token and OAuth access token; raw binary upload read back),
//! while publish, discard, history, restore and reload stay session-only.
//!
//! Gated on `DATABASE_URL` like every DB test; the design objects live under
//! a random `storage_objects` prefix so parallel tests never share a draft.

#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

#[path = "common/mcp.rs"]
mod mcp;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::{Value, json};
use site::config::Config;
use site::entity::{oauth_token, token, user};
use storage_fixture::TestStorage;
use tower::ServiceExt;

use mcp::{call_tool, is_tool_error, tool_json, tool_text};

struct Fixture {
    app: Router,
    db: DatabaseConnection,
    user_id: i32,
    service_token: String,
    session_cookie: String,
    _storage: TestStorage,
}

async fn insert_token(db: &DatabaseConnection, user_id: i32, is_service: bool) -> String {
    let nonce = site::auth::generate_token();
    token::ActiveModel {
        nonce: Set(nonce.clone()),
        user_id: Set(user_id),
        expires_at: Set(None),
        label: Set(Some("design-agents-test".into())),
        is_service: Set(is_service),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert token");
    nonce
}

async fn setup(db_url: &str) -> Fixture {
    let config = Config {
        database_url: db_url.to_string(),
        design_dir: None,
        serper_api_key: None,
        mdcast_url: None,
        mdcast_token: None,
        storage: Default::default(),
    };
    let mut state = site::state::create_state(&config).await;
    let db = state.db.clone();
    let storage = TestStorage::db(&db);
    state.storage = storage.storage.clone();

    let saved = user::ActiveModel {
        username: Set(format!("design-agent-{}", uuid::Uuid::new_v4())),
        password_hash: Set("unused".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert user");
    let service_token = insert_token(&db, saved.id, true).await;
    let session = insert_token(&db, saved.id, false).await;

    let app = Router::new()
        .nest("/api", site::routes::api::router(state.clone()))
        .merge(site::routes::mcp::router())
        .with_state(state);
    Fixture {
        app,
        db,
        user_id: saved.id,
        service_token,
        session_cookie: format!("{}={session}", site::auth::SESSION_COOKIE),
        _storage: storage,
    }
}

impl Fixture {
    async fn cleanup(self) {
        user::Entity::delete_by_id(self.user_id)
            .exec(&self.db)
            .await
            .expect("delete user");
    }
}

enum Auth<'a> {
    Bearer(&'a str),
    Cookie(&'a str),
    None,
}

async fn http(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Auth<'_>,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(uri);
    req = match auth {
        Auth::Bearer(t) => req.header("authorization", format!("Bearer {t}")),
        Auth::Cookie(c) => req.header("cookie", c),
        Auth::None => req,
    };
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body)).expect("request"))
        .await
        .expect("response");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, bytes.to_vec())
}

/// The `WWW-Authenticate` challenge of an unauthenticated `GET uri`.
async fn challenge(app: &Router, uri: &str, bearer: Option<&str>) -> Option<String> {
    let mut req = Request::builder().uri(uri);
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let value = resp.headers().get("www-authenticate")?;
    value.to_str().ok().map(String::from)
}

#[tokio::test]
async fn mcp_design_tools_edit_and_check_the_draft() {
    let Some(db_url) = mcp::test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url).await;
    let (app, tok) = (&fx.app, fx.service_token.as_str());

    let (_, listed, _) = mcp::rpc(app, Some(tok), "tools/list", None).await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for name in [
        "design_list",
        "design_write",
        "design_render_check",
        "page_read",
    ] {
        assert!(names.contains(&name), "{name} missing from {names:?}");
    }
    assert!(!names.iter().any(|n| n.contains("publish")));

    let clean = tool_json(&call_tool(app, tok, "design_render_check", json!({})).await);
    assert_eq!(clean["ok"], true, "{clean}");
    assert!(clean["cases"].as_u64().unwrap_or(0) > 0, "{clean}");

    let broken = "<p>{{ no_such_variable }}</p>";
    let write = json!({ "path": "templates/404.html", "data": broken });
    let written = call_tool(app, tok, "design_write", write).await;
    assert!(!is_tool_error(&written), "{written}");

    let read = call_tool(
        app,
        tok,
        "design_read",
        json!({ "path": "templates/404.html" }),
    )
    .await;
    assert_eq!(tool_json(&read)["data"], broken);
    let changes = tool_json(&call_tool(app, tok, "design_changes", json!({})).await);
    assert_eq!(
        changes["changes"],
        json!([{ "path": "templates/404.html", "kind": "modified" }])
    );

    let check = tool_json(&call_tool(app, tok, "design_render_check", json!({})).await);
    assert_eq!(check["ok"], false, "{check}");
    let errors = check["render_errors"].as_array().expect("render_errors");
    assert!(
        errors.iter().any(|e| e["template"] == "404.html"
            && e["message"].as_str().unwrap_or("").contains("undefined")),
        "{check}"
    );

    let contract = call_tool(app, tok, "design_contract", json!({})).await;
    assert!(tool_text(&contract).contains("### `404.html`"));
    let bad = call_tool(
        app,
        tok,
        "design_write",
        json!({ "path": "../x", "data": "x" }),
    )
    .await;
    assert!(is_tool_error(&bad), "{bad}");

    // Nothing went live: the published design is still the baked one.
    let published = call_tool(
        app,
        tok,
        "design_read",
        json!({ "path": "templates/404.html", "source": "published" }),
    )
    .await;
    assert_ne!(tool_json(&published)["data"], broken);
    fx.cleanup().await;
}

#[tokio::test]
async fn bearer_token_reaches_only_the_draft_routes() {
    let Some(db_url) = mcp::test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url).await;
    let app = &fx.app;
    let bearer = Auth::Bearer(&fx.service_token);
    let font_uri = "/api/design/draft/assets/fonts/x.woff2";
    let font = vec![b'w', b'O', b'F', b'2', 0, 0xff, 0x80, 0x01];

    let (status, body) = http(app, "PUT", font_uri, bearer, font.clone()).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let state: Value = serde_json::from_slice(&body).expect("draft state");
    assert!(
        state["changes"]
            .as_array()
            .expect("changes")
            .contains(&json!({ "path": "assets/fonts/x.woff2", "kind": "added" })),
        "{state}"
    );

    // An OAuth access token works as well.
    let access = site::auth::generate_token();
    oauth_token::ActiveModel {
        access_token: Set(access.clone()),
        refresh_token: Set(site::auth::generate_token()),
        client_id: Set("design-agents-test".into()),
        user_id: Set(fx.user_id),
        expires_at: Set((chrono::Utc::now() + chrono::Duration::hours(1)).into()),
        revoked: Set(false),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("insert oauth token");
    let (status, body) = http(app, "GET", font_uri, Auth::Bearer(&access), Vec::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, font);
    let tree = http(
        app,
        "GET",
        "/api/design/draft",
        Auth::Bearer(&access),
        Vec::new(),
    )
    .await;
    assert_eq!(tree.0, StatusCode::OK);

    for (auth, what) in [
        (Auth::None, "no auth"),
        (Auth::Bearer("garbage"), "bad token"),
    ] {
        let (status, _) = http(app, "GET", font_uri, auth, Vec::new()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{what}");
    }
    // Same OAuth discovery challenge as `/mcp`.
    for bearer in [None, Some("garbage")] {
        let www = challenge(app, font_uri, bearer).await.unwrap_or_default();
        assert!(
            www.starts_with("Bearer ") && www.contains("/.well-known/oauth-protected-resource"),
            "{bearer:?}: {www:?}"
        );
    }

    // Publishing and the rest of the design admin stay session-only.
    let session_only = [
        ("POST", "/api/design/publish"),
        ("POST", "/api/design/publish?force=true"),
        ("POST", "/api/design/draft/discard"),
        ("POST", "/api/design/reload"),
        ("GET", "/api/design/history"),
        ("POST", "/api/design/history/2026-10-10T12:00:00Z/restore"),
        ("POST", "/api/design/preview"),
        ("GET", "/api/design/preview/exit"),
    ];
    for (method, uri) in session_only {
        let (status, _) = http(
            app,
            method,
            uri,
            Auth::Bearer(&fx.service_token),
            Vec::new(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
    let (status, _) = http(
        app,
        "GET",
        "/api/pages",
        Auth::Bearer(&fx.service_token),
        Vec::new(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "other API routes stay session-only"
    );

    // The session still reaches the draft routes, and publishes.
    let cookie = Auth::Cookie(&fx.session_cookie);
    let (status, body) = http(app, "GET", font_uri, cookie, Vec::new()).await;
    assert_eq!((status, body), (StatusCode::OK, font));
    let cookie = Auth::Cookie(&fx.session_cookie);
    let (status, body) = http(app, "POST", "/api/design/publish", cookie, Vec::new()).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    fx.cleanup().await;
}
