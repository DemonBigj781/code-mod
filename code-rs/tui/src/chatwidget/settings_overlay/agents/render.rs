use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use super::model::{
    AgentsGridLayout, AgentsOverviewState, AgentsSettingsContent, model_roles,
};

impl AgentsSettingsContent {
    pub(super) fn render_overview(&self, area: Rect, buf: &mut Buffer, state: &AgentsOverviewState) {
        let lines = Self::build_overview_lines(state, Some(area.width as usize));
        Paragraph::new(lines)
            .style(crate::colors::style_text_on_bg())
            .render(area, buf);
    }

    pub(super) fn build_overview_lines(
        state: &AgentsOverviewState,
        available_width: Option<usize>,
    ) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let s_primary = crate::colors::style_primary();
        let s_primary_bold = crate::colors::style_primary_bold();
        let s_text_dim = crate::colors::style_text_dim();
        lines.push(Line::from(Span::styled(
            "Model capabilities",
            Style::default().add_modifier(Modifier::BOLD),
        )));

        let layout = AgentsGridLayout::new(
            &state.rows,
            available_width.unwrap_or(100).min(u16::MAX as usize) as u16,
        );
        let mut header = vec![Span::raw("  ")];
        header.push(Span::styled(
            pad_cell("Model", layout.name_width as usize, false),
            s_text_dim.add_modifier(Modifier::BOLD),
        ));
        header.push(Span::raw("  "));
        for (index, (role, label, width)) in model_roles().enumerate() {
            if index > 0 {
                header.push(Span::raw(" "));
            }
            let style = if role == state.selected_role {
                s_primary_bold.add_modifier(Modifier::UNDERLINED)
            } else {
                s_text_dim.add_modifier(Modifier::BOLD)
            };
            header.push(Span::styled(
                pad_cell(label, width as usize, true),
                style,
            ));
        }
        lines.push(Line::from(header));

        for (idx, row) in state.rows.iter().enumerate() {
            let selected = idx == state.selected;

            let mut spans = Vec::new();
            spans.push(Span::styled(
                crate::icons::selection_prefix(selected),
                if selected {
                    s_primary
                } else {
                    Style::default()
                },
            ));
            let name = crate::text_formatting::truncate_to_display_width(
                &row.name,
                layout.name_width as usize,
            );
            spans.push(Span::styled(
                pad_cell(&name, layout.name_width as usize, false),
                if selected {
                    s_primary_bold
                } else if !row.installed {
                    Style::default().fg(crate::colors::warning())
                } else {
                    Style::default()
                },
            ));
            spans.push(Span::raw("  "));
            for (index, (role, _, width)) in model_roles().enumerate() {
                if index > 0 {
                    spans.push(Span::raw(" "));
                }
                let enabled = row.role_enabled(role);
                let marker = if enabled {
                    crate::icons::checkbox_on()
                } else {
                    crate::icons::checkbox_off()
                };
                let active = selected && role == state.selected_role;
                let mut style = if enabled {
                    Style::default().fg(crate::colors::success())
                } else {
                    s_text_dim
                };
                if active {
                    style = style
                        .fg(crate::colors::primary())
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED);
                }
                spans.push(Span::styled(
                    pad_cell(marker.as_ref(), width as usize, true),
                    style,
                ));
            }

            lines.push(Line::from(spans));
        }

        lines.push(Line::from(""));

        let add_agent_idx = state.rows.len();
        let add_agent_selected = add_agent_idx == state.selected;
        let mut add_spans: Vec<Span<'static>> = Vec::new();
        add_spans.push(Span::styled(
            crate::icons::selection_prefix(add_agent_selected),
            if add_agent_selected {
                s_primary
            } else {
                Style::default()
            },
        ));
        add_spans.push(Span::styled(
            "Add model agent…",
            if add_agent_selected {
                s_primary_bold
            } else {
                Style::default()
            },
        ));
        if add_agent_selected {
            add_spans.push(Span::raw("  "));
            add_spans.push(Span::styled(
                "Enter provider/model slug",
                s_text_dim,
            ));
        }
        lines.push(Line::from(add_spans));

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Commands",
            Style::default().add_modifier(Modifier::BOLD),
        )));

        for (offset, cmd) in state.commands.iter().enumerate() {
            let idx = state.rows.len() + 1 + offset;
            let selected = idx == state.selected;
            let mut spans = Vec::new();
            spans.push(Span::styled(
                crate::icons::selection_prefix(selected),
                if selected {
                    s_primary
                } else {
                    Style::default()
                },
            ));
            spans.push(Span::styled(
                format!("/{cmd}"),
                if selected {
                    s_primary_bold
                } else {
                    Style::default()
                },
            ));
            if selected {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    "Enter to configure",
                    s_text_dim,
                ));
            }
            lines.push(Line::from(spans));
        }

        let add_idx = state.rows.len() + 1 + state.commands.len();
        let add_selected = add_idx == state.selected;
        let mut add_spans = Vec::new();
        add_spans.push(Span::styled(
            crate::icons::selection_prefix(add_selected),
            if add_selected {
                s_primary
            } else {
                Style::default()
            },
        ));
        add_spans.push(Span::styled(
            "Add new…",
            if add_selected {
                s_primary_bold
            } else {
                Style::default()
            },
        ));
        if add_selected {
            add_spans.push(Span::raw("  "));
            add_spans.push(Span::styled(
                "Enter to create",
                s_text_dim,
            ));
        }
        lines.push(Line::from(add_spans));

        lines.push(Line::from(""));
        lines.push(crate::bottom_pane::settings_ui::hints::shortcut_line(&[
            crate::bottom_pane::settings_ui::hints::hint_nav(" navigate"),
            crate::bottom_pane::settings_ui::hints::KeyHint::new("←/→", " role"),
            crate::bottom_pane::settings_ui::hints::KeyHint::new("Space", " access"),
            crate::bottom_pane::settings_ui::hints::hint_enter(" open"),
            crate::bottom_pane::settings_ui::hints::hint_esc(" close"),
        ]));

        lines
    }
}

fn pad_cell(value: &str, width: usize, centered: bool) -> String {
    let value_width = UnicodeWidthStr::width(value);
    if value_width >= width {
        return value.to_owned();
    }
    let remaining = width - value_width;
    if centered {
        let left = remaining / 2;
        let right = remaining - left;
        format!("{}{}{}", " ".repeat(left), value, " ".repeat(right))
    } else {
        format!("{}{}", value, " ".repeat(remaining))
    }
}
