use std::collections::HashSet;
use std::collections::VecDeque;

const DEFAULT_CAPACITY: usize = 8_192;

pub(super) struct EventDeduper {
    capacity: usize,
    order: VecDeque<(String, u64)>,
    seen: HashSet<(String, u64)>,
}

impl EventDeduper {
    pub(super) fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            seen: HashSet::with_capacity(capacity),
        }
    }

    pub(super) fn is_duplicate(&mut self, id: &str, event_seq: u64) -> bool {
        if id.is_empty() {
            return false;
        }

        let key = (id.to_owned(), event_seq);
        if self.seen.contains(&key) {
            return true;
        }

        self.seen.insert(key.clone());
        self.order.push_back(key);
        while self.order.len() > self.capacity {
            if let Some(expired) = self.order.pop_front() {
                self.seen.remove(&expired);
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_replayed_event_identity() {
        let mut deduper = EventDeduper::with_capacity(4);

        assert!(!deduper.is_duplicate("turn-1", 7));
        assert!(deduper.is_duplicate("turn-1", 7));
        assert!(!deduper.is_duplicate("turn-1", 8));
    }

    #[test]
    fn evicts_old_identities_at_capacity() {
        let mut deduper = EventDeduper::with_capacity(2);

        assert!(!deduper.is_duplicate("turn-1", 1));
        assert!(!deduper.is_duplicate("turn-1", 2));
        assert!(!deduper.is_duplicate("turn-1", 3));
        assert!(!deduper.is_duplicate("turn-1", 1));
    }
}
