use std::fmt;

use code_core::WireApi;
use ratatui::layout::Constraint;
use ratatui::style::Style;
use ratatui::text::Line;

use crate::bottom_pane::settings_ui::action_page::SettingsActionPage;
use crate::bottom_pane::settings_ui::buttons::{
    SettingsButtonKind, StandardButtonSpec, standard_button_specs,
};
use crate::bottom_pane::settings_ui::form_page::{SettingsFormPage, SettingsFormSection};
use crate::bottom_pane::settings_ui::hints::{
    KeyHint, hint_esc, status_and_shortcuts_split,
};
use crate::bottom_pane::settings_ui::panel::SettingsPanelStyle;
use crate::bottom_pane::settings_ui::rows::StyledText;
use crate::colors;
use crate::components::form_text_field::FormTextField;

pub(super) const ENDPOINT_FORM_ROW_COUNT: usize = 6;
pub(super) const ENDPOINT_FORM_DISPLAY_NAME_ROW: usize = 0;
pub(super) const ENDPOINT_FORM_BASE_URL_ROW: usize = 1;
pub(super) const ENDPOINT_FORM_API_KEY_ROW: usize = 2;
pub(super) const ENDPOINT_FORM_WIRE_API_ROW: usize = 3;
pub(super) const ENDPOINT_FORM_SAVE_ROW: usize = 4;
pub(super) const ENDPOINT_FORM_CANCEL_ROW: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EndpointFormAction {
    Save,
    Cancel,
}

pub(super) struct EndpointFormState {
    display_name_field: FormTextField,
    base_url_field: FormTextField,
    api_key_field: FormTextField,
    wire_api_field: FormTextField,
    wire_api: WireApi,
    selected_row: usize,
    hovered_button: Option<EndpointFormAction>,
    submitting: bool,
    error: Option<String>,
}

impl fmt::Debug for EndpointFormState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EndpointFormState")
            .field("wire_api", &self.wire_api)
            .field("selected_row", &self.selected_row)
            .field("submitting", &self.submitting)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl EndpointFormState {
    pub(super) fn new() -> Self {
        let mut display_name_field = FormTextField::new_single_line();
        display_name_field.set_placeholder("Local Ollama");
        let mut base_url_field = FormTextField::new_single_line();
        base_url_field.set_placeholder("http://127.0.0.1:11434/v1");
        let mut api_key_field = FormTextField::new_single_line();
        api_key_field.set_placeholder("optional");
        let mut wire_api_field = FormTextField::new_single_line();
        wire_api_field.set_text("Chat Completions");

        Self {
            display_name_field,
            base_url_field,
            api_key_field,
            wire_api_field,
            wire_api: WireApi::Chat,
            selected_row: ENDPOINT_FORM_DISPLAY_NAME_ROW,
            hovered_button: None,
            submitting: false,
            error: None,
        }
    }

    pub(super) fn page(&self, panel_title: &'static str) -> SettingsFormPage<'static> {
        let status = if let Some(error) = self.error.as_ref() {
            Some(StyledText::new(error.clone(), Style::new().fg(colors::error())))
        } else if self.submitting {
            Some(StyledText::new(
                "Validating endpoint and loading models...".to_owned(),
                Style::new().fg(colors::function()),
            ))
        } else {
            None
        };
        let (status_lines, footer_lines) = status_and_shortcuts_split(
            status,
            &[
                KeyHint::new(crate::bottom_pane::settings_ui::hints::key_tab(), " next"),
                KeyHint::new(crate::bottom_pane::settings_ui::hints::key_ctrl("S"), " save"),
                hint_esc(" cancel"),
            ],
        );
        let page = SettingsActionPage::new(
            panel_title,
            SettingsPanelStyle::bottom_pane(),
            vec![
                Line::from("Add an OpenAI-compatible endpoint. The base URL is normalized to one /v1 root."),
                Line::from("API keys are stored in the encrypted secrets store, never in config.toml."),
                Line::from(""),
            ],
            footer_lines,
        )
        .with_status_lines(status_lines)
        .with_action_rows(1)
        .with_min_body_rows(7);

        SettingsFormPage::new(
            page,
            vec![
                SettingsFormSection::new(
                    "Display name",
                    self.selected_row == ENDPOINT_FORM_DISPLAY_NAME_ROW,
                    Constraint::Length(1),
                ),
                SettingsFormSection::new(
                    "Base URL",
                    self.selected_row == ENDPOINT_FORM_BASE_URL_ROW,
                    Constraint::Length(1),
                ),
                SettingsFormSection::new(
                    "API key (optional)",
                    self.selected_row == ENDPOINT_FORM_API_KEY_ROW,
                    Constraint::Length(1),
                ),
                SettingsFormSection::new(
                    "API shape (Left/Right to change)",
                    self.selected_row == ENDPOINT_FORM_WIRE_API_ROW,
                    Constraint::Length(1),
                ),
            ],
        )
        .with_section_gap_rows(1)
    }

    pub(super) fn button_specs(&self) -> Vec<StandardButtonSpec<EndpointFormAction>> {
        let focused = match self.selected_row {
            ENDPOINT_FORM_SAVE_ROW => Some(EndpointFormAction::Save),
            ENDPOINT_FORM_CANCEL_ROW => Some(EndpointFormAction::Cancel),
            _ => None,
        };
        standard_button_specs(
            &[
                (EndpointFormAction::Save, SettingsButtonKind::Save),
                (EndpointFormAction::Cancel, SettingsButtonKind::Cancel),
            ],
            focused,
            self.hovered_button,
        )
    }

    pub(super) fn fields(&self) -> [&FormTextField; 4] {
        [
            &self.display_name_field,
            &self.base_url_field,
            &self.api_key_field,
            &self.wire_api_field,
        ]
    }

    pub(super) fn selected_field_mut(&mut self) -> Option<&mut FormTextField> {
        match self.selected_row {
            ENDPOINT_FORM_DISPLAY_NAME_ROW => Some(&mut self.display_name_field),
            ENDPOINT_FORM_BASE_URL_ROW => Some(&mut self.base_url_field),
            ENDPOINT_FORM_API_KEY_ROW => Some(&mut self.api_key_field),
            _ => None,
        }
    }

    pub(super) fn select_next_row(&mut self) {
        self.selected_row = (self.selected_row + 1) % ENDPOINT_FORM_ROW_COUNT;
    }

    pub(super) fn select_previous_row(&mut self) {
        self.selected_row = if self.selected_row == 0 {
            ENDPOINT_FORM_ROW_COUNT - 1
        } else {
            self.selected_row - 1
        };
    }

    pub(super) fn set_selected_row(&mut self, row: usize) {
        if row < ENDPOINT_FORM_ROW_COUNT {
            self.selected_row = row;
        }
    }

    pub(super) fn toggle_wire_api(&mut self) {
        self.wire_api = match self.wire_api {
            WireApi::Chat => WireApi::Responses,
            WireApi::Responses | WireApi::ResponsesWebsocket => WireApi::Chat,
        };
        let label = match self.wire_api {
            WireApi::Chat => "Chat Completions",
            WireApi::Responses | WireApi::ResponsesWebsocket => "Responses",
        };
        self.wire_api_field.set_text(label);
        self.error = None;
    }

    pub(super) fn set_hovered_button(&mut self, action: Option<EndpointFormAction>) -> bool {
        if self.hovered_button == action {
            return false;
        }
        self.hovered_button = action;
        true
    }

    pub(super) fn begin_submission(&mut self) {
        self.submitting = true;
        self.error = None;
    }

    pub(super) fn finish_submission(&mut self, error: Option<String>) {
        self.submitting = false;
        self.error = error;
    }

    pub(super) fn clear_error(&mut self) {
        self.error = None;
    }

    pub(super) fn display_name(&self) -> &str {
        self.display_name_field.text()
    }

    pub(super) fn base_url(&self) -> &str {
        self.base_url_field.text()
    }

    pub(super) fn api_key(&self) -> &str {
        self.api_key_field.text()
    }

    pub(super) fn wire_api(&self) -> WireApi {
        self.wire_api
    }

    pub(super) fn selected_row(&self) -> usize {
        self.selected_row
    }

    pub(super) fn is_submitting(&self) -> bool {
        self.submitting
    }

    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    #[cfg(test)]
    pub(super) fn display_name_field_mut(&mut self) -> &mut FormTextField {
        &mut self.display_name_field
    }

    #[cfg(test)]
    pub(super) fn base_url_field_mut(&mut self) -> &mut FormTextField {
        &mut self.base_url_field
    }

    #[cfg(test)]
    pub(super) fn api_key_field_mut(&mut self) -> &mut FormTextField {
        &mut self.api_key_field
    }
}
