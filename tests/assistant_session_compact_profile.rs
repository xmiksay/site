//! #118: `/compact` keeps the chat's agent profile. The successor is spawned
//! under the session's own profile (not the root `build`), so a Designer or
//! Researcher chat keeps running as one — checked on the engine side (the
//! tool executor's active-profile map and the system prompt its seeded first
//! turn saw), not just the DB row. A same-value `PATCH agent_profile` is a
//! no-op that sends no `SetAgent`, and switching a chat with history into
//! `designer` stays a 409. Scripted `Llm` plus the loopback catalog model
//! for the summarize oneshot, as in `assistant_session_compact.rs`.

mod common;
#[path = "common/llm_mock.rs"]
mod llm_mock;
#[path = "common/scripted.rs"]
mod scripted;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use common::{send, test_db_url};
use entanglement_core::stream_from_response;
use entanglement_core::{Llm, LlmRequest, LlmResponse, LlmStream, OutEvent, SessionId};
use llm_mock::spawn_openai_sse_mock;
use scripted::{
    ScriptedFixture, scripted_cleanup, scripted_session_with_model,
    setup_scripted_with_catalog_model,
};
use sea_orm::EntityTrait;
use serde_json::{Value, json};
use site::entity::{assistant_session, llm_provider};

/// Every non-summarize system prompt the scripted backend saw, in order.
type Seen = Arc<Mutex<Vec<String>>>;

struct RecordingLlm(Seen);

#[async_trait]
impl Llm for RecordingLlm {
    async fn stream(&mut self, req: LlmRequest<'_>) -> anyhow::Result<LlmStream> {
        if !req.system.contains("summarization assistant")
            && let Ok(mut seen) = self.0.lock()
        {
            seen.push(req.system.to_string());
        }
        Ok(stream_from_response(LlmResponse {
            text: "pong".into(),
            tool_calls: vec![],
        }))
    }
}

async fn call(fx: &ScriptedFixture, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    send(&fx.app, method, uri, &fx.cookie, Some(body)).await
}

async fn engine_id(fx: &ScriptedFixture, id: i32) -> SessionId {
    let row = assistant_session::Entity::find_by_id(id)
        .one(&fx.db)
        .await
        .expect("load session")
        .expect("session row");
    SessionId::new(row.engine_session_id.expect("engine session id"))
}

/// The profile the tool executor has recorded for `session`, waiting for it
/// to fold the successor's `SessionStarted`.
async fn engine_profile(fx: &ScriptedFixture, session: &SessionId) -> Option<String> {
    for _ in 0..50 {
        let active = fx.engine.policy.active_profiles();
        let name = active
            .lock()
            .ok()
            .and_then(|map| map.get(session).map(|p| p.name.clone()));
        if name.is_some() {
            return name;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

/// Make a fresh chat run as `profile`, give it a turn, compact it; returns
/// the DB id, the retired source and the successor.
async fn compacted_chat(
    fx: &ScriptedFixture,
    model_id: i32,
    profile: &str,
) -> (i32, SessionId, SessionId) {
    let (id, source) = scripted_session_with_model(fx, model_id).await;
    let uri = format!("/assistant/sessions/{id}");
    let (status, body) = call(fx, "PATCH", &uri, json!({ "agent_profile": profile })).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "switch fresh chat to {profile}: {body}"
    );
    let (status, body) = call(
        fx,
        "POST",
        &format!("{uri}/messages"),
        json!({ "text": "ping" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "seed turn: {body}");
    let (status, body) = call(fx, "POST", &format!("{uri}/compact"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "compact: {body}");
    let successor = engine_id(fx, id).await;
    assert_ne!(successor, source);
    (id, source, successor)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compaction_keeps_the_profile_and_same_value_patch_is_a_no_op() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let mock = spawn_openai_sse_mock("SUMMARY: pinged.", Duration::from_millis(0)).await;
    let seen: Seen = Arc::default();
    let factory_seen = seen.clone();
    let (fx, provider_id, model_id) = setup_scripted_with_catalog_model(
        &db_url,
        "compact-profile",
        Arc::new(move || Box::new(RecordingLlm(factory_seen.clone())) as Box<dyn Llm>),
        200_000,
        mock.base_url.clone(),
    )
    .await;

    let mut ids = Vec::new();
    let mut sources = Vec::new();
    for (profile, marker) in [
        ("designer", "`designer` sub-agent"),
        ("researcher", "`researcher` sub-agent"),
    ] {
        let (id, source, successor) = compacted_chat(&fx, model_id, profile).await;
        assert_eq!(
            engine_profile(&fx, &successor).await.as_deref(),
            Some(profile),
            "the successor must run as {profile}"
        );
        let last = seen
            .lock()
            .ok()
            .and_then(|s| s.last().cloned())
            .unwrap_or_default();
        assert!(
            last.contains(marker),
            "seeded turn ran without the {profile} prompt"
        );
        ids.push((id, successor));
        sources.push(source);
    }
    let (designer_id, designer_successor) = ids[0].clone();
    let (researcher_id, _) = ids[1].clone();

    // Same-value PATCH on the compacted Designer chat: OK, no `SetAgent`.
    let mut sub = fx.engine.holly.subscribe();
    let uri = format!("/assistant/sessions/{designer_id}");
    let (status, body) = call(&fx, "PATCH", &uri, json!({ "agent_profile": "designer" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["agent_profile"], "designer");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while let Ok(Ok(ev)) = tokio::time::timeout_at(deadline, sub.recv()).await {
        if let OutEvent::AgentChanged { session, .. } = &ev {
            assert_ne!(session, &designer_successor, "a no-op PATCH sent SetAgent");
        }
    }

    // Switching a chat with history into designer is still refused.
    let uri = format!("/assistant/sessions/{researcher_id}");
    let (status, body) = call(&fx, "PATCH", &uri, json!({ "agent_profile": "designer" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    for (id, _) in &ids {
        scripted_cleanup(&fx, *id).await;
    }
    for source in &sources {
        let _ = site::ai::persistence::delete_session_events(&fx.db, source).await;
    }
    let _ = llm_provider::Entity::delete_by_id(provider_id)
        .exec(&fx.db)
        .await;
}
