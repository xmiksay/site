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
                        "requires_approval": true,
                        "resolved": true,
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

/// A batch mixing a gated call with one the policy auto-allowed (no
/// `ToolRequest`/`InMsg::Approve` ever recorded for it — the engine just runs
/// it) must not offer a stale prompt for the auto-allowed one just because it
/// shares a `requires_approval: true` message with a gated sibling.
#[test]
fn auto_allowed_call_sharing_a_batch_with_a_gated_call_is_marked_resolved_not_pending() {
    let s = SessionId::new("u1:test");
    let records = vec![
        rec(
            &s,
            out(OutEvent::ToolCall {
                session: s.clone(),
                seq: 1,
                request_id: "gated".into(),
                tool: "edit_page".into(),
                input: "{}".into(),
            }),
        ),
        rec(
            &s,
            out(OutEvent::ToolCall {
                session: s.clone(),
                seq: 2,
                request_id: "auto".into(),
                tool: "search_pages".into(),
                input: "{}".into(),
            }),
        ),
        rec(
            &s,
            out(OutEvent::ToolRequest {
                session: s.clone(),
                seq: 3,
                request_id: "gated".into(),
                tool: "edit_page".into(),
                input: "{}".into(),
            }),
        ),
        rec(
            &s,
            inm(InMsg::Approve {
                session: s.clone(),
                request_id: "gated".into(),
                scope: Default::default(),
            }),
        ),
        rec(
            &s,
            out(OutEvent::ToolOutput {
                session: s.clone(),
                seq: 4,
                request_id: "gated".into(),
                tool: "edit_page".into(),
                output: "updated: foo".into(),
                content: vec![],
            }),
        ),
        rec(
            &s,
            out(OutEvent::ToolOutput {
                session: s.clone(),
                seq: 5,
                request_id: "auto".into(),
                tool: "search_pages".into(),
                output: "no matches".into(),
                content: vec![],
            }),
        ),
    ];

    let projected = project(&records);
    let assistant_msg = &projected[0];
    assert_eq!(assistant_msg.role, "assistant");
    assert_eq!(
        assistant_msg.content["tool_calls"],
        json!([
            {
                "id": "gated",
                "name": "edit_page",
                "args": {},
                "requires_approval": true,
                "resolved": true,
            },
            {
                "id": "auto",
                "name": "search_pages",
                "args": {},
                "resolved": true,
            },
        ]),
        "the auto-allowed call must never carry requires_approval, and both \
         must be marked resolved once their tool_result exists: {projected:#?}"
    );
}

/// Two gated calls in one batch, decided one at a time: the *second*
/// decision arrives after the first's `ToolOutput` has already flushed the
/// enclosing assistant message (see `OpenTurn::flush_into`'s doc), so its
/// `InMsg::Approve` record lands in a fresh, already-reset `OpenTurn` and
/// never makes it into that message's `decisions` array. `resolved` must
/// still reflect reality (via the second call's own `tool_result`) even
/// though `decisions` can't.
#[test]
fn a_decision_orphaned_by_an_earlier_flush_still_marks_its_call_resolved() {
    let s = SessionId::new("u1:test");
    let mut records = vec![
        rec(
            &s,
            out(OutEvent::ToolCall {
                session: s.clone(),
                seq: 1,
                request_id: "call-a".into(),
                tool: "create_tag".into(),
                input: "{}".into(),
            }),
        ),
        rec(
            &s,
            out(OutEvent::ToolCall {
                session: s.clone(),
                seq: 2,
                request_id: "call-b".into(),
                tool: "create_tag".into(),
                input: "{}".into(),
            }),
        ),
    ];
    for id in ["call-a", "call-b"] {
        records.push(rec(
            &s,
            out(OutEvent::ToolRequest {
                session: s.clone(),
                seq: 3,
                request_id: id.into(),
                tool: "create_tag".into(),
                input: "{}".into(),
            }),
        ));
    }
    // call-a decided and resolved first — this flushes the batch's assistant
    // message *before* call-b is decided at all.
    records.push(rec(
        &s,
        inm(InMsg::Approve {
            session: s.clone(),
            request_id: "call-a".into(),
            scope: Default::default(),
        }),
    ));
    records.push(rec(
        &s,
        out(OutEvent::ToolOutput {
            session: s.clone(),
            seq: 4,
            request_id: "call-a".into(),
            tool: "create_tag".into(),
            output: "created tag a".into(),
            content: vec![],
        }),
    ));
    // call-b's decision now lands *after* the flush — orphaned from the
    // message it's actually deciding.
    records.push(rec(
        &s,
        inm(InMsg::Approve {
            session: s.clone(),
            request_id: "call-b".into(),
            scope: Default::default(),
        }),
    ));
    records.push(rec(
        &s,
        out(OutEvent::ToolOutput {
            session: s.clone(),
            seq: 5,
            request_id: "call-b".into(),
            tool: "create_tag".into(),
            output: "created tag b".into(),
            content: vec![],
        }),
    ));

    let projected = project(&records);
    let assistant_msg = &projected[0];
    assert_eq!(assistant_msg.role, "assistant");
    // `decisions` only ever recorded call-a — call-b's approval was orphaned,
    // exactly the gap `resolved` exists to cover.
    assert_eq!(
        assistant_msg.content["decisions"],
        json!([{ "tool_call_id": "call-a", "approve": true }])
    );
    let tool_calls = assistant_msg.content["tool_calls"].as_array().unwrap();
    for tc in tool_calls {
        assert_eq!(
            tc["resolved"],
            json!(true),
            "both calls have their own tool_result by now, so both must be \
             resolved regardless of `decisions`: {projected:#?}"
        );
    }
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
