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
use code_app_server_protocol::ThreadSubscribeParams;
use code_app_server_protocol::ThreadSubscribeResponse;
use code_app_server_protocol::ThreadSubscribeStatus;
use code_app_server_protocol::TurnStartParams;
use code_app_server_protocol::TurnStartResponse;
use code_app_server_protocol::UserInput;
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
    command_tx: mpsc::UnboundedSender<AppServerBridgeCommand>,
}

enum AppServerBridgeCommand {
    RefreshLoadedThreads,
    SubscribeThread(ThreadId),
    StartTurn { thread_id: ThreadId, text: String },
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
    let (command_tx, mut command_rx) = mpsc::unbounded_channel();
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
                        command = command_rx.recv() => {
                            match command {
                                Some(AppServerBridgeCommand::RefreshLoadedThreads) => {
                                    refresh_loaded_threads(&request_handle, &app_event_tx).await;
                                }
                                Some(AppServerBridgeCommand::SubscribeThread(thread_id)) => {
                                    subscribe_thread(&request_handle, &app_event_tx, thread_id).await;
                                }
                                Some(AppServerBridgeCommand::StartTurn { thread_id, text }) => {
                                    start_turn(&request_handle, &app_event_tx, thread_id, text).await;
                                }
                                None => break,
                            }
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
        command_tx,
    }
}

impl AppServerSessionBridge {
    pub(super) fn refresh_loaded_threads(&self) {
        let _ = self.command_tx.send(AppServerBridgeCommand::RefreshLoadedThreads);
    }

    pub(super) fn subscribe_thread(&self, thread_id: ThreadId) {
        let _ = self.command_tx.send(AppServerBridgeCommand::SubscribeThread(thread_id));
    }

    pub(super) fn start_turn(&self, thread_id: ThreadId, text: String) {
        let _ = self
            .command_tx
            .send(AppServerBridgeCommand::StartTurn { thread_id, text });
    }
}

async fn start_turn(
    request_handle: &AppServerRequestHandle,
    app_event_tx: &AppEventSender,
    thread_id: ThreadId,
    text: String,
) {
    let response = request_handle
        .request_typed::<TurnStartResponse>(ClientRequest::TurnStart {
            request_id: RequestId::String(format!(
                "tui-turn-start-{}-{}",
                thread_id,
                uuid::Uuid::new_v4()
            )),
            params: TurnStartParams {
                thread_id: thread_id.to_string(),
                input: vec![UserInput::Text {
                    text,
                    text_elements: Vec::new(),
                }],
                ..TurnStartParams::default()
            },
        })
        .await;

    let result = match response {
        Ok(response) => Ok(response.turn),
        Err(err) => {
            tracing::warn!(%thread_id, "failed to start app-server turn: {err}");
            Err(err.to_string())
        }
    };

    app_event_tx.send(AppEvent::AppServerTurnStartFinished { thread_id, result });
}

async fn subscribe_thread(
    request_handle: &AppServerRequestHandle,
    app_event_tx: &AppEventSender,
    thread_id: ThreadId,
) {
    let response = request_handle
        .request_typed::<ThreadSubscribeResponse>(ClientRequest::ThreadSubscribe {
            request_id: RequestId::String(format!(
                "tui-thread-subscribe-{}-{}",
                thread_id,
                uuid::Uuid::new_v4()
            )),
            params: ThreadSubscribeParams {
                thread_id: thread_id.to_string(),
            },
        })
        .await;

    let status = match response {
        Ok(response) => response.status,
        Err(err) => {
            tracing::warn!(%thread_id, "failed to subscribe to app-server thread: {err}");
            ThreadSubscribeStatus::NotLoaded
        }
    };

    let replay_thread = if status == ThreadSubscribeStatus::Subscribed {
        fetch_replayable_thread(request_handle, thread_id).await
    } else {
        None
    };
    let (replayable_turns, replayable_items) = replay_thread
        .as_ref()
        .map(replayable_thread_counts)
        .unwrap_or((0, 0));

    app_event_tx.send(AppEvent::AppServerThreadSubscribeFinished {
        thread_id,
        status,
        replayable_turns,
        replayable_items,
        replay_thread,
    });
}

async fn fetch_replayable_thread(
    request_handle: &AppServerRequestHandle,
    thread_id: ThreadId,
) -> Option<code_app_server_protocol::Thread> {
    let response = request_handle
        .request_typed::<ThreadReadResponse>(ClientRequest::ThreadRead {
            request_id: RequestId::String(format!(
                "tui-thread-replay-read-{}-{}",
                thread_id,
                uuid::Uuid::new_v4()
            )),
            params: ThreadReadParams {
                thread_id: thread_id.to_string(),
                include_turns: true,
            },
        })
        .await;

    match response {
        Ok(response) => Some(response.thread),
        Err(err) => {
            tracing::warn!(%thread_id, "failed to read app-server thread history after subscribe: {err}");
            None
        }
    }
}

fn replayable_thread_counts(thread: &code_app_server_protocol::Thread) -> (usize, usize) {
    let replayable_turns = thread.turns.len();
    let replayable_items = thread.turns.iter().map(|turn| turn.items.len()).sum();
    (replayable_turns, replayable_items)
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
    use super::replayable_thread_counts;
    use code_app_server_protocol::SessionSource;
    use code_app_server_protocol::Thread;
    use code_app_server_protocol::ThreadItem;
    use code_app_server_protocol::ThreadStatus;
    use code_app_server_protocol::Turn;
    use code_app_server_protocol::TurnStatus;
    use code_app_server_protocol::UserInput;
    use code_protocol::ThreadId;
    use std::path::PathBuf;

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

    #[test]
    fn replayable_thread_counts_counts_turns_and_items() {
        let thread = Thread {
            id: "00000000-0000-0000-0000-000000000001".to_string(),
            name: None,
            preview: String::new(),
            status: Some(ThreadStatus::Idle),
            model_provider: "openai".to_string(),
            created_at: 0,
            updated_at: 0,
            path: None,
            cwd: PathBuf::from("/tmp"),
            cli_version: "test".to_string(),
            source: SessionSource::AppServer,
            git_info: None,
            turns: vec![
                Turn {
                    id: "turn-1".to_string(),
                    items: vec![
                        ThreadItem::UserMessage {
                            id: "user-1".to_string(),
                            content: vec![UserInput::Text {
                                text: "hello".to_string(),
                                text_elements: Vec::new(),
                            }],
                        },
                        ThreadItem::AgentMessage {
                            id: "agent-1".to_string(),
                            text: "hi".to_string(),
                        },
                    ],
                    status: TurnStatus::Completed,
                    error: None,
                },
                Turn {
                    id: "turn-2".to_string(),
                    items: Vec::new(),
                    status: TurnStatus::Completed,
                    error: None,
                },
            ],
        };

        assert_eq!(replayable_thread_counts(&thread), (2, 2));
    }
}
