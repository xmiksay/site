//! Regression test for a second stuck-approval bug, distinct from
//! `tests/assistant_session_subagent_approval_race.rs`'s session-routing
//! race: `send_and_collect` (`src/ai/handlers/sessions/turn/collect.rs`) used
//! to treat a decision as "settled" only once its *whole session* emitted
//! `Done`/`Error`/`WaitingApproval` — but a turn with several tool calls
//! pending approval at once only resumes (and only has anything new to say)
//! once *every* one of them is resolved (`entanglement_core::session::
//! TurnState::is_drained`). So approving anything other than the *last*
//! pending call in a batch used to block that HTTP request until the rest of
//! the batch was also decided — from the client's point of view, "I approved
//! it and it came back unconfirmed" for every call except the final one.
//!
//! The engine actually acknowledges each call independently: both the
//! `Approve` and `Reject` paths in `entanglement-runtime`'s
//! `tool_runner::await_decision` end in `seam::reply(...)`, which emits an
//! `OutEvent::ToolOutput { request_id, .. }` the moment *that* call resolves,
//! unconditionally — before the runtime ever checks whether the rest of the
//! batch has drained. `send_and_collect` now settles an `Approve`/`Reject`
//! obligation on its own matching `ToolOutput` instead of waiting on the
//! whole session, so approving calls one at a time returns promptly for each
//! one, and the last approval still drives the turn to completion.
//!
//! DB-gated only (no live model needed) — see `tests/common/scripted.rs`'s
//! `ScriptedFixture` doc for why a scripted `Llm` is used instead.

mod common;
#[path = "common/scripted.rs"]
mod scripted;

use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use common::{send, test_db_url};
use entanglement_core::{Llm, LlmRequest, LlmResponse, LlmStream, ToolCall, stream_from_response};
use scripted::{scripted_cleanup, scripted_session, setup_scripted};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use site::entity::tag;

/// First round: two parallel `create_tag` calls (both permission-gated,
/// defaulting to `Ask` for a fresh user with no `tool_permissions` rows) in
/// one response — the model batching two independent writes into a single
/// turn. Second round (after both are decided): a closing text reply, no
/// further tool calls.
struct TwoTagsLlm {
    calls: u32,
    tag_a: String,
    tag_b: String,
}

#[async_trait]
impl Llm for TwoTagsLlm {
    async fn stream(&mut self, _req: LlmRequest<'_>) -> anyhow::Result<LlmStream> {
        self.calls += 1;
        let resp = if self.calls == 1 {
            LlmResponse {
                text: String::new(),
                tool_calls: vec![
                    ToolCall::new(
                        "tag-a",
                        "create_tag",
                        format!(r#"{{"name":"{}"}}"#, self.tag_a),
                    ),
                    ToolCall::new(
                        "tag-b",
                        "create_tag",
                        format!(r#"{{"name":"{}"}}"#, self.tag_b),
                    ),
                ],
            }
        } else {
            LlmResponse {
                text: "Both tags created.".into(),
                tool_calls: vec![],
            }
        };
        Ok(stream_from_response(resp))
    }
}

/// Every `tool_calls[].id` genuinely still awaiting a decision — mirrors the
/// client's `needsDecision` (`client/src/composables/useAssistantContent.ts`):
/// a call's *own* `requires_approval` (`src/ai/projection/mod.rs`'s
/// `OpenTurn::flush_into`), not merely "some call in this message needs
/// approval". Deliberately narrower than the message-level flag: two
/// parallel calls can have their own `ToolRequest` land at very different
/// times (each is gated by its own concurrent policy check), so a coarser
/// "any undecided id in a requires_approval message" check can believe a call
/// is ready to approve before the engine has actually registered its
/// approval-wait — sending an `Approve` for it then is a silent, permanent
/// no-op (`entanglement_core`'s `PendingDecisions::resolve` on an unknown
/// key), which is exactly the false-ready race this test must not itself
/// reproduce while trying to test something else.
fn pending_call_ids(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| m["content"]["tool_calls"].as_array())
        .flatten()
        .filter(|tc| tc["requires_approval"] == json!(true) && tc["resolved"] != json!(true))
        .filter_map(|tc| tc["id"].as_str().map(String::from))
        .collect()
}

async fn tag_exists(db: &sea_orm::DatabaseConnection, name: &str) -> bool {
    tag::Entity::find()
        .filter(tag::Column::Name.eq(name))
        .one(db)
        .await
        .expect("query tags")
        .is_some()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approving_calls_one_at_a_time_settles_each_promptly_and_the_last_continues_the_turn() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let tag_a = format!("parallel-approval-a-{}", uuid::Uuid::new_v4());
    let tag_b = format!("parallel-approval-b-{}", uuid::Uuid::new_v4());
    let llm_factory: entanglement_core::LlmFactory = {
        let (tag_a, tag_b) = (tag_a.clone(), tag_b.clone());
        std::sync::Arc::new(move || {
            Box::new(TwoTagsLlm {
                calls: 0,
                tag_a: tag_a.clone(),
                tag_b: tag_b.clone(),
            })
        })
    };
    let fx = setup_scripted(&db_url, "parallel-approval", llm_factory).await;
    let (db_session_id, _session_id) = scripted_session(&fx).await;

    let (status, resp) = send(
        &fx.app,
        "POST",
        &format!("/assistant/sessions/{db_session_id}/messages"),
        &fx.cookie,
        Some(json!({ "text": "create two tags" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "send_message: {resp}");

    // `send_and_collect` (for the *prompt* itself) settles as soon as the
    // session's `Status` first flips to `WaitingApproval` — which can arrive
    // before *every* parallel call's own `ToolRequest` has been individually
    // registered (each call's approval-wait is set up by its own concurrent
    // executor task). So the synchronous `send_message` response can be an
    // incomplete snapshot of the batch; poll `GET` (which reflects whatever
    // `DbSink` has durably persisted) until both calls are visible, the same
    // way `tests/assistant_session_subagent_pagewriter.rs` polls for its
    // child's call — approving a call before its own `ToolRequest` has landed
    // races the engine's own registration, not the bug this test targets.
    let mut call_ids = Vec::new();
    let mut detail = resp;
    for _ in 0..20 {
        let (status, resp) = send(
            &fx.app,
            "GET",
            &format!("/assistant/sessions/{db_session_id}"),
            &fx.cookie,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "read session: {resp}");
        let ids = pending_call_ids(resp["messages"].as_array().expect("messages array"));
        detail = resp;
        if ids.len() == 2 {
            call_ids = ids;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        call_ids.len(),
        2,
        "never saw both parallel pending calls: {detail:#}"
    );

    // Approve the first call alone. Before the fix, `send_and_collect` waited
    // for the *whole session* to settle (`Done`/`Error`/`WaitingApproval`),
    // which never happens until the second call is also decided — this
    // request would hang for the full 180s `TURN_TIMEOUT`.
    let start = std::time::Instant::now();
    let (status, approved_a) = send(
        &fx.app,
        "POST",
        &format!("/assistant/sessions/{db_session_id}/messages/0/approve"),
        &fx.cookie,
        Some(json!({ "decisions": [{ "tool_call_id": call_ids[0], "approve": true }] })),
    )
    .await;
    let elapsed = start.elapsed();
    assert_eq!(status, StatusCode::OK, "approve call A: {approved_a}");
    assert!(
        elapsed < Duration::from_secs(10),
        "approving one of two pending calls took {elapsed:?} — looks like it \
         waited for the whole batch to drain instead of settling on its own \
         ToolOutput: {approved_a:#}"
    );
    assert!(
        tag_exists(&fx.db, &tag_a).await,
        "approving call A should have run create_tag for it: {approved_a:#}"
    );
    assert!(
        !tag_exists(&fx.db, &tag_b).await,
        "call B hasn't been decided yet, its create_tag must not have run"
    );

    // Approve the second (last) call — this is what actually drains the
    // batch and lets the turn continue to its closing text reply.
    let start = std::time::Instant::now();
    let (status, approved_b) = send(
        &fx.app,
        "POST",
        &format!("/assistant/sessions/{db_session_id}/messages/0/approve"),
        &fx.cookie,
        Some(json!({ "decisions": [{ "tool_call_id": call_ids[1], "approve": true }] })),
    )
    .await;
    let elapsed = start.elapsed();
    assert_eq!(status, StatusCode::OK, "approve call B: {approved_b}");
    assert!(
        elapsed < Duration::from_secs(10),
        "approving the last pending call took {elapsed:?}: {approved_b:#}"
    );
    assert!(
        tag_exists(&fx.db, &tag_b).await,
        "approving call B should have run create_tag for it: {approved_b:#}"
    );

    let messages = approved_b["messages"].as_array().expect("messages array");
    assert!(
        messages.iter().any(|m| m["role"] == json!("assistant")
            && m["content"]["text"] == json!("Both tags created.")),
        "the turn should have continued to its closing text reply once every \
         pending call was decided: {approved_b:#}"
    );

    for name in [&tag_a, &tag_b] {
        if let Some(row) = tag::Entity::find()
            .filter(tag::Column::Name.eq(name.as_str()))
            .one(&fx.db)
            .await
            .expect("query tags")
        {
            let _ = tag::Entity::delete_by_id(row.id).exec(&fx.db).await;
        }
    }
    scripted_cleanup(&fx, db_session_id).await;
}
