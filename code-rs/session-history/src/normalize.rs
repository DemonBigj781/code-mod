use crate::{Error, Result, files};
use code_secrets::{redact_json, redact_secrets};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;

pub(crate) fn timestamp_ms(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_f64() {
        return (number.is_finite() && number >= 0.0).then(|| {
            if number >= 1_000_000_000_000.0 {
                number as i64
            } else {
                (number * 1000.0) as i64
            }
        });
    }
    value
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|time| time.timestamp_millis())
}

pub(crate) fn clean(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key, clean(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(clean).collect()),
        other => other,
    }
}

pub(crate) fn private_message(item: &Value) -> bool {
    let channel = item["channel"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or(item["metadata"]["channel"].as_str());
    let hidden = |value: &Value| !value.is_null() && value != &Value::Bool(false);
    channel.is_some_and(|channel| {
        ["analysis", "reasoning", "justify", "confidence", "summary"]
            .iter()
            .any(|name| channel.eq_ignore_ascii_case(name))
    }) || hidden(&item["metadata"]["is_visually_hidden_from_conversation"])
        || hidden(&item["metadata"]["is_user_system_message"])
}

pub(crate) fn content_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    content
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    matches!(
                        item["type"].as_str(),
                        Some("input_text" | "output_text" | "text")
                    )
                })
                .filter_map(|item| item["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

pub(crate) fn maybe_json(value: &Value) -> Value {
    if let Some(text) = value.as_str() {
        let text = text.trim();
        if matches!(text.as_bytes().first(), Some(b'{' | b'[')) {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                return parsed;
            }
        }
    }
    value.clone()
}

pub(crate) fn rollout_metadata(store: &str, path: &Path) -> Result<Value> {
    let mut metadata = None;
    files::visit_records(path, |_, record| {
        if record["type"] != "session_meta" {
            return true;
        }
        let payload = &record["payload"];
        let Some(id) = payload["session_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .or(payload["id"].as_str())
            .filter(|id| !id.is_empty())
        else {
            return false;
        };
        let export = payload["source"] == "openai_export";
        let modified = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
        metadata = Some(clean(json!({
            "store_id":store,"session_id":id,"ref":format!("{store}:{id}"),
            "source_kind":if export {"openai_export"} else {"rollout"},"rollout_path":path,
            "created_at_ms":timestamp_ms(payload.get("timestamp").filter(|value| !value.is_null() && value.as_str() != Some("")).unwrap_or(&record["timestamp"])),
            "updated_at_ms":if export {timestamp_ms(&payload["updated_at"])} else {modified},
            "title":payload["title"].as_str().map(|title| redact_secrets(title.to_owned())),"cwd":payload["cwd"].as_str(),"source":payload["source"].as_str(),
            "model_provider":payload["model_provider"].as_str(),"model":payload["model"].as_str(),
            "git_branch":payload["git"]["branch"].as_str(),"archived":payload["archived"].as_bool(),"item_count":payload["item_count"].as_u64()
        })));
        false
    })?;
    metadata.ok_or_else(|| Error::new("source_error", "rollout has no valid session metadata"))
}

pub(crate) fn visit_rollout(
    store: &str,
    session: &str,
    path: &Path,
    include_tools: bool,
    visit: &mut dyn FnMut(Value) -> bool,
) -> Result<()> {
    let mut turn = Value::Null;
    let mut names = HashMap::<String, String>::new();
    files::visit_records(path, |ordinal, record| {
        let payload = &record["payload"];
        if !payload.is_object() {
            return true;
        }
        match record["type"].as_str() {
            Some("turn_context" | "event" | "event_msg") => {
                if let Some(id) = payload.get("turn_id") {
                    if id.is_string() || (record["type"] == "turn_context" && id.is_null()) {
                        turn = id.clone();
                    }
                }
                return true;
            }
            Some("response_item") => {}
            _ => return true,
        }
        if private_message(payload) {
            return true;
        }
        let kind = payload["type"].as_str().unwrap_or("");
        let mut item = json!({"store_id":store,"session_id":session,"timestamp":record["timestamp"].as_str(),
            "created_at_ms":timestamp_ms(&record["timestamp"]),"ordinal":record["ordinal"].as_u64().unwrap_or(ordinal as u64),
            "turn_id":turn,"item_id":payload["id"].as_str(),"source_type":kind,
            "parent_item_id":record["parent_item_id"].as_str(),"is_active_branch":record["is_active_branch"].as_bool()});
        match kind {
            "message" if matches!(payload["role"].as_str(), Some("user" | "assistant")) => {
                let text = content_text(&payload["content"]);
                if text.is_empty() {
                    return true;
                }
                item["kind"] = json!("message");
                item["role"] = payload["role"].clone();
                item["text"] = json!(redact_secrets(text));
            }
            "function_call" | "custom_tool_call" if include_tools => {
                if let (Some(call), Some(name)) =
                    (payload["call_id"].as_str(), payload["name"].as_str())
                {
                    names.insert(call.to_owned(), name.to_owned());
                }
                item["kind"] = json!("tool_call");
                item["tool_name"] = json!(payload["name"].as_str());
                item["tool_call_id"] = json!(payload["call_id"].as_str());
                let arguments = maybe_json(
                    &payload[if kind == "custom_tool_call" {
                        "input"
                    } else {
                        "arguments"
                    }],
                );
                item["arguments"] = redact_json(&arguments);
            }
            "function_call_output" | "custom_tool_call_output" if include_tools => {
                item["kind"] = json!("tool_result");
                item["tool_call_id"] = json!(payload["call_id"].as_str());
                item["tool_name"] = payload["call_id"]
                    .as_str()
                    .and_then(|id| names.get(id))
                    .map(|v| json!(v))
                    .unwrap_or(Value::Null);
                item["result"] = redact_json(&maybe_json(&payload["output"]));
            }
            _ => return true,
        }
        visit(clean(item))
    })
}

pub(crate) fn projected_items(base: Value, payload: &Value, include_tools: bool) -> Vec<Value> {
    if private_message(payload) {
        return Vec::new();
    }
    let mut item = base;
    let kind = item["source_type"]
        .as_str()
        .or(payload["type"].as_str())
        .unwrap_or("")
        .to_owned();
    if matches!(kind.as_str(), "userMessage" | "agentMessage") {
        let text = if kind == "userMessage" {
            payload["content"]
                .as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| {
                    payload["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|part| part["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
        } else {
            payload["text"].as_str().unwrap_or("").to_owned()
        };
        if text.is_empty() {
            return Vec::new();
        }
        item["kind"] = json!("message");
        item["role"] = json!(if kind == "userMessage" {
            "user"
        } else {
            "assistant"
        });
        item["text"] = json!(redact_secrets(text));
        return vec![clean(item)];
    }
    if !include_tools {
        return Vec::new();
    }
    let (name, args, result) = match kind.as_str() {
        "commandExecution" => (
            "shell".to_owned(),
            json!({"command":payload["command"],"cwd":payload["cwd"],"source":payload["source"]}),
            json!({"status":payload["status"],"exit_code":payload["exitCode"],"duration_ms":payload["durationMs"],"output":payload["aggregatedOutput"]}),
        ),
        "mcpToolCall" => (
            [payload["server"].as_str(), payload["tool"].as_str()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("."),
            payload["arguments"].clone(),
            json!({"status":payload["status"],"result":payload["result"],"error":payload["error"],"duration_ms":payload["durationMs"]}),
        ),
        "fileChange" => (
            "file_change".to_owned(),
            json!({"changes":payload["changes"]}),
            json!({"status":payload["status"]}),
        ),
        "webSearch" => (
            "web_search".to_owned(),
            json!({"query":payload["query"],"action":payload["action"]}),
            json!({"results":payload["results"]}),
        ),
        _ => return Vec::new(),
    };
    let id = item["item_id"].as_str().map(ToOwned::to_owned);
    item["tool_call_id"] = payload
        .get("id")
        .cloned()
        .unwrap_or_else(|| item["item_id"].clone());
    item["tool_name"] = json!(name);
    let mut call = item.clone();
    call["kind"] = json!("tool_call");
    call["arguments"] = redact_json(&args);
    let mut output = item;
    output["kind"] = json!("tool_result");
    output["result"] = redact_json(&result);
    if let Some(id) = id {
        call["item_id"] = json!(format!("{id}:call"));
        output["item_id"] = json!(format!("{id}:result"));
    }
    vec![clean(call), clean(output)]
}

pub(crate) fn history_item(store: &str, ordinal: usize, record: &Value) -> Option<Value> {
    let id = record["session_id"].as_str()?;
    let text = record["text"].as_str()?;
    Some(clean(
        json!({"store_id":store,"session_id":id,"kind":"message","role":"user","text":redact_secrets(text.to_owned()),"created_at_ms":timestamp_ms(&record["ts"]),"ordinal":ordinal,"source_type":"history"}),
    ))
}

pub(crate) fn searchable(item: &Value) -> String {
    if let Some(text) = item["text"].as_str() {
        return text.to_owned();
    }
    let value = &item[if item["kind"] == "tool_call" {
        "arguments"
    } else {
        "result"
    }];
    format!(
        "{} {}",
        item["tool_name"].as_str().unwrap_or(""),
        value
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| value.to_string())
    )
}

pub(crate) struct Page {
    offset: usize,
    limit: usize,
    budget: usize,
    seen: usize,
    pub(crate) matched: usize,
    pub(crate) items: Vec<Value>,
    pub(crate) truncated: bool,
}
impl Page {
    pub(crate) fn new(offset: usize, limit: usize, budget: usize) -> Self {
        Self {
            offset,
            limit,
            budget,
            seen: 0,
            matched: 0,
            items: Vec::new(),
            truncated: false,
        }
    }
    pub(crate) fn push(&mut self, mut item: Value) -> bool {
        self.matched += 1;
        if self.seen < self.offset {
            self.seen += 1;
            return true;
        }
        if self.items.len() == self.limit {
            self.truncated = true;
            return false;
        }
        let field = if item.get("text").is_some() {
            "text"
        } else if item["kind"] == "tool_call" {
            "arguments"
        } else {
            "result"
        };
        let value = &item[field];
        let text = if value.is_null() {
            String::new()
        } else {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| value.to_string())
        };
        let size = text.chars().count();
        if size > self.budget {
            if self.budget > 0 {
                item[field] = json!(format!(
                    "{}\n[TRUNCATED {} CHARS]",
                    text.chars().take(self.budget).collect::<String>(),
                    size - self.budget
                ));
                self.items.push(item);
            }
            self.truncated = true;
            return false;
        }
        self.budget -= size;
        self.items.push(item);
        true
    }
    pub(crate) fn output(self) -> Value {
        json!({"next_offset":self.offset+self.items.len(),"items":self.items,"truncated":self.truncated})
    }
}
