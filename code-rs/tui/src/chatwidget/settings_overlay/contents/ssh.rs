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

impl_settings_content_with_paste!(
    SshSettingsContent,
    on_close = cancel_edit,
    on_deactivate = cancel_edit
);
