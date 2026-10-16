//! Suppress raw input diagnostics while any local credential form is open.

use std::sync::atomic::{AtomicUsize, Ordering};

static ACTIVE_FORMS: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct SensitiveInputGuard;

impl SensitiveInputGuard {
    pub(crate) fn new() -> Self {
        ACTIVE_FORMS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for SensitiveInputGuard {
    fn drop(&mut self) {
        ACTIVE_FORMS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) fn active() -> bool {
    ACTIVE_FORMS.load(Ordering::SeqCst) != 0
}

pub(crate) fn redact_event(event: &crate::app_event::AppEvent) -> bool {
    use crate::app_event::AppEvent;
    if !active() {
        return false;
    }
    match event {
        AppEvent::KeyEvent(_) | AppEvent::Paste(_) => true,
        AppEvent::WidgetEvent { event, .. } => redact_event(event),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_sensitive_input_stays_protected_until_all_forms_close() {
        let before = ACTIVE_FORMS.load(Ordering::SeqCst);
        let first = SensitiveInputGuard::new();
        let second = SensitiveInputGuard::new();
        assert!(active());
        drop(first);
        assert!(active());
        drop(second);
        assert_eq!(ACTIVE_FORMS.load(Ordering::SeqCst), before);
    }

    #[test]
    fn ssh_sensitive_input_redacts_raw_and_widget_scoped_events() {
        use crate::app_event::AppEvent;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let _guard = SensitiveInputGuard::new();
        assert!(redact_event(&AppEvent::Paste("test-only-secret".into())));
        assert!(redact_event(&AppEvent::KeyEvent(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE
        ))));
        assert!(redact_event(&AppEvent::WidgetEvent {
            owner: uuid::Uuid::new_v4(),
            event: Box::new(AppEvent::Paste("test-only-secret".into()))
        }));
        assert!(!redact_event(&AppEvent::RequestRedraw));
    }
}
