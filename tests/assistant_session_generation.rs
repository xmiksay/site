//! Issue #42's explicit acceptance criteria, exercised through the real HTTP
//! API against the real `site_test` Postgres — no live LLM backend needed:
//! `InMsg::SetModel`/`SetAgent`/`SetGeneration` never call the model, they
//! only re-resolve/rebind, so unlike `assistant_session_base.rs` this file has
//! no Ollama-reachability gate, only the `DATABASE_URL` one.
//!
//! - `PATCH /assistant/sessions/{id}` with a new `model_id` rebinds the live
//!   engine session without a restart and broadcasts `OutEvent::ModelChanged`.
//! - The same endpoint with `temperature`/`reasoning_effort` merges partial
//!   generation overrides onto the session's existing knobs and broadcasts
//!   `OutEvent::GenerationChanged` carrying the **full** merged params, not
//!   just what this call sent.
//! - `agent_profile` switches the session's live profile via `InMsg::SetAgent`
//!   and broadcasts `OutEvent::AgentChanged`; an unknown profile name (or
//!   `reasoning_effort` value) is rejected with `400` before anything is
//!   written.

mod common;

use std::time::Duration;

use axum::Router;
use common::{send, test_db_url};
use entanglement_core::{OutEvent, ReasoningEffort, SessionId};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::json;
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{assistant_session, llm_model, llm_provider, token, user};
use site::state::{self, AppState};

const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

struct Fixture {
    app: Router,
    db: DatabaseConnection,
    state: AppState,
    cookie: String,
    model_id: i32,
    model_id_2: i32,
    provider_label: String,
    user_id: i32,
    provider_id: i32,
}

async fn setup(db_url: &str, tag: &str) -> Fixture {
    let config = Config {
        database_url: db_url.to_string(),
        design_dir: None,
        serper_api_key: None,
    };
    let state: AppState = state::create_state(&config).await;
    let db = state.db.clone();

    let username = format!("assistant-gen-{tag}-{}", uuid::Uuid::new_v4());
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

    let provider_label = format!("test-gen-{tag}-{}", uuid::Uuid::new_v4());
    let provider = llm_provider::ActiveModel {
        label: Set(provider_label.clone()),
        kind: Set("ollama".to_string()),
        api_key: Set(None),
        base_url: Set(Some("http://localhost:11434/v1".to_string())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert llm_provider");

    let model = llm_model::ActiveModel {
        provider_id: Set(provider.id),
        label: Set("model-a".to_string()),
        model: Set("model-a".to_string()),
        is_default: Set(true),
        // This fixture's whole point is round-tripping temperature/
        // reasoning_effort/thinking_budget_tokens through PATCH (#53's
        // capability gate rejects unsupported knobs, so this model must
        // explicitly declare support for all three rather than rely on the
        // migration's opt-in defaults for the latter two).
        supports_temperature: Set(true),
        supports_reasoning_effort: Set(true),
        supports_thinking: Set(true),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert llm_model a");
    let model_2 = llm_model::ActiveModel {
        provider_id: Set(provider.id),
        label: Set("model-b".to_string()),
        model: Set("model-b".to_string()),
        is_default: Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert llm_model b");

    // The engine's model catalog was hydrated at `create_state` time, before
    // these rows existed — refresh so `InMsg::SetModel` can resolve them.
    state
        .agent_engine
        .catalog
        .refresh()
        .await
        .expect("refresh model catalog");

    let app = site::routes::api::router(state.clone()).with_state(state.clone());

    Fixture {
        app,
        db,
        state,
        cookie: format!("{SESSION_COOKIE}={nonce}"),
        model_id: model.id,
        model_id_2: model_2.id,
        provider_label,
        user_id: saved_user.id,
        provider_id: provider.id,
    }
}

/// Block until an `OutEvent` for `session` satisfying `pred` arrives on `sub`,
/// ignoring every other (unrelated or non-matching) broadcast in between —
/// mirrors `entanglement-core`'s own test-suite pattern (`drain_until` in its
/// `tests/agent_model_pin.rs`/`tests/set_generation.rs`) since those live in a
/// dependency this repo doesn't re-run, only wires.
async fn wait_for(
    sub: &mut tokio::sync::broadcast::Receiver<OutEvent>,
    session: &SessionId,
    mut pred: impl FnMut(&OutEvent) -> bool,
) -> OutEvent {
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for matching event");
        match tokio::time::timeout(remaining, sub.recv()).await {
            Ok(Ok(ev)) if ev.session() == Some(session) && pred(&ev) => return ev,
            Ok(Ok(_)) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(e)) => panic!("engine broadcast closed: {e}"),
            Err(_) => panic!("timed out waiting for matching event"),
        }
    }
}

async fn engine_session_id(db: &DatabaseConnection, session_db_id: i32) -> SessionId {
    let row = assistant_session::Entity::find_by_id(session_db_id)
        .one(db)
        .await
        .expect("query session")
        .expect("session exists");
    SessionId::new(row.engine_session_id.expect("engine_session_id set"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_session_model_rebinds_live_and_emits_model_changed() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "model").await;

    let (status, created) = send(
        &fx.app,
        "POST",
        "/assistant/sessions",
        &fx.cookie,
        Some(json!({ "model_id": fx.model_id })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "create: {created}");
    let session_db_id = created["id"].as_i64().expect("session id") as i32;
    let session_id = engine_session_id(&fx.db, session_db_id).await;

    let mut sub = fx.state.agent_engine.holly.subscribe();
    let (status, updated) = send(
        &fx.app,
        "PATCH",
        &format!("/assistant/sessions/{session_db_id}"),
        &fx.cookie,
        Some(json!({ "model_id": fx.model_id_2 })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "update: {updated}");
    assert_eq!(updated["model_id"], json!(fx.model_id_2));

    let ev = wait_for(&mut sub, &session_id, |ev| {
        matches!(ev, OutEvent::ModelChanged { .. })
    })
    .await;
    match ev {
        OutEvent::ModelChanged {
            provider, model, ..
        } => {
            assert_eq!(provider, fx.provider_label);
            // `ResolvedModel::model` (`catalog.rs`'s `model_resolver`) is the
            // wire model string, not the catalog row id used as the lookup key
            // on the way in.
            assert_eq!(model, "model-b");
        }
        other => panic!("expected ModelChanged, got {other:?}"),
    }

    cleanup(&fx, session_db_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_session_generation_merges_partial_overrides() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "gen").await;

    let (status, created) = send(
        &fx.app,
        "POST",
        "/assistant/sessions",
        &fx.cookie,
        Some(json!({ "model_id": fx.model_id })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "create: {created}");
    let session_db_id = created["id"].as_i64().expect("session id") as i32;
    let session_id = engine_session_id(&fx.db, session_db_id).await;

    // First partial override: temperature only.
    let mut sub = fx.state.agent_engine.holly.subscribe();
    let (status, updated) = send(
        &fx.app,
        "PATCH",
        &format!("/assistant/sessions/{session_db_id}"),
        &fx.cookie,
        Some(json!({ "temperature": 0.3 })),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "update temperature: {updated}"
    );
    assert_eq!(updated["temperature"], json!(0.3));
    assert_eq!(updated["reasoning_effort"], json!(null));

    let ev = wait_for(&mut sub, &session_id, |ev| {
        matches!(ev, OutEvent::GenerationChanged { .. })
    })
    .await;
    match &ev {
        OutEvent::GenerationChanged { generation, .. } => {
            assert_eq!(generation.temperature, Some(0.3));
            assert_eq!(generation.reasoning_effort, None);
        }
        other => panic!("expected GenerationChanged, got {other:?}"),
    }

    // Second partial override: reasoning_effort only. The emitted event must
    // carry the FULL merged params — temperature from the first call must
    // still be present, not clobbered by this call's own partial input.
    let (status, updated) = send(
        &fx.app,
        "PATCH",
        &format!("/assistant/sessions/{session_db_id}"),
        &fx.cookie,
        Some(json!({ "reasoning_effort": "high" })),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "update reasoning_effort: {updated}"
    );
    assert_eq!(
        updated["temperature"],
        json!(0.3),
        "temperature must survive a reasoning_effort-only patch"
    );
    assert_eq!(updated["reasoning_effort"], json!("high"));

    let ev = wait_for(&mut sub, &session_id, |ev| {
        matches!(ev, OutEvent::GenerationChanged { .. })
    })
    .await;
    match ev {
        OutEvent::GenerationChanged { generation, .. } => {
            assert_eq!(
                generation.temperature,
                Some(0.3),
                "GenerationChanged must report the merged, full params, not just this call's override"
            );
            assert_eq!(generation.reasoning_effort, Some(ReasoningEffort::High));
        }
        other => panic!("expected GenerationChanged, got {other:?}"),
    }

    // Third partial override: max_output_tokens/thinking_budget_tokens only.
    // Same merge contract as above — temperature/reasoning_effort from the
    // earlier calls must survive.
    let (status, updated) = send(
        &fx.app,
        "PATCH",
        &format!("/assistant/sessions/{session_db_id}"),
        &fx.cookie,
        Some(json!({ "max_output_tokens": 512, "thinking_budget_tokens": 1024 })),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "update max_output_tokens/thinking_budget_tokens: {updated}"
    );
    assert_eq!(
        updated["temperature"],
        json!(0.3),
        "temperature must survive a max_output_tokens/thinking_budget_tokens-only patch"
    );
    assert_eq!(updated["reasoning_effort"], json!("high"));
    assert_eq!(updated["max_output_tokens"], json!(512));
    assert_eq!(updated["thinking_budget_tokens"], json!(1024));

    let ev = wait_for(&mut sub, &session_id, |ev| {
        matches!(ev, OutEvent::GenerationChanged { .. })
    })
    .await;
    match ev {
        OutEvent::GenerationChanged { generation, .. } => {
            assert_eq!(
                generation.temperature,
                Some(0.3),
                "GenerationChanged must report the merged, full params, not just this call's override"
            );
            assert_eq!(generation.reasoning_effort, Some(ReasoningEffort::High));
            assert_eq!(generation.max_output_tokens, Some(512));
            assert_eq!(generation.thinking_budget_tokens, Some(1024));
        }
        other => panic!("expected GenerationChanged, got {other:?}"),
    }

    // A zero value for either new field must be rejected with 400 before
    // anything is written (mirrors the unknown reasoning_effort/agent_profile
    // 400s already covered elsewhere in this file).
    let (status, rejected) = send(
        &fx.app,
        "PATCH",
        &format!("/assistant/sessions/{session_db_id}"),
        &fx.cookie,
        Some(json!({ "max_output_tokens": 0 })),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::BAD_REQUEST,
        "max_output_tokens: 0 must be rejected: {rejected}"
    );

    cleanup(&fx, session_db_id).await;
}

/// #54: a model-only `PATCH` (no generation fields in the request body) must
/// not silently wipe the session's existing overrides — `rebind()`
/// (`entanglement-core`) rebuilds the live session's `generation` from
/// scratch on every `SetModel`, and this site's `ModelResolver` always
/// resolves to `generation: None` (it has no session handle to read the
/// prior value from), so without `generation_after_model_switch` carrying
/// the row's overrides forward, this `PATCH` would rebind with no follow-up
/// `SetGeneration` at all and `temperature` would vanish from the live
/// session with no broadcast to say so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_session_model_switch_preserves_generation_overrides() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "switch").await;

    let (status, created) = send(
        &fx.app,
        "POST",
        "/assistant/sessions",
        &fx.cookie,
        Some(json!({ "model_id": fx.model_id })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "create: {created}");
    let session_db_id = created["id"].as_i64().expect("session id") as i32;
    let session_id = engine_session_id(&fx.db, session_db_id).await;

    // model-a (fx.model_id) supports all three knobs — set all of them.
    let mut sub = fx.state.agent_engine.holly.subscribe();
    let (status, updated) = send(
        &fx.app,
        "PATCH",
        &format!("/assistant/sessions/{session_db_id}"),
        &fx.cookie,
        Some(json!({
            "temperature": 0.3,
            "reasoning_effort": "high",
            "thinking_budget_tokens": 1024,
        })),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "set overrides: {updated}"
    );
    wait_for(&mut sub, &session_id, |ev| {
        matches!(ev, OutEvent::GenerationChanged { .. })
    })
    .await;

    // model-b (fx.model_id_2) only declares `supports_temperature` (m_029's
    // opt-in defaults) — switching to it, model-only, must carry `temperature`
    // forward and drop `reasoning_effort`/`thinking_budget_tokens` rather than
    // silently dropping all three or erroring.
    let (status, updated) = send(
        &fx.app,
        "PATCH",
        &format!("/assistant/sessions/{session_db_id}"),
        &fx.cookie,
        Some(json!({ "model_id": fx.model_id_2 })),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "switch model: {updated}"
    );
    assert_eq!(
        updated["temperature"],
        json!(0.3),
        "the row's own override must survive a model-only PATCH"
    );

    wait_for(&mut sub, &session_id, |ev| {
        matches!(ev, OutEvent::ModelChanged { .. })
    })
    .await;
    let ev = wait_for(&mut sub, &session_id, |ev| {
        matches!(ev, OutEvent::GenerationChanged { .. })
    })
    .await;
    match ev {
        OutEvent::GenerationChanged { generation, .. } => {
            assert_eq!(
                generation.temperature,
                Some(0.3),
                "temperature must be re-applied to the live session across the switch"
            );
            assert_eq!(
                generation.reasoning_effort, None,
                "model-b doesn't support reasoning_effort — must be dropped, not resurrected"
            );
            assert_eq!(
                generation.thinking_budget_tokens, None,
                "model-b doesn't support thinking — must be dropped, not resurrected"
            );
        }
        other => panic!("expected GenerationChanged after the switch, got {other:?}"),
    }

    cleanup(&fx, session_db_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_session_agent_profile_switches_live_and_rejects_unknown() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let fx = setup(&db_url, "agent").await;

    let (status, created) = send(
        &fx.app,
        "POST",
        "/assistant/sessions",
        &fx.cookie,
        Some(json!({ "model_id": fx.model_id })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "create: {created}");
    let session_db_id = created["id"].as_i64().expect("session id") as i32;
    assert_eq!(created["agent_profile"], json!("build"));
    let session_id = engine_session_id(&fx.db, session_db_id).await;

    let mut sub = fx.state.agent_engine.holly.subscribe();
    let (status, updated) = send(
        &fx.app,
        "PATCH",
        &format!("/assistant/sessions/{session_db_id}"),
        &fx.cookie,
        Some(json!({ "agent_profile": "researcher" })),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "update agent: {updated}"
    );
    assert_eq!(updated["agent_profile"], json!("researcher"));

    let ev = wait_for(&mut sub, &session_id, |ev| {
        matches!(ev, OutEvent::AgentChanged { .. })
    })
    .await;
    match ev {
        OutEvent::AgentChanged { agent, .. } => assert_eq!(agent, "researcher"),
        other => panic!("expected AgentChanged, got {other:?}"),
    }

    // Unknown values are rejected outright — no partial write, no engine send.
    let (status, resp) = send(
        &fx.app,
        "PATCH",
        &format!("/assistant/sessions/{session_db_id}"),
        &fx.cookie,
        Some(json!({ "agent_profile": "not-a-real-profile" })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{resp}");

    let (status, resp) = send(
        &fx.app,
        "PATCH",
        &format!("/assistant/sessions/{session_db_id}"),
        &fx.cookie,
        Some(json!({ "reasoning_effort": "extreme" })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{resp}");

    cleanup(&fx, session_db_id).await;
}

/// Best-effort teardown, mirroring `assistant_session_base.rs`'s `cleanup`.
async fn cleanup(fx: &Fixture, session_id: i32) {
    if let Ok(Some(session)) = assistant_session::Entity::find_by_id(session_id)
        .one(&fx.db)
        .await
        && let Some(engine_session_id) = session.engine_session_id
    {
        let sid = SessionId::new(engine_session_id);
        let _ = site::ai::persistence::delete_session_events(&fx.db, &sid).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = site::ai::persistence::delete_session_events(&fx.db, &sid).await;
    }
    let _ = user::Entity::delete_by_id(fx.user_id).exec(&fx.db).await;
    let _ = llm_model::Entity::delete_by_id(fx.model_id)
        .exec(&fx.db)
        .await;
    let _ = llm_model::Entity::delete_by_id(fx.model_id_2)
        .exec(&fx.db)
        .await;
    let _ = llm_provider::Entity::delete_by_id(fx.provider_id)
        .exec(&fx.db)
        .await;
}
