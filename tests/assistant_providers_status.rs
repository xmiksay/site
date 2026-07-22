//! Integration test for `GET /api/assistant/providers/status` (#89) — the
//! idle throttle-status path never makes a real HTTP request, so unlike
//! `tests/assistant_session_base.rs` this doesn't need a reachable Ollama,
//! only `DATABASE_URL` (skipped gracefully, not failed, when unset — same
//! convention as `tests/policy_db.rs`).

mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{send, test_db_url};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::json;
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{llm_provider, token, user};
use site::state::{self, AppState};

/// Throwaway logged-in user + a single `llm_provider` row, mirroring
/// `assistant_session_base.rs`'s `Fixture`/`setup()` but without any model
/// row or live-Ollama gate — this endpoint never dispatches a request.
struct Fixture {
    app: Router,
    db: DatabaseConnection,
    cookie: String,
    user_id: i32,
    provider_id: i32,
}

async fn setup(db_url: &str) -> Fixture {
    let config = Config {
        database_url: db_url.to_string(),
        design_dir: None,
        serper_api_key: None,
        mdcast_pandoc_path: "pandoc".to_string(),
    };
    let state: AppState = state::create_state(&config).await;
    let db = state.db.clone();

    let username = format!("assistant-throttle-{}", uuid::Uuid::new_v4());
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

    let provider_label = format!("test-throttle-{}", uuid::Uuid::new_v4());
    let provider = llm_provider::ActiveModel {
        label: Set(provider_label),
        kind: Set("ollama".to_string()),
        api_key: Set(None),
        base_url: Set(Some("http://localhost:11434/v1".to_string())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert llm_provider");

    // The engine's model catalog was hydrated at `create_state` time, before
    // this row existed — refresh so it gets its own `ProviderHandle`.
    state
        .agent_engine
        .catalog
        .refresh()
        .await
        .expect("refresh model catalog");

    let app = site::routes::api::router(state.clone()).with_state(state);

    Fixture {
        app,
        db,
        cookie: format!("{SESSION_COOKIE}={nonce}"),
        user_id: saved_user.id,
        provider_id: provider.id,
    }
}

async fn cleanup(fx: &Fixture) {
    let _ = user::Entity::delete_by_id(fx.user_id).exec(&fx.db).await;
    let _ = llm_provider::Entity::delete_by_id(fx.provider_id)
        .exec(&fx.db)
        .await;
}

#[tokio::test]
async fn providers_status_reports_an_idle_provider() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let fx = setup(&db_url).await;

    let (status, body) = send(
        &fx.app,
        "GET",
        "/assistant/providers/status",
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "providers/status: {body}");

    let entries = body.as_array().expect("array response");
    let entry = entries
        .iter()
        .find(|e| e["provider_id"].as_i64() == Some(fx.provider_id as i64))
        .unwrap_or_else(|| panic!("no entry for provider {}: {body:#}", fx.provider_id));

    assert_eq!(entry["in_flight"], json!(0));
    assert_eq!(entry["penalized"], json!(false));
    assert_eq!(entry["backoff_remaining_ms"], json!(null));
    // No `concurrency` set on the fixture's provider row -> the catalog's
    // display-only fallback (`DEFAULT_CONCURRENCY_FALLBACK`, mirroring
    // entanglement_provider's own default).
    assert_eq!(entry["cap"], json!(3));
    assert_eq!(entry["endpoint"], json!("http://localhost:11434/v1"));

    cleanup(&fx).await;
}
