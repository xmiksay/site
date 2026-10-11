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
use entanglement_core::{Holly, OutEvent, SessionId};
use entanglement_runtime::persistence::RecordSink;
use entanglement_runtime::session_store::{LogPayload, LogRecord, integrity_gap, pair_records};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

use crate::entity::assistant_event;

/// Backlog cap on the writer task's channel. `RecordSink::append` must never
/// block (persistence.rs doc, embedding.md §3) — a full channel means the DB
/// writer has fallen far behind, so `append` sheds the record with an `Err`
/// rather than awaiting. Unlike a plain drop, the shed is not silent: it is
/// tallied in [`DbSink::dropped`] and turned into a [`LogPayload::Gap`]
/// tombstone by the writer task's periodic flush (see [`GAP_FLUSH_INTERVAL`]),
/// the same signal `entanglement_runtime`'s own broadcast-lag path writes —
/// so [`integrity_gap`] can actually see this kind of loss too, and
/// `resume_session` degrades gracefully instead of resuming over a silent
/// hole (issue #28).
const SINK_CHANNEL_CAPACITY: usize = 1024;

/// How often the writer task checks for accumulated backlog-drop counts and
/// persists them as `Gap` tombstones. A drop is tallied synchronously (in
/// `append`, off the critical never-block path) but the tombstone itself is
/// written on this cadence rather than inline, so `append` never has to wait
/// for a DB round-trip either.
const GAP_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// A `RecordSink` that appends to `assistant_events` behind a bounded channel
/// and a dedicated writer task, so `append` (called from the persistence tap,
/// which reads `Holly`'s outbound broadcast) never awaits a DB round-trip.
pub struct DbSink {
    tx: mpsc::Sender<(SessionId, LogRecord)>,
    /// Per-root count of records shed since the last gap flush — see the
    /// module doc. A brief, uncontended lock, never held across an `.await`.
    dropped: std::sync::Arc<Mutex<HashMap<SessionId, u64>>>,
}

impl DbSink {
    pub fn new(db: DatabaseConnection) -> Self {
        let (tx, mut rx) = mpsc::channel::<(SessionId, LogRecord)>(SINK_CHANNEL_CAPACITY);
        let dropped = std::sync::Arc::new(Mutex::new(HashMap::new()));
        let writer_dropped = dropped.clone();
        tokio::spawn(async move {
            let mut gap_flush = tokio::time::interval(GAP_FLUSH_INTERVAL);
            gap_flush.tick().await; // first tick fires immediately; nothing to flush yet
            loop {
                tokio::select! {
                    biased;
                    maybe = rx.recv() => {
                        let Some((root, record)) = maybe else { break };
                        insert_record(&db, &root, &record).await;
                    }
                    _ = gap_flush.tick() => {
                        flush_gaps(&db, &writer_dropped).await;
                    }
                }
            }
            // Drain any drop tallied right before the channel closed.
            flush_gaps(&db, &writer_dropped).await;
        });
        DbSink { tx, dropped }
    }
}

impl RecordSink for DbSink {
    fn append(&self, root: &SessionId, record: &LogRecord) -> anyhow::Result<()> {
        self.tx
            .try_send((root.clone(), record.clone()))
            .map_err(|_| {
                *self
                    .dropped
                    .lock()
                    .unwrap()
                    .entry(root.clone())
                    .or_insert(0) += 1;
                anyhow::anyhow!("assistant_events sink backlog full, dropping record")
            })
    }
}

async fn insert_record(db: &DatabaseConnection, root: &SessionId, record: &LogRecord) {
    let payload = match serde_json::to_value(record) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, root = %root.0, "failed to serialize LogRecord");
            return;
        }
    };
    let row = assistant_event::ActiveModel {
        root_session_id: Set(root.0.clone()),
        payload: Set(payload),
        ..Default::default()
    };
    if let Err(e) = row.insert(db).await {
        tracing::error!(error = %e, root = %root.0, "failed to persist assistant event");
    }
}

/// Turn every root's tallied backlog-drop count into one `Gap` tombstone,
/// mirroring `entanglement_runtime::persistence::record_gap`'s shape for the
/// broadcast-lag case — see the module doc.
async fn flush_gaps(db: &DatabaseConnection, dropped: &Mutex<HashMap<SessionId, u64>>) {
    let pending: Vec<(SessionId, u64)> = dropped.lock().unwrap().drain().collect();
    for (root, count) in pending {
        tracing::warn!(root = %root.0, count, "sink backlog overflowed; recording a gap tombstone");
        let record = LogRecord::new(root.clone(), LogPayload::Gap { dropped: count });
        insert_record(db, &root, &record).await;
    }
}

/// Load `root`'s log from `assistant_events` and resume it into `holly`.
/// Nothing is loaded until this is actually called (lazy resume,
/// embedding.md §3) — e.g. the next phase's "open session" handler, when the
/// session isn't already live.
///
/// A detected [`LogPayload::Gap`] tombstone (a broadcast lag or a sink
/// backlog overflow) no longer hard-refuses the whole resume: replaying
/// *through* the gap would silently fold an incomplete history, but the
/// prefix strictly before it is intact, so that prefix is what gets resumed
/// — the tail after the gap is permanently lost, but the session itself
/// stays resumable forever after, instead of every future `ensure_live`
/// failing (issue #28). See [`truncate_at_gap`].
///
/// The tree is resumed **one session at a time, parent before child**
/// ([`live_tree`]), each from its own records only, rather than handing
/// `Holly` the root and letting its cascade (ADR-0112) spawn every live
/// sub-agent at once. The cascade starts all those tasks together, so a
/// resumed child can re-offer its parked tool call before its parent has
/// announced itself; the tool executor then sees an unknown ancestor, fails
/// closed, and permanently refuses the call ("restricted by ancestor agent
/// `unknown`"), and the persistence tap files the child's new records under
/// the child's own id (#124). Waiting for each session's start announcement
/// before resuming its children orders the broadcast for both. Holly's
/// cascade only ever used a session's live-children list to know whom to
/// spawn, so spawning them ourselves leaves the same sessions running.
pub async fn resume_session(
    db: &DatabaseConnection,
    holly: &Holly,
    root: SessionId,
) -> anyhow::Result<SessionId> {
    let records = load_tree_records(db, &root).await?;
    let mut events = holly.subscribe();
    for session in live_tree(&records, &root) {
        let own: Vec<LogRecord> = records
            .iter()
            .filter(|r| r.session == session)
            .cloned()
            .collect();
        holly
            .resume(session.clone(), pair_records(&own))
            .await
            .map_err(|_| anyhow::anyhow!("engine inbox closed"))?;
        await_started(&mut events, &session).await;
    }
    Ok(root)
}

/// Ceiling on waiting for one resumed session's start announcement. A
/// refused resume answers with an `Error` instead, which ends the wait too.
const RESUME_START_TIMEOUT: Duration = Duration::from_secs(30);

/// Wait for `session`'s `AgentChanged` — the last of the events a session
/// task emits on start, so every subscriber (the tool executor included)
/// has folded its identity and profile once it is seen.
async fn await_started(events: &mut broadcast::Receiver<OutEvent>, session: &SessionId) {
    let deadline = tokio::time::Instant::now() + RESUME_START_TIMEOUT;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(
                OutEvent::AgentChanged { session: s, .. } | OutEvent::Error { session: s, .. },
            )) if s == *session => {
                return;
            }
            Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(broadcast::error::RecvError::Closed)) => return,
            Err(_) => {
                tracing::warn!(session = %session.0, "resumed session never announced its start");
                return;
            }
        }
    }
}

/// `root` plus every sub-agent still live as of where its log stops (a
/// `SessionStarted` not followed by its `SessionEnded`/`SessionHibernated`),
/// breadth-first so a parent always precedes its children — the same set
/// `Holly`'s resume cascade would spawn, in a safe order.
fn live_tree(records: &[LogRecord], root: &SessionId) -> Vec<SessionId> {
    let mut parent_of: HashMap<SessionId, SessionId> = HashMap::new();
    let mut live: std::collections::HashSet<SessionId> = std::collections::HashSet::new();
    for r in records {
        match &r.payload {
            LogPayload::Out(OutEvent::SessionStarted {
                session,
                parent: Some(parent),
                ..
            }) => {
                parent_of.insert(session.clone(), parent.clone());
                live.insert(session.clone());
            }
            LogPayload::Out(
                OutEvent::SessionEnded { session, .. }
                | OutEvent::SessionHibernated { session, .. },
            ) => {
                live.remove(session);
            }
            _ => {}
        }
    }
    let mut order = vec![root.clone()];
    let mut cursor = 0;
    while cursor < order.len() {
        let parent = order[cursor].clone();
        cursor += 1;
        let mut children: Vec<SessionId> = parent_of
            .iter()
            .filter(|(child, p)| **p == parent && live.contains(*child) && !order.contains(child))
            .map(|(child, _)| child.clone())
            .collect();
        children.sort_by(|a, b| a.0.cmp(&b.0));
        order.extend(children);
    }
    order
}

/// Resume a sub-agent **child** session on its own, from its slice of `root`'s
/// log (#101).
///
/// Needed because a finished sub-agent is never `CloseSession`d upstream
/// (`entanglement_runtime::subagent` only waits for `Done`), so a child stays
/// prompt-able — but only while it is live. Once the idle-TTL sweep hibernates
/// it, the id has to be rebuilt, and the plain [`resume_session`] can't do it:
/// its `root_session_id = <child uuid>` filter matches zero rows (a child's
/// records are filed under its root, m_032), so it would resume the child from
/// an *empty* log and silently replace an intact transcript with a blank one.
///
/// The filter is on `LogRecord::session` — the tap stamps each record with the
/// session that produced it — rather than handing `Holly` the whole tree:
/// `pair_records` binds each `In` to the *next* `Out` regardless of session, so
/// an unfiltered log could pair the root's own prompt onto a child event and
/// fold a message the child never saw into its context. Narrowing first makes
/// the pairing exact. The cost is that a grandchild is not cascaded along (its
/// records name itself, not `child`); it resumes the same way if it is ever
/// prompted in turn.
///
/// Refuses (rather than resuming blank) when the child has no records at all —
/// that is either a wrong log key or a child whose events never landed, and in
/// both cases materializing an empty session under its id is the exact failure
/// this function exists to prevent.
pub async fn resume_child_session(
    db: &DatabaseConnection,
    holly: &Holly,
    root: &SessionId,
    child: &SessionId,
) -> anyhow::Result<()> {
    let records = load_tree_records(db, root).await?;
    let own: Vec<LogRecord> = records
        .into_iter()
        .filter(|r| r.session == *child)
        .collect();
    if own.is_empty() {
        anyhow::bail!(
            "sub-agent session `{}` has no records under root `{}`",
            child.0,
            root.0
        );
    }
    holly
        .resume(child.clone(), pair_records(&own))
        .await
        .map_err(|_| anyhow::anyhow!("engine inbox closed"))?;
    Ok(())
}

/// `root`'s whole persisted log — the root's own records interleaved with every
/// session spawned under it — truncated at the first gap tombstone (see
/// [`resume_session`]'s doc for why a gap degrades rather than refuses).
async fn load_tree_records(
    db: &DatabaseConnection,
    root: &SessionId,
) -> anyhow::Result<Vec<LogRecord>> {
    let rows = assistant_event::Entity::find()
        .filter(assistant_event::Column::RootSessionId.eq(root.0.clone()))
        .order_by_asc(assistant_event::Column::Id)
        .all(db)
        .await
        .context("loading assistant_events for resume")?;

    let mut records: Vec<LogRecord> = rows
        .into_iter()
        .map(|r| serde_json::from_value(r.payload).context("deserializing LogRecord"))
        .collect::<anyhow::Result<_>>()?;

    if let Some((dropped, discarded)) = truncate_at_gap(&mut records) {
        tracing::warn!(
            root = %root.0,
            dropped,
            discarded,
            "resuming `{}` from the last good prefix before a persistence gap; \
             {discarded} record(s) after it are permanently lost",
            root.0
        );
    }
    Ok(records)
}

/// Truncate `records` to the prefix strictly before its first [`LogPayload::Gap`]
/// tombstone, if any. Returns `(total dropped per the tombstone(s), how many
/// trailing records were discarded)` — `None` when the log carries no gap, in
/// which case `records` is untouched.
fn truncate_at_gap(records: &mut Vec<LogRecord>) -> Option<(u64, usize)> {
    let gap_at = records
        .iter()
        .position(|r| matches!(r.payload, LogPayload::Gap { .. }))?;
    let dropped = integrity_gap(records).unwrap_or(0);
    let discarded = records.len() - gap_at;
    records.truncate(gap_at);
    Some((dropped, discarded))
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

#[cfg(test)]
mod tests;
