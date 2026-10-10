//! `POST /sessions/{id}/compact` — manual context compaction (issue #40).
//!
//! `entanglement_core`'s `"compact"` oneshot (`InMsg::Oneshot { op: "compact",
//! .. }`) is **copy-on-write** (ADR-0101): it summarizes the source session's
//! transcript and reports it via `OutEvent::Compacted`, never mutating the
//! source. The head (this handler) is responsible for forking the summary
//! into a fresh successor session (`InMsg::Spawn` with `predecessor =
//! Some(source)`, ADR-0110) and retiring the source (`InMsg::CloseSession`) —
//! exactly the flow `entanglement_runtime`'s own reference TUI implements
//! (`tui/app/compact.rs` in that crate). This mirrors it for the DB-backed
//! session model here: the `assistant_sessions` row keeps its id/title, only
//! its `engine_session_id` repoints to the successor, so `GET
//! /sessions/{id}` (and every other handler keyed by the DB id) transparently
//! follows the fork.
//!
//! The source's own `assistant_events` log is left untouched — "the original
//! stays idle, intact, independently resumable" (ADR-0101) — just no longer
//! reachable from this DB row.

use axum::Json;
use axum::extract::{Extension, Path, State};
use entanglement_core::{InMsg, OutEvent, SessionId};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use sea_orm::{ActiveModelTrait, Set};

use super::mutate::{resolve_model_with_provider, session_mcp_specs};
use super::turn::{engine_session_id, send_and_collect, to_detail};
use super::{SessionDetail, load_owned, parse_id_array, subagent_links, tree};
use crate::ai::engine::{BUILD_PROFILE, SWITCHABLE_PROFILES, SiteEngine};
use crate::ai::projection;
use crate::entity::assistant_session;
use crate::routes::api::error::{ApiError, ApiResult};
use crate::routes::ws::{Envelope, Topic};
use crate::state::AppState;

/// The profile the successor is spawned under: the session's own, so a
/// Researcher/Page writer/Designer chat stays one after compaction (its
/// seeded first turn included). Safe for `designer`: a row only holds that
/// profile when its whole history ran under it (`mutate::
/// check_designer_switch`). A row value the engine does not register falls
/// back to the root profile instead of failing the spawn.
fn successor_profile(session: &assistant_session::Model) -> String {
    let own = session.agent_profile.as_str();
    let known = SWITCHABLE_PROFILES.contains(&own);
    if known { own } else { BUILD_PROFILE }.to_string()
}

#[derive(serde::Deserialize, Default)]
pub struct CompactBody {
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub kept: Option<u64>,
}

pub async fn compact(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
    Json(input): Json<CompactBody>,
) -> ApiResult<Json<SessionDetail>> {
    let session = load_owned(&state, user_id, id).await?;
    // Root-only (#101): compaction forks a fresh `u{user_id}:{uuid}` successor
    // and repoints the row at it, which for a child row would both claim a root
    // id and cut it loose from the log its siblings are still filed under.
    tree::require_root(&session, "compact")?;
    let source = engine_session_id(&session)?;

    let engine = &state.agent_engine;
    engine
        .ensure_live(&state.db, source.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("failed to resume engine session: {e}")))?;

    let mut args = serde_json::Map::new();
    if let Some(instructions) = &input.instructions {
        args.insert(
            "instructions".into(),
            serde_json::Value::String(instructions.clone()),
        );
    }
    if let Some(kept) = input.kept {
        args.insert("kept".into(), serde_json::Value::from(kept));
    }
    let oneshot = InMsg::Oneshot {
        session: source.clone(),
        op: "compact".into(),
        args: serde_json::Value::Object(args),
    };

    // Pin the source to the session's selected model *before* the summarize
    // oneshot so the summary runs under that model, not the engine default. A
    // resumed source (`ensure_live` above) carries no `SetModel` pin — the pin
    // is never persisted/replayed (`persistence::resume_session`) — so without
    // this the oneshot falls back to the engine default. Sent directly (not
    // via `send_and_collect`'s `msgs`): `SetModel` emits no `Done`/`Error`, so
    // folding it in would make `send_and_collect` wait on an obligation that
    // never settles. With no live turn after a resume it applies immediately,
    // before the oneshot turn. Resolved once and reused for the successor
    // re-pin below.
    let (model_row, provider_row) = resolve_model_with_provider(&state, session.model_id).await?;
    engine
        .holly
        .send(InMsg::SetModel {
            session: source.clone(),
            provider: provider_row.label.clone(),
            model: model_row.id.to_string(),
        })
        .await
        .map_err(|_| ApiError::Internal("engine inbox closed".into()))?;

    let report = send_and_collect(engine, &source, vec![oneshot], Vec::new()).await?;
    if let Some(message) = turn_error(&report) {
        return Err(ApiError::BadRequest(message));
    }
    let Some((compacted, summary)) = compacted_event(&report) else {
        return Err(ApiError::Internal(
            "compact produced neither a summary nor an error".into(),
        ));
    };

    // Carry the session's current MCP selection forward onto the successor
    // *before* spawning it — `set_session_mcp_specs` is a plain in-process
    // cache write (no engine round trip), so it's guaranteed to land before
    // the successor's seeded first turn can make its own tool calls.
    let mcp_ids = parse_id_array(&session.enabled_mcp_server_ids);
    let specs = session_mcp_specs(&state, user_id, &mcp_ids).await;
    let successor = SiteEngine::session_id_for_user(user_id);
    engine.set_session_mcp_specs(successor.clone(), specs);

    let spawn = InMsg::Spawn {
        session: successor.clone(),
        parent: None,
        predecessor: Some(source.clone()),
        agent: successor_profile(&session),
        prompt: summary.clone(),
        // Multi-user mode (0.6, ADR-0147) is not adopted here — the site keys
        // its own `u{user_id}:{uuid}` session ids instead. Ignored anyway: a
        // `predecessor: Some(_)` spawn inherits its predecessor's user.
        user: None,
    };
    let mut collected = send_and_collect(engine, &successor, vec![spawn], Vec::new()).await?;
    engine.mark_live(successor.clone());

    // `entanglement_runtime`'s persistence tap synthesizes the seeded
    // `InMsg::Prompt` into `assistant_events` asynchronously, once the
    // successor's own `SessionStarted` lands (its `persistence.rs`, #421) —
    // it is never re-broadcast, so this response (built from what we just
    // observed, not a re-read — see `turn.rs`'s module doc) mirrors that
    // exact record locally for `projection::project` to render as the
    // successor's opening user-role message.
    collected.insert(
        0,
        LogRecord::new(
            successor.clone(),
            LogPayload::In(InMsg::prompt(successor.clone(), summary.clone())),
        ),
    );

    // Best-effort: the successor's very first turn (the seeded summary
    // above) already ran under the engine's default model by the time this
    // lands (`SetModel` is stashed behind a live turn, applied once it
    // settles — `entanglement_core::protocol::InMsg::SetModel`'s doc) since
    // `InMsg::Spawn` queues that turn immediately and there is no way to
    // land `SetModel` any earlier for an id that doesn't exist yet. Every
    // turn from here on uses the session's actual pinned model. Reuses the
    // `(model_row, provider_row)` resolved for the source pre-pin above.
    engine
        .holly
        .send(InMsg::SetModel {
            session: successor.clone(),
            provider: provider_row.label.clone(),
            model: model_row.id.to_string(),
        })
        .await
        .map_err(|_| ApiError::Internal("engine inbox closed".into()))?;

    engine
        .holly
        .send(InMsg::CloseSession {
            session: source.clone(),
        })
        .await
        .map_err(|_| ApiError::Internal("engine inbox closed".into()))?;
    engine.clear_session_mcp_specs(&source);
    engine.forget_live(&source);

    let mut active: assistant_session::ActiveModel = session.into();
    active.engine_session_id = Set(Some(successor.0.clone()));
    // The root's own log key moves with it — everything this row reads from
    // here on lives under the successor, including any sub-agent it spawns
    // next (`ws_bridge::child_rows` inherits this column). Children spawned
    // *before* the compaction deliberately keep the pre-compaction value:
    // their records are still filed under the old key and stay readable there
    // (#99's whole reason for storing an engine id rather than a row pointer).
    active.root_engine_session_id = Set(successor.0.clone());
    active.updated_at = Set(chrono::Utc::now().fixed_offset());
    let updated = active.update(&state.db).await?;

    publish_compacted(&state, user_id, id, &compacted, &successor);

    let child_ids = subagent_links::hydrate_child_rows(&state.db, &collected).await;
    // The **successor**, not the source: this response is the successor's
    // opening transcript (its seeded prompt plus whatever that first turn
    // produced). `project` used to infer its target from `collected`'s first
    // record, which happened to be the successor's synthesized prompt spliced
    // in above — correct only by accident, and silently wrong the moment that
    // splice moves.
    let mut projected = projection::project(&collected, &successor);
    subagent_links::splice_child_db_ids(&mut projected, &child_ids);
    Ok(Json(to_detail(&updated, projected)))
}

/// The manual (`auto: false`) `Compacted` report in `records` plus its
/// summary text, if any — `send_and_collect` only ever returns records
/// belonging to the targeted session, so this is unambiguous.
fn compacted_event(records: &[LogRecord]) -> Option<(OutEvent, String)> {
    records.iter().find_map(|r| match &r.payload {
        LogPayload::Out(
            ev @ OutEvent::Compacted {
                summary,
                auto: false,
                ..
            },
        ) => Some((ev.clone(), summary.clone())),
        _ => None,
    })
}

/// The turn's own `OutEvent::Error` message, if the oneshot failed instead of
/// producing a summary (e.g. "no conversation history", an oversized
/// transcript — see `entanglement_core::session::summarize::SummarizeError`).
fn turn_error(records: &[LogRecord]) -> Option<String> {
    records.iter().find_map(|r| match &r.payload {
        LogPayload::Out(OutEvent::Error { message, .. }) => Some(message.clone()),
        _ => None,
    })
}

/// Broadcast the fork over the `assistant` WS topic (issue #40) so another
/// open tab on this session notices its `engine_session_id` moved and
/// refetches — same envelope shape `ws_bridge.rs` uses (the real
/// `OutEvent::Compacted` JSON plus a spliced `db_session_id`), with
/// `successor_session_id` added since, unlike every other forwarded event,
/// the session this fired on (`source`) is not the one a follow-up read
/// should resume.
fn publish_compacted(
    state: &AppState,
    user_id: i32,
    db_session_id: i32,
    compacted: &OutEvent,
    successor: &SessionId,
) {
    let Ok(mut payload) = serde_json::to_value(compacted) else {
        tracing::error!(
            db_session_id,
            "failed to serialize Compacted event for ws broadcast"
        );
        return;
    };
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("db_session_id".into(), serde_json::json!(db_session_id));
        obj.insert(
            "successor_session_id".into(),
            serde_json::json!(successor.0),
        );
    }
    state.ws_hub.publish(
        user_id,
        Envelope {
            topic: Topic::Assistant,
            event: "compacted".into(),
            payload,
        },
    );
}
