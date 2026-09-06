use super::client_tracker::ClientTracker;
use super::enroll::RemoteControlEnrollment;
use super::host_device::HostDevice;
use super::protocol::ClientEnvelope;
use super::protocol::ClientId;
use super::protocol::ServerEnvelope;
use super::protocol::StreamId;
use super::segment::split_server_envelope_for_transport;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::TransportEvent;
use base64::Engine;
use futures::SinkExt;
use futures::StreamExt;
use std::collections::HashMap;
use std::io;
use std::io::ErrorKind;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio::time::MissedTickBehavior;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_util::sync::CancellationToken;
use tracing::warn;

pub(crate) const REMOTE_CONTROL_PROTOCOL_VERSION: &str = "3";
const REMOTE_CONTROL_WEBSOCKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const REMOTE_CONTROL_WEBSOCKET_PING_INTERVAL: Duration = Duration::from_secs(10);
const REMOTE_CONTROL_WEBSOCKET_PONG_TIMEOUT: Duration = Duration::from_secs(60);
const REMOTE_CONTROL_CLIENT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) async fn run_remote_control_websocket_once(
    enrollment: RemoteControlEnrollment,
    installation_id: String,
    host: HostDevice,
    transport_event_tx: mpsc::Sender<TransportEvent>,
    shutdown: CancellationToken,
) -> io::Result<()> {
    let request = build_websocket_request(&enrollment, &installation_id, &host)?;
    let (websocket, _) = tokio::time::timeout(
        REMOTE_CONTROL_WEBSOCKET_CONNECT_TIMEOUT,
        connect_async(request),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            ErrorKind::TimedOut,
            "timed out connecting to remote control websocket",
        )
    })?
    .map_err(map_websocket_error)?;
    run_connected_websocket(websocket, transport_event_tx, shutdown).await
}

fn build_websocket_request(
    enrollment: &RemoteControlEnrollment,
    installation_id: &str,
    host: &HostDevice,
) -> io::Result<tungstenite::http::Request<()>> {
    let mut request = enrollment
        .remote_control_target
        .websocket_url
        .as_str()
        .into_client_request()
        .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;
    let headers = request.headers_mut();
    set_header(headers, "x-codex-server-id", &enrollment.server_id)?;
    set_header(
        headers,
        "x-codex-name",
        &base64::engine::general_purpose::STANDARD.encode(&enrollment.server_name),
    )?;
    set_header(
        headers,
        "x-codex-protocol-version",
        REMOTE_CONTROL_PROTOCOL_VERSION,
    )?;
    let token = enrollment.remote_control_token.as_deref().ok_or_else(|| {
        io::Error::new(
            ErrorKind::NotConnected,
            "missing remote control server token",
        )
    })?;
    set_header(headers, "authorization", &format!("Bearer {token}"))?;
    set_header(headers, "x-codex-installation-id", installation_id)?;
    if let Some(device_kind) = host.device_kind.as_deref() {
        set_header(headers, "x-codex-host-device-kind", device_kind)?;
    }
    Ok(request)
}

fn set_header(
    headers: &mut tungstenite::http::HeaderMap,
    name: &'static str,
    value: &str,
) -> io::Result<()> {
    let value = HeaderValue::from_str(value).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid remote control header `{name}`: {error}"),
        )
    })?;
    headers.insert(name, value);
    Ok(())
}

async fn run_connected_websocket(
    websocket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    transport_event_tx: mpsc::Sender<TransportEvent>,
    shutdown: CancellationToken,
) -> io::Result<()> {
    let (mut websocket_writer, mut websocket_reader) = websocket.split();
    let (server_event_tx, mut server_event_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let mut tracker = ClientTracker::new(server_event_tx, transport_event_tx);
    let mut next_seq_by_stream: HashMap<(ClientId, StreamId), u64> = HashMap::new();
    let mut ping_interval = tokio::time::interval(REMOTE_CONTROL_WEBSOCKET_PING_INTERVAL);
    ping_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ping_interval.tick().await;
    let mut maintenance_interval = tokio::time::interval(REMOTE_CONTROL_CLIENT_MAINTENANCE_INTERVAL);
    maintenance_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    maintenance_interval.tick().await;
    let mut last_pong_at = Instant::now();

    let result = 'connection: loop {
        tokio::select! {
            _ = shutdown.cancelled() => break Ok(()),
            _ = ping_interval.tick() => {
                if last_pong_at.elapsed() >= REMOTE_CONTROL_WEBSOCKET_PONG_TIMEOUT {
                    break Err(io::Error::new(ErrorKind::TimedOut, "remote control websocket pong timed out"));
                }
                if let Err(error) = websocket_writer.send(Message::Ping(Vec::new())).await {
                    break Err(map_websocket_error(error));
                }
            }
            _ = maintenance_interval.tick() => {
                if let Err(error) = tracker.drain_finished_clients().await {
                    break 'connection Err(error);
                }
            }
            queued = server_event_rx.recv() => {
                let Some(queued) = queued else {
                    break Ok(());
                };
                let key = (queued.client_id.clone(), queued.stream_id.clone());
                let seq_id = next_seq_by_stream.entry(key).or_insert(0);
                let envelope = ServerEnvelope {
                    event: queued.event,
                    client_id: queued.client_id,
                    stream_id: queued.stream_id,
                    seq_id: *seq_id,
                };
                let segments = match split_server_envelope_for_transport(envelope) {
                    Ok(segments) => segments,
                    Err(error) => break 'connection Err(error),
                };
                for segment in segments {
                    let text = match serde_json::to_string(&segment) {
                        Ok(text) => text,
                        Err(error) => break 'connection Err(io::Error::other(error)),
                    };
                    if let Err(error) = websocket_writer.send(Message::Text(text)).await {
                        break 'connection Err(map_websocket_error(error));
                    }
                }
                *seq_id = seq_id.saturating_add(1);
            }
            incoming = websocket_reader.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientEnvelope>(&text) {
                            Ok(envelope) => {
                                if let Err(error) = tracker.handle_envelope(envelope).await {
                                    break 'connection Err(error);
                                }
                            }
                            Err(error) => warn!("dropping malformed remote control envelope: {error}"),
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if let Err(error) = websocket_writer.send(Message::Pong(payload)).await {
                            break Err(map_websocket_error(error));
                        }
                    }
                    Some(Ok(Message::Pong(_))) => last_pong_at = Instant::now(),
                    Some(Ok(Message::Binary(_))) => {
                        warn!("dropping unsupported remote control binary frame");
                    }
                    Some(Ok(Message::Close(_) | Message::Frame(_))) | None => break Ok(()),
                    Some(Err(error)) => break Err(map_websocket_error(error)),
                }
            }
        }
    };

    tracker.shutdown().await;
    let _ = websocket_writer.close().await;
    result
}

fn map_websocket_error(error: tungstenite::Error) -> io::Error {
    let kind = match &error {
        tungstenite::Error::Io(error) => error.kind(),
        tungstenite::Error::Http(response)
            if matches!(response.status().as_u16(), 401 | 403) =>
        {
            ErrorKind::PermissionDenied
        }
        tungstenite::Error::Http(response) if response.status().as_u16() == 404 => {
            ErrorKind::NotFound
        }
        _ => ErrorKind::Other,
    };
    io::Error::new(kind, format!("remote control websocket error: {error}"))
}
