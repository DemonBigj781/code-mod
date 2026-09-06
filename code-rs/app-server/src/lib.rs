#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::io::Result as IoResult;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use code_common::CliConfigOverrides;
use code_core::AuthManager;
use code_core::config::Config;
use code_core::config::ConfigOverrides;
use code_app_server_protocol::AuthMode;
use mcp_types::JSONRPCMessage;
use mcp_types::RequestId;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing::warn;
use tracing_subscriber::EnvFilter;

use crate::installation_id::resolve_installation_id;
use crate::message_processor::MessageProcessor;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingEnvelope;
use crate::outgoing_message::OutgoingMessage;
use crate::outgoing_message::OutgoingMessageSender;
use crate::outgoing_message::OutgoingNotification;
use crate::remote_control::RemoteControlPolicy;
use crate::remote_control::RemoteControlStartConfig;
use crate::remote_control::auth::CoreRemoteControlAuthProvider;
use crate::remote_control::host_device::HostDevice;
use crate::remote_control::start_remote_control;
use crate::remote_control::state::RemoteControlState;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::ConnectionState;
use crate::transport::OutboundConnectionState;
use crate::transport::TransportEvent;
use crate::transport::route_outgoing_envelope;
use crate::transport::start_stdio_connection;
#[cfg(unix)]
use crate::transport::start_unix_socket_acceptor;
use crate::transport::start_websocket_acceptor;

pub mod code_message_processor;
mod command_exec;
mod exec_server_spawn;
mod error_code;
mod external_agent_config_api;
mod fs_api;
mod fs_watch;
mod installation_id;
#[allow(dead_code)]
mod fuzzy_file_search;
mod message_processor;
pub mod outgoing_message;
mod remote_control;
mod remote_control_processor;
#[cfg(test)]
mod remote_control_processor_tests;
mod transport;
mod thread_state;

pub use crate::transport::AppServerTransport;
pub use crate::remote_control::RemoteControlStartupMode;

const INTERNAL_REQUEST_ID_PREFIX: &str = "__code_internal_request__";

/// Control-plane messages from the processor side to the outbound router task.
enum OutboundControlEvent {
    Opened {
        connection_id: ConnectionId,
        writer: mpsc::Sender<OutgoingMessage>,
        initialized: Arc<AtomicBool>,
        opted_out_notification_methods: Arc<RwLock<HashSet<String>>>,
        disconnect_notify: Option<Arc<Notify>>,
    },
    Closed {
        connection_id: ConnectionId,
    },
}

#[derive(Clone, Debug)]
struct RequestRoute {
    connection_id: ConnectionId,
    original_request_id: RequestId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppServerRuntimeOptions {
    pub remote_control_startup_mode: RemoteControlStartupMode,
    pub install_shutdown_signal_handler: bool,
}

impl Default for AppServerRuntimeOptions {
    fn default() -> Self {
        Self {
            remote_control_startup_mode: RemoteControlStartupMode::ResolvePersisted,
            install_shutdown_signal_handler: true,
        }
    }
}

pub async fn run_main(
    code_linux_sandbox_exe: Option<PathBuf>,
    cli_config_overrides: CliConfigOverrides,
) -> IoResult<()> {
    run_main_with_transport(
        code_linux_sandbox_exe,
        cli_config_overrides,
        AppServerTransport::Stdio,
    )
    .await
}

pub async fn run_main_with_transport(
    code_linux_sandbox_exe: Option<PathBuf>,
    cli_config_overrides: CliConfigOverrides,
    transport: AppServerTransport,
) -> IoResult<()> {
    run_main_with_transport_options(
        code_linux_sandbox_exe,
        cli_config_overrides,
        transport,
        AppServerRuntimeOptions::default(),
    )
    .await
}

pub async fn run_main_with_transport_options(
    code_linux_sandbox_exe: Option<PathBuf>,
    cli_config_overrides: CliConfigOverrides,
    transport: AppServerTransport,
    runtime_options: AppServerRuntimeOptions,
) -> IoResult<()> {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let (transport_event_tx, mut transport_event_rx) =
        mpsc::channel::<TransportEvent>(CHANNEL_CAPACITY);
    let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<OutgoingEnvelope>(CHANNEL_CAPACITY);
    let (outbound_control_tx, mut outbound_control_rx) =
        mpsc::channel::<OutboundControlEvent>(CHANNEL_CAPACITY);

    // Parse CLI overrides once and derive the base Config eagerly so later
    // components do not need to work with raw TOML values.
    let cli_kv_overrides = cli_config_overrides.parse_overrides().map_err(|e| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("error parsing -c overrides: {e}"),
        )
    })?;
    let config_overrides = ConfigOverrides {
        code_linux_sandbox_exe: code_linux_sandbox_exe.clone(),
        ..Default::default()
    };
    let mut config_warnings = Vec::<serde_json::Value>::new();

    let config = match Config::load_with_cli_overrides(cli_kv_overrides.clone(), config_overrides.clone()) {
        Ok(config) => config,
        Err(err) => {
            config_warnings.push(json!({
                "summary": "Invalid configuration; using defaults.",
                "details": err.to_string(),
                "path": serde_json::Value::Null,
                "range": serde_json::Value::Null,
            }));
            Config::load_default_with_cli_overrides(cli_kv_overrides.clone(), config_overrides)
            .map_err(|fallback_err| {
                std::io::Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "error loading default config after config error: {fallback_err}"
                    ),
                )
            })?
        }
    };

    let config = Arc::new(config);
    let auth_manager = AuthManager::shared_with_mode_and_originator(
        config.code_home.clone(),
        AuthMode::ApiKey,
        config.responses_originator_header.clone(),
    );
    let remote_control_state = match RemoteControlState::open(&config.code_home).await {
        Ok(state) => Some(state),
        Err(error) => {
            warn!(error = %error, "remote control persistence is unavailable");
            None
        }
    };
    let installation_id = resolve_installation_id(&config.code_home).await?;
    let shutdown = CancellationToken::new();
    let remote_control_startup_mode = runtime_options.remote_control_startup_mode;

    if matches!(transport, AppServerTransport::Off)
        && matches!(
            remote_control_startup_mode,
            RemoteControlStartupMode::DisabledEphemeral
        )
    {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "no transport configured; use --listen or enable remote control",
        ));
    }
    if matches!(transport, AppServerTransport::Off)
        && matches!(
            remote_control_startup_mode,
            RemoteControlStartupMode::EnabledEphemeral
        )
        && remote_control_state.is_none()
    {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "no transport configured; remote control persistence is unavailable",
        ));
    }

    let (remote_control_task, remote_control_handle) = start_remote_control(
        RemoteControlStartConfig {
            remote_control_url: config.chatgpt_base_url.clone(),
            installation_id,
            host: HostDevice::detect(resolve_server_name()),
            policy: RemoteControlPolicy::Allowed,
        },
        remote_control_state,
        Arc::new(CoreRemoteControlAuthProvider::new(Arc::clone(&auth_manager))),
        transport_event_tx.clone(),
        shutdown.clone(),
        remote_control_startup_mode,
    )
    .await?;

    if matches!(transport, AppServerTransport::Off)
        && matches!(
            remote_control_startup_mode,
            RemoteControlStartupMode::ResolvePersisted
        )
    {
        let persisted_enabled = match remote_control_handle
            .resolve_persisted_preference(None)
            .await
        {
            Ok(enabled) => enabled,
            Err(error) => {
                warn!(error = %error, "failed to resolve persisted remote control preference");
                false
            }
        };
        if !persisted_enabled {
            shutdown.cancel();
            let _ = remote_control_task.await;
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "no transport configured; use --listen or enable remote control",
            ));
        }
    }

    let mut stdio_handles = Vec::<JoinHandle<()>>::new();
    let mut transport_accept_handles = Vec::<JoinHandle<()>>::new();
    let transport_start_result = match &transport {
        AppServerTransport::Stdio => {
            start_stdio_connection(
                transport_event_tx.clone(),
                &mut stdio_handles,
                shutdown.clone(),
            )
            .await
        }
        AppServerTransport::WebSocket { bind_address } => {
            match start_websocket_acceptor(
                *bind_address,
                transport_event_tx.clone(),
                shutdown.clone(),
            )
            .await
            {
                Ok(handle) => {
                    transport_accept_handles.push(handle);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        #[cfg(unix)]
        AppServerTransport::UnixSocket { socket_path } => {
            match start_unix_socket_acceptor(
                socket_path.clone(),
                transport_event_tx.clone(),
                shutdown.clone(),
            )
            .await
            {
                Ok(handle) => {
                    transport_accept_handles.push(handle);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        #[cfg(not(unix))]
        AppServerTransport::UnixSocket { .. } => Err(std::io::Error::new(
            ErrorKind::Unsupported,
            "Unix socket transport is unavailable on this platform",
        )),
        AppServerTransport::Off => Ok(()),
    };
    if let Err(error) = transport_start_result {
        shutdown.cancel();
        let _ = remote_control_task.await;
        return Err(error);
    }
    let shutdown_when_no_connections = matches!(transport, AppServerTransport::Stdio);

    let shutdown_signal_handle = runtime_options.install_shutdown_signal_handler.then(|| {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            match tokio::signal::ctrl_c().await {
                Ok(()) => shutdown.cancel(),
                Err(error) => warn!(error = %error, "failed to listen for shutdown signal"),
            }
        })
    });

    let request_routes = Arc::new(tokio::sync::Mutex::new(HashMap::<RequestId, RequestRoute>::new()));
    let request_routes_for_outbound = Arc::clone(&request_routes);
    let transport_event_tx_for_outbound = transport_event_tx.clone();
    let outbound_handle = tokio::spawn(async move {
        let mut outbound_connections = HashMap::<ConnectionId, OutboundConnectionState>::new();
        let mut pending_closed_connections = VecDeque::<ConnectionId>::new();
        loop {
            tokio::select! {
                biased;
                event = outbound_control_rx.recv() => {
                    let Some(event) = event else {
                        break;
                    };
                    match event {
                        OutboundControlEvent::Opened {
                            connection_id,
                            writer,
                            initialized,
                            opted_out_notification_methods,
                            disconnect_notify,
                        } => {
                            outbound_connections.insert(
                                connection_id,
                                OutboundConnectionState::new(
                                    writer,
                                    initialized,
                                    opted_out_notification_methods,
                                    disconnect_notify,
                                ),
                            );
                        }
                        OutboundControlEvent::Closed { connection_id } => {
                            outbound_connections.remove(&connection_id);
                        }
                    }
                }
                envelope = outgoing_rx.recv() => {
                    let Some(envelope) = envelope else {
                        break;
                    };
                    let Some(envelope) =
                        rewrite_response_routing(envelope, &request_routes_for_outbound).await
                    else {
                        continue;
                    };
                    let disconnected_connections =
                        route_outgoing_envelope(&mut outbound_connections, envelope).await;
                    pending_closed_connections.extend(disconnected_connections);
                }
            }

            while let Some(connection_id) = pending_closed_connections.front().copied() {
                match transport_event_tx_for_outbound
                    .try_send(TransportEvent::ConnectionClosed { connection_id })
                {
                    Ok(()) => {
                        pending_closed_connections.pop_front();
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        break;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        return;
                    }
                }
            }
        }
        info!("outbound router task exited (channel closed)");
    });

    let processor_handle = tokio::spawn({
        let outgoing_message_sender =
            Arc::new(OutgoingMessageSender::new_with_routed_sender(outgoing_tx));
        let outbound_control_tx = outbound_control_tx;
        let request_routes = Arc::clone(&request_routes);
        let mut processor = MessageProcessor::new_with_remote_control(
            Arc::clone(&outgoing_message_sender),
            code_linux_sandbox_exe,
            config,
            config_warnings,
            cli_kv_overrides,
            Some(remote_control_handle.clone()),
            Some(auth_manager),
        );
        let mut connections = HashMap::<ConnectionId, ConnectionState>::new();
        let mut next_internal_request_ordinal = 0u64;
        let mut remote_control_status_rx = remote_control_handle.status_receiver();
        let mut remote_control_status = remote_control_status_rx.borrow().clone();
        let processor_shutdown = shutdown.clone();
        async move {
            loop {
                let event = tokio::select! {
                    _ = processor_shutdown.cancelled() => break,
                    changed = remote_control_status_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let status = remote_control_status_rx.borrow().clone();
                        if status != remote_control_status {
                            remote_control_status = status.clone();
                            match serde_json::to_value(status) {
                                Ok(params) => {
                                    outgoing_message_sender
                                        .send_notification(OutgoingNotification {
                                            method: "remoteControl/status/changed".to_owned(),
                                            params: Some(params),
                                        })
                                        .await;
                                }
                                Err(error) => {
                                    warn!(error = %error, "failed to serialize remote control status");
                                }
                            }
                        }
                        continue;
                    }
                    event = transport_event_rx.recv() => event,
                };
                let Some(event) = event else {
                    break;
                };
                match event {
                    TransportEvent::ConnectionOpened {
                        connection_id,
                        writer,
                        disconnect_notify,
                    } => {
                        let outbound_initialized = Arc::new(AtomicBool::new(false));
                        let outbound_opted_out_notification_methods =
                            Arc::new(RwLock::new(HashSet::new()));
                        if outbound_control_tx
                            .send(OutboundControlEvent::Opened {
                                connection_id,
                                writer,
                                initialized: Arc::clone(&outbound_initialized),
                                opted_out_notification_methods: Arc::clone(
                                    &outbound_opted_out_notification_methods,
                                ),
                                disconnect_notify,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                        connections.insert(
                            connection_id,
                            ConnectionState::new(
                                outbound_initialized,
                                outbound_opted_out_notification_methods,
                            ),
                        );
                    }
                    TransportEvent::ConnectionClosed { connection_id } => {
                        if shutdown_when_no_connections {
                            // Stdio clients can close stdin after sending requests while still
                            // expecting pending responses on stdout.
                            outgoing_message_sender
                                .clear_callbacks_for_connection(connection_id)
                                .await;
                            processor.on_connection_closed(connection_id).await;
                            wait_for_request_routes_for_connection(
                                &request_routes,
                                connection_id,
                            )
                            .await;
                        }

                        if outbound_control_tx
                            .send(OutboundControlEvent::Closed { connection_id })
                            .await
                            .is_err()
                        {
                            break;
                        }
                        connections.remove(&connection_id);
                        remove_request_routes_for_connection(&request_routes, connection_id).await;
                        if !shutdown_when_no_connections {
                            outgoing_message_sender
                                .clear_callbacks_for_connection(connection_id)
                                .await;
                            processor.on_connection_closed(connection_id).await;
                        }

                        if shutdown_when_no_connections && connections.is_empty() {
                            break;
                        }
                    }
                    TransportEvent::IncomingMessage {
                        connection_id,
                        message,
                    } => match message {
                        JSONRPCMessage::Request(mut request) => {
                            let Some(connection_state) = connections.get_mut(&connection_id) else {
                                warn!("dropping request from unknown connection: {:?}", connection_id);
                                continue;
                            };

                            let original_request_id = request.id.clone();
                            let internal_request_id = RequestId::String(format!(
                                "{INTERNAL_REQUEST_ID_PREFIX}{}:{next_internal_request_ordinal}",
                                connection_id.0
                            ));
                            next_internal_request_ordinal += 1;
                            request.id = internal_request_id.clone();
                            {
                                let mut request_routes = request_routes.lock().await;
                                request_routes.insert(
                                    internal_request_id,
                                    RequestRoute {
                                        connection_id,
                                        original_request_id,
                                    },
                                );
                            }

                            let was_initialized = connection_state.session.initialized;
                            processor
                                .process_request(
                                    connection_id,
                                    request,
                                    &mut connection_state.session,
                                    &connection_state.outbound_initialized,
                                    &connection_state.outbound_opted_out_notification_methods,
                                )
                                .await;
                            if !was_initialized && connection_state.session.initialized {
                                processor.send_initialize_notifications(connection_id).await;
                            }
                        }
                        JSONRPCMessage::Response(response) => {
                            processor.process_response(connection_id, response).await;
                        }
                        JSONRPCMessage::Notification(notification) => {
                            processor.process_notification(notification);
                        }
                        JSONRPCMessage::Error(err) => {
                            processor.process_error(connection_id, err).await;
                        }
                    },
                }
            }

            for connection_id in connections.keys().copied().collect::<Vec<_>>() {
                outgoing_message_sender
                    .clear_callbacks_for_connection(connection_id)
                    .await;
                processor.on_connection_closed(connection_id).await;
                remove_request_routes_for_connection(&request_routes, connection_id).await;
            }

            info!("processor task exited (channel closed)");
        }
    });

    drop(transport_event_tx);

    let _ = processor_handle.await;
    shutdown.cancel();
    if let Some(handle) = shutdown_signal_handle {
        handle.abort();
        let _ = handle.await;
    }
    let _ = outbound_handle.await;
    match remote_control_task.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!(error = %error, "remote control task exited with an error"),
        Err(error) if error.is_cancelled() => {}
        Err(error) => warn!(error = %error, "remote control task failed"),
    }

    for handle in transport_accept_handles {
        let _ = handle.await;
    }
    for handle in stdio_handles {
        handle.abort();
        let _ = handle.await;
    }

    Ok(())
}

fn resolve_server_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .map(|hostname| hostname.trim().to_owned())
        .filter(|hostname| !hostname.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|hostname| hostname.trim().to_owned())
                .filter(|hostname| !hostname.is_empty())
        })
        .unwrap_or_else(|| "code-app-server".to_owned())
}

async fn rewrite_response_routing(
    envelope: OutgoingEnvelope,
    request_routes: &Arc<tokio::sync::Mutex<HashMap<RequestId, RequestRoute>>>,
) -> Option<OutgoingEnvelope> {
    let (connection_id, message) = match envelope {
        OutgoingEnvelope::ToConnection {
            connection_id,
            message,
        } => (Some(connection_id), message),
        OutgoingEnvelope::Broadcast { message } => (None, message),
    };

    match message {
        OutgoingMessage::Response(mut response) => {
            let route = {
                let mut request_routes = request_routes.lock().await;
                request_routes.remove(&response.id)
            };
            if let Some(route) = route {
                response.id = route.original_request_id;
                return Some(OutgoingEnvelope::ToConnection {
                    connection_id: route.connection_id,
                    message: OutgoingMessage::Response(response),
                });
            }

            if is_internal_request_id(&response.id) {
                warn!(
                    "dropping response for disconnected request route: {:?}",
                    response.id
                );
                return None;
            }

            Some(outgoing_envelope_for_connection(
                connection_id,
                OutgoingMessage::Response(response),
            ))
        }
        OutgoingMessage::Error(mut outgoing_error) => {
            let route = {
                let mut request_routes = request_routes.lock().await;
                request_routes.remove(&outgoing_error.id)
            };
            if let Some(route) = route {
                outgoing_error.id = route.original_request_id;
                return Some(OutgoingEnvelope::ToConnection {
                    connection_id: route.connection_id,
                    message: OutgoingMessage::Error(outgoing_error),
                });
            }

            if is_internal_request_id(&outgoing_error.id) {
                warn!(
                    "dropping error for disconnected request route: {:?}",
                    outgoing_error.id
                );
                return None;
            }

            Some(outgoing_envelope_for_connection(
                connection_id,
                OutgoingMessage::Error(outgoing_error),
            ))
        }
        message => Some(outgoing_envelope_for_connection(connection_id, message)),
    }
}

fn outgoing_envelope_for_connection(
    connection_id: Option<ConnectionId>,
    message: OutgoingMessage,
) -> OutgoingEnvelope {
    match connection_id {
        Some(connection_id) => OutgoingEnvelope::ToConnection {
            connection_id,
            message,
        },
        None => OutgoingEnvelope::Broadcast { message },
    }
}

fn is_internal_request_id(request_id: &RequestId) -> bool {
    matches!(request_id, RequestId::String(value) if value.starts_with(INTERNAL_REQUEST_ID_PREFIX))
}

async fn remove_request_routes_for_connection(
    request_routes: &Arc<tokio::sync::Mutex<HashMap<RequestId, RequestRoute>>>,
    connection_id: ConnectionId,
) {
    let mut request_routes = request_routes.lock().await;
    request_routes.retain(|_, route| route.connection_id != connection_id);
}

async fn wait_for_request_routes_for_connection(
    request_routes: &Arc<tokio::sync::Mutex<HashMap<RequestId, RequestRoute>>>,
    connection_id: ConnectionId,
) {
    loop {
        let has_pending_requests = {
            let request_routes = request_routes.lock().await;
            request_routes
                .values()
                .any(|route| route.connection_id == connection_id)
        };

        if !has_pending_requests {
            return;
        }

        sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outgoing_message::OutgoingError;
    use crate::outgoing_message::OutgoingResponse;
    use mcp_types::JSONRPCErrorError;
    use serde_json::json;

    fn request_routes(
        internal_request_id: RequestId,
        connection_id: ConnectionId,
        original_request_id: RequestId,
    ) -> Arc<tokio::sync::Mutex<HashMap<RequestId, RequestRoute>>> {
        Arc::new(tokio::sync::Mutex::new(HashMap::from([(
            internal_request_id,
            RequestRoute {
                connection_id,
                original_request_id,
            },
        )])))
    }

    #[tokio::test]
    async fn rewrites_connection_scoped_response_to_original_request_id() {
        let internal_request_id = RequestId::String(format!("{INTERNAL_REQUEST_ID_PREFIX}7:0"));
        let routes = request_routes(
            internal_request_id.clone(),
            ConnectionId(7),
            RequestId::Integer(42),
        );
        let envelope = OutgoingEnvelope::ToConnection {
            connection_id: ConnectionId(7),
            message: OutgoingMessage::Response(OutgoingResponse {
                id: internal_request_id,
                result: json!({ "ok": true }),
            }),
        };

        let rewritten = rewrite_response_routing(envelope, &routes)
            .await
            .expect("response should be routed");

        match rewritten {
            OutgoingEnvelope::ToConnection {
                connection_id,
                message: OutgoingMessage::Response(response),
            } => {
                assert_eq!(connection_id, ConnectionId(7));
                assert_eq!(response.id, RequestId::Integer(42));
            }
            other => panic!("unexpected envelope: {other:?}"),
        }
        assert!(routes.lock().await.is_empty());
    }

    #[tokio::test]
    async fn rewrites_connection_scoped_error_to_original_request_id() {
        let internal_request_id = RequestId::String(format!("{INTERNAL_REQUEST_ID_PREFIX}9:0"));
        let routes = request_routes(
            internal_request_id.clone(),
            ConnectionId(9),
            RequestId::String("client-request".to_string()),
        );
        let envelope = OutgoingEnvelope::ToConnection {
            connection_id: ConnectionId(9),
            message: OutgoingMessage::Error(OutgoingError {
                id: internal_request_id,
                error: JSONRPCErrorError {
                    code: -32000,
                    message: "failure".to_string(),
                    data: None,
                },
            }),
        };

        let rewritten = rewrite_response_routing(envelope, &routes)
            .await
            .expect("error should be routed");

        match rewritten {
            OutgoingEnvelope::ToConnection {
                connection_id,
                message: OutgoingMessage::Error(error),
            } => {
                assert_eq!(connection_id, ConnectionId(9));
                assert_eq!(error.id, RequestId::String("client-request".to_string()));
            }
            other => panic!("unexpected envelope: {other:?}"),
        }
        assert!(routes.lock().await.is_empty());
    }
}
