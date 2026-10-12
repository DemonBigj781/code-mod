use std::sync::Arc;

use code_app_server_client::AppServerClient;
use code_app_server_client::AppServerRequestHandle;
use code_app_server_client::InProcessAppServerClient;
use code_app_server_client::InProcessClientStartArgs;
use code_app_server_protocol::ClientRequest;
use code_app_server_protocol::RequestId;
use code_app_server_protocol::ThreadLoadedListParams;
use code_app_server_protocol::ThreadLoadedListResponse;
use code_app_server_protocol::ThreadReadParams;
use code_app_server_protocol::ThreadReadResponse;
use code_core::config::Config;
use code_core::ConversationManager;
use code_protocol::ThreadId;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;

use super::app_server_event_forwarder::forward_app_server_event;

pub(super) struct AppServerSessionBridge {
    task_handle: JoinHandle<()>,
    refresh_tx: mpsc::UnboundedSender<()>,
}

impl Drop for AppServerSessionBridge {
    fn drop(&mut self) {
        self.task_handle.abort();
    }
}

pub(super) fn spawn_in_process_app_server_bridge(
    config: Config,
    cli_kv_overrides: Vec<(String, toml::Value)>,
    app_event_tx: AppEventSender,
    conversation_manager: Arc<ConversationManager>,
) -> AppServerSessionBridge {
    let (refresh_tx, mut refresh_rx) = mpsc::unbounded_channel();
    let task_handle = tokio::spawn(async move {
        let code_linux_sandbox_exe = config.code_linux_sandbox_exe.clone();
        let client = InProcessAppServerClient::start(InProcessClientStartArgs {
            code_linux_sandbox_exe,
            config: Arc::new(config),
            cli_overrides: cli_kv_overrides,
            config_warnings: Vec::new(),
            client_name: "code-tui".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            experimental_api: false,
            opt_out_notification_methods: Vec::new(),
            channel_capacity: code_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
            conversation_manager: Some(conversation_manager),
        })
        .await;

        match client {
            Ok(client) => {
                let mut app_server_client = AppServerClient::InProcess(client);
                let request_handle = app_server_client.request_handle();
                refresh_loaded_threads(&request_handle, &app_event_tx).await;
                loop {
                    tokio::select! {
                        event = app_server_client.next_event() => {
                            let Some(event) = event else {
                                break;
                            };
                            if !forward_app_server_event(event, &app_event_tx) {
                                break;
                            }
                        }
                        refresh = refresh_rx.recv() => {
                            if refresh.is_none() {
                                break;
                            }
                            refresh_loaded_threads(&request_handle, &app_event_tx).await;
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!("failed to start in-process app-server bridge: {err}");
            }
        }
    });

    AppServerSessionBridge {
        task_handle,
        refresh_tx,
    }
}

impl AppServerSessionBridge {
    pub(super) fn refresh_loaded_threads(&self) {
        let _ = self.refresh_tx.send(());
    }
}

async fn refresh_loaded_threads(
    request_handle: &AppServerRequestHandle,
    app_event_tx: &AppEventSender,
) {
    let response = request_handle
        .request_typed::<ThreadLoadedListResponse>(ClientRequest::ThreadLoadedList {
            request_id: RequestId::String(format!(
                "tui-loaded-threads-{}",
                uuid::Uuid::new_v4()
            )),
            params: ThreadLoadedListParams {
                cursor: None,
                limit: None,
            },
        })
        .await;

    match response {
        Ok(response) => {
            let thread_ids = parse_loaded_thread_ids(response.data);
            if !thread_ids.is_empty() {
                app_event_tx.send(AppEvent::AppServerLoadedThreads(thread_ids.clone()));
                let details = fetch_loaded_thread_details(request_handle, &thread_ids).await;
                if !details.is_empty() {
                    app_event_tx.send(AppEvent::AppServerLoadedThreadDetails(details));
                }
            }
        }
        Err(err) => {
            tracing::warn!("failed to refresh loaded app-server threads: {err}");
        }
    }
}

async fn fetch_loaded_thread_details(
    request_handle: &AppServerRequestHandle,
    thread_ids: &[ThreadId],
) -> Vec<code_app_server_protocol::Thread> {
    let mut threads = Vec::new();
    for thread_id in thread_ids {
        let response = request_handle
            .request_typed::<ThreadReadResponse>(ClientRequest::ThreadRead {
                request_id: RequestId::String(format!(
                    "tui-thread-read-{}-{}",
                    thread_id,
                    uuid::Uuid::new_v4()
                )),
                params: ThreadReadParams {
                    thread_id: thread_id.to_string(),
                    include_turns: false,
                },
            })
            .await;

        match response {
            Ok(response) => threads.push(response.thread),
            Err(err) => {
                tracing::warn!(%thread_id, "failed to read loaded app-server thread details: {err}");
            }
        }
    }
    threads
}

fn parse_loaded_thread_ids(ids: Vec<String>) -> Vec<ThreadId> {
    ids.into_iter()
        .filter_map(|id| match ThreadId::from_string(&id) {
            Ok(thread_id) => Some(thread_id),
            Err(_) => {
                tracing::warn!(%id, "loaded app-server thread id could not be parsed");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_loaded_thread_ids;
    use code_protocol::ThreadId;

    #[test]
    fn parse_loaded_thread_ids_filters_invalid_values_in_order() {
        let first = "00000000-0000-0000-0000-000000000001";
        let second = "00000000-0000-0000-0000-000000000002";

        let parsed = parse_loaded_thread_ids(vec![
            first.to_string(),
            "not-a-thread".to_string(),
            second.to_string(),
        ]);

        assert_eq!(
            parsed,
            vec![
                ThreadId::from_string(first).expect("valid thread id"),
                ThreadId::from_string(second).expect("valid thread id"),
            ]
        );
    }
}
