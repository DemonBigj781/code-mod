use code_protocol::ThreadId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TopSessionTab {
    pub(crate) thread_id: Option<ThreadId>,
    pub(crate) ordinal: usize,
    pub(crate) title: String,
    pub(crate) is_active: bool,
    pub(crate) is_running: bool,
    pub(crate) is_closed: bool,
}

impl TopSessionTab {
    pub(crate) fn main() -> Self {
        Self {
            thread_id: None,
            ordinal: 1,
            title: "Main".into(),
            is_active: true,
            is_running: false,
            is_closed: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TabAction {
    Select(ThreadId),
    Close(ThreadId),
    Previous,
    Next,
    New,
}

#[derive(Clone, Debug)]
pub(crate) struct TabHit {
    pub(crate) area: Rect,
    pub(crate) action: TabAction,
}

pub(crate) fn hit_test(hits: &[TabHit], x: u16, y: u16) -> Option<TabAction> {
    hits.iter()
        .find(|hit| hit.area.contains((x, y).into()))
        .map(|hit| hit.action)
}

pub(crate) fn split_tab_area(area: Rect, enabled: bool) -> (Rect, Rect) {
    let rows = u16::from(enabled && area.height > 1);
    (
        Rect::new(area.x, area.y, area.width, rows),
        Rect::new(
            area.x,
            area.y + rows,
            area.width,
            area.height.saturating_sub(rows),
        ),
    )
}

fn clipped_title(title: &str, width: usize) -> String {
    let clean: String = title
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    if clean.width() <= width {
        return clean;
    }
    let mut text = String::new();
    for grapheme in clean.graphemes(true) {
        if text.width() + grapheme.width() > width.saturating_sub(1) {
            break;
        }
        text.push_str(grapheme);
    }
    if width > 0 {
        text.push('…');
    }
    text
}

/// Draw one independent row; the returned rectangles are the only clickable cells.
pub(crate) fn render_tabs(tabs: &[TopSessionTab], area: Rect, buf: &mut Buffer) -> Vec<TabHit> {
    let mut hits = Vec::new();
    if area.is_empty() {
        return hits;
    }
    Paragraph::new("")
        .style(crate::colors::style_text_on_bg())
        .render(area, buf);
    let width = usize::from(area.width);
    // Reserve a new-tab button and, on overflow, previous/next buttons.
    let overflow = tabs.len().saturating_mul(12).saturating_add(3) > width;
    let controls = if overflow { 9 } else { 3 };
    let available = width.saturating_sub(controls);
    let count = (available / 12).max(1).min(tabs.len());
    let active = tabs.iter().position(|tab| tab.is_active).unwrap_or(0);
    let start = if overflow {
        active.saturating_sub(count.saturating_sub(1))
    } else {
        0
    };
    let tab_width = if count == 0 {
        0
    } else {
        (available / count).min(28)
    };
    let mut x = area.x;
    for tab in tabs.iter().skip(start).take(count) {
        if tab_width < 7 {
            break;
        }
        let style = if tab.is_active {
            crate::colors::style_text()
                .bg(crate::colors::selection())
                .add_modifier(Modifier::BOLD)
        } else {
            crate::colors::style_text_dim()
        };
        let marker = if tab.is_running {
            "*"
        } else if tab.is_closed {
            "-"
        } else {
            ""
        };
        let prefix = format!(" {}{marker} ", tab.ordinal);
        let body_width = tab_width - 3;
        let title = clipped_title(&tab.title, body_width.saturating_sub(prefix.width()));
        let body = format!("{prefix}{title}");
        let body = format!(
            "{body}{}",
            " ".repeat(body_width.saturating_sub(body.width()))
        );
        let rect = Rect::new(x, area.y, tab_width as u16, 1);
        Paragraph::new(Line::from(vec![
            Span::styled(body, style),
            Span::styled(" × ", style),
        ]))
        .render(rect, buf);
        if let Some(id) = tab.thread_id {
            hits.push(TabHit {
                area: Rect::new(x, area.y, body_width as u16, 1),
                action: TabAction::Select(id),
            });
            hits.push(TabHit {
                area: Rect::new(x + body_width as u16, area.y, 3, 1),
                action: TabAction::Close(id),
            });
        }
        x += tab_width as u16;
    }
    let buttons: &[(TabAction, &str)] = if overflow {
        &[
            (TabAction::Previous, " < "),
            (TabAction::Next, " > "),
            (TabAction::New, " + "),
        ]
    } else {
        &[(TabAction::New, " + ")]
    };
    for &(action, label) in buttons {
        if x.saturating_add(3) > area.right() {
            break;
        }
        let rect = Rect::new(x, area.y, 3, 1);
        Paragraph::new(label)
            .style(crate::colors::style_text_dim())
            .render(rect, buf);
        hits.push(TabHit { area: rect, action });
        x += 3;
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(n: u128, active: bool) -> TopSessionTab {
        TopSessionTab {
            thread_id: Some(ThreadId::from_string(&uuid::Uuid::from_u128(n).to_string()).unwrap()),
            ordinal: n as usize,
            title: "Long 界 session title".into(),
            is_active: active,
            is_running: false,
            is_closed: false,
        }
    }

    #[test]
    fn tabs_have_their_own_row_above_status_and_content() {
        let area = Rect::new(2, 3, 80, 24);
        let (tabs, content) = split_tab_area(area, true);
        assert_eq!(tabs, Rect::new(2, 3, 80, 1));
        assert_eq!(content, Rect::new(2, 4, 80, 23));
        assert_eq!(split_tab_area(area, false).1, area);
        assert_eq!(split_tab_area(Rect::new(0, 0, 1, 1), true).0.height, 0);
    }

    #[test]
    fn title_and_close_are_separate_and_offset_with_the_row() {
        let tabs = [tab(1, true), tab(2, false)];
        let area = Rect::new(4, 2, 60, 1);
        let mut buf = Buffer::empty(area);
        let hits = render_tabs(&tabs, area, &mut buf);
        assert_eq!(
            hit_test(&hits, 4, 2),
            Some(TabAction::Select(tabs[0].thread_id.unwrap()))
        );
        assert_eq!(
            hit_test(&hits, 30, 2),
            Some(TabAction::Close(tabs[0].thread_id.unwrap()))
        );
        assert_eq!(hit_test(&hits, 4, 3), None);
        assert!(buf[(30, 2)].symbol().contains('×'));
    }

    #[test]
    fn overflow_keeps_active_tab_and_close_visible() {
        let tabs: Vec<_> = (1..=8).map(|n| tab(n, n == 7)).collect();
        for width in 0..100 {
            let area = Rect::new(0, 0, width, 1);
            let hits = render_tabs(&tabs, area, &mut Buffer::empty(area));
            assert!(hits.iter().all(|hit| hit.area.right() <= width));
            if width >= 21 {
                assert!(
                    hits.iter()
                        .any(|hit| hit.action == TabAction::Close(tabs[6].thread_id.unwrap()))
                );
            }
        }
    }

    #[test]
    fn truncation_uses_terminal_columns_and_removes_control_characters() {
        assert_eq!(clipped_title("界界界", 5), "界界…");
        assert_eq!(clipped_title("a\nb\t", 10), "a b ");
    }
}
