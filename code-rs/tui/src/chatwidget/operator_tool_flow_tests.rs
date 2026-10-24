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
fn failed_direct_queue_dispatch_keeps_prompt_for_retry() {
    let mut harness = ChatWidgetHarness::new();
    let message = UserMessage::from("preserve this prompt".to_owned());
    harness.chat().queued_user_messages.push_back(message.clone());
    harness.chat().queue_agent_note("preserve this note");

    let mut note_send_attempted = false;
    let mut user_input_sent = false;
    harness
        .chat()
        .dispatch_queued_batch_with_sender(vec![message.clone()], |op| match op {
            Op::AddToHistory { text } => {
                note_send_attempted = true;
                Err((Op::AddToHistory { text }, "simulated note send failure".to_owned()))
            }
            Op::UserInput { .. } => {
                user_input_sent = true;
                Ok(())
            }
            other => panic!("unexpected queued dispatch operation: {other:?}"),
        });

    assert!(note_send_attempted);
    assert!(!user_input_sent);

    assert_eq!(harness.chat().queued_user_messages.front(), Some(&message));
    assert_eq!(harness.chat().pending_agent_notes, vec!["preserve this note"]);
    assert_eq!(harness.chat().pending_user_prompts_for_next_turn, 0);
}

#[test]
fn failed_direct_queue_batch_preserves_prompt_order() {
    let mut harness = ChatWidgetHarness::new();
    let first = UserMessage::from("first prompt".to_owned());
    let second = UserMessage::from("second prompt".to_owned());
    harness.chat().queued_user_messages.push_back(first.clone());
    harness.chat().queued_user_messages.push_back(second.clone());

    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    drop(receiver);
    harness.chat().code_op_tx = sender;

    harness
        .chat()
        .dispatch_queued_batch(vec![first.clone(), second.clone()]);

    assert_eq!(
        harness.chat().queued_user_messages.iter().collect::<Vec<_>>(),
        vec![&first, &second]
    );
}

#[test]
fn failed_immediate_hidden_message_is_queued_for_retry() {
    let mut harness = ChatWidgetHarness::new();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    drop(receiver);
    harness.chat().code_op_tx = sender;

    harness
        .chat()
        .submit_hidden_text_message_with_preface("preserve immediate message".to_owned(), String::new());

    assert_eq!(harness.chat().queued_user_messages.len(), 1);
    assert!(matches!(
        harness.chat().queued_user_messages.front(),
        Some(UserMessage { display_text, ordered_items, .. })
            if display_text.is_empty()
                && ordered_items == &vec![InputItem::Text {
                    text: "preserve immediate message".to_owned(),
                }]
    ));
    assert_eq!(harness.chat().pending_user_prompts_for_next_turn, 0);
}

#[test]
fn immediate_hidden_message_flushes_notes_before_prompt() {
    let mut harness = ChatWidgetHarness::new();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    harness.chat().code_op_tx = sender;
    harness.chat().queue_agent_note("send this first");

    harness
        .chat()
        .submit_hidden_text_message_with_preface("send this prompt".to_owned(), String::new());

    assert!(matches!(
        receiver.try_recv().unwrap(),
        Op::AddToHistory { text } if text == "send this first"
    ));
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Op::UserInput { items, .. }
            if items == vec![InputItem::Text {
                text: "send this prompt".to_owned(),
            }]
    ));
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
    while let Ok(op) = receiver.try_recv() {
        assert!(matches!(op, Op::PersistHistorySnapshot { .. }),
            "operator completion may persist history, but must not submit model work: {op:?}");
    }
}
