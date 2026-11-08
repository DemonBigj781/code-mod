use crate::bottom_pane::settings_pages::projects::ProjectsSettingsView;

pub(crate) struct ProjectsSettingsContent {
    view: ProjectsSettingsView,
}

impl ProjectsSettingsContent {
    pub(crate) fn new(view: ProjectsSettingsView) -> Self {
        Self { view }
    }
}

impl_settings_content_with_paste!(
    ProjectsSettingsContent,
    on_close = cancel_edit,
    on_deactivate = cancel_edit
);
