use super::super::SettingsContent;
use crate::bottom_pane::BottomPaneView;
use crate::bottom_pane::settings_pages::ssh::SshSettingsView;

pub(crate) struct SshSettingsContent {
    view: SshSettingsView,
}

impl SshSettingsContent {
    pub(crate) fn new(view: SshSettingsView) -> Self {
        Self { view }
    }
    pub(crate) fn apply_result(&mut self, result: &Result<Vec<String>, String>) {
        self.view.apply_result(result);
    }
}

impl SettingsContent for SshSettingsContent {
    fn render(&self, area: ratatui::layout::Rect, buffer: &mut ratatui::buffer::Buffer) {
        self.view.render(area, buffer);
    }
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        self.view.handle_key_direct(key)
    }
    fn handle_paste(&mut self, text: String) -> bool {
        self.view.paste_direct(text)
    }
    fn handle_mouse(
        &mut self,
        event: crossterm::event::MouseEvent,
        _: ratatui::layout::Rect,
    ) -> bool {
        self.view.mouse_direct(event)
    }
    fn is_complete(&self) -> bool {
        self.view.is_complete()
    }
    fn has_back_navigation(&self) -> bool {
        self.view.has_back_navigation()
    }
    fn on_deactivate(&mut self) {
        self.view.cancel_edit();
    }
    fn on_close(&mut self) {
        self.view.cancel_edit();
    }
}
