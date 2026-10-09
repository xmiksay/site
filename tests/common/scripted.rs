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
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
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
    build_fixture(db, tag, llm_factory, true).await
}

/// Same as [`setup_scripted`], but **without** `site::ai::ws_bridge::spawn` —
/// the live half of #99's sub-agent row writer. Lets a test prove that the
/// handler-side hydration path (`handlers::sessions::subagent_links`) creates a
/// spawned child's `assistant_sessions` row on its own, rather than passing
/// because the WS task happened to win the race.
///
/// Nothing else in the fixture depends on the bridge: it only forwards engine
/// events to `WsHub`, and no integration test subscribes to that.
pub async fn setup_scripted_without_ws_bridge(
    db_url: &str,
    tag: &str,
    llm_factory: entanglement_core::LlmFactory,
) -> ScriptedFixture {
    let db = sea_orm::Database::connect(db_url)
        .await
        .expect("connect to DATABASE_URL");
    build_fixture(db, tag, llm_factory, false).await
}

/// Label prefix every throwaway catalog row below shares, so
/// [`sweep_stale_throwaway_providers`] can recognize (and a human can grep)
/// them.
const THROWAWAY_LABEL_PREFIX: &str = "compact-test-";

/// Delete throwaway `llm_providers` rows (cascading to their `llm_models`)
/// left behind by an *earlier* run that panicked before its own teardown.
///
/// `site_test` isn't reset between runs and `llm_models.is_default` is
/// process-global state every `SiteEngine::spawn` in the whole suite reads
/// (`EngineConfig.context_window` is seeded from `catalog.default_model()`),
/// so a leaked flagged row silently re-budgets an unrelated test's engine.
/// Sweeping at *setup* rather than only at teardown is what makes that
/// self-healing instead of dependent on a panicking test getting to run its
/// own cleanup. The age cutoff is what keeps this safe to call from a fixture
/// while a *sibling* test in the same binary is concurrently using its own
/// freshly inserted row.
async fn sweep_stale_throwaway_providers(db: &DatabaseConnection) {
    let cutoff = (chrono::Utc::now() - chrono::Duration::minutes(5)).fixed_offset();
    let _ = llm_provider::Entity::delete_many()
        .filter(llm_provider::Column::Label.starts_with(THROWAWAY_LABEL_PREFIX))
        .filter(llm_provider::Column::CreatedAt.lt(cutoff))
        .exec(db)
        .await;
}

/// Insert one throwaway `llm_provider` + `llm_model` pair, returning their ids.
///
/// `is_default` is deliberately a parameter rather than always `true`: it is a
/// *table-wide* flag folded into `SiteCatalog`'s `default_model_id`, so two
/// tests running concurrently in the same binary with two flagged rows would
/// both be claiming the engine-wide default (the lowest-id one wins —
/// `catalog::choose_default_model_id` — which is deterministic but still not
/// necessarily *this* test's row). Only a fixture that actually needs
/// `SiteEngine::spawn` to pick *its* row up should flag it.
///
/// `base_url` points the provider row at an in-process mock instead of a real
/// endpoint (`None` = the default local Ollama). Anything that makes the
/// session's *catalog-resolved* model actually stream — `POST .../compact`
/// re-pins the source with `InMsg::SetModel` before summarizing — needs one,
/// since a catalog-built factory is always a real HTTP client and never the
/// fixture's scripted `Llm`.
async fn insert_throwaway_model(
    db: &DatabaseConnection,
    tag: &str,
    context_window: i32,
    is_default: bool,
    base_url: Option<String>,
) -> (i32, i32) {
    sweep_stale_throwaway_providers(db).await;
    let provider = llm_provider::ActiveModel {
        label: Set(format!(
            "{THROWAWAY_LABEL_PREFIX}{tag}-{}",
            uuid::Uuid::new_v4()
        )),
        kind: Set("ollama".to_string()),
        api_key: Set(None),
        base_url: Set(base_url),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway llm_provider");
    let model = llm_model::ActiveModel {
        provider_id: Set(provider.id),
        label: Set("scripted".to_string()),
        model: Set("scripted".to_string()),
        is_default: Set(is_default),
        context_window: Set(Some(context_window)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway llm_model");
    (provider.id, model.id)
}

/// Same as [`setup_scripted`], but spawns the engine with a small
/// `EngineConfig.context_window`, letting a test provoke a real context
/// overflow with a short scripted transcript instead of needing to out-talk
/// the engine's generic several-hundred-thousand-token fallback (#40).
///
/// That window has exactly one source — `SiteEngine::spawn`'s own
/// `catalog.default_model()` lookup — so it has to come from a real
/// `llm_models` row flagged default. The row is therefore inserted, read once
/// by `SiteCatalog::load` during the spawn, and **deleted again immediately**:
/// `EngineConfig` has frozen the window by then, and no session this fixture
/// mints is ever bound to the row (see [`ScriptedFixture`]'s doc — nothing
/// sends `SetModel`, and `assistant_sessions.model_id` stays `NULL`), so
/// nothing needs it afterwards. Deleting it in setup rather than teardown is
/// what makes the fixture both leak-proof under a panicking test and
/// invisible to any other test's `is_default` resolution.
pub async fn setup_scripted_with_context_window(
    db_url: &str,
    tag: &str,
    llm_factory: entanglement_core::LlmFactory,
    context_window: i32,
) -> ScriptedFixture {
    let db = sea_orm::Database::connect(db_url)
        .await
        .expect("connect to DATABASE_URL");
    let (provider_id, _model_id) =
        insert_throwaway_model(&db, tag, context_window, true, None).await;
    let fixture = build_fixture(db, tag, llm_factory, true).await;
    let _ = llm_provider::Entity::delete_by_id(provider_id)
        .exec(&fixture.db)
        .await;
    fixture
}

/// Same as [`setup_scripted`], but seeds a throwaway `llm_provider`/
/// `llm_model` pair that **stays alive** for the fixture's lifetime, for the
/// one caller that needs a session pinned to a resolvable catalog row:
/// `POST .../compact` (#40) reads `assistant_sessions.model_id` back out of
/// the DB (`handlers/sessions/mutate.rs`'s `resolve_model_with_provider`) and
/// re-pins the engine session to it.
///
/// `base_url` is the endpoint that row resolves to — pass a
/// `common::llm_mock` address to keep the resulting turn hermetic, since the
/// catalog builds a real HTTP client for it either way.
///
/// The model is *not* flagged default — `/compact` resolves it by id, and
/// flagging it would make this fixture claim the table-wide default out from
/// under every other concurrently spawning engine (see
/// [`insert_throwaway_model`]). Returns the provider/model ids so the caller
/// can pin the session and delete the provider row (cascading to the model)
/// afterwards.
pub async fn setup_scripted_with_catalog_model(
    db_url: &str,
    tag: &str,
    llm_factory: entanglement_core::LlmFactory,
    context_window: i32,
    base_url: String,
) -> (ScriptedFixture, i32, i32) {
    let db = sea_orm::Database::connect(db_url)
        .await
        .expect("connect to DATABASE_URL");
    let (provider_id, model_id) =
        insert_throwaway_model(&db, tag, context_window, false, Some(base_url)).await;
    let fixture = build_fixture(db, tag, llm_factory, true).await;
    (fixture, provider_id, model_id)
}

async fn build_fixture(
    db: DatabaseConnection,
    tag: &str,
    llm_factory: entanglement_core::LlmFactory,
    ws_bridge: bool,
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
    let storage = site::storage::Storage::db(db.clone());
    let engine = SiteEngine::spawn(
        db.clone(),
        storage.clone(),
        ai_config,
        ws_hub.clone(),
        None,
        Some(llm_factory),
    )
    .await
    .expect("spawn scripted assistant engine");
    if ws_bridge {
        site::ai::ws_bridge::spawn(engine.clone(), ws_hub.clone(), db.clone());
    }
    let state = AppState {
        db: db.clone(),
        storage,
        tmpl: site::templates::Templates::new(std::sync::Arc::new(site::design::DesignStore::new(
            None,
        ))),
        design: std::sync::Arc::new(site::design::DesignStore::new(None)),
        agent_engine: engine.clone(),
        ws_hub,
        mdcast: None,
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
        // A root session is its own log key (#99, m_032).
        root_engine_session_id: Set(session_id.0.clone()),
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
        // A root session is its own log key (#99, m_032).
        root_engine_session_id: Set(session_id.0.clone()),
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
