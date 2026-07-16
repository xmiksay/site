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
//! The #17 sub-agent tests below are DB-gated only, not Ollama-gated: they
//! drive the engine with a small scripted `Llm` (`ScriptedLlm`/`stream_from_
//! response`, the same pattern `entanglement-core`'s own test suite uses)
//! instead of a live model, so a deterministic tool-call decision doesn't
//! depend on a local Ollama instance's availability or non-determinism.

use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use entanglement_core::{Llm, LlmRequest, LlmResponse, LlmStream, ToolCall, stream_from_response};
use http_body_util::BodyExt;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde_json::{Value, json};
use tower::ServiceExt;

use site::ai::AiConfig;
use site::ai::engine::SiteEngine;
use site::auth::SESSION_COOKIE;
use site::config::Config;
use site::entity::{assistant_session, llm_model, llm_provider, token, user};
use site::routes::ws::WsHub;
use site::state::{self, AppState};

const OLLAMA_BASE: &str = "http://localhost:11434";
const MODEL: &str = "qwen3.5:9b";

async fn test_db_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

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
struct ScriptedFixture {
    app: Router,
    db: DatabaseConnection,
    engine: std::sync::Arc<SiteEngine>,
    cookie: String,
    user_id: i32,
}

async fn setup_scripted(
    db_url: &str,
    tag: &str,
    llm_factory: entanglement_core::LlmFactory,
) -> ScriptedFixture {
    let db = sea_orm::Database::connect(db_url)
        .await
        .expect("connect to DATABASE_URL");

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
    let engine = SiteEngine::spawn(db.clone(), ai_config, None, Some(llm_factory))
        .await
        .expect("spawn scripted assistant engine");
    let ws_hub = std::sync::Arc::new(WsHub::new());
    site::ai::ws_bridge::spawn(engine.clone(), ws_hub.clone(), db.clone());
    let state = AppState {
        db: db.clone(),
        tmpl: site::templates::Templates::new(std::sync::Arc::new(site::design::DesignStore::new(
            None,
        ))),
        design: std::sync::Arc::new(site::design::DesignStore::new(None)),
        agent_engine: engine.clone(),
        ws_hub,
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
async fn scripted_session(fx: &ScriptedFixture) -> (i32, entanglement_core::SessionId) {
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

/// A scripted `Llm` for the `approves_a_page_writer_sub_agents_own_pending_
/// tool_call` test: the root spawns `page-writer` with a task naming `path`;
/// the `page-writer` child (system prompt carries `PAGE_WRITER_PROMPT_SUFFIX`)
/// calls `edit_page` on its first round, then finishes on its second (after
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
                        "edit_page",
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

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let body = match body {
        Some(v) => Body::from(serde_json::to_vec(&v).unwrap()),
        None => Body::empty(),
    };
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("cookie", cookie)
        .body(body)
        .unwrap();
    let resp = app.clone().oneshot(req).await.expect("request failed");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body was not JSON: {e} (body: {})",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, value)
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
    let tool_call_id = tool_msg["content"]["tool_calls"][0]["id"]
        .as_str()
        .expect("tool_call id")
        .to_string();

    // 3. Approve it.
    let (status, approved) = send(
        &fx.app,
        "POST",
        &format!("/assistant/sessions/{session_id}/messages/0/approve"),
        &fx.cookie,
        Some(json!({ "decisions": [{ "tool_call_id": tool_call_id, "approve": true }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "approve: {approved}");

    let messages = approved["messages"].as_array().expect("messages array");
    assert!(
        messages.iter().any(|m| m["role"] == "tool_result"),
        "no tool_result message after approval: {approved:#}"
    );
    assert!(
        !messages
            .iter()
            .any(|m| m["content"]["requires_approval"] == json!(true)),
        "turn still shows a pending approval after approving the only call: {approved:#}"
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
    assert_eq!(
        sub_agent_messages,
        json!([{ "role": "assistant", "content": { "text": "2 + 2 = 4.", "tool_calls": [] } }])
    );

    scripted_cleanup(&fx, db_session_id).await;
}

/// #17: a `page-writer` sub-agent's own `edit_page` call needs approval (a
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

    // Poll until the page-writer child's own edit_page call shows up,
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
                            .is_some_and(|c| c.iter().any(|tc| tc["name"] == json!("edit_page")))
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
        panic!("page-writer never reached a pending edit_page call: {detail:#}")
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
        "page-writer's edit_page never created {path} after approval: {approved:#}"
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

/// Same as `cleanup`, minus the `llm_model`/`llm_provider` rows a
/// `ScriptedFixture` never creates.
async fn scripted_cleanup(fx: &ScriptedFixture, session_id: i32) {
    if let Ok(Some(session)) = assistant_session::Entity::find_by_id(session_id)
        .one(&fx.db)
        .await
        && let Some(engine_session_id) = session.engine_session_id
    {
        let sid = entanglement_core::SessionId::new(engine_session_id);
        let _ = site::ai::persistence::delete_session_events(&fx.db, &sid).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = site::ai::persistence::delete_session_events(&fx.db, &sid).await;
    }
    let _ = user::Entity::delete_by_id(fx.user_id).exec(&fx.db).await;
}
