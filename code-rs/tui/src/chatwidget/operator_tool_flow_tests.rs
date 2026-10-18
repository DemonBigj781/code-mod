use super::*;
use crate::chatwidget::smoke_helpers::ChatWidgetHarness;

#[test]
fn operator_tool_output_is_a_notice_and_renders_without_an_assistant_turn() {
    let mut harness = ChatWidgetHarness::new();
    let before = harness.history_len();
    harness.handle_event(Event {
        id: "operator-list".into(),
        event_seq: 0,
        order: None,
        msg: EventMsg::OperatorToolResponse {
            message: "Tool inventory: 1 entry\nlist_dir — native — JSON arguments".into(),
            success: Some(true),
        },
    });
    assert_eq!(harness.history_len(), before + 1);
    assert!(
        !harness
            .drain_events()
            .iter()
            .any(|event| matches!(event, AppEvent::InsertHistory(_)))
    );
    let output = crate::test_helpers::render_chat_widget_to_vt100(&mut harness, 100, 30);
    assert!(output.contains("Tool inventory"), "{output}");
    assert!(output.contains("list_dir"), "{output}");
}

#[test]
fn operator_tool_invalid_json_is_a_local_notice() {
    let mut harness = ChatWidgetHarness::new();
    harness
        .chat()
        .handle_tool_command("use shell {invalid-json}".into());
    let output = crate::test_helpers::render_chat_widget_to_vt100(&mut harness, 100, 30);
    assert!(output.contains("Invalid JSON arguments"), "{output}");
    assert!(
        !output.contains("invalid-json"),
        "do not echo malformed argument contents"
    );
}

#[test]
fn operator_tool_command_submits_exact_arguments_without_user_input() {
    let mut harness = ChatWidgetHarness::new();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    harness.chat().code_op_tx = sender;
    harness.chat().handle_tool_command("list".into());
    assert!(matches!(receiver.try_recv().unwrap(), Op::ListTools));
    harness.chat().handle_tool_command(r#"use shell {"command":["printf","two words"]}"#.into());
    let Op::InvokeTool { name, arguments } = receiver.try_recv().unwrap() else {
        panic!("operator command must submit direct invocation, never user input");
    };
    assert_eq!(name, "shell");
    assert_eq!(arguments, Some(serde_json::json!({"command":["printf","two words"]})));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn operator_tool_lifecycle_does_not_start_automatic_review_or_followup() {
    let mut harness = ChatWidgetHarness::new();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    harness.chat().code_op_tx = sender;
    harness.chat().config.tui.auto_review_enabled = true;
    harness.handle_event(Event {
        id: "operator-run".into(), event_seq: 0, order: None,
        msg: EventMsg::TaskLifecycle(code_core::protocol::TaskLifecycleEvent {
            phase: code_core::protocol::TaskLifecyclePhase::Started,
            origin: code_core::protocol::TaskOriginKind::OperatorTool,
            visible_to_user: true,
            last_agent_message: None,
        }),
    });
    harness.handle_event(Event {
        id: "operator-run".into(), event_seq: 1, order: None, msg: EventMsg::TaskStarted,
    });
    assert_eq!(harness.chat().current_turn_origin, Some(TurnOrigin::OperatorTool));
    harness.chat().turn_had_code_edits = true;
    harness.handle_event(Event {
        id: "operator-run".into(), event_seq: 2, order: None,
        msg: EventMsg::TaskComplete(code_core::protocol::TaskCompleteEvent { last_agent_message: None }),
    });
    assert!(harness.chat().background_review.is_none());
    assert!(harness.chat().auto_review_baseline.is_none());
    assert!(harness.chat().active_task_ids.is_empty());
    assert!(receiver.try_recv().is_err(), "operator completion must not submit model work");
}
