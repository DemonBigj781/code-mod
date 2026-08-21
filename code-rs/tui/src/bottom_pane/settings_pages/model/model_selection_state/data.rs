use std::sync::OnceLock;

use code_common::model_presets::ModelPreset;
use code_core::config_types::{ContextMode, ReasoningEffort, ServiceTier};
use code_core::model_family::{
    STANDARD_CONTEXT_WINDOW_272K, default_auto_compact_limit_for_context_window,
    derive_default_model_family, resolve_context_settings, supports_extended_context,
};
use code_core::remote_models::RemoteModelsStatus;
use code_protocol::num_format::format_with_separators_u64;

use super::presets::{FlatPreset, compare_presets};
use super::target::ModelSelectionTarget;

const SUMMARY_HEADER_LINES: u16 = 3;
const FAST_MODE_SECTION_HEIGHT: u16 = 5;
const CONTEXT_MODE_SECTION_HEIGHT: u16 = 7;
const CONTEXT_MODE_UNAVAILABLE_NOTICE_HEIGHT: u16 = 1;
const FOLLOW_CHAT_SECTION_HEIGHT: u16 = 4;
const ADD_DIRECT_PROVIDER_SECTION_HEIGHT: u16 = 3;
const FOOTER_HEIGHT: u16 = 2;
const FAST_MODE_ROW_OFFSET: usize = 2;
const CONTEXT_MODE_ROW_OFFSET: usize = 3;
const CONTEXT_WINDOW_ROW_OFFSET: usize = 4;
const AUTO_COMPACT_ROW_OFFSET: usize = 5;
const FOLLOW_CHAT_ROW_OFFSET: usize = 2;

pub(crate) struct ModelSelectionViewParams {
    pub(crate) presets: Vec<ModelPreset>,
    pub(crate) current_model: String,
    pub(crate) current_model_provider_id: Option<String>,
    pub(crate) current_effort: ReasoningEffort,
    pub(crate) current_service_tier: Option<ServiceTier>,
    pub(crate) current_context_mode: Option<ContextMode>,
    pub(crate) current_context_window: Option<u64>,
    pub(crate) current_auto_compact_token_limit: Option<i64>,
    pub(crate) use_chat_model: bool,
    pub(crate) direct_provider_catalogs: Vec<DirectProviderModelCatalog>,
    pub(crate) target: ModelSelectionTarget,
}

#[derive(Clone, Debug)]
pub(crate) struct DirectProviderModelCatalog {
    pub(crate) provider_id: String,
    pub(crate) display_name: String,
    pub(crate) status: RemoteModelsStatus,
    pub(crate) presets: Vec<ModelPreset>,
}

#[derive(Clone, Debug)]
pub(crate) struct CurrentSelection {
    pub(crate) current_model: String,
    pub(crate) current_model_provider_id: Option<String>,
    pub(crate) current_effort: ReasoningEffort,
    pub(crate) current_service_tier: Option<ServiceTier>,
    pub(crate) current_context_mode: Option<ContextMode>,
    pub(crate) current_context_window: Option<u64>,
    pub(crate) current_auto_compact_token_limit: Option<i64>,
    pub(crate) use_chat_model: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelSelectionData {
    pub(crate) flat_presets: Vec<FlatPreset>,
    sorted_preset_indices: Vec<usize>,
    presets: Vec<ModelPreset>,
    pub(crate) direct_provider_catalogs: Vec<DirectProviderModelCatalog>,
    pub(crate) current: CurrentSelection,
    pub(crate) target: ModelSelectionTarget,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind {
    FastMode,
    ContextMode,
    ContextWindow,
    AutoCompact,
    FollowChat,
    Preset(usize),
    AddDirectProvider,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SelectionAction {
    ToggleFastMode(Option<ServiceTier>),
    SetContextMode(Option<ContextMode>),
    UseChatModel,
    SetPreset {
        model: String,
        effort: ReasoningEffort,
        model_provider_id: Option<String>,
    },
}

impl SelectionAction {
    pub(crate) fn closes_view(&self) -> bool {
        matches!(
            self,
            SelectionAction::UseChatModel | SelectionAction::SetPreset { .. }
        )
    }
}

impl ModelSelectionData {
    pub(crate) fn supports_fast_mode(&self) -> bool {
        self.target.supports_fast_mode(&self.current.current_model)
    }

    fn build_flat_presets(
        presets: &[ModelPreset],
        direct_provider_catalogs: &[DirectProviderModelCatalog],
    ) -> Vec<FlatPreset> {
        let mut flat_presets: Vec<FlatPreset> = presets
            .iter()
            .flat_map(FlatPreset::from_model_preset)
            .collect();
        for catalog in direct_provider_catalogs {
            flat_presets.extend(catalog.presets.iter().flat_map(|preset| {
                FlatPreset::from_direct_provider_preset(&catalog.provider_id, preset)
            }));
        }
        flat_presets
    }

    fn build_sorted_preset_indices(
        flat_presets: &[FlatPreset],
        direct_provider_catalogs: &[DirectProviderModelCatalog],
    ) -> Vec<usize> {
        let mut indices: Vec<usize> = flat_presets
            .iter()
            .enumerate()
            .filter_map(|(index, preset)| preset.provider_id.is_none().then_some(index))
            .collect();
        indices.sort_by(|&a, &b| compare_presets(&flat_presets[a], &flat_presets[b]));

        for catalog in direct_provider_catalogs {
            let mut provider_indices: Vec<usize> = flat_presets
                .iter()
                .enumerate()
                .filter_map(|(index, preset)| {
                    (preset.provider_id.as_deref() == Some(catalog.provider_id.as_str()))
                        .then_some(index)
                })
                .collect();
            provider_indices.sort_by(|&a, &b| compare_presets(&flat_presets[a], &flat_presets[b]));
            indices.extend(provider_indices);
        }
        indices
    }

    fn sort_direct_provider_catalogs(catalogs: &mut [DirectProviderModelCatalog]) {
        catalogs.sort_by(|a, b| {
            a.display_name
                .to_ascii_lowercase()
                .cmp(&b.display_name.to_ascii_lowercase())
                .then_with(|| a.provider_id.cmp(&b.provider_id))
        });
    }

    pub(crate) fn context_mode_intro_lines() -> &'static [String; 2] {
        static CONTEXT_MODE_INTRO_LINES: OnceLock<[String; 2]> = OnceLock::new();
        CONTEXT_MODE_INTRO_LINES.get_or_init(|| {
            let threshold = format_with_separators_u64(STANDARD_CONTEXT_WINDOW_272K);
            [
                "Fast mode speeds up replies. 1M Context is available on supported models.".to_owned(),
                format!(
                    "Auto uses 1M limits and pre-turn compaction checks. Past {threshold} input tokens, the session is billed at 2x input and 1.5x output."
                ),
            ]
        })
    }

    pub(crate) fn new(params: ModelSelectionViewParams) -> Self {
        let ModelSelectionViewParams {
            presets,
            current_model,
            current_model_provider_id,
            current_effort,
            current_service_tier,
            current_context_mode,
            current_context_window,
            current_auto_compact_token_limit,
            use_chat_model,
            mut direct_provider_catalogs,
            target,
        } = params;
        if !target.supports_direct_providers() {
            direct_provider_catalogs.clear();
        }
        Self::sort_direct_provider_catalogs(&mut direct_provider_catalogs);
        let flat_presets = Self::build_flat_presets(&presets, &direct_provider_catalogs);
        let sorted_preset_indices =
            Self::build_sorted_preset_indices(&flat_presets, &direct_provider_catalogs);

        Self {
            flat_presets,
            sorted_preset_indices,
            presets,
            direct_provider_catalogs,
            current: CurrentSelection {
                current_model,
                current_model_provider_id,
                current_effort,
                current_service_tier,
                current_context_mode,
                current_context_window,
                current_auto_compact_token_limit,
                use_chat_model,
            },
            target,
        }
    }

    pub(crate) fn initial_selection(&self) -> usize {
        self.initial_selection_for_current()
    }

    pub(crate) fn update_presets(
        &mut self,
        presets: Vec<ModelPreset>,
        selected_index: usize,
    ) -> usize {
        let previous_selected = self.entry_at(selected_index);
        let previous_preset = self.selected_preset_identity(previous_selected);
        self.presets = presets;
        self.rebuild_flat_presets();
        self.restore_selection(previous_selected, previous_preset)
    }

    pub(crate) fn update_direct_provider_catalogs(
        &mut self,
        mut catalogs: Vec<DirectProviderModelCatalog>,
        selected_index: usize,
    ) -> usize {
        let previous_selected = self.entry_at(selected_index);
        let previous_preset = self.selected_preset_identity(previous_selected);
        if !self.target.supports_direct_providers() {
            catalogs.clear();
        }
        Self::sort_direct_provider_catalogs(&mut catalogs);
        self.direct_provider_catalogs = catalogs;
        self.rebuild_flat_presets();
        self.restore_selection(previous_selected, previous_preset)
    }

    fn rebuild_flat_presets(&mut self) {
        self.flat_presets = Self::build_flat_presets(&self.presets, &self.direct_provider_catalogs);
        self.sorted_preset_indices =
            Self::build_sorted_preset_indices(&self.flat_presets, &self.direct_provider_catalogs);
    }

    fn selected_preset_identity(
        &self,
        entry: Option<EntryKind>,
    ) -> Option<(Option<String>, String, ReasoningEffort)> {
        match entry {
            Some(EntryKind::Preset(index)) => self.flat_presets.get(index).map(|preset| {
                (
                    preset.provider_id.clone(),
                    preset.model.clone(),
                    preset.effort,
                )
            }),
            _ => None,
        }
    }

    fn preset_entry_prefix(&self) -> usize {
        usize::from(self.supports_fast_mode())
            + usize::from(self.target.supports_context_mode()) * 3
            + usize::from(self.target.supports_follow_chat())
    }

    fn find_preset_entry_index(
        &self,
        provider_id: Option<&str>,
        model: &str,
        effort: Option<ReasoningEffort>,
    ) -> Option<usize> {
        self.sorted_preset_indices
            .iter()
            .position(|flat_index| {
                let preset = &self.flat_presets[*flat_index];
                preset.provider_id.as_deref() == provider_id
                    && preset.model.eq_ignore_ascii_case(model)
                    && effort.is_none_or(|effort| preset.effort == effort)
            })
            .map(|position| self.preset_entry_prefix() + position)
    }

    fn restore_selection(
        &self,
        previous_selected: Option<EntryKind>,
        previous_preset: Option<(Option<String>, String, ReasoningEffort)>,
    ) -> usize {
        let include_fast_mode = self.supports_fast_mode();
        let include_context_mode = self.target.supports_context_mode();
        let context_entry_count = if include_context_mode { 3 } else { 0 };
        let include_follow_chat = self.target.supports_follow_chat();

        let mut next_selected: Option<usize> = None;
        match previous_selected {
            Some(EntryKind::FastMode) => {
                if include_fast_mode {
                    next_selected = Some(0);
                }
            }
            Some(EntryKind::ContextMode) => {
                if include_context_mode {
                    next_selected = Some(usize::from(include_fast_mode));
                }
            }
            Some(EntryKind::ContextWindow) => {
                if include_context_mode {
                    next_selected = Some(usize::from(include_fast_mode) + 1);
                }
            }
            Some(EntryKind::AutoCompact) => {
                if include_context_mode {
                    next_selected = Some(usize::from(include_fast_mode) + 2);
                }
            }
            Some(EntryKind::FollowChat) => {
                if include_follow_chat {
                    next_selected = Some(usize::from(include_fast_mode) + context_entry_count);
                }
            }
            Some(EntryKind::Preset(_)) => {
                if let Some((provider_id, previous_model, previous_effort)) = previous_preset {
                    next_selected = if provider_id.is_some() {
                        self.find_preset_entry_index(
                            provider_id.as_deref(),
                            &previous_model,
                            Some(previous_effort),
                        )
                    } else {
                        self.find_preset_entry_index(None, &previous_model, Some(previous_effort))
                    };
                }
            }
            Some(EntryKind::AddDirectProvider) => {
                if self.target.supports_direct_providers() {
                    next_selected = Some(self.entry_count().saturating_sub(1));
                }
            }
            None => {}
        }

        let mut next_selected =
            next_selected.unwrap_or_else(|| self.initial_selection_for_current());

        let total = self.entry_count();
        if total == 0 {
            next_selected = 0;
        } else if next_selected >= total {
            next_selected = total - 1;
        }

        next_selected
    }

    fn initial_selection_for_current(&self) -> usize {
        let include_fast_mode = self.supports_fast_mode();
        let include_context_mode = self.target.supports_context_mode();
        let include_follow_chat = self.target.supports_follow_chat();
        let context_entry_count = if include_context_mode { 3 } else { 0 };

        if include_follow_chat && self.current.use_chat_model {
            return usize::from(include_fast_mode) + context_entry_count;
        }

        let current_provider_id = self.current.current_model_provider_id.as_deref();
        if let Some(provider_id) =
            current_provider_id.filter(|provider_id| self.is_direct_provider(provider_id))
        {
            if let Some(index) = self.find_preset_entry_index(
                Some(provider_id),
                &self.current.current_model,
                Some(self.current.current_effort),
            ) {
                return index;
            }
            if let Some(index) =
                self.find_preset_entry_index(Some(provider_id), &self.current.current_model, None)
            {
                return index;
            }
        } else {
            if let Some(index) = self.find_preset_entry_index(
                None,
                &self.current.current_model,
                Some(self.current.current_effort),
            ) {
                return index;
            }
            if let Some(index) =
                self.find_preset_entry_index(None, &self.current.current_model, None)
            {
                return index;
            }
        }

        if include_follow_chat {
            if self.flat_presets.is_empty() {
                usize::from(include_fast_mode) + context_entry_count
            } else {
                usize::from(include_fast_mode) + context_entry_count + 1
            }
        } else if include_fast_mode {
            if self.flat_presets.is_empty() {
                0
            } else {
                usize::from(include_fast_mode) + context_entry_count
            }
        } else {
            0
        }
    }

    pub(crate) fn is_direct_provider(&self, provider_id: &str) -> bool {
        self.direct_provider_catalogs
            .iter()
            .any(|catalog| catalog.provider_id == provider_id)
    }

    pub(crate) fn preset_is_current(&self, preset: &FlatPreset) -> bool {
        let provider_matches = match preset.provider_id.as_deref() {
            Some(provider_id) => {
                self.current.current_model_provider_id.as_deref() == Some(provider_id)
            }
            None => !self
                .current
                .current_model_provider_id
                .as_deref()
                .is_some_and(|provider_id| self.is_direct_provider(provider_id)),
        };
        provider_matches
            && !self.current.use_chat_model
            && preset
                .model
                .eq_ignore_ascii_case(&self.current.current_model)
            && preset.effort == self.current.current_effort
    }

    pub(crate) fn supports_extended_context(&self) -> bool {
        supports_extended_context(&self.current.current_model)
    }

    pub(crate) fn current_model_display_name(&self) -> String {
        self.flat_presets
            .iter()
            .find(|preset| {
                let provider_matches = match preset.provider_id.as_deref() {
                    Some(provider_id) => {
                        self.current.current_model_provider_id.as_deref() == Some(provider_id)
                    }
                    None => !self
                        .current
                        .current_model_provider_id
                        .as_deref()
                        .is_some_and(|provider_id| self.is_direct_provider(provider_id)),
                };
                provider_matches
                    && preset
                        .model
                        .eq_ignore_ascii_case(&self.current.current_model)
            })
            .map_or_else(
                || self.current.current_model.clone(),
                |preset| preset.display_name.clone(),
            )
    }

    pub(crate) fn entries(&self) -> Vec<EntryKind> {
        let mut entries = Vec::new();
        if self.supports_fast_mode() {
            entries.push(EntryKind::FastMode);
        }
        if self.target.supports_context_mode() {
            entries.push(EntryKind::ContextMode);
            entries.push(EntryKind::ContextWindow);
            entries.push(EntryKind::AutoCompact);
        }
        if self.target.supports_follow_chat() {
            entries.push(EntryKind::FollowChat);
        }
        for idx in self.sorted_preset_indices.iter().copied() {
            entries.push(EntryKind::Preset(idx));
        }
        if self.target.supports_direct_providers() {
            entries.push(EntryKind::AddDirectProvider);
        }
        entries
    }

    pub(crate) fn entry_count(&self) -> usize {
        usize::from(self.supports_fast_mode())
            + usize::from(self.target.supports_context_mode()) * 3
            + usize::from(self.target.supports_follow_chat())
            + self.flat_presets.len()
            + usize::from(self.target.supports_direct_providers())
    }

    pub(crate) fn context_mode_entry_index(&self) -> Option<usize> {
        self.target
            .supports_context_mode()
            .then(|| usize::from(self.supports_fast_mode()))
    }

    pub(crate) fn context_window_entry_index(&self) -> Option<usize> {
        self.context_mode_entry_index().map(|index| index + 1)
    }

    pub(crate) fn auto_compact_entry_index(&self) -> Option<usize> {
        self.context_mode_entry_index().map(|index| index + 2)
    }

    pub(crate) fn follow_chat_entry_index(&self) -> Option<usize> {
        self.target.supports_follow_chat().then(|| {
            usize::from(self.supports_fast_mode())
                + usize::from(self.target.supports_context_mode()) * 3
        })
    }

    pub(crate) fn entry_at(&self, entry_index: usize) -> Option<EntryKind> {
        let mut next_index = 0;
        if self.supports_fast_mode() {
            if entry_index == next_index {
                return Some(EntryKind::FastMode);
            }
            next_index += 1;
        }
        if self.target.supports_context_mode() {
            if entry_index == next_index {
                return Some(EntryKind::ContextMode);
            }
            next_index += 1;
            if entry_index == next_index {
                return Some(EntryKind::ContextWindow);
            }
            next_index += 1;
            if entry_index == next_index {
                return Some(EntryKind::AutoCompact);
            }
            next_index += 1;
        }
        if self.target.supports_follow_chat() {
            if entry_index == next_index {
                return Some(EntryKind::FollowChat);
            }
            next_index += 1;
        }

        let preset_index = entry_index.checked_sub(next_index)?;
        if let Some(flat_index) = self.sorted_preset_indices.get(preset_index) {
            return Some(EntryKind::Preset(*flat_index));
        }
        (self.target.supports_direct_providers()
            && preset_index == self.sorted_preset_indices.len())
        .then_some(EntryKind::AddDirectProvider)
    }

    pub(crate) fn content_line_count(&self) -> u16 {
        let mut lines = SUMMARY_HEADER_LINES;
        if self.supports_fast_mode() {
            lines = lines.saturating_add(FAST_MODE_SECTION_HEIGHT);
        }
        if self.target.supports_context_mode() {
            lines = lines.saturating_add(CONTEXT_MODE_SECTION_HEIGHT);
            if !self.supports_extended_context() {
                lines = lines.saturating_add(CONTEXT_MODE_UNAVAILABLE_NOTICE_HEIGHT);
            }
        }
        if self.target.supports_follow_chat() {
            lines = lines.saturating_add(FOLLOW_CHAT_SECTION_HEIGHT);
        }

        let mut previous_model: Option<&str> = None;
        for idx in self.sorted_preset_indices.iter().copied() {
            let flat_preset = &self.flat_presets[idx];
            if flat_preset.provider_id.is_some() {
                continue;
            }
            let is_new_model =
                previous_model.is_none_or(|prev| !prev.eq_ignore_ascii_case(&flat_preset.model));

            if is_new_model {
                if previous_model.is_some() {
                    lines = lines.saturating_add(1);
                }
                lines = lines.saturating_add(1);
                if !flat_preset.model_description.trim().is_empty() {
                    lines = lines.saturating_add(1);
                }
                previous_model = Some(&flat_preset.model);
            }

            lines = lines.saturating_add(1);
        }

        if self.target.supports_direct_providers() {
            lines = lines.saturating_add(ADD_DIRECT_PROVIDER_SECTION_HEIGHT);
            for catalog in &self.direct_provider_catalogs {
                lines = lines.saturating_add(1);
                lines = lines.saturating_add(
                    u16::try_from(
                        self.preset_indices_for_provider(Some(&catalog.provider_id))
                            .len(),
                    )
                    .unwrap_or(u16::MAX),
                );
            }
        }

        lines.saturating_add(FOOTER_HEIGHT)
    }

    pub(crate) fn entry_line(&self, entry_index: usize) -> usize {
        debug_assert!(entry_index < self.entry_count());
        let mut line = usize::from(SUMMARY_HEADER_LINES);

        if self.supports_fast_mode() {
            if entry_index == 0 {
                return line + FAST_MODE_ROW_OFFSET;
            }
            line += usize::from(FAST_MODE_SECTION_HEIGHT);
        }

        if let Some(context_entry_index) = self.context_mode_entry_index() {
            if entry_index == context_entry_index {
                return line + CONTEXT_MODE_ROW_OFFSET;
            }
            if self.context_window_entry_index() == Some(entry_index) {
                return line + CONTEXT_WINDOW_ROW_OFFSET;
            }
            if self.auto_compact_entry_index() == Some(entry_index) {
                return line + AUTO_COMPACT_ROW_OFFSET;
            }
            line += usize::from(CONTEXT_MODE_SECTION_HEIGHT);
            if !self.supports_extended_context() {
                line += usize::from(CONTEXT_MODE_UNAVAILABLE_NOTICE_HEIGHT);
            }
        }

        if self.target.supports_follow_chat() {
            if self.follow_chat_entry_index() == Some(entry_index) {
                return line + FOLLOW_CHAT_ROW_OFFSET;
            }
            line += usize::from(FOLLOW_CHAT_SECTION_HEIGHT);
        }

        let selected_entry = self.entry_at(entry_index);
        let mut previous_model: Option<&str> = None;
        for preset_index in self.sorted_preset_indices.iter().copied() {
            let flat_preset = &self.flat_presets[preset_index];
            if flat_preset.provider_id.is_some() {
                continue;
            }
            let is_new_model =
                previous_model.is_none_or(|model| !model.eq_ignore_ascii_case(&flat_preset.model));

            if is_new_model {
                if previous_model.is_some() {
                    line += 1;
                }
                line += 1;
                if !flat_preset.model_description.trim().is_empty() {
                    line += 1;
                }
                previous_model = Some(&flat_preset.model);
            }

            if selected_entry == Some(EntryKind::Preset(preset_index)) {
                return line;
            }
            line += 1;
        }

        if self.target.supports_direct_providers() {
            line += 2;
            for catalog in &self.direct_provider_catalogs {
                line += 1;
                for preset_index in self.preset_indices_for_provider(Some(&catalog.provider_id)) {
                    if selected_entry == Some(EntryKind::Preset(preset_index)) {
                        return line;
                    }
                    line += 1;
                }
            }
        }

        line
    }

    pub(crate) fn preset_indices_for_provider(&self, provider_id: Option<&str>) -> Vec<usize> {
        self.sorted_preset_indices
            .iter()
            .copied()
            .filter(|index| self.flat_presets[*index].provider_id.as_deref() == provider_id)
            .collect()
    }

    pub(crate) fn apply_selection(&mut self, entry: EntryKind) -> Option<SelectionAction> {
        match entry {
            EntryKind::FastMode => {
                let next_service_tier =
                    if matches!(self.current.current_service_tier, Some(ServiceTier::Fast)) {
                        None
                    } else {
                        Some(ServiceTier::Fast)
                    };
                self.current.current_service_tier = next_service_tier;
                Some(SelectionAction::ToggleFastMode(next_service_tier))
            }
            EntryKind::ContextMode => {
                let next_context_mode = match self.current.current_context_mode {
                    None | Some(ContextMode::Disabled) => Some(ContextMode::OneM),
                    Some(ContextMode::OneM) => Some(ContextMode::Auto),
                    Some(ContextMode::Auto) => Some(ContextMode::Disabled),
                };
                let family = derive_default_model_family(&self.current.current_model);
                let (next_context_window, next_auto_compact_token_limit) =
                    resolve_context_settings(
                        &self.current.current_model,
                        next_context_mode,
                        None,
                        None,
                        &family,
                    );
                self.current.current_context_mode = next_context_mode;
                self.current.current_context_window = next_context_window;
                self.current.current_auto_compact_token_limit = next_auto_compact_token_limit;
                Some(SelectionAction::SetContextMode(next_context_mode))
            }
            EntryKind::ContextWindow | EntryKind::AutoCompact => None,
            EntryKind::FollowChat => {
                self.current.use_chat_model = true;
                Some(SelectionAction::UseChatModel)
            }
            EntryKind::Preset(idx) => {
                let flat_preset = self.flat_presets.get(idx)?.clone();
                self.current.current_model.clone_from(&flat_preset.model);
                if flat_preset.provider_id.is_some() {
                    self.current
                        .current_model_provider_id
                        .clone_from(&flat_preset.provider_id);
                }
                self.current.current_effort = flat_preset.effort;
                self.current.use_chat_model = false;
                Some(SelectionAction::SetPreset {
                    model: flat_preset.model,
                    effort: flat_preset.effort,
                    model_provider_id: flat_preset.provider_id,
                })
            }
            EntryKind::AddDirectProvider => None,
        }
    }

    pub(crate) fn context_window_is_default(&self) -> bool {
        let family = derive_default_model_family(&self.current.current_model);
        let (default_context_window, _) = resolve_context_settings(
            &self.current.current_model,
            self.current.current_context_mode,
            None,
            None,
            &family,
        );
        self.current.current_context_window == default_context_window
    }

    pub(crate) fn auto_compact_is_default(&self) -> bool {
        match (
            self.current.current_context_window,
            self.current.current_auto_compact_token_limit,
        ) {
            (Some(window), Some(limit)) => {
                limit == default_auto_compact_limit_for_context_window(window)
            }
            _ => true,
        }
    }
}
