//! The hand-built `assistant_events` log `tests/assistant_session_subagent_
//! resume.rs` resumes: a root whose `agent_spawn`ed `page-writer` child is
//! parked on a pending `page_edit` approval, written straight into the table
//! instead of driven through a live turn. `#[path]`-included by that test
//! only; split out to keep it under the 400-line cap.

use entanglement_core::{ContentPart, InMsg, OutEvent, SessionId};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use sea_orm::{ActiveModelTrait, Set};

/// Insert one `LogRecord` into `assistant_events` under `root`'s
/// `root_session_id` — same shape `DbSink`/`entanglement_runtime`'s
/// persistence tap produce, but callable directly so a whole root+child log
/// can be fabricated without ever running a live turn (see the module doc).
async fn insert(db: &sea_orm::DatabaseConnection, root: &SessionId, record: LogRecord) {
    let value = serde_json::to_value(&record).expect("serialize LogRecord");
    site::entity::assistant_event::ActiveModel {
        root_session_id: Set(root.0.clone()),
        payload: Set(value),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert assistant_event row");
}

/// Insert the whole root + child log under `root`'s `root_session_id`; the
/// child's call targets the page at `path`.
pub async fn insert_parked_subagent_log(
    db: &sea_orm::DatabaseConnection,
    root: &SessionId,
    child: &SessionId,
    path: &str,
) {
    // The root's own settled turn: prompt → spawn `page-writer` → the spawn's
    // own immediate reply (naming the child, #421/ADR-0113's synthesized
    // prompt is a *runtime*-side behavior for a live spawn — here the
    // equivalent user-role framing message for the child is inserted
    // directly, below) → done.
    insert(
        db,
        root,
        LogRecord::new(
            root.clone(),
            LogPayload::Out(OutEvent::SessionStarted {
                session: root.clone(),
                parent: None,
                predecessor: None,
                profile: "build".into(),
                model: None,
                user: None,
                root: true,
                ts: 0,
            }),
        ),
    )
    .await;
    insert(
        db,
        root,
        LogRecord::new(
            root.clone(),
            LogPayload::In(InMsg::prompt(root.clone(), "draft a page")),
        ),
    )
    .await;
    insert(
        db,
        root,
        LogRecord::new(
            root.clone(),
            LogPayload::Out(OutEvent::ToolCall {
                session: root.clone(),
                seq: 1,
                request_id: "spawn-1".into(),
                tool: "agent_spawn".into(),
                input: format!(
                    r#"{{"agent":"page-writer","prompt":"create a page at path {path} with markdown content 'hello from a resumed sub-agent'"}}"#
                ),
            }),
        ),
    )
    .await;
    insert(
        db,
        root,
        LogRecord::new(
            root.clone(),
            LogPayload::Out(OutEvent::ToolOutput {
                session: root.clone(),
                seq: 2,
                request_id: "spawn-1".into(),
                tool: "agent_spawn".into(),
                output: format!(
                    "Sub-agent launched under the `page-writer` profile. agent_id: {}. \
                     Call agent_poll with this agent_id to await its answer.",
                    child.0
                ),
                content: Vec::<ContentPart>::new(),
            }),
        ),
    )
    .await;
    insert(
        db,
        root,
        LogRecord::new(
            root.clone(),
            LogPayload::Out(OutEvent::Done {
                session: root.clone(),
                seq: 3,
            }),
        ),
    )
    .await;

    // The child's own log, filed under the *same* `root_session_id` (the
    // `DbSink`/persistence-tap convention `crate::ai::projection::project`
    // and the engine's own cascaded resume both depend on) — started, framed
    // by its spawning prompt, and parked on a pending `page_edit` call (a
    // fresh test user has no `tool_permissions` rows, so `page_edit` defaults
    // to `Ask` — this is genuinely a **live**, unresolved sub-agent as of
    // where the log stops).
    insert(
        db,
        root,
        LogRecord::new(
            child.clone(),
            LogPayload::Out(OutEvent::SessionStarted {
                session: child.clone(),
                parent: Some(root.clone()),
                predecessor: None,
                profile: "page-writer".into(),
                model: None,
                user: None,
                root: false,
                ts: 0,
            }),
        ),
    )
    .await;
    // A live session always logs its starting profile as `AgentChanged` right
    // after `SessionStarted`, and that record — not `SessionStarted`'s
    // `profile` — is what replay restores the profile from.
    insert(
        db,
        root,
        LogRecord::new(
            child.clone(),
            LogPayload::Out(OutEvent::AgentChanged {
                session: child.clone(),
                agent: "page-writer".into(),
                profile_detail: None,
            }),
        ),
    )
    .await;
    insert(
        db,
        root,
        LogRecord::new(
            child.clone(),
            LogPayload::In(InMsg::prompt(
                child.clone(),
                format!(
                    "create a page at path {path} with markdown content 'hello from a resumed sub-agent'"
                ),
            )),
        ),
    )
    .await;
    insert(
        db,
        root,
        LogRecord::new(
            child.clone(),
            LogPayload::Out(OutEvent::ToolCall {
                session: child.clone(),
                seq: 1,
                request_id: "edit-1".into(),
                tool: "page_edit".into(),
                input: format!(
                    r#"{{"path":"{path}","markdown":"hello from a resumed sub-agent"}}"#
                ),
            }),
        ),
    )
    .await;
    insert(
        db,
        root,
        LogRecord::new(
            child.clone(),
            LogPayload::Out(OutEvent::ToolRequest {
                session: child.clone(),
                seq: 2,
                request_id: "edit-1".into(),
                tool: "page_edit".into(),
                input: format!(
                    r#"{{"path":"{path}","markdown":"hello from a resumed sub-agent"}}"#
                ),
            }),
        ),
    )
    .await;
}
