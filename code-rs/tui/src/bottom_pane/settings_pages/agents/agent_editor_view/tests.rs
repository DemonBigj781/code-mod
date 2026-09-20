use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;

use super::model::{FIELD_COMMAND, FIELD_SAVE, FIELD_TOGGLE};
use super::{AgentEditorInit, AgentEditorView};

#[test]
fn field_order_matches_visual_layout() {
    let (app_event_tx_raw, _app_event_rx) = std::sync::mpsc::channel();
    let app_event_tx = AppEventSender::new(app_event_tx_raw);
    let mut view = AgentEditorView::new(AgentEditorInit {
        name: "coder".to_string(),
        enabled: true,
        args_read_only: None,
        args_write: None,
        instructions: None,
        description: Some("desc".to_string()),
        command: "coder".to_string(),
        builtin: true,
        app_event_tx,
    });

    view.field = FIELD_COMMAND;
    view.handle_key_event_direct(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(view.field, FIELD_TOGGLE);
}

#[test]
fn new_agent_editor_saves_provider_and_model_slug_without_an_executable_form() {
    let (app_event_tx_raw, app_event_rx) = std::sync::mpsc::channel();
    let app_event_tx = AppEventSender::new(app_event_tx_raw);
    let mut view = AgentEditorView::new(AgentEditorInit {
        name: String::new(),
        enabled: true,
        args_read_only: None,
        args_write: None,
        instructions: None,
        description: None,
        command: String::new(),
        builtin: false,
        app_event_tx,
    });
    assert!(view.simple_model_mode);
    view.name_field.set_text("openrouter");
    view.command_field.set_text("vendor/model:free");
    view.field = FIELD_SAVE;

    assert!(view.handle_key_event_direct(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    match app_event_rx.try_recv().expect("agent update") {
        AppEvent::UpdateAgentConfig { name, command, .. } => {
            assert_eq!(name, "openrouter/vendor/model:free");
            assert_eq!(
                command,
                "coder --model vendor/model:free -c model_provider=openrouter",
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn new_agent_editor_can_switch_to_the_advanced_executable_form() {
    let (app_event_tx_raw, _app_event_rx) = std::sync::mpsc::channel();
    let app_event_tx = AppEventSender::new(app_event_tx_raw);
    let mut view = AgentEditorView::new(AgentEditorInit {
        name: String::new(),
        enabled: true,
        args_read_only: None,
        args_write: None,
        instructions: None,
        description: None,
        command: String::new(),
        builtin: false,
        app_event_tx,
    });
    view.field = FIELD_TOGGLE;

    assert!(view.handle_key_event_direct(KeyEvent::new(
        KeyCode::Char('a'),
        KeyModifiers::NONE,
    )));
    assert!(!view.simple_model_mode);
}
