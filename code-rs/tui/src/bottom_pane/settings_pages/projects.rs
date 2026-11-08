use std::cell::Cell;
use std::path::PathBuf;

use code_core::project_registry::{
    ProjectCommand, ProjectDefinition, ProjectKind, Registry, execute_project_command,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Widget, Wrap};

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::{BottomPane, BottomPaneView, CancellationEvent, ConditionalUpdate};
use crate::components::form_text_field::FormTextField;

struct AddForm {
    kind: ProjectKind,
    path: FormTextField,
    name: FormTextField,
    depth: FormTextField,
    focus: usize,
}

enum Mode {
    List,
    Add(AddForm),
    Remove(ProjectDefinition),
}

pub(crate) struct ProjectsSettingsView {
    registry: Registry,
    cwd: PathBuf,
    tx: AppEventSender,
    reserved_aliases: Vec<String>,
    projects: Vec<ProjectDefinition>,
    active_alias: Option<String>,
    selected: usize,
    mode: Mode,
    status: String,
    error: Option<String>,
    complete: bool,
    list_area: Cell<Rect>,
    list_start: Cell<usize>,
}

impl ProjectsSettingsView {
    pub(crate) fn new(
        home: PathBuf,
        cwd: PathBuf,
        tx: AppEventSender,
        reserved_aliases: Vec<String>,
    ) -> Self {
        let mut view = Self {
            registry: Registry::new(&home),
            cwd,
            tx,
            reserved_aliases,
            projects: Vec::new(),
            active_alias: None,
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
        let selected = self
            .projects
            .get(self.selected)
            .map(ProjectDefinition::alias);
        match (self.registry.list(), self.registry.active()) {
            (Ok(projects), Ok(active)) => {
                self.projects = projects;
                self.active_alias = active.map(|project| project.alias());
                self.selected = selected
                    .and_then(|alias| {
                        self.projects
                            .iter()
                            .position(|project| project.alias() == alias)
                    })
                    .unwrap_or(self.selected)
                    .min(self.projects.len().saturating_sub(1));
                self.error = None;
            }
            (Err(error), _) | (_, Err(error)) => {
                self.projects.clear();
                self.active_alias = None;
                self.selected = 0;
                self.error = Some(error.to_string());
            }
        }
    }

    fn open_add(&mut self) {
        let mut path = FormTextField::new_single_line();
        path.set_text(&self.cwd.display().to_string());
        let mut depth = FormTextField::new_single_line();
        depth.set_text("0");
        self.mode = Mode::Add(AddForm {
            kind: ProjectKind::Single,
            path,
            name: FormTextField::new_single_line(),
            depth,
            focus: 0,
        });
        self.error = None;
        self.status.clear();
    }

    fn execute(&mut self, command: ProjectCommand) {
        match execute_project_command(
            &self.registry,
            &self.cwd,
            command,
            self.reserved_aliases.clone(),
        ) {
            Ok(result) => {
                self.status = result.lines.join(" ");
                self.error = None;
                self.reload();
                self.tx.send(AppEvent::ProjectRegistryChanged(result));
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn save_add(&mut self) {
        let Mode::Add(form) = &self.mode else {
            return;
        };
        let depth = match form.depth.text().parse::<u16>() {
            Ok(depth) => depth,
            Err(_) => {
                self.error = Some("Git search depth must be a non-negative integer.".to_owned());
                return;
            }
        };
        let command = ProjectCommand::Add {
            kind: form.kind,
            directory: PathBuf::from(form.path.text()),
            name: form.name.text().to_owned(),
            git_search_depth: depth,
        };
        self.mode = Mode::List;
        self.execute(command.clone());
        if self.error.is_some() {
            self.open_add_from_command(command);
        }
    }

    fn open_add_from_command(&mut self, command: ProjectCommand) {
        let ProjectCommand::Add {
            kind,
            directory,
            name,
            git_search_depth,
        } = command
        else {
            return;
        };
        let mut path = FormTextField::new_single_line();
        path.set_text(&directory.display().to_string());
        let mut name_field = FormTextField::new_single_line();
        name_field.set_text(&name);
        let mut depth = FormTextField::new_single_line();
        depth.set_text(&git_search_depth.to_string());
        self.mode = Mode::Add(AddForm {
            kind,
            path,
            name: name_field,
            depth,
            focus: 0,
        });
    }

    pub(crate) fn has_back_navigation(&self) -> bool {
        !matches!(self.mode, Mode::List)
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }

    pub(crate) fn cancel_edit(&mut self) {
        self.mode = Mode::List;
    }

    pub(crate) fn handle_key_direct(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Esc {
            if matches!(self.mode, Mode::List) {
                self.complete = true;
            } else {
                self.cancel_edit();
            }
            return true;
        }
        if let Mode::Remove(project) = &self.mode {
            match key.code {
                KeyCode::Char('y' | 'Y') => {
                    let name = project.name.clone();
                    self.mode = Mode::List;
                    self.execute(ProjectCommand::Remove(name));
                }
                KeyCode::Char('n' | 'N') => self.mode = Mode::List,
                _ => {}
            }
            return true;
        }
        if let Mode::Add(form) = &mut self.mode {
            if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                self.save_add();
                return true;
            }
            match key.code {
                KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1) % 4,
                KeyCode::BackTab | KeyCode::Up => form.focus = (form.focus + 3) % 4,
                KeyCode::Enter if form.focus < 3 => form.focus += 1,
                KeyCode::Enter => self.save_add(),
                KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if form.focus == 0 => {
                    form.kind = match form.kind {
                        ProjectKind::Single => ProjectKind::Group,
                        ProjectKind::Group => ProjectKind::Master,
                        ProjectKind::Master => ProjectKind::Single,
                    };
                    if form.kind == ProjectKind::Single {
                        form.depth.set_text("0");
                    }
                }
                _ => match form.focus {
                    1 => form.path.handle_key(key),
                    2 => form.name.handle_key(key),
                    3 => form.depth.handle_key(key),
                    _ => {}
                },
            }
            return true;
        }
        match key.code {
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.projects.len().saturating_sub(1));
            }
            KeyCode::Char('n' | 'N') => self.open_add(),
            KeyCode::Enter | KeyCode::Char('u' | 'U') => {
                if let Some(name) = self
                    .projects
                    .get(self.selected)
                    .map(|project| project.name.clone())
                {
                    self.execute(ProjectCommand::Use(name));
                }
            }
            KeyCode::Delete | KeyCode::Char('d' | 'D') => {
                if let Some(project) = self.projects.get(self.selected).cloned() {
                    self.mode = Mode::Remove(project);
                }
            }
            KeyCode::Char('r' | 'R') => {
                self.reload();
                self.status = "Project registry refreshed.".to_owned();
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn handle_key_event_direct(&mut self, key: KeyEvent) -> bool {
        self.handle_key_direct(key)
    }

    pub(crate) fn paste_direct(&mut self, text: String) -> bool {
        let Mode::Add(form) = &mut self.mode else {
            return false;
        };
        if form.focus == 0 {
            return false;
        }
        if text.chars().any(char::is_control) || text.len() > 8192 {
            self.error = Some("Paste must be one line and at most 8192 bytes.".to_owned());
            return true;
        }
        match form.focus {
            1 => form.path.handle_paste(text),
            2 => form.name.handle_paste(text),
            3 => form.depth.handle_paste(text),
            _ => return false,
        }
        true
    }

    pub(crate) fn handle_paste_direct(&mut self, text: String) -> bool {
        self.paste_direct(text)
    }

    pub(crate) fn mouse_direct(&mut self, event: MouseEvent) -> bool {
        if !matches!(self.mode, Mode::List) {
            return false;
        }
        match event.kind {
            MouseEventKind::ScrollDown => {
                self.selected = (self.selected + 1).min(self.projects.len().saturating_sub(1));
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
                    if index < self.projects.len() {
                        self.selected = index;
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn render_view(&self, area: Rect, buffer: &mut Buffer) {
        Clear.render(area, buffer);
        let title = Line::styled(
            "Projects • Registered scopes",
            Style::default().add_modifier(Modifier::BOLD),
        );
        if let Mode::Add(form) = &self.mode {
            let lines = vec![
                title,
                Line::from("Register a bounded repository scope."),
                Line::from(""),
                Line::from(format!(
                    "{} Kind: {} (Left/Right)",
                    if form.focus == 0 { ">" } else { " " },
                    form.kind.label()
                )),
                Line::from(format!(
                    "{} Path: {}",
                    if form.focus == 1 { ">" } else { " " },
                    form.path.text()
                )),
                Line::from(format!(
                    "{} Name: {}",
                    if form.focus == 2 { ">" } else { " " },
                    form.name.text()
                )),
                Line::from(format!(
                    "{} Git search depth: {}",
                    if form.focus == 3 { ">" } else { " " },
                    form.depth.text()
                )),
                Line::from("Single projects require depth 0; maximum depth is 64."),
                Line::from(self.error.clone().unwrap_or_default()),
                Line::from("Tab next • Enter / Ctrl+S save • Esc cancel"),
            ];
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .render(area, buffer);
            return;
        }
        if let Mode::Remove(project) = &self.mode {
            Paragraph::new(vec![
                title,
                Line::from(format!("Remove project `{}`?", project.name)),
                Line::from(format!(
                    "The directory is not deleted: {}",
                    project.path.display()
                )),
                Line::from("Y remove • N / Esc cancel"),
            ])
            .wrap(Wrap { trim: false })
            .render(area, buffer);
            return;
        }

        Paragraph::new(vec![
            title,
            Line::from("N Add • Enter/U Use • D Remove • R Refresh"),
            Line::from(format!("{} registered project(s)", self.projects.len())),
        ])
        .render(
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
        if self.projects.is_empty() {
            Paragraph::new(if self.error.is_some() {
                "Registry unavailable; see error below."
            } else {
                "No projects registered. Press N to add one."
            })
            .wrap(Wrap { trim: false })
            .render(list, buffer);
        } else {
            let rows = self
                .projects
                .iter()
                .enumerate()
                .skip(start)
                .take(list.height as usize)
                .map(|(index, project)| {
                    let style = if index == self.selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    let active = if self.active_alias.as_deref() == Some(project.alias().as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    Line::styled(
                        format!(
                            "{active} /{} [{} depth={}] {}",
                            project.name,
                            project.kind.label(),
                            project.git_search_depth,
                            project.path.display()
                        ),
                        style,
                    )
                })
                .collect::<Vec<_>>();
            Paragraph::new(rows).render(list, buffer);
        }
        let footer = vec![
            Line::from("* active scope · Enter selects and switches the session directory"),
            Line::from(self.error.as_ref().unwrap_or(&self.status).clone()),
            Line::from("↑↓ select • Esc back"),
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

impl BottomPaneView<'_> for ProjectsSettingsView {
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
        self.render_view(area, buffer);
    }
}

impl crate::bottom_pane::chrome_view::ChromeRenderable for ProjectsSettingsView {
    fn render_in_framed_chrome(&self, area: Rect, buffer: &mut Buffer) {
        self.render_view(area, buffer);
    }

    fn render_in_content_only_chrome(&self, area: Rect, buffer: &mut Buffer) {
        self.render_view(area, buffer);
    }
}

impl crate::bottom_pane::chrome_view::ChromeMouseHandler for ProjectsSettingsView {
    fn handle_mouse_event_direct_in_framed_chrome(
        &mut self,
        event: MouseEvent,
        _area: Rect,
    ) -> bool {
        self.mouse_direct(event)
    }

    fn handle_mouse_event_direct_in_content_only_chrome(
        &mut self,
        event: MouseEvent,
        _area: Rect,
    ) -> bool {
        self.mouse_direct(event)
    }
}

crate::bottom_pane::chrome_view::impl_chrome_view!(ProjectsSettingsView);

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        ProjectsSettingsView,
        std::sync::mpsc::Receiver<AppEvent>,
    ) {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let view = ProjectsSettingsView::new(
            home.path().to_owned(),
            cwd.path().to_owned(),
            AppEventSender::new(tx),
            vec!["help".to_owned()],
        );
        (home, cwd, view, rx)
    }

    #[test]
    fn projects_settings_empty_state_and_cancel_do_not_mutate_registry() {
        let (home, _, mut view, _) = view();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 16));
        view.render_view(buffer.area, &mut buffer);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("No projects registered."));
        view.handle_key_direct(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(view.has_back_navigation());
        view.handle_key_direct(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!view.has_back_navigation());
        assert!(!home.path().join("projects.json").exists());
    }

    #[test]
    fn projects_settings_selection_preserves_an_already_active_scope() {
        let (home, cwd, mut view, rx) = view();
        Registry::new(home.path())
            .add(
                ProjectKind::Single,
                cwd.path(),
                "demo",
                0,
                std::iter::empty::<&str>(),
            )
            .unwrap();
        view.reload();

        view.handle_key_direct(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let AppEvent::ProjectRegistryChanged(result) = rx.recv().unwrap() else {
            panic!("expected project registry event");
        };
        assert!(!result.scope_changed);
        assert!(!result.registry_changed);
        assert_eq!(result.switch_cwd, Some(cwd.path().canonicalize().unwrap()));
    }

    #[test]
    fn projects_settings_remove_requires_confirmation() {
        let (home, cwd, mut view, rx) = view();
        Registry::new(home.path())
            .add(
                ProjectKind::Single,
                cwd.path(),
                "demo",
                0,
                std::iter::empty::<&str>(),
            )
            .unwrap();
        view.reload();

        view.handle_key_direct(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(Registry::new(home.path()).list().unwrap().len(), 1);
        view.handle_key_direct(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(Registry::new(home.path()).list().unwrap().len(), 1);
        view.handle_key_direct(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        view.handle_key_direct(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(Registry::new(home.path()).list().unwrap().is_empty());
        assert!(matches!(
            rx.recv().unwrap(),
            AppEvent::ProjectRegistryChanged(_)
        ));
    }
}
