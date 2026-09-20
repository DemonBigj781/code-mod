use crossterm::event::{KeyCode, KeyEvent};

use crate::app_event::{AgentUpdateIntent, AppEvent};
use crate::components::form_text_field::FormTextField;

use super::model::{
    FIELD_CANCEL,
    FIELD_COMMAND,
    FIELD_DESCRIPTION,
    FIELD_INSTRUCTIONS,
    FIELD_NAME,
    FIELD_READ_ONLY,
    FIELD_SAVE,
    FIELD_TOGGLE,
    FIELD_WRITE,
};
use super::AgentEditorView;

impl AgentEditorView {
    fn persist_current_agent(&mut self, require_description: bool) -> bool {
        if self.simple_model_mode {
            let provider = self.name_field.text().trim();
            let model = self.command_field.text().trim();
            self.name_error = if provider.is_empty() {
                Some("Provider is required.".to_owned())
            } else if provider.chars().any(char::is_whitespace) {
                Some("Provider cannot contain whitespace.".to_owned())
            } else {
                None
            };
            self.command_error = if model.is_empty() {
                Some("Model slug is required.".to_owned())
            } else if model.chars().any(char::is_whitespace) {
                Some("Model slug cannot contain whitespace.".to_owned())
            } else {
                None
            };
            if self.name_error.is_some() || self.command_error.is_some() {
                return false;
            }
            self.app_event_tx.send(AppEvent::UpdateAgentConfig {
                intent: AgentUpdateIntent::CreateModel,
                name: format!("{provider}/{model}"),
                enabled: self.enabled,
                args_read_only: None,
                args_write: None,
                instructions: None,
                description: None,
                command: format!("coder --model {model} -c model_provider={provider}"),
            });
            return true;
        }

        let ro = self
            .params_ro
            .text()
            .split_whitespace()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let wr = self
            .params_wr
            .text()
            .split_whitespace()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let ro_opt = if ro.is_empty() { None } else { Some(ro) };
        let wr_opt = if wr.is_empty() { None } else { Some(wr) };
        let instr_opt = {
            let t = self.instr.text().trim().to_owned();
            if t.is_empty() { None } else { Some(t) }
        };
        let desc_opt = {
            let t = self.description_field.text().trim().to_owned();
            if t.is_empty() {
                if require_description {
                    self.description_error =
                        Some("Describe what this agent is good at before saving.".to_owned());
                    return false;
                }
                self.description_error = None;
                None
            } else {
                self.description_error = None;
                Some(t)
            }
        };

        let trimmed_name = self.name_field.text().trim();
        if self.name_editable && trimmed_name.is_empty() {
            self.name_error = Some("Agent ID is required.".to_owned());
            return false;
        }
        self.name_error = None;
        let final_name = if trimmed_name.is_empty() {
            self.name.clone()
        } else {
            trimmed_name.to_owned()
        };
        let command_value = self.command_field.text().trim();
        let final_command = if command_value.is_empty() {
            self.command.clone()
        } else {
            command_value.to_owned()
        };
        self.app_event_tx.send(AppEvent::UpdateAgentConfig {
            intent: AgentUpdateIntent::Edit,
            name: final_name,
            enabled: self.enabled,
            args_read_only: ro_opt,
            args_write: wr_opt,
            instructions: instr_opt,
            description: desc_opt,
            command: final_command,
        });
        true
    }

    fn paste_into_field(field: &mut FormTextField, text: &str) -> bool {
        let before = field.text().len();
        field.handle_paste(text.to_owned());
        field.text().len() != before
    }

    pub(super) fn paste_into_current_field(&mut self, text: &str) -> bool {
        match self.field {
            FIELD_NAME => Self::paste_into_field(&mut self.name_field, text),
            FIELD_COMMAND => Self::paste_into_field(&mut self.command_field, text),
            FIELD_READ_ONLY => Self::paste_into_field(&mut self.params_ro, text),
            FIELD_WRITE => Self::paste_into_field(&mut self.params_wr, text),
            FIELD_DESCRIPTION => Self::paste_into_field(&mut self.description_field, text),
            FIELD_INSTRUCTIONS => Self::paste_into_field(&mut self.instr, text),
            _ => false,
        }
    }

    pub(super) fn handle_key_internal(&mut self, key_event: KeyEvent) -> bool {
        let last_field_idx = FIELD_CANCEL;
        if self.simple_model_mode
            && matches!(key_event.code, KeyCode::Char('a' | 'A'))
            && self.field != FIELD_NAME
            && self.field != FIELD_COMMAND
        {
            let provider = self.name_field.text().trim().to_owned();
            let model = self.command_field.text().trim().to_owned();
            if !provider.is_empty() && !model.is_empty() {
                self.name_field.set_text(&format!("{provider}/{model}"));
                self.command_field.set_text(&format!(
                    "coder --model {model} -c model_provider={provider}"
                ));
            }
            self.simple_model_mode = false;
            self.name_error = None;
            self.command_error = None;
            self.field = FIELD_NAME;
            return true;
        }
        match key_event {
            KeyEvent {
                code: KeyCode::Esc,
                ..
            } => {
                self.complete = true;
                self.app_event_tx.send(AppEvent::ShowAgentsOverview);
                true
            }
            KeyEvent {
                code: KeyCode::Tab | KeyCode::Down,
                ..
            } => {
                self.field = if self.simple_model_mode {
                    match self.field {
                        FIELD_NAME => FIELD_COMMAND,
                        FIELD_COMMAND => FIELD_TOGGLE,
                        FIELD_TOGGLE => FIELD_SAVE,
                        FIELD_SAVE => FIELD_CANCEL,
                        _ => FIELD_CANCEL,
                    }
                } else {
                    (self.field + 1).min(last_field_idx)
                };
                true
            }
            KeyEvent {
                code: KeyCode::BackTab | KeyCode::Up,
                ..
            } => {
                self.field = if self.simple_model_mode {
                    match self.field {
                        FIELD_CANCEL => FIELD_SAVE,
                        FIELD_SAVE => FIELD_TOGGLE,
                        FIELD_TOGGLE => FIELD_COMMAND,
                        FIELD_COMMAND => FIELD_NAME,
                        _ => FIELD_NAME,
                    }
                } else {
                    self.field.saturating_sub(1)
                };
                true
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            } if self.field == FIELD_TOGGLE => {
                self.enabled = true;
                if !self.simple_model_mode {
                    let _ = self.persist_current_agent(false);
                }
                true
            }
            KeyEvent {
                code: KeyCode::Right,
                ..
            } if self.field == FIELD_TOGGLE => {
                self.enabled = false;
                if !self.simple_model_mode {
                    let _ = self.persist_current_agent(false);
                }
                true
            }
            KeyEvent {
                code: KeyCode::Char(' '),
                ..
            } if self.field == FIELD_TOGGLE => {
                self.enabled = !self.enabled;
                if !self.simple_model_mode {
                    let _ = self.persist_current_agent(false);
                }
                true
            }
            ev @ KeyEvent { .. } if self.field == FIELD_NAME => {
                if self.name_editable {
                    let _ = self.name_field.handle_key(ev);
                }
                true
            }
            ev @ KeyEvent { .. } if self.field == FIELD_COMMAND => {
                let _ = self.command_field.handle_key(ev);
                self.command_error = None;
                true
            }
            ev @ KeyEvent { .. } if self.field == FIELD_READ_ONLY => {
                let _ = self.params_ro.handle_key(ev);
                true
            }
            ev @ KeyEvent { .. } if self.field == FIELD_WRITE => {
                let _ = self.params_wr.handle_key(ev);
                true
            }
            ev @ KeyEvent { .. } if self.field == FIELD_DESCRIPTION => {
                let _ = self.description_field.handle_key(ev);
                self.description_error = None;
                true
            }
            ev @ KeyEvent { .. } if self.field == FIELD_INSTRUCTIONS => {
                let _ = self.instr.handle_key(ev);
                true
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } if self.field == FIELD_SAVE => {
                if self.persist_current_agent(true) {
                    self.complete = true;
                    self.app_event_tx.send(AppEvent::ShowAgentsOverview);
                } else {
                    self.field = if self.name_error.is_some() {
                        FIELD_NAME
                    } else if self.command_error.is_some() {
                        FIELD_COMMAND
                    } else {
                        FIELD_DESCRIPTION
                    };
                }
                true
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } if self.field == FIELD_CANCEL => {
                self.complete = true;
                self.app_event_tx.send(AppEvent::ShowAgentsOverview);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn handle_key_event_direct(&mut self, key_event: KeyEvent) -> bool {
        self.handle_key_internal(key_event)
    }
}
