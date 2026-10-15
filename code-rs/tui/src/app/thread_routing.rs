//! Active app-server thread/session routing state for the TUI.
//!
//! This module ports the pure routing rules from the legacy app-server TUI path
//! into the current fork without assuming the rest of the legacy app-server
//! transport has already been wired in. `App` owns IO and widget mutation; this
//! module keeps the active-thread bookkeeping, stable tab order, and cached
//! session snapshots testable in isolation.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use code_core::config_types::ReasoningEffort;
use code_core::config_types::ServiceTier;
use code_core::protocol::AskForApproval;
use code_core::protocol::SandboxPolicy;
use code_protocol::ThreadId;

pub(crate) fn swap_active_thread_backing<T>(
    current_thread_id: ThreadId,
    active: &mut T,
    inactive: &mut HashMap<ThreadId, T>,
    target_thread_id: ThreadId,
) -> bool {
    let Some(target) = inactive.remove(&target_thread_id) else {
        return false;
    };
    let previous = std::mem::replace(active, target);
    inactive.insert(current_thread_id, previous);
    true
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ThreadSessionState {
    pub(crate) thread_id: ThreadId,
    pub(crate) forked_from_id: Option<ThreadId>,
    pub(crate) fork_parent_title: Option<String>,
    pub(crate) thread_name: Option<String>,
    pub(crate) model: String,
    pub(crate) model_provider_id: String,
    pub(crate) service_tier: Option<ServiceTier>,
    pub(crate) approval_policy: AskForApproval,
    pub(crate) sandbox_policy: SandboxPolicy,
    pub(crate) cwd: PathBuf,
    pub(crate) instruction_source_paths: Vec<PathBuf>,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    pub(crate) history_log_id: u64,
    pub(crate) history_entry_count: usize,
    pub(crate) rollout_path: Option<PathBuf>,
}

impl ThreadSessionState {
    pub(crate) fn display_title(&self, is_primary: bool) -> String {
        if is_primary {
            return "Main".to_owned();
        }

        self.thread_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.fork_parent_title
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(|name| format!("Fork: {name}"))
            })
            .unwrap_or_else(|| "Thread".to_owned())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ThreadNavigationDirection {
    Previous,
    Next,
}

#[derive(Debug, Default)]
pub(crate) struct ActiveThreadRouter {
    primary_thread_id: Option<ThreadId>,
    active_thread_id: Option<ThreadId>,
    sessions: HashMap<ThreadId, ThreadSessionState>,
    thread_metadata: HashMap<ThreadId, ThreadTabMetadata>,
    order: Vec<ThreadId>,
    hidden_threads: HashSet<ThreadId>,
}

/// Display/navigation metadata learned from app-server thread discovery.
///
/// This mirrors the pure parts of upstream Codex's `AgentNavigationState`: a
/// thread keeps its first-seen order even as title/role/running/closed metadata
/// changes, and hidden threads remain cached without appearing in tab cycling.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ThreadTabMetadata {
    pub(crate) title: Option<String>,
    pub(crate) agent_nickname: Option<String>,
    pub(crate) agent_role: Option<String>,
    pub(crate) agent_path: Option<String>,
    pub(crate) is_running: bool,
    pub(crate) is_closed: bool,
}

impl ActiveThreadRouter {
    pub(crate) fn active_thread_id(&self) -> Option<ThreadId> {
        self.active_thread_id.or(self.primary_thread_id)
    }

    pub(crate) fn upsert_session(&mut self, session: ThreadSessionState) {
        let thread_id = session.thread_id;
        if self.primary_thread_id.is_none() {
            self.primary_thread_id = Some(thread_id);
        }
        self.active_thread_id.get_or_insert(thread_id);
        self.ensure_ordered(thread_id);
        self.sessions.insert(thread_id, session);
    }

    pub(crate) fn upsert_thread_metadata(
        &mut self,
        thread_id: ThreadId,
        metadata: ThreadTabMetadata,
    ) {
        if self.primary_thread_id.is_none() {
            self.primary_thread_id = Some(thread_id);
        }
        self.active_thread_id.get_or_insert(thread_id);
        self.ensure_ordered(thread_id);
        let entry = self.thread_metadata.entry(thread_id).or_default();
        if metadata.title.is_some() {
            entry.title = metadata.title;
        }
        if metadata.agent_nickname.is_some() {
            entry.agent_nickname = metadata.agent_nickname;
        }
        if metadata.agent_role.is_some() {
            entry.agent_role = metadata.agent_role;
        }
        if metadata.agent_path.is_some() {
            entry.agent_path = metadata.agent_path;
        }
        entry.is_closed = metadata.is_closed;
        entry.is_running = metadata.is_running && !entry.is_closed;
    }

    pub(crate) fn set_thread_visible(&mut self, thread_id: ThreadId, is_visible: bool) {
        self.ensure_ordered(thread_id);
        if is_visible {
            self.hidden_threads.remove(&thread_id);
        } else {
            self.hidden_threads.insert(thread_id);
        }
    }

    pub(crate) fn update_thread_title(&mut self, thread_id: ThreadId, title: Option<String>) {
        if !self.thread_metadata.contains_key(&thread_id) {
            self.upsert_thread_metadata(thread_id, ThreadTabMetadata::default());
        }
        self.thread_metadata.get_mut(&thread_id).expect("registered metadata").title = title;
    }

    pub(crate) fn restore_order(&mut self, order: &[ThreadId]) {
        let mut seen = HashSet::new();
        self.order = order.iter().chain(self.order.iter()).copied()
            .filter(|id| self.order.contains(id) && seen.insert(*id)).collect();
    }

    pub(crate) fn is_visible(&self, id: ThreadId) -> bool {
        self.order.contains(&id) && !self.hidden_threads.contains(&id)
    }

    pub(crate) fn set_active_thread(&mut self, thread_id: ThreadId) -> bool {
        if !self.order.contains(&thread_id) || self.hidden_threads.contains(&thread_id) {
            return false;
        }
        self.active_thread_id = Some(thread_id);
        true
    }

    pub(crate) fn adjacent_thread_id(
        &self,
        direction: ThreadNavigationDirection,
    ) -> Option<ThreadId> {
        let current = self.active_thread_id()?;
        if self.visible_order().len() < 2 {
            return None;
        }
        let visible_order = self.visible_order();
        let current_idx = visible_order.iter().position(|thread_id| *thread_id == current)?;
        (1..visible_order.len()).find_map(|offset| {
            let idx = match direction {
                ThreadNavigationDirection::Next => (current_idx + offset) % visible_order.len(),
                ThreadNavigationDirection::Previous => {
                    (current_idx + visible_order.len() - offset) % visible_order.len()
                }
            };
            visible_order.get(idx).copied()
        })
    }

    #[cfg(test)]
    pub(crate) fn switch_adjacent(&mut self, direction: ThreadNavigationDirection) -> Option<ThreadId> {
        let next = self.adjacent_thread_id(direction)?;
        self.set_active_thread(next).then_some(next)
    }

    pub(crate) fn ordered_tabs(&self) -> Vec<ThreadTab> {
        self.visible_order()
            .iter()
            .enumerate()
            .map(|(index, thread_id)| {
                let is_primary = Some(*thread_id) == self.primary_thread_id;
                let title = self.display_title(*thread_id, is_primary);
                ThreadTab {
                    thread_id: *thread_id,
                    ordinal: index + 1,
                    title,
                    is_active: Some(*thread_id) == self.active_thread_id(),
                    is_primary,
                    is_running: self
                        .thread_metadata
                        .get(thread_id)
                        .is_some_and(|metadata| metadata.is_running),
                    is_closed: self
                        .thread_metadata
                        .get(thread_id)
                        .is_some_and(|metadata| metadata.is_closed),
                }
            })
            .collect()
    }

    fn ensure_ordered(&mut self, thread_id: ThreadId) {
        if !self.order.contains(&thread_id) {
            self.order.push(thread_id);
        }
    }

    fn visible_order(&self) -> Vec<ThreadId> {
        self.order
            .iter()
            .copied()
            .filter(|thread_id| !self.hidden_threads.contains(thread_id))
            .collect()
    }

    fn display_title(&self, thread_id: ThreadId, is_primary: bool) -> String {
        if let Some(metadata) = self.thread_metadata.get(&thread_id) {
            if let Some(title) = metadata
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
            {
                return title.to_owned();
            }
            if !is_primary {
                if let Some(agent_path) = metadata
                    .agent_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|agent_path| !agent_path.is_empty())
                {
                    return format!("`{agent_path}`");
                }
                if metadata.agent_nickname.is_some() || metadata.agent_role.is_some() {
                    return format_agent_label(
                        metadata.agent_nickname.as_deref(),
                        metadata.agent_role.as_deref(),
                    );
                }
            }
        }

        self.sessions
            .get(&thread_id)
            .map(|session| session.display_title(is_primary))
            .unwrap_or_else(|| {
                if is_primary {
                    "Main".to_owned()
                } else {
                    let thread_id = thread_id.to_string();
                    let short_id: String = thread_id.chars().take(8).collect();
                    format!("Agent ({short_id})")
                }
            })
    }
}

fn format_agent_label(agent_nickname: Option<&str>, agent_role: Option<&str>) -> String {
    match (
        agent_nickname.map(str::trim).filter(|value| !value.is_empty()),
        agent_role.map(str::trim).filter(|value| !value.is_empty()),
    ) {
        (Some(nickname), Some(role)) => format!("{nickname} ({role})"),
        (Some(nickname), None) => nickname.to_owned(),
        (None, Some(role)) => role.to_owned(),
        (None, None) => "Agent".to_owned(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ThreadTab {
    pub(crate) thread_id: ThreadId,
    pub(crate) ordinal: usize,
    pub(crate) title: String,
    pub(crate) is_active: bool,
    pub(crate) is_primary: bool,
    pub(crate) is_running: bool,
    pub(crate) is_closed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn tab_isolation_title_changes_preserve_running_state_and_other_titles() {
        let mut router = ActiveThreadRouter::default();
        let first = thread_id(91);
        let second = thread_id(92);
        for id in [first, second] {
            router.upsert_thread_metadata(id, ThreadTabMetadata { is_running: true, ..Default::default() });
        }
        router.update_thread_title(first, Some("Alpha".into()));
        router.update_thread_title(second, Some("Bravo".into()));
        router.update_thread_title(first, Some("New topic".into()));
        let tabs = router.ordered_tabs();
        assert_eq!(tabs[0].title, "New topic");
        assert_eq!(tabs[1].title, "Bravo");
        assert!(tabs.iter().all(|tab| tab.is_running));
        router.update_thread_title(first, None);
        assert_eq!(router.ordered_tabs()[0].title, "Main");
        assert_eq!(router.ordered_tabs()[1].title, "Bravo");
    }

    fn thread_id(suffix: u32) -> ThreadId {
        ThreadId::from_string(&format!("00000000-0000-0000-0000-{suffix:012}"))
            .expect("valid thread id")
    }

    fn session(thread_id: ThreadId, name: Option<&str>) -> ThreadSessionState {
        ThreadSessionState {
            thread_id,
            forked_from_id: None,
            fork_parent_title: None,
            thread_name: name.map(ToOwned::to_owned),
            model: "gpt-test".to_owned(),
            model_provider_id: "test".to_owned(),
            service_tier: None,
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            cwd: PathBuf::from("/tmp"),
            instruction_source_paths: Vec::new(),
            reasoning_effort: None,
            history_log_id: 0,
            history_entry_count: 0,
            rollout_path: None,
        }
    }

    #[test]
    fn closed_tabs_stay_hidden_after_discovery_and_restore_in_saved_order() {
        let mut router = ActiveThreadRouter::default();
        let first = thread_id(1); let second = thread_id(2); let third = thread_id(3);
        for id in [first, second, third] { router.upsert_session(session(id, None)); }
        router.set_active_thread(second);
        router.set_thread_visible(first, false);
        router.upsert_thread_metadata(first, ThreadTabMetadata::default());
        assert!(!router.is_visible(first));
        assert!(!router.set_active_thread(first));
        assert_eq!(router.adjacent_thread_id(ThreadNavigationDirection::Previous), Some(third));
        router.set_thread_visible(first, true);
        router.restore_order(&[third, first, third, thread_id(99)]);
        assert_eq!(router.ordered_tabs().iter().map(|tab| tab.thread_id).collect::<Vec<_>>(), vec![third, first, second]);
        assert_eq!(router.active_thread_id(), Some(second));
    }

    #[test]
    fn active_thread_navigation_preserves_first_seen_order() {
        let main = thread_id(1);
        let side = thread_id(2);
        let third = thread_id(3);
        let mut router = ActiveThreadRouter::default();

        router.upsert_session(session(main, None));
        router.upsert_session(session(side, Some("Review")));
        router.upsert_session(session(third, Some("Docs")));

        assert_eq!(router.active_thread_id(), Some(main));
        assert_eq!(router.switch_adjacent(ThreadNavigationDirection::Next), Some(side));
        assert_eq!(router.switch_adjacent(ThreadNavigationDirection::Next), Some(third));
        assert_eq!(router.switch_adjacent(ThreadNavigationDirection::Next), Some(main));
        assert_eq!(router.switch_adjacent(ThreadNavigationDirection::Previous), Some(third));
    }

    #[test]
    fn ordered_tabs_mark_primary_and_active() {
        let main = thread_id(11);
        let side = thread_id(12);
        let mut router = ActiveThreadRouter::default();

        router.upsert_session(session(main, None));
        router.upsert_session(session(side, Some("Review")));
        assert!(router.set_active_thread(side));

        let tabs = router.ordered_tabs();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].ordinal, 1);
        assert_eq!(tabs[0].title, "Main");
        assert!(tabs[0].is_primary);
        assert!(!tabs[0].is_active);
        assert!(!tabs[0].is_running);
        assert!(!tabs[0].is_closed);
        assert_eq!(tabs[1].ordinal, 2);
        assert_eq!(tabs[1].title, "Review");
        assert!(!tabs[1].is_primary);
        assert!(tabs[1].is_active);
    }

    #[test]
    fn metadata_preserves_first_seen_order_and_labels_agent_tabs() {
        let main = thread_id(21);
        let agent = thread_id(22);
        let mut router = ActiveThreadRouter::default();

        router.upsert_session(session(main, None));
        router.upsert_thread_metadata(
            agent,
            ThreadTabMetadata {
                agent_nickname: Some("Builder".to_owned()),
                agent_role: Some("implementation".to_owned()),
                is_running: true,
                ..ThreadTabMetadata::default()
            },
        );

        let tabs = router.ordered_tabs();
        assert_eq!(tabs.iter().map(|tab| tab.thread_id).collect::<Vec<_>>(), vec![main, agent]);
        assert_eq!(tabs[1].title, "Builder (implementation)");
        assert!(tabs[1].is_running);
        assert!(!tabs[1].is_closed);
    }

    #[test]
    fn hidden_threads_remain_cached_but_are_skipped_by_navigation_and_tabs() {
        let main = thread_id(31);
        let hidden = thread_id(32);
        let visible = thread_id(33);
        let mut router = ActiveThreadRouter::default();

        router.upsert_session(session(main, None));
        router.upsert_session(session(hidden, Some("Hidden")));
        router.upsert_session(session(visible, Some("Visible")));
        router.set_thread_visible(hidden, false);

        assert_eq!(router.adjacent_thread_id(ThreadNavigationDirection::Next), Some(visible));
        assert!(!router.set_active_thread(hidden));
        let tabs = router.ordered_tabs();
        assert_eq!(tabs.iter().map(|tab| tab.thread_id).collect::<Vec<_>>(), vec![main, visible]);
    }

    #[test]
    fn status_metadata_updates_do_not_erase_existing_labels() {
        let agent = thread_id(41);
        let mut router = ActiveThreadRouter::default();

        router.upsert_thread_metadata(
            agent,
            ThreadTabMetadata {
                title: Some("Investigate bug".to_owned()),
                ..ThreadTabMetadata::default()
            },
        );
        router.upsert_thread_metadata(
            agent,
            ThreadTabMetadata {
                is_running: true,
                ..ThreadTabMetadata::default()
            },
        );

        let tabs = router.ordered_tabs();
        assert_eq!(tabs[0].title, "Investigate bug");
    }

    #[test]
    fn backing_thread_swap_preserves_the_previous_session() {
        let main = thread_id(51);
        let side = thread_id(52);
        let mut active = "main transcript";
        let mut inactive = HashMap::from([(side, "side transcript")]);

        assert!(swap_active_thread_backing(
            main,
            &mut active,
            &mut inactive,
            side,
        ));
        assert_eq!(active, "side transcript");
        assert_eq!(inactive.get(&main), Some(&"main transcript"));
        assert!(!inactive.contains_key(&side));
    }

    #[test]
    fn backing_thread_swap_does_not_change_state_without_a_target() {
        let main = thread_id(61);
        let missing = thread_id(62);
        let mut active = "main transcript";
        let mut inactive = HashMap::new();

        assert!(!swap_active_thread_backing(
            main,
            &mut active,
            &mut inactive,
            missing,
        ));
        assert_eq!(active, "main transcript");
        assert!(inactive.is_empty());
    }
}
