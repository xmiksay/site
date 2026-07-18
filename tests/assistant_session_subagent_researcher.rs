//! #17 sub-agent flow: a `researcher` sub-agent, DB-gated only (not
//! Ollama-gated) — the engine is driven with a small scripted `Llm`
//! (`stream_from_response`, the same pattern `entanglement-core`'s own test
//! suite uses) instead of a live model, so a deterministic tool-call decision
//! doesn't depend on a local Ollama instance's availability or
//! non-determinism.
//!
//! The `page-writer` sub-agent flow lives in the sibling
//! `tests/assistant_session_subagent_pagewriter.rs`; the base acceptance flow
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
use serde_json::{Value, json};

/// A scripted `Llm` for the `spawns_a_researcher_sub_agent_and_nests_its_
/// progress` test: the root's first round emits `agent_spawn(researcher,
/// "what is 2+2")`, its second round (after the spawn's own immediate tool
/// result) just finishes with plain text. The `researcher` child (identified
/// by its system prompt carrying `engine.rs`'s `RESEARCHER_PROMPT_SUFFIX`
/// marker) answers directly with no tool call. Each session gets its own
/// instance (`LlmFactory` is called once per session), so `calls` never needs
/// to distinguish root from child — the system-prompt check does that.
#[derive(Default)]
struct ResearcherScriptedLlm {
    calls: u32,
}

#[async_trait]
impl Llm for ResearcherScriptedLlm {
    async fn stream(&mut self, req: LlmRequest<'_>) -> anyhow::Result<LlmStream> {
        self.calls += 1;
        let resp = if req.system.contains("`researcher` sub-agent") {
            LlmResponse {
                text: "2 + 2 = 4.".into(),
                tool_calls: vec![],
            }
        } else if self.calls == 1 {
            LlmResponse {
                text: String::new(),
                tool_calls: vec![ToolCall::new(
                    "spawn-1",
                    "agent_spawn",
                    r#"{"agent":"researcher","prompt":"what is 2+2"}"#,
                )],
            }
        } else {
            LlmResponse {
                text: "Researching, thanks!".into(),
                tool_calls: vec![],
            }
        };
        Ok(stream_from_response(resp))
    }
}

/// #17: the model spawns a `researcher` sub-agent via `agent_spawn`. Unlike a
/// host tool, `agent`/`agent_spawn`/`agent_poll` bypass permission entirely
/// (`entanglement_runtime::tool_runner`'s `Intercept::Spawn` route) — no
/// approval step, the tool_result comes back in the same turn. This exercises
/// the whole chain end to end: `engine.rs`'s `researcher` profile + its
/// `profile_tool_specs` roster actually being advertised to the root's
/// `build` profile, the child session's tool calls resolving permission
/// against the *same* user (the `SESSION_PARENTS`/`root_session_of` fix — a
/// bare-uuid child session that failed to resolve would fail closed and the
/// child's own turn would error out instead of answering), and
/// `projection::project` nesting the child's turn under the spawning message.
/// DB-gated only — see `ScriptedFixture`'s doc for why no live model is used.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawns_a_researcher_sub_agent_and_nests_its_progress() {
    let Some(db_url) = test_db_url().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let llm_factory: entanglement_core::LlmFactory =
        std::sync::Arc::new(|| Box::new(ResearcherScriptedLlm::default()));
    let fx = setup_scripted(&db_url, "researcher", llm_factory).await;
    let (db_session_id, _session_id) = scripted_session(&fx).await;

    let (status, resp) = send(
        &fx.app,
        "POST",
        &format!("/assistant/sessions/{db_session_id}/messages"),
        &fx.cookie,
        Some(json!({ "text": "research 2+2" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "send_message: {resp}");
    let messages = resp["messages"].as_array().cloned().unwrap_or_default();
    let spawn_msg = messages
        .iter()
        .find(|m| {
            m["role"] == "assistant"
                && m["content"]["tool_calls"].as_array().is_some_and(|c| {
                    c.iter().any(|tc| {
                        tc["name"] == json!("agent_spawn") || tc["name"] == json!("agent")
                    })
                })
        })
        .cloned()
        .unwrap_or_else(|| panic!("no assistant message with an agent_spawn/agent call: {resp:#}"));
    assert_ne!(
        spawn_msg["content"]["requires_approval"],
        json!(true),
        "agent_spawn bypasses permission entirely, it must never require approval: {resp:#}"
    );

    // The child runs detached (ADR-0026) — poll until its nested turn shows
    // up fully settled instead of racing the background task.
    let mut sub_agent_messages: Option<Value> = None;
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
        if let Some(m) = messages.iter().find(|m| {
            m["content"]["sub_agents"]
                .as_array()
                .is_some_and(|s| !s.is_empty())
        }) {
            let agents = m["content"]["sub_agents"].as_array().unwrap();
            assert_eq!(agents[0]["profile"], json!("researcher"), "{resp:#}");
            let child_messages = agents[0]["messages"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if child_messages
                .iter()
                .any(|cm| cm["role"] == "assistant" && !cm["content"]["text"].is_null())
            {
                sub_agent_messages = Some(json!(child_messages));
                break;
            }
        }
        detail = resp;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let sub_agent_messages = sub_agent_messages.unwrap_or_else(|| {
        panic!("researcher sub-agent never produced a nested, settled turn: {detail:#}")
    });
    // entanglement-runtime 0.3 (#421) now synthesizes the child's own
    // spawn-initiating `InMsg::Prompt` into its persisted log (previously only
    // the assistant's eventual reply was recorded), so replay — and this
    // projection — surfaces the researcher's framing task as a leading user
    // message alongside its answer.
    assert_eq!(
        sub_agent_messages,
        json!([
            { "role": "user", "content": { "text": "what is 2+2" } },
            { "role": "assistant", "content": { "text": "2 + 2 = 4.", "tool_calls": [] } },
        ])
    );

    scripted_cleanup(&fx, db_session_id).await;
}
