//! Scripted-`Llm` fixture, shared by `tests/assistant_session_subagent_
//! researcher.rs`, `tests/assistant_session_subagent_pagewriter.rs`, and
//! `tests/assistant_session_compact.rs` only — pulled in via `#[path =
//! "common/scripted.rs"] mod scripted;` (not `mod.rs`'s own `mod scripted;`)
//! so `tests/assistant_session_base.rs`, which doesn't need any of this,
//! never compiles it. Each including file is its own crate (integration
//! tests each compile as a separate binary), so which of these functions
//! counts as "used" differs per binary — `#[allow(dead_code)]` rather than
//! splitting this into three near-identical files.
#![allow(dead_code)]

use axum::Router;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use site::ai::AiConfig;
use site::ai::engine::SiteEngine;
use site::auth::SESSION_COOKIE;
use site::entity::{assistant_session, llm_model, llm_provider, token, user};
use site::routes::ws::WsHub;
use site::state::AppState;

/// A fixture backed by a scripted `Llm` instead of a real provider (#17's
/// sub-agent tests) — no `llm_provider`/`llm_model` rows, so no `cleanup` of
/// those is needed either. `assistant_sessions` rows are created directly
/// (`scripted_session`, below) rather than through `POST /assistant/sessions`,
/// which always sends `InMsg::SetModel` bound to a DB-catalog model — that
/// would rebind the session off `llm_factory` (this fixture's scripted
/// backend) onto the real per-model factory `SiteCatalog::model_resolver`
/// builds. Skipping session creation's `SetModel` leaves the session on the
/// engine-wide default (`EngineConfig.llm_factory`, set from
/// `SiteEngine::spawn`'s `llm_factory_override`) for its whole life — exactly
/// the scripted backend this fixture wired in.
pub struct ScriptedFixture {
    pub app: Router,
    pub db: DatabaseConnection,
    pub engine: std::sync::Arc<SiteEngine>,
    pub cookie: String,
    pub user_id: i32,
}

pub async fn setup_scripted(
    db_url: &str,
    tag: &str,
    llm_factory: entanglement_core::LlmFactory,
) -> ScriptedFixture {
    let db = sea_orm::Database::connect(db_url)
        .await
        .expect("connect to DATABASE_URL");
    build_fixture(db, tag, llm_factory).await
}

/// Same as [`setup_scripted`], but also seeds a throwaway `llm_provider`/
/// `llm_model` row flagged default — never bound to any session via
/// `SetModel` (see [`ScriptedFixture`]'s doc), so it changes nothing about
/// which backend a session's turns run against. It exists purely so
/// `SiteEngine::spawn`'s own `catalog.default_model()` lookup resolves a
/// small `context_window`, letting a test provoke a real context overflow
/// with a short scripted transcript instead of needing to out-talk the
/// engine's generic several-hundred-thousand-token fallback (#40). Returns
/// the provider/model row ids alongside the fixture so the caller can pin a
/// session to the model directly (`assistant_sessions.model_id`) and clean
/// the provider row up afterward (cascades to the model row).
pub async fn setup_scripted_with_context_window(
    db_url: &str,
    tag: &str,
    llm_factory: entanglement_core::LlmFactory,
    context_window: i32,
) -> (ScriptedFixture, i32, i32) {
    let db = sea_orm::Database::connect(db_url)
        .await
        .expect("connect to DATABASE_URL");

    let provider = llm_provider::ActiveModel {
        label: Set(format!("compact-test-{tag}-{}", uuid::Uuid::new_v4())),
        kind: Set("ollama".to_string()),
        api_key: Set(None),
        base_url: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway llm_provider");
    let model = llm_model::ActiveModel {
        provider_id: Set(provider.id),
        label: Set("scripted".to_string()),
        model: Set("scripted".to_string()),
        is_default: Set(true),
        context_window: Set(Some(context_window)),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway llm_model");

    let fixture = build_fixture(db, tag, llm_factory).await;
    (fixture, provider.id, model.id)
}

async fn build_fixture(
    db: DatabaseConnection,
    tag: &str,
    llm_factory: entanglement_core::LlmFactory,
) -> ScriptedFixture {
    let username = format!("assistant-flow-{tag}-{}", uuid::Uuid::new_v4());
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

    let ai_config = std::sync::Arc::new(AiConfig::new());
    let ws_hub = std::sync::Arc::new(WsHub::new());
    let engine = SiteEngine::spawn(
        db.clone(),
        ai_config,
        ws_hub.clone(),
        None,
        Some(llm_factory),
    )
    .await
    .expect("spawn scripted assistant engine");
    site::ai::ws_bridge::spawn(engine.clone(), ws_hub.clone(), db.clone());
    let state = AppState {
        db: db.clone(),
        tmpl: site::templates::Templates::new(std::sync::Arc::new(site::design::DesignStore::new(
            None,
        ))),
        design: std::sync::Arc::new(site::design::DesignStore::new(None)),
        agent_engine: engine.clone(),
        ws_hub,
    };
    let app = site::routes::api::router(state.clone()).with_state(state);

    ScriptedFixture {
        app,
        db,
        engine,
        cookie: format!("{SESSION_COOKIE}={nonce}"),
        user_id: saved_user.id,
    }
}

/// Mint a root engine session and its `assistant_sessions` row directly (see
/// `ScriptedFixture`'s doc for why this bypasses `POST /assistant/sessions`).
pub async fn scripted_session(fx: &ScriptedFixture) -> (i32, entanglement_core::SessionId) {
    let session_id = SiteEngine::session_id_for_user(fx.user_id);
    let now = chrono::Utc::now().fixed_offset();
    let saved = assistant_session::ActiveModel {
        user_id: Set(fx.user_id),
        title: Set("New chat".into()),
        provider: Set("test".into()),
        model: Set("scripted".into()),
        model_id: Set(None),
        enabled_mcp_server_ids: Set(serde_json::json!([])),
        engine_session_id: Set(Some(session_id.0.clone())),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("insert assistant_session row");
    fx.engine.mark_live(session_id.clone());
    (saved.id, session_id)
}

/// Same as [`scripted_session`], but pins `assistant_sessions.model_id` to a
/// real catalog row (from [`setup_scripted_with_context_window`]) instead of
/// leaving it `None` — needed by anything that resolves the session's model
/// back out of the DB (e.g. `POST .../compact`, #40's manual-compaction
/// route, which re-pins the successor's model via `SetModel`).
pub async fn scripted_session_with_model(
    fx: &ScriptedFixture,
    model_id: i32,
) -> (i32, entanglement_core::SessionId) {
    let session_id = SiteEngine::session_id_for_user(fx.user_id);
    let now = chrono::Utc::now().fixed_offset();
    let saved = assistant_session::ActiveModel {
        user_id: Set(fx.user_id),
        title: Set("New chat".into()),
        provider: Set("test".into()),
        model: Set("scripted".into()),
        model_id: Set(Some(model_id)),
        enabled_mcp_server_ids: Set(serde_json::json!([])),
        engine_session_id: Set(Some(session_id.0.clone())),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("insert assistant_session row");
    fx.engine.mark_live(session_id.clone());
    (saved.id, session_id)
}

/// Same as the base flow's `cleanup`, minus the `llm_model`/`llm_provider`
/// rows a `ScriptedFixture` never creates.
pub async fn scripted_cleanup(fx: &ScriptedFixture, session_id: i32) {
    if let Ok(Some(session)) = assistant_session::Entity::find_by_id(session_id)
        .one(&fx.db)
        .await
        && let Some(engine_session_id) = session.engine_session_id
    {
        let sid = entanglement_core::SessionId::new(engine_session_id);
        let _ = site::ai::persistence::delete_session_events(&fx.db, &sid).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let _ = site::ai::persistence::delete_session_events(&fx.db, &sid).await;
    }
    let _ = user::Entity::delete_by_id(fx.user_id).exec(&fx.db).await;
}
