//! Model-free operator calls using the same handlers and policies as model calls.

use super::Session;
use crate::openai_tools::{OpenAiTool, get_openai_tools};
use crate::protocol::{EventMsg, TaskCompleteEvent};
use crate::tools::router::{ToolDispatchMeta, ToolRouter};
use crate::turn_diff_tracker::TurnDiffTracker;
use code_protocol::models::{
    FunctionCallOutputBody, FunctionCallOutputContentItem, ResponseInputItem, ResponseItem,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::Arc;

const MAX_ARGUMENT_BYTES: usize = 1024 * 1024;
const MAX_NOTICE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Function,
    Freeform,
    Provider,
    Unavailable,
}

struct Tool {
    name: String,
    source: String,
    kind: Kind,
    schema: Option<Value>,
    reason: Option<String>,
}

impl Tool {
    fn prepare(&self, arguments: Option<Value>, call_id: String) -> Result<ResponseItem, String> {
        match self.kind {
            Kind::Provider => return Err("This is a provider-native tool; it requires a model request and cannot be invoked directly.".into()),
            Kind::Unavailable => return Err(self.reason.clone().unwrap_or_else(|| "Tool is not enabled for this session.".into())),
            Kind::Freeform => {
                let Some(Value::String(input)) = arguments else {
                    return Err("This tool requires its raw input as a JSON string.".into());
                };
                if input.len() > MAX_ARGUMENT_BYTES { return Err("Tool input exceeds 1 MiB.".into()); }
                return Ok(ResponseItem::CustomToolCall { id: None, status: None, call_id,
                    name: self.name.clone(), input });
            }
            Kind::Function => {},
        }
        let arguments = arguments.unwrap_or_else(|| json!({}));
        let Some(object) = arguments.as_object() else {
            return Err("Function-tool arguments must be a JSON object.".into());
        };
        let Some(schema) = self.schema.as_ref().and_then(Value::as_object) else {
            return Err("The loaded tool has no usable input schema.".into());
        };
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            let missing = required
                .iter()
                .filter_map(Value::as_str)
                .filter(|key| !object.contains_key(*key))
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(format!(
                    "Missing required arguments: {}",
                    missing.join(", ")
                ));
            }
        }
        let arguments = arguments.to_string();
        if arguments.len() > MAX_ARGUMENT_BYTES {
            return Err("Tool arguments exceed 1 MiB.".into());
        }
        Ok(ResponseItem::FunctionCall {
            id: None,
            name: self.name.clone(),
            namespace: None,
            arguments,
            call_id,
        })
    }
}

fn insert(catalog: &mut BTreeMap<String, Tool>, tool: Tool) {
    if let Some(existing) = catalog.get_mut(&tool.name) {
        existing.kind = Kind::Unavailable;
        existing.reason =
            Some("Ambiguous loaded tool name; resolve the naming conflict before use.".into());
    } else {
        catalog.insert(tool.name.clone(), tool);
    }
}

fn catalog(sess: &Session) -> BTreeMap<String, Tool> {
    let config = sess.client.build_tools_config();
    let mut tools = BTreeMap::new();
    let specs = get_openai_tools(
        &config,
        None,
        cfg!(feature = "browser-automation"),
        false,
        &[],
    );
    for spec in specs {
        let spec = if matches!(spec, OpenAiTool::LocalShell {}) {
            crate::openai_tools::operator_shell_tool()
        } else {
            spec
        };
        let (name, kind, schema) = match spec {
            OpenAiTool::Function(spec) => (
                spec.name,
                Kind::Function,
                serde_json::to_value(spec.parameters).ok(),
            ),
            OpenAiTool::Freeform(spec) => (spec.name, Kind::Freeform, None),
            OpenAiTool::WebSearch(_) => ("web_search".into(), Kind::Provider, None),
            OpenAiTool::LocalShell {} => unreachable!("local shell normalized above"),
        };
        let source = "native".to_owned();
        insert(
            &mut tools,
            Tool {
                name,
                source,
                kind,
                schema,
                reason: None,
            },
        );
    }
    for spec in &sess.dynamic_tools {
        // The current dispatcher supports only unnamespaced dynamic calls.
        let (name, kind, reason) = match &spec.namespace {
            Some(namespace) => (
                format!("{namespace}.{}", spec.name),
                Kind::Unavailable,
                Some("Namespaced client tools are not supported by the direct dispatcher.".into()),
            ),
            None => (spec.name.clone(), Kind::Function, None),
        };
        insert(
            &mut tools,
            Tool {
                name,
                source: "client".into(),
                kind,
                schema: Some(spec.input_schema.clone()),
                reason,
            },
        );
    }
    for name in ToolRouter::global().registered_names() {
        tools.entry(name.clone()).or_insert(Tool {
            name,
            source: "native".into(),
            kind: Kind::Unavailable,
            schema: None,
            reason: Some("Registered handler is not enabled/exposed for this session.".into()),
        });
    }
    for (name, server, tool) in sess
        .mcp_connection_manager()
        .list_all_tools_with_server_names()
    {
        insert(
            &mut tools,
            Tool {
                name,
                source: format!("MCP {server}"),
                kind: Kind::Function,
                schema: serde_json::to_value(tool.input_schema).ok(),
                reason: None,
            },
        );
    }
    tools
}

fn append_preview(output: &mut String, text: &str, budget: &mut usize) -> bool {
    for character in text.chars() {
        let size = character.len_utf8();
        if size > *budget {
            return false;
        }
        *budget -= size;
        if !character.is_control() || matches!(character, '\n' | '\t') {
            output.push(character);
        }
    }
    true
}

fn bounded_notice(text: &str) -> String {
    let mut output = String::new();
    let mut budget = MAX_NOTICE_BYTES - 80;
    if !append_preview(&mut output, text, &mut budget) {
        output.push_str("\n[Operator tool notice truncated at 64 KiB]");
    }
    let redacted = code_secrets::redact_secrets(output);
    let mut output = String::new();
    let mut budget = MAX_NOTICE_BYTES - 80;
    if !append_preview(&mut output, &redacted, &mut budget) {
        output.push_str("\n[Operator tool notice truncated at 64 KiB]");
    }
    output
}

pub(super) async fn respond(sess: &Session, sub_id: &str, message: String, success: Option<bool>) {
    sess.send_event(sess.make_event(
        sub_id,
        EventMsg::OperatorToolResponse {
            message: bounded_notice(&message),
            success,
        },
    ))
    .await;
}

pub(super) async fn list(sess: &Session, sub_id: &str) {
    let catalog = catalog(sess);
    let mut message = format!("Tool inventory: {} entries\n", catalog.len());
    for tool in catalog.values() {
        let availability = match tool.kind {
            Kind::Function => "JSON arguments",
            Kind::Freeform => "JSON string input",
            Kind::Provider => "provider-native; not directly callable",
            Kind::Unavailable => "unavailable",
        };
        let _ = writeln!(
            message,
            "{} — {} — {}",
            tool.name, tool.source, availability
        );
    }
    let servers = sess.mcp_connection_manager().list_tools_by_server();
    let disabled = sess
        .mcp_connection_manager()
        .list_disabled_tools_by_server();
    let failures = sess.mcp_connection_manager().list_server_failures();
    let mut names = servers
        .keys()
        .chain(disabled.keys())
        .chain(failures.keys())
        .cloned()
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    let _ = writeln!(
        message,
        "\nMCP servers in the current inventory: {}",
        names.len()
    );
    for name in names {
        let count = servers.get(&name).map_or(0, Vec::len);
        let excluded = disabled.get(&name).map_or(0, Vec::len);
        let _ = writeln!(
            message,
            "{name}: {count} tools, {excluded} disabled{}",
            if failures.contains_key(&name) {
                ", unavailable/startup error"
            } else {
                ""
            }
        );
        if let Some(tools) = disabled.get(&name) {
            for tool in tools {
                let _ = writeln!(message, "  {name}/{tool} — disabled");
            }
        }
    }
    message.push_str("\nUse /tool use <name> [JSON arguments]. Existing approvals and sandbox rules still apply.");
    respond(sess, sub_id, message, Some(true)).await;
}

struct TaskCleanup {
    session: Arc<Session>,
    id: String,
}
impl Drop for TaskCleanup {
    fn drop(&mut self) {
        self.session.remove_task(&self.id);
    }
}

pub(super) async fn run(
    sess: Arc<Session>,
    sub_id: String,
    name: String,
    arguments: Option<Value>,
) {
    let _cleanup = TaskCleanup {
        session: sess.clone(),
        id: sub_id.clone(),
    };
    let _mcp_permissions = super::mcp_access::McpTurnAllowGuard::new(sess.clone(), sub_id.clone());
    sess.send_event(sess.make_event(&sub_id, EventMsg::TaskLifecycle(
        crate::protocol::TaskLifecycleEvent {
            phase: crate::protocol::TaskLifecyclePhase::Started,
            origin: crate::protocol::TaskOriginKind::OperatorTool,
            visible_to_user: true,
            last_agent_message: None,
        },
    ))).await;
    sess.send_event(sess.make_event(&sub_id, EventMsg::TaskStarted))
        .await;
    let catalog = catalog(&sess);
    let request = match catalog.get(&name) {
        Some(tool) => tool.prepare(arguments, format!("operator-tool-{}", uuid::Uuid::new_v4())),
        None => Err("Tool is not loaded. Run /tool list for exact available names.".into()),
    };
    match request {
        Err(message) => respond(&sess, &sub_id, message, Some(false)).await,
        Ok(item) => {
            if sess
                .mcp_connection_manager()
                .parse_tool_name(&name)
                .is_some()
            {
                // An explicit operator selection satisfies discovery, never server/tool enablement.
                sess.merge_mcp_tool_selection(vec![name.clone()]);
            }
            let mut tracker = TurnDiffTracker::new();
            let meta =
                ToolDispatchMeta::new(&sub_id, Some(0), Some(0), sess.current_request_ordinal());
            let result = ToolRouter::global()
                .dispatch_response_item(&sess, &mut tracker, meta, item)
                .await;
            let (success, output) = match result {
                Some(ResponseInputItem::FunctionCallOutput { output, .. })
                | Some(ResponseInputItem::CustomToolCallOutput { output, .. }) => {
                    let mut text = String::new();
                    let mut budget = MAX_NOTICE_BYTES - 256;
                    let complete = match &output.body {
                        FunctionCallOutputBody::Text(value) => {
                            append_preview(&mut text, value, &mut budget)
                        }
                        FunctionCallOutputBody::ContentItems(items) => {
                            let mut complete = true;
                            for item in items {
                                let value = match item {
                                    FunctionCallOutputContentItem::InputText { text } => {
                                        text.as_str()
                                    }
                                    FunctionCallOutputContentItem::InputImage { .. } => {
                                        "[image content returned]"
                                    }
                                };
                                if !append_preview(&mut text, value, &mut budget) {
                                    complete = false;
                                    break;
                                }
                                if !append_preview(&mut text, "\n", &mut budget) {
                                    complete = false;
                                    break;
                                }
                            }
                            complete
                        }
                    };
                    if !complete {
                        text.push_str("\n[Tool result preview truncated]");
                    }
                    (output.success, text)
                }
                _ => (Some(false), "Tool returned no supported result.".into()),
            };
            respond(
                &sess,
                &sub_id,
                format!("Operator tool {name}:\n{output}"),
                success,
            )
            .await;
        }
    }
    sess.revoke_deno_turn_permissions().await;
    sess.remove_task(&sub_id);
    sess.send_event(sess.make_event(
        &sub_id,
        EventMsg::TaskComplete(TaskCompleteEvent {
            last_agent_message: None,
        }),
    ))
    .await;
    super::streaming::resume_queued_work(sess);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function(required: Vec<&str>) -> Tool {
        Tool {
            name: "example".into(),
            source: "native".into(),
            kind: Kind::Function,
            schema: Some(json!({"type":"object","required":required})),
            reason: None,
        }
    }

    #[test]
    fn operator_tool_arguments_are_optional_only_when_the_tool_allows_it() {
        assert!(function(vec![]).prepare(None, "call".into()).is_ok());
        assert!(
            function(vec!["path"])
                .prepare(None, "call".into())
                .unwrap_err()
                .contains("path")
        );
        assert!(
            function(vec!["path"])
                .prepare(Some(json!({"path":"file"})), "call".into())
                .is_ok()
        );
        assert!(
            function(vec![])
                .prepare(Some(json!("not an object")), "call".into())
                .is_err()
        );
    }

    #[test]
    fn operator_tool_unavailable_provider_and_ambiguous_tools_never_prepare_calls() {
        let mut tool = function(vec![]);
        tool.kind = Kind::Unavailable;
        assert!(tool.prepare(None, "call".into()).is_err());
        tool.kind = Kind::Provider;
        assert!(tool.prepare(None, "call".into()).is_err());
        let mut tools = BTreeMap::new();
        insert(&mut tools, function(vec![]));
        insert(&mut tools, function(vec![]));
        assert!(tools["example"].prepare(None, "call".into()).is_err());
    }

    #[test]
    fn operator_tool_notices_are_bounded_and_strip_terminal_controls() {
        let text = bounded_notice(&format!("\x1b[2J{}", "字".repeat(MAX_NOTICE_BYTES)));
        assert!(!text.contains('\x1b'));
        assert!(text.len() <= MAX_NOTICE_BYTES);
        assert!(text.contains("truncated"));
        let text = bounded_notice(&"password=x ".repeat(MAX_NOTICE_BYTES));
        assert!(text.len() <= MAX_NOTICE_BYTES);
        assert!(!text.contains("password=x"));
    }
}
