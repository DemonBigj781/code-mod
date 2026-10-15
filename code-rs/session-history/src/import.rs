use crate::{Error, Result, Settings, files, normalize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;

const FORMAT: &str = "code-session-mcp.openai-export.v1";
const MAX_JSON: u64 = 128 * 1024 * 1024;
const MAX_TOTAL_JSON: u64 = 512 * 1024 * 1024;

pub(crate) fn dataset_id(name: &str) -> Result<String> {
    if !(1..=64).contains(&name.len())
        || !name.as_bytes()[0].is_ascii_lowercase() && !name.as_bytes()[0].is_ascii_digit()
        || !name
            .bytes()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'-' || ch == b'_')
    {
        return Err(Error::new(
            "invalid_argument",
            "dataset_name must be 1-64 lowercase ASCII letters, digits, hyphens or underscores, starting with a letter or digit",
        ));
    }
    Ok(format!("openai-{name}"))
}

pub(crate) fn read_manifest(root: &Path) -> Result<Value> {
    let file = files::open_regular(&root.join("openai-export.json"))?;
    let mut bytes = Vec::new();
    file.take(16_385).read_to_end(&mut bytes)?;
    if bytes.len() > 16_384 {
        return Err(Error::new(
            "invalid_export",
            "export manifest exceeds size limit",
        ));
    }
    let value: Value = serde_json::from_slice(&bytes)?;
    if value["format"] != FORMAT {
        return Err(Error::new(
            "invalid_export",
            "unsupported export dataset format",
        ));
    }
    let mut out = json!({"format":FORMAT});
    for key in [
        "session_count",
        "item_count",
        "excluded_message_count",
        "non_text_message_count",
    ] {
        let count = value[key]
            .as_u64()
            .ok_or_else(|| Error::new("invalid_export", "invalid export dataset counts"))?;
        out[key] = json!(count);
    }
    Ok(out)
}

fn load_document(reader: impl Read) -> Result<Vec<Value>> {
    let mut bytes = Vec::new();
    reader.take(MAX_JSON + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON {
        return Err(Error::new(
            "limit_exceeded",
            "conversation JSON exceeds per-file byte limit",
        ));
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    serde_json::from_slice::<Vec<Value>>(bytes).map_err(|_| {
        Error::new(
            "invalid_export",
            "conversation file must be a UTF-8 JSON array",
        )
    })
}

fn conversation_member(name: &str) -> bool {
    let basename = name.rsplit('/').next().unwrap_or(name);
    basename == "conversations.json"
        || basename
            .strip_prefix("conversations-")
            .and_then(|n| n.strip_suffix(".json"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn valid_id<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.chars().count() <= 256 && !s.chars().any(char::is_control))
        .ok_or_else(|| Error::new("invalid_export", format!("missing or invalid {field}")))
}

fn timestamp(value: &Value) -> Value {
    normalize::timestamp_ms(value)
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|time| json!(time.to_rfc3339()))
        .unwrap_or(Value::Null)
}

#[derive(Default)]
struct Counts {
    sessions: usize,
    items: usize,
    excluded: usize,
    non_text: usize,
}

fn message(node: &str, message: &Value, counts: &mut Counts) -> Result<Option<Value>> {
    if message.is_null() {
        return Ok(None);
    }
    let author = &message["author"];
    let content = &message["content"];
    if !author.is_object()
        || !content.is_object()
        || message
            .get("metadata")
            .is_some_and(|v| !v.is_null() && !v.is_object())
    {
        return Err(Error::new(
            "invalid_export",
            "message author, content and metadata must be objects",
        ));
    }
    let role = author["role"]
        .as_str()
        .ok_or_else(|| Error::new("invalid_export", "message role must be a string"))?;
    let kind = content["content_type"]
        .as_str()
        .ok_or_else(|| Error::new("invalid_export", "message content_type must be a string"))?;
    if [message.get("channel"), message["metadata"].get("channel")]
        .into_iter()
        .flatten()
        .filter(|value| !value.is_null())
        .any(|value| !value.is_string())
    {
        return Err(Error::new(
            "invalid_export",
            "message channel must be a string",
        ));
    }
    if !matches!(role, "user" | "assistant" | "tool")
        || normalize::private_message(message)
        || matches!(kind, "thoughts" | "reasoning_recap" | "reasoning")
    {
        counts.excluded += 1;
        return Ok(None);
    }
    let mut non_text = false;
    let text = if matches!(
        kind,
        "code" | "execution_output" | "text" | "audio_transcription"
    ) && content["text"].is_string()
    {
        content["text"].as_str().unwrap_or_default().to_owned()
    } else {
        let mut parts = Vec::new();
        if matches!(kind, "text" | "multimodal_text") && content["parts"].is_array() {
            for part in content["parts"].as_array().into_iter().flatten() {
                if let Some(text) = part.as_str() {
                    parts.push(text.to_owned());
                } else if matches!(
                    part["content_type"].as_str(),
                    Some("text" | "audio_transcription")
                ) && part["text"].is_string()
                {
                    parts.push(part["text"].as_str().unwrap_or_default().to_owned());
                } else {
                    non_text = true;
                }
            }
        } else {
            non_text = true;
        }
        if non_text {
            parts.push("[Non-text export content omitted]".into());
        }
        parts.join("\n")
    };
    if non_text {
        counts.non_text += 1;
    }
    if text.is_empty() {
        counts.excluded += 1;
        return Ok(None);
    }
    let recipient = message["recipient"].as_str();
    Ok(Some(if role == "tool" {
        json!({"id":node,"type":"function_call_output","output":code_secrets::redact_json(&tool_value(&text))})
    } else if role == "assistant" && recipient.is_some_and(|r| !r.is_empty() && r != "all") {
        json!({"id":node,"type":"function_call","call_id":node,"name":recipient.map(|name| code_secrets::redact_secrets(name.to_owned()).chars().take(256).collect::<String>()),"arguments":code_secrets::redact_json(&tool_value(&text))})
    } else {
        json!({"id":node,"type":"message","role":role,"content":[{"type":"text","text":code_secrets::redact_secrets(text)}]})
    }))
}

fn tool_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!(text))
}

fn records(conversation: &Value, counts: &mut Counts) -> Result<(String, Vec<Value>)> {
    let id = valid_id(
        conversation
            .get("id")
            .filter(|value| !value.is_null() && value.as_str() != Some(""))
            .unwrap_or(&conversation["conversation_id"]),
        "conversation ID",
    )?
    .to_owned();
    let graph = conversation["mapping"]
        .as_object()
        .ok_or_else(|| Error::new("invalid_export", "conversation mapping must be an object"))?;
    if graph.len() > 100_000 {
        return Err(Error::new(
            "limit_exceeded",
            "conversation has too many nodes",
        ));
    }
    let mut children = HashMap::<Option<&str>, Vec<&str>>::new();
    for (key, node) in graph {
        valid_id(&json!(key), "node ID")?;
        if !node.is_object() {
            return Err(Error::new("invalid_export", "node must be an object"));
        }
        let parent = node["parent"].as_str();
        if (!node["parent"].is_null() && parent.is_none())
            || parent.is_some_and(|p| !graph.contains_key(p))
        {
            return Err(Error::new(
                "invalid_export",
                "conversation contains a missing parent",
            ));
        }
        children.entry(parent).or_default().push(key);
    }
    let mut ordered = Vec::new();
    let mut pending = children.get(&None).cloned().unwrap_or_default();
    pending.reverse();
    while let Some(node) = pending.pop() {
        ordered.push(node);
        if let Some(kids) = children.get(&Some(node)) {
            pending.extend(kids.iter().rev().copied());
        }
    }
    if ordered.len() != graph.len() {
        return Err(Error::new(
            "invalid_export",
            "conversation contains a parent cycle",
        ));
    }
    let active = if !conversation["current_node"].is_null() {
        let mut current = Some(valid_id(&conversation["current_node"], "current_node")?);
        let mut active = HashSet::new();
        while let Some(key) = current {
            let node = graph
                .get(key)
                .ok_or_else(|| Error::new("invalid_export", "current_node does not exist"))?;
            active.insert(key);
            current = node["parent"].as_str();
        }
        Some(active)
    } else {
        None
    };
    let mut turns = HashMap::<&str, Option<String>>::new();
    let mut output = Vec::new();
    for key in ordered {
        let node = &graph[key];
        let parent = node["parent"].as_str();
        let mut turn = parent.and_then(|p| turns.get(p)).cloned().flatten();
        let payload = message(key, &node["message"], counts)?;
        if payload.as_ref().is_some_and(|p| p["role"] == "user") {
            turn = Some(key.to_owned());
        }
        turns.insert(key, turn.clone());
        let Some(mut payload) = payload else {
            continue;
        };
        if payload["type"] == "function_call_output" {
            payload["call_id"] = json!(parent);
        }
        output.push(json!({"type":"turn_context","payload":{"turn_id":turn}}));
        output.push(json!({"type":"response_item","payload":payload,"timestamp":timestamp(&node["message"]["create_time"]),"parent_item_id":parent,"is_active_branch":active.as_ref().map(|set|set.contains(key))}));
    }
    let item_count = output.len() / 2;
    counts.items += item_count;
    output.insert(0,json!({"type":"session_meta","payload":{
        "id":id,"source":"openai_export","model_provider":"openai",
        "title":conversation["title"].as_str().map(|s|code_secrets::redact_secrets(s.to_owned()).chars().take(500).collect::<String>()),
        "model":conversation["default_model_slug"].as_str().map(|s|code_secrets::redact_secrets(s.to_owned()).chars().take(256).collect::<String>()),
        "timestamp":timestamp(&conversation["create_time"]),"updated_at":timestamp(&conversation["update_time"]),
        "archived":conversation["is_archived"].as_bool(),"item_count":item_count}}));
    Ok((id, output))
}

fn private_dir(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}
fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(
        path.parent()
            .ok_or_else(|| Error::new("invalid_export", "missing parent"))?,
    )?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path).map_err(|e| e.error)?;
    Ok(())
}

pub(crate) fn import_export(settings: &Settings, filename: &str, name: &str, staging_parent: Option<&Path>) -> Result<Value> {
    if filename.is_empty() || matches!(filename, "." | "..") || filename.contains(['/', '\\', ':'])
    {
        return Err(Error::new(
            "invalid_argument",
            "file_name must be a basename inside the import inbox",
        ));
    }
    let id = dataset_id(name)?;
    if settings.stores.iter().any(|store| store.id == id) {
        return Err(Error::new(
            "dataset_exists",
            "dataset ID conflicts with a configured store",
        ));
    }
    let inbox = settings.data_dir.join("inbox");
    let path = inbox.join(filename);
    let file = files::open_regular(&path)?;
    files::allowed_path(&inbox, &path)?;
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if !matches!(extension.as_str(), "json" | "zip") {
        return Err(Error::new(
            "invalid_argument",
            "expected a JSON or ZIP export",
        ));
    }
    if file.metadata()?.len()
        > if extension == "json" {
            MAX_JSON
        } else {
            8 * 1024 * 1024 * 1024
        }
    {
        return Err(Error::new("limit_exceeded", "export exceeds byte limit"));
    }
    let datasets = settings.data_dir.canonicalize()?.join("datasets");
    if datasets
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(Error::new(
            "invalid_argument",
            "datasets directory must not be a symlink",
        ));
    }
    private_dir(&datasets)?;
    let destination = datasets.join(&id);
    if destination.symlink_metadata().is_ok() {
        return Err(Error::new(
            "dataset_exists",
            "dataset already exists; choose a new dataset_name",
        ));
    }
    let staging = tempfile::Builder::new()
        .prefix(".import-")
        .tempdir_in(staging_parent.unwrap_or(&datasets))?;
    private_dir(&staging.path().join("sessions"))?;
    let mut counts = Counts::default();
    let mut seen = HashSet::new();
    let mut consume = |document: Vec<Value>| -> Result<()> {
        for conversation in document {
            if counts.sessions >= 50_000 {
                return Err(Error::new("limit_exceeded", "too many conversations"));
            }
            let (session, records) = records(&conversation, &mut counts)?;
            if !seen.insert(session.clone()) {
                return Err(Error::new("invalid_export", "duplicate conversation IDs"));
            }
            let filename = format!("rollout-{:x}.jsonl", Sha256::digest(session.as_bytes()));
            let mut bytes = Vec::new();
            for record in records {
                serde_json::to_writer(&mut bytes, &record)?;
                bytes.push(b'\n');
            }
            private_write(&staging.path().join("sessions").join(filename), &bytes)?;
            counts.sessions += 1;
        }
        Ok(())
    };
    if extension == "json" {
        consume(load_document(file)?)?;
    } else {
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|_| Error::new("invalid_export", "invalid ZIP archive"))?;
        if archive.len() > 50_000 {
            return Err(Error::new("limit_exceeded", "too many ZIP entries"));
        }
        let mut selected = Vec::new();
        let mut total = 0u64;
        for i in 0..archive.len() {
            let member = archive
                .by_index_raw(i)
                .map_err(|_| Error::new("invalid_export", "unsupported ZIP member"))?;
            if member.is_dir() || !conversation_member(member.name()) {
                continue;
            }
            if member.name().starts_with('/')
                || member.name().contains('\\')
                || member.name().split('/').any(|part| part == "..")
            {
                return Err(Error::new(
                    "invalid_export",
                    "unsafe conversation member path",
                ));
            }
            if member.size() > MAX_JSON {
                return Err(Error::new(
                    "limit_exceeded",
                    "conversation JSON exceeds per-file byte limit",
                ));
            }
            total = total.saturating_add(member.size());
            if total > MAX_TOTAL_JSON {
                return Err(Error::new(
                    "limit_exceeded",
                    "conversation JSON exceeds total byte limit",
                ));
            }
            selected.push((member.name().to_owned(), i));
        }
        if selected.is_empty() {
            return Err(Error::new(
                "invalid_export",
                "ZIP has no conversation JSON files",
            ));
        }
        selected.sort();
        for (_, index) in selected {
            let member = archive
                .by_index(index)
                .map_err(|_| Error::new("invalid_export", "cannot read conversation member"))?;
            let document = load_document(member).map_err(|error| {
                if error.code == "source_error" {
                    Error::new(
                        "invalid_export",
                        "ZIP conversation member is corrupt or unsupported",
                    )
                } else {
                    error
                }
            })?;
            consume(document)?;
        }
    }
    let manifest = json!({"format":FORMAT,"store_id":id,"session_count":counts.sessions,"item_count":counts.items,"excluded_message_count":counts.excluded,"non_text_message_count":counts.non_text});
    // Reserve the dataset name exclusively; the manifest is published only after all session files.
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&destination).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            Error::new("dataset_exists", "dataset already exists")
        } else {
            error.into()
        }
    })?;
    let publish = (|| -> Result<()> {
        std::fs::rename(
            staging.path().join("sessions"),
            destination.join("sessions"),
        )?;
        private_write(
            &destination.join("openai-export.json"),
            &serde_json::to_vec(&manifest)?,
        )?;
        Ok(())
    })();
    if let Err(error) = publish {
        let _ = std::fs::remove_dir_all(&destination);
        return Err(error);
    }
    let mut result = manifest;
    result["dataset_path"] = json!(destination);
    Ok(result)
}
