//! End-to-end test of the issue's explicit acceptance criterion: create a
//! session, send a message that provokes a real tool call, approve it, and
//! see the turn complete — all through the actual HTTP API (`tower::
//! ServiceExt::oneshot`, no real socket) against the real `site_test`
//! Postgres and a real local Ollama model (`qwen3.5:9b`), no mocking.
//!
//! Gated on two live dependencies, each skipped gracefully (not failed) when
//! unavailable so `cargo test`/`make verify` stays green in an environment
//! without them:
//! - `DATABASE_URL` unset -> skip (same convention as `tests/policy_db.rs`).
//! - `http://localhost:11434` unreachable -> skip (no local Ollama).
//!
//! The #17 sub-agent flows (DB-gated only, no live model needed) live in the
//! sibling `tests/assistant_session_subagent.rs`; shared setup helpers live in
//! `tests/common/mod.rs`.

mod common;

use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use common::{send, test_db_url};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::{Value, json};
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{assistant_session, llm_model, llm_provider, token, user};
use site::state::{self, AppState};

const OLLAMA_BASE: &str = "http://localhost:11434";
const MODEL: &str = "qwen3.5:9b";

async fn ollama_reachable() -> bool {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("build reqwest client");
    client
        .get(format!("{OLLAMA_BASE}/api/tags"))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Build a throwaway logged-in user (row + valid session token) and the app
/// router wired to a fresh `SiteEngine` against `db_url`. Each test gets its
/// own user/session/provider/model rows (unique via a random tag) since
/// `site_test` isn't reset between runs.
struct Fixture {
    app: Router,
    db: DatabaseConnection,
    cookie: String,
    model_id: i32,
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

    let provider_label = format!("test-ollama-{tag}-{}", uuid::Uuid::new_v4());
    let provider = llm_provider::ActiveModel {
        label: Set(provider_label),
        kind: Set("ollama".to_string()),
        api_key: Set(None),
        base_url: Set(Some(format!("{OLLAMA_BASE}/v1"))),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert llm_provider");

    let model = llm_model::ActiveModel {
        provider_id: Set(provider.id),
        label: Set(MODEL.to_string()),
        model: Set(MODEL.to_string()),
        is_default: Set(true),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert llm_model");

    // The engine's model catalog was hydrated at `create_state` time, before
    // these rows existed — refresh so `InMsg::SetModel` can resolve them.
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
        model_id: model.id,
        user_id: saved_user.id,
        provider_id: provider.id,
    }
}

/// Find the first projected `{"role": "assistant", ...}` message whose
/// content carries a non-empty `tool_calls` array — matches `to_detail`'s
/// `MessageView` shape from `src/ai/handlers/sessions/turn.rs`.
fn assistant_tool_call(messages: &[Value]) -> Option<&Value> {
    messages.iter().find(|m| {
        m["role"] == "assistant"
            && m["content"]["tool_calls"]
                .as_array()
                .is_some_and(|c| !c.is_empty())
    })
}

/// The first tool call still awaiting a decision, across every message.
///
/// `requires_approval` (`src/ai/projection/mod.rs`'s `OpenTurn::flush_into`)
/// is a historical marker: once set on a message it stays `true` even after
/// every call in that message is resolved, because a turn can straddle
/// several approve round-trips (the model may issue a fresh call after seeing
/// a prior tool's result). The client mirrors this — `AssistantMessageContent
/// .vue` only renders the approval prompt when `requiresApproval(content) &&
/// decisionFor(content, tc.id) === undefined` — so "still pending" means a
/// `tool_calls[].id` with no matching entry in that *same message's*
/// `decisions` array, not merely the presence of the flag.
fn first_unresolved_call(messages: &[Value]) -> Option<String> {
    messages.iter().find_map(|m| {
        if m["content"]["requires_approval"] != json!(true) {
            return None;
        }
        let decided: std::collections::HashSet<&str> = m["content"]["decisions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d["tool_call_id"].as_str())
            .collect();
        m["content"]["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tc| tc["id"].as_str())
            .find(|id| !decided.contains(id))
            .map(str::to_string)
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_session_then_message_then_approve_completes_the_turn() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    if !ollama_reachable().await {
        eprintln!("skipping: {OLLAMA_BASE} not reachable");
        return;
    }

    let fx = setup(&db_url, "flow").await;

    // 1. Create session.
    let (status, created) = send(
        &fx.app,
        "POST",
        "/assistant/sessions",
        &fx.cookie,
        Some(json!({ "model_id": fx.model_id })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create session: {created}");
    let session_id = created["id"].as_i64().expect("session id");

    // 2. Send a message engineered to reliably trigger a tool call from a
    // small local model: an explicit, unambiguous instruction naming the
    // exact tool and forbidding any other response (verified reliable in the
    // ~90% range against this exact Ollama/model pair via raw
    // `/v1/chat/completions` probes while writing this test). A 9B model
    // occasionally still emits an empty completion instead of the tool call
    // (real, observed non-determinism, not a product bug) — retry the send a
    // few times in the same session before giving up, since that's cheap and
    // keeps the test meaningfully exercising a real tool call/approval round
    // trip instead of either flaking in CI or asserting nothing.
    let prompt = "Call the list_tags tool right now with no arguments. \
                  Only call the tool, do not write any other text.";
    const MAX_ATTEMPTS: u32 = 3;
    let mut detail = Value::Null;
    let mut tool_msg: Option<Value> = None;
    for attempt in 1..=MAX_ATTEMPTS {
        let (status, resp) = send(
            &fx.app,
            "POST",
            &format!("/assistant/sessions/{session_id}/messages"),
            &fx.cookie,
            Some(json!({ "text": prompt })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "send_message: {resp}");
        let messages = resp["messages"].as_array().cloned().unwrap_or_default();
        tool_msg = assistant_tool_call(&messages).cloned();
        detail = resp;
        if tool_msg.is_some() {
            break;
        }
        eprintln!(
            "attempt {attempt}/{MAX_ATTEMPTS}: model produced no tool call, retrying: {detail:#}"
        );
    }
    let tool_msg = tool_msg.unwrap_or_else(|| {
        panic!("no assistant message with tool_calls after {MAX_ATTEMPTS} attempts: {detail:#}")
    });
    assert_eq!(
        tool_msg["content"]["requires_approval"],
        json!(true),
        "fresh user has no tool_permissions rules, so list_tags must default to Ask/requires_approval: {detail:#}"
    );
    // 3. Approve every call as it comes up. A small model sometimes issues a
    // second tool call after seeing the first result (real, observed
    // non-determinism — see the module doc on `first_unresolved_call`), so
    // this round-trips until nothing is left pending rather than assuming a
    // single approve settles the turn.
    const MAX_ROUNDS: u32 = 4;
    let mut approved = detail.clone();
    let mut rounds = 0;
    while let Some(call_id) =
        first_unresolved_call(approved["messages"].as_array().expect("messages array"))
    {
        rounds += 1;
        assert!(
            rounds <= MAX_ROUNDS,
            "turn still has an unresolved approval after {MAX_ROUNDS} rounds: {approved:#}"
        );
        let (status, resp) = send(
            &fx.app,
            "POST",
            &format!("/assistant/sessions/{session_id}/messages/0/approve"),
            &fx.cookie,
            Some(json!({ "decisions": [{ "tool_call_id": call_id, "approve": true }] })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "approve round {rounds}: {resp}");
        approved = resp;
    }
    assert!(
        rounds > 0,
        "no tool call ever required approval: {detail:#}"
    );

    let messages = approved["messages"].as_array().expect("messages array");
    assert!(
        messages.iter().any(|m| m["role"] == "tool_result"),
        "no tool_result message after approval: {approved:#}"
    );
    assert!(
        !messages.iter().any(|m| m["role"] == "error"),
        "turn ended in an error after approval: {approved:#}"
    );
    let last = messages.last().expect("at least one message");
    // The completion criterion is "the turn settles with no pending
    // approval" (the issue's acceptance criterion), not a specific trailing
    // chat bubble: given this exact prompt's "do not write any other text"
    // instruction, a small local model sometimes carries that instruction
    // into its post-tool-result turn too and emits no further text, ending
    // the turn right on `tool_result` (observed directly while writing this
    // test — real model non-determinism, not a product bug). So the last
    // message is allowed to be either the `tool_result` itself or a further
    // `assistant` reply, but nothing else (e.g. a still-open approval).
    assert!(
        matches!(
            last["role"].as_str(),
            Some("tool_result") | Some("assistant")
        ),
        "turn ended on an unexpected message role: {approved:#}"
    );

    cleanup(&fx, session_id as i32).await;
}

/// Best-effort teardown: purge the `assistant_events` log (not FK-linked, so
/// not covered by the user cascade), then delete the throwaway user
/// (cascades `assistant_sessions`/`tool_permissions`) and the provider/model
/// rows this test created (not user-scoped, so not covered by that cascade
/// either) — `site_test` isn't reset between runs, so this is what keeps
/// repeated runs from accumulating rows.
async fn cleanup(fx: &Fixture, session_id: i32) {
    if let Ok(Some(session)) = assistant_session::Entity::find_by_id(session_id)
        .one(&fx.db)
        .await
        && let Some(engine_session_id) = session.engine_session_id
    {
        let sid = entanglement_core::SessionId::new(engine_session_id);
        // `DbSink` appends asynchronously behind its own writer task (see
        // `persistence.rs`'s module doc) — the turn's trailing `Done`/
        // `Status` records can still be in flight for a moment after the
        // approve response returns. One delete-then-settle-then-delete pass
        // is enough to also catch those instead of leaving a couple of
        // orphaned rows behind on every run.
        let _ = site::ai::persistence::delete_session_events(&fx.db, &sid).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = site::ai::persistence::delete_session_events(&fx.db, &sid).await;
    }
    let _ = user::Entity::delete_by_id(fx.user_id).exec(&fx.db).await;
    let _ = llm_model::Entity::delete_by_id(fx.model_id)
        .exec(&fx.db)
        .await;
    let _ = llm_provider::Entity::delete_by_id(fx.provider_id)
        .exec(&fx.db)
        .await;
}
