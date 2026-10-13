use code_app_server_protocol::Thread;
use code_app_server_protocol::ThreadItem;
use code_app_server_protocol::UserInput;
use code_protocol::ThreadId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;
use unicode_width::UnicodeWidthStr;

use crate::chatwidget::TopSessionTab;

pub(super) fn render_replay_view(
    thread: &Thread,
    tabs: &[TopSessionTab],
    area: Rect,
    buf: &mut Buffer,
) {
    let mut lines = Vec::new();
    lines.push(render_top_tabs_line(tabs));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "Read-only app-server replay",
        crate::colors::style_text().add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(Span::styled(
        "This thread is owned by the app server. Composer input is disabled here; switch back to an attached TUI tab to send messages.",
        crate::colors::style_text_dim(),
    )));
    lines.push(Line::from(""));

    lines.extend(
        replay_text_lines(thread)
            .into_iter()
            .map(|line| Line::from(Span::styled(line, crate::colors::style_text()))),
    );

    Paragraph::new(lines)
        .block(Block::default().style(crate::colors::style_text()))
        .wrap(Wrap { trim: false })
        .render(area, buf);
}

pub(super) fn top_tab_thread_at_x(tabs: &[TopSessionTab], x: u16) -> Option<ThreadId> {
    let mut column = 0usize;
    let x = usize::from(x);
    for tab in effective_tabs(tabs) {
        let text = tab_text(tab);
        let width = text.width();
        if x >= column && x < column.saturating_add(width) {
            return tab.thread_id;
        }
        column = column.saturating_add(width).saturating_add(2);
    }
    None
}

fn render_top_tabs_line(tabs: &[TopSessionTab]) -> Line<'static> {
    let mut spans = Vec::new();
    for tab in effective_tabs(tabs) {
        let style = if tab.is_active {
            crate::colors::style_text()
                .bg(crate::colors::selection())
                .add_modifier(Modifier::BOLD)
        } else {
            crate::colors::style_text_dim()
        };
        spans.push(Span::styled(tab_text(tab), style));
        spans.push(Span::styled("  ", crate::colors::style_text_dim()));
    }
    Line::from(spans)
}

fn effective_tabs(tabs: &[TopSessionTab]) -> Vec<&TopSessionTab> {
    tabs.iter().collect()
}

fn tab_text(tab: &TopSessionTab) -> String {
    if tab.is_running {
        format!("[{} * {}]", tab.ordinal, tab.title)
    } else if tab.is_closed {
        format!("[{} x {}]", tab.ordinal, tab.title)
    } else {
        format!("[{} {}]", tab.ordinal, tab.title)
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

    #[test]
    fn replay_text_lines_include_title_and_messages() {
        let lines = replay_text_lines(&replay_thread()).join("\n");

        assert!(lines.contains("Thread: Replay Demo"));
        assert!(lines.contains("user: What changed?"));
        assert!(lines.contains("agent: Tabs can now replay cached app-server threads."));
    }

    #[test]
    fn top_tab_hit_test_matches_rendered_tab_widths() {
        let first =
            ThreadId::from_string("00000000-0000-0000-0000-000000000001").expect("valid thread id");
        let second =
            ThreadId::from_string("00000000-0000-0000-0000-000000000002").expect("valid thread id");
        let tabs = vec![
            TopSessionTab {
                thread_id: Some(first),
                ordinal: 1,
                title: "Main".to_owned(),
                is_active: false,
                is_running: false,
                is_closed: false,
            },
            TopSessionTab {
                thread_id: Some(second),
                ordinal: 2,
                title: "Replay".to_owned(),
                is_active: true,
                is_running: true,
                is_closed: false,
            },
        ];

        assert_eq!(top_tab_thread_at_x(&tabs, 0), Some(first));
        assert_eq!(top_tab_thread_at_x(&tabs, 8), None);
        assert_eq!(top_tab_thread_at_x(&tabs, 10), Some(second));
    }
}
