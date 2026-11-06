use super::helpers::get_browser_manager_for_session;
use super::helpers::inspect_dom_target;
use crate::codex::Session;
use crate::codex::ToolCallCtx;
use crate::tools::events::execute_custom_tool;
use crate::tools::handlers::{tool_error, tool_output};
use code_protocol::models::ResponseInputItem;
use serde_json::Value;

fn format_inspection(target: &str, inspection: &Value) -> String {
    use std::fmt::Write;

    let mut output = String::new();
    let _ = writeln!(output, "Target: {target}");
    if let Some(tag_name) = inspection.get("tagName").and_then(Value::as_str)
        && !tag_name.is_empty()
    {
        let _ = writeln!(output, "Element: {}", tag_name.to_ascii_lowercase());
    }
    if let Some(attributes) = inspection.get("attributes").and_then(Value::as_array) {
        output.push_str("Attributes:\n");
        let mut values = attributes.iter();
        while let (Some(name), Some(value)) = (values.next(), values.next()) {
            let _ = writeln!(
                output,
                "  {}=\"{}\"",
                name.as_str().unwrap_or(""),
                value.as_str().unwrap_or("")
            );
        }
    }
    if let Some(html) = inspection.get("outerHTML").and_then(Value::as_str) {
        let single_line = html.replace('\n', " ");
        let snippet: String = single_line
            .chars()
            .take(crate::util::MAX_SNIPPET_CHARS)
            .collect();
        output.push_str("\nOuterHTML (truncated):\n");
        output.push_str(&snippet);
        if single_line.chars().count() > snippet.chars().count() {
            output.push('…');
        }
        output.push('\n');
    }
    if inspection.get("boxModel").is_some() {
        output.push_str("\nBoxModel: available (content rectangle)\n");
    }
    if let Some(count) = inspection.get("matchedCssRules").and_then(Value::as_u64) {
        let _ = writeln!(output, "Matched CSS rules: {count}");
    }
    output
}

pub(super) async fn handle_browser_inspect_selector(
    sess: &Session,
    ctx: &ToolCallCtx,
    arguments: String,
) -> ResponseInputItem {
    let params = serde_json::from_str::<Value>(&arguments).ok();
    let call_id = ctx.call_id.clone();
    execute_custom_tool(
        sess,
        ctx,
        "browser_inspect_selector".to_owned(),
        params,
        || async move {
            let Some(browser_manager) = get_browser_manager_for_session(sess).await else {
                return tool_error(call_id, "Browser is not initialized. Use browser_open to start the browser.");
            };
            let Ok(arguments) = serde_json::from_str::<Value>(&arguments) else {
                return tool_error(call_id, "Invalid inspect_selector arguments");
            };
            let selector = arguments
                .get("selector")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if selector.is_empty() {
                return tool_error(call_id, "selector must be non-empty");
            }
            match inspect_dom_target(&browser_manager, Some(selector), None, None).await {
                Ok(inspection) => tool_output(
                    call_id,
                    format_inspection(&format!("selector '{selector}'"), &inspection),
                ),
                Err(error) => tool_error(call_id, error),
            }
        },
    )
    .await
}

pub(super) async fn handle_browser_inspect(
    sess: &Session,
    ctx: &ToolCallCtx,
    arguments: String,
) -> ResponseInputItem {
    let params = serde_json::from_str::<Value>(&arguments).ok();
    let call_id = ctx.call_id.clone();
    execute_custom_tool(sess, ctx, "browser_inspect".to_owned(), params, || async move {
        let Some(browser_manager) = get_browser_manager_for_session(sess).await else {
            return tool_error(call_id, "Browser is not initialized. Use browser_open to start the browser.");
        };
        let Ok(arguments) = serde_json::from_str::<Value>(&arguments) else {
            return tool_error(call_id, "Invalid browser_inspect arguments");
        };
        let id = arguments.get("id").and_then(Value::as_str);
        let coordinates = match (
            arguments.get("x").and_then(Value::as_f64),
            arguments.get("y").and_then(Value::as_f64),
        ) {
            (Some(x), Some(y)) => Some((x, y)),
            _ if id.is_none() => browser_manager.get_cursor_position().await.ok(),
            _ => None,
        };
        let label = if let Some(id) = id {
            format!("id '#{id}'")
        } else if let Some((x, y)) = coordinates {
            format!("coordinates ({x}, {y})")
        } else {
            "current cursor".to_owned()
        };
        match inspect_dom_target(&browser_manager, None, id, coordinates).await {
            Ok(inspection) => tool_output(call_id, format_inspection(&label, &inspection)),
            Err(error) => tool_error(call_id, error),
        }
    })
    .await
}
