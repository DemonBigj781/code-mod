use super::{JsonSchema, MEMORIES_TOOL_NAME, OpenAiTool, ResponsesApiTool};
use std::collections::BTreeMap;

pub(super) fn create_memories_tool() -> OpenAiTool {
    let mut properties = BTreeMap::new();
    properties.insert(
        "action".into(),
        JsonSchema::String {
            description: Some(
                "Read-only operation over the currently published Memories generation.".into(),
            ),
            allowed_values: Some(
                ["status", "search", "read", "list_rollouts"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            ),
        },
    );
    for (name, description) in [
        (
            "query",
            "Nonempty literal case-insensitive query for search.",
        ),
        (
            "path",
            "Logical artifact path for read: memory_summary.md, raw_memories.md, or rollout_summaries/<slug>.md.",
        ),
        (
            "source",
            "Optional search source: all, summary, memories, or rollouts. Defaults to all.",
        ),
    ] {
        properties.insert(
            name.into(),
            JsonSchema::String {
                description: Some(description.into()),
                allowed_values: None,
            },
        );
    }
    for (name, description) in [
        ("offset", "Zero-based pagination offset for list_rollouts."),
        ("limit", "Result limit: default 20, maximum 100."),
        (
            "line_start",
            "One-based first line for read. Defaults to 1.",
        ),
        (
            "line_count",
            "Maximum lines for read. Defaults to 200, maximum 2000.",
        ),
        (
            "max_chars",
            "Maximum returned text characters. Defaults to 50000, maximum 200000.",
        ),
    ] {
        properties.insert(
            name.into(),
            JsonSchema::Number {
                description: Some(description.into()),
            },
        );
    }
    OpenAiTool::Function(ResponsesApiTool {
        name: MEMORIES_TOOL_NAME.into(),
        description: "Native Memories retrieval. Resolves the active published generation on every call, so callers never depend on movable snapshot paths. Search and read curated memory artifacts using stable logical paths; use session_history for raw prior-session messages.".into(),
        strict: false,
        parameters: JsonSchema::Object {
            properties,
            required: Some(vec!["action".into()]),
            additional_properties: Some(false.into()),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_stable_memory_operations() {
        let tool = serde_json::to_value(create_memories_tool()).expect("valid tool schema");
        assert_eq!(tool["name"], "memories");
        assert_eq!(
            tool["parameters"]["properties"]["action"]["enum"],
            serde_json::json!(["status", "search", "read", "list_rollouts"])
        );
    }
}
