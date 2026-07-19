//! Decision-routing helpers `approve` (the parent module) needs: which
//! session actually owns a `tool_call_id`'s pending call, whether anything
//! else is still open alongside it, and the "always deny" remember-rule
//! write. Split out of `turn.rs` to keep that file under the project's
//! 400-line cap.

use std::time::Duration;

use entanglement_core::{OutEvent, SessionId};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use super::load_prior_records;
use crate::ai::tool_permissions::Effect;
use crate::entity::tool_permission;
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Every `(session, request_id)` still awaiting a decision *before* this
/// request's own decisions are applied — passed to `send_and_collect` purely
/// as a readiness signal (see its doc): a batch with several pending calls
/// only resumes once *all* of them resolve
/// (`entanglement_core::session::TurnState::is_drained`), so `approve`'s own
/// decisions settling isn't enough on its own to know whether it's safe to
/// also wait for that session's continuation — this says whether anything
/// *else* is still open. `ToolCall`/`ToolRequest`/`ToolOutput` records are
/// never pruned (same fact `session_for_call`'s doc relies on), so a linear
/// scan of `prior` for a `ToolRequest` with no later matching `ToolOutput` is
/// exact, not a heuristic.
pub(super) fn open_tool_requests(prior: &[LogRecord]) -> Vec<(SessionId, String)> {
    let resolved: std::collections::HashSet<&str> = prior
        .iter()
        .filter_map(|r| match &r.payload {
            LogPayload::Out(OutEvent::ToolOutput { request_id, .. }) => Some(request_id.as_str()),
            _ => None,
        })
        .collect();
    prior
        .iter()
        .filter_map(|r| match &r.payload {
            LogPayload::Out(OutEvent::ToolRequest { request_id, .. })
                if !resolved.contains(request_id.as_str()) =>
            {
                Some((r.session.clone(), request_id.clone()))
            }
            _ => None,
        })
        .collect()
}

/// Persist an "always deny" `tool_permissions` rule for the tool behind
/// `call_id`, resolved by scanning `prior` for the `ToolCall`/`ToolRequest`
/// record that named it. Mirrors the old approve-handler's direct insert;
/// only reachable for `!approve && remember`, since an *allowed* `Always`
/// grant is recorded by the engine itself (see `approve`'s doc).
pub(super) async fn remember_deny(
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
/// `ToolCall`/`ToolRequest` record lives, otherwise `None`. `PendingDecisions`
/// keys a waiter by `(session, request_id)`, so `approve`/`reject` must
/// address the exact session that registered it. Consumed only by
/// [`session_for_call_awaiting`], which retries a fresh DB read before giving
/// up — see its doc for why a bare miss here isn't final.
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

/// How many times [`session_for_call_awaiting`] reloads `assistant_events`
/// before concluding `call_id` is genuinely unknown.
const CALL_LOOKUP_RETRY_ATTEMPTS: u32 = 5;
/// Delay between each reload. Mirrors `session_tree.rs`'s
/// `SESSION_PARENT_RETRY_DELAY` pattern for the analogous "child link not yet
/// visible" race, but longer: that retry only waits on an in-memory
/// `DashMap` write from this same process, while this one re-runs an actual
/// `assistant_event::Entity::find()...all(db)` query, so a plain scheduler
/// hiccup isn't the only source of delay — give a real DB round trip a bit
/// more slack.
const CALL_LOOKUP_RETRY_DELAY: Duration = Duration::from_millis(25);

/// [`session_for_call`], but retried a few times against a fresh DB read
/// before giving up — closes the TOCTOU window `DbSink` (`crate::ai::
/// persistence`) leaves open: it appends every broadcast `LogRecord` to
/// `assistant_events` behind a bounded channel and its own async writer task,
/// so a user can already see (over WS) and click Approve/Reject on a
/// `ToolRequest` whose row hasn't landed in the DB yet by the time this
/// handler's `load_prior_records` reads history. `ToolCall`/`ToolRequest`
/// records are never pruned even after the call resolves, so if `call_id`
/// still doesn't appear anywhere in the session's tree after
/// [`CALL_LOOKUP_RETRY_ATTEMPTS`] reloads, it was never actually issued in
/// this session's tree — a garbage/typo'd id, or a stale resubmission of one
/// already resolved and gone from `PendingDecisions` — and this fails fast
/// with a `BadRequest` rather than routing to the root and letting
/// `send_and_collect` hang for the full `TURN_TIMEOUT` on a target that will
/// never settle.
///
/// `pub` (not the more natural private/`pub(super)`), and re-exported from
/// `sessions/mod.rs`, solely so
/// `tests/assistant_session_subagent_approval_race.rs` — a separate crate —
/// can exercise the retry directly against a real DB. Reproducing the race
/// deterministically through the full `POST .../approve` HTTP path would
/// need to win a timing race against `DbSink`'s async writer during a live
/// sub-agent turn; calling this function directly against a fabricated,
/// still-behind-the-DB `prior` is the same fix path `session_tree.rs`'s
/// `record_session_started` is already `pub` for (see its doc).
pub async fn session_for_call_awaiting(
    db: &sea_orm::DatabaseConnection,
    session_id: &SessionId,
    prior: &[LogRecord],
    call_id: &str,
) -> ApiResult<SessionId> {
    if let Some(session) = session_for_call(prior, call_id) {
        return Ok(session);
    }
    for attempt in 0..CALL_LOOKUP_RETRY_ATTEMPTS {
        tokio::time::sleep(CALL_LOOKUP_RETRY_DELAY).await;
        let reloaded = load_prior_records(db, session_id).await?;
        if let Some(session) = session_for_call(&reloaded, call_id) {
            return Ok(session);
        }
        tracing::debug!(call_id, attempt, "tool call not yet in history, retrying");
    }
    Err(ApiError::BadRequest(format!(
        "tool call `{call_id}` is unknown or already resolved"
    )))
}
