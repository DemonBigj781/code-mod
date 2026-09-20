use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
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
        agents_enabled: true,
        selected: 0,
        selected_role: Some(role),
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
    assert_eq!(state.selected_role, Some(ModelRole::Subagent));
    assert!(rx.try_recv().is_err());

    assert!(AgentsSettingsContent::handle_overview_key(
        &mut state,
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        &sender,
    ));
    assert_eq!(state.selected_role, Some(ModelRole::Session));
}

#[test]
fn e_opens_uninstalled_model_for_editing() {
    let (tx, rx) = mpsc::channel();
    let sender = AppEventSender::new(tx);
    let mut state = state(ModelRole::Session, true);
    state.rows[0].installed = false;

    assert!(AgentsSettingsContent::handle_overview_key(
        &mut state,
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        &sender,
    ));

    assert!(matches!(
        rx.try_recv(),
        Ok(AppEvent::ShowAgentEditor { name }) if name == "provider/model",
    ));
}

#[test]
fn i_requests_guided_install_for_uninstalled_model() {
    let (tx, rx) = mpsc::channel();
    let sender = AppEventSender::new(tx);
    let mut state = state(ModelRole::Session, true);
    state.rows[0].installed = false;

    assert!(AgentsSettingsContent::handle_overview_key(
        &mut state,
        KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
        &sender,
    ));

    assert!(matches!(
        rx.try_recv(),
        Ok(AppEvent::RequestAgentInstall {
            name,
            selected_index: 0,
        }) if name == "provider/model",
    ));
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
        AgentsSettingsContent::new_overview(overview.rows, Vec::new(), true, 0, sender);
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
fn mouse_activation_on_model_name_toggles_all_roles() {
    let (tx, rx) = mpsc::channel();
    let sender = AppEventSender::new(tx);
    let mut content = AgentsSettingsContent::new_overview(
        state(ModelRole::Review, false).rows,
        Vec::new(),
        true,
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
        Some(AppEvent::UpdateAllModelRoles { name, enabled: true, .. })
            if name == "provider/model",
    ));
}

#[test]
fn enter_on_model_name_toggles_all_roles_together() {
    let (tx, rx) = mpsc::channel();
    let sender = AppEventSender::new(tx);
    let mut state = state(ModelRole::Review, false);
    state.selected_role = None;

    assert!(AgentsSettingsContent::handle_overview_key(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &sender,
    ));

    assert!(matches!(
        rx.try_recv(),
        Ok(AppEvent::UpdateAllModelRoles { name, enabled: true, .. })
            if name == "provider/model",
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
    assert!(rendered.contains("Read agents"));
}

#[test]
fn overview_scrolls_the_selected_model_into_a_bounded_viewport() {
    let rows = (0..12)
        .map(|index| AgentOverviewRow {
            name: format!("provider/model-{index}"),
            session_enabled: true,
            subagent_enabled: true,
            review_enabled: true,
            auto_drive_enabled: true,
            installed: true,
            description: None,
            command: "coder".to_owned(),
        })
        .collect::<Vec<_>>();
    let (tx, _rx) = mpsc::channel();
    let content = AgentsSettingsContent::new_overview(
        rows,
        Vec::new(),
        true,
        11,
        AppEventSender::new(tx),
    );
    let area = Rect::new(0, 0, 80, 5);
    let mut buffer = Buffer::empty(area);

    content.render(area, &mut buffer);

    let rendered = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("provider/model-11"),
        "selected model must remain visible:\n{rendered}"
    );
}

#[test]
fn scrolled_overview_mouse_hit_testing_uses_the_visible_model_row() {
    let mut overview = state(ModelRole::Session, true);
    overview.rows = (0..12)
        .map(|index| AgentOverviewRow {
            name: format!("provider/model-{index}"),
            session_enabled: true,
            subagent_enabled: true,
            review_enabled: true,
            auto_drive_enabled: true,
            installed: true,
            description: None,
            command: "coder".to_owned(),
        })
        .collect();
    overview.selected = 11;
    let area = Rect::new(0, 0, 80, 5);
    let visible_selected_row = MouseEvent {
        kind: MouseEventKind::Moved,
        column: 3,
        row: 4,
        modifiers: KeyModifiers::NONE,
    };

    assert_eq!(
        AgentsSettingsContent::overview_selection_at(
            &overview,
            area,
            visible_selected_row,
        ),
        Some(11),
    );
}

#[test]
fn master_switch_toggles_all_read_agents_independently() {
    let (tx, rx) = mpsc::channel();
    let sender = AppEventSender::new(tx);
    let mut state = state(ModelRole::Subagent, true);
    state.selected = state.rows.len();

    assert!(AgentsSettingsContent::handle_overview_key(
        &mut state,
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        &sender,
    ));

    assert!(matches!(
        rx.try_recv(),
        Ok(AppEvent::UpdateSubagentsEnabled { enabled: false }),
    ));
}
