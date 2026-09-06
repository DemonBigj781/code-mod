use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use code_app_server_protocol::JSONRPCMessage;
use code_app_server_protocol::JSONRPCRequest;
use code_app_server_protocol::RemoteControlConnectionStatus;
use code_app_server_protocol::RemoteControlDisableParams;
use code_app_server_protocol::RemoteControlDisableResponse;
use code_app_server_protocol::RemoteControlEnableParams;
use code_app_server_protocol::RemoteControlEnableResponse;
use code_app_server_protocol::RemoteControlPairingStartParams;
use code_app_server_protocol::RemoteControlPairingStartResponse;
use code_app_server_protocol::RemoteControlStatusChangedNotification;
use code_app_server_protocol::RequestId;
use serde::de::DeserializeOwned;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::time::Instant;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;

use crate::ControlClient;
use crate::ProbeInfo;
use crate::RemoteControlReadyStatus;
use crate::client;

const REQUEST_ID: RequestId = RequestId::Integer(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ControlSocketClient;

#[async_trait::async_trait]
impl ControlClient for ControlSocketClient {
    async fn probe(&self, socket_path: &Path) -> Result<ProbeInfo> {
        client::probe(socket_path).await
    }

    async fn enable(&self, socket_path: &Path) -> Result<RemoteControlReadyStatus> {
        enable(socket_path).await
    }

    async fn disable(&self, socket_path: &Path) -> Result<RemoteControlReadyStatus> {
        disable(socket_path).await
    }

    async fn start_pairing(
        &self,
        socket_path: &Path,
    ) -> Result<RemoteControlPairingStartResponse> {
        start_pairing(socket_path).await
    }
}

pub async fn enable(socket_path: &Path) -> Result<RemoteControlReadyStatus> {
    let mut websocket = initialized_connection(socket_path).await?;
    enable_connected(&mut websocket).await
}

pub async fn enable_with_connect_retry(
    socket_path: &Path,
    connect_timeout: Duration,
    connect_retry_delay: Duration,
) -> Result<RemoteControlReadyStatus> {
    let deadline = Instant::now() + connect_timeout;
    loop {
        match initialized_connection(socket_path).await {
            Ok(mut websocket) => return enable_connected(&mut websocket).await,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(connect_retry_delay).await;
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "timed out connecting to app-server control socket {}",
                        socket_path.display()
                    )
                });
            }
        }
    }
}

async fn enable_connected<S>(
    websocket: &mut WebSocketStream<S>,
) -> Result<RemoteControlReadyStatus>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let response: RemoteControlEnableResponse = request(
        websocket,
        "remoteControl/enable",
        Some(serde_json::to_value(RemoteControlEnableParams {
            ephemeral: true,
        })?),
    )
    .await?;
    let mut ready = RemoteControlReadyStatus::from(response);
    if ready.status == RemoteControlConnectionStatus::Connecting {
        ready = wait_for_terminal_status(websocket).await?;
    }
    websocket.close(None).await.ok();
    Ok(ready)
}

pub async fn disable(socket_path: &Path) -> Result<RemoteControlReadyStatus> {
    let mut websocket = initialized_connection(socket_path).await?;
    let response: RemoteControlDisableResponse = request(
        &mut websocket,
        "remoteControl/disable",
        Some(serde_json::to_value(RemoteControlDisableParams {
            ephemeral: true,
        })?),
    )
    .await?;
    websocket.close(None).await.ok();
    Ok(response.into())
}

pub async fn start_pairing(socket_path: &Path) -> Result<RemoteControlPairingStartResponse> {
    let mut websocket = initialized_connection(socket_path).await?;
    let response = request(
        &mut websocket,
        "remoteControl/pairing/start",
        Some(serde_json::to_value(RemoteControlPairingStartParams {
            manual_code: true,
        })?),
    )
    .await?;
    websocket.close(None).await.ok();
    Ok(response)
}

async fn initialized_connection(
    socket_path: &Path,
) -> Result<WebSocketStream<tokio::net::UnixStream>> {
    let mut websocket = client::connect(socket_path).await?;
    client::initialize(&mut websocket, true).await?;
    client::send_initialized(&mut websocket).await?;
    Ok(websocket)
}

async fn request<S, T>(
    websocket: &mut WebSocketStream<S>,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<T>
where
    S: AsyncRead + AsyncWrite + Unpin,
    T: DeserializeOwned,
{
    client::send_message(
        websocket,
        &JSONRPCMessage::Request(JSONRPCRequest {
            id: REQUEST_ID.clone(),
            method: method.to_string(),
            params,
            trace: None,
        }),
    )
    .await
    .with_context(|| format!("failed to send {method} request"))?;
    loop {
        let message = timeout(REQUEST_TIMEOUT, client::read_message(websocket))
            .await
            .with_context(|| format!("timed out waiting for {method} response"))??;
        match message {
            JSONRPCMessage::Response(response) if response.id == REQUEST_ID => {
                return serde_json::from_value(response.result)
                    .with_context(|| format!("failed to parse {method} response"));
            }
            JSONRPCMessage::Error(error) if error.id == REQUEST_ID => {
                return Err(anyhow!(
                    "{method} failed with {}: {}",
                    error.error.code,
                    error.error.message
                ));
            }
            _ => {}
        }
    }
}

async fn wait_for_terminal_status<S>(
    websocket: &mut WebSocketStream<S>,
) -> Result<RemoteControlReadyStatus>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let message = timeout(remaining, client::read_message(websocket))
            .await
            .context("timed out waiting for remote-control readiness")??;
        let JSONRPCMessage::Notification(notification) = message else {
            continue;
        };
        if notification.method != "remoteControl/status/changed" {
            continue;
        }
        let Some(params) = notification.params else {
            continue;
        };
        let status: RemoteControlStatusChangedNotification = serde_json::from_value(params)
            .context("failed to parse remote-control status notification")?;
        let latest: RemoteControlReadyStatus = status.into();
        if latest.status != RemoteControlConnectionStatus::Connecting {
            return Ok(latest);
        }
    }
    Err(anyhow!("timed out waiting for remote-control readiness"))
}

impl From<RemoteControlEnableResponse> for RemoteControlReadyStatus {
    fn from(value: RemoteControlEnableResponse) -> Self {
        Self {
            status: value.status,
            server_name: value.server_name,
            installation_id: value.installation_id,
            environment_id: value.environment_id,
        }
    }
}

impl From<RemoteControlDisableResponse> for RemoteControlReadyStatus {
    fn from(value: RemoteControlDisableResponse) -> Self {
        Self {
            status: value.status,
            server_name: value.server_name,
            installation_id: value.installation_id,
            environment_id: value.environment_id,
        }
    }
}

impl From<RemoteControlStatusChangedNotification> for RemoteControlReadyStatus {
    fn from(value: RemoteControlStatusChangedNotification) -> Self {
        Self {
            status: value.status,
            server_name: value.server_name,
            installation_id: value.installation_id,
            environment_id: value.environment_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use anyhow::Result;
    use code_app_server_protocol::JSONRPCMessage;
    use code_app_server_protocol::JSONRPCNotification;
    use code_app_server_protocol::JSONRPCResponse;
    use code_app_server_protocol::RemoteControlConnectionStatus;
    use code_app_server_protocol::RemoteControlDisableResponse;
    use code_app_server_protocol::RemoteControlEnableResponse;
    use code_app_server_protocol::RemoteControlPairingStartResponse;
    use pretty_assertions::assert_eq;
    use tokio::net::UnixListener;
    use tokio::net::UnixStream;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::accept_async;

    use super::REQUEST_ID;
    use super::disable;
    use super::enable;
    use super::enable_with_connect_retry;
    use super::start_pairing;
    use crate::client;

    const INSTALLATION_ID: &str = "11111111-1111-4111-8111-111111111111";

    #[tokio::test]
    async fn enable_negotiates_experimental_api_and_waits_for_connected_status() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let socket_path = temp.path().join("enable.sock");
        let listener = UnixListener::bind(&socket_path)?;
        make_socket_private(&socket_path).await?;
        let server = tokio::spawn(async move {
            let mut websocket = accept_initialized(listener).await?;
            let request = read_request(&mut websocket, "remoteControl/enable").await?;
            assert_eq!(request.params, Some(serde_json::json!({ "ephemeral": true })));
            send_response(
                &mut websocket,
                RemoteControlEnableResponse {
                    status: RemoteControlConnectionStatus::Connecting,
                    server_name: "test-server".to_string(),
                    installation_id: INSTALLATION_ID.to_string(),
                    environment_id: None,
                },
            )
            .await?;
            client::send_message(
                &mut websocket,
                &JSONRPCMessage::Notification(JSONRPCNotification {
                    method: "remoteControl/status/changed".to_string(),
                    params: Some(serde_json::json!({
                        "status": "connected",
                        "serverName": "test-server",
                        "installationId": INSTALLATION_ID,
                        "environmentId": "env-test"
                    })),
                }),
            )
            .await?;
            Ok::<_, anyhow::Error>(())
        });

        let ready = enable(&socket_path).await?;
        assert_eq!(ready.status, RemoteControlConnectionStatus::Connected);
        assert_eq!(ready.environment_id.as_deref(), Some("env-test"));
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn disable_uses_ephemeral_typed_rpc() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let socket_path = temp.path().join("disable.sock");
        let listener = UnixListener::bind(&socket_path)?;
        make_socket_private(&socket_path).await?;
        let server = tokio::spawn(async move {
            let mut websocket = accept_initialized(listener).await?;
            let request = read_request(&mut websocket, "remoteControl/disable").await?;
            assert_eq!(request.params, Some(serde_json::json!({ "ephemeral": true })));
            send_response(
                &mut websocket,
                RemoteControlDisableResponse {
                    status: RemoteControlConnectionStatus::Disabled,
                    server_name: "test-server".to_string(),
                    installation_id: INSTALLATION_ID.to_string(),
                    environment_id: None,
                },
            )
            .await?;
            Ok::<_, anyhow::Error>(())
        });

        let ready = disable(&socket_path).await?;
        assert_eq!(ready.status, RemoteControlConnectionStatus::Disabled);
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn pairing_requests_a_manual_code() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let socket_path = temp.path().join("pair.sock");
        let listener = UnixListener::bind(&socket_path)?;
        make_socket_private(&socket_path).await?;
        let server = tokio::spawn(async move {
            let mut websocket = accept_initialized(listener).await?;
            let request = read_request(&mut websocket, "remoteControl/pairing/start").await?;
            assert_eq!(request.params, Some(serde_json::json!({ "manualCode": true })));
            send_response(
                &mut websocket,
                RemoteControlPairingStartResponse {
                    pairing_code: "pair-code".to_string(),
                    manual_pairing_code: Some("manual-code".to_string()),
                    environment_id: "env-test".to_string(),
                    expires_at: 1234,
                },
            )
            .await?;
            Ok::<_, anyhow::Error>(())
        });

        let pairing = start_pairing(&socket_path).await?;
        assert_eq!(pairing.manual_pairing_code.as_deref(), Some("manual-code"));
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn enable_retries_until_the_private_socket_appears() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let socket_path = temp.path().join("delayed.sock");
        let server_path = socket_path.clone();
        let server = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let listener = UnixListener::bind(&server_path)?;
            make_socket_private(&server_path).await?;
            let mut websocket = accept_initialized(listener).await?;
            read_request(&mut websocket, "remoteControl/enable").await?;
            send_response(
                &mut websocket,
                RemoteControlEnableResponse {
                    status: RemoteControlConnectionStatus::Connected,
                    server_name: "test-server".to_string(),
                    installation_id: INSTALLATION_ID.to_string(),
                    environment_id: Some("env-test".to_string()),
                },
            )
            .await?;
            Ok::<_, anyhow::Error>(())
        });

        let ready = enable_with_connect_retry(
            &socket_path,
            Duration::from_secs(1),
            Duration::from_millis(5),
        )
        .await?;
        assert_eq!(ready.status, RemoteControlConnectionStatus::Connected);
        server.await??;
        Ok(())
    }

    async fn accept_initialized(
        listener: UnixListener,
    ) -> Result<WebSocketStream<UnixStream>> {
        let (stream, _) = listener.accept().await?;
        let mut websocket = accept_async(stream).await?;
        let initialize = client::read_message(&mut websocket).await?;
        let JSONRPCMessage::Request(initialize) = initialize else {
            anyhow::bail!("expected initialize request");
        };
        assert_eq!(initialize.method, "initialize");
        assert_eq!(
            initialize.params.as_ref().and_then(|params| {
                params
                    .get("capabilities")
                    .and_then(|capabilities| capabilities.get("experimentalApi"))
            }),
            Some(&serde_json::Value::Bool(true))
        );
        client::send_message(
            &mut websocket,
            &JSONRPCMessage::Response(JSONRPCResponse {
                id: initialize.id,
                result: serde_json::json!({ "userAgent": "code_app_server_daemon/1.2.3" }),
            }),
        )
        .await?;
        let initialized = client::read_message(&mut websocket).await?;
        let JSONRPCMessage::Notification(initialized) = initialized else {
            anyhow::bail!("expected initialized notification");
        };
        assert_eq!(initialized.method, "initialized");
        Ok(websocket)
    }

    async fn read_request(
        websocket: &mut WebSocketStream<UnixStream>,
        method: &str,
    ) -> Result<code_app_server_protocol::JSONRPCRequest> {
        let message = client::read_message(websocket).await?;
        let JSONRPCMessage::Request(request) = message else {
            anyhow::bail!("expected {method} request");
        };
        assert_eq!(request.id, REQUEST_ID);
        assert_eq!(request.method, method);
        Ok(request)
    }

    async fn send_response<T: serde::Serialize>(
        websocket: &mut WebSocketStream<UnixStream>,
        response: T,
    ) -> Result<()> {
        client::send_message(
            websocket,
            &JSONRPCMessage::Response(JSONRPCResponse {
                id: REQUEST_ID,
                result: serde_json::to_value(response)?,
            }),
        )
        .await
    }

    async fn make_socket_private(path: &std::path::Path) -> Result<()> {
        tokio::fs::set_permissions(
            path,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .await?;
        Ok(())
    }
}
