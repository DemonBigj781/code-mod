use crossterm::event::MouseEvent;
use code_core::config_types::ModelRole;
use ratatui::layout::Rect;

use super::model::{AgentsGridLayout, AgentsOverviewState, AgentsSettingsContent};

impl AgentsSettingsContent {
    pub(super) fn overview_selection_at(
        state: &AgentsOverviewState,
        area: Rect,
        mouse_event: MouseEvent,
    ) -> Option<usize> {
        if area.is_empty() {
            return None;
        }
        if mouse_event.column < area.x
            || mouse_event.column >= area.x.saturating_add(area.width)
            || mouse_event.row < area.y
            || mouse_event.row >= area.y.saturating_add(area.height)
        {
            return None;
        }

        let rel_y = mouse_event.row.saturating_sub(area.y) as usize;
        let content_y = rel_y.saturating_add(state.scroll_offset(area.height as usize));
        let rows_len = state.rows.len();
        let command_len = state.commands.len();

        if content_y >= 2 && content_y < 2 + rows_len {
            return Some(content_y - 2);
        }

        let master_line = rows_len + 3;
        if content_y == master_line {
            return Some(rows_len);
        }

        let add_agent_line = rows_len + 4;
        if content_y == add_agent_line {
            return Some(rows_len + 1);
        }

        let command_start = rows_len + 7;
        if content_y >= command_start && content_y < command_start + command_len {
            return Some(rows_len + 2 + (content_y - command_start));
        }

        let add_command_line = command_start + command_len;
        if content_y == add_command_line {
            return Some(rows_len + 2 + command_len);
        }

        None
    }

    pub(super) fn overview_role_at(
        state: &AgentsOverviewState,
        area: Rect,
        mouse_event: MouseEvent,
    ) -> Option<ModelRole> {
        let relative_column = mouse_event.column.checked_sub(area.x)?;
        AgentsGridLayout::new(&state.rows, area.width).role_at(relative_column)
    }
}
