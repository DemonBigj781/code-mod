use std::collections::HashMap;

use code_app_server_protocol::Thread;
use code_app_server_protocol::ThreadItem;
use code_app_server_protocol::ThreadStatus;
use code_app_server_protocol::Turn;
use code_app_server_protocol::TurnStatus;
use code_app_server_protocol::UserInput;
use code_protocol::ThreadId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;


pub(super) fn render_replay_view(
    thread: &Thread,
    composer_text: &str,
    turn_pending: bool,
    notice: Option<&str>,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.is_empty() { return; }
    let header_height = area.height.saturating_sub(1).min(3);
    let header_area = Rect::new(area.x, area.y, area.width, header_height);
    let status = format!("{} · {}", thread.name.as_deref().unwrap_or("App-server session"), thread.cwd.display());
    Paragraph::new(status)
        .block(Block::bordered().title("Session").border_style(crate::colors::style_border()))
        .style(crate::colors::style_text()).render(header_area, buf);

    let notice_height = u16::from(notice.is_some() && area.height > header_height + 1);
    let label = if turn_pending { "app-server> [sending] " } else { "app-server> " };
    let composer = Paragraph::new(format!("{label}{composer_text}"))
        .style(crate::colors::style_text()).wrap(Wrap { trim: false });
    let composer_height = (composer.line_count(area.width) as u16).max(1).min(6)
        .min(area.height.saturating_sub(header_height + notice_height));
    let composer_area = Rect::new(area.x, area.bottom() - composer_height - notice_height, area.width, composer_height);
    composer.render(composer_area, buf);
    let history_area = Rect::new(area.x, header_area.bottom(), area.width, composer_area.y.saturating_sub(header_area.bottom()));
    let history = Paragraph::new(replay_text_lines(thread).into_iter().map(Line::from).collect::<Vec<_>>())
        .style(crate::colors::style_text()).wrap(Wrap { trim: false });
    let scroll = history.line_count(history_area.width).saturating_sub(usize::from(history_area.height)).min(usize::from(u16::MAX)) as u16;
    history.scroll((scroll, 0)).render(history_area, buf);
    if let Some(notice) = notice.filter(|_| notice_height > 0) {
        Paragraph::new(notice).style(crate::colors::style_text_dim())
            .render(Rect::new(area.x, area.bottom() - 1, area.width, 1), buf);
    }
}

pub(super) fn replay_text_lines(thread: &Thread) -> Vec<String> {
    let mut lines = Vec::new();
    let title = thread
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            let preview = thread.preview.trim();
            if preview.is_empty() {
                "Untitled thread"
            } else {
                preview
            }
        });
    lines.push(format!("Thread: {title}"));
    lines.push(format!("ID: {}", thread.id));
    lines.push(format!("CWD: {}", thread.cwd.display()));
    lines.push(format!("Turns: {}", thread.turns.len()));
    lines.push(String::new());

    if thread.turns.is_empty() {
        lines.push("No replayable turns were returned by thread/read yet.".to_owned());
        return lines;
    }

    for (turn_index, turn) in thread.turns.iter().enumerate() {
        lines.push(format!(
            "Turn {} · {:?} · {} items",
            turn_index + 1,
            turn.status,
            turn.items.len()
        ));
        if let Some(error) = &turn.error {
            push_prefixed_text(&mut lines, "  error", &error.message);
            if let Some(details) = &error.additional_details {
                push_prefixed_text(&mut lines, "  details", details);
            }
        }
        for item in &turn.items {
            push_item_lines(&mut lines, item);
        }
        lines.push(String::new());
    }

    lines
}

pub(super) fn update_replay_thread_status(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    status: ThreadStatus,
) -> bool {
    let Some(thread) = replay_thread_mut(replays, thread_id) else {
        return false;
    };
    thread.status = Some(status);
    true
}

pub(super) fn update_replay_thread_name(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    name: Option<String>,
) -> bool {
    let Some(thread) = replay_thread_mut(replays, thread_id) else {
        return false;
    };
    thread.name = name;
    true
}

pub(super) fn mark_replay_thread_closed(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
) -> bool {
    update_replay_thread_status(replays, thread_id, ThreadStatus::NotLoaded)
}

pub(super) fn upsert_replay_turn(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    turn: Turn,
) -> bool {
    let Some(thread) = replay_thread_mut(replays, thread_id) else {
        return false;
    };
    if let Some(existing) = thread.turns.iter_mut().find(|existing| existing.id == turn.id) {
        *existing = turn;
    } else {
        thread.turns.push(turn);
    }
    true
}

pub(super) fn upsert_replay_item(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    turn_id: &str,
    item: ThreadItem,
) -> bool {
    let Some(thread) = replay_thread_mut(replays, thread_id) else {
        return false;
    };
    let item_id = thread_item_id(&item).to_owned();
    let turn = if let Some(turn) = thread.turns.iter_mut().find(|turn| turn.id == turn_id) {
        turn
    } else {
        thread.turns.push(Turn {
            id: turn_id.to_owned(),
            items: Vec::new(),
            status: TurnStatus::InProgress,
            error: None,
        });
        thread.turns.last_mut().expect("just pushed a turn")
    };

    if let Some(existing) = turn
        .items
        .iter_mut()
        .find(|existing| thread_item_id(existing) == item_id)
    {
        *existing = item;
    } else {
        turn.items.push(item);
    }
    true
}

pub(super) fn append_agent_message_delta(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    delta: &str,
) -> bool {
    mutate_replay_item(replays, thread_id, turn_id, item_id, |item| {
        let ThreadItem::AgentMessage { text, .. } = item else {
            return false;
        };
        text.push_str(delta);
        true
    })
}

pub(super) fn append_plan_delta(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    delta: &str,
) -> bool {
    mutate_replay_item(replays, thread_id, turn_id, item_id, |item| {
        let ThreadItem::Plan { text, .. } = item else {
            return false;
        };
        text.push_str(delta);
        true
    })
}

pub(super) fn append_command_output_delta(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    delta: &str,
) -> bool {
    mutate_replay_item(replays, thread_id, turn_id, item_id, |item| {
        let ThreadItem::CommandExecution {
            aggregated_output, ..
        } = item
        else {
            return false;
        };
        aggregated_output.get_or_insert_with(String::new).push_str(delta);
        true
    })
}

pub(super) fn append_file_change_output_delta(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    delta: &str,
) -> bool {
    mutate_replay_item(replays, thread_id, turn_id, item_id, |item| {
        let ThreadItem::FileChange { changes, .. } = item else {
            return false;
        };
        if let Some(change) = changes.last_mut() {
            change.diff.push_str(delta);
            return true;
        }
        false
    })
}

pub(super) fn append_reasoning_text_delta(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    content_index: i64,
    delta: &str,
) -> bool {
    append_indexed_text_delta(
        replays,
        thread_id,
        turn_id,
        item_id,
        content_index,
        delta,
        |item| match item {
            ThreadItem::Reasoning { content, .. } => Some(content),
            _ => None,
        },
    )
}

pub(super) fn append_reasoning_summary_delta(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    summary_index: i64,
    delta: &str,
) -> bool {
    append_indexed_text_delta(
        replays,
        thread_id,
        turn_id,
        item_id,
        summary_index,
        delta,
        |item| match item {
            ThreadItem::Reasoning { summary, .. } => Some(summary),
            _ => None,
        },
    )
}

pub(super) fn add_reasoning_summary_part(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    summary_index: i64,
) -> bool {
    let Ok(summary_index) = usize::try_from(summary_index) else {
        return false;
    };
    mutate_replay_item(replays, thread_id, turn_id, item_id, |item| {
        let ThreadItem::Reasoning { summary, .. } = item else {
            return false;
        };
        ensure_text_slot(summary, summary_index);
        true
    })
}

fn append_indexed_text_delta(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    index: i64,
    delta: &str,
    list_for_item: impl FnOnce(&mut ThreadItem) -> Option<&mut Vec<String>>,
) -> bool {
    let Ok(index) = usize::try_from(index) else {
        return false;
    };
    mutate_replay_item(replays, thread_id, turn_id, item_id, |item| {
        let Some(parts) = list_for_item(item) else {
            return false;
        };
        ensure_text_slot(parts, index);
        parts[index].push_str(delta);
        true
    })
}

fn ensure_text_slot(parts: &mut Vec<String>, index: usize) {
    while parts.len() <= index {
        parts.push(String::new());
    }
}

fn mutate_replay_item(
    replays: &mut HashMap<ThreadId, Thread>,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    mutate: impl FnOnce(&mut ThreadItem) -> bool,
) -> bool {
    let Some(thread) = replay_thread_mut(replays, thread_id) else {
        return false;
    };
    let Some(turn) = thread.turns.iter_mut().find(|turn| turn.id == turn_id) else {
        return false;
    };
    let Some(item) = turn
        .items
        .iter_mut()
        .find(|item| thread_item_id(item) == item_id)
    else {
        return false;
    };
    mutate(item)
}

fn replay_thread_mut<'a>(
    replays: &'a mut HashMap<ThreadId, Thread>,
    thread_id: &str,
) -> Option<&'a mut Thread> {
    let Ok(thread_id) = ThreadId::from_string(thread_id) else {
        tracing::warn!(%thread_id, "app-server replay notification thread id could not be parsed");
        return None;
    };
    replays.get_mut(&thread_id)
}

fn thread_item_id(item: &ThreadItem) -> &str {
    match item {
        ThreadItem::UserMessage { id, .. }
        | ThreadItem::HookPrompt { id, .. }
        | ThreadItem::AgentMessage { id, .. }
        | ThreadItem::Plan { id, .. }
        | ThreadItem::Reasoning { id, .. }
        | ThreadItem::CommandExecution { id, .. }
        | ThreadItem::FileChange { id, .. }
        | ThreadItem::McpToolCall { id, .. }
        | ThreadItem::CollabAgentToolCall { id, .. }
        | ThreadItem::WebSearch { id, .. }
        | ThreadItem::ImageView { id, .. }
        | ThreadItem::ImageGeneration { id, .. }
        | ThreadItem::EnteredReviewMode { id, .. }
        | ThreadItem::ExitedReviewMode { id, .. }
        | ThreadItem::ContextCompaction { id } => id,
    }
}

fn push_item_lines(lines: &mut Vec<String>, item: &ThreadItem) {
    match item {
        ThreadItem::UserMessage { content, .. } => {
            push_prefixed_text(lines, "  user", &format_user_inputs(content));
        }
        ThreadItem::HookPrompt { fragments, .. } => {
            let text = fragments
                .iter()
                .map(|fragment| fragment.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            push_prefixed_text(lines, "  hook", &text);
        }
        ThreadItem::AgentMessage { text, .. } => push_prefixed_text(lines, "  agent", text),
        ThreadItem::Plan { text, .. } => push_prefixed_text(lines, "  plan", text),
        ThreadItem::Reasoning {
            summary, content, ..
        } => {
            let text = if !summary.is_empty() {
                summary.join("\n")
            } else {
                content.join("\n")
            };
            push_prefixed_text(lines, "  reasoning", &text);
        }
        ThreadItem::CommandExecution {
            command,
            cwd,
            status,
            aggregated_output,
            exit_code,
            ..
        } => {
            push_prefixed_text(
                lines,
                "  command",
                &format!(
                    "{command}\n    cwd: {}\n    status: {status:?}\n    exit: {exit_code:?}",
                    cwd.display()
                ),
            );
            if let Some(output) = aggregated_output
                && !output.trim().is_empty()
            {
                push_prefixed_text(lines, "  output", output);
            }
        }
        ThreadItem::FileChange {
            changes, status, ..
        } => {
            let paths = changes
                .iter()
                .map(|change| format!("{} ({:?})", change.path, change.kind))
                .collect::<Vec<_>>()
                .join("\n");
            push_prefixed_text(lines, "  files", &format!("{status:?}\n{paths}"));
        }
        ThreadItem::McpToolCall {
            server,
            tool,
            status,
            error,
            ..
        } => {
            let mut text = format!("{server}.{tool} · {status:?}");
            if let Some(error) = error {
                text.push_str(&format!("\nerror: {}", error.message));
            }
            push_prefixed_text(lines, "  mcp", &text);
        }
        ThreadItem::CollabAgentToolCall {
            tool,
            status,
            prompt,
            ..
        } => {
            let mut text = format!("{tool:?} · {status:?}");
            if let Some(prompt) = prompt {
                text.push_str("\n");
                text.push_str(prompt);
            }
            push_prefixed_text(lines, "  collab", &text);
        }
        ThreadItem::WebSearch { query, action, .. } => push_prefixed_text(
            lines,
            "  web",
            &format!("query: {query}\naction: {action:?}"),
        ),
        ThreadItem::ImageView { path, .. } => push_prefixed_text(lines, "  image", path),
        ThreadItem::ImageGeneration {
            status,
            revised_prompt,
            result,
            saved_path,
            ..
        } => {
            let mut text = format!("status: {status}\nresult: {result}");
            if let Some(prompt) = revised_prompt {
                text.push_str(&format!("\nprompt: {prompt}"));
            }
            if let Some(path) = saved_path {
                text.push_str(&format!("\nsaved: {}", path.display()));
            }
            push_prefixed_text(lines, "  image-gen", &text);
        }
        ThreadItem::EnteredReviewMode { review, .. } => {
            push_prefixed_text(lines, "  review", &format!("entered: {review}"));
        }
        ThreadItem::ExitedReviewMode { review, .. } => {
            push_prefixed_text(lines, "  review", &format!("exited: {review}"));
        }
        ThreadItem::ContextCompaction { .. } => {
            lines.push("  context: compacted".to_owned());
        }
    }
}

fn format_user_inputs(inputs: &[UserInput]) -> String {
    inputs
        .iter()
        .map(|input| match input {
            UserInput::Text { text, .. } => text.clone(),
            UserInput::Image { url } => format!("[image: {url}]"),
            UserInput::LocalImage { path } => format!("[local image: {}]", path.display()),
            UserInput::Skill { name, path } => format!("[skill: {name} at {}]", path.display()),
            UserInput::Mention { name, path } => format!("[mention: {name} at {path}]"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn push_prefixed_text(lines: &mut Vec<String>, prefix: &str, text: &str) {
    let mut segments = text.lines();
    if let Some(first) = segments.next() {
        lines.push(format!("{prefix}: {first}"));
        for segment in segments {
            lines.push(format!("{}  {segment}", " ".repeat(prefix.len())));
        }
    } else {
        lines.push(format!("{prefix}:"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use code_app_server_protocol::SessionSource;
    use code_app_server_protocol::ThreadStatus;
    use code_app_server_protocol::Turn;
    use code_app_server_protocol::TurnStatus;
    use code_app_server_protocol::{
        CommandExecutionStatus, FileUpdateChange, PatchApplyStatus, PatchChangeKind,
    };

    fn replay_thread() -> Thread {
        Thread {
            id: "00000000-0000-0000-0000-000000000001".to_owned(),
            name: Some("Replay Demo".to_owned()),
            preview: "hello".to_owned(),
            status: Some(ThreadStatus::Idle),
            model_provider: "openai".to_owned(),
            created_at: 0,
            updated_at: 0,
            path: None,
            cwd: std::path::PathBuf::from("/tmp"),
            cli_version: "0.0.0".to_owned(),
            source: SessionSource::Cli,
            git_info: None,
            turns: vec![Turn {
                id: "turn-1".to_owned(),
                status: TurnStatus::Completed,
                error: None,
                items: vec![
                    ThreadItem::UserMessage {
                        id: "user-1".to_owned(),
                        content: vec![UserInput::Text {
                            text: "What changed?".to_owned(),
                            text_elements: Vec::new(),
                        }],
                    },
                    ThreadItem::AgentMessage {
                        id: "agent-1".to_owned(),
                        text: "Tabs can now replay cached app-server threads.".to_owned(),
                    },
                ],
            }],
        }
    }

    fn replay_cache(thread: Thread) -> HashMap<ThreadId, Thread> {
        let thread_id = ThreadId::from_string(&thread.id).expect("valid thread id");
        HashMap::from([(thread_id, thread)])
    }

    #[test]
    fn replay_keeps_status_composer_and_command_notice_visible() {
        let area = Rect::new(0, 1, 80, 10);
        let mut buf = Buffer::empty(area);
        render_replay_view(&replay_thread(), "draft", false, Some("Tabs restored"), area, &mut buf);
        let rows: Vec<String> = (area.y..area.bottom()).map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect()).collect();
        assert!(rows[0].contains("Session"));
        assert!(rows[8].contains("app-server> draft"));
        assert!(rows[9].contains("Tabs restored"));
    }

    #[test]
    fn replay_text_lines_include_title_and_messages() {
        let lines = replay_text_lines(&replay_thread()).join("\n");

        assert!(lines.contains("Thread: Replay Demo"));
        assert!(lines.contains("user: What changed?"));
        assert!(lines.contains("agent: Tabs can now replay cached app-server threads."));
    }

    #[test]
    fn replay_cache_updates_thread_metadata() {
        let mut cache = replay_cache(replay_thread());
        let thread_id = "00000000-0000-0000-0000-000000000001";

        assert!(update_replay_thread_name(
            &mut cache,
            thread_id,
            Some("Renamed".to_owned())
        ));
        assert!(update_replay_thread_status(
            &mut cache,
            thread_id,
            ThreadStatus::Active {
                active_flags: Vec::new(),
            }
        ));

        let thread = cache.values().next().expect("cached thread");
        assert_eq!(thread.name.as_deref(), Some("Renamed"));
        assert!(matches!(thread.status, Some(ThreadStatus::Active { .. })));
    }

    #[test]
    fn replay_cache_upserts_turns_by_id() {
        let mut cache = replay_cache(replay_thread());
        let thread_id = "00000000-0000-0000-0000-000000000001";
        let replacement = Turn {
            id: "turn-1".to_owned(),
            status: TurnStatus::Failed,
            error: None,
            items: Vec::new(),
        };
        let appended = Turn {
            id: "turn-2".to_owned(),
            status: TurnStatus::InProgress,
            error: None,
            items: Vec::new(),
        };

        assert!(upsert_replay_turn(&mut cache, thread_id, replacement));
        assert!(upsert_replay_turn(&mut cache, thread_id, appended));

        let thread = cache.values().next().expect("cached thread");
        assert_eq!(thread.turns.len(), 2);
        assert_eq!(thread.turns[0].id, "turn-1");
        assert_eq!(thread.turns[0].status, TurnStatus::Failed);
        assert_eq!(thread.turns[1].id, "turn-2");
    }

    #[test]
    fn replay_cache_upserts_items_by_id_and_creates_missing_turn() {
        let mut cache = replay_cache(replay_thread());
        let thread_id = "00000000-0000-0000-0000-000000000001";

        assert!(upsert_replay_item(
            &mut cache,
            thread_id,
            "turn-1",
            ThreadItem::AgentMessage {
                id: "agent-1".to_owned(),
                text: "replacement".to_owned(),
            },
        ));
        assert!(upsert_replay_item(
            &mut cache,
            thread_id,
            "turn-2",
            ThreadItem::AgentMessage {
                id: "agent-2".to_owned(),
                text: "new turn item".to_owned(),
            },
        ));

        let thread = cache.values().next().expect("cached thread");
        assert_eq!(thread.turns.len(), 2);
        assert_eq!(thread.turns[0].items.len(), 2);
        assert!(matches!(
            &thread.turns[0].items[1],
            ThreadItem::AgentMessage { text, .. } if text == "replacement"
        ));
        assert_eq!(thread.turns[1].id, "turn-2");
        assert_eq!(thread.turns[1].status, TurnStatus::InProgress);
        assert_eq!(thread.turns[1].items.len(), 1);
    }

    #[test]
    fn replay_cache_applies_text_deltas_to_existing_items() {
        let mut thread = replay_thread();
        thread.turns[0].items.extend([
            ThreadItem::Plan {
                id: "plan-1".to_owned(),
                text: "step".to_owned(),
            },
            ThreadItem::Reasoning {
                id: "reasoning-1".to_owned(),
                summary: Vec::new(),
                content: Vec::new(),
            },
        ]);
        let mut cache = replay_cache(thread);
        let thread_id = "00000000-0000-0000-0000-000000000001";

        assert!(append_agent_message_delta(
            &mut cache,
            thread_id,
            "turn-1",
            "agent-1",
            " more"
        ));
        assert!(append_plan_delta(
            &mut cache,
            thread_id,
            "turn-1",
            "plan-1",
            " done"
        ));
        assert!(add_reasoning_summary_part(
            &mut cache,
            thread_id,
            "turn-1",
            "reasoning-1",
            1
        ));
        assert!(append_reasoning_summary_delta(
            &mut cache,
            thread_id,
            "turn-1",
            "reasoning-1",
            1,
            "summary"
        ));
        assert!(append_reasoning_text_delta(
            &mut cache,
            thread_id,
            "turn-1",
            "reasoning-1",
            0,
            "raw"
        ));

        let thread = cache.values().next().expect("cached thread");
        assert!(matches!(
            &thread.turns[0].items[1],
            ThreadItem::AgentMessage { text, .. } if text.ends_with(" more")
        ));
        assert!(matches!(
            &thread.turns[0].items[2],
            ThreadItem::Plan { text, .. } if text == "step done"
        ));
        assert!(matches!(
            &thread.turns[0].items[3],
            ThreadItem::Reasoning { summary, content, .. }
                if summary == &vec![String::new(), "summary".to_owned()]
                    && content == &vec!["raw".to_owned()]
        ));
    }

    #[test]
    fn replay_cache_applies_output_deltas_to_existing_items() {
        let mut thread = replay_thread();
        thread.turns[0].items.extend([
            ThreadItem::CommandExecution {
                id: "cmd-1".to_owned(),
                command: "echo hi".to_owned(),
                cwd: std::path::PathBuf::from("/tmp"),
                process_id: None,
                status: CommandExecutionStatus::InProgress,
                command_actions: Vec::new(),
                aggregated_output: None,
                exit_code: None,
                duration_ms: None,
            },
            ThreadItem::FileChange {
                id: "file-1".to_owned(),
                changes: vec![FileUpdateChange {
                    path: "src/main.rs".to_owned(),
                    kind: PatchChangeKind::Update { move_path: None },
                    diff: "@@".to_owned(),
                }],
                status: PatchApplyStatus::InProgress,
            },
        ]);
        let mut cache = replay_cache(thread);
        let thread_id = "00000000-0000-0000-0000-000000000001";

        assert!(append_command_output_delta(
            &mut cache,
            thread_id,
            "turn-1",
            "cmd-1",
            "hello"
        ));
        assert!(append_file_change_output_delta(
            &mut cache,
            thread_id,
            "turn-1",
            "file-1",
            " +line"
        ));

        let thread = cache.values().next().expect("cached thread");
        assert!(matches!(
            &thread.turns[0].items[2],
            ThreadItem::CommandExecution { aggregated_output: Some(output), .. } if output == "hello"
        ));
        assert!(matches!(
            &thread.turns[0].items[3],
            ThreadItem::FileChange { changes, .. } if changes[0].diff == "@@ +line"
        ));
    }

    #[test]
    fn replay_cache_ignores_delta_for_missing_or_mismatched_item() {
        let mut cache = replay_cache(replay_thread());
        let thread_id = "00000000-0000-0000-0000-000000000001";

        assert!(!append_agent_message_delta(
            &mut cache,
            thread_id,
            "turn-1",
            "missing",
            "ignored"
        ));
        assert!(!append_plan_delta(
            &mut cache,
            thread_id,
            "turn-1",
            "agent-1",
            "ignored"
        ));
        assert!(!append_reasoning_text_delta(
            &mut cache,
            thread_id,
            "turn-1",
            "agent-1",
            -1,
            "ignored"
        ));
    }
}
