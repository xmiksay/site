//! Sub-agent (#17) nesting: attach a spawned child session's own projected
//! turn to the specific `agent_spawn`/`agent` call that produced it. Split
//! out of `mod.rs` to keep that file under the project's 400-line cap.
//!
//! `assistant_events` rows for a whole session tree (a root plus any
//! `researcher`/`page-writer` children it spawned) all share one
//! `root_session_id` — see `persistence.rs` — so `project`'s `records` can
//! contain several sessions' worth of interleaved rows. `project` partitions
//! them by `LogRecord.session`: the root's own records fold exactly as
//! before; every other session's records are a sub-agent's own turn
//! sequence, folded independently by `fold` and attached here as a
//! `sub_agents` array on the assistant message whose `tool_calls` include the
//! `agent_spawn`/`agent` call that produced it.
//!
//! Matching a child to its spawning call is **structural, not positional**:
//! `InMsg::Spawn` is never persisted (see `entanglement_runtime::persistence`'s
//! doc), so there is no direct field linking a `ToolCall` to the `SessionId`
//! it produced — but `entanglement_runtime::subagent::launch`'s own immediate
//! reply *text* always names the child (`"...agent_id: {uuid}..."` for a
//! detached `agent_spawn`, `` "sub-agent `{uuid}` completed..." `` for a
//! blocking `agent`), and that reply is exactly the `tool_result` paired with
//! *that* call's own `tool_call_id`. [`extract_child_session_id`] recovers the
//! uuid from it, so matching only ever considers a call's own result — never
//! an earlier or later message's — and correctly handles both a spawn that
//! never actually started a session (a refusal's text contains no valid uuid,
//! so it's skipped rather than stealing a later real child) and two spawns in
//! the same batch racing to start concurrently (each still names its own
//! child, so log order between them is irrelevant). A child's profile name
//! comes from its own `SessionStarted` record; its task/prompt comes from the
//! spawning call's own `args.prompt`, so the client never has to re-derive
//! either by position.

use std::collections::HashMap;

use entanglement_core::SessionId;
use entanglement_runtime::session_store::LogRecord;
use serde_json::{Value, json};

use super::{ProjectedMessage, fold};

/// Attach each spawned sub-agent's projection to the specific
/// `agent_spawn`/`agent` call that produced it — see the module doc for why
/// this is a structural match (via [`extract_child_session_id`]), not a
/// positional one. Any child whose owning call couldn't be matched (a
/// gap-truncated resume dropped the spawning message, say) is still
/// surfaced — appended as a turn-less trailing message rather than silently
/// dropped.
pub(super) fn attach_sub_agents(
    out: &mut Vec<ProjectedMessage>,
    mut child_records: HashMap<SessionId, Vec<&LogRecord>>,
    child_profiles: &HashMap<SessionId, String>,
) {
    // Every tool_call_id's own result text, so a spawn call's match is scoped
    // to *its* result regardless of how far away it landed in `out`. Owned
    // (not borrowed) so the loop below can mutate `out` at the same time.
    let outputs: HashMap<String, String> = out
        .iter()
        .filter(|m| m.role == "tool_result")
        .filter_map(|m| {
            Some((
                m.content.get("tool_call_id")?.as_str()?.to_string(),
                m.content.get("output")?.as_str()?.to_string(),
            ))
        })
        .collect();

    for msg in out.iter_mut() {
        if msg.role != "assistant" {
            continue;
        }
        let Some(tool_calls) = msg
            .content
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .cloned()
        else {
            continue;
        };
        let mut sub_agents = Vec::new();
        for tc in &tool_calls {
            let is_spawn = matches!(
                tc.get("name").and_then(Value::as_str),
                Some("agent_spawn" | "agent")
            );
            if !is_spawn {
                continue;
            }
            let Some(call_id) = tc.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(child_id) = outputs
                .get(call_id)
                .and_then(|output| extract_child_session_id(output))
            else {
                continue;
            };
            let child = SessionId::new(child_id);
            let Some(recs) = child_records.remove(&child) else {
                continue;
            };
            let task = tc
                .get("args")
                .and_then(|a| a.get("prompt"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            sub_agents.push(sub_agent_json(&child, child_profiles, task, fold(&recs)));
        }
        if !sub_agents.is_empty()
            && let Some(obj) = msg.content.as_object_mut()
        {
            obj.insert("sub_agents".into(), Value::Array(sub_agents));
        }
    }

    if !child_records.is_empty() {
        let leftover: Vec<Value> = child_records
            .into_iter()
            .map(|(child, recs)| sub_agent_json(&child, child_profiles, "", fold(&recs)))
            .collect();
        out.push(ProjectedMessage {
            role: "sub_agents",
            content: Value::Array(leftover),
        });
    }
}

fn sub_agent_json(
    child: &SessionId,
    child_profiles: &HashMap<SessionId, String>,
    task: &str,
    messages: Vec<ProjectedMessage>,
) -> Value {
    json!({
        "agent_id": child.0,
        "profile": child_profiles.get(child).cloned().unwrap_or_default(),
        "task": task,
        "messages": messages
            .into_iter()
            .map(|m| json!({ "role": m.role, "content": m.content }))
            .collect::<Vec<_>>(),
    })
}

/// Recover a sub-agent child's own `SessionId` from its spawning call's
/// `tool_result` text — see the module doc. `entanglement_runtime::subagent::
/// launch`'s reply always embeds the child's raw uuid as one whitespace/
/// punctuation-delimited token (`` `{uuid}` `` or `agent_id: {uuid}.`);
/// scanning for the first token that parses as a uuid finds it regardless of
/// which of the two reply templates (detached `agent_spawn` vs blocking
/// `agent`) produced the text. A refusal's text (no valid uuid anywhere)
/// correctly yields `None` — nothing to match, not a wrong match.
///
/// Deliberately deferred (issue #28): `entanglement_runtime::subagent::launch`
/// has no structured field naming the child session either, and this crate is
/// a versioned dependency (not vendored in this repo), so there is nothing to
/// change here yet. Replace with a structural field the day `launch` grows
/// one.
fn extract_child_session_id(output: &str) -> Option<String> {
    output
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find_map(|tok| uuid::Uuid::parse_str(tok).ok().map(|_| tok.to_string()))
}
