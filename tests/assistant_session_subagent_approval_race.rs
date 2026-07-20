//! Regression tests for the stuck-tool-approval bug: `approve`'s routing of a
//! decision to the session that actually owns its pending call
//! (`session_for_call_awaiting` in `src/ai/handlers/sessions/turn.rs`).
//!
//! Two failure modes existed before the fix, both from the same root cause —
//! the old code fell back to the root session the instant a `tool_call_id`
//! wasn't found in the just-loaded `prior: Vec<LogRecord>`:
//!
//! 1. **The race**: `DbSink` (`crate::ai::persistence`) appends every
//!    broadcast `LogRecord` to `assistant_events` behind a bounded channel
//!    and its own async writer task, so there's a real window where a user
//!    has already seen (over WS) and clicked Approve on a sub-agent's
//!    `ToolRequest` whose row hasn't landed in the DB yet when `approve`'s
//!    `load_prior_records` reads history. Falling back to the root in that
//!    case silently resolves nothing (`PendingDecisions` is keyed by
//!    `(session, request_id)`) and the child's real pending call is left
//!    parked forever, invisible from the UI, while `send_and_collect` blocks
//!    the request for the full 180s `TURN_TIMEOUT`.
//! 2. **The unknown id**: a stale/garbage/typo'd `tool_call_id` that never
//!    appears anywhere in the session's history isn't a timing issue at all
//!    — falling back to the root for it is just wrong, and also hangs for
//!    180s before timing out.
//!
//! `session_for_call_awaiting` fixes both: retry a few times against a fresh
//! DB read (closing the race), then fail fast with a `BadRequest` once the
//! call genuinely never shows up (closing the second case) — see its own doc
//! for the exact rationale, mirrored from `src/ai/engine/session_tree.rs`'s
//! analogous `user_id_from_session_awaiting` retry.
//!
//! The race (case 1) needs deterministic control over when a `ToolRequest`
//! row lands in `assistant_events` relative to the lookup — reproducing that
//! through a full live sub-agent turn over the real HTTP API would mean
//! winning a timing race against `DbSink`'s writer task, which isn't
//! reproducible on demand. Instead, this fabricates the "not there yet"
//! `prior` directly (the same technique
//! `tests/assistant_session_subagent_resume.rs` uses to fabricate a
//! replayable log) and calls `session_for_call_awaiting` — re-exported
//! `pub` for exactly this — directly, inserting the real row from a delayed
//! background task. The unknown-id case (case 2) needs no such contrivance:
//! it's exercised through the real `POST .../approve` endpoint.

mod common;
#[path = "common/scripted.rs"]
mod scripted;

use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use common::{send, test_db_url};
use entanglement_core::{
    Llm, LlmRequest, LlmResponse, LlmStream, OutEvent, SessionId, stream_from_response,
};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use scripted::{scripted_cleanup, scripted_session, setup_scripted};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde_json::json;
use site::ai::engine::SiteEngine;
use site::ai::handlers::sessions::session_for_call_awaiting;
use site::ai::persistence::delete_session_events;
use site::entity::{assistant_event, user};

/// Never actually driven — both tests here either bypass the LLM entirely
/// (the race test fabricates its log directly) or never get far enough to
/// need a real reply (the fail-fast test's session has no history at all).
#[derive(Default)]
struct NeverCalledLlm;

#[async_trait]
impl Llm for NeverCalledLlm {
    async fn stream(&mut self, _req: LlmRequest<'_>) -> anyhow::Result<LlmStream> {
        Ok(stream_from_response(LlmResponse {
            text: "unused".into(),
            tool_calls: vec![],
        }))
    }
}

/// Insert one `LogRecord` into `assistant_events` under `root`'s
/// `root_session_id` — the same shape `DbSink`/`entanglement_runtime`'s
/// persistence tap produce, mirrored from
/// `tests/assistant_session_subagent_resume.rs`'s helper of the same name.
async fn insert(db: &sea_orm::DatabaseConnection, root: &SessionId, record: LogRecord) {
    let value = serde_json::to_value(&record).expect("serialize LogRecord");
    assistant_event::ActiveModel {
        root_session_id: Set(root.0.clone()),
        payload: Set(value),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert assistant_event row");
}

/// `approve`'s own `load_prior_records`, reimplemented here rather than
/// exposed from `turn.rs` — it's a plain read with no retry logic of its
/// own, so there's nothing worth widening its visibility for.
async fn load_records(db: &sea_orm::DatabaseConnection, root: &SessionId) -> Vec<LogRecord> {
    let rows = assistant_event::Entity::find()
        .filter(assistant_event::Column::RootSessionId.eq(root.0.clone()))
        .order_by_asc(assistant_event::Column::Id)
        .all(db)
        .await
        .expect("query assistant_events");
    rows.into_iter()
        .map(|r| serde_json::from_value(r.payload).expect("deserialize LogRecord"))
        .collect()
}

async fn cleanup_fabricated(db: &sea_orm::DatabaseConnection, root: &SessionId, user_id: i32) {
    // Same double-delete-with-a-pause shape `scripted_cleanup` uses, in case
    // any late writer is still mid-flight.
    let _ = delete_session_events(db, root).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = delete_session_events(db, root).await;
    let _ = user::Entity::delete_by_id(user_id).exec(db).await;
}

/// Case 1 (the race): a sub-agent child's `page_edit` call lands in
/// `assistant_events` a beat *after* `prior` was snapshotted — exactly the
/// `DbSink`-async-writer TOCTOU window `session_for_call_awaiting` exists to
/// close. The naive one-shot lookup (`prior` alone) must miss; the awaiting
/// version, given the same stale `prior`, must still resolve to the child
/// once its row lands, instead of falling back to the root.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_for_call_awaiting_finds_a_call_that_lands_after_prior_was_read() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let llm_factory: entanglement_core::LlmFactory =
        std::sync::Arc::new(|| Box::new(NeverCalledLlm));
    let fx = setup_scripted(&db_url, "approval-race", llm_factory).await;

    let root = SiteEngine::session_id_for_user(fx.user_id);
    let child = SessionId::new_uuid();

    insert(
        &fx.db,
        &root,
        LogRecord::new(
            root.clone(),
            LogPayload::Out(OutEvent::SessionStarted {
                session: root.clone(),
                parent: None,
                predecessor: None,
                profile: "build".into(),
                model: None,
                root: true,
                ts: 0,
            }),
        ),
    )
    .await;
    insert(
        &fx.db,
        &root,
        LogRecord::new(
            child.clone(),
            LogPayload::Out(OutEvent::SessionStarted {
                session: child.clone(),
                parent: Some(root.clone()),
                predecessor: None,
                profile: "page-writer".into(),
                model: None,
                root: false,
                ts: 0,
            }),
        ),
    )
    .await;

    // The snapshot `approve` would have read right before the client's
    // Approve for "edit-1" arrives — the child's own call doesn't exist here
    // yet.
    let prior = load_records(&fx.db, &root).await;
    assert!(
        !prior.iter().any(|r| matches!(
            &r.payload,
            LogPayload::Out(OutEvent::ToolCall { request_id, .. } | OutEvent::ToolRequest { request_id, .. })
                if request_id == "edit-1"
        )),
        "test setup bug: `edit-1` must not be visible in `prior` yet for this \
         to exercise the retry path at all"
    );

    // `DbSink`'s writer task catching up a beat later, from a detached task —
    // exactly the shape of the real race (the call becomes visible on the WS
    // broadcast before its row is durably committed).
    let db2 = fx.db.clone();
    let root2 = root.clone();
    let child2 = child.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        insert(
            &db2,
            &root2,
            LogRecord::new(
                child2.clone(),
                LogPayload::Out(OutEvent::ToolCall {
                    session: child2.clone(),
                    seq: 1,
                    request_id: "edit-1".into(),
                    tool: "page_edit".into(),
                    input: r#"{"path":"test/whatever","markdown":"x"}"#.into(),
                }),
            ),
        )
        .await;
        insert(
            &db2,
            &root2,
            LogRecord::new(
                child2.clone(),
                LogPayload::Out(OutEvent::ToolRequest {
                    session: child2.clone(),
                    seq: 2,
                    request_id: "edit-1".into(),
                    tool: "page_edit".into(),
                    input: r#"{"path":"test/whatever","markdown":"x"}"#.into(),
                }),
            ),
        )
        .await;
    });

    let resolved = session_for_call_awaiting(&fx.db, &root, &prior, "edit-1")
        .await
        .expect("retry should find the call once DbSink's writer catches up");
    assert_eq!(
        resolved, child,
        "must route to the child that actually owns the call, not fall back to root"
    );

    cleanup_fabricated(&fx.db, &root, fx.user_id).await;
}

/// Case 2 (the unknown id): a `tool_call_id` that never existed anywhere in
/// the session's history must fail fast with a `4xx`, not fall back to the
/// root and hang for the full 180s `TURN_TIMEOUT`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approve_with_an_unknown_tool_call_id_fails_fast() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let llm_factory: entanglement_core::LlmFactory =
        std::sync::Arc::new(|| Box::new(NeverCalledLlm));
    let fx = setup_scripted(&db_url, "approval-race-unknown", llm_factory).await;
    let (db_session_id, _session_id) = scripted_session(&fx).await;

    let start = std::time::Instant::now();
    let (status, resp) = send(
        &fx.app,
        "POST",
        &format!("/assistant/sessions/{db_session_id}/messages/0/approve"),
        &fx.cookie,
        Some(json!({ "decisions": [{ "tool_call_id": "never-issued-id", "approve": true }] })),
    )
    .await;
    let elapsed = start.elapsed();

    assert!(
        status.is_client_error(),
        "expected a 4xx for an unknown tool_call_id, got {status}: {resp}"
    );
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert!(
        elapsed < Duration::from_secs(10),
        "approve took {elapsed:?} — looks like it fell through to the 180s \
         turn timeout instead of failing fast: {resp:#}"
    );

    scripted_cleanup(&fx, db_session_id).await;
}
