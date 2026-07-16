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

mod collect;

use axum::Json;
use axum::extract::{Extension, Path, State};
use entanglement_core::{ApprovalScope, InMsg, OutEvent, SessionId};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use super::{MessageView, SessionDetail, SessionSummary, load_owned};
use crate::ai::projection::{self, ProjectedMessage};
use crate::ai::tool_permissions::Effect;
use crate::entity::{assistant_event, assistant_session, tool_permission};
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
    let collected = collect::send_and_collect(engine, &session_id, vec![msg]).await?;

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
        // child parked forever. Route each decision to whichever session
        // actually owns the call (falling back to the root if the call isn't
        // found in `prior` yet — the same eventual-consistency assumption
        // `remember_deny`'s tool-name lookup below already makes).
        let target_session =
            session_for_call(&prior, &d.tool_call_id).unwrap_or_else(|| session_id.clone());
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

    let collected = collect::send_and_collect(engine, &session_id, msgs).await?;
    build_detail(&state, id, prior, collected).await
}

/// Persist an "always deny" `tool_permissions` rule for the tool behind
/// `call_id`, resolved by scanning `prior` for the `ToolCall`/`ToolRequest`
/// record that named it. Mirrors the old approve-handler's direct insert;
/// only reachable for `!approve && remember`, since an *allowed* `Always`
/// grant is recorded by the engine itself (see `approve`'s doc).
async fn remember_deny(
    state: &AppState,
    user_id: i32,
    prior: &[LogRecord],
    call_id: &str,
) -> ApiResult<()> {
    let Some(tool_name) = tool_name_for_call(prior, call_id) else {
        tracing::warn!(
            call_id,
            "remembered deny: no matching tool call in history; skipping"
        );
        return Ok(());
    };
    let existing = tool_permission::Entity::find()
        .filter(tool_permission::Column::UserId.eq(user_id))
        .filter(tool_permission::Column::Name.eq(tool_name.as_str()))
        .one(&state.db)
        .await?;
    match existing {
        Some(row) if row.effect == Effect::Deny.as_str() => {}
        Some(row) => {
            let mut active: tool_permission::ActiveModel = row.into();
            active.effect = Set(Effect::Deny.as_str().to_string());
            active.update(&state.db).await?;
        }
        None => {
            tool_permission::ActiveModel {
                user_id: Set(user_id),
                name: Set(tool_name),
                effect: Set(Effect::Deny.as_str().to_string()),
                priority: Set(100),
                ..Default::default()
            }
            .insert(&state.db)
            .await?;
        }
    }
    Ok(())
}

fn tool_name_for_call(records: &[LogRecord], call_id: &str) -> Option<String> {
    records.iter().rev().find_map(|r| match &r.payload {
        LogPayload::Out(OutEvent::ToolCall {
            request_id, tool, ..
        })
        | LogPayload::Out(OutEvent::ToolRequest {
            request_id, tool, ..
        }) if request_id == call_id => Some(tool.clone()),
        _ => None,
    })
}

/// The session that actually owns `call_id`'s pending tool call — a
/// sub-agent (#17) child's own session if that's where the matching
/// `ToolCall`/`ToolRequest` record lives, otherwise `None` (the caller falls
/// back to the root). `PendingDecisions` keys a waiter by `(session,
/// request_id)`, so `approve`/`reject` must address the exact session that
/// registered it.
fn session_for_call(records: &[LogRecord], call_id: &str) -> Option<SessionId> {
    records.iter().rev().find_map(|r| match &r.payload {
        LogPayload::Out(OutEvent::ToolCall { request_id, .. })
        | LogPayload::Out(OutEvent::ToolRequest { request_id, .. })
            if request_id == call_id =>
        {
            Some(r.session.clone())
        }
        _ => None,
    })
}

fn engine_session_id(session: &assistant_session::Model) -> ApiResult<SessionId> {
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
