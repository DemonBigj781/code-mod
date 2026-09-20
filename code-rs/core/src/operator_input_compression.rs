use crate::config_types::OperatorInputCompressionConfig;
use code_protocol::models::{ContentItem, ResponseItem};
use std::collections::{HashMap, HashSet, VecDeque};

const MAX_CACHE_SUBMISSIONS: usize = 16_384;
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;

enum CachedCompression {
    Unchanged,
    Compressed(String),
}

impl CachedCompression {
    fn byte_len(&self) -> usize {
        match self {
            Self::Unchanged => 0,
            Self::Compressed(text) => text.len(),
        }
    }

    fn apply(&self, text: &mut String) {
        if let Self::Compressed(compressed) = self {
            text.clone_from(compressed);
        }
    }

    fn is_compressed(&self) -> bool {
        matches!(self, Self::Compressed(_))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OperatorInputCompressionStats {
    pub(crate) input_item_count: usize,
    pub(crate) candidate_text_count: usize,
    pub(crate) transformed_text_count: usize,
    pub(crate) cache_hit_count: usize,
    pub(crate) cache_miss_count: usize,
}

#[derive(Default)]
struct CachedSubmission {
    standard: Vec<Option<CachedCompression>>,
    aggressive: Vec<Option<CachedCompression>>,
    cached_bytes: usize,
}

#[derive(Default)]
pub(crate) struct OperatorInputCompressionCache {
    submissions: HashMap<String, CachedSubmission>,
    insertion_order: VecDeque<String>,
    cached_bytes: usize,
    #[cfg(test)]
    hits: usize,
    #[cfg(test)]
    misses: usize,
}

impl OperatorInputCompressionCache {
    fn apply_cached(
        &mut self,
        submission_id: &str,
        content_index: usize,
        aggressive: bool,
        text: &mut String,
    ) -> Option<bool> {
        let found = self.submissions.get(submission_id).and_then(|submission| {
            let entries = if aggressive {
                &submission.aggressive
            } else {
                &submission.standard
            };
            let value = entries.get(content_index).and_then(Option::as_ref)?;
            let compressed = value.is_compressed();
            value.apply(text);
            Some(compressed)
        });
        #[cfg(test)]
        if found.is_some() {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        found
    }

    fn insert(
        &mut self,
        submission_id: &str,
        content_index: usize,
        aggressive: bool,
        value: CachedCompression,
    ) {
        if !self.submissions.contains_key(submission_id) {
            self.submissions
                .insert(submission_id.to_owned(), CachedSubmission::default());
            self.insertion_order.push_back(submission_id.to_owned());
            self.cached_bytes = self.cached_bytes.saturating_add(submission_id.len());
        }

        let submission = self
            .submissions
            .get_mut(submission_id)
            .expect("compression cache submission was just inserted");
        let entries = if aggressive {
            &mut submission.aggressive
        } else {
            &mut submission.standard
        };
        if entries.len() <= content_index {
            entries.resize_with(content_index + 1, || None);
        }
        let old_bytes = entries[content_index]
            .as_ref()
            .map_or(0, CachedCompression::byte_len);
        let new_bytes = value.byte_len();
        entries[content_index] = Some(value);
        submission.cached_bytes = submission
            .cached_bytes
            .saturating_sub(old_bytes)
            .saturating_add(new_bytes);
        self.cached_bytes = self
            .cached_bytes
            .saturating_sub(old_bytes)
            .saturating_add(new_bytes);

        while self.submissions.len() > MAX_CACHE_SUBMISSIONS
            || self.cached_bytes > MAX_CACHE_BYTES
        {
            let Some(oldest_id) = self.insertion_order.pop_front() else {
                break;
            };
            if let Some(oldest) = self.submissions.remove(&oldest_id) {
                self.cached_bytes = self
                    .cached_bytes
                    .saturating_sub(oldest_id.len())
                    .saturating_sub(oldest.cached_bytes);
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    fn test_counts(&self) -> (usize, usize) {
        (self.hits, self.misses)
    }
}

#[cfg(test)]
pub(crate) fn compress_operator_text(
    input: &str,
    config: &OperatorInputCompressionConfig,
) -> String {
    compress_operator_text_if_changed(input, config).unwrap_or_else(|| input.to_owned())
}

fn compress_operator_text_if_changed(
    input: &str,
    config: &OperatorInputCompressionConfig,
) -> Option<String> {
    if !config.enabled || input.trim().chars().count() < 24 || input.contains("```") {
        return None;
    }

    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for paragraph in input.split("\n\n") {
        let trimmed = paragraph.trim();
        if trimmed.is_empty() {
            continue;
        }
        if is_protected(trimmed) {
            output.push(trimmed.to_owned());
            continue;
        }

        let mut compressed = normalize_whitespace(trimmed);
        compressed = strip_filler_prefixes(compressed);
        if config.aggressive {
            compressed = aggressive_rewrite(compressed);
        }
        compressed = capitalize_first(compressed.trim());
        if compressed.is_empty() {
            continue;
        }

        let identity = compressed.to_ascii_lowercase();
        if seen.insert(identity) {
            output.push(compressed);
        }
    }

    let compressed = if output.is_empty() {
        return None;
    } else {
        output.join("\n\n")
    };
    (compressed != input).then_some(compressed)
}

pub(crate) fn compress_operator_items(
    items: Vec<ResponseItem>,
    config: &OperatorInputCompressionConfig,
) -> Vec<ResponseItem> {
    compress_operator_items_with_stats(items, config).0
}

pub(crate) fn compress_operator_items_with_stats(
    mut items: Vec<ResponseItem>,
    config: &OperatorInputCompressionConfig,
) -> (Vec<ResponseItem>, OperatorInputCompressionStats) {
    let mut stats = OperatorInputCompressionStats {
        input_item_count: items.len(),
        ..OperatorInputCompressionStats::default()
    };
    if !config.enabled {
        return (items, stats);
    }

    for item in &mut items {
        let ResponseItem::Message {
            id: Some(_),
            role,
            content,
            ..
        } = item
        else {
            continue;
        };
        if role != "user" {
            continue;
        }
        for content_item in content {
            if let ContentItem::InputText { text } = content_item {
                stats.candidate_text_count = stats.candidate_text_count.saturating_add(1);
                if let Some(compressed) = compress_operator_text_if_changed(text, config) {
                    *text = compressed;
                    stats.transformed_text_count =
                        stats.transformed_text_count.saturating_add(1);
                }
            }
        }
    }

    (items, stats)
}

#[cfg(test)]
pub(crate) fn compress_operator_items_cached(
    items: Vec<ResponseItem>,
    config: &OperatorInputCompressionConfig,
    cache: &mut OperatorInputCompressionCache,
) -> Vec<ResponseItem> {
    compress_operator_items_cached_with_stats(items, config, cache).0
}

pub(crate) fn compress_operator_items_cached_with_stats(
    mut items: Vec<ResponseItem>,
    config: &OperatorInputCompressionConfig,
    cache: &mut OperatorInputCompressionCache,
) -> (Vec<ResponseItem>, OperatorInputCompressionStats) {
    let mut stats = OperatorInputCompressionStats {
        input_item_count: items.len(),
        ..OperatorInputCompressionStats::default()
    };
    if !config.enabled {
        return (items, stats);
    }

    for item in &mut items {
        let ResponseItem::Message {
            id: Some(submission_id),
            role,
            content,
            ..
        } = item
        else {
            continue;
        };
        if role != "user" {
            continue;
        }
        for (content_index, content_item) in content.iter_mut().enumerate() {
            let ContentItem::InputText { text } = content_item else {
                continue;
            };
            stats.candidate_text_count = stats.candidate_text_count.saturating_add(1);
            if let Some(was_compressed) =
                cache.apply_cached(submission_id, content_index, config.aggressive, text)
            {
                stats.cache_hit_count = stats.cache_hit_count.saturating_add(1);
                if was_compressed {
                    stats.transformed_text_count =
                        stats.transformed_text_count.saturating_add(1);
                }
                continue;
            }
            stats.cache_miss_count = stats.cache_miss_count.saturating_add(1);

            let value = compress_operator_text_if_changed(text, config)
                .map_or(CachedCompression::Unchanged, CachedCompression::Compressed);
            let was_compressed = value.is_compressed();
            value.apply(text);
            cache.insert(submission_id, content_index, config.aggressive, value);
            if was_compressed {
                stats.transformed_text_count =
                    stats.transformed_text_count.saturating_add(1);
            }
        }
    }

    (items, stats)
}

fn normalize_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_filler_prefixes(mut text: String) -> String {
    const PREFIXES: &[&str] = &[
        "actually, ",
        "basically, ",
        "please ",
        "could you ",
        "would you ",
        "can you ",
        "i would like you to ",
        "i want you to ",
        "just ",
    ];

    loop {
        let lower = text.to_ascii_lowercase();
        let Some(prefix) = PREFIXES.iter().find(|prefix| lower.starts_with(**prefix)) else {
            return text;
        };
        text = text[prefix.len()..].trim_start().to_owned();
    }
}

fn aggressive_rewrite(text: String) -> String {
    let mut rewritten = text
        .replace(", and then you should ", "; ")
        .replace(", then you should ", "; ")
        .replace(" and then you should ", "; ");
    rewritten = strip_filler_prefixes(rewritten);

    let mut clauses = Vec::new();
    for (index, clause) in rewritten.split(';').enumerate() {
        let clause = clause.trim();
        let lower = clause.to_ascii_lowercase();
        let clause = lower
            .strip_prefix("you should ")
            .map_or(clause, |_| &clause["you should ".len()..]);
        let clause = if index == 0 {
            capitalize_first(clause.trim())
        } else {
            clause.trim().to_owned()
        };
        clauses.push(clause);
    }
    clauses.join("; ")
}

fn capitalize_first(input: &str) -> String {
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_uppercase().chain(chars).collect()
}

fn is_protected(input: &str) -> bool {
    if input.contains('`')
        || input.contains('"')
        || input.matches('\'').count() >= 2
        || input.contains("http://")
        || input.contains("https://")
        || input.split_whitespace().any(|token| token.contains('/'))
        || input.contains('\\')
        || input.chars().any(|ch| ch.is_ascii_digit())
        || input.contains('{')
        || input.contains('}')
        || input.contains('[')
        || input.contains(']')
        || input.contains('<')
        || input.contains('>')
        || input.contains('@')
        || input.lines().any(is_list_line)
    {
        return true;
    }

    let lower = input.to_ascii_lowercase();
    [
        " must ",
        " never ",
        " do not ",
        " don't ",
        " required ",
        " exactly ",
        " only ",
        " preserve ",
        " shall ",
    ]
    .iter()
    .any(|keyword| format!(" {lower} ").contains(keyword))
}

fn is_list_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || trimmed
            .split_once('.')
            .is_some_and(|(prefix, rest)| {
                !rest.is_empty() && prefix.chars().all(|ch| ch.is_ascii_digit())
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standard() -> OperatorInputCompressionConfig {
        OperatorInputCompressionConfig {
            enabled: true,
            aggressive: false,
        }
    }

    #[test]
    fn standard_compression_removes_redundant_prose() {
        let input = "Please   update the documentation.\n\nPlease update the documentation.\n\nActually, please run the tests.";
        assert_eq!(
            compress_operator_text(input, &standard()),
            "Update the documentation.\n\nRun the tests."
        );
    }

    #[test]
    fn aggressive_compression_is_additive_and_disabled_by_default() {
        let mut config = standard();
        config.aggressive = true;
        assert_eq!(
            compress_operator_text(
                "I would like you to review the code, and then you should fix the bug.",
                &config,
            ),
            "Review the code; fix the bug."
        );
    }

    #[test]
    fn protected_content_and_short_prompts_remain_exact() {
        let config = standard();
        for input in [
            "Fix it",
            "Run `cargo test -p code-core -j1` exactly.",
            "Open https://example.com/a/1 and preserve /tmp/file.json.",
            "Please update src/main.rs and preserve /tmp/file.json.",
            "Keep 'quoted text' unchanged.",
            "Preserve [alpha, beta] exactly.",
            "- must keep ID abc-123\n- never change 42",
            "```json\n{\"enabled\": true}\n```",
        ] {
            assert_eq!(compress_operator_text(input, &config), input);
        }
    }

    #[test]
    fn disabled_compression_returns_original() {
        let config = OperatorInputCompressionConfig {
            enabled: false,
            aggressive: true,
        };
        let input = "Please   update this. Please update this.";
        assert_eq!(compress_operator_text(input, &config), input);
    }

    #[test]
    fn item_compression_only_changes_identified_operator_messages() {
        let original = "Please   update the documentation.\n\nPlease update the documentation.";
        let items = vec![
            ResponseItem::Message {
                id: Some("submission-1".to_owned()),
                role: "user".to_owned(),
                content: vec![ContentItem::InputText {
                    text: original.to_owned(),
                }],
                end_turn: None,
                phase: None,
            },
            ResponseItem::Message {
                id: None,
                role: "user".to_owned(),
                content: vec![ContentItem::InputText {
                    text: original.to_owned(),
                }],
                end_turn: None,
                phase: None,
            },
        ];

        let compressed = compress_operator_items(items, &standard());
        let texts = compressed
            .iter()
            .filter_map(|item| match item {
                ResponseItem::Message { content, .. } => content.first(),
                _ => None,
            })
            .filter_map(|content| match content {
                ContentItem::InputText { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(texts, vec!["Update the documentation.", original]);
    }

    #[test]
    fn compression_is_idempotent() {
        let input = "Please update the documentation.\n\nPlease update the documentation.";
        let once = compress_operator_text(input, &standard());
        assert_eq!(compress_operator_text(&once, &standard()), once);
    }

    #[test]
    fn cached_item_compression_reuses_immutable_submission_content() {
        let original = "Please   update the documentation.\n\nPlease update the documentation.";
        let item = ResponseItem::Message {
            id: Some("submission-cache".to_owned()),
            role: "user".to_owned(),
            content: vec![ContentItem::InputText {
                text: original.to_owned(),
            }],
            end_turn: None,
            phase: None,
        };
        let mut cache = OperatorInputCompressionCache::default();

        let first = compress_operator_items_cached(vec![item.clone()], &standard(), &mut cache);
        assert_eq!(cache.test_counts(), (0, 1));
        let second = compress_operator_items_cached(vec![item], &standard(), &mut cache);
        assert_eq!(cache.test_counts(), (1, 1));
        assert_eq!(first, second);
    }

    #[test]
    fn cached_item_compression_reports_transform_and_cache_statistics() {
        let original = "Please   update the documentation.\n\nPlease update the documentation.";
        let item = ResponseItem::Message {
            id: Some("submission-telemetry".to_owned()),
            role: "user".to_owned(),
            content: vec![ContentItem::InputText {
                text: original.to_owned(),
            }],
            end_turn: None,
            phase: None,
        };
        let mut cache = OperatorInputCompressionCache::default();

        let (_, first) = compress_operator_items_cached_with_stats(
            vec![item.clone()],
            &standard(),
            &mut cache,
        );
        assert_eq!(first.input_item_count, 1);
        assert_eq!(first.candidate_text_count, 1);
        assert_eq!(first.transformed_text_count, 1);
        assert_eq!(first.cache_hit_count, 0);
        assert_eq!(first.cache_miss_count, 1);

        let (_, second) =
            compress_operator_items_cached_with_stats(vec![item], &standard(), &mut cache);
        assert_eq!(second.input_item_count, 1);
        assert_eq!(second.candidate_text_count, 1);
        assert_eq!(second.transformed_text_count, 1);
        assert_eq!(second.cache_hit_count, 1);
        assert_eq!(second.cache_miss_count, 0);
    }

    #[test]
    fn cached_item_compression_keeps_standard_and_aggressive_modes_separate() {
        let item = ResponseItem::Message {
            id: Some("submission-modes".to_owned()),
            role: "user".to_owned(),
            content: vec![ContentItem::InputText {
                text: "I would like you to review the code, and then you should fix the bug."
                    .to_owned(),
            }],
            end_turn: None,
            phase: None,
        };
        let mut aggressive = standard();
        aggressive.aggressive = true;
        let mut cache = OperatorInputCompressionCache::default();

        let standard_items =
            compress_operator_items_cached(vec![item.clone()], &standard(), &mut cache);
        let aggressive_items =
            compress_operator_items_cached(vec![item], &aggressive, &mut cache);

        assert_ne!(standard_items, aggressive_items);
        assert_eq!(cache.test_counts(), (0, 2));
    }
}
