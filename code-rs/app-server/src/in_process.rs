//! In-process app-server runtime host for local embedders.
//!
//! This module mirrors the socket/stdio app-server transport with in-memory
//! channels. Client commands and the consumer-facing event queue are bounded;
//! an ordered internal event staging queue keeps backpressure from blocking
//! request resolution while the delivery task waits for consumer capacity. It
//! intentionally routes every request through the normal [`MessageProcessor`]
//! so embedded clients observe the same JSON-RPC behavior as remote clients
//! without forking a second execution contract.

use std::collections::HashSet;
use std::io::Error as IoError;
use std::io::ErrorKind;
use std::io::Result as IoResult;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use code_app_server_protocol::ClientNotification;
use code_app_server_protocol::ClientRequest;
use code_app_server_protocol::ConfigWarningNotification;
use code_app_server_protocol::InitializeParams;
use code_app_server_protocol::JSONRPCErrorError as ProtocolJsonRpcError;
use code_app_server_protocol::RequestId as ProtocolRequestId;
use code_app_server_protocol::Result as ProtocolResult;
use code_app_server_protocol::ServerNotification;
use code_app_server_protocol::ServerRequest;
use code_core::config::Config;
use code_core::ConversationManager;
use mcp_types::JSONRPCNotification;
use mcp_types::JSONRPCRequest;
use mcp_types::JSONRPCResponse;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use toml::Value as TomlValue;
use tracing::warn;

use crate::error_code::INTERNAL_ERROR_CODE;
use crate::error_code::INVALID_REQUEST_ERROR_CODE;
use crate::message_processor::ConnectionSessionState;
use crate::message_processor::MessageProcessor;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessage;
use crate::outgoing_message::OutgoingMessageSender;
use crate::transport::CHANNEL_CAPACITY;

const IN_PROCESS_CONNECTION_ID: ConnectionId = ConnectionId(0);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Default bounded channel capacity for in-process runtime queues.
pub const DEFAULT_IN_PROCESS_CHANNEL_CAPACITY: usize = CHANNEL_CAPACITY;

type PendingClientRequestResponse = std::result::Result<ProtocolResult, ProtocolJsonRpcError>;

/// Input needed to start an in-process app-server runtime.
#[derive(Clone)]
pub struct InProcessStartArgs {
    pub code_linux_sandbox_exe: Option<PathBuf>,
    pub config: Arc<Config>,
    pub cli_overrides: Vec<(String, TomlValue)>,
    pub config_warnings: Vec<ConfigWarningNotification>,
    pub initialize: InitializeParams,
    pub channel_capacity: usize,
    pub conversation_manager: Option<Arc<ConversationManager>>,
}

/// Event emitted from the app-server to the in-process client.
///
/// Events are delivered in order without a lag/drop mode. If the event
/// consumer closes, pending server requests are rejected instead of orphaned.
#[derive(Debug, Clone)]
pub enum InProcessServerEvent {
    ServerRequest(ServerRequest),
    ServerNotification(ServerNotification),
}

enum InProcessClientMessage {
    Request {
        request: Box<ClientRequest>,
        response_tx: oneshot::Sender<PendingClientRequestResponse>,
    },
    Notification {
        notification: ClientNotification,
    },
    ServerRequestResponse {
        request_id: ProtocolRequestId,
        result: ProtocolResult,
    },
    ServerRequestError {
        request_id: ProtocolRequestId,
        error: ProtocolJsonRpcError,
    },
    Shutdown {
        done_tx: oneshot::Sender<()>,
    },
}

#[derive(Clone)]
pub struct InProcessClientSender {
    client_tx: mpsc::Sender<InProcessClientMessage>,
}

impl InProcessClientSender {
    pub async fn request(&self, request: ClientRequest) -> IoResult<PendingClientRequestResponse> {
        let (response_tx, response_rx) = oneshot::channel();
        self.try_send_client_message(InProcessClientMessage::Request {
            request: Box::new(request),
            response_tx,
        })?;
        response_rx.await.map_err(|err| {
            IoError::new(
                ErrorKind::BrokenPipe,
                format!("in-process request response channel closed: {err}"),
            )
        })
    }

    pub fn notify(&self, notification: ClientNotification) -> IoResult<()> {
        self.try_send_client_message(InProcessClientMessage::Notification { notification })
    }

    pub fn respond_to_server_request(
        &self,
        request_id: ProtocolRequestId,
        result: ProtocolResult,
    ) -> IoResult<()> {
        self.try_send_client_message(InProcessClientMessage::ServerRequestResponse {
            request_id,
            result,
        })
    }

    pub fn fail_server_request(
        &self,
        request_id: ProtocolRequestId,
        error: ProtocolJsonRpcError,
    ) -> IoResult<()> {
        self.try_send_client_message(InProcessClientMessage::ServerRequestError { request_id, error })
    }

    fn try_send_client_message(&self, message: InProcessClientMessage) -> IoResult<()> {
        self.client_tx.try_send(message).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => {
                IoError::new(ErrorKind::WouldBlock, "in-process app-server queue is full")
            }
            mpsc::error::TrySendError::Closed(_) => IoError::new(
                ErrorKind::BrokenPipe,
                "in-process app-server runtime has stopped",
            ),
        })
    }
}

pub struct InProcessClientHandle {
    client: InProcessClientSender,
    event_rx: mpsc::Receiver<InProcessServerEvent>,
    runtime_handle: JoinHandle<()>,
}

impl InProcessClientHandle {
    pub async fn request(&self, request: ClientRequest) -> IoResult<PendingClientRequestResponse> {
        self.client.request(request).await
    }

    pub fn notify(&self, notification: ClientNotification) -> IoResult<()> {
        self.client.notify(notification)
    }

    pub fn respond_to_server_request(
        &self,
        request_id: ProtocolRequestId,
        result: ProtocolResult,
    ) -> IoResult<()> {
        self.client.respond_to_server_request(request_id, result)
    }

    pub fn fail_server_request(
        &self,
        request_id: ProtocolRequestId,
        error: ProtocolJsonRpcError,
    ) -> IoResult<()> {
        self.client.fail_server_request(request_id, error)
    }

    pub async fn next_event(&mut self) -> Option<InProcessServerEvent> {
        self.event_rx.recv().await
    }

    pub async fn shutdown(self) -> IoResult<()> {
        let (done_tx, done_rx) = oneshot::channel();
        self.client
            .client_tx
            .send(InProcessClientMessage::Shutdown { done_tx })
            .await
            .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process runtime stopped"))?;
        timeout(SHUTDOWN_TIMEOUT, done_rx)
            .await
            .map_err(|_| IoError::new(ErrorKind::TimedOut, "in-process shutdown timed out"))?
            .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process shutdown dropped"))?;
        let _ = self.runtime_handle.await;
        Ok(())
    }

    pub fn sender(&self) -> InProcessClientSender {
        self.client.clone()
    }
}

pub async fn start(args: InProcessStartArgs) -> IoResult<InProcessClientHandle> {
    let initialize = args.initialize.clone();
    let handle = start_uninitialized(args);
    let request = ClientRequest::Initialize {
        request_id: ProtocolRequestId::Integer(0),
        params: initialize,
    };
    handle.request(request).await?.map_err(|error| {
        IoError::new(
            ErrorKind::InvalidData,
            format!("in-process initialize failed: {}", error.message),
        )
    })?;
    handle.notify(ClientNotification::Initialized)?;
    Ok(handle)
}

fn start_uninitialized(args: InProcessStartArgs) -> InProcessClientHandle {
    let channel_capacity = args.channel_capacity.max(1);
    let (client_tx, mut client_rx) = mpsc::channel::<InProcessClientMessage>(channel_capacity);
    let (event_tx, event_rx) = mpsc::channel::<InProcessServerEvent>(channel_capacity);
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<OutgoingMessage>();
    let outgoing = Arc::new(OutgoingMessageSender::new(outgoing_tx));
    let outbound_initialized = Arc::new(AtomicBool::new(false));
    let outbound_opted_out_notification_methods = Arc::new(RwLock::new(HashSet::new()));
    let (pending_event_tx, mut pending_event_rx) = mpsc::unbounded_channel::<OutgoingMessage>();
    let event_outgoing = Arc::clone(&outgoing);
    let event_opted_out_notification_methods =
        Arc::clone(&outbound_opted_out_notification_methods);
    tokio::spawn(async move {
        while let Some(outgoing_message) = pending_event_rx.recv().await {
            forward_outgoing_message(
                outgoing_message,
                &event_tx,
                &event_outgoing,
                &event_opted_out_notification_methods,
            )
            .await;
        }
    });
    let mut session = ConnectionSessionState::default();
    let config_warnings = args
        .config_warnings
        .into_iter()
        .filter_map(|warning| serde_json::to_value(warning).ok())
        .collect::<Vec<_>>();
    let mut processor = match args.conversation_manager {
        Some(conversation_manager) => MessageProcessor::new_with_conversation_manager(
            Arc::clone(&outgoing),
            args.code_linux_sandbox_exe,
            args.config,
            config_warnings,
            args.cli_overrides,
            None,
            conversation_manager.auth_manager(),
            conversation_manager,
        ),
        None => MessageProcessor::new_with_remote_control(
            Arc::clone(&outgoing),
            args.code_linux_sandbox_exe,
            args.config,
            config_warnings,
            args.cli_overrides,
            None,
            None,
        ),
    };

    let runtime_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                message = client_rx.recv() => {
                    let Some(message) = message else { break; };
                    match message {
                        InProcessClientMessage::Request { request, response_tx } => {
                            let response = process_client_request(
                                &mut processor,
                                *request,
                                &mut session,
                                &outbound_initialized,
                                &outbound_opted_out_notification_methods,
                                &mut outgoing_rx,
                                &pending_event_tx,
                                &outgoing,
                            ).await;
                            let _ = response_tx.send(response);
                        }
                        InProcessClientMessage::Notification { notification } => {
                            process_client_notification(&mut processor, notification);
                        }
                        InProcessClientMessage::ServerRequestResponse { request_id, result } => {
                            process_server_request_response(&mut processor, request_id, result).await;
                        }
                        InProcessClientMessage::ServerRequestError { request_id, error } => {
                            process_server_request_error(&mut processor, request_id, error).await;
                        }
                        InProcessClientMessage::Shutdown { done_tx } => {
                            processor.on_connection_closed(IN_PROCESS_CONNECTION_ID).await;
                            let _ = done_tx.send(());
                            break;
                        }
                    }
                }
                outgoing_message = outgoing_rx.recv() => {
                    let Some(outgoing_message) = outgoing_message else { break; };
                    queue_outgoing_message(
                        outgoing_message,
                        &pending_event_tx,
                        &outgoing,
                    ).await;
                }
            }
        }
        outgoing
            .clear_callbacks_for_connection(IN_PROCESS_CONNECTION_ID)
            .await;
    });

    InProcessClientHandle {
        client: InProcessClientSender { client_tx },
        event_rx,
        runtime_handle,
    }
}

async fn process_client_request(
    processor: &mut MessageProcessor,
    request: ClientRequest,
    session: &mut ConnectionSessionState,
    outbound_initialized: &AtomicBool,
    outbound_opted_out_notification_methods: &RwLock<HashSet<String>>,
    outgoing_rx: &mut mpsc::UnboundedReceiver<OutgoingMessage>,
    pending_event_tx: &mpsc::UnboundedSender<OutgoingMessage>,
    outgoing: &OutgoingMessageSender,
) -> PendingClientRequestResponse {
    let request = match client_request_to_mcp_request(request) {
        Ok(request) => request,
        Err(error) => return Err(error),
    };
    let response_id = request.id.clone();
    let was_initialized = session.initialized;
    processor
        .process_request(
            IN_PROCESS_CONNECTION_ID,
            request,
            session,
            outbound_initialized,
            outbound_opted_out_notification_methods,
        )
        .await;
    if !was_initialized && session.initialized {
        processor
            .send_initialize_notifications(IN_PROCESS_CONNECTION_ID)
            .await;
    }

    loop {
        match outgoing_rx.recv().await {
            Some(OutgoingMessage::Response(response)) if response.id == response_id => return Ok(response.result),
            Some(OutgoingMessage::Error(error)) if error.id == response_id => {
                return Err(convert_json(error.error));
            }
            Some(outgoing_message) => {
                queue_outgoing_message(outgoing_message, pending_event_tx, outgoing).await;
            }
            None => {
                return Err(ProtocolJsonRpcError {
                    code: INTERNAL_ERROR_CODE,
                    message: "in-process response channel closed".to_string(),
                    data: None,
                });
            }
        }
    }
}

fn process_client_notification(processor: &mut MessageProcessor, notification: ClientNotification) {
    match client_notification_to_mcp_notification(notification) {
        Ok(notification) => processor.process_notification(notification),
        Err(error) => warn!(error = %error.message, "failed to convert in-process notification"),
    }
}

async fn process_server_request_response(
    processor: &mut MessageProcessor,
    request_id: ProtocolRequestId,
    result: ProtocolResult,
) {
    let response = JSONRPCResponse {
        jsonrpc: mcp_types::JSONRPC_VERSION.to_string(),
        id: convert_json(request_id),
        result,
    };
    processor.process_response(IN_PROCESS_CONNECTION_ID, response).await;
}

async fn process_server_request_error(
    processor: &mut MessageProcessor,
    request_id: ProtocolRequestId,
    error: ProtocolJsonRpcError,
) {
    let error = mcp_types::JSONRPCError {
        jsonrpc: mcp_types::JSONRPC_VERSION.to_string(),
        id: convert_json(request_id),
        error: convert_json(error),
    };
    processor.process_error(IN_PROCESS_CONNECTION_ID, error).await;
}

async fn queue_outgoing_message(
    outgoing_message: OutgoingMessage,
    pending_event_tx: &mpsc::UnboundedSender<OutgoingMessage>,
    outgoing: &OutgoingMessageSender,
) {
    if let Err(error) = pending_event_tx.send(outgoing_message) {
        reject_undelivered_server_request(
            error.0,
            outgoing,
            "in-process event forwarder stopped",
        )
        .await;
    }
}

async fn forward_outgoing_message(
    outgoing_message: OutgoingMessage,
    event_tx: &mpsc::Sender<InProcessServerEvent>,
    outgoing: &OutgoingMessageSender,
    opted_out_notification_methods: &RwLock<HashSet<String>>,
) {
    let (event, server_request_id) = match outgoing_message {
        OutgoingMessage::Request(request) => {
            let request_id = request.id.clone();
            match convert_json::<_, code_app_server_protocol::JSONRPCRequest>(request).try_into() {
                Ok(request) => (
                    InProcessServerEvent::ServerRequest(request),
                    Some(request_id),
                ),
                Err(error) => {
                    warn!(error = %error, "failed to decode in-process server request");
                    reject_server_request(
                        outgoing,
                        request_id,
                        "failed to decode in-process server request",
                    )
                    .await;
                    return;
                }
            }
        }
        OutgoingMessage::Notification(notification) => {
            let opted_out = opted_out_notification_methods
                .read()
                .map(|methods| methods.contains(notification.method.as_str()))
                .unwrap_or_else(|_| {
                    warn!("failed to read in-process opted-out notifications");
                    false
                });
            if opted_out {
                return;
            }
            match convert_json::<_, code_app_server_protocol::JSONRPCNotification>(notification)
                .try_into()
            {
                Ok(notification) => (
                    InProcessServerEvent::ServerNotification(notification),
                    None,
                ),
                Err(error) => {
                    warn!(error = %error, "failed to decode in-process server notification");
                    return;
                }
            }
        }
        OutgoingMessage::Response(_) | OutgoingMessage::Error(_) => return,
    };

    if event_tx.send(event).await.is_err()
        && let Some(request_id) = server_request_id
    {
        reject_server_request(
            outgoing,
            request_id,
            "in-process server request consumer is closed",
        )
        .await;
    }
}

async fn reject_undelivered_server_request(
    outgoing_message: OutgoingMessage,
    outgoing: &OutgoingMessageSender,
    message: &str,
) {
    if let OutgoingMessage::Request(request) = outgoing_message {
        reject_server_request(outgoing, request.id, message).await;
    }
}

async fn reject_server_request(
    outgoing: &OutgoingMessageSender,
    request_id: mcp_types::RequestId,
    message: &str,
) {
    outgoing
        .notify_client_error_for_connection(
            Some(IN_PROCESS_CONNECTION_ID),
            request_id,
            mcp_types::JSONRPCErrorError {
                code: INTERNAL_ERROR_CODE,
                message: message.to_string(),
                data: None,
            },
        )
        .await;
}

fn client_request_to_mcp_request(request: ClientRequest) -> Result<JSONRPCRequest, ProtocolJsonRpcError> {
    let mut value = serde_json::to_value(&request).map_err(internal_conversion_error)?;
    let request_id = request.id().clone();
    let method = request.method();
    let params = value
        .as_object_mut()
        .and_then(|object| object.remove("params"));
    Ok(JSONRPCRequest {
        jsonrpc: mcp_types::JSONRPC_VERSION.to_string(),
        id: convert_json(request_id),
        method,
        params,
    })
}

fn client_notification_to_mcp_notification(
    notification: ClientNotification,
) -> Result<JSONRPCNotification, ProtocolJsonRpcError> {
    let value = serde_json::to_value(&notification).map_err(internal_conversion_error)?;
    convert_json_result(value).map_err(internal_conversion_error)
}

fn convert_json<T, U>(value: T) -> U
where
    T: serde::Serialize,
    U: DeserializeOwned,
{
    convert_json_result(value).expect("JSON-RPC compatibility conversion should not fail")
}

fn convert_json_result<T, U>(value: T) -> Result<U, serde_json::Error>
where
    T: serde::Serialize,
    U: DeserializeOwned,
{
    serde_json::from_value(serde_json::to_value(value)?)
}

fn internal_conversion_error(error: serde_json::Error) -> ProtocolJsonRpcError {
    ProtocolJsonRpcError {
        code: INVALID_REQUEST_ERROR_CODE,
        message: format!("invalid in-process JSON-RPC message: {error}"),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use code_app_server_protocol::ClientInfo;
    use code_app_server_protocol::InitializeCapabilities;
    use code_core::config::ConfigBuilder;
    use serde_json::json;

    fn config_warning_message(summary: &str) -> OutgoingMessage {
        OutgoingMessage::Notification(crate::outgoing_message::OutgoingNotification {
            method: "configWarning".to_string(),
            params: Some(json!({
                "summary": summary,
                "details": null,
                "path": null,
                "range": null,
            })),
        })
    }

    #[tokio::test]
    async fn full_event_queue_preserves_notifications_in_order() {
        let (event_tx, mut event_rx) = mpsc::channel(1);
        let (outgoing_tx, _outgoing_rx) = mpsc::unbounded_channel();
        let outgoing = Arc::new(OutgoingMessageSender::new(outgoing_tx));
        let opted_out_notification_methods = Arc::new(RwLock::new(HashSet::new()));
        forward_outgoing_message(
            config_warning_message("first"),
            &event_tx,
            &outgoing,
            &opted_out_notification_methods,
        )
        .await;

        let outgoing = Arc::clone(&outgoing);
        let opted_out_notification_methods = Arc::clone(&opted_out_notification_methods);
        let mut forward = tokio::spawn(async move {
            forward_outgoing_message(
                config_warning_message("second"),
                &event_tx,
                &outgoing,
                &opted_out_notification_methods,
            )
            .await;
        });

        assert!(
            timeout(Duration::from_millis(25), &mut forward)
                .await
                .is_err(),
            "lossless forwarding should wait for queue capacity"
        );
        let first = event_rx.recv().await.expect("first notification");
        let second = timeout(Duration::from_millis(100), event_rx.recv())
            .await
            .expect("second notification should not be dropped")
            .expect("event channel should remain open");
        forward.await.expect("forwarding task should finish");

        let summaries = [first, second].map(|event| match event {
            InProcessServerEvent::ServerNotification(ServerNotification::ConfigWarning(
                warning,
            )) => warning.summary,
            event => panic!("expected config warning notification, got {event:?}"),
        });
        assert_eq!(summaries, ["first", "second"]);
    }

    #[tokio::test]
    async fn closed_event_queue_rejects_server_request() {
        let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel();
        let outgoing = OutgoingMessageSender::new(outgoing_tx);
        let callback = outgoing
            .send_request(
                "account/chatgptAuthTokens/refresh",
                Some(json!({
                    "reason": "unauthorized",
                    "previousAccountId": null,
                })),
            )
            .await;
        let request = outgoing_rx.recv().await.expect("server request");
        let (event_tx, event_rx) = mpsc::channel(1);
        drop(event_rx);

        forward_outgoing_message(request, &event_tx, &outgoing, &RwLock::default()).await;

        assert!(
            timeout(Duration::from_millis(100), callback)
                .await
                .expect("request callback should be rejected promptly")
                .is_err(),
            "rejected server request should close its callback"
        );
    }

    #[tokio::test]
    async fn initialize_notification_opt_out_is_honored() {
        let code_home = tempfile::tempdir().expect("create code home").keep();
        let config = ConfigBuilder::new()
            .with_code_home(code_home)
            .load()
            .expect("load isolated config");
        let mut handle = start(InProcessStartArgs {
            code_linux_sandbox_exe: None,
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            config_warnings: vec![ConfigWarningNotification {
                summary: "hidden warning".to_string(),
                details: None,
                path: None,
                range: None,
            }],
            initialize: InitializeParams {
                client_info: ClientInfo {
                    name: "in-process-test".to_string(),
                    title: None,
                    version: "0".to_string(),
                },
                capabilities: Some(InitializeCapabilities {
                    experimental_api: false,
                    opt_out_notification_methods: Some(vec!["configWarning".to_string()]),
                }),
            },
            channel_capacity: 1,
            conversation_manager: None,
        })
        .await
        .expect("start in-process runtime");

        assert!(
            timeout(Duration::from_millis(50), handle.next_event())
                .await
                .is_err(),
            "opted-out config warning should not be emitted"
        );
        handle.shutdown().await.expect("shutdown in-process runtime");
    }
}
