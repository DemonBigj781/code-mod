use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::bottom_pane::chrome::ChromeMode;
use crate::bottom_pane::settings_ui::line_runs::selection_id_at;
use crate::bottom_pane::settings_ui::selectable_list_mouse::route_scroll_state_mouse_with_hit_test_no_ensure_visible;
use crate::components::scroll_state::ScrollState;
use crate::ui_interaction::{SelectableListMouseConfig, SelectableListMouseResult};

use crate::bottom_pane::ConditionalUpdate;
use crate::components::mode_guard::ModeGuard;

use super::endpoint_form::{
    ENDPOINT_FORM_API_KEY_ROW, ENDPOINT_FORM_BASE_URL_ROW, ENDPOINT_FORM_CANCEL_ROW,
    ENDPOINT_FORM_DISPLAY_NAME_ROW, ENDPOINT_FORM_SAVE_ROW, ENDPOINT_FORM_WIRE_API_ROW,
    EndpointFormAction,
};
use super::{ModelSelectionView, ViewMode};

impl ModelSelectionView {
    pub(super) fn hit_test_in_body(&self, body: Rect, x: u16, y: u16) -> Option<usize> {
        selection_id_at(body, x, y, self.scroll_offset, &self.build_render_runs())
    }

    fn handle_mouse_event_shared(&mut self, mouse_event: MouseEvent, body: Rect) -> ConditionalUpdate {
        let mut state = ScrollState {
            selected_idx: Some(self.selected_index),
            scroll_top: 0,
        };
        let outcome = route_scroll_state_mouse_with_hit_test_no_ensure_visible(
            mouse_event,
            &mut state,
            self.entry_count(),
            |x, y, _scroll_top| self.hit_test_in_body(body, x, y),
            SelectableListMouseConfig {
                hover_select: false,
                scroll_select: false,
                ..SelectableListMouseConfig::default()
            },
        );
        self.selected_index = state.selected_idx.unwrap_or(0);

        if matches!(outcome.result, SelectableListMouseResult::Activated) {
            self.select_item(self.selected_index);
            return ConditionalUpdate::NeedsRedraw;
        }

        match mouse_event.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_up();
                return ConditionalUpdate::NeedsRedraw;
            }
            MouseEventKind::ScrollDown => {
                self.scroll_down();
                return ConditionalUpdate::NeedsRedraw;
            }
            _ => {}
        }

        if outcome.changed {
            ConditionalUpdate::NeedsRedraw
        } else {
            ConditionalUpdate::NoRedraw
        }
    }

    pub(super) fn handle_mouse_event_direct_in_chrome(
        &mut self,
        chrome: ChromeMode,
        mouse_event: MouseEvent,
        area: Rect,
    ) -> ConditionalUpdate {
        if matches!(self.mode, ViewMode::Main) {
            let Some(layout) = self.page().layout_in_chrome(chrome, area) else {
                return ConditionalUpdate::NoRedraw;
            };
            return self.handle_mouse_event_shared(mouse_event, layout.body);
        }
        if matches!(self.mode, ViewMode::AddDirectProvider(_)) {
            return self.handle_direct_provider_mouse_event(chrome, mouse_event, area);
        }

        match &mut self.mode {
            ViewMode::Edit {
                target,
                field,
                error,
            } => {
                let handled = match mouse_event.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let field_area = Self::edit_page(*target, error.as_deref())
                            .layout_in_chrome(chrome, area)
                            .map(|layout| layout.field);
                        if let Some(field_area) = field_area {
                            field.handle_mouse_click(mouse_event.column, mouse_event.row, field_area)
                        } else {
                            false
                        }
                    }
                    MouseEventKind::ScrollDown => field.handle_mouse_scroll(true),
                    MouseEventKind::ScrollUp => field.handle_mouse_scroll(false),
                    _ => false,
                };
                if handled {
                    ConditionalUpdate::NeedsRedraw
                } else {
                    ConditionalUpdate::NoRedraw
                }
            }
            ViewMode::Main | ViewMode::AddDirectProvider(_) | ViewMode::Transition => {
                ConditionalUpdate::NoRedraw
            }
        }
    }

    fn handle_direct_provider_mouse_event(
        &mut self,
        chrome: ChromeMode,
        mouse_event: MouseEvent,
        area: Rect,
    ) -> ConditionalUpdate {
        let app_event_tx = self.app_event_tx.clone();
        let panel_title = self.data.target.panel_title();
        let mut mode_guard = ModeGuard::replace(&mut self.mode, ViewMode::Transition, |mode| {
            matches!(mode, ViewMode::Transition)
        });
        let ViewMode::AddDirectProvider(form) = mode_guard.mode_mut() else {
            return ConditionalUpdate::NoRedraw;
        };
        let page = form.page(panel_title);
        let Some(layout) = page.layout_in_chrome(chrome, area) else {
            return ConditionalUpdate::NoRedraw;
        };
        let buttons = form.button_specs();

        match mouse_event.kind {
            MouseEventKind::Moved => {
                let action = page.standard_action_at_end(
                    &layout,
                    mouse_event.column,
                    mouse_event.row,
                    &buttons,
                );
                if form.set_hovered_button(action) {
                    ConditionalUpdate::NeedsRedraw
                } else {
                    ConditionalUpdate::NoRedraw
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if form.is_submitting() {
                    return ConditionalUpdate::NoRedraw;
                }
                if let Some(action) = page.standard_action_at_end(
                    &layout,
                    mouse_event.column,
                    mouse_event.row,
                    &buttons,
                ) {
                    match action {
                        EndpointFormAction::Save => {
                            form.set_selected_row(ENDPOINT_FORM_SAVE_ROW);
                            let _ = Self::submit_direct_provider_form_state(&app_event_tx, form);
                        }
                        EndpointFormAction::Cancel => {
                            form.set_selected_row(ENDPOINT_FORM_CANCEL_ROW);
                            self.mode = ViewMode::Main;
                        }
                    }
                    return ConditionalUpdate::NeedsRedraw;
                }

                let Some(field_index) = page.field_index_at(
                    &layout,
                    mouse_event.column,
                    mouse_event.row,
                ) else {
                    return ConditionalUpdate::NoRedraw;
                };
                let row = match field_index {
                    0 => ENDPOINT_FORM_DISPLAY_NAME_ROW,
                    1 => ENDPOINT_FORM_BASE_URL_ROW,
                    2 => ENDPOINT_FORM_API_KEY_ROW,
                    3 => ENDPOINT_FORM_WIRE_API_ROW,
                    _ => return ConditionalUpdate::NoRedraw,
                };
                form.set_selected_row(row);
                form.clear_error();
                if row == ENDPOINT_FORM_WIRE_API_ROW {
                    form.toggle_wire_api();
                } else if let Some(field) = form.selected_field_mut()
                    && let Some(section) = layout.sections.get(field_index)
                {
                    let _ = field.handle_mouse_click(
                        mouse_event.column,
                        mouse_event.row,
                        section.inner,
                    );
                }
                ConditionalUpdate::NeedsRedraw
            }
            _ => ConditionalUpdate::NoRedraw,
        }
    }
}
