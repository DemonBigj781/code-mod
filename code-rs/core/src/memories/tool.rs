use std::io;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    list_rollout_summaries, published_artifact_paths, read_memory_summary, read_raw_memories,
    read_rollout_summary,
};

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;
const DEFAULT_MAX_CHARS: usize = 50_000;
const MAX_MAX_CHARS: usize = 200_000;
const DEFAULT_LINE_COUNT: usize = 200;
const MAX_LINE_COUNT: usize = 2_000;
const MAX_SCAN_CHARS: usize = 16 * 1024 * 1024;
const MAX_ROLLOUTS_SCANNED: usize = 500;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Action {
    Status,
    Search,
    Read,
    ListRollouts,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    action: Action,
    query: Option<String>,
    path: Option<String>,
    source: Option<String>,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
    line_start: Option<usize>,
    line_count: Option<usize>,
    max_chars: Option<usize>,
}

pub(super) fn execute(code_home: &Path, arguments: &str) -> Result<Value, Value> {
    let request: Request = serde_json::from_str(arguments)
        .map_err(|error| tool_error("invalid_argument", error.to_string()))?;
    validate(&request)?;
    let result = match request.action {
        Action::Status => status(code_home),
        Action::Search => search(code_home, &request),
        Action::Read => read(code_home, &request),
        Action::ListRollouts => list_rollouts(code_home, &request),
    };
    result.map_err(|error| {
        let code = match error.kind() {
            io::ErrorKind::InvalidInput => "invalid_argument",
            io::ErrorKind::NotFound => "not_found",
            _ => "source_error",
        };
        tool_error(code, error.to_string())
    })
}

fn validate(request: &Request) -> Result<(), Value> {
    if request
        .limit
        .is_some_and(|limit| limit == 0 || limit > MAX_LIMIT)
    {
        return Err(tool_error(
            "limit_exceeded",
            format!("limit must be between 1 and {MAX_LIMIT}"),
        ));
    }
    if request
        .line_count
        .is_some_and(|count| count == 0 || count > MAX_LINE_COUNT)
    {
        return Err(tool_error(
            "limit_exceeded",
            format!("line_count must be between 1 and {MAX_LINE_COUNT}"),
        ));
    }
    if request
        .max_chars
        .is_some_and(|count| count == 0 || count > MAX_MAX_CHARS)
    {
        return Err(tool_error(
            "limit_exceeded",
            format!("max_chars must be between 1 and {MAX_MAX_CHARS}"),
        ));
    }
    if request.line_start == Some(0) {
        return Err(tool_error(
            "invalid_argument",
            "line_start must be one-based",
        ));
    }
    if request.action == Action::Search
        && request
            .query
            .as_deref()
            .is_none_or(|query| query.trim().is_empty())
    {
        return Err(tool_error(
            "invalid_argument",
            "query is required for search",
        ));
    }
    if request.action == Action::Read && request.path.as_deref().is_none_or(str::is_empty) {
        return Err(tool_error("invalid_argument", "path is required for read"));
    }
    if request
        .source
        .as_deref()
        .is_some_and(|source| !matches!(source, "all" | "summary" | "memories" | "rollouts"))
    {
        return Err(tool_error(
            "invalid_argument",
            "source must be all, summary, memories, or rollouts",
        ));
    }
    Ok(())
}

fn status(code_home: &Path) -> io::Result<Value> {
    let paths = published_artifact_paths(code_home)?;
    let rollouts = list_rollout_summaries(code_home)?;
    Ok(json!({
        "generation": paths.generation,
        "artifacts": {
            "memory_summary.md": paths.summary_path.is_file(),
            "raw_memories.md": paths.raw_memories_path.is_file(),
            "rollout_summaries": paths.rollout_summaries_dir.is_dir(),
        },
        "rollout_count": rollouts.len(),
    }))
}

fn list_rollouts(code_home: &Path, request: &Request) -> io::Result<Value> {
    let entries = list_rollout_summaries(code_home)?;
    let total = entries.len();
    let items = entries
        .into_iter()
        .skip(request.offset)
        .take(request.limit.unwrap_or(DEFAULT_LIMIT))
        .map(|entry| {
            json!({
                "path": format!("rollout_summaries/{}.md", entry.slug),
                "size_bytes": entry.size_bytes,
                "modified_at": entry.modified_at,
            })
        })
        .collect::<Vec<_>>();
    let next = request.offset.saturating_add(items.len());
    Ok(json!({
        "count": items.len(),
        "total": total,
        "next_offset": (next < total).then_some(next),
        "items": items,
    }))
}

fn read(code_home: &Path, request: &Request) -> io::Result<Value> {
    let path = request.path.as_deref().unwrap_or_default();
    let text = read_logical_path(code_home, path)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("memory artifact not found: {path}"),
        )
    })?;
    let line_start = request.line_start.unwrap_or(1);
    let line_count = request.line_count.unwrap_or(DEFAULT_LINE_COUNT);
    let max_chars = request.max_chars.unwrap_or(DEFAULT_MAX_CHARS);
    let lines = text.lines().collect::<Vec<_>>();
    let start = line_start.saturating_sub(1).min(lines.len());
    let end = start.saturating_add(line_count).min(lines.len());
    let mut content = String::new();
    let mut truncated = false;
    for (index, line) in lines[start..end].iter().enumerate() {
        let rendered = format!("{}: {}\n", start + index + 1, line);
        if content.len().saturating_add(rendered.len()) > max_chars {
            truncated = true;
            break;
        }
        content.push_str(&rendered);
    }
    Ok(json!({
        "path": path,
        "line_start": start.saturating_add(1),
        "line_end": start.saturating_add(content.lines().count()),
        "total_lines": lines.len(),
        "content": content,
        "truncated": truncated || end < lines.len(),
    }))
}

fn search(code_home: &Path, request: &Request) -> io::Result<Value> {
    let query = request.query.as_deref().unwrap_or_default().to_lowercase();
    let source = request.source.as_deref().unwrap_or("all");
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT);
    let max_chars = request.max_chars.unwrap_or(DEFAULT_MAX_CHARS);
    let mut documents = Vec::new();
    if matches!(source, "all" | "summary")
        && let Some(text) = read_memory_summary(code_home)?
    {
        documents.push(("memory_summary.md".to_owned(), text));
    }
    if matches!(source, "all" | "memories")
        && let Some(text) = read_raw_memories(code_home)?
    {
        documents.push(("raw_memories.md".to_owned(), text));
    }
    if matches!(source, "all" | "rollouts") {
        for entry in list_rollout_summaries(code_home)?
            .into_iter()
            .take(MAX_ROLLOUTS_SCANNED)
        {
            if let Some(text) = read_rollout_summary(code_home, &entry.slug)? {
                documents.push((format!("rollout_summaries/{}.md", entry.slug), text));
            }
        }
    }

    let mut scanned_chars = 0usize;
    let mut output_chars = 0usize;
    let mut matches = Vec::new();
    let mut scan_truncated = false;
    'documents: for (path, text) in documents {
        for (index, line) in text.lines().enumerate() {
            scanned_chars = scanned_chars.saturating_add(line.len());
            if scanned_chars > MAX_SCAN_CHARS {
                scan_truncated = true;
                break 'documents;
            }
            if !line.to_lowercase().contains(&query) {
                continue;
            }
            let excerpt = truncate_excerpt(line, 500);
            if output_chars.saturating_add(excerpt.len()) > max_chars || matches.len() >= limit {
                return Ok(json!({
                    "count": matches.len(),
                    "matches": matches,
                    "truncated": true,
                    "scan_truncated": scan_truncated,
                }));
            }
            output_chars = output_chars.saturating_add(excerpt.len());
            matches.push(json!({
                "path": path,
                "line_start": index + 1,
                "line_end": index + 1,
                "excerpt": excerpt,
            }));
        }
    }
    Ok(json!({
        "count": matches.len(),
        "matches": matches,
        "truncated": scan_truncated,
        "scan_truncated": scan_truncated,
    }))
}

fn read_logical_path(code_home: &Path, path: &str) -> io::Result<Option<String>> {
    match path {
        "memory_summary.md" => read_memory_summary(code_home),
        "raw_memories.md" => read_raw_memories(code_home),
        _ => {
            let Some(slug) = path
                .strip_prefix("rollout_summaries/")
                .and_then(|path| path.strip_suffix(".md"))
            else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "path must be memory_summary.md, raw_memories.md, or rollout_summaries/<slug>.md",
                ));
            };
            if slug.is_empty()
                || slug == "."
                || slug == ".."
                || slug.contains('/')
                || slug.contains('\\')
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid rollout summary path",
                ));
            }
            read_rollout_summary(code_home, slug)
        }
    }
}

fn truncate_excerpt(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_owned();
    }
    let mut excerpt = line
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    excerpt.push('…');
    excerpt
}

fn tool_error(code: &str, message: impl Into<String>) -> Value {
    json!({"ok": false, "error": {"code": code, "message": message.into()}})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_generation(code_home: &Path, generation: &str, marker: &str) {
        let root = code_home.join("memories");
        let snapshot = root.join("snapshots").join(generation);
        std::fs::create_dir_all(snapshot.join("rollout_summaries")).expect("snapshot dirs");
        std::fs::write(root.join("current"), format!("{generation}\n")).expect("current");
        std::fs::write(
            snapshot.join("memory_summary.md"),
            format!("summary {marker}\n"),
        )
        .expect("summary");
        std::fs::write(
            snapshot.join("raw_memories.md"),
            format!("memory {marker}\n"),
        )
        .expect("memories");
        std::fs::write(
            snapshot.join("rollout_summaries/demo.md"),
            format!("rollout {marker}\n"),
        )
        .expect("rollout");
    }

    #[test]
    fn tool_resolves_the_current_generation_on_every_call() {
        let code_home = tempfile::tempdir().expect("code home");
        write_generation(code_home.path(), "first", "old");
        let first = execute(code_home.path(), r#"{"action":"search","query":"old"}"#)
            .expect("first search");
        assert_eq!(first["count"], 3);

        write_generation(code_home.path(), "second", "new");
        let second = execute(code_home.path(), r#"{"action":"search","query":"new"}"#)
            .expect("second search");
        assert_eq!(second["count"], 3);
        let stale = execute(code_home.path(), r#"{"action":"search","query":"old"}"#)
            .expect("stale search");
        assert_eq!(stale["count"], 0);
    }

    #[test]
    fn read_rejects_paths_outside_logical_memory_artifacts() {
        let code_home = tempfile::tempdir().expect("code home");
        let error = execute(
            code_home.path(),
            r#"{"action":"read","path":"rollout_summaries/../secret.md"}"#,
        )
        .expect_err("path traversal must fail");
        assert_eq!(error["error"]["code"], "invalid_argument");
    }
}
