use super::*;
use entanglement_core::SessionId;
use entanglement_provider::ContentPart;

fn rec(session: &SessionId, payload: LogPayload) -> LogRecord {
    LogRecord {
        ts: 0,
        session: session.clone(),
        payload,
    }
}

fn out(ev: OutEvent) -> LogPayload {
    LogPayload::Out(ev)
}
fn inm(msg: InMsg) -> LogPayload {
    LogPayload::In(msg)
}

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

/// A root turn spawns a `researcher` sub-agent via `agent_spawn`; the
/// child's own record sequence (interleaved in the same root-session log,
/// per `persistence.rs`) folds into a nested `sub_agents` entry on the
/// spawning assistant message instead of polluting the root's own turn.
#[test]
fn sub_agent_turn_nests_under_the_spawning_assistant_message() {
    let root = SessionId::new("u1:root");
    let child = SessionId::new("3fa85f64-5717-4562-b3fc-2c963f66afa6");
    let records = vec![
        rec(
            &root,
            inm(InMsg::Prompt {
                session: root.clone(),
                content: vec![ContentPart::text("research topic X")],
            }),
        ),
        rec(
            &root,
            out(OutEvent::ToolCall {
                session: root.clone(),
                seq: 1,
                request_id: "spawn-1".into(),
                tool: "agent_spawn".into(),
                input: r#"{"agent":"researcher","prompt":"look into X"}"#.into(),
            }),
        ),
        // The child's own session, interleaved in the same log.
        rec(
            &child,
            out(OutEvent::SessionStarted {
                session: child.clone(),
                parent: Some(root.clone()),
                profile: "researcher".into(),
                model: None,
                root: false,
                ts: 0,
            }),
        ),
        rec(
            &child,
            out(OutEvent::TextDelta {
                session: child.clone(),
                seq: 1,
                text: "X is...".into(),
            }),
        ),
        rec(
            &child,
            out(OutEvent::Done {
                session: child.clone(),
                seq: 2,
            }),
        ),
        // Back on the root: the (detached) agent_spawn call's own immediate
        // reply, then the parent turn finishes.
        rec(
            &root,
            out(OutEvent::ToolOutput {
                session: root.clone(),
                seq: 2,
                request_id: "spawn-1".into(),
                tool: "agent_spawn".into(),
                output: format!(
                    "Sub-agent launched under the `researcher` profile. agent_id: {}. Call agent_poll with this agent_id to await its answer.",
                    child.0
                ),
                content: vec![],
            }),
        ),
        rec(
            &root,
            out(OutEvent::TextDelta {
                session: root.clone(),
                seq: 3,
                text: "Researching...".into(),
            }),
        ),
        rec(
            &root,
            out(OutEvent::Done {
                session: root.clone(),
                seq: 4,
            }),
        ),
    ];

    let projected = project(&records);
    // The root's own turn is unpolluted by the child's TextDelta/Done: user,
    // the spawning assistant turn, its tool_result, then the closing text.
    assert_eq!(projected.len(), 4, "{projected:#?}");
    assert_eq!(projected[0].role, "user");

    let spawning_turn = &projected[1];
    assert_eq!(spawning_turn.role, "assistant");
    let sub_agents = spawning_turn.content["sub_agents"]
        .as_array()
        .expect("sub_agents attached to the spawning turn");
    assert_eq!(sub_agents.len(), 1);
    assert_eq!(sub_agents[0]["agent_id"], json!(child.0));
    assert_eq!(sub_agents[0]["profile"], json!("researcher"));
    assert_eq!(sub_agents[0]["task"], json!("look into X"));
    assert_eq!(
        sub_agents[0]["messages"],
        json!([{ "role": "assistant", "content": { "text": "X is...", "tool_calls": [] } }])
    );

    assert_eq!(projected[2].role, "tool_result");
    assert_eq!(projected[3].role, "assistant");
    assert_eq!(projected[3].content["text"], json!("Researching..."));
    assert!(projected[3].content.get("sub_agents").is_none());
}

/// A regression for count/position-based pairing: an earlier `agent_spawn`
/// call is *refused* (no child session ever starts — its tool_result names no
/// uuid) and a later, unrelated call actually spawns one. The real child must
/// attach to the call that actually produced it, never to the refused one
/// merely because it came first in the log.
#[test]
fn a_refused_spawn_does_not_steal_a_later_calls_real_child() {
    let root = SessionId::new("u1:root");
    let child = SessionId::new("3fa85f64-5717-4562-b3fc-2c963f66afa6");
    let records = vec![
        rec(
            &root,
            out(OutEvent::ToolCall {
                session: root.clone(),
                seq: 1,
                request_id: "spawn-refused".into(),
                tool: "agent_spawn".into(),
                input: r#"{"agent":"ghost","prompt":"nope"}"#.into(),
            }),
        ),
        rec(
            &root,
            out(OutEvent::ToolOutput {
                session: root.clone(),
                seq: 2,
                request_id: "spawn-refused".into(),
                tool: "agent_spawn".into(),
                output: "sub-agent spawn refused: unknown agent profile `ghost`.".into(),
                content: vec![],
            }),
        ),
        rec(
            &root,
            out(OutEvent::ToolCall {
                session: root.clone(),
                seq: 3,
                request_id: "spawn-ok".into(),
                tool: "agent_spawn".into(),
                input: r#"{"agent":"researcher","prompt":"look into X"}"#.into(),
            }),
        ),
        rec(
            &child,
            out(OutEvent::SessionStarted {
                session: child.clone(),
                parent: Some(root.clone()),
                profile: "researcher".into(),
                model: None,
                root: false,
                ts: 0,
            }),
        ),
        rec(
            &child,
            out(OutEvent::TextDelta {
                session: child.clone(),
                seq: 1,
                text: "X is...".into(),
            }),
        ),
        rec(
            &child,
            out(OutEvent::Done {
                session: child.clone(),
                seq: 2,
            }),
        ),
        rec(
            &root,
            out(OutEvent::ToolOutput {
                session: root.clone(),
                seq: 4,
                request_id: "spawn-ok".into(),
                tool: "agent_spawn".into(),
                output: format!(
                    "Sub-agent launched under the `researcher` profile. agent_id: {}. Call agent_poll with this agent_id to await its answer.",
                    child.0
                ),
                content: vec![],
            }),
        ),
        rec(
            &root,
            out(OutEvent::Done {
                session: root.clone(),
                seq: 5,
            }),
        ),
    ];

    let projected = project(&records);
    // Each agent_spawn call's own ToolOutput flushes independently (no
    // approval pause to batch them): [assistant(refused), tool_result,
    // assistant(ok), tool_result].
    assert_eq!(projected.len(), 4, "{projected:#?}");
    assert!(
        projected[0].content.get("sub_agents").is_none(),
        "the refused call must not get an (incorrect) sub_agents entry: {:#?}",
        projected[0]
    );
    let ok_turn = &projected[2];
    assert_eq!(ok_turn.role, "assistant");
    let sub_agents = ok_turn.content["sub_agents"]
        .as_array()
        .expect("sub_agents attached to the call that actually spawned one");
    assert_eq!(sub_agents.len(), 1, "{sub_agents:#?}");
    assert_eq!(sub_agents[0]["agent_id"], json!(child.0));
    assert_eq!(sub_agents[0]["task"], json!("look into X"));
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
