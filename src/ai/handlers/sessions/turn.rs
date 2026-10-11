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
use routing::{Readiness, decision_readiness, remember_deny};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

use super::tree::root_engine_id;
use super::{MessageView, SessionDetail, SessionSummary, load_owned, subagent_links};
use crate::ai::persistence;
use crate::ai::projection::{self, ProjectedMessage};
use crate::entity::{assistant_event, assistant_session};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

/// How long `approve` waits for a call's approval waiter to be (re-)parked.
/// Re-offering is a policy check and a task hop; only a call that is no
/// longer pending reaches this.
const DECISION_WAITER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
    // A sub-agent row is prompt-able in its own right (#101): the turn runs on
    // the row's own engine session (`target`), while the log it reads and folds
    // stays keyed on the tree's root — see `ensure_target_live`.
    let target = engine_session_id(&session)?;
    let root = root_engine_id(&session);
    let engine = &state.agent_engine;
    ensure_target_live(&state, &root, &target).await?;
    let prior = load_prior_records(&state.db, &root).await?;

    let msg = InMsg::prompt(target.clone(), input.text);
    let collected = collect::send_and_collect(engine, &target, vec![msg], None).await?;

    build_detail(&state, id, &target, prior, collected).await
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
    let target = engine_session_id(&session)?;
    let root = root_engine_id(&session);
    if body.decisions.is_empty() {
        return Err(ApiError::BadRequest("decisions is required".into()));
    }

    let engine = &state.agent_engine;
    ensure_target_live(&state, &root, &target).await?;
    // Root-keyed on purpose, even when approving from a child's own view:
    // `session_for_call_awaiting` and `open_calls_after` scan *every*
    // session's records to find who owns a `tool_call_id`, so narrowing this to
    // the child would break routing a decision to a sibling or to the root.
    let prior = load_prior_records(&state.db, &root).await?;

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
            session_for_call_awaiting(&state.db, &root, &prior, &d.tool_call_id).await?;
        // The in-memory waiter set first: request ids repeat across rounds
        // (e.g. Gemini's `{name}#{ordinal}`), so a snapshot still missing a
        // re-asked call's new `ToolRequest` row reads as "resolved".
        let readiness = decision_readiness(
            engine.decision_parked(&target_session, &d.tool_call_id),
            &prior,
            &target_session,
            &d.tool_call_id,
        );
        // A stale UI or a double click: nothing is parked, so a decision
        // would be a silent no-op the request then waits out.
        if readiness == Readiness::Resolved {
            return Err(ApiError::BadRequest(format!(
                "tool call `{}` is already resolved",
                d.tool_call_id
            )));
        }
        // A just-resumed session re-offers its parked call asynchronously; a
        // decision sent before its waiter exists is dropped and this request
        // would sit out `TURN_TIMEOUT` (#124). Sent anyway on a miss: the
        // call's output may only be lagging behind in `assistant_events`.
        if readiness == Readiness::Unknown
            && !engine
                .await_decision_waiter(&target_session, &d.tool_call_id, DECISION_WAITER_TIMEOUT)
                .await
        {
            tracing::warn!(
                tool_call_id = %d.tool_call_id,
                "no parked approval waiter for this call; sending the decision anyway"
            );
        }
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

    let collected =
        collect::send_and_collect(engine, &target, msgs, Some((&state.db, &root))).await?;
    build_detail(&state, id, &target, prior, collected).await
}

/// Make sure `target` has a live in-memory engine task before a turn is driven
/// on it (#101).
///
/// For a root (`target == root`) this is the plain `ensure_live` it always was.
/// For a sub-agent child it is a three-step ladder, because a child's own id
/// can never be handed to `ensure_live`: its `assistant_events` live under the
/// root, so resuming that id would replay an empty log and cache a *blank*
/// session under it forever (see `tree`'s module doc).
///
/// 1. Resume the **root**. A root resume cascades over its whole spawn sub-tree
///    (ADR-0112), so a child that was live when recording stopped usually comes
///    back with it — no child-specific work needed at all.
/// 2. If the child is live (already running — upstream never closes a finished
///    sub-agent, so this is the common case — or just re-materialized by that
///    cascade), stop: `Holly::resume` refuses a live id.
/// 3. Otherwise rebuild it from its own slice of the root's log
///    (`persistence::resume_child_session`).
///
/// No branch falls through to a bare `InMsg::Prompt` on `target` without one
/// of these three having run first, so a sub-agent turn can never hit
/// entanglement's lazy-`Prompt` path (`holly.rs`'s unknown-session fallback,
/// which materializes a blank session under the default `build` profile) —
/// that path is a genuine hole this handler must not open, not the resume
/// cascade itself (see `subagent_links::repair_profile`).
async fn ensure_target_live(
    state: &AppState,
    root: &SessionId,
    target: &SessionId,
) -> ApiResult<()> {
    let engine = &state.agent_engine;
    engine
        .ensure_live(&state.db, root.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("failed to resume engine session: {e}")))?;
    if target == root || engine.await_live(target).await {
        return Ok(());
    }
    persistence::resume_child_session(&state.db, &engine.holly, root, target)
        .await
        .map_err(|e| {
            // The child id and the underlying cause are useful in the log but
            // not to a client, which can't act on either.
            tracing::warn!(error = %e, child = %target.0, root = %root.0, "failed to resume sub-agent session");
            ApiError::Conflict("this sub-agent session can no longer be resumed".into())
        })?;
    engine.mark_live(target.clone());
    Ok(())
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

/// Fold `prior + collected` into this response. `target` is the session whose
/// transcript is being returned — the one the turn ran on; any sub-agent it
/// spawned becomes a reference card, not a nested transcript (#100).
async fn build_detail(
    state: &AppState,
    id: i32,
    target: &SessionId,
    prior: Vec<LogRecord>,
    collected: Vec<LogRecord>,
) -> ApiResult<Json<SessionDetail>> {
    let mut records = prior;
    records.extend(collected);
    // The turn that just ran may itself have spawned a sub-agent; write its
    // row here rather than hoping `ws_bridge`'s independent task got there
    // first, or this very response carries a card with no session to open
    // (see `subagent_links`).
    let child_ids = subagent_links::hydrate_child_rows(&state.db, &records).await;
    let mut projected = projection::project(&records, target);
    subagent_links::splice_child_db_ids(&mut projected, &child_ids);
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
