//! [`send_and_collect`] — the engine-driving/event-collection core
//! `send_message`/`approve` (in `turn.rs`, the parent module) both call.
//! Split out on its own so `turn.rs` stays under the project's 400-line cap;
//! see `turn.rs`'s module doc for the response-construction rationale this
//! function exists to serve.

use std::time::Duration;

use entanglement_core::{AgentState, InMsg, OutEvent, SessionId};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use tokio::sync::broadcast;

use crate::ai::engine::{self, SiteEngine};
use crate::routes::api::error::{ApiError, ApiResult};

/// A stuck turn (dead provider, hung tool) must not hang the HTTP request
/// forever — generous enough for a real multi-tool-call turn, short enough
/// that a client isn't left hanging indefinitely.
const TURN_TIMEOUT: Duration = Duration::from_secs(180);

/// Send `msgs` through the engine (subscribing first — embedding.md §1's
/// race-avoidance: a reply must never arrive before we start listening for
/// it), then collect every record belonging to `session` — the sent `InMsg`s
/// plus each observed `OutEvent` — until the turn settles (`Done`, `Error`,
/// or a paused `WaitingApproval` status) or [`TURN_TIMEOUT`] elapses.
///
/// Also collects events from any sub-agent (#17) `session` spawns during this
/// call, so a synchronous "research X"/"draft a page" response already
/// includes the child's own turn instead of only picking it up on a later
/// resume/read. `descendants` is tracked locally off this same broadcast
/// receiver rather than via `engine::root_session_of`'s process-global cache —
/// that cache is written by a *different* subscriber task (`engine.rs`'s
/// session watcher), so trusting it here would race this loop against that
/// task's own processing of the same `SessionStarted`. Reassessed for #43:
/// entanglement 0.3.0's resume-cascade (ADR-0112) and spawn-prompt
/// persistence (ADR-0113) are both about *replaying a settled log*, not about
/// ordering across this process's own live broadcast subscribers, so they
/// don't touch this race — `send_and_collect` only ever runs against a
/// session already `ensure_live`'d for the current turn, live traffic within
/// one HTTP request, never a resume in progress. The local `descendants` fold
/// stays.
///
/// Each sent `InMsg` settles independently, by its own criterion:
///
/// - A plain [`InMsg::Prompt`] (or anything else with no `request_id`)
///   settles when *its own session* emits `Done`, `Error`, or a paused
///   `WaitingApproval` status — the original, turn-level notion.
/// - An [`InMsg::Approve`]/[`InMsg::Reject`] settles the moment its own
///   `request_id` gets a matching [`OutEvent::ToolOutput`] — which the engine
///   emits unconditionally as soon as *that specific call* resolves
///   (`entanglement_core::session`'s `SessionCmd::ToolResult` handling calls
///   `emit_tool_output` before it ever checks whether the rest of the current
///   batch has drained). This is deliberately *not* gated on the whole
///   session settling: a turn with several tool calls pending at once only
///   resumes once *every* one of them is resolved (`TurnState::is_drained`),
///   so if this waited for `Done`/`Error`/`WaitingApproval` on the session
///   like a prompt does, approving anything other than the last pending call
///   in a batch would hang this request for the full [`TURN_TIMEOUT`] — the
///   session simply has nothing new to say until the rest of the batch is
///   also decided.
///
/// `extra_pending` (`approve`'s `open_tool_requests`) names every call
/// already outstanding on a target session *before* this request's own
/// decisions, purely as a **readiness signal** — it never blocks this
/// response. The moment this request's *own* decisions for a session settle,
/// that session graduates into `session_targets` (below) *only if*
/// `extra_pending` has no other still-open entry for it too: only then does
/// that session's `TurnState` genuinely drain (`is_drained`), so the engine's
/// own `drive_turn` is already running or about to — *now* it's correct to
/// also wait (full [`TURN_TIMEOUT`] budget) for that session's `Done`/
/// `Error`/next `WaitingApproval`, the same way a plain prompt always has,
/// instead of returning the instant the last `ToolOutput` lands and silently
/// omitting the continuation. If a sibling `extra_pending` call for that
/// session is *still* open at that moment (a batch this request only
/// partially decided), this correctly does **not** wait further — that's
/// exactly the case that used to hang for the full timeout, since nothing
/// new arrives on that session until the rest of the batch is decided
/// elsewhere.
///
/// A batched `approve` whose `decisions` resolve calls on two different
/// sessions (the root and a child, or two sibling children) waits for every
/// one of them — settling on whichever finishes first would silently omit
/// the others' outcomes from the response. A still-running detached
/// `agent_spawn`/`agent` child that *isn't* targeted by any `msgs` never
/// blocks this response either way (ADR-0026).
pub(in crate::ai::handlers::sessions) async fn send_and_collect(
    engine: &SiteEngine,
    session: &SessionId,
    msgs: Vec<InMsg>,
    extra_pending: Vec<(SessionId, String)>,
) -> ApiResult<Vec<LogRecord>> {
    let mut sub = engine.holly.subscribe();
    let mut collected = Vec::with_capacity(msgs.len() + 8);
    let mut targets: std::collections::HashSet<SessionId> = std::collections::HashSet::new();
    // This request's own decisions — the only obligations that gate the
    // response at all. Settled individually by each one's own `ToolOutput`
    // (see this function's doc), not by the session as a whole draining.
    let mut pending_calls: std::collections::HashSet<(SessionId, String)> =
        std::collections::HashSet::new();
    // Sessions that had at least one `Approve`/`Reject` — once *all* of a
    // session's own `pending_calls` entries drain, it's a candidate to
    // graduate into `session_targets` below (gated further on
    // `open_others`).
    let mut approve_sessions: std::collections::HashSet<SessionId> =
        std::collections::HashSet::new();
    // Session-level obligations, settled by that session's own `Done`/
    // `Error`/`WaitingApproval`: a plain prompt (or anything with no
    // `request_id`) immediately; an approve-session only once it's confirmed
    // fully drained (see this function's doc).
    let mut session_targets: std::collections::HashSet<SessionId> =
        std::collections::HashSet::new();
    for msg in msgs {
        // #17: an `Approve`/`Reject` for a sub-agent's own pending call
        // targets that *child* session (see `approve`'s `session_for_call`
        // routing) — tag the record with the message's own target, not
        // unconditionally the root, so `projection::project`'s per-session
        // partitioning doesn't misfile it under the wrong turn.
        let msg_session = msg.session().cloned().unwrap_or_else(|| session.clone());
        targets.insert(msg_session.clone());
        match &msg {
            InMsg::Approve { request_id, .. } | InMsg::Reject { request_id, .. } => {
                pending_calls.insert((msg_session.clone(), request_id.clone()));
                approve_sessions.insert(msg_session.clone());
            }
            _ => {
                session_targets.insert(msg_session.clone());
            }
        }
        engine
            .holly
            .send(msg.clone())
            .await
            .map_err(|_| ApiError::Internal("engine inbox closed".into()))?;
        collected.push(LogRecord::new(msg_session, LogPayload::In(msg)));
    }
    if targets.is_empty() {
        targets.insert(session.clone());
        session_targets.insert(session.clone());
    }
    // Readiness-only tracking (see this function's doc) — never gates the
    // response, only whether a session that just drained *our* decisions is
    // also free of any other still-open call.
    let mut open_others: std::collections::HashSet<(SessionId, String)> = extra_pending
        .into_iter()
        .filter(|key| !pending_calls.contains(key))
        .collect();
    for (s, _) in &open_others {
        targets.insert(s.clone());
    }

    let mut descendants: std::collections::HashSet<SessionId> = std::collections::HashSet::new();
    let mut settled_sessions: std::collections::HashSet<SessionId> =
        std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + TURN_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(ApiError::Internal("assistant turn timed out".into()));
        }
        let ev = match tokio::time::timeout(remaining, sub.recv()).await {
            Ok(Ok(ev)) => ev,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                return Err(ApiError::Internal("engine event stream closed".into()));
            }
            Err(_) => return Err(ApiError::Internal("assistant turn timed out".into())),
        };
        let Some(ev_session) = ev.session().cloned() else {
            continue; // a session-less query reply (SessionList/History)
        };
        if let OutEvent::SessionStarted {
            session: started,
            parent: Some(parent),
            ..
        } = &ev
            && (*session == *parent || descendants.contains(parent))
        {
            descendants.insert(started.clone());
        }
        // Ours if: the root itself, a target we deliberately addressed
        // (`approve`'s `session_for_call` routing — an *existing* child,
        // whose one-time `SessionStarted` this fresh subscription never
        // sees), a child spawned live during *this* call (`descendants`,
        // populated above), or — the general case, safe once the watcher
        // task's cache has caught up — a descendant per
        // `engine::root_session_of`.
        let ours = ev_session == *session
            || targets.contains(&ev_session)
            || descendants.contains(&ev_session)
            || engine::root_session_of(&ev_session) == *session;
        if !ours {
            continue; // an unrelated session's event on the shared fan-out
        }
        if let OutEvent::ToolOutput { request_id, .. } = &ev {
            let key = (ev_session.clone(), request_id.clone());
            pending_calls.remove(&key);
            open_others.remove(&key);
            // Our own decisions for this session just drained *and* nothing
            // else was left open for it either — it's now safe to also wait
            // for its continuation (see this function's doc). Checked on
            // every `ToolOutput` for this session, not just ours, since a
            // sibling resolving (concurrently, elsewhere) can be what
            // finally empties `open_others` for it.
            if approve_sessions.contains(&ev_session)
                && !pending_calls.iter().any(|(s, _)| s == &ev_session)
                && !open_others.iter().any(|(s, _)| s == &ev_session)
            {
                session_targets.insert(ev_session.clone());
            }
        }
        if session_targets.contains(&ev_session)
            && matches!(
                ev,
                OutEvent::Done { .. }
                    | OutEvent::Error { .. }
                    | OutEvent::Status {
                        state: AgentState::WaitingApproval,
                        ..
                    }
            )
        {
            settled_sessions.insert(ev_session.clone());
        }
        // Safety net: if the whole session ends before individually
        // acknowledging one of our calls (shouldn't happen — `seam::reply`
        // always runs on both the `Approve` and `Reject` paths — but a
        // `Done`/`Error` is a stronger signal than any per-call obligation),
        // don't hang on it forever either.
        if matches!(ev, OutEvent::Done { .. } | OutEvent::Error { .. }) {
            pending_calls.retain(|(s, _)| s != &ev_session);
        }
        collected.push(LogRecord::new(ev_session, LogPayload::Out(ev)));
        if pending_calls.is_empty() && settled_sessions.len() == session_targets.len() {
            break;
        }
    }
    Ok(collected)
}
