//! Bridges the entanglement engine's live event stream to the global WS hub
//! (`assistant.*` topic) — issue #16's connection to the #15 engine swap.
//!
//! A single process-wide task subscribes to `agent_engine.holly.subscribe()`
//! (the same broadcast channel `sessions/turn.rs` and `engine.rs`'s
//! hibernate-watcher already tap) and forwards every content/lifecycle
//! `OutEvent` — real token-level `TextDelta`/`ReasoningDelta`/`ToolCallDelta`
//! included — to whichever user owns that event's `SessionId`. This is
//! genuine streaming, not turn-boundary polling: a second tab sees the same
//! deltas the tab that sent the prompt is receiving over its own (awaited)
//! REST response.
//!
//! `OutEvent` already derives `Serialize` with `#[serde(tag = "kind", ...)]`
//! (`entanglement_core::protocol`), so the envelope's `payload` is just the
//! event's own wire shape — no hand-written per-variant payload structs to
//! keep in sync with the engine crate. The envelope's `event` name is that
//! same `"kind"` tag (e.g. `"text_delta"`, `"tool_request"`, `"done"`).
//!
//! The engine only knows `SessionId` (`u{user_id}:{uuid}`); the client only
//! knows the DB `assistant_sessions.id` it got from REST. Every forwarded
//! event gets a `db_session_id` field spliced into its payload, resolved via
//! a small process-lifetime cache (`engine_session_id` is set once at session
//! creation and never changes, so nothing needs to invalidate the cache
//! short of the process restarting).
//!
//! ## Sub-agent (#17) events
//!
//! A `researcher`/`page-writer` child's own events carry the *child's*
//! `SessionId` (a bare uuid, not a DB `assistant_sessions` row of its own —
//! only the root session is ever persisted there). `forward` resolves
//! `db_session_id` off the event's **root** ancestor instead, so a child's
//! deltas land in the same WS stream as its parent turn, with the child's own
//! id spliced in as `agent_session_id` so the client can tell them apart and
//! nest them. Root-level events get no `agent_session_id` at all (rather than
//! one equal to their own session), keeping the envelope shape byte-identical
//! to before #17 for every session that never spawns a sub-agent.
//!
//! Root resolution is **not** simply `engine::root_session_of` — that reads a
//! process-global cache (`engine.rs`'s `SESSION_PARENTS`) written by a
//! *different* task (`engine.rs`'s own session watcher), which independently
//! subscribes to this same broadcast. Two independent subscribers racing over
//! one shared cache means this task could process a child's own
//! `SessionStarted` — the very event that names its parent — before the other
//! task has recorded the link, silently dropping it (`user_id_from_session`
//! fails the unprefixed uuid, `forward` returns early). `local_parents`
//! (below) sidesteps the race entirely: it's fed from the exact same ordered
//! stream this task is already walking, so a child's parent link is always
//! recorded *before* this function ever needs to resolve its root — no
//! dependency on the other task's timing at all. It only falls through to the
//! global cache for a child whose `SessionStarted` predates this
//! subscription (a session already established when the process started).

use dashmap::DashMap;
use entanglement_core::{OutEvent, SessionId};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::ai::engine::{SiteEngine, root_session_of, user_id_from_session};
use crate::entity::assistant_session;
use crate::routes::ws::{Envelope, Topic, WsHub};

/// Spawn the forwarding task. Fire-and-forget: it runs for the process
/// lifetime, same as `SiteEngine`'s own internal tasks (`engine.rs` holds
/// those `JoinHandle`s only to document intent, not to ever abort them).
pub fn spawn(engine: Arc<SiteEngine>, hub: Arc<WsHub>, db: DatabaseConnection) {
    let mut sub = engine.holly.subscribe();
    let session_db_ids: Arc<DashMap<String, i32>> = Arc::new(DashMap::new());
    tokio::spawn(async move {
        // Single-owner (only this loop ever touches it), so a plain HashMap
        // — not a DashMap — is enough; see the module doc for why this beats
        // trusting `engine::root_session_of`'s cache alone.
        let mut local_parents: HashMap<SessionId, SessionId> = HashMap::new();
        loop {
            match sub.recv().await {
                Ok(ev) => forward(&hub, &db, &session_db_ids, &mut local_parents, ev).await,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Content/lifecycle events worth showing a head — mirrors the issue's list
/// (session state, deltas, approval prompts, tool display, done/error,
/// hibernated) plus, for #17, a **child** session's own `SessionStarted` (a
/// root's is never forwarded — nothing for the client to nest it under, and
/// `AssistantView.vue` already knows about its own root session from REST),
/// and, for #42, `ModelChanged`/`GenerationChanged`/`AgentChanged` — a live
/// `/model`/generation/profile switch from *another* tab must be visible
/// without a manual reload. Everything else (`SessionList`, `History`,
/// `Plan`, `TaskList`, `Usage`, `Compacted`, `UserQuestion`, file-change
/// records, `SessionEnded`) has no consumer in `AssistantView.vue` today; add
/// a case here (and a client handler) when one needs it rather than
/// forwarding everything speculatively.
fn is_forwarded(ev: &OutEvent) -> bool {
    match ev {
        OutEvent::Status { .. }
        | OutEvent::TextDelta { .. }
        | OutEvent::ReasoningDelta { .. }
        | OutEvent::ToolCallDelta { .. }
        | OutEvent::ToolCall { .. }
        | OutEvent::ToolRequest { .. }
        | OutEvent::ToolOutput { .. }
        | OutEvent::Done { .. }
        | OutEvent::Error { .. }
        | OutEvent::SessionHibernated { .. }
        | OutEvent::ModelChanged { .. }
        | OutEvent::GenerationChanged { .. }
        | OutEvent::AgentChanged { .. } => true,
        OutEvent::SessionStarted { parent, .. } => parent.is_some(),
        _ => false,
    }
}

async fn forward(
    hub: &WsHub,
    db: &DatabaseConnection,
    session_db_ids: &DashMap<String, i32>,
    local_parents: &mut HashMap<SessionId, SessionId>,
    ev: OutEvent,
) {
    // Record the parent link *before* anything below needs to resolve a
    // root off it — including this very event, if it's the child's own
    // `SessionStarted` (see the module doc).
    if let OutEvent::SessionStarted {
        session,
        parent: Some(parent),
        ..
    } = &ev
    {
        local_parents.insert(session.clone(), parent.clone());
    }

    if !is_forwarded(&ev) {
        return;
    }
    let Some(session) = ev.session() else {
        return;
    };
    let root = local_root_of(local_parents, session);
    // Resolve the user off the already-locally-resolved `root`, not `session`
    // — `user_id_from_session` would otherwise re-derive the root itself via
    // the very global cache this function exists to not depend on.
    let Ok(user_id) = user_id_from_session(&root) else {
        return;
    };
    let Some(db_session_id) = resolve_db_session_id(db, session_db_ids, &root).await else {
        return;
    };

    let Ok(mut payload) = serde_json::to_value(&ev) else {
        tracing::error!(session = %session.0, "failed to serialize OutEvent for ws forward");
        return;
    };
    let event = payload
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("db_session_id".into(), serde_json::json!(db_session_id));
        // Only present for a sub-agent's own events — a root event carries no
        // `agent_session_id`, keeping every pre-#17 consumer's shape intact.
        if session != &root {
            obj.insert("agent_session_id".into(), serde_json::json!(session.0));
        }
    }

    hub.publish(
        user_id,
        Envelope {
            topic: Topic::Assistant,
            event,
            payload,
        },
    );
}

/// Walk `local_parents` (this task's own race-free view, fed synchronously
/// from the same ordered broadcast — see the module doc) up to `session`'s
/// root ancestor, falling back to `engine::root_session_of`'s global cache
/// only past where the local map runs out (a session whose `SessionStarted`
/// predates this subscription). Cycle-guarded like every other ancestor walk
/// in this codebase, in case a malformed/duplicated event ever links a
/// session to itself transitively.
fn local_root_of(local_parents: &HashMap<SessionId, SessionId>, session: &SessionId) -> SessionId {
    let mut current = session.clone();
    let mut visited = HashSet::new();
    while visited.insert(current.clone()) {
        match local_parents.get(&current) {
            Some(parent) => current = parent.clone(),
            None => break,
        }
    }
    root_session_of(&current)
}

/// Resolve an engine `SessionId` to its owning `assistant_sessions.id`,
/// caching hits — every event within a turn (potentially dozens of deltas)
/// would otherwise round-trip the DB per event.
async fn resolve_db_session_id(
    db: &DatabaseConnection,
    cache: &DashMap<String, i32>,
    session: &SessionId,
) -> Option<i32> {
    if let Some(id) = cache.get(&session.0) {
        return Some(*id);
    }
    let row = assistant_session::Entity::find()
        .filter(assistant_session::Column::EngineSessionId.eq(session.0.clone()))
        .one(db)
        .await
        .inspect_err(|e| {
            tracing::error!(error = %e, session = %session.0, "failed to resolve db session id for ws forward")
        })
        .ok()??;
    cache.insert(session.0.clone(), row.id);
    Some(row.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_the_events_the_ui_actually_renders() {
        let session = SiteEngine::session_id_for_user(1);
        assert!(is_forwarded(&OutEvent::TextDelta {
            session: session.clone(),
            seq: 1,
            text: "hi".into(),
        }));
        assert!(is_forwarded(&OutEvent::Done {
            session: session.clone(),
            seq: 2,
        }));
        assert!(is_forwarded(&OutEvent::SessionHibernated {
            session: session.clone(),
            ts: 0,
        }));
    }

    #[test]
    fn does_not_forward_events_with_no_ui_consumer() {
        let session = SiteEngine::session_id_for_user(1);
        assert!(!is_forwarded(&OutEvent::SessionEnded {
            session: session.clone(),
            ts: 0,
        }));
        assert!(!is_forwarded(&OutEvent::Usage {
            session,
            seq: 1,
            input_tokens: 0,
            output_tokens: 0,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: None,
        }));
    }

    /// #42: a live `/model`/generation/profile switch from another tab must
    /// reach the client, or `AssistantSessionToolbar.vue`'s picker goes stale.
    #[test]
    fn forwards_model_generation_and_agent_changes() {
        let session = SiteEngine::session_id_for_user(1);
        assert!(is_forwarded(&OutEvent::ModelChanged {
            session: session.clone(),
            provider: "anthropic".into(),
            model: "claude".into(),
            context_window: None,
        }));
        assert!(is_forwarded(&OutEvent::GenerationChanged {
            session: session.clone(),
            generation: entanglement_core::GenerationParams::default(),
        }));
        assert!(is_forwarded(&OutEvent::AgentChanged {
            session,
            agent: "researcher".into(),
            profile_detail: None,
        }));
    }

    /// The race the module doc describes: `local_root_of` must resolve a
    /// child's root from `local_parents` alone — no dependency on
    /// `engine::root_session_of`'s separately-populated global cache — since
    /// this is exactly what lets `forward` resolve a child's own
    /// `SessionStarted` correctly even if `engine.rs`'s watcher task hasn't
    /// processed that same event yet.
    #[test]
    fn local_root_of_resolves_without_the_global_cache() {
        let root = SiteEngine::session_id_for_user(4242);
        let child = SessionId::new_uuid();
        let mut local_parents = HashMap::new();
        local_parents.insert(child.clone(), root.clone());

        assert_eq!(local_root_of(&local_parents, &child), root);
        assert_eq!(local_root_of(&local_parents, &root), root);
    }

    #[test]
    fn forwards_a_sub_agent_started_but_not_a_root_started() {
        let root = SiteEngine::session_id_for_user(1);
        let child = SessionId::new_uuid();
        assert!(
            !is_forwarded(&OutEvent::SessionStarted {
                session: root.clone(),
                parent: None,
                predecessor: None,
                profile: "build".into(),
                model: None,
                root: true,
                ts: 0,
            }),
            "a root's own SessionStarted has nothing for the client to nest it under"
        );
        assert!(
            is_forwarded(&OutEvent::SessionStarted {
                session: child,
                parent: Some(root),
                predecessor: None,
                profile: "researcher".into(),
                model: None,
                root: false,
                ts: 0,
            }),
            "a sub-agent's SessionStarted is how the client learns to render its nested block"
        );
    }
}
