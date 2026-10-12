use code_app_server_client::AppServerClient;
use code_app_server_client::AppServerEvent;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;

/// Forward events emitted by the app-server client into the TUI event queue.
///
/// This is intentionally narrow while the TUI is still being migrated from
/// direct `code_core` events to app-server protocol events: server
/// notifications are delivered to the app, while request/response handling is
/// left to the upcoming session bridge.
pub(super) async fn forward_app_server_events(
    mut app_server_client: AppServerClient,
    app_event_tx: AppEventSender,
) {
    while let Some(event) = app_server_client.next_event().await {
        if !forward_app_server_event(event, &app_event_tx) {
            break;
        }
    }
}

pub(super) fn forward_app_server_event(
    event: AppServerEvent,
    app_event_tx: &AppEventSender,
) -> bool {
    match event {
        AppServerEvent::ServerNotification(notification) => {
            app_event_tx.send_with_result(AppEvent::from(notification))
        }
        AppServerEvent::ServerRequest(request) => {
            app_event_tx.send_background_event_with_placement_and_order(
                format!(
                    "App-server request `{:?}` is not wired into the TUI bridge yet.",
                    request.id()
                ),
                crate::app_event::BackgroundPlacement::Tail,
                None,
            );
            true
        }
        AppServerEvent::Disconnected { message } => {
            app_event_tx.send_background_event_with_placement_and_order(
                format!("App-server disconnected: {message}"),
                crate::app_event::BackgroundPlacement::Tail,
                None,
            );
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use code_app_server_protocol::ConfigWarningNotification;
    use code_app_server_protocol::ServerNotification;

    fn test_sender() -> (AppEventSender, std::sync::mpsc::Receiver<AppEvent>) {
        let (tx, rx) = channel();
        (AppEventSender::new(tx), rx)
    }

    #[test]
    fn forwards_server_notification_to_app_event_channel() {
        let (sender, rx) = test_sender();
        let notification = ServerNotification::ConfigWarning(ConfigWarningNotification {
            summary: "bad config".to_string(),
            details: Some("example details".to_string()),
            path: None,
            range: None,
        });

        let forwarded = forward_app_server_event(
            AppServerEvent::ServerNotification(notification.clone()),
            &sender,
        );

        assert!(forwarded);
        let event = rx
            .recv_timeout(Duration::from_millis(50))
            .expect("server notification should be forwarded");
        match event {
            AppEvent::AppServerNotification(actual) => assert_eq!(*actual, notification),
            other => panic!("expected app-server notification, got {other:?}"),
        }
    }

    #[test]
    fn disconnected_event_is_reported_as_background_message() {
        let (sender, rx) = test_sender();

        let forwarded = forward_app_server_event(
            AppServerEvent::Disconnected {
                message: "worker stopped".to_string(),
            },
            &sender,
        );

        assert!(forwarded);
        let event = rx
            .recv_timeout(Duration::from_millis(50))
            .expect("disconnect should be surfaced");
        match event {
            AppEvent::InsertBackgroundEvent { message, .. } => {
                assert!(message.contains("App-server disconnected: worker stopped"));
            }
            other => panic!("expected background event, got {other:?}"),
        }
    }

    #[test]
    fn notification_forwarding_stops_when_app_channel_is_closed() {
        let (sender, rx) = test_sender();
        drop(rx);

        let forwarded = forward_app_server_event(
            AppServerEvent::ServerNotification(ServerNotification::ConfigWarning(
                ConfigWarningNotification {
                    summary: "bad config".to_string(),
                    details: None,
                    path: None,
                    range: None,
                },
            )),
            &sender,
        );

        assert!(!forwarded);
    }
}
