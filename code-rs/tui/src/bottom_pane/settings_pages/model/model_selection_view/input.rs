use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::components::mode_guard::ModeGuard;
use crate::app_event::AppEvent;

use super::endpoint_form::{
    ENDPOINT_FORM_API_KEY_ROW, ENDPOINT_FORM_BASE_URL_ROW, ENDPOINT_FORM_CANCEL_ROW,
    ENDPOINT_FORM_DISPLAY_NAME_ROW, ENDPOINT_FORM_SAVE_ROW, ENDPOINT_FORM_WIRE_API_ROW,
};
use super::{EditTarget, ModelSelectionView, ViewMode};
use super::super::model_selection_state::EntryKind;

impl ModelSelectionView {
    pub(crate) fn handle_key_event_direct(&mut self, key_event: KeyEvent) -> bool {
        let app_event_tx = self.app_event_tx.clone();
        let mut mode_guard = ModeGuard::replace(&mut self.mode, ViewMode::Transition, |mode| {
            matches!(mode, ViewMode::Transition)
        });

        match mode_guard.mode_mut() {
            ViewMode::Main => self.handle_key_event_main(key_event),
            ViewMode::Edit {
                target,
                field,
                error,
            } => match (key_event.code, key_event.modifiers) {
                (KeyCode::Esc, _) => {
                    self.mode = ViewMode::Main;
                    true
                }
                (KeyCode::Enter, _) => {
                    match self.save_edit_value(*target, field.text()) {
                        Ok(()) => {
                            self.mode = ViewMode::Main;
                        }
                        Err(message) => {
                            *error = Some(message);
                        }
                    }
                    true
                }
                (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                    match self.save_edit_value(*target, field.text()) {
                        Ok(()) => {
                            self.mode = ViewMode::Main;
                            if self.data.target.supports_context_mode() {
                                self.app_event_tx.send(AppEvent::PersistSessionContextSettings {
                                    context_window: self.data.current.current_context_window,
                                    auto_compact_token_limit: self
                                        .data
                                        .current
                                        .current_auto_compact_token_limit,
                                });
                            }
                        }
                        Err(message) => {
                            *error = Some(message);
                        }
                    }
                    true
                }
                _ => {
                    *error = None;
                    field.handle_key(key_event)
                }
            },
            ViewMode::AddDirectProvider(form) => {
                if matches!(key_event.code, KeyCode::Esc) {
                    self.mode = ViewMode::Main;
                    return true;
                }
                if form.is_submitting() {
                    return true;
                }

                let is_ctrl_s = key_event.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key_event.code, KeyCode::Char('s' | 'S'));
                if is_ctrl_s {
                    return Self::submit_direct_provider_form_state(&app_event_tx, form);
                }

                match key_event.code {
                    KeyCode::Tab => {
                        form.select_next_row();
                        true
                    }
                    KeyCode::BackTab => {
                        form.select_previous_row();
                        true
                    }
                    KeyCode::Left | KeyCode::Right
                        if form.selected_row() == ENDPOINT_FORM_WIRE_API_ROW =>
                    {
                        form.toggle_wire_api();
                        true
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => match form.selected_row() {
                        ENDPOINT_FORM_DISPLAY_NAME_ROW
                        | ENDPOINT_FORM_BASE_URL_ROW
                        | ENDPOINT_FORM_API_KEY_ROW => {
                            form.select_next_row();
                            true
                        }
                        ENDPOINT_FORM_WIRE_API_ROW => {
                            form.toggle_wire_api();
                            form.select_next_row();
                            true
                        }
                        ENDPOINT_FORM_SAVE_ROW => {
                            Self::submit_direct_provider_form_state(&app_event_tx, form)
                        }
                        ENDPOINT_FORM_CANCEL_ROW => {
                            self.mode = ViewMode::Main;
                            true
                        }
                        _ => false,
                    },
                    _ => {
                        form.clear_error();
                        form.selected_field_mut()
                            .is_some_and(|field| field.handle_key(key_event))
                    }
                }
            }
            ViewMode::Transition => {
                self.mode = ViewMode::Main;
                false
            }
        }
    }

    fn handle_key_event_main(&mut self, key_event: KeyEvent) -> bool {
        let selected_entry = self.data.entry_at(self.selected_index);
        match key_event {
            KeyEvent {
                code: KeyCode::Char('s'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                if !self.data.target.supports_context_mode() {
                    return false;
                }
                self.app_event_tx.send(AppEvent::PersistSessionContextSettings {
                    context_window: self.data.current.current_context_window,
                    auto_compact_token_limit: self.data.current.current_auto_compact_token_limit,
                });
                true
            }
            KeyEvent {
                code: KeyCode::Up | KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.move_selection_up();
                true
            }
            KeyEvent {
                code: KeyCode::Down | KeyCode::Char('j'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.move_selection_down();
                true
            }
            KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::NONE,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('-'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.adjust_selected_numeric_value(-1),
            KeyEvent { code: KeyCode::Right, modifiers: KeyModifiers::NONE, .. } |
KeyEvent {
code: KeyCode::Char('+' | '='),
modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT, .. } => self.adjust_selected_numeric_value(1),
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers::NONE,
                ..
            } if c.is_ascii_digit() => {
                let edit_target = match selected_entry {
                    Some(EntryKind::ContextWindow) => Some(EditTarget::ContextWindow),
                    Some(EntryKind::AutoCompact) => Some(EditTarget::AutoCompact),
                    _ => None,
                };
                if let Some(target) = edit_target {
                    self.open_edit_for(target, true);
                    if let ViewMode::Edit { field, .. } = &mut self.mode {
                        let _ = field.handle_key(key_event);
                    }
                    true
                } else {
                    false
                }
            }
            KeyEvent {
                code: KeyCode::Backspace,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                let edit_target = match selected_entry {
                    Some(EntryKind::ContextWindow) => Some(EditTarget::ContextWindow),
                    Some(EntryKind::AutoCompact) => Some(EditTarget::AutoCompact),
                    _ => None,
                };
                if let Some(target) = edit_target {
                    self.open_edit_for(target, true);
                    true
                } else {
                    false
                }
            }
            KeyEvent {
                code: KeyCode::Enter | KeyCode::Char(' '),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.confirm_selection();
                true
            }
            KeyEvent {
                code: KeyCode::Esc,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.send_closed(false);
                true
            }
            _ => false,
        }
    }
}
