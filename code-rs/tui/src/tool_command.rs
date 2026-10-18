use serde_json::Value;

pub(crate) const USAGE: &str = "Usage: /tool list | /tool use <name> [JSON arguments]";
pub(crate) const MAX_ARGUMENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, PartialEq)]
pub(crate) enum ToolCommand {
    List,
    Use { name: String, arguments: Option<Value> },
}

pub(crate) fn parse(_input: &str) -> Result<ToolCommand, String> {
    Err(USAGE.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn operator_tool_list_is_local_and_accepts_the_empty_default() {
        assert_eq!(parse("list").unwrap(), ToolCommand::List);
        assert_eq!(parse("").unwrap(), ToolCommand::List);
    }

    #[test]
    fn operator_tool_use_keeps_optional_arguments_absent() {
        assert_eq!(parse("use list_mcp_resources").unwrap(), ToolCommand::Use {
            name: "list_mcp_resources".into(), arguments: None,
        });
    }

    #[test]
    fn operator_tool_use_preserves_json_quotes_and_whitespace() {
        assert_eq!(parse(r#"use shell {"command":["printf","hello world"]}"#).unwrap(), ToolCommand::Use {
            name: "shell".into(), arguments: Some(json!({"command":["printf","hello world"]})),
        });
        assert_eq!(parse(r#"use apply_patch "line one\nline two""#).unwrap(), ToolCommand::Use {
            name: "apply_patch".into(), arguments: Some(json!("line one\nline two")),
        });
    }

    #[test]
    fn operator_tool_commands_reject_missing_names_invalid_json_and_large_arguments() {
        for input in ["use", "list extra", "unknown", "use shell {bad-json}"] {
            assert!(parse(input).is_err());
        }
        assert!(parse(&format!("use shell {}", " ".repeat(MAX_ARGUMENT_BYTES + 1))).is_err());
    }
}
