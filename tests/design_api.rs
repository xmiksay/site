//! `/api/design` (#110) over a full `AppState`: fs storage driving what the
//! public site renders (save, 422 on a broken template, external edit +
//! Reload, revert), and db storage being read-only.
//!
//! Gated on `DATABASE_URL` like every DB test.

// Shared with tests/storage.rs; not every helper is used here.
#[allow(dead_code)]
#[path = "common/storage.rs"]
mod storage_fixture;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use serde_json::Value;
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{token, user};
use site::storage::StorageConfig;
use storage_fixture::TestStorage;
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
}

async fn app(storage: StorageConfig) -> App {
    let config = Config {
        database_url: test_db_url().expect("DATABASE_URL"),
        design_dir: None,
        serper_api_key: None,
        mdcast_url: None,
        mdcast_token: None,
        storage,
    };
    let state = site::state::create_state(&config).await;
    let db = state.db.clone();
    let saved = user::ActiveModel {
        username: Set(format!("design-{}", uuid::Uuid::new_v4())),
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
    let app = Router::new()
        .nest("/api", site::routes::api::router(state.clone()))
        .fallback(get(site::routes::public::catch_all))
        .with_state(state);
    App {
        app,
        db,
        cookie: format!("{SESSION_COOKIE}={nonce}"),
        user_id: saved.id,
    }
}

impl App {
    async fn call(&self, method: &str, uri: &str, body: &str) -> (StatusCode, Vec<u8>) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", &self.cookie)
            .body(Body::from(body.to_string()))
            .expect("request");
        let resp = self.app.clone().oneshot(req).await.expect("response");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        (status, bytes.to_vec())
    }

    async fn json(&self, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
        let (status, bytes) = self.call(method, uri, body).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
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

#[tokio::test]
async fn design_api_over_fs_drives_the_public_site() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let ts = TestStorage::fs(&db);
    let app = app(ts.fs_config()).await;
    let marker = format!("DESIGN-{}", uuid::Uuid::new_v4());

    let (status, state) = app.json("GET", "/api/design", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (state["storage"].as_str(), state["editable"].as_bool()),
        (Some("fs"), Some(true))
    );
    assert_eq!(entry(&state, "templates/404.html")["baked"], true);
    assert!(
        state["files"].as_array().expect("files").iter().all(|f| {
            let p = f["path"].as_str().unwrap_or("");
            ["templates/", "assets/", "mdcast/"]
                .iter()
                .any(|r| p.starts_with(r))
        }),
        "only bundle roots are listed"
    );

    let (status, state) = app
        .json("PUT", "/api/design/files/templates/404.html", &marker)
        .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(entry(&state, "templates/404.html")["overridden"], true);
    let (_, page) = app.call("GET", "/no-such-page-for-design-test", "").await;
    assert_eq!(String::from_utf8_lossy(&page), marker);

    let (status, body) = app
        .json("PUT", "/api/design/files/templates/404.html", "{% for %}")
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("templates/404.html"),
        "{body}"
    );
    let (_, current) = app
        .call("GET", "/api/design/files/templates/404.html", "")
        .await;
    assert_eq!(
        String::from_utf8_lossy(&current),
        marker,
        "rejected save changed nothing"
    );

    let (status, baked) = app
        .call(
            "GET",
            "/api/design/files/templates/404.html?source=baked",
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(String::from_utf8_lossy(&baked), marker);

    // Edited in the storage directly, picked up by Reload.
    let StorageConfig::Fs { dir } = ts.fs_config() else {
        unreachable!()
    };
    std::fs::write(dir.join("design/templates/404.html"), "EXTERNAL").expect("external edit");
    let (status, state) = app.json("POST", "/api/design/reload", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["last_reload"]["ok"], true);
    let (_, page) = app.call("GET", "/no-such-page-for-design-test", "").await;
    assert_eq!(String::from_utf8_lossy(&page), "EXTERNAL");

    let (status, state) = app
        .json("DELETE", "/api/design/files/templates/404.html", "")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(entry(&state, "templates/404.html")["overridden"], false);
    let (status, _) = app
        .call("DELETE", "/api/design/files/templates/404.html", "")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for bad in ["preview/x.html", "templates/..%2Fx"] {
        let (status, _) = app
            .call("PUT", &format!("/api/design/files/{bad}"), "x")
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    app.cleanup().await;
}

#[tokio::test]
async fn design_api_over_db_is_read_only() {
    let Some(_db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let app = app(StorageConfig::Db).await;
    let (status, state) = app.json("GET", "/api/design", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (state["storage"].as_str(), state["editable"].as_bool()),
        (Some("db"), Some(false))
    );
    let (status, _) = app
        .call("PUT", "/api/design/files/templates/404.html", "x")
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = app
        .call("GET", "/api/design/files/templates/base.html", "")
        .await;
    assert_eq!(status, StatusCode::OK, "baked files stay readable");
    app.cleanup().await;
}
