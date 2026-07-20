//! #17 sub-agent flow: a `page-writer` sub-agent, DB-gated only (not
//! Ollama-gated) — the engine is driven with a small scripted `Llm`
//! (`stream_from_response`, the same pattern `entanglement-core`'s own test
//! suite uses) instead of a live model, so a deterministic tool-call decision
//! doesn't depend on a local Ollama instance's availability or
//! non-determinism.
//!
//! The `researcher` sub-agent flow lives in the sibling
//! `tests/assistant_session_subagent_researcher.rs`; the base acceptance flow
//! (real Ollama) lives in `tests/assistant_session_base.rs`; shared setup
//! helpers live in `tests/common/mod.rs`.

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
use serde_json::json;

/// A scripted `Llm` for the `approves_a_page_writer_sub_agents_own_pending_
/// tool_call` test: the root spawns `page-writer` with a task naming `path`;
/// the `page-writer` child (system prompt carries `PAGE_WRITER_PROMPT_SUFFIX`)
/// calls `page_edit` on its first round, then finishes on its second (after
/// the approved call's tool result comes back).
struct PageWriterScriptedLlm {
    calls: u32,
    path: String,
}

#[async_trait]
impl Llm for PageWriterScriptedLlm {
    async fn stream(&mut self, req: LlmRequest<'_>) -> anyhow::Result<LlmStream> {
        self.calls += 1;
        let resp = if req.system.contains("`page-writer` sub-agent") {
            if self.calls == 1 {
                LlmResponse {
                    text: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "edit-1",
                        "page_edit",
                        format!(
                            r#"{{"path":"{}","markdown":"hello from a sub-agent"}}"#,
                            self.path
                        ),
                    )],
                }
            } else {
                LlmResponse {
                    text: "Page created.".into(),
                    tool_calls: vec![],
                }
            }
        } else if self.calls == 1 {
            LlmResponse {
                text: String::new(),
                tool_calls: vec![ToolCall::new(
                    "spawn-1",
                    "agent_spawn",
                    format!(
                        r#"{{"agent":"page-writer","prompt":"create a page at path {} with markdown content 'hello from a sub-agent'"}}"#,
                        self.path
                    ),
                )],
            }
        } else {
            LlmResponse {
                text: "Delegated to page-writer.".into(),
                tool_calls: vec![],
            }
        };
        Ok(stream_from_response(resp))
    }
}

/// #17: a `page-writer` sub-agent's own `page_edit` call needs approval (a
/// fresh test user has no `tool_permissions` rows, so it defaults to `Ask`)
/// — unlike `agent_spawn` itself, this *is* a permission-gated host tool, and
/// it is gated inside the *child* session. This is the sharpest edge of the
/// whole feature: `PendingDecisions` keys a waiter by `(session, request_id)`,
/// so `approve`'s `session_for_call` routing (and `send_and_collect`'s
/// matching settle-on-`targets` fix) must address the exact child session —
/// addressing the long-since-`Done` root (the pre-#17 behavior) would
/// silently resolve nothing and hang. DB-gated only — see `ScriptedFixture`'s
/// doc for why no live model is used.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approves_a_page_writer_sub_agents_own_pending_tool_call() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let path = format!("test/subagent-{}", uuid::Uuid::new_v4());
    let llm_factory: entanglement_core::LlmFactory = {
        let path = path.clone();
        std::sync::Arc::new(move || {
            Box::new(PageWriterScriptedLlm {
                calls: 0,
                path: path.clone(),
            })
        })
    };
    let fx = setup_scripted(&db_url, "page-writer", llm_factory).await;
    let (db_session_id, _session_id) = scripted_session(&fx).await;

    let (status, resp) = send(
        &fx.app,
        "POST",
        &format!("/assistant/sessions/{db_session_id}/messages"),
        &fx.cookie,
        Some(json!({ "text": "draft a page" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "send_message: {resp}");
    let messages = resp["messages"].as_array().cloned().unwrap_or_default();
    assert!(
        messages.iter().any(|m| {
            m["role"] == "assistant"
                && m["content"]["tool_calls"].as_array().is_some_and(|c| {
                    c.iter().any(|tc| {
                        tc["name"] == json!("agent_spawn") || tc["name"] == json!("agent")
                    })
                })
        }),
        "no agent_spawn/agent call: {resp:#}"
    );

    // Poll until the page-writer child's own page_edit call shows up,
    // pending approval, nested under the spawning turn.
    let mut call_id: Option<String> = None;
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
        let messages = resp["messages"].as_array().cloned().unwrap_or_default();
        for m in &messages {
            let Some(agents) = m["content"]["sub_agents"].as_array() else {
                continue;
            };
            for agent in agents {
                let child_messages = agent["messages"].as_array().cloned().unwrap_or_default();
                // `ToolCall` (display) and `ToolRequest` (the approval pause)
                // are two separate, separately-persisted events — a poll can
                // land between them and see `tool_calls` populated but
                // `requires_approval` still false. Only treat the call as
                // ready once both have landed; otherwise keep polling instead
                // of asserting on a transiently-incomplete read.
                if let Some(cm) = child_messages.iter().find(|cm| {
                    cm["content"]["requires_approval"] == json!(true)
                        && cm["content"]["tool_calls"]
                            .as_array()
                            .is_some_and(|c| c.iter().any(|tc| tc["name"] == json!("page_edit")))
                }) {
                    call_id = cm["content"]["tool_calls"][0]["id"]
                        .as_str()
                        .map(String::from);
                }
            }
        }
        detail = resp;
        if call_id.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let call_id = call_id.unwrap_or_else(|| {
        panic!("page-writer never reached a pending page_edit call: {detail:#}")
    });

    // Approve it — this is the routing fix: the waiter for this call lives
    // under the child session, not the root `session_id` in the URL.
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
        "page-writer's page_edit never created {path} after approval: {approved:#}"
    );
    assert!(
        page.as_ref().is_some_and(|p| p.private),
        "a sub-agent-created page must default to private"
    );
    if let Some(p) = page {
        let _ = site::entity::page::Entity::delete_by_id(p.id)
            .exec(&fx.db)
            .await;
    }

    scripted_cleanup(&fx, db_session_id).await;
}
