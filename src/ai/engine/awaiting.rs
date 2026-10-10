//! [`AwaitingDecisions`] — which `(session, request_id)` tool calls currently
//! have an approval waiter parked in the engine, folded from the broadcast.
//!
//! `entanglement_runtime` registers a call's `PendingDecisions` waiter
//! *before* it emits that call's `ToolRequest`, and an `Approve`/`Reject`
//! that reaches the engine with no waiter registered is a silent no-op — the
//! approving request then sits out the whole `TURN_TIMEOUT`. A resumed session
//! (a fresh process, a hibernated one, a sub-agent cascade) still has its old
//! `ToolRequest` row in `assistant_events`, but its waiter only exists once
//! the resumed session has re-offered the call and the executor has resolved
//! its permission again — asynchronously, after `ensure_live` returned. So
//! seeing the call's `ToolRequest` broadcast *in this process* is the exact
//! readiness signal `approve` waits for (#124); the persisted history is not.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashSet;
use entanglement_core::{OutEvent, SessionId};
use tokio::sync::Notify;

#[derive(Clone, Default)]
pub(super) struct AwaitingDecisions {
    calls: Arc<DashSet<(SessionId, String)>>,
    changed: Arc<Notify>,
}

impl AwaitingDecisions {
    /// Fold one broadcast event. A hibernated or ended session's waiters are
    /// gone with its task; a later resume re-offers (and re-announces) them.
    pub(super) fn fold(&self, ev: &OutEvent) {
        match ev {
            OutEvent::ToolRequest {
                session,
                request_id,
                ..
            } => {
                self.calls.insert((session.clone(), request_id.clone()));
                self.changed.notify_waiters();
            }
            OutEvent::ToolOutput {
                session,
                request_id,
                ..
            } => {
                self.calls.remove(&(session.clone(), request_id.clone()));
            }
            OutEvent::SessionHibernated { session, .. }
            | OutEvent::SessionEnded { session, .. } => {
                self.calls.retain(|(s, _)| s != session);
            }
            _ => {}
        }
    }

    /// Wait until `(session, request_id)` has a parked waiter, up to `limit`.
    /// `false` means it never showed up: the call is already resolved, or was
    /// never re-offered.
    pub(super) async fn wait_for(
        &self,
        session: &SessionId,
        request_id: &str,
        limit: Duration,
    ) -> bool {
        let key = (session.clone(), request_id.to_string());
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            // Armed before the check, so a `ToolRequest` folded in between
            // still wakes this waiter.
            let changed = self.changed.notified();
            if self.calls.contains(&key) {
                return true;
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return self.calls.contains(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(session: &SessionId, id: &str) -> OutEvent {
        OutEvent::ToolRequest {
            session: session.clone(),
            seq: 1,
            request_id: id.into(),
            tool: "tag_create".into(),
            input: "{}".into(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_wakes_on_a_late_tool_request() {
        let awaiting = AwaitingDecisions::default();
        let session = SessionId::new_uuid();
        let late = awaiting.clone();
        let offered = session.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            late.fold(&request(&offered, "call-1"));
        });
        assert!(
            awaiting
                .wait_for(&session, "call-1", Duration::from_secs(30))
                .await
        );
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_gives_up_on_a_call_never_offered() {
        let awaiting = AwaitingDecisions::default();
        let session = SessionId::new_uuid();
        awaiting.fold(&request(&session, "other"));
        assert!(
            !awaiting
                .wait_for(&session, "call-1", Duration::from_secs(30))
                .await
        );
    }

    #[tokio::test(start_paused = true)]
    async fn hibernation_forgets_a_sessions_parked_calls() {
        let awaiting = AwaitingDecisions::default();
        let session = SessionId::new_uuid();
        awaiting.fold(&request(&session, "call-1"));
        awaiting.fold(&OutEvent::SessionHibernated {
            session: session.clone(),
            ts: 0,
        });
        assert!(
            !awaiting
                .wait_for(&session, "call-1", Duration::from_secs(1))
                .await
        );
    }
}
