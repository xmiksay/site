//! `send_message`/`approve` — the two handlers that actually drive a turn
//! through `Holly`, plus the shared wait/collect/project plumbing they need.
//! Split out of `mod.rs` to keep that file under the 400-line cap; see its
//! module doc for how the two files fit together.
//!
//! ## Why the response is built from what we just saw, not a re-read of
//! `assistant_events`
//!
//! `DbSink` (`crate::ai::persistence`) appends every broadcast record to
//! `assistant_events` behind a bounded channel and its own writer task, so
//! `append` never blocks the tap. That means there is no guarantee the row
//! for e.g. the `Done` this handler just observed is durably committed the
//! instant the handler observes it — a naive "wait for Done, then re-query
//! the DB" would race that writer. Instead, [`collect::send_and_collect`]
//! builds the exact same `LogRecord`s locally (the `InMsg`s this handler
//! sends plus every `OutEvent` it observes for the session) and folds them
//! onto the *previously persisted* prefix read before sending — `DbSink`
//! will persist this same tail independently, so the next request's DB read
//! sees it with this handler never having written anything itself.
//!
//! Reassessed for #43: this is purely a site-side artifact of `DbSink`'s own
//! async-writer design (`src/ai/persistence.rs`), not something
//! `entanglement_runtime`'s own persistence tap or resume guarantees (ADR-
//! 0112/0113) have any bearing on — those govern what a *replayed* log looks
//! like, not when this site's DB writer catches up to a broadcast it already
//! tapped. The local fold stays.

mod collect;
mod routing;

pub(in crate::ai::handlers::sessions) use collect::send_and_collect;
pub use routing::session_for_call_awaiting;

use axum::Json;
use axum::extract::{Extension, Path, State};
use entanglement_core::{ApprovalScope, InMsg, SessionId};
use entanglement_runtime::session_store::LogRecord;
use routing::{open_tool_requests, remember_deny};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

use super::{MessageView, SessionDetail, SessionSummary, load_owned};
use crate::ai::projection::{self, ProjectedMessage};
use crate::entity::{assistant_event, assistant_session};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(serde::Deserialize)]
pub struct SendMessage {
    pub text: String,
}

#[derive(serde::Deserialize)]
pub struct ApprovalDecision {
    pub tool_call_id: String,
    pub approve: bool,
    /// When true, persist a rule so future calls of the same tool name skip
    /// the approval prompt (allow if `approve`, deny otherwise).
    #[serde(default)]
    pub remember: bool,
}

#[derive(serde::Deserialize)]
pub struct ApproveBody {
    pub decisions: Vec<ApprovalDecision>,
}

pub async fn send_message(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
    Json(input): Json<SendMessage>,
) -> ApiResult<Json<SessionDetail>> {
    let session = load_owned(&state, user_id, id).await?;
    if input.text.trim().is_empty() {
        return Err(ApiError::BadRequest("text is required".into()));
    }
    let session_id = engine_session_id(&session)?;

    let engine = &state.agent_engine;
    engine
        .ensure_live(&state.db, session_id.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("failed to resume engine session: {e}")))?;
    let prior = load_prior_records(&state.db, &session_id).await?;

    let msg = InMsg::prompt(session_id.clone(), input.text);
    let collected = collect::send_and_collect(engine, &session_id, vec![msg], Vec::new()).await?;

    build_detail(&state, id, prior, collected).await
}

/// POST /sessions/{id}/messages/{message_id}/approve
/// Body: { decisions: [{ tool_call_id, approve, remember }] }
///
/// `message_id` is accepted only for URL-shape compatibility with the
/// unchanged Vue client (`stores/assistant.ts` still POSTs to
/// `.../messages/{messageId}/approve`) — it no longer identifies a real
/// persisted row (messages aren't individually stored anymore, only the
/// event log is), so it is deliberately never resolved to anything here. The
/// actual target of each decision is `tool_call_id` (the engine's
/// `request_id`), unique within its own pending batch.
pub async fn approve(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path((id, _message_id)): Path<(i32, i32)>,
    Json(body): Json<ApproveBody>,
) -> ApiResult<Json<SessionDetail>> {
    let session = load_owned(&state, user_id, id).await?;
    let session_id = engine_session_id(&session)?;
    if body.decisions.is_empty() {
        return Err(ApiError::BadRequest("decisions is required".into()));
    }

    let engine = &state.agent_engine;
    engine
        .ensure_live(&state.db, session_id.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("failed to resume engine session: {e}")))?;
    let prior = load_prior_records(&state.db, &session_id).await?;

    // `ApprovalScope::Always` on an *approve* is persisted for us by
    // `SitePolicy::GrantStore::record`, invoked internally by the engine's
    // tool executor — no direct `tool_permissions` write needed here (unlike
    // the old handler, which inserted the row itself). But the engine's
    // `GrantStore` seam only models "always allow" grants; `Reject` carries
    // no scope at all, so a remembered *denial* has no engine-side
    // equivalent — that half still needs a direct write here, matching the
    // old handler's behavior for that one case.
    let mut msgs = Vec::with_capacity(body.decisions.len());
    for d in &body.decisions {
        // #17: `PendingDecisions` (the engine's approval waiter registry) is
        // keyed by `(session, request_id)`, not `request_id` alone — a
        // `researcher`/`page-writer` sub-agent's own pending tool call lives
        // under its *child* session, not this root, so an `Approve`/`Reject`
        // addressed to the root would silently resolve nothing and leave the
        // child parked forever (a documented, safe engine-side no-op that
        // then hangs `send_and_collect` for the full `TURN_TIMEOUT`). Route
        // each decision to whichever session actually owns the call, retrying
        // a fresh DB read a few times if `prior` doesn't have it yet (closes
        // `DbSink`'s async-writer TOCTOU window — see
        // `session_for_call_awaiting`'s doc) and failing fast with a `4xx` if
        // the call never shows up: silently falling back to the root here
        // used to work by accident for a root-level call, but was actively
        // wrong for a stale/unknown/misrouted sub-agent call id.
        let target_session =
            session_for_call_awaiting(&state.db, &session_id, &prior, &d.tool_call_id).await?;
        if d.approve {
            msgs.push(InMsg::Approve {
                session: target_session,
                request_id: d.tool_call_id.clone(),
                scope: if d.remember {
                    ApprovalScope::Always
                } else {
                    ApprovalScope::Once
                },
            });
        } else {
            if d.remember {
                remember_deny(&state, user_id, &prior, &d.tool_call_id).await?;
            }
            msgs.push(InMsg::Reject {
                session: target_session,
                request_id: d.tool_call_id.clone(),
                reason: None,
            });
        }
    }

    let extra_pending = open_tool_requests(&prior);
    let collected = collect::send_and_collect(engine, &session_id, msgs, extra_pending).await?;
    build_detail(&state, id, prior, collected).await
}

pub(super) fn engine_session_id(session: &assistant_session::Model) -> ApiResult<SessionId> {
    session
        .engine_session_id
        .clone()
        .map(SessionId::new)
        .ok_or_else(|| ApiError::Internal("session has no engine_session_id".into()))
}

/// Load `session`'s full persisted log, oldest first — the prefix every
/// request folds its own newly-observed records onto.
pub(super) async fn load_prior_records(
    db: &sea_orm::DatabaseConnection,
    session: &SessionId,
) -> ApiResult<Vec<LogRecord>> {
    let rows = assistant_event::Entity::find()
        .filter(assistant_event::Column::RootSessionId.eq(session.0.clone()))
        .order_by_asc(assistant_event::Column::Id)
        .all(db)
        .await?;
    rows.into_iter()
        .map(|r| {
            serde_json::from_value(r.payload)
                .map_err(|e| ApiError::Internal(format!("corrupt assistant_events row: {e}")))
        })
        .collect()
}

async fn build_detail(
    state: &AppState,
    id: i32,
    prior: Vec<LogRecord>,
    collected: Vec<LogRecord>,
) -> ApiResult<Json<SessionDetail>> {
    let mut records = prior;
    records.extend(collected);
    let projected = projection::project(&records);
    let session = assistant_session::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(to_detail(&session, projected)))
}

/// Wrap projected `{role, content}` pairs into the client's `MessageView`
/// shape. `assistant_events` rows don't map 1:1 to a client message (see
/// `projection`'s doc), so `id`/`seq` are just the 1-based position in the
/// projected list — stable within one response, which is all
/// `AssistantView.vue` needs (it has no per-message timestamp UI, only
/// in-order rendering), and `created_at` is stamped from the session's own
/// `updated_at` rather than threading per-record timestamps through
/// `project()` — the simpler of the two options the issue allowed for.
pub(super) fn to_detail(
    session: &assistant_session::Model,
    projected: Vec<ProjectedMessage>,
) -> SessionDetail {
    let stamp = session.updated_at.to_string();
    let messages = projected
        .into_iter()
        .enumerate()
        .map(|(i, p)| MessageView {
            id: (i + 1) as i32,
            seq: (i + 1) as i32,
            role: p.role.to_string(),
            content: p.content,
            created_at: stamp.clone(),
        })
        .collect();
    SessionDetail {
        summary: SessionSummary::from(session),
        messages,
    }
}
