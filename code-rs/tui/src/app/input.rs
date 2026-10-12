use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};

use crate::app_event::AppEvent;
use crate::onboarding::onboarding_screen::KeyboardHandler;
use code_app_server_protocol::ServerNotification;
use code_app_server_protocol::ThreadStatus;
use code_core::protocol::{Event, EventMsg};
use code_protocol::ThreadId;

use super::state::{App, AppState};
use super::thread_routing::ThreadTabMetadata;
use super::thread_routing::ThreadNavigationDirection;

impl App<'_> {
    /// Dispatch a `KeyEvent` to the current view and let it decide what to do
    /// with it.
    pub(super) fn dispatch_key_event(&mut self, key_event: KeyEvent) {
        if self.handle_active_thread_navigation_key(key_event) {
            return;
        }

        match &mut self.app_state {
            AppState::Chat { widget } => {
                widget.handle_key_event(key_event);
            }
            AppState::Onboarding { screen } => match key_event.code {
                KeyCode::Char('q') => {
                    self.app_event_tx.send(AppEvent::ExitRequest);
                }
                _ => screen.handle_key_event(key_event),
            },
        }
    }

    fn handle_active_thread_navigation_key(&mut self, key_event: KeyEvent) -> bool {
        if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
            || !key_event.modifiers.contains(KeyModifiers::CONTROL)
        {
            return false;
        }

        let direction = match key_event.code {
            KeyCode::Left => ThreadNavigationDirection::Previous,
            KeyCode::Right => ThreadNavigationDirection::Next,
            _ => return false,
        };

        if self.active_thread_router.switch_adjacent(direction).is_none() {
            return false;
        }
        self.sync_active_thread_tabs_to_widget();
        self.schedule_redraw();
        true
    }

    pub(super) fn dispatch_paste_event(&mut self, pasted: String) {
        match &mut self.app_state {
            AppState::Chat { widget } => widget.handle_paste(pasted),
            AppState::Onboarding { .. } => {}
        }
    }

    pub(super) fn dispatch_mouse_event(&mut self, mouse_event: MouseEvent) {
        match &mut self.app_state {
            AppState::Chat { widget } => {
                widget.handle_mouse_event(mouse_event);
            }
            AppState::Onboarding { .. } => {}
        }
    }

    pub(super) fn dispatch_code_event(&mut self, event: Event) {
        if self.event_deduper.is_duplicate(&event.id, event.event_seq) {
            tracing::warn!(
                event_id = %event.id,
                event_seq = event.event_seq,
                "discarding duplicate code event"
            );
            return;
        }

        if let EventMsg::SessionConfigured(session) = &event.msg {
            self.register_session_configured_with_router(session);
        }

        match &mut self.app_state {
            AppState::Chat { widget } => widget.handle_code_event(event),
            AppState::Onboarding { .. } => {}
        }
    }

    pub(super) fn dispatch_app_server_notification(&mut self, notification: ServerNotification) {
        match notification {
            ServerNotification::ThreadStarted(notification) => {
                self.register_app_server_thread_started_with_router(notification.thread);
            }
            ServerNotification::ThreadStatusChanged(notification) => {
                self.register_app_server_thread_status_with_router(
                    notification.thread_id,
                    notification.status,
                );
            }
            ServerNotification::ThreadNameUpdated(notification) => {
                self.register_app_server_thread_name_with_router(
                    notification.thread_id,
                    notification.thread_name,
                );
            }
            ServerNotification::ThreadClosed(notification) => {
                self.register_app_server_thread_closed_with_router(notification.thread_id);
            }
            _ => {}
        }
    }

    fn register_app_server_thread_started_with_router(&mut self, thread: code_app_server_protocol::Thread) {
        let Ok(thread_id) = ThreadId::from_string(&thread.id) else {
            tracing::warn!(thread_id = %thread.id, "app-server thread id could not be parsed");
            return;
        };
        self.active_thread_router.upsert_thread_metadata(
            thread_id,
            ThreadTabMetadata {
                title: title_from_app_server_thread(&thread),
                agent_nickname: None,
                agent_role: None,
                agent_path: None,
                is_running: false,
                is_closed: false,
            },
        );
        self.sync_active_thread_tabs_to_widget();
        self.schedule_redraw();
    }

    fn register_app_server_thread_status_with_router(
        &mut self,
        thread_id: String,
        status: ThreadStatus,
    ) {
        let Ok(thread_id) = ThreadId::from_string(&thread_id) else {
            tracing::warn!(%thread_id, "app-server thread status id could not be parsed");
            return;
        };
        let is_running = matches!(status, ThreadStatus::Active { .. });
        self.active_thread_router.upsert_thread_metadata(
            thread_id,
            ThreadTabMetadata {
                is_running,
                is_closed: matches!(status, ThreadStatus::NotLoaded),
                ..ThreadTabMetadata::default()
            },
        );
        self.sync_active_thread_tabs_to_widget();
        self.schedule_redraw();
    }

    fn register_app_server_thread_name_with_router(
        &mut self,
        thread_id: String,
        thread_name: Option<String>,
    ) {
        let Ok(thread_id) = ThreadId::from_string(&thread_id) else {
            tracing::warn!(%thread_id, "app-server thread name id could not be parsed");
            return;
        };
        self.active_thread_router.upsert_thread_metadata(
            thread_id,
            ThreadTabMetadata {
                title: thread_name,
                ..ThreadTabMetadata::default()
            },
        );
        self.sync_active_thread_tabs_to_widget();
        self.schedule_redraw();
    }

    fn register_app_server_thread_closed_with_router(&mut self, thread_id: String) {
        let Ok(thread_id) = ThreadId::from_string(&thread_id) else {
            tracing::warn!(%thread_id, "app-server thread closed id could not be parsed");
            return;
        };
        self.active_thread_router.upsert_thread_metadata(
            thread_id,
            ThreadTabMetadata {
                is_closed: true,
                is_running: false,
                ..ThreadTabMetadata::default()
            },
        );
        self.sync_active_thread_tabs_to_widget();
        self.schedule_redraw();
    }

    pub(super) fn normalize_non_enhanced_release_code(code: KeyCode) -> KeyCode {
        match code {
            KeyCode::Char('\r' | '\n') => KeyCode::Enter,
            KeyCode::Char('\t') => KeyCode::Tab,
            KeyCode::Char('\u{1b}') => KeyCode::Esc,
            other => other,
        }
    }
}

fn title_from_app_server_thread(thread: &code_app_server_protocol::Thread) -> Option<String> {
    let preview = thread.preview.trim();
    (!preview.is_empty()).then(|| preview.to_owned())
}
