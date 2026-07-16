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

use dashmap::DashMap;
use entanglement_core::{OutEvent, SessionId};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::ai::engine::{SiteEngine, user_id_from_session};
use crate::entity::assistant_session;
use crate::routes::ws::{Envelope, Topic, WsHub};

/// Spawn the forwarding task. Fire-and-forget: it runs for the process
/// lifetime, same as `SiteEngine`'s own internal tasks (`engine.rs` holds
/// those `JoinHandle`s only to document intent, not to ever abort them).
pub fn spawn(engine: Arc<SiteEngine>, hub: Arc<WsHub>, db: DatabaseConnection) {
    let mut sub = engine.holly.subscribe();
    let session_db_ids: Arc<DashMap<String, i32>> = Arc::new(DashMap::new());
    tokio::spawn(async move {
        loop {
            match sub.recv().await {
                Ok(ev) => forward(&hub, &db, &session_db_ids, ev).await,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Content/lifecycle events worth showing a head — mirrors the issue's list
/// (session state, deltas, approval prompts, tool display, done/error,
/// hibernated). Everything else (`SessionStarted`, `SessionList`, `History`,
/// `AgentChanged`, `ModelChanged`, `Plan`, `TaskList`, `Usage`, `Compacted`,
/// `UserQuestion`, file-change records, `SessionEnded`) has no consumer in
/// `AssistantView.vue` today; add a case here (and a client handler) when one
/// needs it rather than forwarding everything speculatively.
fn is_forwarded(ev: &OutEvent) -> bool {
    matches!(
        ev,
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
    )
}

async fn forward(
    hub: &WsHub,
    db: &DatabaseConnection,
    session_db_ids: &DashMap<String, i32>,
    ev: OutEvent,
) {
    if !is_forwarded(&ev) {
        return;
    }
    let Some(session) = ev.session() else {
        return;
    };
    let Ok(user_id) = user_id_from_session(session) else {
        return;
    };
    let Some(db_session_id) = resolve_db_session_id(db, session_db_ids, session).await else {
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
}
