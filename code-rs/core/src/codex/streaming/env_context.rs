use super::*;

const HISTORY_TRACE_MAX_ITEMS: usize = 32;
const HISTORY_TRACE_ITEM_MAX_BYTES: usize = 192;
const HISTORY_TRACE_MAX_BYTES: usize = 8 * 1024;

fn history_trace_enabled(trace_env: Option<&std::ffi::OsStr>) -> bool {
    trace_env.is_some()
}

fn truncate_trace_value(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn message_trace_text(content: &[ContentItem]) -> String {
    let mut text = String::new();
    for item in content {
        let value = match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => text,
            ContentItem::InputImage { .. } => continue,
        };
        if !text.is_empty() {
            text.push(' ');
        }
        let remaining = 96usize.saturating_sub(text.len());
        let mut end = remaining.min(value.len());
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        text.push_str(&value[..end]);
        if text.len() >= 96 || end < value.len() {
            break;
        }
    }
    truncate_trace_value(text.as_str(), 96)
}

fn render_history_trace_item(index: usize, item: &ResponseItem) -> String {
    let rendered = match item {
        ResponseItem::Message { role, content, .. } => format!(
            "{index}:message:{}:{}",
            truncate_trace_value(role, 24),
            message_trace_text(content),
        ),
        ResponseItem::Reasoning { .. } => format!("{index}:reasoning"),
        ResponseItem::LocalShellCall { call_id, .. } => format!(
            "{index}:local_shell_call:{}",
            truncate_trace_value(call_id.as_deref().unwrap_or("none"), 64),
        ),
        ResponseItem::FunctionCall {
            name,
            call_id,
            arguments,
            ..
        } => format!(
            "{index}:function_call:{}:{}:args={}B",
            truncate_trace_value(name, 48),
            truncate_trace_value(call_id, 64),
            arguments.len(),
        ),
        ResponseItem::ToolSearchCall { call_id, .. } => format!(
            "{index}:tool_search_call:{}",
            truncate_trace_value(call_id.as_deref().unwrap_or("none"), 64),
        ),
        ResponseItem::FunctionCallOutput { call_id, .. } => format!(
            "{index}:function_call_output:{}",
            truncate_trace_value(call_id, 64),
        ),
        ResponseItem::CustomToolCall {
            call_id, name, input, ..
        } => format!(
            "{index}:custom_tool_call:{}:{}:input={}B",
            truncate_trace_value(name, 48),
            truncate_trace_value(call_id, 64),
            input.len(),
        ),
        ResponseItem::CustomToolCallOutput { call_id, name, .. } => format!(
            "{index}:custom_tool_call_output:{}:{}",
            truncate_trace_value(name.as_deref().unwrap_or("unnamed"), 48),
            truncate_trace_value(call_id, 64),
        ),
        ResponseItem::ToolSearchOutput { call_id, .. } => format!(
            "{index}:tool_search_output:{}",
            truncate_trace_value(call_id.as_deref().unwrap_or("none"), 64),
        ),
        ResponseItem::WebSearchCall { id, .. } => format!(
            "{index}:web_search_call:{}",
            truncate_trace_value(id.as_deref().unwrap_or("none"), 64),
        ),
        ResponseItem::ImageGenerationCall { id, result, .. } => format!(
            "{index}:image_generation_call:{}:result={}B",
            truncate_trace_value(id, 64),
            result.len(),
        ),
        ResponseItem::GhostSnapshot { .. } => format!("{index}:ghost_snapshot"),
        ResponseItem::CompactionSummary { encrypted_content } => {
            format!("{index}:compaction_summary:{}B", encrypted_content.len())
        }
        ResponseItem::Other => format!("{index}:other"),
    };
    truncate_trace_value(rendered.as_str(), HISTORY_TRACE_ITEM_MAX_BYTES)
}

fn render_history_trace_preview(items: &[ResponseItem]) -> String {
    let content_budget = HISTORY_TRACE_MAX_BYTES.saturating_sub(96);
    let mut parts = Vec::new();
    let mut used = 0usize;
    for (index, item) in items.iter().enumerate().rev().take(HISTORY_TRACE_MAX_ITEMS) {
        let part = render_history_trace_item(index, item);
        let separator = usize::from(!parts.is_empty()) * 3;
        if used.saturating_add(separator).saturating_add(part.len()) > content_budget {
            break;
        }
        used = used.saturating_add(separator).saturating_add(part.len());
        parts.push(part);
    }
    parts.reverse();

    let omitted = items.len().saturating_sub(parts.len());
    let mut rendered = String::new();
    if omitted > 0 {
        rendered.push_str(format!("… {omitted} older items omitted … | ").as_str());
    }
    rendered.push_str(parts.join(" | ").as_str());
    truncate_trace_value(rendered.as_str(), HISTORY_TRACE_MAX_BYTES)
}

pub(in crate::codex) fn debug_history(label: &str, items: &[ResponseItem]) {
    let trace_env = std::env::var_os("CODEX_COMPACT_TRACE");
    if !history_trace_enabled(trace_env.as_deref()) {
        return;
    }
    let rendered = render_history_trace_preview(items);
    tracing::debug!(target = "code_core::compact_history", "{label} => [{rendered}]");
}

#[derive(Debug)]
pub(in crate::codex) struct TimelineReplayContext {
    pub(in crate::codex) timeline: ContextTimeline,
    pub(in crate::codex) next_sequence: u64,
    pub(in crate::codex) last_snapshot: Option<EnvironmentContextSnapshot>,
    pub(in crate::codex) legacy_baseline: Option<EnvironmentContextSnapshot>,
}

impl Default for TimelineReplayContext {
    fn default() -> Self {
        Self {
            timeline: ContextTimeline::new(),
            next_sequence: 1,
            last_snapshot: None,
            legacy_baseline: None,
        }
    }
}

pub(in crate::codex) fn process_rollout_env_item(ctx: &mut TimelineReplayContext, item: &ResponseItem) {
    if let Some(snapshot) = parse_env_snapshot_from_response(item) {
        if ctx.timeline.baseline().is_none()
            && let Err(err) = ctx.timeline.add_baseline_once(snapshot.clone())
        {
            tracing::warn!("env_ctx_v2: failed to seed baseline during replay: {err}");
        }

        match ctx.timeline.record_snapshot(snapshot.clone()) {
            Ok(true) => crate::telemetry::global_telemetry().record_snapshot_commit(),
            Ok(false) => crate::telemetry::global_telemetry().record_dedup_drop(),
            Err(err) => tracing::warn!("env_ctx_v2: failed to record snapshot during replay: {err}"),
        }

        ctx.last_snapshot = Some(snapshot);
        return;
    }

    if let Some(delta) = parse_env_delta_from_response(item) {
        if let Some(base_snapshot) = ctx.last_snapshot.clone() {
            if delta.base_fingerprint != base_snapshot.fingerprint() {
                tracing::warn!(
                    "env_ctx_v2: delta base fingerprint mismatch during replay; requesting baseline resend"
                );
                crate::telemetry::global_telemetry().record_baseline_resend();
                crate::telemetry::global_telemetry().record_delta_gap();
                ctx.timeline = ContextTimeline::new();
                ctx.last_snapshot = None;
                ctx.legacy_baseline = None;
                ctx.next_sequence = 1;
                return;
            }

            let sequence = ctx.next_sequence;
            match ctx.timeline.apply_delta(sequence, delta.clone()) {
                Ok(_) => {
                    ctx.next_sequence = ctx.next_sequence.saturating_add(1);
                }
                Err(err) => {
                    tracing::warn!("env_ctx_v2: failed to apply delta during replay: {err}");
                    crate::telemetry::global_telemetry().record_delta_gap();
                    return;
                }
            }

            let next_snapshot = base_snapshot.apply_delta(&delta);
            match ctx.timeline.record_snapshot(next_snapshot.clone()) {
                Ok(true) => crate::telemetry::global_telemetry().record_snapshot_commit(),
                Ok(false) => crate::telemetry::global_telemetry().record_dedup_drop(),
                Err(err) => tracing::warn!("env_ctx_v2: failed to record snapshot during replay: {err}"),
            }

            ctx.last_snapshot = Some(next_snapshot);
        } else {
            tracing::warn!("env_ctx_v2: encountered delta before baseline while replaying rollout");
            crate::telemetry::global_telemetry().record_delta_gap();
        }
        return;
    }

    if ctx.legacy_baseline.is_none()
        && is_legacy_system_status(item)
        && let Some(snapshot) = parse_legacy_status_snapshot(item)
    {
        ctx.legacy_baseline = Some(snapshot);
    }
}

fn extract_tagged_json<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text.rfind(close)?;
    if end <= start {
        return None;
    }
    Some(text[start..end].trim())
}

pub(in crate::codex) fn parse_env_snapshot_from_response(
    item: &ResponseItem,
) -> Option<EnvironmentContextSnapshot> {
    if let ResponseItem::Message { role, content, .. } = item {
        if role != "user" {
            return None;
        }
        for piece in content {
            if let ContentItem::InputText { text } = piece
                && let Some(json) = extract_tagged_json(
                    text,
                    ENVIRONMENT_CONTEXT_OPEN_TAG,
                    ENVIRONMENT_CONTEXT_CLOSE_TAG,
                )
                && let Ok(snapshot) = serde_json::from_str::<EnvironmentContextSnapshot>(json)
            {
                return Some(snapshot);
            }
        }
    }
    None
}

pub(in crate::codex) fn parse_env_delta_from_response(
    item: &ResponseItem,
) -> Option<EnvironmentContextDelta> {
    if let ResponseItem::Message { role, content, .. } = item {
        if role != "user" {
            return None;
        }
        for piece in content {
            if let ContentItem::InputText { text } = piece
                && let Some(json) = extract_tagged_json(
                    text,
                    ENVIRONMENT_CONTEXT_DELTA_OPEN_TAG,
                    ENVIRONMENT_CONTEXT_DELTA_CLOSE_TAG,
                )
                && let Ok(delta) = serde_json::from_str::<EnvironmentContextDelta>(json)
            {
                return Some(delta);
            }
        }
    }
    None
}

fn is_legacy_system_status(item: &ResponseItem) -> bool {
    if let ResponseItem::Message { role, content, .. } = item {
        if role != "user" {
            return false;
        }
        return content.iter().any(|c| {
            if let ContentItem::InputText { text } = c {
                text.contains("== System Status ==")
            } else {
                false
            }
        });
    }
    false
}

fn parse_legacy_status_snapshot(item: &ResponseItem) -> Option<EnvironmentContextSnapshot> {
    if let ResponseItem::Message { role, content, .. } = item {
        if role != "user" {
            return None;
        }
        for piece in content {
            if let ContentItem::InputText { text } = piece {
                if !text.contains("== System Status ==") {
                    continue;
                }

                let mut cwd: Option<String> = None;
                let mut branch: Option<String> = None;
                for line in text.lines() {
                    let trimmed = line.trim();
                    if let Some(rest) = trimmed.strip_prefix("cwd:") {
                        let value = rest.trim();
                        if !value.is_empty() {
                            cwd = Some(value.to_owned());
                        }
                    } else if let Some(rest) = trimmed.strip_prefix("branch:") {
                        let value = rest.trim();
                        if !value.is_empty() && value != "unknown" {
                            branch = Some(value.to_owned());
                        }
                    }
                }

                return Some(EnvironmentContextSnapshot {
                    version: EnvironmentContextSnapshot::VERSION,
                    cwd,
                    git_project_root: None,
                    approval_policy: None,
                    sandbox_mode: None,
                    network_access: None,
                    writable_roots: Vec::new(),
                    operating_system: None,
                    common_tools: Vec::new(),
                    shell: None,
                    git_branch: branch,
                    reasoning_effort: None,
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod history_trace_tests {
    use super::*;
    use code_protocol::models::FunctionCallOutputBody;
    use code_protocol::models::FunctionCallOutputPayload;
    use std::ffi::OsStr;

    #[test]
    fn history_trace_requires_an_explicit_opt_in() {
        assert!(!history_trace_enabled(None));
        assert!(history_trace_enabled(Some(OsStr::new("1"))));
    }

    #[test]
    fn history_trace_preview_is_bounded_and_keeps_the_recent_tail() {
        let mut items = (0..80)
            .map(|index| ResponseItem::FunctionCallOutput {
                call_id: format!("call-{index}"),
                output: FunctionCallOutputPayload {
                    body: FunctionCallOutputBody::Text("x".repeat(16 * 1024)),
                    success: Some(true),
                },
            })
            .collect::<Vec<_>>();
        items.push(ResponseItem::Message {
            id: Some("latest".to_owned()),
            role: "user".to_owned(),
            content: vec![ContentItem::InputText {
                text: format!("LATEST_HISTORY_SENTINEL{}", "界".repeat(16 * 1024)),
            }],
            end_turn: None,
            phase: None,
        });

        let preview = render_history_trace_preview(&items);

        assert!(preview.len() <= HISTORY_TRACE_MAX_BYTES);
        assert!(preview.contains("LATEST_HISTORY_SENTINEL"));
        assert!(preview.contains("older items omitted"));
        assert!(!preview.contains(&"x".repeat(1024)));
    }
}
