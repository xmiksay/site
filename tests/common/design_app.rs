//! The `/api/design` test harness: a full `AppState` router with a logged-in
//! throwaway user, shared by the design API integration tests.

use std::sync::Arc;

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
use site::routes::ws::{Envelope, Topic, WsHub};
use site::storage::{Storage, StorageConfig};
use tokio::sync::mpsc;
use tower::ServiceExt;

pub fn test_db_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

pub async fn test_db() -> Option<DatabaseConnection> {
    let url = test_db_url()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

pub struct App {
    pub app: Router,
    pub db: DatabaseConnection,
    pub cookie: String,
    pub user_id: i32,
    pub username: String,
    pub hub: Arc<WsHub>,
}

/// `scoped` replaces the state's storage after startup: a db test isolates
/// its keyed objects under a prefix that way (startup itself reads unscoped).
pub async fn app(storage: StorageConfig, scoped: Option<Storage>) -> App {
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
    pub async fn call(
        &self,
        method: &str,
        uri: &str,
        body: impl Into<Body>,
    ) -> (StatusCode, Vec<u8>) {
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

    pub async fn json(&self, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
        let (status, bytes) = self.call(method, uri, body.to_string()).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    pub async fn public_404(&self) -> String {
        let (_, page) = self.call("GET", "/no-such-page-for-design-test", "").await;
        String::from_utf8_lossy(&page).into_owned()
    }

    pub async fn cleanup(self) {
        user::Entity::delete_by_id(self.user_id)
            .exec(&self.db)
            .await
            .expect("delete user");
    }
}

pub fn entry<'a>(state: &'a Value, path: &str) -> &'a Value {
    state["files"]
        .as_array()
        .expect("files")
        .iter()
        .find(|f| f["path"] == path)
        .unwrap_or(&Value::Null)
}

/// The next `design.*` event, skipping nothing: every draft mutation sends
/// exactly one.
pub fn next_design_event(rx: &mut mpsc::Receiver<Envelope>) -> (String, Value) {
    let envelope = rx.try_recv().expect("a design event");
    assert_eq!(envelope.topic, Topic::Design);
    (envelope.event, envelope.payload)
}
