//! Unit tests for [`super::project`], split by scenario: single-session turn
//! shapes live in `basic.rs`, sub-agent (#17) nesting scenarios in
//! `subagent.rs`. Shared record-building helpers live here.

use super::*;
use entanglement_core::SessionId;
use entanglement_provider::ContentPart;

mod basic;
mod subagent;

fn rec(session: &SessionId, payload: LogPayload) -> LogRecord {
    LogRecord {
        ts: 0,
        session: session.clone(),
        payload,
    }
}

fn out(ev: OutEvent) -> LogPayload {
    LogPayload::Out(ev)
}
fn inm(msg: InMsg) -> LogPayload {
    LogPayload::In(msg)
}
