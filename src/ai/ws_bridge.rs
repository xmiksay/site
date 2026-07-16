//! Bridges the assistant turn loop to the global WS hub (`assistant.*` topic).
//!
//! The engine swap (#15 — entanglement `Holly`, streaming `OutEvent`s) hasn't
//! landed yet: `loop_driver::run_turn` still runs synchronously to completion
//! inside the HTTP request/response cycle, so there is no token-level delta
//! stream to forward. This module instead publishes turn-boundary events
//! (started / completed / error) so every tab a user has open stays in sync
//! with the tab that issued the prompt/approve call. Once #15 lands and
//! exposes `Holly::subscribe()`, this is where that subscription replaces
//! these call sites with real `TextDelta`/`ReasoningDelta`/`ToolCallDelta`
//! events.

use serde_json::{Value, json};

use crate::routes::ws::{Topic, WsHub};

/// A turn is about to run (prompt sent or tool calls approved).
pub fn turn_started(hub: &WsHub, user_id: i32, session_id: i32) {
    hub.publish(
        user_id,
        crate::routes::ws::Envelope {
            topic: Topic::Assistant,
            event: "turn_started".into(),
            payload: json!({ "session_id": session_id }),
        },
    );
}

/// A turn finished (paused for approval, or ran to completion). `detail`
/// is the same JSON shape returned by the REST session endpoints
/// (`SessionDetail`), so listening tabs can apply it directly.
pub fn turn_completed(hub: &WsHub, user_id: i32, detail: Value) {
    hub.publish(
        user_id,
        crate::routes::ws::Envelope {
            topic: Topic::Assistant,
            event: "turn_completed".into(),
            payload: detail,
        },
    );
}

/// The turn loop returned an error (already recorded as an `error` message).
pub fn turn_error(hub: &WsHub, user_id: i32, session_id: i32, message: &str) {
    hub.publish(
        user_id,
        crate::routes::ws::Envelope {
            topic: Topic::Assistant,
            event: "error".into(),
            payload: json!({ "session_id": session_id, "message": message }),
        },
    );
}
