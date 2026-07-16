//! Fold a session's `assistant_events` rows (each a
//! `entanglement_runtime::session_store::LogRecord`) into the JSON shape the
//! Vue admin client already understands — `role` one of `user | assistant |
//! tool_result | error`, with the exact `content` shapes the old
//! `loop_driver.rs`-authored `assistant_messages` rows used (see the
//! doc-comment on [`project`]). Pure logic, no DB access — the easiest piece
//! in this batch to unit test, per the issue (see `tests.rs`).
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
//! `"unknown tool:"` — [`looks_like_tool_error`] keys off that. It's a
//! heuristic, not a structural guarantee; a future engine release exposing a
//! real flag on `ToolOutput` should replace it.

#[cfg(test)]
mod tests;

use std::collections::HashSet;

use entanglement_core::{InMsg, OutEvent};
use entanglement_runtime::session_store::{LogPayload, LogRecord};
use serde_json::{Value, json};

/// One projected client-visible message: `{"role": ..., "content": ...}`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedMessage {
    pub role: &'static str,
    pub content: Value,
}

/// Fold a root session's ordered event log into projected messages. Records
/// must be in the order they were appended (`assistant_events` ordered by
/// `id`, i.e. insertion order) — this is a linear fold, not a sort.
pub fn project(records: &[LogRecord]) -> Vec<ProjectedMessage> {
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
