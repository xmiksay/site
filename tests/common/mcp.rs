//! Shared setup for the MCP-endpoint integration tests (`tests/mcp_endpoint.rs`,
//! `tests/mcp_pages.rs`, `tests/mcp_tags_files_galleries.rs`). Included via
//! `#[path = "common/mcp.rs"]` rather than folded into `tests/common/mod.rs`:
//! that module's `send` helper is cookie/session-oriented (the `/api/*` JSON
//! API convention), while `POST /mcp` is Bearer-token/JSON-RPC — different
//! enough on both the request and response shape to warrant its own small
//! helper instead of overloading the shared one (see `tests/common/mod.rs`'s
//! module doc for why `send` itself must stay as-is).
//!
//! Same DB-gated / throwaway-user-per-test / cascade-delete convention as
//! `tests/policy_db.rs`: every test gets its own user + service token, and
//! deletes the user (cascading the token) when done.

// This module is compiled fresh into each of the three `tests/mcp_*.rs`
// binaries via `#[path]`, and no single binary calls every helper (e.g. only
// the tags/files/galleries file needs `tool_json`) — allow the resulting
// per-binary dead-code warnings rather than force every binary to touch every
// helper just to silence the lint.
#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::{Value, json};
use site::config::Config;
use site::entity::{token, user};
use site::state::{self, AppState};
use tower::ServiceExt;

pub struct Fixture {
    pub app: Router,
    pub db: DatabaseConnection,
    pub token: String,
    pub user_id: i32,
}

/// Same DB-gated skip convention as `tests/common/mod.rs::test_db_url` —
/// duplicated here (rather than pulling in that module) so binaries that
/// only need this one helper don't also drag in `send` and warn on it as
/// unused (that helper is cookie/JSON-API-shaped; the MCP tests never call
/// it).
pub async fn test_db_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

/// Build a throwaway user + `is_service = true` bearer token, and the real
/// `POST /mcp` router wired to a fresh `AppState` against `db_url`. Each test
/// gets its own user/token rows (unique via a random tag) since `site_test`
/// isn't reset between runs.
pub async fn setup(db_url: &str, tag: &str) -> Fixture {
    let config = Config {
        database_url: db_url.to_string(),
        design_dir: None,
        serper_api_key: None,
        mdcast_pandoc_path: "pandoc".to_string(),
    };
    let state: AppState = state::create_state(&config).await;
    let db = state.db.clone();

    let username = format!("mcp-test-{tag}-{}", uuid::Uuid::new_v4());
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
        label: Set(Some("mcp-test".to_string())),
        is_service: Set(true),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert service token");

    let app = site::routes::mcp::router().with_state(state);

    Fixture {
        app,
        db,
        token: nonce,
        user_id: saved_user.id,
    }
}

pub async fn cleanup_user(db: &DatabaseConnection, user_id: i32) {
    user::Entity::delete_by_id(user_id)
        .exec(db)
        .await
        .expect("delete throwaway user"); // cascades to the tokens row
}

/// POST a JSON-RPC 2.0 request to `/mcp`. `bearer: None` omits the
/// `Authorization` header entirely (for the no-header auth test); pass a
/// garbage string to exercise the invalid-token path.
pub async fn rpc(
    app: &Router,
    bearer: Option<&str>,
    method: &str,
    params: Option<Value>,
) -> (StatusCode, Value, HeaderMap) {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json");
    if let Some(t) = bearer {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    let req = builder
        .body(Body::from(
            serde_json::to_vec(&body).expect("serialize request"),
        ))
        .expect("build request");
    let resp = app.clone().oneshot(req).await.expect("request failed");
    let status = resp.status();
    let headers = resp.headers().clone();
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
    (status, value, headers)
}

/// `tools/call` round trip: returns the parsed JSON-RPC response body.
pub async fn call_tool(app: &Router, token: &str, name: &str, arguments: Value) -> Value {
    let (status, body, _) = rpc(
        app,
        Some(token),
        "tools/call",
        Some(json!({ "name": name, "arguments": arguments })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "tools/call http status: {body:?}");
    body
}

/// Extract the `content[0].text` string a successful (or tool-level-error)
/// `tools/call` response carries in its `result`.
pub fn tool_text(resp: &Value) -> &str {
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no result.content[0].text in {resp:?}"))
}

/// Tools that return structured data (`json_result` in `rpc.rs`) pretty-print
/// a JSON object into that same `content[0].text` field — parse it back out.
pub fn tool_json(resp: &Value) -> Value {
    serde_json::from_str(tool_text(resp))
        .unwrap_or_else(|e| panic!("tool text was not JSON: {e} ({resp:?})"))
}

pub fn is_tool_error(resp: &Value) -> bool {
    resp["result"]["isError"].as_bool().unwrap_or(false)
}
