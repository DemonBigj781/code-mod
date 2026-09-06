use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use code_app_server_protocol::ClientInfo;
use code_app_server_protocol::InitializeCapabilities;
use code_app_server_protocol::InitializeParams;
use code_app_server_protocol::InitializeResponse;
use code_app_server_protocol::JSONRPCMessage;
use code_app_server_protocol::JSONRPCNotification;
use code_app_server_protocol::JSONRPCRequest;
use code_app_server_protocol::RequestId;
use futures::SinkExt;
use futures::StreamExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::net::UnixStream;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::Message;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const INITIALIZE_REQUEST_ID: RequestId = RequestId::Integer(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeInfo {
    pub app_server_version: String,
}

pub async fn probe(socket_path: &Path) -> Result<ProbeInfo> {
    timeout(RESPONSE_TIMEOUT, probe_inner(socket_path))
        .await
        .with_context(|| {
            format!(
                "timed out probing app-server control socket {}",
                socket_path.display()
            )
        })?
}

async fn probe_inner(socket_path: &Path) -> Result<ProbeInfo> {
    let mut websocket = connect(socket_path).await?;
    let response = initialize(&mut websocket, false).await?;
    send_initialized(&mut websocket).await?;
    websocket.close(None).await.ok();
    Ok(ProbeInfo {
        app_server_version: parse_version_from_user_agent(&response.user_agent)?,
    })
}

pub async fn connect(socket_path: &Path) -> Result<WebSocketStream<UnixStream>> {
    validate_control_socket(socket_path).await?;
    let stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("failed to connect to {}", socket_path.display()))?;
    let (websocket, _) = client_async("ws://localhost/", stream)
        .await
        .with_context(|| format!("failed to upgrade {}", socket_path.display()))?;
    Ok(websocket)
}

async fn validate_control_socket(socket_path: &Path) -> Result<()> {
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    let metadata = tokio::fs::symlink_metadata(socket_path)
        .await
        .with_context(|| format!("failed to inspect {}", socket_path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(anyhow!(
            "app-server control path is not a Unix socket: {}",
            socket_path.display()
        ));
    }
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err(anyhow!(
            "app-server control socket is not owned by the current user"
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(anyhow!(
            "app-server control socket permissions are not private: {}",
            socket_path.display()
        ));
    }
    Ok(())
}

pub async fn initialize<S>(
    websocket: &mut WebSocketStream<S>,
    experimental_api: bool,
) -> Result<InitializeResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let message = JSONRPCMessage::Request(JSONRPCRequest {
        id: INITIALIZE_REQUEST_ID.clone(),
        method: "initialize".to_string(),
        params: Some(serde_json::to_value(InitializeParams {
            client_info: ClientInfo {
                name: "code_app_server_daemon".to_string(),
                title: Some("Code App Server Daemon".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            capabilities: experimental_api.then_some(InitializeCapabilities {
                experimental_api: true,
                ..Default::default()
            }),
        })?),
        trace: None,
    });
    send_message(websocket, &message).await?;
    loop {
        let message = timeout(RESPONSE_TIMEOUT, read_message(websocket))
            .await
            .context("timed out waiting for initialize response")??;
        match message {
            JSONRPCMessage::Response(response) if response.id == INITIALIZE_REQUEST_ID => {
                return serde_json::from_value(response.result)
                    .context("failed to parse initialize response");
            }
            JSONRPCMessage::Error(error) if error.id == INITIALIZE_REQUEST_ID => {
                return Err(anyhow!(
                    "initialize failed with {}: {}",
                    error.error.code,
                    error.error.message
                ));
            }
            _ => {}
        }
    }
}

pub async fn send_initialized<S>(websocket: &mut WebSocketStream<S>) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_message(
        websocket,
        &JSONRPCMessage::Notification(JSONRPCNotification {
            method: "initialized".to_string(),
            params: None,
        }),
    )
    .await
    .context("failed to send initialized notification")
}

pub async fn send_message<S>(
    websocket: &mut WebSocketStream<S>,
    message: &JSONRPCMessage,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    websocket
        .send(Message::Text(serde_json::to_string(message)?.into()))
        .await?;
    Ok(())
}

pub async fn read_message<S>(websocket: &mut WebSocketStream<S>) -> Result<JSONRPCMessage>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let frame = websocket
            .next()
            .await
            .ok_or_else(|| anyhow!("app-server closed the control socket"))??;
        if let Message::Text(payload) = frame {
            return serde_json::from_str(&payload)
                .context("failed to parse app-server JSON-RPC message");
        }
    }
}

fn parse_version_from_user_agent(user_agent: &str) -> Result<String> {
    let (_, rest) = user_agent
        .split_once('/')
        .ok_or_else(|| anyhow!("app-server user-agent omitted version separator"))?;
    rest.split_whitespace()
        .next()
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("app-server user-agent omitted version"))
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use code_app_server_protocol::JSONRPCMessage;
    use code_app_server_protocol::JSONRPCResponse;
    use pretty_assertions::assert_eq;
    use tokio::net::UnixListener;
    use tokio_tungstenite::accept_async;

    use super::INITIALIZE_REQUEST_ID;
    use super::probe;
    use super::read_message;
    use super::send_message;

    #[tokio::test]
    async fn probe_uses_initialize_and_initialized_over_unix_websocket() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let socket_path = temp.path().join("control.sock");
        let listener = UnixListener::bind(&socket_path)?;
        tokio::fs::set_permissions(
            &socket_path,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .await?;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            let mut websocket = accept_async(stream).await?;
            let initialize = read_message(&mut websocket).await?;
            let JSONRPCMessage::Request(initialize) = initialize else {
                anyhow::bail!("expected initialize request");
            };
            assert_eq!(initialize.id, INITIALIZE_REQUEST_ID);
            assert_eq!(initialize.method, "initialize");
            assert_eq!(
                initialize
                    .params
                    .as_ref()
                    .and_then(|params| params.get("capabilities")),
                None
            );
            send_message(
                &mut websocket,
                &JSONRPCMessage::Response(JSONRPCResponse {
                    id: INITIALIZE_REQUEST_ID,
                    result: serde_json::json!({
                        "userAgent": "code_app_server_daemon/1.2.3 (Linux; x86_64)"
                    }),
                }),
            )
            .await?;
            let initialized = read_message(&mut websocket).await?;
            let JSONRPCMessage::Notification(initialized) = initialized else {
                anyhow::bail!("expected initialized notification");
            };
            assert_eq!(initialized.method, "initialized");
            Ok::<_, anyhow::Error>(())
        });

        let info = probe(&socket_path).await?;
        assert_eq!(info.app_server_version, "1.2.3");
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn probe_rejects_a_control_socket_with_group_or_other_access() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let socket_path = temp.path().join("public-control.sock");
        let _listener = UnixListener::bind(&socket_path)?;
        tokio::fs::set_permissions(
            &socket_path,
            std::os::unix::fs::PermissionsExt::from_mode(0o666),
        )
        .await?;

        let error = probe(&socket_path)
            .await
            .expect_err("public control socket must be rejected");
        assert!(error.to_string().contains("permissions are not private"));
        Ok(())
    }
}
