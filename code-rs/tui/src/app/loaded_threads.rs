//! Loaded app-server thread filtering for TUI tab restoration.
//!
//! The app server exposes loaded threads as a flat list. For tab parity we only
//! want subagent descendants of the current primary thread to appear beside the
//! active tab, not every loaded session in the process. This module keeps that
//! tree walk pure and testable.

use std::collections::HashMap;
use std::collections::HashSet;

use code_app_server_protocol::SessionSource;
use code_app_server_protocol::Thread;
use code_app_server_protocol::ThreadStatus;
use code_protocol::ThreadId;
use code_protocol::protocol::SubAgentSource;

/// A loaded subagent thread discovered from `SessionSource::SubAgent` edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedSubagentThread {
    pub(crate) thread_id: ThreadId,
    pub(crate) title: Option<String>,
    pub(crate) agent_nickname: Option<String>,
    pub(crate) agent_role: Option<String>,
    pub(crate) agent_path: Option<String>,
    pub(crate) is_running: bool,
    pub(crate) is_closed: bool,
}

/// Walk the subagent spawn tree rooted at `primary_thread_id`.
///
/// The result excludes the primary thread itself and is sorted by thread id for
/// deterministic tests and stable first-seen registration order.
pub(crate) fn find_loaded_subagent_threads_for_primary(
    threads: Vec<Thread>,
    primary_thread_id: ThreadId,
) -> Vec<LoadedSubagentThread> {
    let mut threads_by_id = HashMap::new();
    for thread in threads {
        let Ok(thread_id) = ThreadId::from_string(&thread.id) else {
            continue;
        };
        threads_by_id.insert(thread_id, thread);
    }

    let mut included = HashSet::new();
    let mut pending = vec![primary_thread_id];
    while let Some(parent_thread_id) = pending.pop() {
        for (thread_id, thread) in &threads_by_id {
            if included.contains(thread_id) {
                continue;
            }

            let Some(source_parent_thread_id) = thread_spawn_parent_thread_id(&thread.source)
            else {
                continue;
            };

            if source_parent_thread_id != parent_thread_id {
                continue;
            }

            included.insert(*thread_id);
            pending.push(*thread_id);
        }
    }

    let mut loaded_threads: Vec<LoadedSubagentThread> = included
        .into_iter()
        .filter_map(|thread_id| {
            threads_by_id
                .remove(&thread_id)
                .map(|thread| LoadedSubagentThread {
                    title: title_from_thread(&thread),
                    is_running: matches!(&thread.status, Some(ThreadStatus::Active { .. })),
                    is_closed: matches!(&thread.status, Some(ThreadStatus::NotLoaded)),
                    thread_id,
                    // code-rs does not currently persist upstream's explicit
                    // agent nickname/role/path fields. Reuse the user-facing
                    // name as the best available label, and leave role/path
                    // empty until the source data exists.
                    agent_nickname: thread.name,
                    agent_role: None,
                    agent_path: None,
                })
        })
        .collect();
    loaded_threads.sort_by_key(|thread| thread.thread_id.to_string());
    loaded_threads
}

fn title_from_thread(thread: &Thread) -> Option<String> {
    thread
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            let preview = thread.preview.trim();
            (!preview.is_empty()).then(|| preview.to_owned())
        })
}

fn thread_spawn_parent_thread_id(source: &SessionSource) -> Option<ThreadId> {
    match source {
        SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id, ..
        }) => Some(*parent_thread_id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread_id(suffix: u128) -> ThreadId {
        ThreadId::from_string(&format!("{suffix:032x}")).expect("valid thread id")
    }

    fn test_thread(thread_id: ThreadId, source: SessionSource) -> Thread {
        Thread {
            id: thread_id.to_string(),
            name: None,
            preview: String::new(),
            status: None,
            model_provider: "openai".to_string(),
            created_at: 0,
            updated_at: 0,
            path: None,
            cwd: std::path::PathBuf::from("/tmp"),
            cli_version: "0.0.0".to_string(),
            source,
            git_info: None,
            turns: Vec::new(),
        }
    }

    fn spawned(parent_thread_id: ThreadId, depth: i32) -> SessionSource {
        SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id,
            depth,
        })
    }

    #[test]
    fn finds_loaded_subagent_tree_for_primary_thread() {
        let primary = thread_id(1);
        let child_id = thread_id(2);
        let grandchild_id = thread_id(3);
        let unrelated_id = thread_id(4);

        let primary_thread = test_thread(primary, SessionSource::Cli);
        let mut child = test_thread(child_id, spawned(primary, 1));
        child.name = Some("Scout".to_string());
        child.status = Some(ThreadStatus::Active {
            active_flags: Vec::new(),
        });
        let mut grandchild = test_thread(grandchild_id, spawned(child_id, 2));
        grandchild.preview = "Grandchild preview".to_string();
        grandchild.status = Some(ThreadStatus::NotLoaded);
        let unrelated = test_thread(unrelated_id, spawned(unrelated_id, 1));

        let loaded = find_loaded_subagent_threads_for_primary(
            vec![primary_thread, child, grandchild, unrelated],
            primary,
        );

        assert_eq!(
            loaded,
            vec![
                LoadedSubagentThread {
                    thread_id: child_id,
                    title: Some("Scout".to_string()),
                    agent_nickname: Some("Scout".to_string()),
                    agent_role: None,
                    agent_path: None,
                    is_running: true,
                    is_closed: false,
                },
                LoadedSubagentThread {
                    thread_id: grandchild_id,
                    title: Some("Grandchild preview".to_string()),
                    agent_nickname: None,
                    agent_role: None,
                    agent_path: None,
                    is_running: false,
                    is_closed: true,
                },
            ]
        );
    }
}
