use std::cmp::Ordering;

use code_common::model_picker_order::picker_rank_for_model;
use code_common::model_presets::ModelPreset;
use code_core::config_types::ReasoningEffort;

/// Flattened preset entry combining a model with a specific reasoning effort.
#[derive(Clone, Debug)]
pub(crate) struct FlatPreset {
    pub(crate) provider_id: Option<String>,
    pub(crate) model: String,
    pub(crate) display_name: String,
    pub(crate) effort: ReasoningEffort,
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) model_description: String,
    pub(crate) picker_rank: u16,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum OpenRouterSection {
    Free,
    Paid,
}

pub(crate) fn openrouter_section(
    provider_id: Option<&str>,
    model: &str,
) -> Option<OpenRouterSection> {
    provider_id
        .is_some_and(|provider| provider.eq_ignore_ascii_case("openrouter"))
        .then(|| {
            if model.to_ascii_lowercase().ends_with(":free") {
                OpenRouterSection::Free
            } else {
                OpenRouterSection::Paid
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrouter_sections_are_case_insensitive_and_free_first() {
        assert_eq!(
            openrouter_section(Some("OpenRouter"), "vendor/model:FREE"),
            Some(OpenRouterSection::Free),
        );
        assert_eq!(
            openrouter_section(Some("openrouter"), "vendor/model"),
            Some(OpenRouterSection::Paid),
        );
        assert_eq!(openrouter_section(Some("openai"), "vendor/model:free"), None);
        assert!(OpenRouterSection::Free < OpenRouterSection::Paid);
    }
}

impl FlatPreset {
    pub(crate) fn from_model_preset(preset: &ModelPreset) -> Vec<Self> {
        preset
            .supported_reasoning_efforts
            .iter()
            .map(|effort_preset| {
                let effort_label = reasoning_effort_label(effort_preset.effort.into());
                FlatPreset {
                    provider_id: None,
                    model: preset.model.clone(),
                    display_name: preset.display_name.clone(),
                    effort: effort_preset.effort.into(),
                    label: format!("{} {}", preset.display_name, effort_label.to_lowercase()),
                    description: effort_preset.description.clone(),
                    model_description: preset.description.clone(),
                    picker_rank: picker_rank_for_model(&preset.model),
                }
            })
            .collect()
    }

    pub(crate) fn from_direct_provider_preset(
        provider_id: &str,
        preset: &ModelPreset,
    ) -> Vec<Self> {
        Self::from_model_preset(preset)
            .into_iter()
            .map(|mut flat| {
                flat.provider_id = Some(provider_id.to_owned());
                flat
            })
            .collect()
    }
}

pub(crate) fn reasoning_effort_label(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Ultra => "Ultra",
        ReasoningEffort::Max => "Max",
        ReasoningEffort::XHigh => "XHigh",
        ReasoningEffort::High => "High",
        ReasoningEffort::Medium => "Medium",
        ReasoningEffort::Low => "Low",
        ReasoningEffort::Minimal => "Minimal",
        ReasoningEffort::None => "None",
    }
}

pub(crate) fn compare_presets(a: &FlatPreset, b: &FlatPreset) -> Ordering {
    a.picker_rank
        .cmp(&b.picker_rank)
        .then_with(|| a.display_name.cmp(&b.display_name))
        .then_with(|| a.model.cmp(&b.model))
        .then_with(|| a.provider_id.cmp(&b.provider_id))
        .then_with(|| effort_rank(a.effort).cmp(&effort_rank(b.effort)))
        .then_with(|| a.label.cmp(&b.label))
}

fn effort_rank(effort: ReasoningEffort) -> u8 {
    match effort {
        ReasoningEffort::Ultra => 0,
        ReasoningEffort::Max => 1,
        ReasoningEffort::XHigh => 2,
        ReasoningEffort::High => 3,
        ReasoningEffort::Medium => 4,
        ReasoningEffort::Low => 5,
        ReasoningEffort::Minimal => 6,
        ReasoningEffort::None => 7,
    }
}
