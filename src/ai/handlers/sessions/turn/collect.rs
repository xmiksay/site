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
/// The turn settles once *every* targeted session has settled — normally
/// just `session` (a plain prompt/root-only approve), but `approve`'s
/// `session_for_call` routing can target a sub-agent child directly when a
/// decision is for its own pending call; that child (not the long-since-
/// `Done` root) is what actually resumes and eventually settles, so waiting
/// on `session` alone would hang for the full [`TURN_TIMEOUT`]. A batched
/// `approve` whose `decisions` resolve calls on two different sessions (the
/// root and a child, or two sibling children) waits for both — settling on
/// whichever finishes first would silently omit the other's outcome from the
/// response. A still-running detached `agent_spawn`/`agent` child that
/// *isn't* targeted by any `msgs` never blocks this response either way
/// (ADR-0026).
pub(in crate::ai::handlers::sessions) async fn send_and_collect(
    engine: &SiteEngine,
    session: &SessionId,
    msgs: Vec<InMsg>,
) -> ApiResult<Vec<LogRecord>> {
    let mut sub = engine.holly.subscribe();
    let mut collected = Vec::with_capacity(msgs.len() + 8);
    let mut targets: std::collections::HashSet<SessionId> = std::collections::HashSet::new();
    for msg in msgs {
        // #17: an `Approve`/`Reject` for a sub-agent's own pending call
        // targets that *child* session (see `approve`'s `session_for_call`
        // routing) — tag the record with the message's own target, not
        // unconditionally the root, so `projection::project`'s per-session
        // partitioning doesn't misfile it under the wrong turn.
        let msg_session = msg.session().cloned().unwrap_or_else(|| session.clone());
        targets.insert(msg_session.clone());
        engine
            .holly
            .send(msg.clone())
            .await
            .map_err(|_| ApiError::Internal("engine inbox closed".into()))?;
        collected.push(LogRecord::new(msg_session, LogPayload::In(msg)));
    }
    if targets.is_empty() {
        targets.insert(session.clone());
    }

    let mut descendants: std::collections::HashSet<SessionId> = std::collections::HashSet::new();
    // Every target must settle, not just the first — a batched `approve`
    // whose `decisions` route to two different sessions (root + a child, or
    // two sibling children) must wait for both, or the response would
    // silently omit whichever target settled second.
    let mut settled_targets: std::collections::HashSet<SessionId> =
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
        if targets.contains(&ev_session)
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
            settled_targets.insert(ev_session.clone());
        }
        collected.push(LogRecord::new(ev_session, LogPayload::Out(ev)));
        if settled_targets.len() == targets.len() {
            break;
        }
    }
    Ok(collected)
}
