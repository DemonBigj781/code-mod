use std::cell::Cell;
use std::path::PathBuf;

use code_core::ssh_registry::{
    AuthMethod, Connection, Registry, parse_endpoints, validate_connection_name,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Widget, Wrap};

use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::panes::ssh_credentials::SshCredentialView;
use crate::bottom_pane::{BottomPane, BottomPaneView, CancellationEvent, ConditionalUpdate};
use crate::components::form_text_field::FormTextField;
use crate::ssh_registry::{describe, run_registry_job};

struct EditForm {
    original: Option<Connection>,
    name: FormTextField,
    endpoints: FormTextField,
    auth: AuthMethod,
    focus: usize,
}

enum Mode {
    List,
    Edit(EditForm),
    Credentials(Box<SshCredentialView>),
    Remove(Connection),
}

pub(crate) struct SshSettingsView {
    registry: Registry,
    cwd: PathBuf,
    tx: AppEventSender,
    connections: Vec<Connection>,
    selected: usize,
    mode: Mode,
    status: String,
    error: Option<String>,
    complete: bool,
    list_area: Cell<Rect>,
    list_start: Cell<usize>,
}

impl SshSettingsView {
    pub(crate) fn new(home: PathBuf, cwd: PathBuf, tx: AppEventSender) -> Self {
        let mut view = Self {
            registry: Registry::new(&home),
            cwd,
            tx,
            connections: Vec::new(),
            selected: 0,
            mode: Mode::List,
            status: String::new(),
            error: None,
            complete: false,
            list_area: Cell::new(Rect::default()),
            list_start: Cell::new(0),
        };
        view.reload();
        view
    }

    fn reload(&mut self) {
        let selected = self.connections.get(self.selected).map(|c| c.id);
        match self.registry.list() {
            Ok(connections) => {
                self.connections = connections;
                self.selected = selected
                    .and_then(|id| self.connections.iter().position(|c| c.id == id))
                    .unwrap_or(self.selected)
                    .min(self.connections.len().saturating_sub(1));
                self.error = None;
            }
            Err(error) => {
                self.connections.clear();
                self.selected = 0;
                self.error = Some(error.to_string());
            }
        }
    }

    pub(crate) fn apply_result(&mut self, result: &Result<Vec<String>, String>) {
        self.reload();
        match result {
            Ok(lines) => self.status = lines.join(" "),
            Err(error) => self.error = Some(error.clone()),
        }
    }

    pub(crate) fn has_back_navigation(&self) -> bool {
        !matches!(self.mode, Mode::List)
    }

    pub(crate) fn cancel_edit(&mut self) {
        self.mode = Mode::List;
    }

    fn open_edit(&mut self, original: Option<Connection>) {
        let mut name = FormTextField::new_single_line();
        let mut endpoints = FormTextField::new_single_line();
        if let Some(connection) = &original {
            name.set_text(&connection.name);
            endpoints.set_text(
                &connection
                    .endpoints
                    .iter()
                    .map(|e| e.display())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        self.mode = Mode::Edit(EditForm {
            original,
            name,
            endpoints,
            auth: AuthMethod::Key,
            focus: 0,
        });
        self.error = None;
        self.status.clear();
    }

    fn save_edit(&mut self) {
        let Mode::Edit(form) = &self.mode else {
            return;
        };
        let name = form.name.text().to_owned();
        if let Err(error) = validate_connection_name(&name) {
            self.error = Some(error.to_string());
            return;
        }
        let endpoints = match parse_endpoints(form.endpoints.text()) {
            Ok(endpoints) => endpoints,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let registry = self.registry.clone();
        let tx = self.tx.clone();
        if let Some(expected) = form.original.clone() {
            run_registry_job(tx, move || {
                registry.edit(&expected, &name, endpoints)?;
                Ok(vec![format!(
                    "Updated {name}; connection identity and credentials preserved."
                )])
            });
            self.mode = Mode::List;
        } else {
            self.mode = Mode::Credentials(Box::new(SshCredentialView::new(
                name.clone(),
                form.auth,
                self.cwd.clone(),
                move |credentials| {
                    run_registry_job(tx, move || {
                        let connection = registry.add(&name, endpoints, credentials)?;
                        Ok(vec![format!(
                            "Registered {}. No connection attempted.",
                            describe(&connection)
                        )])
                    });
                },
            )));
        }
        self.error = None;
    }

    fn open_auth(&mut self, method: AuthMethod) {
        let Some(expected) = self.connections.get(self.selected).cloned() else {
            return;
        };
        let name = expected.name.clone();
        let registry = self.registry.clone();
        let tx = self.tx.clone();
        self.mode = Mode::Credentials(Box::new(SshCredentialView::new(
            name.clone(),
            method,
            self.cwd.clone(),
            move |credentials| {
                run_registry_job(tx, move || {
                    registry.set_auth(&expected, credentials)?;
                    Ok(vec![format!("Saved authentication for {name}.")])
                });
            },
        )));
    }

    pub(crate) fn handle_key_direct(&mut self, key: KeyEvent) -> bool {
        if let Mode::Credentials(view) = &mut self.mode {
            view.key(key);
            if view.is_complete() {
                self.mode = Mode::List;
            }
            return true;
        }
        if key.code == KeyCode::Esc {
            if matches!(self.mode, Mode::List) {
                self.complete = true;
            } else {
                self.cancel_edit();
            }
            return true;
        }
        if let Mode::Remove(expected) = &self.mode {
            match key.code {
                KeyCode::Char('y' | 'Y') => {
                    let expected = expected.clone();
                    let registry = self.registry.clone();
                    run_registry_job(self.tx.clone(), move || {
                        registry.remove_expected(&expected)?;
                        Ok(vec![format!(
                            "Removed {} and its saved SSH credentials.",
                            expected.name
                        )])
                    });
                    self.mode = Mode::List;
                }
                KeyCode::Char('n' | 'N') => self.mode = Mode::List,
                _ => {}
            }
            return true;
        }
        if let Mode::Edit(form) = &mut self.mode {
            if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                self.save_edit();
                return true;
            }
            let count = if form.original.is_some() { 2 } else { 3 };
            match key.code {
                KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1) % count,
                KeyCode::BackTab | KeyCode::Up => form.focus = (form.focus + count - 1) % count,
                KeyCode::Enter if form.focus + 1 < count => form.focus += 1,
                KeyCode::Enter => self.save_edit(),
                KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if form.focus == 2 => {
                    form.auth = if form.auth == AuthMethod::Key {
                        AuthMethod::Password
                    } else {
                        AuthMethod::Key
                    };
                }
                _ => {
                    let field = if form.focus == 0 {
                        &mut form.name
                    } else {
                        &mut form.endpoints
                    };
                    if form.focus < 2 {
                        field.handle_key(key);
                    }
                }
            }
            return true;
        }
        match key.code {
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.connections.len().saturating_sub(1))
            }
            KeyCode::Char('n' | 'N') => self.open_edit(None),
            KeyCode::Enter | KeyCode::Char('e' | 'E') => {
                if let Some(connection) = self.connections.get(self.selected).cloned() {
                    self.open_edit(Some(connection));
                }
            }
            KeyCode::Char('p' | 'P') => self.open_auth(AuthMethod::Password),
            KeyCode::Char('k' | 'K') => self.open_auth(AuthMethod::Key),
            KeyCode::Delete | KeyCode::Char('d' | 'D') => {
                if let Some(connection) = self.connections.get(self.selected).cloned() {
                    self.mode = Mode::Remove(connection);
                }
            }
            KeyCode::Char('r' | 'R') => {
                self.reload();
                self.status = "Registry refreshed.".into();
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn paste_direct(&mut self, text: String) -> bool {
        match &mut self.mode {
            Mode::Credentials(view) => {
                view.paste(text);
                true
            }
            Mode::Edit(form) if form.focus < 2 => {
                let field = if form.focus == 0 {
                    &mut form.name
                } else {
                    &mut form.endpoints
                };
                if text.chars().any(char::is_control) || field.text().len() + text.len() > 8192 {
                    self.error = Some("Paste must be one line and at most 8192 bytes.".into());
                } else {
                    field.handle_paste(text);
                }
                true
            }
            _ => false,
        }
    }

    pub(crate) fn mouse_direct(&mut self, event: MouseEvent) -> bool {
        if !matches!(self.mode, Mode::List) {
            return false;
        }
        match event.kind {
            MouseEventKind::ScrollDown => {
                self.selected = (self.selected + 1).min(self.connections.len().saturating_sub(1));
                true
            }
            MouseEventKind::ScrollUp => {
                self.selected = self.selected.saturating_sub(1);
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let area = self.list_area.get();
                if event.column >= area.x
                    && event.column < area.right()
                    && event.row >= area.y
                    && event.row < area.bottom()
                {
                    let index = self.list_start.get() + (event.row - area.y) as usize;
                    if index < self.connections.len() {
                        self.selected = index;
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }
}

impl BottomPaneView<'_> for SshSettingsView {
    fn handle_key_event(&mut self, _: &mut BottomPane<'_>, key: KeyEvent) {
        self.handle_key_direct(key);
    }
    fn handle_paste(&mut self, text: String) -> ConditionalUpdate {
        if self.paste_direct(text) {
            ConditionalUpdate::NeedsRedraw
        } else {
            ConditionalUpdate::NoRedraw
        }
    }
    fn handle_mouse_event(
        &mut self,
        _: &mut BottomPane<'_>,
        event: MouseEvent,
        _: Rect,
    ) -> ConditionalUpdate {
        if self.mouse_direct(event) {
            ConditionalUpdate::NeedsRedraw
        } else {
            ConditionalUpdate::NoRedraw
        }
    }
    fn is_complete(&self) -> bool {
        self.complete
    }
    fn on_ctrl_c(&mut self, _: &mut BottomPane<'_>) -> CancellationEvent {
        self.cancel_edit();
        self.complete = true;
        CancellationEvent::Handled
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
    fn desired_height(&self, _: u16) -> u16 {
        16
    }

    fn render(&self, area: Rect, buffer: &mut Buffer) {
        Clear.render(area, buffer);
        if let Mode::Credentials(view) = &self.mode {
            view.render(area, buffer);
            return;
        }
        let title = Line::styled(
            "SSH • Saved PCs",
            Style::default().add_modifier(Modifier::BOLD),
        );
        if let Mode::Edit(form) = &self.mode {
            let mut lines = vec![
                title,
                Line::from("Metadata only — credentials are entered separately."),
                Line::from(""),
            ];
            lines.push(Line::from(format!(
                "{} Name: {}",
                if form.focus == 0 { ">" } else { " " },
                form.name.text()
            )));
            lines.push(Line::from(format!(
                "{} Endpoints: {}",
                if form.focus == 1 { ">" } else { " " },
                form.endpoints.text()
            )));
            if form.original.is_none() {
                lines.push(Line::from(format!(
                    "{} Authentication: {} (Left/Right)",
                    if form.focus == 2 { ">" } else { " " },
                    form.auth.label()
                )));
            }
            lines.push(Line::from(
                "Example endpoints: user@host,user@alternate:2222",
            ));
            lines.push(Line::from(self.error.clone().unwrap_or_default()));
            lines.push(Line::from("Tab next • Enter / Ctrl+S save • Esc cancel"));
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .render(area, buffer);
            return;
        }
        if let Mode::Remove(connection) = &self.mode {
            Paragraph::new(vec![
                title,
                Line::from(format!("Remove {}?", connection.name)),
                Line::from("This removes the saved profile and its stored credentials."),
                Line::from("Y remove • N / Esc cancel"),
            ])
            .wrap(Wrap { trim: false })
            .render(area, buffer);
            return;
        }
        let top = vec![
            title,
            Line::from("N Add • E Edit • P Password • K Key • D Remove • R Refresh"),
            Line::from(format!(
                "{} saved PC(s) · No network connections made by this menu",
                self.connections.len()
            )),
        ];
        Paragraph::new(top).render(
            Rect::new(area.x, area.y, area.width, area.height.min(3)),
            buffer,
        );
        let list = Rect::new(
            area.x,
            area.y.saturating_add(3),
            area.width,
            area.height.saturating_sub(6),
        );
        self.list_area.set(list);
        let start = self
            .selected
            .saturating_sub((list.height as usize).saturating_sub(1));
        self.list_start.set(start);
        if self.connections.is_empty() {
            Paragraph::new(if self.error.is_some() {
                "Registry unavailable; see error below."
            } else {
                "No PCs registered. Press N to add one."
            })
            .wrap(Wrap { trim: false })
            .render(list, buffer);
        } else {
            let rows = self
                .connections
                .iter()
                .enumerate()
                .skip(start)
                .take(list.height as usize)
                .map(|(index, connection)| {
                    let style = if index == self.selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    Line::styled(describe(connection), style)
                })
                .collect::<Vec<_>>();
            Paragraph::new(rows).render(list, buffer);
        }
        let footer = vec![
            Line::from("Local aliases: self / 127.0.0.1 / 0.0.0.0 · CGNAT endpoints last"),
            Line::from(self.error.as_ref().unwrap_or(&self.status).clone()),
            Line::from("↑↓ select • Enter edit • Esc back"),
        ];
        let height = area.height.min(3);
        Paragraph::new(footer).render(
            Rect::new(
                area.x,
                area.bottom().saturating_sub(height),
                area.width,
                height,
            ),
            buffer,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use code_core::ssh_registry::Credentials;

    fn view() -> (
        tempfile::TempDir,
        SshSettingsView,
        std::sync::mpsc::Receiver<crate::app_event::AppEvent>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let view = SshSettingsView::new(
            directory.path().to_owned(),
            directory.path().to_owned(),
            AppEventSender::new(tx),
        );
        (directory, view, rx)
    }

    #[test]
    fn ssh_settings_empty_state_and_cancel_are_visible_and_non_mutating() {
        let (directory, mut view, _) = view();
        for width in [32, 80, 120] {
            let mut buffer = Buffer::empty(Rect::new(0, 0, width, 16));
            view.render(buffer.area, &mut buffer);
            let text = buffer
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            assert!(text.contains("No PCs registered."));
        }
        view.handle_key_direct(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(view.has_back_navigation());
        view.paste_direct("Example PC".into());
        view.handle_key_direct(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!view.has_back_navigation());
        assert!(!directory.path().join("ssh-connections.json").exists());
    }

    #[test]
    fn ssh_settings_remove_requires_confirmation_and_secret_form_cancels() {
        let (_directory, mut view, _) = view();
        view.registry
            .add(
                "example",
                parse_endpoints("u@192.0.2.1").unwrap(),
                Credentials::Key {
                    path: None,
                    passphrase: None,
                },
            )
            .unwrap();
        view.reload();
        view.handle_key_direct(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(matches!(view.mode, Mode::Remove(_)));
        view.handle_key_direct(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(view.registry.list().unwrap().len(), 1);
        view.open_auth(AuthMethod::Password);
        assert!(crate::sensitive_input::active());
        view.cancel_edit();
        assert!(!view.has_back_navigation());
    }

    #[tokio::test]
    async fn ssh_settings_add_agent_profile_updates_the_dynamic_registry() {
        let (_directory, mut view, receiver) = view();
        view.open_edit(None);
        let Mode::Edit(form) = &mut view.mode else {
            panic!("expected form");
        };
        form.name.set_text("New PC");
        form.endpoints.set_text("u@100.64.0.1,u@192.0.2.2");
        view.save_edit();
        view.handle_key_direct(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(event) = receiver.try_recv() {
                    break event;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let crate::app_event::AppEvent::SshRegistryResult { result } = event else {
            panic!("expected registry result");
        };
        assert!(result.is_ok(), "{result:?}");
        view.apply_result(&result);
        assert_eq!(view.connections.len(), 1);
        assert_eq!(view.connections[0].ordered_endpoints()[0].host, "192.0.2.2");
    }
}
