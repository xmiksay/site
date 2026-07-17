//! Site-wide authenticated WebSocket hub.
//!
//! One long-lived connection per browser tab, registered per `user_id`.
//! `pages.*` / `files.*` / `galleries.*` / `tags.*` change events broadcast
//! to every connected user (shared content), published uniformly from
//! `src/routes/broadcast.rs` regardless of which edge (REST API, MCP, or the
//! AI assistant) performed the mutation. `assistant.*` events publish only to
//! the owning user's connections.
//!
//! `assistant.*` carries real token-level streaming: `src/ai/ws_bridge.rs`
//! subscribes to the entanglement engine's `Holly::subscribe()` broadcast
//! (#15) and forwards each content/lifecycle `OutEvent` (`text_delta` /
//! `reasoning_delta` / `tool_call*` / `status` / `done` / `error` / …) to the
//! owning user's connections, so every tab streams the same deltas as the tab
//! that issued the prompt.

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, State};
use axum::response::Response;
use axum::routing::get;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::state::AppState;

const PING_INTERVAL: Duration = Duration::from_secs(30);
const CHANNEL_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Assistant,
    Pages,
    Files,
    Galleries,
    Tags,
}

#[derive(Clone, Debug, Serialize)]
pub struct Envelope {
    pub topic: Topic,
    pub event: String,
    pub payload: Value,
}

/// Per-user registry of live WebSocket senders.
pub struct WsHub {
    conns: DashMap<i32, Vec<mpsc::Sender<Envelope>>>,
}

impl Default for WsHub {
    fn default() -> Self {
        Self::new()
    }
}

impl WsHub {
    pub fn new() -> Self {
        Self {
            conns: DashMap::new(),
        }
    }

    /// Register a new connection for `user_id`, returning the sender handed
    /// to `publish`/`broadcast` and the receiver the socket task drains.
    pub fn register(&self, user_id: i32) -> (mpsc::Sender<Envelope>, mpsc::Receiver<Envelope>) {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        self.conns.entry(user_id).or_default().push(tx.clone());
        (tx, rx)
    }

    /// Remove a specific connection (identified by its sender) for `user_id`.
    pub fn unregister(&self, user_id: i32, tx: &mpsc::Sender<Envelope>) {
        if let Some(mut senders) = self.conns.get_mut(&user_id) {
            senders.retain(|s| !s.same_channel(tx));
            if senders.is_empty() {
                drop(senders);
                self.conns.remove(&user_id);
            }
        }
    }

    /// Send an envelope to every connection owned by `user_id`. Connections
    /// whose receiver has been dropped are pruned; a momentarily full channel
    /// just drops this message rather than tearing down the connection.
    pub fn publish(&self, user_id: i32, envelope: Envelope) {
        let Some(mut senders) = self.conns.get_mut(&user_id) else {
            return;
        };
        senders.retain(|tx| {
            !matches!(
                tx.try_send(envelope.clone()),
                Err(mpsc::error::TrySendError::Closed(_))
            )
        });
        if senders.is_empty() {
            drop(senders);
            self.conns.remove(&user_id);
        }
    }

    /// Send an envelope to every connected user (shared content changes).
    pub fn broadcast(&self, envelope: Envelope) {
        self.conns.retain(|_, senders| {
            senders.retain(|tx| {
                !matches!(
                    tx.try_send(envelope.clone()),
                    Err(mpsc::error::TrySendError::Closed(_))
                )
            });
            !senders.is_empty()
        });
    }

    /// Convenience wrapper building the envelope for `broadcast`.
    pub fn broadcast_event(&self, topic: Topic, event: impl Into<String>, payload: Value) {
        self.broadcast(Envelope {
            topic,
            event: event.into(),
            payload,
        });
    }

    /// Serialize `value` and broadcast it; a serialization failure (never
    /// expected for the plain summary structs this is called with) just
    /// drops the notification instead of failing the CRUD request.
    pub fn broadcast_serialized<T: Serialize>(
        &self,
        topic: Topic,
        event: impl Into<String>,
        value: &T,
    ) {
        match serde_json::to_value(value) {
            Ok(payload) => self.broadcast_event(topic, event, payload),
            Err(e) => tracing::error!(error = %e, "failed to serialize ws broadcast payload"),
        }
    }

    #[cfg(test)]
    fn connection_count(&self, user_id: i32) -> usize {
        self.conns.get(&user_id).map(|v| v.len()).unwrap_or(0)
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(upgrade))
}

async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state, user_id))
}

async fn handle_socket(socket: WebSocket, state: AppState, user_id: i32) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = state.ws_hub.register(user_id);

    let mut recv_task = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    ping_interval.tick().await; // first tick fires immediately; skip it

    loop {
        tokio::select! {
            biased;
            _ = &mut recv_task => break,
            maybe_envelope = rx.recv() => {
                match maybe_envelope {
                    Some(envelope) => {
                        let Ok(text) = serde_json::to_string(&envelope) else { continue };
                        if ws_sender.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            _ = ping_interval.tick() => {
                if ws_sender.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }

    recv_task.abort();
    state.ws_hub.unregister(user_id, &tx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn envelope(event: &str) -> Envelope {
        Envelope {
            topic: Topic::Pages,
            event: event.into(),
            payload: json!({ "id": 1 }),
        }
    }

    #[tokio::test]
    async fn register_and_unregister_removes_user_entry() {
        let hub = WsHub::new();
        let (tx, _rx) = hub.register(1);
        assert_eq!(hub.connection_count(1), 1);
        hub.unregister(1, &tx);
        assert_eq!(hub.connection_count(1), 0);
    }

    #[tokio::test]
    async fn publish_fans_out_to_all_of_a_users_connections() {
        let hub = WsHub::new();
        let (_tx1, mut rx1) = hub.register(1);
        let (_tx2, mut rx2) = hub.register(1);

        hub.publish(1, envelope("created"));

        let e1 = rx1.recv().await.expect("rx1 should receive");
        let e2 = rx2.recv().await.expect("rx2 should receive");
        assert_eq!(e1.event, "created");
        assert_eq!(e2.event, "created");
    }

    #[tokio::test]
    async fn publish_does_not_leak_across_users() {
        let hub = WsHub::new();
        let (_tx_a, mut rx_a) = hub.register(1);
        let (_tx_b, mut rx_b) = hub.register(2);

        hub.publish(1, envelope("created"));

        assert!(rx_a.try_recv().is_ok());
        assert!(rx_b.try_recv().is_err());
    }

    #[tokio::test]
    async fn publish_prunes_dropped_receivers() {
        let hub = WsHub::new();
        let (_tx, rx) = hub.register(1);
        drop(rx);

        hub.publish(1, envelope("created"));

        assert_eq!(hub.connection_count(1), 0);
    }

    #[tokio::test]
    async fn broadcast_reaches_every_connected_user() {
        let hub = WsHub::new();
        let (_tx_a, mut rx_a) = hub.register(1);
        let (_tx_b, mut rx_b) = hub.register(2);

        hub.broadcast_event(Topic::Pages, "updated", json!({ "id": 7 }));

        let a = rx_a.recv().await.expect("rx_a should receive");
        let b = rx_b.recv().await.expect("rx_b should receive");
        assert_eq!(a.event, "updated");
        assert_eq!(b.event, "updated");
        assert_eq!(a.payload, json!({ "id": 7 }));
    }

    #[tokio::test]
    async fn broadcast_prunes_dropped_receivers() {
        let hub = WsHub::new();
        let (_tx, rx) = hub.register(1);
        drop(rx);

        hub.broadcast_event(Topic::Pages, "updated", json!({ "id": 7 }));

        assert_eq!(hub.connection_count(1), 0);
    }
}
