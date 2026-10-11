//! Unit tests for `routing.rs`, split out to keep it under the 400-line cap.

use super::*;

fn request(session: &SessionId, id: &str) -> LogRecord {
    LogRecord::new(
        session.clone(),
        LogPayload::Out(OutEvent::ToolRequest {
            session: session.clone(),
            seq: 1,
            request_id: id.into(),
            tool: "tag_create".into(),
            input: "{}".into(),
        }),
    )
}

fn output(session: &SessionId, id: &str) -> LogRecord {
    LogRecord::new(
        session.clone(),
        LogPayload::Out(OutEvent::ToolOutput {
            session: session.clone(),
            seq: 2,
            request_id: id.into(),
            tool: "tag_create".into(),
            output: "ok".into(),
            content: Vec::new(),
        }),
    )
}

#[test]
fn call_resolved_only_once_its_latest_request_has_an_output() {
    let s = SessionId::new("s");
    let other = SessionId::new("other");
    assert!(!call_resolved(&[request(&s, "a")], &s, "a"));
    assert!(call_resolved(&[request(&s, "a"), output(&s, "a")], &s, "a"));
    // Same id answered on another session, or reused by a later round.
    assert!(!call_resolved(
        &[request(&s, "a"), output(&other, "a")],
        &s,
        "a"
    ));
    assert!(!call_resolved(
        &[request(&s, "a"), output(&s, "a"), request(&s, "a")],
        &s,
        "a"
    ));
    assert!(!call_resolved(&[], &s, "a"));
}

/// Gemini's ids repeat per round (`tag_create#0`); round two re-asks
/// before its `ToolRequest` row lands.
#[test]
fn a_reused_id_parked_again_is_not_refused_on_a_stale_snapshot() {
    let s = SessionId::new("s");
    let stale = [request(&s, "tag_create#0"), output(&s, "tag_create#0")];
    assert_eq!(
        decision_readiness(true, &stale, &s, "tag_create#0"),
        Readiness::Parked
    );
    assert_eq!(
        decision_readiness(false, &stale, &s, "tag_create#0"),
        Readiness::Resolved
    );
    let fresh = [
        request(&s, "tag_create#0"),
        output(&s, "tag_create#0"),
        request(&s, "tag_create#0"),
    ];
    assert_eq!(
        decision_readiness(false, &fresh, &s, "tag_create#0"),
        Readiness::Unknown
    );
}

#[test]
fn open_tool_requests_follows_the_latest_request_per_session_and_id() {
    let s = SessionId::new("s");
    let other = SessionId::new("other");
    let key = |id: &str| (s.clone(), id.to_string());
    assert_eq!(
        open_tool_requests(&[request(&s, "a"), output(&s, "a"), request(&s, "a")]),
        vec![key("a")]
    );
    assert_eq!(
        open_tool_requests(&[request(&s, "a"), output(&s, "a")]),
        Vec::<(SessionId, String)>::new()
    );
    // An output on another session answers nothing here.
    assert_eq!(
        open_tool_requests(&[request(&s, "a"), output(&other, "a"), request(&s, "b")]),
        vec![key("a"), key("b")]
    );
}

/// `jsonb @>` on objects is recursive key/value containment; mirrored
/// here against the exact serialization `DbSink` stores.
fn contains(stored: &serde_json::Value, probe: &serde_json::Value) -> bool {
    match (stored, probe) {
        (serde_json::Value::Object(s), serde_json::Value::Object(p)) => p
            .iter()
            .all(|(k, v)| s.get(k).is_some_and(|sv| contains(sv, v))),
        _ => stored == probe,
    }
}

#[test]
fn barrier_probe_matches_only_the_output_at_its_own_seq() {
    let s = SessionId::new("s");
    let at = |seq| {
        serde_json::to_value(LogRecord::new(
            s.clone(),
            LogPayload::Out(OutEvent::ToolOutput {
                session: s.clone(),
                seq,
                request_id: "tag_create#0".into(),
                tool: "tag_create".into(),
                output: "ok".into(),
                content: Vec::new(),
            }),
        ))
        .expect("serialize LogRecord")
    };
    let probe = barrier_probe(&s, "tag_create#0", 7);
    assert!(contains(&at(7), &probe));
    // Round one's output under the reused id must not pass the barrier.
    assert!(!contains(&at(3), &probe));
}
