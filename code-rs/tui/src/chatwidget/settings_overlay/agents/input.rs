use crossterm::event::{KeyCode, KeyEvent};

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;

use super::model::{
    AgentsOverviewState, AgentsSettingsContent, next_model_role, previous_model_role,
};

impl AgentsSettingsContent {
    fn toggle_model_target(
        state: &AgentsOverviewState,
        app_event_tx: &AppEventSender,
    ) -> bool {
        let Some(row) = state.rows.get(state.selected) else {
            return false;
        };
        if let Some(role) = state.selected_role {
            app_event_tx.send(AppEvent::UpdateModelRole {
                name: row.name.clone(),
                role,
                enabled: !row.role_enabled(role),
                description: row.description.clone(),
                command: row.command.clone(),
            });
        } else {
            app_event_tx.send(AppEvent::UpdateAllModelRoles {
                name: row.name.clone(),
                enabled: !row.all_roles_enabled(),
                description: row.description.clone(),
                command: row.command.clone(),
            });
        }
        true
    }

    pub(super) fn handle_overview_key(
        state: &mut AgentsOverviewState,
        key: KeyEvent,
        app_event_tx: &AppEventSender,
    ) -> bool {
        match key.code {
            KeyCode::Up => {
                if state.total_rows() == 0 {
                    return true;
                }
                if state.selected == 0 {
                    state.selected = state.total_rows().saturating_sub(1);
                } else {
                    state.selected -= 1;
                }
                app_event_tx.send(AppEvent::AgentsOverviewSelectionChanged {
                    index: state.selected,
                });
                true
            }
            KeyCode::Down => {
                let total = state.total_rows();
                if total == 0 {
                    return true;
                }
                state.selected = (state.selected + 1) % total;
                app_event_tx.send(AppEvent::AgentsOverviewSelectionChanged {
                    index: state.selected,
                });
                true
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if state.selected < state.rows.len() {
                    state.selected_role = previous_model_role(state.selected_role);
                }
                true
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if state.selected < state.rows.len() {
                    state.selected_role = next_model_role(state.selected_role);
                }
                true
            }
            KeyCode::Char(' ') => {
                if state.selected < state.rows.len() {
                    Self::toggle_model_target(state, app_event_tx);
                } else if state.selected == state.rows.len() {
                    app_event_tx.send(AppEvent::UpdateSubagentsEnabled {
                        enabled: !state.agents_enabled,
                    });
                }
                true
            }
            KeyCode::Char('e' | 'E') => {
                let Some(row) = state.rows.get(state.selected) else {
                    return false;
                };
                app_event_tx.send(AppEvent::ShowAgentEditor {
                    name: row.name.clone(),
                });
                true
            }
            KeyCode::Char('i' | 'I') => {
                let Some(row) = state.rows.get(state.selected) else {
                    return false;
                };
                if row.installed {
                    return false;
                }
                app_event_tx.send(AppEvent::RequestAgentInstall {
                    name: row.name.clone(),
                    selected_index: state.selected,
                });
                true
            }
            KeyCode::Enter => {
                let idx = state.selected;
                let master_idx = state.rows.len();
                let add_agent_idx = master_idx + 1;
                if idx < master_idx {
                    Self::toggle_model_target(state, app_event_tx);
                } else if idx == master_idx {
                    app_event_tx.send(AppEvent::UpdateSubagentsEnabled {
                        enabled: !state.agents_enabled,
                    });
                } else if idx == add_agent_idx {
                    app_event_tx.send(AppEvent::ShowAgentEditorNew);
                } else {
                    let cmd_idx = idx.saturating_sub(state.rows.len() + 2);
                    if cmd_idx < state.commands.len() {
                        if let Some(name) = state.commands.get(cmd_idx) {
                            app_event_tx.send(AppEvent::ShowSubagentEditorForName {
                                name: name.clone(),
                            });
                        }
                    } else {
                        app_event_tx.send(AppEvent::ShowSubagentEditorNew);
                    }
                }
                true
            }
            _ => false,
        }
    }
}
