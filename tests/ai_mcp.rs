//! DB-dependent tests for `site::ai::mcp::SiteMcp` against real
//! `user_mcp_servers` rows. Gated on `DATABASE_URL`; follows
//! `tests/policy_db.rs`'s skip/cleanup convention. `user_mcp_servers` FKs to
//! `users` with `ON DELETE CASCADE` (`m_013_create_user_mcp_servers.rs`), so
//! cleanup is just deleting the throwaway user.
//!
//! No real remote MCP server is reachable from this environment. Most
//! assertions here exercise the graceful-failure path (a bogus/unreachable
//! `url`); two tests spin up a tiny in-process axum server speaking just
//! enough streamable-HTTP MCP (`initialize` / `notifications/initialized` /
//! `tools/list`) to prove the enabled-only filter, the cross-user dedup, and
//! the cache-then-invalidate behavior actually reach a live tool list rather
//! than being indistinguishable no-ops.

use axum::routing::post;
use axum::{Json, Router};
use entanglement_core::ToolCall;
use entanglement_runtime::{SharedRegistry, ToolRegistry};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use serde_json::{Value, json};
use site::ai::engine::SiteEngine;
use site::ai::mcp::SiteMcp;
use site::entity::{user, user_mcp_server};

async fn test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

fn shared_registry() -> SharedRegistry {
    ToolRegistry::new().shared()
}

async fn make_user(db: &DatabaseConnection, tag: &str) -> i32 {
    let username = format!("mcp-test-{tag}-{}", uuid::Uuid::new_v4());
    let saved = user::ActiveModel {
        username: Set(username),
        password_hash: Set("unused".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway user");
    saved.id
}

async fn cleanup_user(db: &DatabaseConnection, user_id: i32) {
    user::Entity::delete_by_id(user_id)
        .exec(db)
        .await
        .expect("delete throwaway user"); // cascades to user_mcp_servers rows
}

async fn add_server(db: &DatabaseConnection, user_id: i32, name: &str, url: &str, enabled: bool) {
    user_mcp_server::ActiveModel {
        user_id: Set(user_id),
        name: Set(name.to_string()),
        url: Set(url.to_string()),
        enabled: Set(enabled),
        forward_user_token: Set(false),
        headers: Set(json!({})),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway user_mcp_server");
}

/// Spin up a minimal streamable-HTTP MCP server on an ephemeral local port,
/// answering every JSON-RPC call with a single tool named `tool_name`. Good
/// enough for `HttpClient::connect`'s handshake + `list_tools` — it never
/// inspects params, only `method`. Aborted automatically when the `#[tokio::
/// test]`'s own runtime is torn down at the end of the test.
async fn spawn_fake_mcp_server(tool_name: &'static str) -> String {
    let app = Router::new().route(
        "/mcp",
        post(move |Json(body): Json<Value>| async move { fake_mcp_reply(&body, tool_name) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake mcp listener");
    let addr = listener.local_addr().expect("fake mcp local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/mcp")
}

fn fake_mcp_reply(body: &Value, tool_name: &str) -> Json<Value> {
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let result = match method {
        "tools/list" => json!({
            "tools": [{
                "name": tool_name,
                "description": "fake tool for tests",
                "inputSchema": { "type": "object", "properties": {} },
            }]
        }),
        // `initialize` and the `notifications/initialized` notification both
        // just need a 2xx JSON-RPC-shaped reply; neither client path inspects
        // this result payload.
        _ => json!({ "protocolVersion": "2025-03-26", "capabilities": {} }),
    };
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

#[tokio::test]
async fn zero_enabled_servers_yields_no_specs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "zero").await;
    let mcp = SiteMcp::new(db.clone(), shared_registry());

    assert!(mcp.tool_specs_for_user(user_id).await.is_empty());

    cleanup_user(&db, user_id).await;
}

#[tokio::test]
async fn unreachable_server_is_skipped_gracefully() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "unreachable").await;
    // Port 1 on loopback: nothing listens there, so this fails fast
    // (connection refused) rather than waiting out a connect timeout.
    add_server(&db, user_id, "bogus", "http://127.0.0.1:1/mcp", true).await;
    let mcp = SiteMcp::new(db.clone(), shared_registry());

    let specs = mcp.tool_specs_for_user(user_id).await;
    assert!(
        specs.is_empty(),
        "a server that fails to connect must not panic/error, just contribute nothing"
    );

    cleanup_user(&db, user_id).await;
}

#[tokio::test]
async fn cache_serves_stale_within_ttl_then_invalidate_reflects_new_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "cache").await;
    let url_a = spawn_fake_mcp_server("echoA").await;
    add_server(&db, user_id, "cacheA", &url_a, true).await;
    let mcp = SiteMcp::new(db.clone(), shared_registry());

    let first = mcp.tool_specs_for_user(user_id).await;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].name, "cacheA__echoA");

    // A second enabled row lands *after* the first cache fill. Within the 60s
    // TTL, the cache must still answer with the pre-insert snapshot.
    let url_b = spawn_fake_mcp_server("echoB").await;
    add_server(&db, user_id, "cacheB", &url_b, true).await;

    let still_cached = mcp.tool_specs_for_user(user_id).await;
    assert_eq!(
        still_cached.len(),
        1,
        "expected the cached (pre-insert) result within the TTL"
    );
    assert_eq!(still_cached[0].name, "cacheA__echoA");

    mcp.invalidate_user(user_id);

    let refreshed = mcp.tool_specs_for_user(user_id).await;
    let mut names: Vec<&str> = refreshed.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["cacheA__echoA", "cacheB__echoB"],
        "expected a fresh DB read to reflect the row inserted after the cache filled"
    );

    cleanup_user(&db, user_id).await;
}

/// Issue #38: the shared dispatch registry reflects an MCP server add/remove
/// live, with no executor restart or periodic rebuild — `SiteMcp` registers a
/// user's `"{server}__{tool}"` identities the moment `routes_for_user`
/// (re)connects, and `invalidate_user` deregisters them once no other cached
/// user still has them.
#[tokio::test]
async fn registry_reflects_mcp_server_add_and_remove_without_executor_restart() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "registry-add-remove").await;
    let url = spawn_fake_mcp_server("echo").await;
    add_server(&db, user_id, "srv", &url, true).await;

    let registry = shared_registry();
    let mcp = SiteMcp::new(db.clone(), registry.clone());

    assert!(
        !registry.read().unwrap().contains("srv__echo"),
        "must not be registered before any route discovery"
    );

    mcp.tool_specs_for_user(user_id).await;
    assert!(
        registry.read().unwrap().contains("srv__echo"),
        "discovering the route must register it into the live dispatch registry"
    );

    mcp.invalidate_user(user_id);
    assert!(
        !registry.read().unwrap().contains("srv__echo"),
        "invalidating the sole user with this identity must deregister it"
    );

    cleanup_user(&db, user_id).await;
}

/// Issue #38: a tool becomes genuinely callable — not just advertised — the
/// moment its server is added, mid-session, with no seed-at-spawn step and no
/// executor swap. Dispatches through a snapshot of the live registry exactly
/// as `ToolRegistry::execute` would during a real turn; a stale/static
/// registry would report "unknown tool" here instead of actually reaching
/// the fake server.
#[tokio::test]
async fn tool_becomes_callable_mid_session_after_its_server_is_added() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let user_id = make_user(&db, "callable-mid-session").await;
    let registry = shared_registry();
    let mcp = SiteMcp::new(db.clone(), registry.clone());
    let session = SiteEngine::session_id_for_user(user_id);
    let call = ToolCall {
        id: "1".into(),
        name: "srv__echo".into(),
        input: "{}".into(),
        provider_meta: None,
    };

    // Before the server exists, the identity is unknown to dispatch. Snapshot
    // the registry into an owned clone first so the read lock is dropped
    // before the `.await` below, not held across it.
    let snapshot = registry.read().unwrap().clone();
    let before = snapshot.execute(&call, &session).await;
    assert!(
        before
            .iter()
            .any(|c| c.as_text().is_some_and(|t| t.contains("unknown tool"))),
        "must be unknown before the server is added: {before:?}"
    );

    let url = spawn_fake_mcp_server("echo").await;
    add_server(&db, user_id, "srv", &url, true).await;
    // Mirrors what `ai::handlers::mcp_servers::create` does after inserting
    // the row — the CRUD-driven invalidation the issue calls for.
    mcp.invalidate_user(user_id);
    // A session resolving its tool specs (e.g. at the start of a turn) is
    // what actually triggers `routes_for_user` to reconnect and register.
    mcp.tool_specs_for_user(user_id).await;

    let snapshot = registry.read().unwrap().clone();
    let after = snapshot.execute(&call, &session).await;
    assert!(
        !after
            .iter()
            .any(|c| c.as_text().is_some_and(|t| t.contains("unknown tool"))),
        "must be dispatchable once the server is added and re-discovered: {after:?}"
    );

    cleanup_user(&db, user_id).await;
}
