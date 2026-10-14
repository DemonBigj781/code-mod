use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use code_protocol::ThreadId;
use uuid::Uuid;

use super::state::{App, AppState};
use super::thread_routing::ThreadNavigationDirection;
use crate::app_event::AppEvent;
use crate::chatwidget::{ChatWidget, ChatWidgetInit};
use crate::session_tabs::TabAction;
use crate::tab_commands::{TabCommand, parse_tab_command};
use crate::tab_persistence::{ComposerDraft, TabLayout};

pub(super) struct TabRestore {
    layout: TabLayout,
    resolving: bool,
    queue: VecDeque<(Uuid, PathBuf, PathBuf)>,
    waiting: Option<(ThreadId, PathBuf)>,
    errors: Vec<String>,
}

impl App<'_> {
    pub(super) fn tab_notice(&mut self, message: impl Into<String>) {
        let message_for_replay = message.into();
        if let AppState::Chat { widget } = &mut self.app_state {
            widget.flash_footer_notice(message_for_replay.clone());
        }
        self.tab_notice_message = Some((message_for_replay, std::time::Instant::now()));
        self.schedule_redraw_in(std::time::Duration::from_secs(10));
        self.schedule_redraw();
    }

    pub(super) fn handle_tab_action(&mut self, action: TabAction) {
        if self.active_chat_thread_id().is_none() {
            self.tab_notice("Session is still loading; try again shortly.");
            return;
        }
        match action {
            TabAction::Select(id) => {
                self.switch_active_thread_backing(id);
            }
            TabAction::Close(id) => self.close_session_tab(id),
            TabAction::New => {
                self.new_session_tab();
            }
            TabAction::Previous | TabAction::Next => {
                let direction = if action == TabAction::Next {
                    ThreadNavigationDirection::Next
                } else {
                    ThreadNavigationDirection::Previous
                };
                if let Some(id) = self.active_thread_router.adjacent_thread_id(direction) {
                    self.switch_active_thread_backing(id);
                }
            }
        }
        self.persist_tab_state();
    }

    pub(super) fn handle_tab_command(&mut self, args: &str) {
        let command = match parse_tab_command(args) {
            Ok(command) => command,
            Err(usage) => {
                self.tab_notice(usage);
                return;
            }
        };
        match command {
            TabCommand::Next => self.handle_tab_action(TabAction::Next),
            TabCommand::Previous => self.handle_tab_action(TabAction::Previous),
            TabCommand::New => self.handle_tab_action(TabAction::New),
            TabCommand::Restore => self.restore_previous_tabs(),
            TabCommand::Select(n) | TabCommand::Close(Some(n)) => {
                if let Some(id) = self
                    .active_thread_router
                    .ordered_tabs()
                    .get(n - 1)
                    .map(|tab| tab.thread_id)
                {
                    self.handle_tab_action(if matches!(command, TabCommand::Select(_)) {
                        TabAction::Select(id)
                    } else {
                        TabAction::Close(id)
                    });
                } else {
                    self.tab_notice(format!("No tab {n}. Use /tabs to list open tabs."));
                }
            }
            TabCommand::Close(None) => {
                if let Some(id) = self.active_thread_router.active_thread_id() {
                    self.handle_tab_action(TabAction::Close(id));
                }
            }
            TabCommand::List => {
                let tabs = self
                    .active_thread_router
                    .ordered_tabs()
                    .into_iter()
                    .map(|tab| {
                        format!(
                            "{}{}: {}",
                            tab.ordinal,
                            if tab.is_active { "*" } else { "" },
                            tab.title
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                self.tab_notice(format!(
                    "{tabs} — /tab next|prev|N|close [N]|new; /tabs restore"
                ));
            }
        }
    }

    pub(super) fn new_session_tab(&mut self) -> bool {
        if self.active_chat_thread_id().is_none() {
            self.tab_notice("Wait for the current session to finish loading.");
            return false;
        }
        self.persist_tab_state();
        let mut config = self.config.clone();
        if let Some(thread) = self.active_app_server_replay_thread() {
            config.cwd = thread.cwd.clone();
        } else if let AppState::Chat { widget } = &self.app_state {
            config.cwd = widget.config_ref().cwd.clone();
        }
        config.experimental_resume = None;
        let mut widget = ChatWidget::new(ChatWidgetInit {
            config,
            conversation_manager: self._server.clone(),
            app_event_tx: self.app_event_tx.clone(),
            initial_prompt: None,
            initial_images: Vec::new(),
            terminal_info: self.terminal_info.clone(),
            show_order_overlay: self.show_order_overlay,
            latest_upgrade_version: self.latest_upgrade_version.clone(),
        });
        widget.enable_perf(self.timing_enabled);
        self.replace_active_chat_widget(widget);
        self.pending_tab_activation = None;
        self.tab_layout_touched = true;
        self.tab_hits.clear();
        self.schedule_redraw();
        true
    }

    fn close_session_tab(&mut self, id: ThreadId) {
        if !self.active_thread_router.is_visible(id) {
            return;
        }
        self.persist_tab_state();
        if self.active_thread_router.active_thread_id() == Some(id) {
            if let Some(next) = self
                .active_thread_router
                .adjacent_thread_id(ThreadNavigationDirection::Next)
            {
                if !self.switch_active_thread_backing(next) {
                    return;
                }
            } else if !self.new_session_tab() {
                return;
            }
        }
        // Closing a view must not kill running tools or delete its saved conversation/draft.
        self.active_thread_router.set_thread_visible(id, false);
        if self.pending_tab_activation == Some(id) {
            self.pending_tab_activation = None;
        }
        self.tab_layout_touched = true;
        self.tab_hits.clear();
        self.tab_notice("Tab closed. Session and draft kept; running work is not cancelled.");
    }

    pub(super) fn persist_tab_state(&mut self) {
        let mut error = self.tab_persistence.layout_load_error.clone();
        let mut has_text = false;
        let store = &mut self.tab_persistence;
        let active = match &mut self.app_state {
            AppState::Chat { widget } => Some(widget),
            _ => None,
        };
        for widget in active
            .into_iter()
            .chain(self.inactive_chat_widgets.values_mut())
        {
            let Some(id) = widget.session_id() else {
                continue;
            };
            if !widget.composer_draft_initialized() {
                match store.load_draft(id) {
                    Ok(draft) if widget.composer_is_empty() => {
                        widget.restore_composer_draft(&draft)
                    }
                    Ok(_) => widget.mark_composer_draft_initialized(),
                    Err(err) => {
                        error = Some(err.to_string());
                        continue;
                    }
                }
            }
            let draft = widget.composer_draft();
            has_text |= !draft.text.is_empty();
            if let Err(err) = store.save_draft(id, draft) {
                error = Some(err.to_string());
            }
        }
        for id in self.app_server_thread_replays.keys() {
            // Local widgets are authoritative when both representations are cached.
            if self.inactive_chat_widgets.contains_key(id) {
                continue;
            }
            if let AppState::Chat { widget } = &self.app_state
                && widget.session_id() == Some(Uuid::from(*id))
            {
                continue;
            }
            let uuid = Uuid::from(*id);
            if !store.has_loaded_draft(uuid) {
                match store.load_draft(uuid) {
                    Ok(draft) => {
                        let text = self.app_server_replay_composers.entry(*id).or_default();
                        if text.is_empty() {
                            *text = draft.text;
                        }
                    }
                    Err(err) => {
                        error = Some(err.to_string());
                        continue;
                    }
                }
            }
            let text = self
                .app_server_replay_composers
                .get(id)
                .cloned()
                .unwrap_or_default();
            has_text |= !text.is_empty();
            let draft = ComposerDraft {
                cursor: text.len(),
                text,
                ..Default::default()
            };
            if let Err(err) = store.save_draft(uuid, draft) {
                error = Some(err.to_string());
            }
        }
        self.tab_layout_touched |= has_text;
        if self.tab_layout_touched
            && self.tab_restore.is_none()
            && self.active_chat_thread_id().is_some()
        {
            let layout = TabLayout {
                sessions: self
                    .active_thread_router
                    .ordered_tabs()
                    .iter()
                    .map(|tab| Uuid::from(tab.thread_id))
                    .collect(),
                active: self.active_thread_router.active_thread_id().map(Uuid::from),
            };
            if let Err(err) = self.tab_persistence.save_layout(layout) {
                error = Some(err.to_string());
            }
        }
        if error != self.tab_save_error {
            if let Some(message) = &error {
                self.tab_notice(format!("Could not save/restore tab drafts: {message}"));
            }
            self.tab_save_error = error;
        }
    }

    fn restore_previous_tabs(&mut self) {
        if self.active_chat_thread_id().is_none() {
            self.tab_notice("Wait for the current session to finish loading.");
            return;
        }
        if self.tab_restore.is_some() {
            self.tab_notice("Tab restoration is already in progress.");
            return;
        }
        let Some(layout) = self
            .tab_persistence
            .previous
            .clone()
            .filter(|layout| !layout.sessions.is_empty())
        else {
            self.tab_notice("No previous tab layout saved for this folder.");
            return;
        };
        let mut loaded: HashSet<Uuid> = self
            .inactive_chat_widgets
            .keys()
            .chain(self.app_server_thread_replays.keys())
            .copied()
            .map(Uuid::from)
            .collect();
        if let Some(id) = self.active_chat_thread_id() {
            loaded.insert(Uuid::from(id));
        }
        for id in &layout.sessions {
            if loaded.contains(id) {
                let id = ThreadId::from_string(&id.to_string()).expect("UUID");
                self.active_thread_router.set_thread_visible(id, true);
            }
        }
        let ids = layout
            .sessions
            .iter()
            .copied()
            .filter(|id| !loaded.contains(id))
            .collect::<Vec<_>>();
        self.tab_restore = Some(TabRestore {
            layout,
            resolving: true,
            queue: VecDeque::new(),
            waiting: None,
            errors: Vec::new(),
        });
        let code_home = self.config.code_home.clone();
        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let catalog = code_core::SessionCatalog::new(code_home);
            let mut sessions = Vec::new();
            let mut errors = Vec::new();
            for id in ids {
                match catalog.find_by_id(&id.to_string()).await {
                    Ok(Some(entry)) => {
                        let path = catalog.entry_rollout_path(&entry);
                        if path.is_file() && entry.cwd_real.is_dir() {
                            sessions.push((id, path, entry.cwd_real));
                        } else {
                            errors.push(format!("{id}: session file or working directory missing"));
                        }
                    }
                    Ok(None) => errors.push(format!("{id}: saved session not found")),
                    Err(err) => errors.push(format!("{id}: {err}")),
                }
            }
            tx.send(AppEvent::TabRestoreResolved { sessions, errors });
        });
        self.tab_notice("Loading previous tabs and their sessions…");
    }

    pub(super) fn resolve_tab_restore(
        &mut self,
        sessions: Vec<(Uuid, PathBuf, PathBuf)>,
        errors: Vec<String>,
    ) {
        if let Some(restore) = &mut self.tab_restore {
            restore.resolving = false;
            restore.queue = sessions.into();
            restore.errors.extend(errors);
        }
        self.continue_tab_restore();
    }

    pub(super) fn continue_tab_restore(&mut self) {
        let Some(restore) = &mut self.tab_restore else {
            return;
        };
        if restore.resolving {
            return;
        }
        if let Some((id, _)) = &restore.waiting {
            let ready = self
                .inactive_chat_widgets
                .get(id)
                .is_some_and(|widget| widget.session_id() == Some(Uuid::from(*id)))
                || matches!(&self.app_state, AppState::Chat { widget } if widget.session_id() == Some(Uuid::from(*id)));
            if !ready {
                return;
            }
            restore.waiting = None;
        }
        if let Some((uuid, path, cwd)) = restore.queue.pop_front() {
            let id = ThreadId::from_string(&uuid.to_string()).expect("UUID");
            let mut config = self.config.clone();
            config.experimental_resume = Some(path.clone());
            config.cwd = cwd;
            let mut widget = ChatWidget::new(ChatWidgetInit {
                config,
                conversation_manager: self._server.clone(),
                app_event_tx: self.app_event_tx.clone(),
                initial_prompt: None,
                initial_images: Vec::new(),
                terminal_info: self.terminal_info.clone(),
                show_order_overlay: self.show_order_overlay,
                latest_upgrade_version: self.latest_upgrade_version.clone(),
            });
            widget.enable_perf(self.timing_enabled);
            self.inactive_chat_widgets.insert(id, Box::new(widget));
            self.active_thread_router.set_thread_visible(id, true);
            restore.waiting = Some((id, path));
            return;
        }
        let restore = self.tab_restore.take().expect("checked above");
        let order: Vec<_> = restore
            .layout
            .sessions
            .iter()
            .map(|id| ThreadId::from_string(&id.to_string()).expect("UUID"))
            .collect();
        self.active_thread_router.restore_order(&order);
        self.persist_tab_state();
        if let Some(active) = restore.layout.active {
            self.switch_active_thread_backing(
                ThreadId::from_string(&active.to_string()).expect("UUID"),
            );
        }
        self.tab_layout_touched = true;
        self.tab_hits.clear();
        if restore.errors.is_empty() {
            self.tab_notice("Previous tabs restored. Existing tabs and unsent drafts were kept.");
        } else {
            self.tab_notice(format!(
                "Tabs restored with skipped sessions: {}",
                restore.errors.join("; ")
            ));
        }
        self.persist_tab_state();
    }

    pub(super) fn fail_tab_restore(&mut self, path: &Path) {
        let Some(restore) = &mut self.tab_restore else {
            return;
        };
        if let Some((id, pending_path)) = &restore.waiting
            && pending_path == path
        {
            let id = *id;
            restore.waiting = None;
            restore
                .errors
                .push(format!("{id}: failed to resume session"));
            self.inactive_chat_widgets.remove(&id);
            self.active_thread_router.set_thread_visible(id, false);
        }
        self.continue_tab_restore();
    }
}
