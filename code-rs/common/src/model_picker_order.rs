/// Shared picker ordering for well-known models.
///
/// Keep this intentionally small and explicit. The UI can later layer user
/// favorites or pinned models above this baseline without changing view code.
const MODEL_PICKER_RANKS: &[(&str, u16)] = &[
    ("openrouter/free-max", 0),
    ("gpt-6.1-sol", 1),
    ("gpt-6-astra", 2),
    ("gpt-6-sol", 3),
    ("gpt-6-luna", 4),
    ("gpt-5.6-sol", 5),
    ("gpt-6.1-terra", 6),
    ("gpt-5.6-terra", 7),
    ("gpt-5.6-luna", 8),
    ("gpt-5.5", 9),
    ("gpt-5.4", 10),
    ("gpt-5.3-codex", 11),
    ("gpt-5.3-codex-spark", 12),
    ("gpt-5.2-codex", 13),
    ("gpt-5.2", 14),
    ("gpt-5.1-codex-max", 15),
    ("gpt-5.1-codex", 16),
    ("gpt-5.1-codex-mini", 17),
    ("gpt-5.1", 18),
];

pub fn picker_rank_for_model(model: &str) -> u16 {
    MODEL_PICKER_RANKS
        .iter()
        .find_map(|(name, rank)| name.eq_ignore_ascii_case(model).then_some(*rank))
        .unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::picker_rank_for_model;

    #[test]
    fn gpt_5_5_ranks_above_gpt_5_4() {
        assert!(picker_rank_for_model("gpt-5.5") < picker_rank_for_model("gpt-5.4"));
    }

    #[test]
    fn current_openai_models_follow_catalog_priority() {
        let models = [
            "gpt-6.1-sol",
            "gpt-6-astra",
            "gpt-6-sol",
            "gpt-6-luna",
            "gpt-5.6-sol",
            "gpt-6.1-terra",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
        ];

        for pair in models.windows(2) {
            assert!(
                picker_rank_for_model(pair[0]) < picker_rank_for_model(pair[1]),
                "{} should rank before {}",
                pair[0],
                pair[1]
            );
        }
    }
}
