use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::model::{
    AgentOverviewRow, AgentsGridLayout, AgentsOverviewState, AgentsSettingsContent,
};
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::chatwidget::settings_overlay::SettingsContent;
use code_core::config_types::ModelRole;

fn state(role: ModelRole, enabled: bool) -> AgentsOverviewState {
    AgentsOverviewState {
        rows: vec![AgentOverviewRow {
            name: "provider/model".to_owned(),
            session_enabled: role != ModelRole::Session || enabled,
            subagent_enabled: role != ModelRole::Subagent || enabled,
            review_enabled: role != ModelRole::Review || enabled,
            auto_drive_enabled: role != ModelRole::AutoDrive || enabled,
            installed: true,
            description: Some("Model agent".to_owned()),
            command: "coder --model model -c model_provider=provider".to_owned(),
        }],
        commands: Vec::new(),
        selected: 0,
        selected_role: role,
    }
}

fn toggle_event(role: ModelRole, enabled: bool, key: KeyCode) -> AppEvent {
    let (tx, rx) = mpsc::channel();
    let sender = AppEventSender::new(tx);
    let mut state = state(role, enabled);
    assert!(AgentsSettingsContent::handle_overview_key(
        &mut state,
        KeyEvent::new(key, KeyModifiers::NONE),
        &sender,
    ));
    rx.try_recv().expect("agent update event")
}

#[test]
fn space_toggles_selected_model_role_and_preserves_generated_command() {
    match toggle_event(ModelRole::Review, false, KeyCode::Char(' ')) {
        AppEvent::UpdateModelRole {
            role,
            enabled,
            command,
            ..
        } => {
            assert_eq!(role, ModelRole::Review);
            assert!(enabled);
            assert_eq!(
                command,
                "coder --model model -c model_provider=provider",
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn left_and_right_move_between_capability_columns() {
    let (tx, rx) = mpsc::channel();
    let sender = AppEventSender::new(tx);
    let mut state = state(ModelRole::Session, true);

    assert!(AgentsSettingsContent::handle_overview_key(
        &mut state,
        KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        &sender,
    ));
    assert_eq!(state.selected_role, ModelRole::Subagent);
    assert!(rx.try_recv().is_err());

    assert!(AgentsSettingsContent::handle_overview_key(
        &mut state,
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        &sender,
    ));
    assert_eq!(state.selected_role, ModelRole::Session);
}

#[test]
fn mouse_activation_toggles_the_clicked_role() {
    let (tx, rx) = mpsc::channel();
    let sender = AppEventSender::new(tx);
    let overview = state(ModelRole::AutoDrive, false);
    let area = Rect::new(0, 0, 80, 20);
    let layout = AgentsGridLayout::new(&overview.rows, area.width);
    let role_column = area.x.saturating_add(
        (0..area.width)
            .find(|column| layout.role_at(*column) == Some(ModelRole::AutoDrive))
            .expect("auto drive column"),
    );
    let mut content =
        AgentsSettingsContent::new_overview(overview.rows, Vec::new(), 0, sender);
    assert!(content.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: role_column,
            row: 2,
            modifiers: KeyModifiers::NONE,
        },
        area,
    ));

    assert!(matches!(
        rx.try_iter().last(),
        Some(AppEvent::UpdateModelRole {
            role: ModelRole::AutoDrive,
            enabled: true,
            ..
        }),
    ));
}

#[test]
fn mouse_activation_on_model_name_opens_the_model_editor() {
    let (tx, rx) = mpsc::channel();
    let sender = AppEventSender::new(tx);
    let mut content = AgentsSettingsContent::new_overview(
        state(ModelRole::Review, false).rows,
        Vec::new(),
        0,
        sender,
    );
    let area = Rect::new(0, 0, 80, 20);

    assert!(content.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 2,
            modifiers: KeyModifiers::NONE,
        },
        area,
    ));

    assert!(matches!(
        rx.try_iter().last(),
        Some(AppEvent::ShowAgentEditor { name }) if name == "provider/model",
    ));
}

#[test]
fn overview_renders_universal_capability_headers() {
    let state = state(ModelRole::Session, true);
    let lines = AgentsSettingsContent::build_overview_lines(&state, Some(90));
    let rendered = lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Model"));
    assert!(rendered.contains("Session"));
    assert!(rendered.contains("Sub-agent"));
    assert!(rendered.contains("Review"));
    assert!(rendered.contains("Auto Drive"));
}
