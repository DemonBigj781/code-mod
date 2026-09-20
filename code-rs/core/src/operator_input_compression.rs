use crate::config_types::OperatorInputCompressionConfig;
use code_protocol::models::{ContentItem, ResponseItem};
use std::collections::HashSet;

pub(crate) fn compress_operator_text(
    input: &str,
    config: &OperatorInputCompressionConfig,
) -> String {
    if !config.enabled || input.trim().chars().count() < 24 || input.contains("```") {
        return input.to_owned();
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

    if output.is_empty() {
        input.to_owned()
    } else {
        output.join("\n\n")
    }
}

pub(crate) fn compress_operator_items(
    mut items: Vec<ResponseItem>,
    config: &OperatorInputCompressionConfig,
) -> Vec<ResponseItem> {
    if !config.enabled {
        return items;
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
                let compressed = compress_operator_text(text, config);
                if !compressed.trim().is_empty() {
                    *text = compressed;
                }
            }
        }
    }

    items
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
}
