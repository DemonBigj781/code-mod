use code_core::ssh_registry::{AuthMethod, Credentials, resolve_key_path};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Widget, Wrap};
use std::path::PathBuf;

use crate::bottom_pane::{BottomPane, BottomPaneView, CancellationEvent, ConditionalUpdate};
use crate::components::form_text_field::FormTextField;
use crate::sensitive_input::SensitiveInputGuard;

pub(crate) struct SshCredentialView {
    name: String,
    method: AuthMethod,
    cwd: PathBuf,
    value: FormTextField,
    passphrase: FormTextField,
    selected: usize,
    error: Option<String>,
    complete: bool,
    submit: Option<Box<dyn FnOnce(Credentials) + Send>>,
    _privacy: SensitiveInputGuard,
}

impl SshCredentialView {
    pub(crate) fn new(
        name: String,
        method: AuthMethod,
        cwd: PathBuf,
        submit: impl FnOnce(Credentials) + Send + 'static,
    ) -> Self {
        Self {
            name,
            method,
            cwd,
            value: FormTextField::new_single_line(),
            passphrase: FormTextField::new_single_line(),
            selected: 0,
            error: None,
            complete: false,
            submit: Some(Box::new(submit)),
            _privacy: SensitiveInputGuard::new(),
        }
    }

    fn field(&mut self) -> &mut FormTextField {
        if self.selected == 0 {
            &mut self.value
        } else {
            &mut self.passphrase
        }
    }

    fn save(&mut self) {
        let credentials = match self.method {
            AuthMethod::Password => {
                if self.value.text().is_empty() {
                    self.error = Some("Enter a password, or Esc to cancel.".into());
                    return;
                }
                Credentials::Password(self.value.text().to_owned())
            }
            AuthMethod::Key => {
                let path = match resolve_key_path(self.value.text(), &self.cwd) {
                    Ok(path) => path,
                    Err(error) => {
                        self.error = Some(error.to_string());
                        return;
                    }
                };
                let passphrase =
                    (!self.passphrase.text().is_empty()).then(|| self.passphrase.text().to_owned());
                if path.is_none() && passphrase.is_some() {
                    self.error = Some("Leave passphrase empty when using the SSH agent.".into());
                    return;
                }
                Credentials::Key { path, passphrase }
            }
        };
        if let Some(submit) = self.submit.take() {
            submit(credentials);
        }
        self.complete = true;
    }

    pub(crate) fn key(&mut self, key: KeyEvent) {
        if self.complete {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.complete = true;
                self.submit.take();
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => self.save(),
            KeyCode::Tab | KeyCode::BackTab if self.method == AuthMethod::Key => {
                self.selected = 1 - self.selected;
            }
            KeyCode::Tab | KeyCode::BackTab => {}
            KeyCode::Enter if self.method == AuthMethod::Key && self.selected == 0 => {
                self.selected = 1;
            }
            KeyCode::Enter => self.save(),
            _ => {
                if matches!(key.code, KeyCode::Char(c) if self.field().text().len() + c.len_utf8() > 4096)
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    self.error = Some("Credential input is limited to 4096 bytes.".into());
                    return;
                }
                self.field().handle_key(key);
            }
        }
    }

    pub(crate) fn paste(&mut self, text: String) {
        if text.chars().any(char::is_control) || self.field().text().len() + text.len() > 4096 {
            self.error = Some("Paste must be one line and at most 4096 bytes.".into());
            return;
        }
        self.field().handle_paste(text);
    }
}

impl BottomPaneView<'_> for SshCredentialView {
    fn handle_key_event(&mut self, _: &mut BottomPane<'_>, event: KeyEvent) {
        self.key(event);
    }
    fn handle_paste(&mut self, text: String) -> ConditionalUpdate {
        self.paste(text);
        ConditionalUpdate::NeedsRedraw
    }
    fn on_ctrl_c(&mut self, _: &mut BottomPane<'_>) -> CancellationEvent {
        self.complete = true;
        self.submit.take();
        CancellationEvent::Handled
    }
    fn is_complete(&self) -> bool {
        self.complete
    }
    fn desired_height(&self, _: u16) -> u16 {
        9
    }
    fn render(&self, area: Rect, buffer: &mut Buffer) {
        Clear.render(area, buffer);
        let title = Line::styled(
            format!("SSH credentials — {}", self.name),
            Style::default().add_modifier(Modifier::BOLD),
        );
        let mask = |value: &str| {
            if value.is_empty() {
                "(empty)".to_owned()
            } else {
                "********".to_owned()
            }
        };
        let mut lines = vec![
            title,
            Line::from("Credentials stay local; never sent to the AI or saved in the registry."),
            Line::from(""),
        ];
        match self.method {
            AuthMethod::Password => lines.push(Line::from(format!(
                "> Password: {}",
                mask(self.value.text())
            ))),
            AuthMethod::Key => {
                lines.push(Line::from(format!(
                    "{} Key file: {}",
                    if self.selected == 0 { ">" } else { " " },
                    if self.value.text().is_empty() {
                        "(empty = SSH agent)"
                    } else {
                        self.value.text()
                    }
                )));
                lines.push(Line::from(format!(
                    "{} Passphrase: {}",
                    if self.selected == 1 { ">" } else { " " },
                    mask(self.passphrase.text())
                )));
            }
        }
        lines.push(Line::from(self.error.clone().unwrap_or_default()));
        lines.push(Line::from(
            "Enter / Ctrl+S save • Tab next field • Esc cancel",
        ));
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn ssh_password_preview_is_masked_and_cancel_does_not_submit() {
        let submitted = Arc::new(Mutex::new(false));
        let capture = submitted.clone();
        let mut view = SshCredentialView::new(
            "example".into(),
            AuthMethod::Password,
            PathBuf::from("/"),
            move |_| {
                *capture.lock().unwrap() = true;
            },
        );
        view.paste("test-only-password-123".into());
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 10));
        view.render(buffer.area, &mut buffer);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!text.contains("test-only-password"));
        assert!(text.contains("********"));
        view.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(view.is_complete());
        assert!(!*submitted.lock().unwrap());
    }

    #[test]
    fn ssh_credential_input_preserves_password_spaces_and_rejects_multiline_paste() {
        let result = Arc::new(Mutex::new(None));
        let capture = result.clone();
        let mut view = SshCredentialView::new(
            "example".into(),
            AuthMethod::Password,
            PathBuf::from("/"),
            move |credentials| {
                if let Credentials::Password(value) = credentials {
                    *capture.lock().unwrap() = Some(value);
                }
            },
        );
        view.paste("bad\ninput".into());
        assert!(view.value.text().is_empty());
        view.paste(" password with spaces ".into());
        view.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            result.lock().unwrap().as_deref(),
            Some(" password with spaces ")
        );
    }
}
