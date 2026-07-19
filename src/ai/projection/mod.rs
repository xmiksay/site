//! Fold a session's `assistant_events` rows (each a
//! `entanglement_runtime::session_store::LogRecord`) into the JSON shape the
//! Vue admin client already understands — `role` one of `user | assistant |
//! tool_result | error`, with the `content` shapes documented on [`project`].
//! Pure logic, no DB access — the easiest piece in this batch to unit test
//! (see `tests.rs`).
//!
//! **Scope note:** this returns `{"role", "content"}` pairs only, not a full
//! `MessageView` (`id`/`seq`/`created_at`). An `assistant_events` row doesn't
//! map 1:1 to a client-visible "message" the way an `assistant_messages` row
//! used to — several `TextDelta`s fold into one assistant message, and a
//! multi-tool-call batch's calls/results interleave across several rows — so
//! deciding stable synthetic ids/seqs for the wrapped `MessageView` is left to
//! the next phase, once it settles how `GET /api/assistant/sessions/{id}`
//! numbers these going forward.
//!
//! **`is_error` limitation:** `OutEvent::ToolOutput` carries no explicit error
//! flag (unlike the old system's `ToolRegistry::dispatch` which returned one
//! directly). `entanglement_runtime::tool_runner`'s own reply text for every
//! failure path (`Deny`, reject, spawn-mask, unknown tool, or the executor's
//! `tool `{name}` failed: {e}` wrap) always starts with `"tool `"` or
//! `"unknown tool:"` — [`looks_like_tool_error`] keys off that. Re-checked
//! against entanglement-core 0.3.0 (issue #43): `ToolOutput` still carries
//! only `output`/`content`, no error flag, so the heuristic stands. It's a
//! heuristic, not a structural guarantee; a future engine release exposing a
//! real flag on `ToolOutput` should replace it.
//!
//! ## Sub-agent (#17) nesting
//!
//! `assistant_events` rows for a whole session tree (a root plus any
//! `researcher`/`page-writer` children it spawned) all share one
//! `root_session_id` — see `persistence.rs` — so `records` here can contain
//! several sessions' worth of interleaved rows. [`project`] partitions them
//! by `LogRecord.session`: the root's own records fold exactly as before;
//! every other session's records are a sub-agent's own turn sequence, folded
//! independently by the same logic and attached as a `sub_agents` array on
//! the assistant message whose `tool_calls` include the `agent_spawn`/`agent`
//! call that produced it.
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

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use entanglement_core::{InMsg, OutEvent, SessionId};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use serde_json::{Value, json};

/// One projected client-visible message: `{"role": ..., "content": ...}`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedMessage {
    pub role: &'static str,
    pub content: Value,
}

/// Fold a root session's ordered event log into projected messages, nesting
/// any sub-agent (#17) children under the turn that spawned them (see the
/// module doc). Records must be in the order they were appended
/// (`assistant_events` ordered by `id`, i.e. insertion order) — this is a
/// linear fold, not a sort. The very first record's session is taken as the
/// root — always true in practice, since a sub-agent session cannot exist
/// before its root does.
pub fn project(records: &[LogRecord]) -> Vec<ProjectedMessage> {
    let Some(root) = records.first().map(|r| r.session.clone()) else {
        return Vec::new();
    };

    let mut own: Vec<&LogRecord> = Vec::new();
    let mut child_records: HashMap<SessionId, Vec<&LogRecord>> = HashMap::new();
    let mut child_profiles: HashMap<SessionId, String> = HashMap::new();
    for record in records {
        if record.session == root {
            own.push(record);
            continue;
        }
        if let LogPayload::Out(OutEvent::SessionStarted { profile, .. }) = &record.payload {
            child_profiles.insert(record.session.clone(), profile.clone());
        }
        child_records
            .entry(record.session.clone())
            .or_default()
            .push(record);
    }

    let mut out = fold(&own);
    if !child_records.is_empty() {
        attach_sub_agents(&mut out, child_records, &child_profiles);
    }
    out
}

/// Attach each spawned sub-agent's projection to the specific
/// `agent_spawn`/`agent` call that produced it — see the module doc for why
/// this is a structural match (via [`extract_child_session_id`]), not a
/// positional one. Any child whose owning call couldn't be matched (a
/// gap-truncated resume dropped the spawning message, say) is still
/// surfaced — appended as a turn-less trailing message rather than silently
/// dropped.
fn attach_sub_agents(
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

/// The original per-session fold, shared by [`project`] for the root's own
/// records and for each sub-agent child's own record slice.
fn fold(records: &[&LogRecord]) -> Vec<ProjectedMessage> {
    let mut out = Vec::new();
    let mut turn = OpenTurn::default();

    for record in records {
        match &record.payload {
            LogPayload::In(InMsg::Prompt { content, .. }) => {
                turn.flush_into(&mut out);
                let text = entanglement_core::content_text(content);
                out.push(ProjectedMessage {
                    role: "user",
                    content: json!({ "text": text }),
                });
            }
            LogPayload::In(InMsg::Approve { request_id, .. }) => {
                turn.decisions
                    .push(json!({ "tool_call_id": request_id, "approve": true }));
            }
            LogPayload::In(InMsg::Reject { request_id, .. }) => {
                turn.decisions
                    .push(json!({ "tool_call_id": request_id, "approve": false }));
            }
            LogPayload::Out(OutEvent::TextDelta { text, .. }) => {
                turn.open = true;
                turn.text.push_str(text);
            }
            LogPayload::Out(OutEvent::ToolCall {
                request_id,
                tool,
                input,
                ..
            }) => {
                turn.open = true;
                let args: Value = serde_json::from_str(input).unwrap_or_else(|_| json!(input));
                turn.tool_calls.push(json!({
                    "id": request_id,
                    "name": tool,
                    "args": args,
                }));
            }
            LogPayload::Out(OutEvent::ToolRequest { request_id, .. }) => {
                turn.open = true;
                turn.pending.insert(request_id.clone());
            }
            LogPayload::Out(OutEvent::ToolOutput {
                request_id, output, ..
            }) => {
                // A resolved call means this turn's tool_calls are fully
                // enumerated (core emits the whole batch before any result
                // comes back) — flush the assistant message before recording
                // the result, so the client sees them in the right order.
                turn.flush_into(&mut out);
                out.push(ProjectedMessage {
                    role: "tool_result",
                    content: json!({
                        "tool_call_id": request_id,
                        "output": output,
                        "is_error": looks_like_tool_error(output),
                    }),
                });
            }
            LogPayload::Out(OutEvent::Done { .. }) => {
                turn.flush_into(&mut out);
            }
            LogPayload::Out(OutEvent::Error { message, .. }) => {
                turn.flush_into(&mut out);
                out.push(ProjectedMessage {
                    role: "error",
                    content: json!({ "text": message }),
                });
            }
            _ => {} // lifecycle/status events carry no client-visible content
        }
    }
    turn.flush_into(&mut out);
    out
}

/// `tool_runner`'s reply text for every failure path (`Deny`/reject/mask/
/// unknown-tool/execution-error) starts with one of these two prefixes — see
/// the module doc for why this is a heuristic, not a structural flag.
///
/// Deliberately deferred (originally issue #28, re-investigated for 0.3.0 by
/// #43): `OutEvent::ToolOutput` still carries no error flag to key off
/// instead, and this crate is a versioned dependency (not vendored in this
/// repo), so there is nothing to change here yet. Replace with a structural
/// flag the day `ToolOutput` grows one.
fn looks_like_tool_error(output: &str) -> bool {
    output.starts_with("tool `") || output.starts_with("unknown tool:")
}

/// Buffered state for the assistant turn currently being folded — reset by
/// [`OpenTurn::flush_into`], which is a no-op if nothing has accumulated.
#[derive(Default)]
struct OpenTurn {
    open: bool,
    text: String,
    tool_calls: Vec<Value>,
    pending: HashSet<String>,
    decisions: Vec<Value>,
}

impl OpenTurn {
    fn flush_into(&mut self, out: &mut Vec<ProjectedMessage>) {
        if !self.open {
            return;
        }
        let mut content = json!({
            "text": if self.text.is_empty() { Value::Null } else { json!(self.text) },
            "tool_calls": self.tool_calls,
        });
        if let Some(obj) = content.as_object_mut() {
            if !self.pending.is_empty() {
                obj.insert("requires_approval".into(), Value::Bool(true));
            }
            if !self.decisions.is_empty() {
                obj.insert(
                    "decisions".into(),
                    Value::Array(std::mem::take(&mut self.decisions)),
                );
            }
        }
        out.push(ProjectedMessage {
            role: "assistant",
            content,
        });
        *self = OpenTurn::default();
    }
}
