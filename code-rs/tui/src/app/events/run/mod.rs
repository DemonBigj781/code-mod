use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};
#[cfg(debug_assertions)]
use std::path::PathBuf;

use color_eyre::eyre::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::SynchronizedUpdate;

use code_cloud_tasks_client::{CloudTaskError, TaskId};
use code_core::config::add_project_allowed_command;
use code_core::config_types::AuthCredentialsStoreMode;
use code_core::config_types::Notifications;
#[cfg(debug_assertions)]
use code_core::protocol::Event;
use code_core::protocol::{Op, SandboxPolicy};
use code_core::SessionCatalog;
use code_login::ServerOptions;
use portable_pty::PtySize;

use crate::app_event::{AppEvent, SessionPickerAction};
use crate::bottom_pane::SettingsSection;
use crate::chatwidget::ChatWidget;
use crate::cloud_tasks_service;
use crate::exec_command::strip_bash_lc_and_escape;
use crate::external_editor;
use crate::get_git_diff::get_git_diff;
use crate::history_cell;
use crate::slash_command::SlashCommand;
use crate::thread_spawner;
use crate::tui;

use super::priority::is_image_clipboard_paste_shortcut;
use super::shell_style_profile_summary::generate_shell_style_profile_summary;
use super::super::render::flatten_draw_result;
use super::super::state::{
    App,
    AppState,
    ChatWidgetArgs,
    LoginFlowState,
    ThemeSplitPreview,
    BACKPRESSURE_FORCED_DRAW_SKIPS,
};

fn auth_credentials_store_mode_label(mode: AuthCredentialsStoreMode) -> &'static str {
    match mode {
        AuthCredentialsStoreMode::File => "file",
        AuthCredentialsStoreMode::Keyring => "keyring",
        AuthCredentialsStoreMode::Auto => "auto",
        AuthCredentialsStoreMode::Ephemeral => "ephemeral",
    }
}

/// Mirror history lines into the terminal scrollback (non-alt-screen mode).
/// Wraps the synchronized-update dance so each call site is a one-liner.
fn mirror_to_scrollback(
    terminal: &mut tui::Tui,
    widget: &ChatWidget,
    lines: Vec<ratatui::text::Line<'_>>,
) {
    use std::io::stdout;
    let width = terminal.size().map_or(80, |s| s.width);
    let reserve = widget.desired_bottom_height(width).max(1);
    let _ = execute!(stdout(), crossterm::terminal::BeginSynchronizedUpdate);
    crate::insert_history::insert_history_lines_above(terminal, reserve, lines);
    let _ = execute!(stdout(), crossterm::terminal::EndSynchronizedUpdate);
}

impl App<'_> {
    pub(crate) fn run(&mut self, terminal: &mut tui::Tui) -> Result<()> {
        // Insert an event to trigger the first render.
        let app_event_tx = self.app_event_tx.clone();
        app_event_tx.send(AppEvent::RequestRedraw);
        // Some Windows/macOS terminals report an initial size that stabilizes
        // shortly after entering the alt screen. Schedule one follow‑up frame
        // to catch any late size change without polling.
        app_event_tx.send(AppEvent::ScheduleFrameIn(crate::timing::ANIMATION_FRAME_INTERVAL));

        'main: loop {
            let Some(event) = self.next_event_priority() else { break 'main };
            self.persist_tab_state();
            if !self.dispatch_app_event(event, terminal)? {
                break 'main;
            }
            self.persist_tab_state();
            self.continue_tab_restore();
        }
        self.persist_tab_state();
        if self.alt_screen_active {
            terminal.clear()?;
        }

        Ok(())
    }

    fn dispatch_app_event(&mut self, event: AppEvent, terminal: &mut tui::Tui) -> Result<bool> {
        let AppEvent::WidgetEvent { owner, event } = event else {
            return self.dispatch_raw_app_event(event, terminal, false);
        };
        if matches!(&self.app_state, AppState::Chat { widget }
            if widget.event_sender().owner() == Some(owner))
        {
            let AppState::Chat { widget } = &self.app_state else { unreachable!() };
            let sender = widget.event_sender();
            let previous_sender = std::mem::replace(&mut self.app_event_tx, sender);
            let result = self.dispatch_raw_app_event(*event, terminal, false);
            self.app_event_tx = previous_sender;
            return result;
        }

        let target = self.inactive_chat_widgets.iter().find_map(|(id, widget)| {
            (widget.event_sender().owner() == Some(owner)).then_some(*id)
        });
        let Some(target) = target else {
            tracing::debug!(%owner, "discarding callback for a removed widget");
            return Ok(true);
        };
        let widget = self.inactive_chat_widgets.remove(&target).expect("located widget");
        let previous_config = std::mem::replace(&mut self.config, widget.config_ref().clone());
        let previous_sender = std::mem::replace(&mut self.app_event_tx, widget.event_sender());
        let previous_state = std::mem::replace(&mut self.app_state, AppState::Chat { widget });
        let selected = self.active_thread_router.active_thread_id();
        let result = self.dispatch_raw_app_event(*event, terminal, true);
        let background_state = std::mem::replace(&mut self.app_state, previous_state);
        self.app_event_tx = previous_sender;
        self.config = previous_config;
        if let AppState::Chat { widget } = background_state {
            let id = widget.session_id()
                .and_then(|id| code_protocol::ThreadId::from_string(&id.to_string()).ok())
                .unwrap_or(target);
            self.inactive_chat_widgets.insert(id, widget);
        }
        if let Some(selected) = selected {
            self.active_thread_router.set_active_thread(selected);
        }
        result
    }

    fn dispatch_raw_app_event(
        &mut self,
        event: AppEvent,
        terminal: &mut tui::Tui,
        background: bool,
    ) -> Result<bool> {
        include!("history_insert.rs");
        Ok(true)
    }
}

#[cfg(test)]
mod tab_isolation_tests {
    use super::*;
    use crate::app::AppInitArgs;
    use code_core::config::{Config, ConfigOverrides, ConfigToml};
    use code_core::protocol::{Event, EventMsg, SessionConfiguredEvent};

    #[test]
    fn tab_isolation_routes_delayed_answers_without_changing_the_selected_tab() {
        let _runtime = crate::chatwidget::smoke_helpers::enter_test_runtime_guard();
        let _env = crate::chatwidget::smoke_helpers::TEST_ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("CODE_TUI_TEST_MODE", "1"); }
        let home = tempfile::tempdir().unwrap();
        let mut config = Config::load_from_base_config_with_overrides(
            ConfigToml::default(), ConfigOverrides::default(), home.path().to_path_buf(),
        ).unwrap();
        config.cwd = home.path().to_path_buf();
        config.tui.alternate_screen = true;
        let mut app = App::new(AppInitArgs {
            config, cli_kv_overrides: vec![], config_overrides: ConfigOverrides::default(),
            initial_prompt: None, initial_images: vec![], show_trust_screen: false,
            debug: false, show_order_overlay: false,
            terminal_info: crate::tui::TerminalInfo { picker: None, font_size: (8, 16) },
            enable_perf: false, resume_picker: false, fork_picker: false,
            fork_source_path: None, startup_footer_notice: None, latest_upgrade_version: None,
        });
        app.input_running.store(false, Ordering::Release);
        let configure = |id: uuid::Uuid| Event {
            id: id.to_string(), event_seq: 0, order: None,
            msg: EventMsg::SessionConfigured(SessionConfiguredEvent {
                session_id: id, model: "test".into(), history_log_id: 0, history_entry_count: 0,
            }),
        };
        let first = uuid::Uuid::new_v4();
        app.dispatch_thread_code_event(first, configure(first));
        let AppState::Chat { widget } = &app.app_state else { panic!("chat") };
        let first_owner = widget.event_sender().owner().unwrap();
        assert!(app.new_session_tab());
        let second = uuid::Uuid::new_v4();
        app.dispatch_thread_code_event(second, configure(second));
        let AppState::Chat { widget } = &app.app_state else { panic!("chat") };
        let second_owner = widget.event_sender().owner().unwrap();
        let first_id = code_protocol::ThreadId::from_string(&first.to_string()).unwrap();
        let second_id = code_protocol::ThreadId::from_string(&second.to_string()).unwrap();
        let mut terminal = ratatui::Terminal::with_options(
            ratatui::backend::CrosstermBackend::new(std::io::BufWriter::new(std::io::stdout())),
            ratatui::TerminalOptions { viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 80, 24)) },
        ).unwrap();

        for (owner, selected, marker, other) in [
            (first_owner, second_id, "alpha-only", "bravo-only"),
            (second_owner, first_id, "bravo-only", "alpha-only"),
        ] {
            assert!(app.switch_active_thread_backing(selected));
            let event = AppEvent::WidgetEvent { owner, event: Box::new(AppEvent::InsertFinalAnswer {
                id: Some(marker.into()), lines: vec![ratatui::text::Line::from(marker)], source: marker.into(),
            }) };
            assert!(app.dispatch_app_event(event, &mut terminal).unwrap());
            assert_eq!(app.active_thread_router.active_thread_id(), Some(selected));
            let AppState::Chat { widget } = &app.app_state else { panic!("chat") };
            assert!(!serde_json::to_string(&widget.export_response_items()).unwrap().contains(marker));
            let target = app.inactive_chat_widgets.values().find(|widget| widget.event_sender().owner() == Some(owner)).unwrap();
            let text = serde_json::to_string(&target.export_response_items()).unwrap();
            assert!(text.contains(marker), "answer missing from owner: {text}");
            assert!(!text.contains(other));
        }
        let before = app.active_chat_thread_id();
        app.dispatch_app_event(AppEvent::WidgetEvent {
            owner: uuid::Uuid::new_v4(), event: Box::new(AppEvent::ExitRequest),
        }, &mut terminal).unwrap();
        assert_eq!(app.active_chat_thread_id(), before);
        app.commit_anim_running.store(false, Ordering::Release);
        #[cfg(unix)]
        app.clear_sigterm_guard();
    }
}
