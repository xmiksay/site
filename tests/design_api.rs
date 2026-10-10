//! `/api/design` (#110, #115) over a full `AppState`: the draft API (tree +
//! changes, raw text and binary files, delete, discard), publish (422 on a
//! broken template, live untouched), history and restore, the WS
//! `design.*` events, and Reload after a bucket edit — all driving what the
//! public 404 page renders, over fs and db storage.
//!
//! Gated on `DATABASE_URL` like every DB test.

// Shared with tests/storage.rs; not every helper is used here.
#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use serde_json::{Value, json};
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{token, user};
use site::routes::ws::{Envelope, Topic, WsHub};
use site::storage::{Storage, StorageConfig};
use storage_fixture::TestStorage;
use tokio::sync::mpsc;
use tower::ServiceExt;

fn test_db_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

async fn test_db() -> Option<DatabaseConnection> {
    let url = test_db_url()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

struct App {
    app: Router,
    db: DatabaseConnection,
    cookie: String,
    user_id: i32,
    username: String,
    hub: Arc<WsHub>,
}

/// `scoped` replaces the state's storage after startup: a db test isolates
/// its keyed objects under a prefix that way (startup itself reads unscoped).
async fn app(storage: StorageConfig, scoped: Option<Storage>) -> App {
    let config = Config {
        database_url: test_db_url().expect("DATABASE_URL"),
        design_dir: None,
        serper_api_key: None,
        mdcast_url: None,
        mdcast_token: None,
        storage,
    };
    let mut state = site::state::create_state(&config).await;
    if let Some(scoped) = scoped {
        state.storage = scoped;
    }
    let db = state.db.clone();
    let username = format!("design-{}", uuid::Uuid::new_v4());
    let saved = user::ActiveModel {
        username: Set(username.clone()),
        password_hash: Set("unused".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert user");
    let nonce = site::auth::generate_token();
    token::ActiveModel {
        nonce: Set(nonce.clone()),
        user_id: Set(saved.id),
        expires_at: Set(None),
        label: Set(Some("test".into())),
        is_service: Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert session token");
    let hub = state.ws_hub.clone();
    let app = Router::new()
        .nest("/api", site::routes::api::router(state.clone()))
        .fallback(get(site::routes::public::catch_all))
        .with_state(state);
    App {
        app,
        db,
        cookie: format!("{SESSION_COOKIE}={nonce}"),
        user_id: saved.id,
        username,
        hub,
    }
}

impl App {
    async fn call(&self, method: &str, uri: &str, body: impl Into<Body>) -> (StatusCode, Vec<u8>) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", &self.cookie)
            .body(body.into())
            .expect("request");
        let resp = self.app.clone().oneshot(req).await.expect("response");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        (status, bytes.to_vec())
    }

    async fn json(&self, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
        let (status, bytes) = self.call(method, uri, body.to_string()).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn public_404(&self) -> String {
        let (_, page) = self.call("GET", "/no-such-page-for-design-test", "").await;
        String::from_utf8_lossy(&page).into_owned()
    }

    async fn cleanup(self) {
        user::Entity::delete_by_id(self.user_id)
            .exec(&self.db)
            .await
            .expect("delete user");
    }
}

fn entry<'a>(state: &'a Value, path: &str) -> &'a Value {
    state["files"]
        .as_array()
        .expect("files")
        .iter()
        .find(|f| f["path"] == path)
        .unwrap_or(&Value::Null)
}

/// The next `design.*` event, skipping nothing: every draft mutation sends
/// exactly one.
fn next_design_event(rx: &mut mpsc::Receiver<Envelope>) -> (String, Value) {
    let envelope = rx.try_recv().expect("a design event");
    assert_eq!(envelope.topic, Topic::Design);
    (envelope.event, envelope.payload)
}

async fn exercise(app: &App, ts: &TestStorage, kind: &str) {
    let (_tx, mut rx) = app.hub.register(app.user_id);
    let marker = format!("DESIGN-{}", uuid::Uuid::new_v4());
    let baked_404 = app.public_404().await;

    let (status, state) = app.json("GET", "/api/design/draft", "").await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(state["storage"], kind);
    assert_eq!(state["changes"], json!([]));
    assert_eq!(entry(&state, "templates/404.html")["baked"], true);
    assert_eq!(entry(&state, "templates/404.html")["overridden"], false);

    let (status, state) = app
        .json("PUT", "/api/design/draft/templates/404.html", &marker)
        .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(entry(&state, "templates/404.html")["overridden"], true);
    assert_eq!(
        state["changes"],
        json!([{ "path": "templates/404.html", "kind": "modified" }])
    );
    assert_eq!(
        next_design_event(&mut rx),
        (
            "draft_changed".into(),
            json!({ "action": "put", "path": "templates/404.html" })
        )
    );
    assert_eq!(app.public_404().await, baked_404, "invisible until publish");
    let read = |source: &'static str| async move {
        let uri = format!("/api/design/draft/templates/404.html{source}");
        let (status, body) = app.call("GET", &uri, "").await;
        assert_eq!(status, StatusCode::OK, "{source}");
        String::from_utf8_lossy(&body).into_owned()
    };
    assert_eq!(read("").await, marker);
    assert_eq!(read("?source=published").await, read("?source=baked").await);

    // Raw binary round trip, then delete.
    let png = vec![0x89, b'P', b'N', b'G', 0, 0xff];
    let (status, _) = app
        .call("PUT", "/api/design/draft/assets/img/x.png", png.clone())
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = app
        .call("GET", "/api/design/draft/assets/img/x.png", "")
        .await;
    assert_eq!(body, png);
    let (status, _) = app
        .call("DELETE", "/api/design/draft/assets/img/x.png", "")
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app
        .call("DELETE", "/api/design/draft/assets/img/x.png", "")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let events: Vec<_> = (0..2)
        .map(|_| next_design_event(&mut rx).1["action"].clone())
        .collect();
    assert_eq!(events, [json!("put"), json!("delete")]);

    // A broken template is accepted in the draft but blocks the publish.
    let (status, _) = app
        .call("PUT", "/api/design/draft/templates/404.html", "{% for %}")
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = app.json("POST", "/api/design/publish", "").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("templates/404.html"),
        "{body}"
    );
    assert_eq!(app.public_404().await, baked_404, "live untouched");
    assert!(
        ts.storage
            .get("design/templates/404.html")
            .await
            .expect("get")
            .is_none()
    );

    let (status, _) = app
        .call(
            "PUT",
            "/api/design/draft/templates/404.html",
            marker.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, first) = app.json("POST", "/api/design/publish", "").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["by"], app.username.as_str());
    assert_eq!(app.public_404().await, marker);
    while let Ok(e) = rx.try_recv() {
        if e.event == "published" {
            assert_eq!(e.payload["id"], first["id"]);
        }
    }

    let (_, _) = app
        .call("PUT", "/api/design/draft/templates/404.html", "SECOND")
        .await;
    let (status, _) = app.json("POST", "/api/design/publish", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(app.public_404().await, "SECOND");
    let (status, history) = app.json("GET", "/api/design/history", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history.as_array().map(Vec::len), Some(2), "{history}");
    assert_eq!(history[1], first, "newest first");

    while rx.try_recv().is_ok() {}
    let id = first["id"].as_str().expect("id");
    let (status, state) = app
        .json("POST", &format!("/api/design/history/{id}/restore"), "")
        .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(
        state["changes"],
        json!([{ "path": "templates/404.html", "kind": "modified" }])
    );
    assert_eq!(
        app.public_404().await,
        "SECOND",
        "restore goes to the draft"
    );
    assert_eq!(
        next_design_event(&mut rx),
        (
            "draft_changed".into(),
            json!({ "action": "restore", "version": id })
        )
    );

    let (status, state) = app.json("POST", "/api/design/draft/discard", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["changes"], json!([]));
    assert_eq!(next_design_event(&mut rx).1, json!({ "action": "discard" }));

    let (status, _) = app
        .call(
            "POST",
            "/api/design/history/2001-01-01T00:00:00Z/restore",
            "",
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for bad in ["preview/x.html", "templates/..%2Fx"] {
        let (status, _) = app
            .call("PUT", &format!("/api/design/draft/{bad}"), "x")
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    // Edited in the storage directly, picked up by Reload.
    ts.storage
        .put("design/templates/404.html", "EXTERNAL".into())
        .await
        .expect("external edit");
    let (status, state) = app.json("POST", "/api/design/reload", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["last_reload"]["ok"], true);
    assert_eq!(app.public_404().await, "EXTERNAL");
    assert_eq!(
        state["changes"],
        json!([{ "path": "templates/404.html", "kind": "modified" }]),
        "the draft still holds the last published version"
    );
}

#[tokio::test]
async fn design_api_over_fs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let ts = TestStorage::fs(&db);
    let app = app(ts.fs_config(), None).await;
    exercise(&app, &ts, "fs").await;
    app.cleanup().await;
}

#[tokio::test]
async fn design_api_over_db() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let ts = TestStorage::db(&db);
    let app = app(StorageConfig::Db, Some(ts.storage.clone())).await;
    exercise(&app, &ts, "db").await;
    app.cleanup().await;
}
