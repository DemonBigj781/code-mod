use std::sync::atomic::Ordering;

use super::{App, AppState};
use super::super::thread_routing::ThreadSessionState;
use super::super::thread_routing::ThreadTabMetadata;
use crate::chatwidget::ChatWidget;
use crate::session_tabs::TopSessionTab;
use code_core::protocol::SessionConfiguredEvent;
use code_protocol::ThreadId;

impl<'a> App<'a> {
    pub(crate) const DEFAULT_TERMINAL_TITLE: &'static str = "Code";

    #[cfg(unix)]
    pub(crate) fn sigterm_triggered(&self) -> bool {
        self.sigterm_flag.load(Ordering::Relaxed)
    }

    #[cfg(unix)]
    pub(crate) fn clear_sigterm_guard(&mut self) {
        self.sigterm_guard.take();
    }

    pub(crate) fn token_usage(&self) -> code_core::protocol::TokenUsage {
        let usage = match &self.app_state {
            AppState::Chat { widget } => widget.token_usage().clone(),
            AppState::Onboarding { .. } => code_core::protocol::TokenUsage::default(),
        };
        // ensure background helpers stop before returning
        self.commit_anim_running.store(false, Ordering::Release);
        self.input_running.store(false, Ordering::Release);
        usage
    }

    pub(crate) fn session_id(&self) -> Option<uuid::Uuid> {
        match &self.app_state {
            AppState::Chat { widget } => widget.session_id(),
            AppState::Onboarding { .. } => None,
        }
    }

    pub(crate) fn reload_config_with_startup_overrides(&self) -> anyhow::Result<code_core::config::Config> {
        let mut config = code_core::config::Config::load_with_cli_overrides(
            self.cli_kv_overrides.clone(),
            self.config_overrides.clone(),
        )?;

        // Preserve CLI-only overrides that don't flow through `ConfigOverrides`.
        config.demo_developer_message.clone_from(&self.config.demo_developer_message);

        Ok(config)
    }

    pub(crate) fn top_session_tabs(&self) -> Vec<TopSessionTab> {
        self
            .active_thread_router
            .ordered_tabs()
            .into_iter()
            .map(|tab| TopSessionTab {
                thread_id: Some(tab.thread_id),
                ordinal: tab.ordinal,
                title: tab.title,
                is_active: tab.is_active,
                is_running: tab.is_running,
                is_closed: tab.is_closed,
            })
            .collect()
    }

    pub(crate) fn active_chat_thread_id(&self) -> Option<ThreadId> {
        match &self.app_state {
            AppState::Chat { widget } => widget
                .session_id()
                .and_then(|id| ThreadId::from_string(&id.to_string()).ok()),
            AppState::Onboarding { .. } => None,
        }
    }

    pub(crate) fn active_app_server_replay_thread(
        &self,
    ) -> Option<&code_app_server_protocol::Thread> {
        let active_thread_id = self.active_thread_router.active_thread_id()?;
        if self.active_chat_thread_id() == Some(active_thread_id) {
            return None;
        }
        self.app_server_thread_replays.get(&active_thread_id)
    }

    pub(crate) fn is_app_server_replay_view_active(&self) -> bool {
        self.active_app_server_replay_thread().is_some()
    }

    pub(crate) fn replace_active_chat_widget(&mut self, new_widget: ChatWidget<'a>) {
        self.config = new_widget.config_ref().clone();
        let AppState::Chat { widget } = &mut self.app_state else {
            self.app_state = AppState::Chat {
                widget: Box::new(new_widget),
            };
            return;
        };

        let previous = std::mem::replace(widget, Box::new(new_widget));
        let Some(previous_thread_id) = previous
            .session_id()
            .and_then(|id| ThreadId::from_string(&id.to_string()).ok())
        else {
            return;
        };
        self.inactive_chat_widgets
            .insert(previous_thread_id, previous);
    }

    pub(crate) fn register_session_configured_with_router(
        &mut self,
        session: &SessionConfiguredEvent,
    ) {
        let Ok(thread_id) = ThreadId::from_string(&session.session_id.to_string()) else {
            tracing::warn!(session_id = %session.session_id, "session id could not be converted to thread id");
            return;
        };

        let session_state = ThreadSessionState {
            thread_id,
            forked_from_id: None,
            fork_parent_title: None,
            thread_name: None,
            model: session.model.clone(),
            model_provider_id: self.config.model_provider_id.clone(),
            service_tier: self.config.service_tier,
            approval_policy: self.config.approval_policy,
            sandbox_policy: self.config.sandbox_policy.clone(),
            cwd: self.config.cwd.clone(),
            instruction_source_paths: Vec::new(),
            reasoning_effort: Some(self.config.model_reasoning_effort),
            history_log_id: session.history_log_id,
            history_entry_count: session.history_entry_count,
            rollout_path: None,
        };
        self.active_thread_router.upsert_session(session_state);
        self.active_thread_router.upsert_thread_metadata(
            thread_id,
            ThreadTabMetadata {
                title: None,
                agent_nickname: None,
                agent_role: None,
                agent_path: None,
                is_running: true,
                is_closed: false,
            },
        );
        if let Some(bridge) = &self._app_server_bridge {
            bridge.refresh_loaded_threads();
        }
    }

    /// Return a human-readable performance summary if timing was enabled.
    pub(crate) fn perf_summary(&self) -> Option<String> {
        if !self.timing_enabled {
            return None;
        }
        let mut out = String::new();
        if let AppState::Chat { widget } = &self.app_state {
            out.push_str(&widget.perf_summary());
            out.push_str("\n\n");
        }
        out.push_str(&self.timing.summarize());
        Some(out)
    }
}
