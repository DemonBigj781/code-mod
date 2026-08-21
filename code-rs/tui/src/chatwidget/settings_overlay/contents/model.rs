use code_common::model_presets::ModelPreset;

use crate::bottom_pane::settings_pages::model::{DirectProviderModelCatalog, ModelSelectionView};

pub(crate) struct ModelSettingsContent {
    view: ModelSelectionView,
}

impl ModelSettingsContent {
    pub(crate) fn new(view: ModelSelectionView) -> Self {
        Self { view }
    }

    pub(crate) fn update_presets(&mut self, presets: Vec<ModelPreset>) {
        self.view.update_presets(presets);
    }

    pub(crate) fn update_direct_provider_catalogs(
        &mut self,
        catalogs: Vec<DirectProviderModelCatalog>,
    ) {
        self.view.update_direct_provider_catalogs(catalogs);
    }

    pub(crate) fn finish_direct_provider_add(&mut self, result: Result<(), String>) {
        self.view.finish_direct_provider_add(result);
    }
}

impl_settings_content_with_paste!(ModelSettingsContent);
