use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use super::QueuedUserInput;

/// Session configurations share one inbox; replacing a client never clones or loses submissions.
#[derive(Clone, Default)]
pub(in crate::codex) struct OperatorInbox {
    pending: Arc<Mutex<VecDeque<QueuedUserInput>>>,
    changed: Arc<Notify>,
}

impl OperatorInbox {
    pub(super) fn push(&self, input: QueuedUserInput) {
        self.pending.lock().expect("operator inbox poisoned").push_back(input);
        self.changed.notify_waiters();
    }

    pub(super) fn len(&self) -> usize {
        self.pending.lock().expect("operator inbox poisoned").len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(super) fn pop(&self) -> Option<QueuedUserInput> {
        self.pending.lock().expect("operator inbox poisoned").pop_front()
    }

    pub(super) fn drain(&self) -> Vec<QueuedUserInput> {
        self.pending.lock().expect("operator inbox poisoned").drain(..).collect()
    }

    pub(super) async fn wait(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            // Register before inspecting the queue so a concurrent submission cannot be missed.
            changed.as_mut().enable();
            if !self.is_empty() {
                return;
            }
            changed.await;
        }
    }
}
