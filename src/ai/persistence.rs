//! `DbSink` — the new engine's `RecordSink`, appending every `LogRecord` to
//! `assistant_events` (m_023), plus the lazy-resume and session-delete
//! helpers built on top of it (embedding.md §3 / §5).
//!
//! Lifecycle: this phase relies on `EngineConfig.idle_ttl` (set in
//! `engine.rs`) to auto-hibernate an idle session rather than an explicit
//! per-turn hibernate call — simpler, and sufficient since nothing is wired
//! to live traffic yet. The next phase's session-delete handler should call
//! [`delete_session_events`] (after telling the engine to close/forget the
//! session) to purge the log.

use anyhow::Context;
use entanglement_core::{Holly, SessionId};
use entanglement_runtime::persistence::RecordSink;
use entanglement_runtime::session_store::{LogRecord, integrity_gap, pair_records};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use tokio::sync::mpsc;

use crate::entity::assistant_event;

/// Backlog cap on the writer task's channel. `RecordSink::append` must never
/// block (persistence.rs doc, embedding.md §3) — a full channel means the DB
/// writer has fallen far behind, so `append` sheds the record with an `Err`
/// rather than stalling the tap and manufacturing a `Gap` tombstone itself.
const SINK_CHANNEL_CAPACITY: usize = 1024;

/// A `RecordSink` that appends to `assistant_events` behind a bounded channel
/// and a dedicated writer task, so `append` (called from the persistence tap,
/// which reads `Holly`'s outbound broadcast) never awaits a DB round-trip.
pub struct DbSink {
    tx: mpsc::Sender<(SessionId, LogRecord)>,
}

impl DbSink {
    pub fn new(db: DatabaseConnection) -> Self {
        let (tx, mut rx) = mpsc::channel::<(SessionId, LogRecord)>(SINK_CHANNEL_CAPACITY);
        tokio::spawn(async move {
            while let Some((root, record)) = rx.recv().await {
                let payload = match serde_json::to_value(&record) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!(error = %e, root = %root.0, "failed to serialize LogRecord");
                        continue;
                    }
                };
                let row = assistant_event::ActiveModel {
                    root_session_id: Set(root.0.clone()),
                    payload: Set(payload),
                    ..Default::default()
                };
                if let Err(e) = row.insert(&db).await {
                    tracing::error!(error = %e, root = %root.0, "failed to persist assistant event");
                }
            }
        });
        DbSink { tx }
    }
}

impl RecordSink for DbSink {
    fn append(&self, root: &SessionId, record: &LogRecord) -> anyhow::Result<()> {
        self.tx
            .try_send((root.clone(), record.clone()))
            .map_err(|_| anyhow::anyhow!("assistant_events sink backlog full, dropping record"))
    }
}

/// Load `root`'s log from `assistant_events`, refuse on a detected gap (a
/// dropped broadcast record — resuming over one would silently fold an
/// incomplete history), and resume it into `holly`. Nothing is loaded until
/// this is actually called (lazy resume, embedding.md §3) — e.g. the next
/// phase's "open session" handler, when the session isn't already live.
pub async fn resume_session(
    db: &DatabaseConnection,
    holly: &Holly,
    root: SessionId,
) -> anyhow::Result<SessionId> {
    let rows = assistant_event::Entity::find()
        .filter(assistant_event::Column::RootSessionId.eq(root.0.clone()))
        .order_by_asc(assistant_event::Column::Id)
        .all(db)
        .await
        .context("loading assistant_events for resume")?;

    let records: Vec<LogRecord> = rows
        .into_iter()
        .map(|r| serde_json::from_value(r.payload).context("deserializing LogRecord"))
        .collect::<anyhow::Result<_>>()?;

    if let Some(dropped) = integrity_gap(&records) {
        anyhow::bail!(
            "refusing to resume `{}`: log is missing {dropped} record(s)",
            root.0
        );
    }

    holly
        .resume(root.clone(), pair_records(&records))
        .await
        .map_err(|_| anyhow::anyhow!("engine inbox closed"))
}

/// Delete every persisted event for `root` — call after telling the engine to
/// close the session (`InMsg::CloseSession`), when the next phase's
/// session-delete handler removes the `assistant_sessions` row.
pub async fn delete_session_events(
    db: &DatabaseConnection,
    root: &SessionId,
) -> anyhow::Result<()> {
    assistant_event::Entity::delete_many()
        .filter(assistant_event::Column::RootSessionId.eq(root.0.clone()))
        .exec(db)
        .await
        .context("deleting assistant_events")?;
    Ok(())
}
