use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};

use super::loaded_threads::LoadedSubagentThread;
use super::loaded_threads::find_loaded_subagent_threads_for_primary;
use super::state::{App, AppState};
use super::thread_routing::ThreadNavigationDirection;
use super::thread_routing::ThreadTabMetadata;
use super::thread_routing::swap_active_thread_backing;
use crate::app_event::AppEvent;
use crate::onboarding::onboarding_screen::KeyboardHandler;
use code_app_server_protocol::ServerNotification;
use code_app_server_protocol::ThreadStatus;
use code_core::protocol::{Event, EventMsg};
use code_protocol::ThreadId;

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
        let AppState::Chat { widget } = &self.app_state else {
            return false;
        };
        let Some(direction) = thread_navigation_direction(
            key_event,
            widget.composer_is_empty(),
            widget.has_active_modal_view(),
        ) else {
            return false;
        };

        let Some(current_thread_id) = widget
            .session_id()
            .and_then(|id| ThreadId::from_string(&id.to_string()).ok())
        else {
            return false;
        };
        let Some(target_thread_id) = self.active_thread_router.adjacent_thread_id(direction) else {
            return false;
        };

        let AppState::Chat { widget } = &mut self.app_state else {
            return false;
        };
        if !swap_active_thread_backing(
            current_thread_id,
            widget,
            &mut self.inactive_chat_widgets,
            target_thread_id,
        ) {
            return false;
        }
        let activated = self.active_thread_router.set_active_thread(target_thread_id);
        debug_assert!(activated, "adjacent thread should remain registered");
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
            if let Ok(thread_id) = ThreadId::from_string(&session.session_id.to_string()) {
                self.active_thread_router.set_active_thread(thread_id);
                self.sync_active_thread_tabs_to_widget();
            }
        }

        match &mut self.app_state {
            AppState::Chat { widget } => widget.handle_code_event(event),
            AppState::Onboarding { .. } => {}
        }
    }

    pub(super) fn dispatch_thread_code_event(&mut self, thread_id: uuid::Uuid, event: Event) {
        let Ok(thread_id) = ThreadId::from_string(&thread_id.to_string()) else {
            tracing::warn!(%thread_id, "conversation event thread id could not be parsed");
            return;
        };
        let is_inactive = self.inactive_chat_widgets.contains_key(&thread_id);
        let active_thread_id = self.active_thread_router.active_thread_id();
        let is_bootstrapping_active = matches!(&self.app_state, AppState::Chat { widget } if widget.session_id().is_none());
        if let EventMsg::SessionConfigured(session) = &event.msg {
            self.register_session_configured_with_router(session);
            if !is_inactive {
                self.active_thread_router.set_active_thread(thread_id);
                self.sync_active_thread_tabs_to_widget();
            }
        }

        match thread_event_destination(
            active_thread_id,
            thread_id,
            is_inactive,
            is_bootstrapping_active,
        ) {
            ThreadEventDestination::Active => {
                if let AppState::Chat { widget } = &mut self.app_state {
                    widget.handle_code_event(event);
                }
            }
            ThreadEventDestination::Inactive => {
                if let Some(widget) = self.inactive_chat_widgets.get_mut(&thread_id) {
                    widget.handle_code_event(event);
                }
            }
            ThreadEventDestination::Drop => {}
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

    pub(super) fn register_app_server_loaded_threads_with_router(
        &mut self,
        thread_ids: Vec<ThreadId>,
    ) {
        if thread_ids.is_empty() {
            return;
        }

        for thread_id in thread_ids {
            self.active_thread_router.upsert_thread_metadata(
                thread_id,
                ThreadTabMetadata {
                    is_running: true,
                    is_closed: false,
                    ..ThreadTabMetadata::default()
                },
            );
        }
        self.sync_active_thread_tabs_to_widget();
        self.schedule_redraw();
    }

    pub(super) fn register_app_server_loaded_thread_details_with_router(
        &mut self,
        threads: Vec<code_app_server_protocol::Thread>,
    ) {
        if threads.is_empty() {
            return;
        }

        if let Some(primary_thread_id) = self.active_thread_router.active_thread_id() {
            let loaded_subagents =
                find_loaded_subagent_threads_for_primary(threads.clone(), primary_thread_id);
            if !loaded_subagents.is_empty() {
                self.register_loaded_subagent_threads_with_router(loaded_subagents);
                return;
            }
        }

        for thread in threads {
            let Ok(thread_id) = ThreadId::from_string(&thread.id) else {
                tracing::warn!(thread_id = %thread.id, "loaded app-server thread id could not be parsed");
                continue;
            };
            self.active_thread_router.upsert_thread_metadata(
                thread_id,
                ThreadTabMetadata {
                    title: title_from_app_server_thread(&thread),
                    is_running: thread_is_running(&thread),
                    is_closed: thread_is_closed(&thread),
                    ..ThreadTabMetadata::default()
                },
            );
        }
        self.sync_active_thread_tabs_to_widget();
        self.schedule_redraw();
    }

    fn register_loaded_subagent_threads_with_router(&mut self, threads: Vec<LoadedSubagentThread>) {
        for thread in threads {
            self.active_thread_router.upsert_thread_metadata(
                thread.thread_id,
                ThreadTabMetadata {
                    title: thread.title,
                    agent_nickname: thread.agent_nickname,
                    agent_role: thread.agent_role,
                    agent_path: thread.agent_path,
                    is_running: thread.is_running,
                    is_closed: thread.is_closed,
                },
            );
        }
        self.sync_active_thread_tabs_to_widget();
        self.schedule_redraw();
    }

    fn register_app_server_thread_started_with_router(
        &mut self,
        thread: code_app_server_protocol::Thread,
    ) {
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
                is_running: thread_is_running(&thread),
                is_closed: thread_is_closed(&thread),
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

fn thread_navigation_direction(
    key_event: KeyEvent,
    composer_is_empty: bool,
    has_active_modal: bool,
) -> Option<ThreadNavigationDirection> {
    if !composer_is_empty
        || has_active_modal
        || !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        || !key_event.modifiers.contains(KeyModifiers::CONTROL)
    {
        return None;
    }

    match key_event.code {
        KeyCode::Left => Some(ThreadNavigationDirection::Previous),
        KeyCode::Right => Some(ThreadNavigationDirection::Next),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThreadEventDestination {
    Active,
    Inactive,
    Drop,
}

fn thread_event_destination(
    active_thread_id: Option<ThreadId>,
    event_thread_id: ThreadId,
    is_inactive_thread: bool,
    is_bootstrapping_active_thread: bool,
) -> ThreadEventDestination {
    if is_inactive_thread {
        return ThreadEventDestination::Inactive;
    }

    if active_thread_id == Some(event_thread_id) || is_bootstrapping_active_thread {
        return ThreadEventDestination::Active;
    }

    ThreadEventDestination::Drop
}

fn title_from_app_server_thread(thread: &code_app_server_protocol::Thread) -> Option<String> {
    if let Some(name) = thread
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return Some(name.to_owned());
    }

    let preview = thread.preview.trim();
    (!preview.is_empty()).then(|| preview.to_owned())
}

fn thread_is_running(thread: &code_app_server_protocol::Thread) -> bool {
    matches!(
        &thread.status,
        Some(code_app_server_protocol::ThreadStatus::Active { .. })
    )
}

fn thread_is_closed(thread: &code_app_server_protocol::Thread) -> bool {
    matches!(
        &thread.status,
        Some(code_app_server_protocol::ThreadStatus::NotLoaded)
    )
}

#[cfg(test)]
mod app_server_thread_metadata_tests {
    use super::*;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use code_app_server_protocol::SessionSource;
    use code_app_server_protocol::Thread;
    use code_app_server_protocol::ThreadStatus;

    #[test]
    fn thread_events_only_fall_through_to_the_matching_or_bootstrapping_widget() {
        let active = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid active thread id");
        let other = ThreadId::from_string("00000000-0000-0000-0000-000000000002")
            .expect("valid other thread id");

        assert_eq!(
            thread_event_destination(Some(active), other, false, false),
            ThreadEventDestination::Drop
        );
        assert_eq!(
            thread_event_destination(Some(active), active, false, false),
            ThreadEventDestination::Active
        );
        assert_eq!(
            thread_event_destination(None, other, false, true),
            ThreadEventDestination::Active
        );
        assert_eq!(
            thread_event_destination(None, other, false, false),
            ThreadEventDestination::Drop
        );
        assert_eq!(
            thread_event_destination(Some(active), other, true, false),
            ThreadEventDestination::Inactive
        );
    }

    fn test_thread() -> Thread {
        Thread {
            id: "00000000-0000-0000-0000-000000000001".to_string(),
            name: None,
            preview: String::new(),
            status: None,
            model_provider: "openai".to_string(),
            created_at: 0,
            updated_at: 0,
            path: None,
            cwd: std::path::PathBuf::from("/tmp"),
            cli_version: "0.0.0".to_string(),
            source: SessionSource::Cli,
            git_info: None,
            turns: Vec::new(),
        }
    }

    #[test]
    fn thread_title_prefers_name_over_preview() {
        let mut thread = test_thread();
        thread.name = Some("Named thread".to_string());
        thread.preview = "Preview title".to_string();

        assert_eq!(
            title_from_app_server_thread(&thread).as_deref(),
            Some("Named thread")
        );
    }

    #[test]
    fn thread_title_falls_back_to_preview() {
        let mut thread = test_thread();
        thread.preview = "Preview title".to_string();

        assert_eq!(
            title_from_app_server_thread(&thread).as_deref(),
            Some("Preview title")
        );
    }

    #[test]
    fn thread_status_helpers_map_active_and_not_loaded() {
        let mut thread = test_thread();
        thread.status = Some(ThreadStatus::Active {
            active_flags: Vec::new(),
        });
        assert!(thread_is_running(&thread));
        assert!(!thread_is_closed(&thread));

        thread.status = Some(ThreadStatus::NotLoaded);
        assert!(!thread_is_running(&thread));
        assert!(thread_is_closed(&thread));
    }

    #[test]
    fn thread_navigation_requires_an_empty_composer_without_a_modal() {
        let next = KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL);
        let previous = KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL);

        assert_eq!(
            thread_navigation_direction(next, true, false),
            Some(ThreadNavigationDirection::Next)
        );
        assert_eq!(
            thread_navigation_direction(previous, true, false),
            Some(ThreadNavigationDirection::Previous)
        );
        assert_eq!(thread_navigation_direction(next, false, false), None);
        assert_eq!(thread_navigation_direction(next, true, true), None);
    }
}
