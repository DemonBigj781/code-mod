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
