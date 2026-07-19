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
//! `root_session_id` — so `records` here can contain several sessions' worth
//! of interleaved rows. [`project`] partitions them by `LogRecord.session`
//! and hands every non-root session's records to [`subagents`] to fold and
//! attach — see that module's doc for the structural (not positional)
//! child-to-spawning-call matching.

#[cfg(test)]
mod tests;

mod subagents;

use std::collections::{HashMap, HashSet};

use entanglement_core::{InMsg, OutEvent, SessionId};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use serde_json::{Value, json};
use subagents::attach_sub_agents;

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
    mark_resolved_calls(&mut out);
    out
}

/// Retroactively flag every `tool_calls[]` entry that already has a matching
/// `tool_result` message as `"resolved": true` — the most robust signal
/// available for "is this call actually done", and one the client should
/// trust over `decisions` (below). `flush_into`'s own per-call
/// `requires_approval` (paired with this) says whether a call was *ever*
/// gated at all; this says whether it's *still* worth a prompt. A client
/// should only ever offer Allow/Reject for a call with `requires_approval:
/// true` and no `resolved: true` — anything else is stale by construction.
///
/// Why this can't be derived from `decisions` alone: `InMsg::Approve`/
/// `Reject` is recorded into whichever `OpenTurn` happens to be accumulating
/// *at the moment that record is folded* — but a batch flushes (see
/// `ToolOutput`'s match arm above) the instant its *first* call resolves,
/// before every sibling in the same batch is necessarily decided. A
/// second/third decision for that same already-flushed message, arriving
/// after the reset, lands in a fresh `OpenTurn` instead — silently orphaned
/// from the message it was actually deciding. Presence of a `tool_result` for
/// the same `tool_call_id` sidesteps this entirely: it's only ever emitted
/// once a call has genuinely resolved, regardless of how or when its
/// decision got folded.
fn mark_resolved_calls(out: &mut [ProjectedMessage]) {
    let resolved: HashSet<String> = out
        .iter()
        .filter(|m| m.role == "tool_result")
        .filter_map(|m| m.content.get("tool_call_id")?.as_str().map(String::from))
        .collect();
    for msg in out.iter_mut() {
        if msg.role != "assistant" {
            continue;
        }
        let Some(tool_calls) = msg
            .content
            .get_mut("tool_calls")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for tc in tool_calls.iter_mut() {
            let is_resolved = tc
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| resolved.contains(id));
            if is_resolved && let Some(obj) = tc.as_object_mut() {
                obj.insert("resolved".into(), Value::Bool(true));
            }
        }
    }
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
        // Per-call, not just the message-level `requires_approval` below: a
        // batch can freely mix a call that actually paused for approval
        // (present in `self.pending`, i.e. it got its own `ToolRequest`) with
        // one the policy auto-allowed (only ever got a `ToolCall`, the
        // display-only event every call gets regardless). Both end up in
        // `tool_calls` either way, but only the former should ever offer an
        // Allow/Reject prompt — flagging the message as a whole isn't
        // specific enough for the client to tell them apart (see
        // `mark_resolved_calls`'s doc for the concrete symptom this caused).
        for tc in self.tool_calls.iter_mut() {
            let is_pending = tc
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| self.pending.contains(id));
            if is_pending && let Some(obj) = tc.as_object_mut() {
                obj.insert("requires_approval".into(), Value::Bool(true));
            }
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
