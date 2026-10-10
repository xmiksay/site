//! Unit tests for `src/ai/persistence.rs`, split out to keep that file
//! under the 400-line cap.

use super::*;
use entanglement_core::{InMsg, OutEvent};

fn out_record(session: &SessionId, seq: u64) -> LogRecord {
    LogRecord::new(
        session.clone(),
        LogPayload::Out(OutEvent::TextDelta {
            session: session.clone(),
            seq,
            text: format!("chunk-{seq}"),
        }),
    )
}

fn in_record(session: &SessionId, text: &str) -> LogRecord {
    LogRecord::new(
        session.clone(),
        LogPayload::In(InMsg::prompt(session.clone(), text)),
    )
}

fn gap_record(session: &SessionId, dropped: u64) -> LogRecord {
    LogRecord::new(session.clone(), LogPayload::Gap { dropped })
}

#[test]
fn truncate_at_gap_is_a_no_op_on_an_intact_log() {
    let session = SessionId::new("root");
    let mut records = vec![
        in_record(&session, "hi"),
        out_record(&session, 1),
        out_record(&session, 2),
    ];
    let before = records.len();

    assert!(truncate_at_gap(&mut records).is_none());
    assert_eq!(records.len(), before);
}

#[test]
fn truncate_at_gap_keeps_only_the_prefix_before_the_first_gap() {
    let session = SessionId::new("root");
    let mut records = vec![
        in_record(&session, "hi"),
        out_record(&session, 1),
        gap_record(&session, 3),
        // These would have replayed over the gap into a wrong `Context`.
        out_record(&session, 2),
        out_record(&session, 3),
    ];

    let (dropped, discarded) = truncate_at_gap(&mut records).expect("gap detected");

    assert_eq!(dropped, 3);
    assert_eq!(
        discarded, 3,
        "the gap tombstone itself plus the 2 trailing records"
    );
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|r| !matches!(r.payload, LogPayload::Gap { .. })),
        "kept prefix must not include the tombstone"
    );
}

#[test]
fn truncate_at_gap_sums_multiple_tombstones_but_still_only_keeps_the_first_prefix() {
    let session = SessionId::new("root");
    let mut records = vec![
        in_record(&session, "hi"),
        gap_record(&session, 2),
        gap_record(&session, 5),
    ];

    let (dropped, discarded) = truncate_at_gap(&mut records).expect("gap detected");

    assert_eq!(dropped, 7, "both tombstones' counts are summed");
    assert_eq!(discarded, 2);
    assert_eq!(records.len(), 1);
}

/// Every `LogRecord` shape this site actually persists must survive the
/// `serde_json::to_value`/`from_value` pair `append`/`resume_session` use
/// against `assistant_events.payload` — losing a field there is silent,
/// and `resume_session` hard-errors on a row it can't read, taking the
/// whole session down with it. #97 (the 0.4 → 0.6 bump) purged the table
/// rather than shimming the old shapes; this test is what makes the *next*
/// bump's blast radius visible before it reaches a database.
///
/// Re-serializing the round-tripped record and comparing JSON (rather than
/// matching each variant field by field) is deliberate: a variant that
/// grows a field upstream is covered without touching this test, and a
/// field that silently stops serializing fails it.
#[test]
fn every_persisted_log_record_shape_round_trips_through_json() {
    let session = SessionId::new("u1:11111111-1111-4111-8111-111111111111");
    let child = SessionId::new("22222222-2222-4222-8222-222222222222");
    let out = |ev: OutEvent| LogRecord::new(session.clone(), LogPayload::Out(ev));

    let records = vec![
        in_record(&session, "hi"),
        LogRecord::new(
            session.clone(),
            LogPayload::In(InMsg::Approve {
                session: session.clone(),
                request_id: "call-1".into(),
                scope: entanglement_core::ApprovalScope::Always,
            }),
        ),
        LogRecord::new(
            session.clone(),
            LogPayload::In(InMsg::Reject {
                session: session.clone(),
                request_id: "call-2".into(),
                reason: Some("no".into()),
            }),
        ),
        out(OutEvent::SessionStarted {
            session: child.clone(),
            parent: Some(session.clone()),
            predecessor: None,
            profile: "researcher".into(),
            model: Some("claude".into()),
            root: false,
            ts: 1,
            user: None,
        }),
        out(OutEvent::TextDelta {
            session: session.clone(),
            seq: 1,
            text: "chunk".into(),
        }),
        out(OutEvent::ReasoningDelta {
            session: session.clone(),
            seq: 2,
            text: "thinking".into(),
        }),
        out(OutEvent::ToolCall {
            session: session.clone(),
            seq: 3,
            request_id: "call-1".into(),
            tool: "page_edit".into(),
            input: "{}".into(),
        }),
        out(OutEvent::ToolRequest {
            session: session.clone(),
            seq: 4,
            request_id: "call-1".into(),
            tool: "page_edit".into(),
            input: "{}".into(),
        }),
        out(OutEvent::ToolOutput {
            session: session.clone(),
            seq: 5,
            request_id: "call-1".into(),
            tool: "page_edit".into(),
            output: "ok".into(),
            content: Vec::new(),
        }),
        out(OutEvent::Compacted {
            session: session.clone(),
            seq: 6,
            summary: "so far".into(),
            kept: 2,
            auto: false,
        }),
        out(OutEvent::AmbiguousRetry {
            session: session.clone(),
            seq: 7,
            nudge: "continue".into(),
        }),
        out(OutEvent::Error {
            session: session.clone(),
            seq: 8,
            message: "boom".into(),
        }),
        out(OutEvent::Done {
            session: session.clone(),
            seq: 9,
        }),
        gap_record(&session, 3),
    ];

    for record in records {
        let value = serde_json::to_value(&record).expect("serializing LogRecord");
        let round_tripped: LogRecord = serde_json::from_value(value.clone())
            .unwrap_or_else(|e| panic!("deserializing {value}: {e}"));
        let again = serde_json::to_value(&round_tripped).expect("re-serializing LogRecord");
        assert_eq!(value, again, "a field was lost round-tripping {value}");
    }
}

/// `OutEvent::AmbiguousRetry` (ADR-0118) round-trips with its fields
/// intact, not merely with a stable JSON shape — the field-level companion
/// to the blanket test above, kept from #88.
#[test]
fn ambiguous_retry_out_event_round_trips_through_json() {
    let session = SessionId::new("root");
    let record = LogRecord::new(
        session.clone(),
        LogPayload::Out(OutEvent::AmbiguousRetry {
            session: session.clone(),
            seq: 7,
            nudge: "please continue or call a tool".into(),
        }),
    );

    let value = serde_json::to_value(&record).expect("serializing LogRecord");
    let round_tripped: LogRecord = serde_json::from_value(value).expect("deserializing LogRecord");

    match round_tripped.payload {
        LogPayload::Out(OutEvent::AmbiguousRetry {
            session: got_session,
            seq,
            nudge,
        }) => {
            assert_eq!(got_session, session);
            assert_eq!(seq, 7);
            assert_eq!(nudge, "please continue or call a tool");
        }
        other => panic!("expected AmbiguousRetry, got {other:?}"),
    }
}

#[test]
fn a_dropped_append_is_tallied_per_root_and_flushed_as_one_gap_tombstone() {
    let dropped: HashMap<SessionId, u64> = HashMap::new();
    let dropped = Mutex::new(dropped);
    let root_a = SessionId::new("root-a");
    let root_b = SessionId::new("root-b");

    // Simulate what `DbSink::append` does inline on a full channel: tally
    // the drop without touching the DB.
    for root in [&root_a, &root_a, &root_b] {
        *dropped.lock().unwrap().entry(root.clone()).or_insert(0) += 1;
    }

    let pending: HashMap<SessionId, u64> = dropped.lock().unwrap().drain().collect();
    assert_eq!(pending.get(&root_a), Some(&2));
    assert_eq!(pending.get(&root_b), Some(&1));
    assert!(
        dropped.lock().unwrap().is_empty(),
        "drain must clear the tally so it isn't double-flushed"
    );
}

fn started(session: &SessionId, parent: Option<&SessionId>) -> LogRecord {
    LogRecord::new(
        session.clone(),
        LogPayload::Out(OutEvent::SessionStarted {
            session: session.clone(),
            parent: parent.cloned(),
            predecessor: None,
            profile: "build".into(),
            model: None,
            user: None,
            root: parent.is_none(),
            ts: 0,
        }),
    )
}

fn hibernated(session: &SessionId) -> LogRecord {
    LogRecord::new(
        session.clone(),
        LogPayload::Out(OutEvent::SessionHibernated {
            session: session.clone(),
            ts: 0,
        }),
    )
}

#[test]
fn live_tree_orders_parents_before_children_and_skips_settled_ones() {
    let root = SessionId::new("root");
    let child = SessionId::new("child");
    let grandchild = SessionId::new("grandchild");
    let gone = SessionId::new("gone");
    let records = vec![
        started(&root, None),
        started(&child, Some(&root)),
        started(&grandchild, Some(&child)),
        started(&gone, Some(&root)),
        hibernated(&gone),
        out_record(&child, 1),
    ];
    assert_eq!(live_tree(&records, &root), vec![root, child, grandchild]);
}

#[test]
fn live_tree_revives_a_child_started_again_after_hibernating() {
    let root = SessionId::new("root");
    let child = SessionId::new("child");
    let records = vec![
        started(&root, None),
        started(&child, Some(&root)),
        hibernated(&child),
        started(&child, Some(&root)),
    ];
    assert_eq!(live_tree(&records, &root), vec![root, child]);
}
