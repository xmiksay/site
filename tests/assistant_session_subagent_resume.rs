//! #43: resume of a session with a **live sub-agent** must replay the child
//! correctly from the DB log alone — no reliance on the in-process
//! `SESSION_PARENTS` cache (`src/ai/engine/session_tree.rs`) having already
//! seen that child before.
//!
//! Unlike `tests/assistant_session_subagent_pagewriter.rs` (which drives the
//! whole spawn live, so by the time it approves the child's pending call this
//! process's `SESSION_PARENTS` cache is already warm from that same live
//! spawn), this test never runs a live turn for the conversation it resumes:
//! the root + `page-writer` child's prior log is inserted straight into
//! `assistant_events`, mirroring `entanglement_runtime`'s own persisted shape
//! (`root_session_id` on every row, the *root's* id, matching
//! `crate::ai::persistence::DbSink`'s convention). The child session id this
//! test uses has never been mentioned to this process before that read — so
//! `SESSION_PARENTS` genuinely starts cold for it, and the only way the
//! subsequent approve (routed to the child, resolving the child's owning user
//! via `user_id_from_session_awaiting`) can succeed is entanglement 0.3.0's
//! resume-cascade (ADR-0112) re-materializing the child and re-announcing its
//! `SessionStarted`, which `engine.rs`'s generic watcher then folds exactly
//! like a live spawn's.

mod common;
#[path = "common/scripted.rs"]
mod scripted;

use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use chrono::Utc;
use common::{send, test_db_url};
use entanglement_core::{
    ContentPart, InMsg, Llm, LlmRequest, LlmResponse, LlmStream, OutEvent, SessionId,
    stream_from_response,
};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use scripted::{scripted_cleanup, setup_scripted};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::json;
use site::ai::engine::SiteEngine;
use site::entity::assistant_session;

/// The `page-writer` child's continuation after its `page_edit` call is
/// approved and executes — the only round this scripted `Llm` ever needs to
/// answer, since the call itself (and everything before it) is pre-baked
/// straight into `assistant_events`, never driven live.
#[derive(Default)]
struct ResumeScriptedLlm;

#[async_trait]
impl Llm for ResumeScriptedLlm {
    async fn stream(&mut self, _req: LlmRequest<'_>) -> anyhow::Result<LlmStream> {
        Ok(stream_from_response(LlmResponse {
            text: "Page created.".into(),
            tool_calls: vec![],
        }))
    }
}

/// Insert one `LogRecord` into `assistant_events` under `root`'s
/// `root_session_id` — same shape `DbSink`/`entanglement_runtime`'s
/// persistence tap produce, but callable directly so a whole root+child log
/// can be fabricated without ever running a live turn (see the module doc).
async fn insert(db: &sea_orm::DatabaseConnection, root: &SessionId, record: LogRecord) {
    let value = serde_json::to_value(&record).expect("serialize LogRecord");
    site::entity::assistant_event::ActiveModel {
        root_session_id: Set(root.0.clone()),
        payload: Set(value),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert assistant_event row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_replays_a_live_sub_agent_from_the_db_log_alone() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let llm_factory: entanglement_core::LlmFactory =
        std::sync::Arc::new(|| Box::new(ResumeScriptedLlm));
    let fx = setup_scripted(&db_url, "resume", llm_factory).await;

    let root = SiteEngine::session_id_for_user(fx.user_id);
    let child = SessionId::new_uuid();
    let path = format!("test/subagent-resume-{}", uuid::Uuid::new_v4());

    // The `assistant_sessions` row — deliberately *not* `engine.mark_live`d
    // (unlike `scripted::scripted_session`): this session must be genuinely
    // cold in this process, so the first touch below has to go through
    // `SiteEngine::ensure_live`'s `resume_session` path for real.
    let now = Utc::now().fixed_offset();
    let saved = assistant_session::ActiveModel {
        user_id: Set(fx.user_id),
        title: Set("New chat".into()),
        provider: Set("test".into()),
        model: Set("scripted".into()),
        model_id: Set(None),
        enabled_mcp_server_ids: Set(json!([])),
        engine_session_id: Set(Some(root.0.clone())),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("insert assistant_session row");
    let db_session_id = saved.id;

    // The root's own settled turn: prompt → spawn `page-writer` → the spawn's
    // own immediate reply (naming the child, #421/ADR-0113's synthesized
    // prompt is a *runtime*-side behavior for a live spawn — here the
    // equivalent user-role framing message for the child is inserted
    // directly, below) → done.
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
            root.clone(),
            LogPayload::In(InMsg::prompt(root.clone(), "draft a page")),
        ),
    )
    .await;
    insert(
        &fx.db,
        &root,
        LogRecord::new(
            root.clone(),
            LogPayload::Out(OutEvent::ToolCall {
                session: root.clone(),
                seq: 1,
                request_id: "spawn-1".into(),
                tool: "agent_spawn".into(),
                input: format!(
                    r#"{{"agent":"page-writer","prompt":"create a page at path {path} with markdown content 'hello from a resumed sub-agent'"}}"#
                ),
            }),
        ),
    )
    .await;
    insert(
        &fx.db,
        &root,
        LogRecord::new(
            root.clone(),
            LogPayload::Out(OutEvent::ToolOutput {
                session: root.clone(),
                seq: 2,
                request_id: "spawn-1".into(),
                tool: "agent_spawn".into(),
                output: format!(
                    "Sub-agent launched under the `page-writer` profile. agent_id: {}. \
                     Call agent_poll with this agent_id to await its answer.",
                    child.0
                ),
                content: Vec::<ContentPart>::new(),
            }),
        ),
    )
    .await;
    insert(
        &fx.db,
        &root,
        LogRecord::new(
            root.clone(),
            LogPayload::Out(OutEvent::Done {
                session: root.clone(),
                seq: 3,
            }),
        ),
    )
    .await;

    // The child's own log, filed under the *same* `root_session_id` (the
    // `DbSink`/persistence-tap convention `crate::ai::projection::project`
    // and the engine's own cascaded resume both depend on) — started, framed
    // by its spawning prompt, and parked on a pending `page_edit` call (a
    // fresh test user has no `tool_permissions` rows, so `page_edit` defaults
    // to `Ask` — this is genuinely a **live**, unresolved sub-agent as of
    // where the log stops).
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
    insert(
        &fx.db,
        &root,
        LogRecord::new(
            child.clone(),
            LogPayload::In(InMsg::prompt(
                child.clone(),
                format!(
                    "create a page at path {path} with markdown content 'hello from a resumed sub-agent'"
                ),
            )),
        ),
    )
    .await;
    insert(
        &fx.db,
        &root,
        LogRecord::new(
            child.clone(),
            LogPayload::Out(OutEvent::ToolCall {
                session: child.clone(),
                seq: 1,
                request_id: "edit-1".into(),
                tool: "page_edit".into(),
                input: format!(
                    r#"{{"path":"{path}","markdown":"hello from a resumed sub-agent"}}"#
                ),
            }),
        ),
    )
    .await;
    insert(
        &fx.db,
        &root,
        LogRecord::new(
            child.clone(),
            LogPayload::Out(OutEvent::ToolRequest {
                session: child.clone(),
                seq: 2,
                request_id: "edit-1".into(),
                tool: "page_edit".into(),
                input: format!(
                    r#"{{"path":"{path}","markdown":"hello from a resumed sub-agent"}}"#
                ),
            }),
        ),
    )
    .await;

    // First touch: `GET` resumes the root, which — thanks to ADR-0112 —
    // cascades the still-parked `page-writer` child back to life too, purely
    // from the rows just inserted above. Assert the projection already nests
    // the child's pending call under the spawning message, sourced entirely
    // from the replayed log.
    let (status, resp) = send(
        &fx.app,
        "GET",
        &format!("/assistant/sessions/{db_session_id}"),
        &fx.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "read session: {resp}");
    let messages = resp["messages"].as_array().cloned().unwrap_or_default();
    let call_id = messages
        .iter()
        .find_map(|m| {
            let agents = m["content"]["sub_agents"].as_array()?;
            let agent = agents
                .iter()
                .find(|a| a["profile"] == json!("page-writer"))?;
            let child_messages = agent["messages"].as_array()?;
            let pending = child_messages.iter().find(|cm| {
                cm["content"]["requires_approval"] == json!(true)
                    && cm["content"]["tool_calls"]
                        .as_array()
                        .is_some_and(|c| c.iter().any(|tc| tc["name"] == json!("page_edit")))
            })?;
            pending["content"]["tool_calls"][0]["id"]
                .as_str()
                .map(String::from)
        })
        .unwrap_or_else(|| {
            panic!("resumed page-writer child never showed a pending page_edit call: {resp:#}")
        });
    assert_eq!(
        call_id, "edit-1",
        "the id round-tripped through replay+projection: {resp:#}"
    );

    // The `GET` above only *starts* the resume (`ensure_live` sends
    // `InMsg::Resume` and returns); the resumed child's own session task then
    // asynchronously re-offers its parked `page_edit` call as a fresh
    // `ToolExec`, which is what actually drives the permission resolution
    // this test exists to exercise (and registers the `PendingDecisions`
    // waiter the approve below needs). Give that a moment to land before
    // approving, or the approve could race a `PendingDecisions` entry that
    // hasn't been (re-)registered yet.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Approve it — `PendingDecisions` is keyed by `(session, request_id)`, so
    // this only resolves if `approve`'s `session_for_call` routing correctly
    // addresses the *child*, and the child's own `ToolExec`'s permission
    // resolution correctly resolves its owning user — both purely off the
    // just-resumed cascade, since this process has never linked `child` to
    // `root` any other way.
    let (status, approved) = send(
        &fx.app,
        "POST",
        &format!("/assistant/sessions/{db_session_id}/messages/0/approve"),
        &fx.cookie,
        Some(json!({ "decisions": [{ "tool_call_id": call_id, "approve": true }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "approve: {approved}");

    let page = site::entity::page::Entity::find()
        .filter(site::entity::page::Column::Path.eq(path.clone()))
        .one(&fx.db)
        .await
        .expect("query pages");
    assert!(
        page.is_some(),
        "resumed page-writer's page_edit never created {path} after approval: {approved:#}"
    );
    if let Some(p) = page {
        let _ = site::entity::page::Entity::delete_by_id(p.id)
            .exec(&fx.db)
            .await;
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    scripted_cleanup(&fx, db_session_id).await;
}
