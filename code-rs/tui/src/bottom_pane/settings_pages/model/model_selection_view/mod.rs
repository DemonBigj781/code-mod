mod endpoint_form;
mod input;
mod model;
mod mouse;
mod pane_impl;
mod render;

#[cfg(test)]
mod tests;

use std::cell::Cell;

use code_common::model_presets::ModelPreset;

use crate::app_event_sender::AppEventSender;
use crate::components::form_text_field::FormTextField;

use super::model_selection_state::{
    DirectProviderModelCatalog, ModelSelectionData, ModelSelectionViewParams,
};
use endpoint_form::EndpointFormState;

pub(super) const SUMMARY_LINE_COUNT: usize = 3;
// Shortcut bar only (blank footer line was removed).
pub(super) const FOOTER_LINE_COUNT: usize = 1;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditTarget {
    ContextWindow,
    AutoCompact,
}

#[derive(Debug)]
pub(super) enum ViewMode {
    Main,
    Edit {
        target: EditTarget,
        field: FormTextField,
        error: Option<String>,
    },
    AddDirectProvider(EndpointFormState),
    Transition,
}

pub(crate) struct ModelSelectionView {
    data: ModelSelectionData,
    selected_index: usize,
    app_event_tx: AppEventSender,
    is_complete: bool,
    scroll_offset: usize,
    visible_body_rows: Cell<usize>,
    mode: ViewMode,
}

crate::bottom_pane::chrome_view::impl_chrome_view!(ModelSelectionView, framed);

impl ModelSelectionView {
    pub fn new(params: ModelSelectionViewParams, app_event_tx: AppEventSender) -> Self {
        let data = ModelSelectionData::new(params);
        let selected_index = data.initial_selection();
        Self {
            data,
            selected_index,
            app_event_tx,
            is_complete: false,
            scroll_offset: 0,
            visible_body_rows: Cell::new(0),
            mode: ViewMode::Main,
        }
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.is_complete
    }

    pub(crate) fn update_presets(&mut self, presets: Vec<ModelPreset>) {
        self.selected_index = self.data.update_presets(presets, self.selected_index);
    }

    pub(crate) fn update_direct_provider_catalogs(
        &mut self,
        catalogs: Vec<DirectProviderModelCatalog>,
    ) {
        self.selected_index = self
            .data
            .update_direct_provider_catalogs(catalogs, self.selected_index);
    }

    pub(crate) fn has_back_navigation(&self) -> bool {
        !matches!(self.mode, ViewMode::Main)
    }

    pub(crate) fn handle_paste_direct(&mut self, text: String) -> bool {
        match &mut self.mode {
            ViewMode::Edit { field, .. } => {
                field.handle_paste(text);
                true
            }
            ViewMode::AddDirectProvider(form) => {
                if form.is_submitting() {
                    return true;
                }
                if let Some(field) = form.selected_field_mut() {
                    field.handle_paste(text);
                }
                true
            }
            ViewMode::Main | ViewMode::Transition => false,
        }
    }
}
