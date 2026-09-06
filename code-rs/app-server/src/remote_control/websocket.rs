use super::auth::RemoteControlAuthProvider;
use super::client_tracker::ClientTracker;
use super::enroll::RemoteControlEnrollment;
use super::enroll::RemoteControlEnrollmentSelection;
use super::enroll::resolve_remote_control_enrollment;
use super::host_device::HostDevice;
use super::protocol::ClientEnvelope;
use super::protocol::ClientId;
use super::protocol::RemoteControlTarget;
use super::protocol::ServerEnvelope;
use super::protocol::StreamId;
use super::segment::split_server_envelope_for_transport;
use super::state::RemoteControlState;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::TransportEvent;
use base64::Engine;
use futures::SinkExt;
use futures::StreamExt;
use std::collections::HashMap;
use std::io;
use std::io::ErrorKind;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::sync::watch;
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
const REMOTE_CONTROL_RECONNECT_INITIAL_DELAY: Duration = Duration::from_millis(200);
const REMOTE_CONTROL_RECONNECT_BACKOFF_CAP: Duration = Duration::from_secs(30);
const REMOTE_APP_SERVER_NOT_FOUND_DETAIL: &str = "Remote app server not found";

pub(crate) struct RemoteControlWebsocketConfig {
    pub(crate) state: Arc<RemoteControlState>,
    pub(crate) target: RemoteControlTarget,
    pub(crate) auth_provider: Arc<dyn RemoteControlAuthProvider>,
    pub(crate) installation_id: String,
    pub(crate) host: HostDevice,
    pub(crate) app_server_client_name: Option<String>,
    pub(crate) remote_control_enabled: Option<bool>,
    pub(crate) current_enrollment: Arc<Mutex<Option<RemoteControlEnrollment>>>,
    pub(crate) transport_event_tx: mpsc::Sender<TransportEvent>,
    pub(crate) shutdown: CancellationToken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WebsocketConnectFailureKind {
    Unauthorized,
    StaleEnrollment,
    Other,
}

struct WebsocketConnectFailure {
    kind: WebsocketConnectFailureKind,
    error: io::Error,
}

enum RetryOutcome {
    Retry,
    AuthChanged,
    Shutdown,
}

pub(crate) async fn run_remote_control_websocket(
    config: RemoteControlWebsocketConfig,
) -> io::Result<()> {
    let mut subscribe_cursor = None;
    let mut reconnect_attempt = 0_u32;
    let mut selection = RemoteControlEnrollmentSelection::ReuseOrCreate;
    let mut auth_change_rx = config.auth_provider.subscribe();

    loop {
        let enrollment = tokio::select! {
            _ = config.shutdown.cancelled() => return Ok(()),
            changed = auth_change_rx.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                reconnect_attempt = 0;
                selection = RemoteControlEnrollmentSelection::ReuseOrCreate;
                continue;
            }
            enrollment = resolve_enrollment_for_connection(&config, selection) => {
                match enrollment {
                    Ok(enrollment) => enrollment,
                    Err(error) => {
                        warn!(error = %error, error_kind = ?error.kind(), "failed to prepare remote control enrollment");
                        match wait_for_reconnect(&config.shutdown, &mut auth_change_rx, &mut reconnect_attempt).await {
                            RetryOutcome::Retry => {}
                            RetryOutcome::AuthChanged => reconnect_attempt = 0,
                            RetryOutcome::Shutdown => return Ok(()),
                        }
                        selection = RemoteControlEnrollmentSelection::ReuseOrCreate;
                        continue;
                    }
                }
            }
        };

        let connect_result = tokio::select! {
            _ = config.shutdown.cancelled() => return Ok(()),
            changed = auth_change_rx.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                reconnect_attempt = 0;
                continue;
            }
            result = connect_remote_control_websocket(
                &enrollment,
                &config.installation_id,
                &config.host,
                subscribe_cursor.as_deref(),
            ) => result,
        };

        let websocket = match connect_result {
            Ok(websocket) => websocket,
            Err(failure) => {
                match failure.kind {
                    WebsocketConnectFailureKind::Unauthorized => {
                        clear_server_token_if_current(&config.current_enrollment, &enrollment).await;
                        selection = RemoteControlEnrollmentSelection::ReuseOrCreate;
                    }
                    WebsocketConnectFailureKind::StaleEnrollment => {
                        selection = RemoteControlEnrollmentSelection::ReplaceExisting;
                    }
                    WebsocketConnectFailureKind::Other => {
                        selection = RemoteControlEnrollmentSelection::ReuseOrCreate;
                    }
                }
                warn!(
                    error = %failure.error,
                    error_kind = ?failure.error.kind(),
                    failure_kind = ?failure.kind,
                    "remote control websocket connection failed"
                );
                match wait_for_reconnect(&config.shutdown, &mut auth_change_rx, &mut reconnect_attempt).await {
                    RetryOutcome::Retry => {}
                    RetryOutcome::AuthChanged => {
                        reconnect_attempt = 0;
                        selection = RemoteControlEnrollmentSelection::ReuseOrCreate;
                    }
                    RetryOutcome::Shutdown => return Ok(()),
                }
                continue;
            }
        };

        reconnect_attempt = 0;
        selection = RemoteControlEnrollmentSelection::ReuseOrCreate;
        let connection_shutdown = config.shutdown.child_token();
        let connection = run_connected_websocket(
            websocket,
            config.transport_event_tx.clone(),
            connection_shutdown.clone(),
            &mut subscribe_cursor,
        );
        tokio::pin!(connection);
        let session_result = tokio::select! {
            result = &mut connection => Some(result),
            _ = config.shutdown.cancelled() => {
                connection_shutdown.cancel();
                let _ = connection.await;
                return Ok(());
            }
            changed = auth_change_rx.changed() => {
                connection_shutdown.cancel();
                let _ = connection.await;
                if changed.is_err() {
                    return Ok(());
                }
                None
            }
        };
        if session_result.is_none() {
            continue;
        }
        if let Some(Err(error)) = session_result {
            if config.transport_event_tx.is_closed() {
                return Err(error);
            }
            warn!(error = %error, error_kind = ?error.kind(), "remote control websocket session ended with an error");
        }
        match wait_for_reconnect(&config.shutdown, &mut auth_change_rx, &mut reconnect_attempt).await {
            RetryOutcome::Retry => {}
            RetryOutcome::AuthChanged => reconnect_attempt = 0,
            RetryOutcome::Shutdown => return Ok(()),
        }
    }
}

pub(crate) async fn run_remote_control_websocket_once(
    enrollment: RemoteControlEnrollment,
    installation_id: String,
    host: HostDevice,
    mut subscribe_cursor: Option<String>,
    transport_event_tx: mpsc::Sender<TransportEvent>,
    shutdown: CancellationToken,
) -> io::Result<Option<String>> {
    let websocket = connect_remote_control_websocket(
        &enrollment,
        &installation_id,
        &host,
        subscribe_cursor.as_deref(),
    )
    .await
    .map_err(|failure| failure.error)?;
    run_connected_websocket(
        websocket,
        transport_event_tx,
        shutdown,
        &mut subscribe_cursor,
    )
    .await?;
    Ok(subscribe_cursor)
}

fn build_websocket_request(
    enrollment: &RemoteControlEnrollment,
    installation_id: &str,
    host: &HostDevice,
    subscribe_cursor: Option<&str>,
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
    set_header(headers, "x-codex-account-id", &enrollment.account_id)?;
    set_header(
        headers,
        "x-codex-environment-id",
        &enrollment.environment_id,
    )?;
    set_header(headers, "x-codex-host-os", &host.os)?;
    set_header(headers, "x-codex-host-arch", &host.arch)?;
    if let Some(device_kind) = host.device_kind.as_deref() {
        set_header(headers, "x-codex-host-device-kind", device_kind)?;
    }
    if let Some(subscribe_cursor) = subscribe_cursor {
        set_header(
            headers,
            "x-codex-subscribe-cursor",
            subscribe_cursor,
        )?;
    }
    Ok(request)
}

async fn resolve_enrollment_for_connection(
    config: &RemoteControlWebsocketConfig,
    selection: RemoteControlEnrollmentSelection,
) -> io::Result<RemoteControlEnrollment> {
    let mut current_enrollment = config.current_enrollment.lock().await;
    let enrollment = resolve_remote_control_enrollment(
        &config.state,
        &config.target,
        config.auth_provider.as_ref(),
        &config.installation_id,
        &config.host,
        config.app_server_client_name.as_deref(),
        current_enrollment.as_ref(),
        config.remote_control_enabled,
        selection,
    )
    .await?;
    *current_enrollment = Some(enrollment.clone());
    Ok(enrollment)
}

async fn connect_remote_control_websocket(
    enrollment: &RemoteControlEnrollment,
    installation_id: &str,
    host: &HostDevice,
    subscribe_cursor: Option<&str>,
) -> Result<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    WebsocketConnectFailure,
> {
    let request = build_websocket_request(enrollment, installation_id, host, subscribe_cursor)
        .map_err(|error| WebsocketConnectFailure {
            kind: WebsocketConnectFailureKind::Other,
            error,
        })?;
    let connect_result = tokio::time::timeout(
        REMOTE_CONTROL_WEBSOCKET_CONNECT_TIMEOUT,
        connect_async(request),
    )
    .await
    .map_err(|_| WebsocketConnectFailure {
        kind: WebsocketConnectFailureKind::Other,
        error: io::Error::new(
            ErrorKind::TimedOut,
            "timed out connecting to remote control websocket",
        ),
    })?;
    match connect_result {
        Ok((websocket, _)) => Ok(websocket),
        Err(error) => {
            let kind = match &error {
                tungstenite::Error::Http(response)
                    if matches!(response.status().as_u16(), 401 | 403) =>
                {
                    WebsocketConnectFailureKind::Unauthorized
                }
                tungstenite::Error::Http(response)
                    if websocket_response_reports_missing_remote_app_server(response) =>
                {
                    WebsocketConnectFailureKind::StaleEnrollment
                }
                _ => WebsocketConnectFailureKind::Other,
            };
            Err(WebsocketConnectFailure {
                kind,
                error: map_websocket_error(error),
            })
        }
    }
}

fn websocket_response_reports_missing_remote_app_server(
    response: &tungstenite::http::Response<Option<Vec<u8>>>,
) -> bool {
    response.status().as_u16() == 404
        && response.body().as_deref().is_some_and(|body| {
            serde_json::from_slice::<serde_json::Value>(body).is_ok_and(|body| {
                body.get("detail").and_then(serde_json::Value::as_str)
                    == Some(REMOTE_APP_SERVER_NOT_FOUND_DETAIL)
            })
        })
}

async fn clear_server_token_if_current(
    current_enrollment: &Mutex<Option<RemoteControlEnrollment>>,
    failed_enrollment: &RemoteControlEnrollment,
) {
    let mut current_enrollment = current_enrollment.lock().await;
    let Some(current) = current_enrollment.as_mut() else {
        return;
    };
    if current.account_id == failed_enrollment.account_id
        && current.server_id == failed_enrollment.server_id
        && current.environment_id == failed_enrollment.environment_id
        && current.remote_control_token == failed_enrollment.remote_control_token
    {
        current.remote_control_token = None;
        current.expires_at = None;
        current.next_refresh_at = None;
    }
}

async fn wait_for_reconnect(
    shutdown: &CancellationToken,
    auth_change_rx: &mut watch::Receiver<u64>,
    reconnect_attempt: &mut u32,
) -> RetryOutcome {
    let delay = next_reconnect_delay(reconnect_attempt);
    tokio::select! {
        _ = shutdown.cancelled() => RetryOutcome::Shutdown,
        changed = auth_change_rx.changed() => {
            if changed.is_ok() {
                RetryOutcome::AuthChanged
            } else {
                RetryOutcome::Shutdown
            }
        }
        _ = tokio::time::sleep(delay) => RetryOutcome::Retry,
    }
}

fn next_reconnect_delay(reconnect_attempt: &mut u32) -> Duration {
    let multiplier = 1_u32 << (*reconnect_attempt).min(16);
    let delay = REMOTE_CONTROL_RECONNECT_INITIAL_DELAY
        .saturating_mul(multiplier)
        .min(REMOTE_CONTROL_RECONNECT_BACKOFF_CAP);
    *reconnect_attempt = if delay == REMOTE_CONTROL_RECONNECT_BACKOFF_CAP {
        0
    } else {
        reconnect_attempt.saturating_add(1)
    };
    delay
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
    subscribe_cursor: &mut Option<String>,
) -> io::Result<()> {
    let (mut websocket_writer, mut websocket_reader) = websocket.split();
    let (server_event_tx, mut server_event_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let mut tracker = ClientTracker::new(server_event_tx, transport_event_tx, shutdown.clone());
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
                                if let Some(cursor) = envelope.cursor.as_ref() {
                                    *subscribe_cursor = Some(cursor.clone());
                                }
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
