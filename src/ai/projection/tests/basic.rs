//! Single-session projection shapes (no sub-agent nesting) — see
//! `subagent.rs` for the #17 nesting scenarios.

use super::*;

/// prompt -> assistant text-only turn.
#[test]
fn text_only_turn_projects_user_then_assistant() {
    let s = SessionId::new("u1:test");
    let records = vec![
        rec(
            &s,
            inm(InMsg::Prompt {
                session: s.clone(),
                content: vec![ContentPart::text("hi")],
            }),
        ),
        rec(
            &s,
            out(OutEvent::TextDelta {
                session: s.clone(),
                seq: 1,
                text: "Hello".into(),
            }),
        ),
        rec(
            &s,
            out(OutEvent::TextDelta {
                session: s.clone(),
                seq: 2,
                text: " there".into(),
            }),
        ),
        rec(
            &s,
            out(OutEvent::Done {
                session: s.clone(),
                seq: 3,
            }),
        ),
    ];

    let projected = project(&records);
    assert_eq!(
        projected,
        vec![
            ProjectedMessage {
                role: "user",
                content: json!({ "text": "hi" })
            },
            ProjectedMessage {
                role: "assistant",
                content: json!({ "text": "Hello there", "tool_calls": [] }),
            },
        ]
    );
}

/// prompt -> tool-call -> approval-pause -> tool-result -> final text.
#[test]
fn tool_call_with_approval_pause_projects_full_round_trip() {
    let s = SessionId::new("u1:test");
    let records = vec![
        rec(
            &s,
            inm(InMsg::Prompt {
                session: s.clone(),
                content: vec![ContentPart::text("edit the page")],
            }),
        ),
        rec(
            &s,
            out(OutEvent::ToolCall {
                session: s.clone(),
                seq: 1,
                request_id: "call-1".into(),
                tool: "edit_page".into(),
                input: r#"{"path":"foo","markdown":"bar"}"#.into(),
            }),
        ),
        rec(
            &s,
            out(OutEvent::ToolRequest {
                session: s.clone(),
                seq: 2,
                request_id: "call-1".into(),
                tool: "edit_page".into(),
                input: r#"{"path":"foo","markdown":"bar"}"#.into(),
            }),
        ),
        rec(
            &s,
            inm(InMsg::Approve {
                session: s.clone(),
                request_id: "call-1".into(),
                scope: Default::default(),
            }),
        ),
        rec(
            &s,
            out(OutEvent::ToolOutput {
                session: s.clone(),
                seq: 3,
                request_id: "call-1".into(),
                tool: "edit_page".into(),
                output: "updated: foo".into(),
                content: vec![],
            }),
        ),
        rec(
            &s,
            out(OutEvent::TextDelta {
                session: s.clone(),
                seq: 4,
                text: "Done!".into(),
            }),
        ),
        rec(
            &s,
            out(OutEvent::Done {
                session: s.clone(),
                seq: 5,
            }),
        ),
    ];

    let projected = project(&records);
    assert_eq!(
        projected,
        vec![
            ProjectedMessage {
                role: "user",
                content: json!({ "text": "edit the page" })
            },
            ProjectedMessage {
                role: "assistant",
                content: json!({
                    "text": Value::Null,
                    "tool_calls": [{
                        "id": "call-1",
                        "name": "edit_page",
                        "args": { "path": "foo", "markdown": "bar" },
                    }],
                    "requires_approval": true,
                    "decisions": [{ "tool_call_id": "call-1", "approve": true }],
                }),
            },
            ProjectedMessage {
                role: "tool_result",
                content: json!({
                    "tool_call_id": "call-1",
                    "output": "updated: foo",
                    "is_error": false,
                }),
            },
            ProjectedMessage {
                role: "assistant",
                content: json!({ "text": "Done!", "tool_calls": [] }),
            },
        ]
    );
}

#[test]
fn tool_error_output_is_flagged() {
    let s = SessionId::new("u1:test");
    let records = vec![
        rec(
            &s,
            out(OutEvent::ToolCall {
                session: s.clone(),
                seq: 1,
                request_id: "call-2".into(),
                tool: "bash".into(),
                input: "{}".into(),
            }),
        ),
        rec(
            &s,
            out(OutEvent::ToolOutput {
                session: s.clone(),
                seq: 2,
                request_id: "call-2".into(),
                tool: "bash".into(),
                output: "tool `bash` denied by permission profile".into(),
                content: vec![],
            }),
        ),
    ];
    let projected = project(&records);
    let tool_result = &projected[1];
    assert_eq!(tool_result.role, "tool_result");
    assert_eq!(tool_result.content["is_error"], json!(true));
}

#[test]
fn error_event_flushes_open_turn_and_projects_error_message() {
    let s = SessionId::new("u1:test");
    let records = vec![
        rec(
            &s,
            out(OutEvent::TextDelta {
                session: s.clone(),
                seq: 1,
                text: "partial".into(),
            }),
        ),
        rec(
            &s,
            out(OutEvent::Error {
                session: s.clone(),
                seq: 2,
                message: "boom".into(),
            }),
        ),
    ];
    let projected = project(&records);
    assert_eq!(
        projected,
        vec![
            ProjectedMessage {
                role: "assistant",
                content: json!({ "text": "partial", "tool_calls": [] }),
            },
            ProjectedMessage {
                role: "error",
                content: json!({ "text": "boom" })
            },
        ]
    );
}
