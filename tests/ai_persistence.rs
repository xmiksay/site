//! DB-dependent tests for `site::ai::persistence` (`DbSink`, `resume_session`,
//! `delete_session_events`) against a real `assistant_events` table. Gated on
//! `DATABASE_URL`; follows `tests/policy_db.rs`'s skip/cleanup convention.
//!
//! `assistant_events.root_session_id` is a free-form string, not an FK to
//! `users` (the engine has no notion of our `users` table) — see
//! `src/entity/assistant_event.rs` — so no throwaway user is needed here, only
//! a unique-per-test session id and an explicit `delete_session_events` at the
//! end of each test.

use std::time::Duration;

use entanglement_core::{AgentState, EngineConfig, Holly, InMsg, OutEvent, SessionId};
use entanglement_runtime::persistence::RecordSink;
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, Set,
};
use site::ai::persistence::{DbSink, delete_session_events, resume_session};
use site::entity::assistant_event;

async fn test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

fn unique_session(tag: &str) -> SessionId {
    SessionId::new(format!("persist-test-{tag}-{}", uuid::Uuid::new_v4()))
}

/// Insert a `LogRecord` straight into `assistant_events`, bypassing `DbSink` —
/// direct and synchronous, so a test can build an exact ordered log (including
/// a deliberately broken one) without racing the sink's async writer task.
async fn insert_record(db: &DatabaseConnection, session: &SessionId, payload: LogPayload) {
    let record = LogRecord::new(session.clone(), payload);
    let value = serde_json::to_value(&record).expect("serialize LogRecord");
    assistant_event::ActiveModel {
        root_session_id: Set(session.0.clone()),
        payload: Set(value),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert assistant_event row");
}

async fn count_rows(db: &DatabaseConnection, session: &SessionId) -> u64 {
    assistant_event::Entity::find()
        .filter(assistant_event::Column::RootSessionId.eq(session.0.clone()))
        .count(db)
        .await
        .expect("count assistant_events")
}

/// Poll until `session` has at least `expected` persisted rows or `timeout`
/// elapses. `DbSink::append` only guarantees the channel accepted the record
/// (module doc) — the actual DB write lands asynchronously on the writer task,
/// so a direct post-`append` read would be racy without this.
async fn wait_for_rows(
    db: &DatabaseConnection,
    session: &SessionId,
    expected: u64,
    timeout: Duration,
) -> u64 {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let count = count_rows(db, session).await;
        if count >= expected || tokio::time::Instant::now() >= deadline {
            return count;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn append_is_eventually_persisted_to_assistant_events() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let session = unique_session("append");
    let sink = DbSink::new(db.clone());
    let record = LogRecord::new(
        session.clone(),
        LogPayload::In(InMsg::prompt(session.clone(), "hello")),
    );

    sink.append(&session, &record)
        .expect("append should accept the record");

    let count = wait_for_rows(&db, &session, 1, Duration::from_secs(5)).await;
    assert_eq!(count, 1, "expected the async writer to persist the record");

    delete_session_events(&db, &session).await.expect("cleanup");
}

/// `RecordSink::append` "must never block" (module doc): a full channel sheds
/// the record with an `Err` instead of awaiting the DB writer. `#[tokio::test]`
/// defaults to a **current-thread** runtime, and every call in the loop below
/// is synchronous (`try_send`, no `.await`) — so the writer task `DbSink::new`
/// spawns is never actually polled until this test hits its first real await.
/// That makes the channel fill deterministically at its fixed capacity instead
/// of racing a real drain, with no sleep-based flakiness.
#[tokio::test]
async fn append_sheds_records_once_the_channel_is_full() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let session = unique_session("backpressure");
    let sink = DbSink::new(db.clone());
    let record = LogRecord::new(
        session.clone(),
        LogPayload::In(InMsg::prompt(session.clone(), "x")),
    );

    let mut accepted: u64 = 0;
    let mut saw_backpressure_err = false;
    for _ in 0..4000 {
        match sink.append(&session, &record) {
            Ok(()) => accepted += 1,
            Err(_) => {
                saw_backpressure_err = true;
                break;
            }
        }
    }
    assert!(
        saw_backpressure_err,
        "expected append to shed a record once the channel filled (accepted {accepted} first)"
    );

    // Best-effort cleanup: give the writer a chance to drain what it did
    // accept before deleting, so this test doesn't leave orphan rows behind in
    // `site_test` (which isn't reset between runs).
    wait_for_rows(&db, &session, accepted, Duration::from_secs(15)).await;
    delete_session_events(&db, &session).await.expect("cleanup");
}

#[tokio::test]
async fn resume_session_round_trips_a_paired_log() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let holly = Holly::spawn(EngineConfig::default());
    let session = unique_session("resume");

    insert_record(
        &db,
        &session,
        LogPayload::In(InMsg::prompt(session.clone(), "hi")),
    )
    .await;
    insert_record(
        &db,
        &session,
        LogPayload::Out(OutEvent::Status {
            session: session.clone(),
            state: AgentState::Idle,
        }),
    )
    .await;

    let resumed = resume_session(&db, &holly, session.clone())
        .await
        .expect("resume_session should succeed on an intact paired log");
    assert_eq!(resumed, session);

    delete_session_events(&db, &session).await.expect("cleanup");
}

#[tokio::test]
async fn resume_session_truncates_at_a_gap_and_still_succeeds() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let holly = Holly::spawn(EngineConfig::default());
    let session = unique_session("gap");

    insert_record(
        &db,
        &session,
        LogPayload::In(InMsg::prompt(session.clone(), "hi")),
    )
    .await;
    // A dropped-broadcast tombstone: as of issue #28, `resume_session` no
    // longer hard-refuses on this — replaying *through* it would silently
    // fold an incomplete history, but the prefix strictly before it is
    // intact, so `resume_session` truncates to that prefix and still
    // succeeds (see `src/ai/persistence.rs`'s `truncate_at_gap`) rather than
    // permanently locking the session out of ever resuming again.
    insert_record(&db, &session, LogPayload::Gap { dropped: 3 }).await;
    insert_record(
        &db,
        &session,
        LogPayload::Out(OutEvent::Status {
            session: session.clone(),
            state: AgentState::Idle,
        }),
    )
    .await;

    let resumed = resume_session(&db, &holly, session.clone())
        .await
        .expect("resume_session should truncate at the gap and still succeed");
    assert_eq!(resumed, session);

    delete_session_events(&db, &session).await.expect("cleanup");
}

#[tokio::test]
async fn delete_session_events_removes_every_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let session = unique_session("delete");

    insert_record(
        &db,
        &session,
        LogPayload::In(InMsg::prompt(session.clone(), "hi")),
    )
    .await;
    insert_record(
        &db,
        &session,
        LogPayload::Out(OutEvent::Status {
            session: session.clone(),
            state: AgentState::Idle,
        }),
    )
    .await;
    assert_eq!(count_rows(&db, &session).await, 2);

    delete_session_events(&db, &session)
        .await
        .expect("delete_session_events");

    assert_eq!(count_rows(&db, &session).await, 0);
}
