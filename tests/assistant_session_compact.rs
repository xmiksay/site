//! Context compaction (issue #40), DB-gated only (a scripted `Llm`, same
//! pattern as `tests/assistant_session_subagent_*.rs` — no live Ollama
//! needed for a deterministic response). Two acceptance criteria:
//!
//! - `overflow_auto_summarizes_instead_of_pruning`: a tiny `context_window`
//!   (from a throwaway `llm_models` row, `setup_scripted_with_context_window`)
//!   forces a real overflow after a few short turns; asserts the engine
//!   emitted `OutEvent::Compacted { auto: true, .. }` (only ever emitted by
//!   the LLM-summarization path, `entanglement_core::session::turn::
//!   try_auto_compact` — the lossy placeholder-prune fallback emits no event
//!   at all) rather than silently pruning.
//! - `manual_compact_forks_a_successor_and_retires_the_source`: `POST
//!   .../compact` forks a fresh engine session (copy-on-write, ADR-0101/
//!   0110), repoints the DB row's `engine_session_id` at it, and leaves the
//!   source's own `assistant_events` log intact but unreachable. A follow-up
//!   plain `GET` on the same DB id proves projection follows the successor
//!   (not the now-retired source).

mod common;
#[path = "common/scripted.rs"]
mod scripted;

use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use common::{send, test_db_url};
use entanglement_core::{Llm, LlmRequest, LlmResponse, LlmStream, OutEvent, stream_from_response};
use entanglement_runtime::session_store::LogPayload;
use scripted::{
    ScriptedFixture, scripted_cleanup, scripted_session_with_model,
    setup_scripted_with_context_window,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde_json::json;
use site::entity::{assistant_event, assistant_session, llm_provider};

/// Fixed-length so each turn's token contribution is exactly predictable —
/// see the module doc's worked budget in `overflow_auto_summarizes_instead_
/// of_pruning`. 100 chars each: long enough that `render_transcript`'s
/// per-message `"[role]\n...\n\n"` wrapper overhead (the summarizer's own
/// budget check, `entanglement_core::session::summarize::summarize`) stays a
/// small fraction of the message body — too short a fixed length and that
/// overhead alone pushes the *summarized head* over budget even though the
/// raw context total just barely crossed it.
const FIXED_LEN_PROMPT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"; // 100 chars
const FIXED_LEN_REPLY: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"; // 100 chars

struct OverflowScriptedLlm;

#[async_trait]
impl Llm for OverflowScriptedLlm {
    async fn stream(&mut self, req: LlmRequest<'_>) -> anyhow::Result<LlmStream> {
        let resp = if req.system.contains("summarization assistant") {
            LlmResponse {
                text: "auto-summary: repeated pings exchanged.".into(),
                tool_calls: vec![],
            }
        } else {
            LlmResponse {
                text: FIXED_LEN_REPLY.to_string(),
                tool_calls: vec![],
            }
        };
        Ok(stream_from_response(resp))
    }
}

#[derive(Default)]
struct CompactScriptedLlm {
    calls: u32,
}

#[async_trait]
impl Llm for CompactScriptedLlm {
    async fn stream(&mut self, req: LlmRequest<'_>) -> anyhow::Result<LlmStream> {
        self.calls += 1;
        let resp = if req.system.contains("summarization assistant") {
            LlmResponse {
                text: "SUMMARY_MARKER: user pinged, agent ponged.".into(),
                tool_calls: vec![],
            }
        } else {
            LlmResponse {
                text: format!("REPLY_MARKER-{}", self.calls),
                tool_calls: vec![],
            }
        };
        Ok(stream_from_response(resp))
    }
}

/// Every `assistant_events` row for `root`, oldest first, deserialized as
/// `LogRecord`s — mirrors `ai::persistence::resume_session`'s own read.
async fn events_for(
    fx: &ScriptedFixture,
    root: &str,
) -> Vec<entanglement_runtime::session_store::LogRecord> {
    assistant_event::Entity::find()
        .filter(assistant_event::Column::RootSessionId.eq(root))
        .order_by_asc(assistant_event::Column::Id)
        .all(&fx.db)
        .await
        .expect("query assistant_events")
        .into_iter()
        .map(|r| serde_json::from_value(r.payload).expect("deserialize LogRecord"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overflow_auto_summarizes_instead_of_pruning() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    // context_window = 200 -> limit = floor(200 * 0.85) = 170 tokens.
    // `Context::estimated_tokens` sums *all* messages' raw chars first and
    // ceils once at the end (not per message). The overflow check runs at the
    // start of each round, right after that turn's new prompt lands but
    // before its reply (`session.rs` pushes the prompt before calling
    // `drive_turn`) — so it isn't re-checked between turns. After 3 complete
    // turns (6 messages, 600 chars) the total is already 172 tokens, over
    // budget, but nothing notices until turn 4 starts: its prompt push brings
    // the total to 7 messages / 700 chars = 200 tokens, and *that's* when
    // `try_auto_compact` fires — with a genuine 4-message head to summarize
    // (`AUTO_COMPACT_KEEP_TAIL` = 4, clamped to a safe turn boundary — see
    // `entanglement_core::context::Context::safe_kept`). Both the head (4
    // messages, ~128 tokens once wrapped by `render_transcript`) and the kept
    // tail (3 messages, ~95 tokens) fit comfortably under the same 170-token
    // budget the summarizer itself checks against, so `summarize` succeeds
    // instead of refusing with `TranscriptTooLarge`/`TailTooLarge`.
    let (fx, provider_id, model_id) = setup_scripted_with_context_window(
        &db_url,
        "overflow",
        std::sync::Arc::new(|| Box::new(OverflowScriptedLlm) as Box<dyn Llm>),
        200,
    )
    .await;
    let (session_db_id, engine_session_id) = scripted_session_with_model(&fx, model_id).await;

    for turn in 1..=4 {
        let (status, resp) = send(
            &fx.app,
            "POST",
            &format!("/assistant/sessions/{session_db_id}/messages"),
            &fx.cookie,
            Some(json!({ "text": FIXED_LEN_PROMPT })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "turn {turn}: {resp}");
        assert!(
            !resp["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|m| m["role"] == "error"),
            "turn {turn} ended in an error: {resp:#}"
        );
    }

    // `DbSink` appends asynchronously behind its own writer task
    // (`persistence.rs`'s module doc) — the last turn's trailing records can
    // still be in flight for a moment after its HTTP response returns.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let records = events_for(&fx, &engine_session_id.0).await;
    let auto_compacted = records.iter().any(|r| {
        matches!(
            &r.payload,
            LogPayload::Out(OutEvent::Compacted { auto: true, .. })
        )
    });
    assert!(
        auto_compacted,
        "expected an auto (LLM-summarized) Compacted event among the persisted records; \
         records: {records:#?}"
    );
    // The lossy prune fallback (`Context::compact`) never emits any event —
    // its absence alongside the assertion above is exactly what distinguishes
    // "auto-summarized" from "silently pruned" for this test.

    scripted_cleanup(&fx, session_db_id).await;
    let _ = llm_provider::Entity::delete_by_id(provider_id)
        .exec(&fx.db)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_compact_forks_a_successor_and_retires_the_source() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    // A generous window — this test is about the fork/retire mechanics, not
    // budget arithmetic (that's `overflow_auto_summarizes_instead_of_pruning`).
    let (fx, provider_id, model_id) = setup_scripted_with_context_window(
        &db_url,
        "manual-compact",
        std::sync::Arc::new(|| Box::new(CompactScriptedLlm::default()) as Box<dyn Llm>),
        200_000,
    )
    .await;
    let (session_db_id, source_session_id) = scripted_session_with_model(&fx, model_id).await;

    let (status, resp) = send(
        &fx.app,
        "POST",
        &format!("/assistant/sessions/{session_db_id}/messages"),
        &fx.cookie,
        Some(json!({ "text": "ping" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "seed message: {resp}");

    let (status, compacted) = send(
        &fx.app,
        "POST",
        &format!("/assistant/sessions/{session_db_id}/compact"),
        &fx.cookie,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "compact: {compacted}");

    let messages = compacted["messages"].as_array().expect("messages array");
    assert!(
        !messages.is_empty(),
        "compact response has no messages: {compacted:#}"
    );
    let first = &messages[0];
    assert_eq!(first["role"], "user", "first message: {compacted:#}");
    assert!(
        first["content"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("SUMMARY_MARKER")),
        "first message should seed the fork with the compaction summary: {compacted:#}"
    );
    assert!(
        messages.iter().any(|m| m["role"] == "assistant"
            && m["content"]["text"]
                .as_str()
                .is_some_and(|t| t.contains("REPLY_MARKER"))),
        "the successor's own first turn should have replied: {compacted:#}"
    );
    assert!(
        !messages.iter().any(|m| m["role"] == "error"),
        "compact turn ended in an error: {compacted:#}"
    );

    // The DB row now points at a *different* engine session (the successor) —
    // same DB id/title, ADR-0110's copy-on-write fork.
    let updated = assistant_session::Entity::find_by_id(session_db_id)
        .one(&fx.db)
        .await
        .expect("query assistant_sessions")
        .expect("session row still exists");
    let successor_session_id = updated
        .engine_session_id
        .clone()
        .expect("engine_session_id set");
    assert_ne!(
        successor_session_id, source_session_id.0,
        "engine_session_id must repoint to the forked successor"
    );

    // The source's own log survives untouched (ADR-0101: "the original stays
    // idle, intact, independently resumable"), just no longer reachable from
    // this DB row.
    let source_records = events_for(&fx, &source_session_id.0).await;
    assert!(
        !source_records.is_empty(),
        "the source session's assistant_events must not be deleted by compact"
    );

    // Projection follows the successor: a plain follow-up `GET` (not the
    // compact response itself) must resolve through the new
    // `engine_session_id` and see the same seeded transcript. `DbSink`
    // appends asynchronously behind its own writer task (`persistence.rs`'s
    // module doc) — give it a moment to catch up.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (status, read_back) = send(
        &fx.app,
        "GET",
        &format!("/assistant/sessions/{session_db_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "read back: {read_back}");
    let read_messages = read_back["messages"].as_array().expect("messages array");
    assert!(
        read_messages
            .first()
            .and_then(|m| m["content"]["text"].as_str())
            .is_some_and(|t| t.contains("SUMMARY_MARKER")),
        "a fresh read must project the successor's own history: {read_back:#}"
    );

    scripted_cleanup(&fx, session_db_id).await;
    // `scripted_cleanup` only knows the *current* `engine_session_id`
    // (the successor) — the retired source's log needs its own cleanup.
    let _ = site::ai::persistence::delete_session_events(&fx.db, &source_session_id).await;
    let _ = llm_provider::Entity::delete_by_id(provider_id)
        .exec(&fx.db)
        .await;
}
