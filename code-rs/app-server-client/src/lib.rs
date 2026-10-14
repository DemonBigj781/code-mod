//! Shared in-process app-server client facade for CLI surfaces.
//!
//! This crate is the current-fork counterpart of upstream Codex's
//! `codex-app-server-client`: callers talk to app-server protocol requests and
//! notifications while the embedded runtime remains behind a bounded command
//! worker. The consumer-facing event queue is intentionally unbounded so an
//! unread event cannot block request resolution; events remain ordered and
//! lossless.

use std::error::Error;
use std::fmt;
use std::io::Error as IoError;
use std::io::ErrorKind;
use std::io::Result as IoResult;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use code_app_server::in_process::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
pub use code_app_server::in_process::InProcessServerEvent;
use code_app_server::in_process::InProcessStartArgs;
use code_app_server_protocol::ClientInfo;
use code_app_server_protocol::ClientNotification;
use code_app_server_protocol::ClientRequest;
use code_app_server_protocol::ConfigWarningNotification;
use code_app_server_protocol::InitializeCapabilities;
use code_app_server_protocol::InitializeParams;
use code_app_server_protocol::JSONRPCErrorError;
use code_app_server_protocol::RequestId;
use code_app_server_protocol::Result as JsonRpcResult;
use code_app_server_protocol::ServerNotification;
use code_app_server_protocol::ServerRequest;
use code_core::config::Config;
use code_core::ConversationManager;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::timeout;
use toml::Value as TomlValue;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub type RequestResult = std::result::Result<JsonRpcResult, JSONRPCErrorError>;

#[derive(Debug, Clone)]
pub enum AppServerEvent {
    ServerNotification(ServerNotification),
    ServerRequest(ServerRequest),
    Disconnected { message: String },
}

impl From<InProcessServerEvent> for AppServerEvent {
    fn from(value: InProcessServerEvent) -> Self {
        match value {
            InProcessServerEvent::ServerNotification(notification) => {
                Self::ServerNotification(notification)
            }
            InProcessServerEvent::ServerRequest(request) => Self::ServerRequest(request),
        }
    }
}

#[derive(Debug)]
pub enum TypedRequestError {
    Transport {
        method: String,
        source: IoError,
    },
    Server {
        method: String,
        source: JSONRPCErrorError,
    },
    Deserialize {
        method: String,
        source: serde_json::Error,
    },
}

impl fmt::Display for TypedRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { method, source } => write!(f, "{method} transport error: {source}"),
            Self::Server { method, source } => write!(f, "{method} failed: {}", source.message),
            Self::Deserialize { method, source } => {
                write!(f, "{method} response decode error: {source}")
            }
        }
    }
}

impl Error for TypedRequestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport { source, .. } => Some(source),
            Self::Server { .. } => None,
            Self::Deserialize { source, .. } => Some(source),
        }
    }
}

#[derive(Clone)]
pub struct InProcessClientStartArgs {
    pub code_linux_sandbox_exe: Option<PathBuf>,
    pub config: Arc<Config>,
    pub cli_overrides: Vec<(String, TomlValue)>,
    pub config_warnings: Vec<ConfigWarningNotification>,
    pub client_name: String,
    pub client_version: String,
    pub experimental_api: bool,
    pub opt_out_notification_methods: Vec<String>,
    pub channel_capacity: usize,
    pub conversation_manager: Option<Arc<ConversationManager>>,
}

impl InProcessClientStartArgs {
    pub fn initialize_params(&self) -> InitializeParams {
        let capabilities = InitializeCapabilities {
            experimental_api: self.experimental_api,
            opt_out_notification_methods: if self.opt_out_notification_methods.is_empty() {
                None
            } else {
                Some(self.opt_out_notification_methods.clone())
            },
        };

        InitializeParams {
            client_info: ClientInfo {
                name: self.client_name.clone(),
                title: None,
                version: self.client_version.clone(),
            },
            capabilities: Some(capabilities),
        }
    }

    fn into_runtime_start_args(self) -> InProcessStartArgs {
        let initialize = self.initialize_params();
        InProcessStartArgs {
            code_linux_sandbox_exe: self.code_linux_sandbox_exe,
            config: self.config,
            cli_overrides: self.cli_overrides,
            config_warnings: self.config_warnings,
            initialize,
            channel_capacity: self.channel_capacity,
            conversation_manager: self.conversation_manager,
        }
    }
}

enum ClientCommand {
    Request {
        request: Box<ClientRequest>,
        response_tx: oneshot::Sender<IoResult<RequestResult>>,
    },
    Notify {
        notification: ClientNotification,
        response_tx: oneshot::Sender<IoResult<()>>,
    },
    ResolveServerRequest {
        request_id: RequestId,
        result: JsonRpcResult,
        response_tx: oneshot::Sender<IoResult<()>>,
    },
    RejectServerRequest {
        request_id: RequestId,
        error: JSONRPCErrorError,
        response_tx: oneshot::Sender<IoResult<()>>,
    },
    Shutdown {
        response_tx: oneshot::Sender<IoResult<()>>,
    },
}

pub struct InProcessAppServerClient {
    command_tx: mpsc::Sender<ClientCommand>,
    event_rx: mpsc::UnboundedReceiver<InProcessServerEvent>,
    worker_handle: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
pub struct InProcessAppServerRequestHandle {
    command_tx: mpsc::Sender<ClientCommand>,
}

#[derive(Clone)]
pub enum AppServerRequestHandle {
    InProcess(InProcessAppServerRequestHandle),
}

pub enum AppServerClient {
    InProcess(InProcessAppServerClient),
}

impl InProcessAppServerClient {
    pub async fn start(args: InProcessClientStartArgs) -> IoResult<Self> {
        let channel_capacity = args.channel_capacity.max(1);
        let mut runtime = code_app_server::in_process::start(args.into_runtime_start_args()).await?;
        let runtime_sender = runtime.sender();
        let (command_tx, mut command_rx) = mpsc::channel::<ClientCommand>(channel_capacity);
        let (event_tx, event_rx) = mpsc::unbounded_channel::<InProcessServerEvent>();

        let worker_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    command = command_rx.recv() => {
                        match command {
                            Some(ClientCommand::Request { request, response_tx }) => {
                                let runtime_sender = runtime_sender.clone();
                                tokio::spawn(async move {
                                    let result = runtime_sender.request(*request).await;
                                    let _ = response_tx.send(result);
                                });
                            }
                            Some(ClientCommand::Notify { notification, response_tx }) => {
                                let _ = response_tx.send(runtime_sender.notify(notification));
                            }
                            Some(ClientCommand::ResolveServerRequest { request_id, result, response_tx }) => {
                                let _ = response_tx.send(runtime_sender.respond_to_server_request(request_id, result));
                            }
                            Some(ClientCommand::RejectServerRequest { request_id, error, response_tx }) => {
                                let _ = response_tx.send(runtime_sender.fail_server_request(request_id, error));
                            }
                            Some(ClientCommand::Shutdown { response_tx }) => {
                                let _ = response_tx.send(runtime.shutdown().await);
                                break;
                            }
                            None => {
                                let _ = runtime.shutdown().await;
                                break;
                            }
                        }
                    }
                    event = runtime.next_event() => {
                        let Some(event) = event else { break; };
                        if event_tx.send(event).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            command_tx,
            event_rx,
            worker_handle,
        })
    }

    pub fn request_handle(&self) -> InProcessAppServerRequestHandle {
        InProcessAppServerRequestHandle {
            command_tx: self.command_tx.clone(),
        }
    }

    pub async fn request(&self, request: ClientRequest) -> IoResult<RequestResult> {
        send_request(&self.command_tx, request).await
    }

    pub async fn request_typed<T>(&self, request: ClientRequest) -> Result<T, TypedRequestError>
    where
        T: DeserializeOwned,
    {
        request_typed_with_sender(&self.command_tx, request).await
    }

    pub async fn notify(&self, notification: ClientNotification) -> IoResult<()> {
        send_notify(&self.command_tx, notification).await
    }

    pub async fn respond_to_server_request(
        &self,
        request_id: RequestId,
        result: JsonRpcResult,
    ) -> IoResult<()> {
        send_resolve_server_request(&self.command_tx, request_id, result).await
    }

    pub async fn fail_server_request(
        &self,
        request_id: RequestId,
        error: JSONRPCErrorError,
    ) -> IoResult<()> {
        send_reject_server_request(&self.command_tx, request_id, error).await
    }

    pub async fn next_event(&mut self) -> Option<InProcessServerEvent> {
        self.event_rx.recv().await
    }

    pub async fn shutdown(self) -> IoResult<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(ClientCommand::Shutdown { response_tx })
            .await
            .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process worker stopped"))?;
        timeout(SHUTDOWN_TIMEOUT, response_rx)
            .await
            .map_err(|_| IoError::new(ErrorKind::TimedOut, "in-process client shutdown timed out"))?
            .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process shutdown response dropped"))??;
        let _ = self.worker_handle.await;
        Ok(())
    }
}

impl InProcessAppServerRequestHandle {
    pub async fn request(&self, request: ClientRequest) -> IoResult<RequestResult> {
        send_request(&self.command_tx, request).await
    }

    pub async fn request_typed<T>(&self, request: ClientRequest) -> Result<T, TypedRequestError>
    where
        T: DeserializeOwned,
    {
        request_typed_with_sender(&self.command_tx, request).await
    }

    pub async fn notify(&self, notification: ClientNotification) -> IoResult<()> {
        send_notify(&self.command_tx, notification).await
    }
}

impl AppServerRequestHandle {
    pub async fn request(&self, request: ClientRequest) -> IoResult<RequestResult> {
        match self {
            Self::InProcess(handle) => handle.request(request).await,
        }
    }

    pub async fn request_typed<T>(&self, request: ClientRequest) -> Result<T, TypedRequestError>
    where
        T: DeserializeOwned,
    {
        match self {
            Self::InProcess(handle) => handle.request_typed(request).await,
        }
    }

    pub async fn notify(&self, notification: ClientNotification) -> IoResult<()> {
        match self {
            Self::InProcess(handle) => handle.notify(notification).await,
        }
    }
}

impl AppServerClient {
    pub fn request_handle(&self) -> AppServerRequestHandle {
        match self {
            Self::InProcess(client) => {
                AppServerRequestHandle::InProcess(client.request_handle())
            }
        }
    }

    pub async fn next_event(&mut self) -> Option<AppServerEvent> {
        match self {
            Self::InProcess(client) => client.next_event().await.map(Into::into),
        }
    }
}

async fn send_request(
    command_tx: &mpsc::Sender<ClientCommand>,
    request: ClientRequest,
) -> IoResult<RequestResult> {
    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(ClientCommand::Request {
            request: Box::new(request),
            response_tx,
        })
        .await
        .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process worker stopped"))?;
    response_rx
        .await
        .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process response dropped"))?
}

async fn send_notify(
    command_tx: &mpsc::Sender<ClientCommand>,
    notification: ClientNotification,
) -> IoResult<()> {
    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(ClientCommand::Notify {
            notification,
            response_tx,
        })
        .await
        .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process worker stopped"))?;
    response_rx
        .await
        .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process notify response dropped"))?
}

async fn send_resolve_server_request(
    command_tx: &mpsc::Sender<ClientCommand>,
    request_id: RequestId,
    result: JsonRpcResult,
) -> IoResult<()> {
    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(ClientCommand::ResolveServerRequest {
            request_id,
            result,
            response_tx,
        })
        .await
        .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process worker stopped"))?;
    response_rx
        .await
        .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process resolve response dropped"))?
}

async fn send_reject_server_request(
    command_tx: &mpsc::Sender<ClientCommand>,
    request_id: RequestId,
    error: JSONRPCErrorError,
) -> IoResult<()> {
    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(ClientCommand::RejectServerRequest {
            request_id,
            error,
            response_tx,
        })
        .await
        .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process worker stopped"))?;
    response_rx
        .await
        .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "in-process reject response dropped"))?
}

async fn request_typed_with_sender<T>(
    command_tx: &mpsc::Sender<ClientCommand>,
    request: ClientRequest,
) -> Result<T, TypedRequestError>
where
    T: DeserializeOwned,
{
    let method = request.method();
    let value = send_request(command_tx, request)
        .await
        .map_err(|source| TypedRequestError::Transport {
            method: method.clone(),
            source,
        })?
        .map_err(|source| TypedRequestError::Server {
            method: method.clone(),
            source,
        })?;
    serde_json::from_value(value).map_err(|source| TypedRequestError::Deserialize { method, source })
}

pub mod legacy_core {
    pub mod config {
        pub use code_core::config::*;
    }

    pub mod otel_init {
        pub use code_core::otel_init::*;
    }

    pub mod personality_migration {
        pub use code_core::personality_migration::*;
    }

    pub mod plugins {
        pub use code_core::plugins::PluginsManager;
    }

    pub mod review_format {
        pub use code_core::review_format::*;
    }

    pub mod util {
        pub use code_core::util::*;
    }

    pub mod windows_sandbox {
        pub use code_core::windows_sandbox::*;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use code_app_server_protocol::ConfigWarningNotification;
    use code_core::config::ConfigBuilder;
    use tokio::time::Duration;

    fn test_start_args(summary: &str) -> InProcessClientStartArgs {
        let code_home = tempfile::tempdir().expect("create code home").keep();
        let config = ConfigBuilder::new()
            .with_code_home(code_home)
            .load()
            .expect("load isolated config");
        InProcessClientStartArgs {
            code_linux_sandbox_exe: None,
            config: Arc::new(config),
            cli_overrides: Vec::new(),
            config_warnings: vec![ConfigWarningNotification {
                summary: summary.to_owned(),
                details: None,
                path: None,
                range: None,
            }],
            client_name: "code-app-server-client-test".to_owned(),
            client_version: "0".to_owned(),
            experimental_api: false,
            opt_out_notification_methods: Vec::new(),
            channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
            conversation_manager: None,
        }
    }

    #[tokio::test]
    async fn in_process_facade_receives_server_notifications() {
        let mut client = InProcessAppServerClient::start(test_start_args("facade warning"))
            .await
            .expect("start app-server client facade");

        let event = tokio::time::timeout(Duration::from_secs(2), client.next_event())
            .await
            .expect("server notification should arrive")
            .expect("event stream should remain open");

        match AppServerEvent::from(event) {
            AppServerEvent::ServerNotification(ServerNotification::ConfigWarning(warning)) => {
                assert_eq!(warning.summary, "facade warning");
            }
            event => panic!("expected config warning notification, got {event:?}"),
        }

        client.shutdown().await.expect("shutdown facade");
    }
}
