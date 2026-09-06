use super::auth::RemoteControlAuth;
use super::auth::RemoteControlAuthProvider;
use super::enroll::RemoteControlEnrollment;
use super::host_device::HostDevice;
use super::protocol::ClientEnvelope;
use super::protocol::ClientEvent;
use super::protocol::ClientId;
use super::protocol::ServerEnvelope;
use super::protocol::ServerEvent;
use super::protocol::StreamId;
use super::protocol::normalize_remote_control_url;
use super::state::RemoteControlState;
use super::websocket::RemoteControlWebsocketConfig;
use super::websocket::run_remote_control_websocket;
use super::websocket::run_remote_control_websocket_once;
use async_trait::async_trait;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessage;
use crate::outgoing_message::OutgoingNotification;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::TransportEvent;
use futures::SinkExt;
use futures::StreamExt;
use serde_json::json;
use std::io;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use time::OffsetDateTime;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

struct TestAuthProvider {
    auth: StdMutex<RemoteControlAuth>,
    change_tx: watch::Sender<u64>,
}

impl TestAuthProvider {
    fn new(access_token: &str, account_id: &str) -> Self {
        let (change_tx, _) = watch::channel(0);
        Self {
            auth: StdMutex::new(RemoteControlAuth::for_testing(access_token, account_id)),
            change_tx,
        }
    }

    fn replace_auth(&self, access_token: &str, account_id: &str) {
        *self.auth.lock().expect("lock test auth") =
            RemoteControlAuth::for_testing(access_token, account_id);
        self.change_tx.send_modify(|revision| *revision += 1);
    }
}

#[async_trait]
impl RemoteControlAuthProvider for TestAuthProvider {
    async fn load(&self) -> io::Result<RemoteControlAuth> {
        Ok(self.auth.lock().expect("lock test auth").clone())
    }

    async fn recover_unauthorized(&self) -> io::Result<bool> {
        Ok(false)
    }

    fn subscribe(&self) -> watch::Receiver<u64> {
        self.change_tx.subscribe()
    }
}

#[tokio::test]
async fn websocket_bridges_two_clients_and_segments_large_outbound_messages() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind websocket fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept websocket");
        let mut websocket = accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
            assert_eq!(request.headers()["x-codex-server-id"], "server-a");
            assert_eq!(request.headers()["x-codex-name"], "ZGVjaw==");
            assert_eq!(request.headers()["x-codex-protocol-version"], "3");
            assert_eq!(request.headers()["x-codex-installation-id"], "installation-a");
            assert_eq!(request.headers()["x-codex-account-id"], "account-a");
            assert_eq!(request.headers()["x-codex-environment-id"], "env-a");
            assert_eq!(request.headers()["x-codex-host-os"], "linux");
            assert_eq!(request.headers()["x-codex-host-arch"], "x86_64");
            assert_eq!(request.headers()["x-codex-host-device-kind"], "desktop");
            assert_eq!(request.headers()["x-codex-subscribe-cursor"], "cursor-before");
            assert_eq!(request.headers()["authorization"], "Bearer server-token");
            Ok(response)
        })
        .await
        .expect("accept websocket handshake");
        for (client_id, stream_id) in [("client-1", "stream-1"), ("client-2", "stream-2")] {
            websocket
                .send(Message::Text(
                    serde_json::to_string(&initialize_envelope_with_cursor(
                        client_id,
                        stream_id,
                        Some("cursor-after"),
                    ))
                        .expect("serialize initialize"),
                ))
                .await
                .expect("send initialize");
        }

        let first: ServerEnvelope = serde_json::from_str(
            &next_text(&mut websocket).await,
        )
        .expect("parse first server envelope");
        assert_eq!(first.client_id.0, "client-1");
        assert!(matches!(first.event, ServerEvent::ServerMessage { .. }));

        let mut chunks = Vec::new();
        loop {
            let envelope: ServerEnvelope =
                serde_json::from_str(&next_text(&mut websocket).await).expect("parse server envelope");
            assert_eq!(envelope.client_id.0, "client-2");
            let segment_count = match &envelope.event {
                ServerEvent::ServerMessageChunk { segment_count, .. } => *segment_count,
                event => panic!("expected segmented message, got {event:?}"),
            };
            chunks.push(envelope);
            if chunks.len() == segment_count {
                break;
            }
        }
        let seq_id = chunks[0].seq_id;
        assert!(chunks.iter().all(|chunk| chunk.seq_id == seq_id));
        assert!(chunks.len() > 1);

        websocket
            .send(Message::Text(
                serde_json::to_string(&ClientEnvelope {
                    event: ClientEvent::ClientClosed,
                    client_id: ClientId("client-1".to_string()),
                    stream_id: Some(StreamId("stream-1".to_string())),
                    seq_id: Some(2),
                    cursor: None,
                })
                .expect("serialize close"),
            ))
            .await
            .expect("send close");
        websocket.close(None).await.expect("close fixture websocket");
    });

    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    let enrollment = RemoteControlEnrollment {
        remote_control_target: target,
        account_id: "account-a".to_string(),
        environment_id: "env-a".to_string(),
        server_id: "server-a".to_string(),
        server_name: "deck".to_string(),
        remote_control_token: Some("server-token".to_string()),
        expires_at: Some(OffsetDateTime::now_utc() + time::Duration::hours(1)),
        next_refresh_at: None,
    };
    let host = HostDevice::for_testing("deck", "linux", "x86_64", Some("desktop"));
    let shutdown = CancellationToken::new();
    let (transport_tx, mut transport_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let relay = tokio::spawn(run_remote_control_websocket_once(
        enrollment,
        "installation-a".to_string(),
        host,
        Some("cursor-before".to_string()),
        transport_tx,
        shutdown.clone(),
    ));

    let (connection_1, writer_1) = opened_connection(&mut transport_rx).await;
    assert_initialize(&mut transport_rx, connection_1).await;
    let (connection_2, writer_2) = opened_connection(&mut transport_rx).await;
    assert_initialize(&mut transport_rx, connection_2).await;
    assert_ne!(connection_1, connection_2);

    writer_1
        .send(notification("direct/one", "small".to_string()))
        .await
        .expect("send direct message");
    writer_2
        .send(notification("direct/two", "x".repeat(200 * 1024)))
        .await
        .expect("send large direct message");

    fixture.await.expect("join fixture");
    let mut closed = Vec::new();
    while closed.len() < 2 {
        match timeout(Duration::from_secs(2), transport_rx.recv())
            .await
            .expect("close event timeout")
            .expect("transport remains open")
        {
            TransportEvent::ConnectionClosed { connection_id } => closed.push(connection_id),
            TransportEvent::ConnectionOpened { .. } | TransportEvent::IncomingMessage { .. } => {}
        }
    }
    closed.sort_by_key(|connection_id| connection_id.0);
    let mut expected = [connection_1, connection_2];
    expected.sort_by_key(|connection_id| connection_id.0);
    assert_eq!(closed, expected);
    let subscribe_cursor = relay
        .await
        .expect("join relay")
        .expect("relay session succeeds");
    assert_eq!(subscribe_cursor.as_deref(), Some("cursor-after"));
}

#[tokio::test]
async fn websocket_loop_reconnects_with_the_last_subscribe_cursor() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind websocket fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture_shutdown = CancellationToken::new();
    let fixture_shutdown_task = fixture_shutdown.clone();
    let fixture = tokio::spawn(async move {
        for (attempt, expected_cursor, next_cursor) in [
            (1, None, "cursor-one"),
            (2, Some("cursor-one"), "cursor-two"),
        ] {
            let (stream, _) = listener.accept().await.expect("accept websocket");
            let mut websocket = accept_hdr_async(stream, move |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                assert_eq!(
                    request
                        .headers()
                        .get("x-codex-subscribe-cursor")
                        .and_then(|value| value.to_str().ok()),
                    expected_cursor,
                );
                Ok(response)
            })
            .await
            .expect("accept websocket handshake");
            websocket
                .send(Message::Text(
                    serde_json::to_string(&initialize_envelope_with_cursor(
                        &format!("client-{attempt}"),
                        &format!("stream-{attempt}"),
                        Some(next_cursor),
                    ))
                    .expect("serialize initialize"),
                ))
                .await
                .expect("send initialize");
            if attempt == 1 {
                websocket.close(None).await.expect("close first websocket");
            } else {
                fixture_shutdown_task.cancelled().await;
                websocket.close(None).await.expect("close second websocket");
            }
        }
    });

    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path())
        .await
        .expect("open state");
    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    let current_enrollment = Arc::new(tokio::sync::Mutex::new(Some(test_enrollment(
        target.clone(),
        "account-a",
        "env-a",
        "server-a",
        "server-token",
    ))));
    let auth_provider = Arc::new(TestAuthProvider::new("access-token", "account-a"));
    let shutdown = CancellationToken::new();
    let (transport_tx, mut transport_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let relay = tokio::spawn(run_remote_control_websocket(RemoteControlWebsocketConfig {
        state,
        target,
        auth_provider,
        installation_id: "installation-a".to_string(),
        host: HostDevice::for_testing("deck", "linux", "x86_64", Some("desktop")),
        app_server_client_name: Some("desktop".to_string()),
        remote_control_enabled: Some(true),
        current_enrollment,
        transport_event_tx: transport_tx,
        shutdown: shutdown.clone(),
    }));

    let (first_connection, _) = opened_connection(&mut transport_rx).await;
    assert_initialize(&mut transport_rx, first_connection).await;
    assert_closed(&mut transport_rx, first_connection).await;
    let (second_connection, _) = opened_connection(&mut transport_rx).await;
    assert_initialize(&mut transport_rx, second_connection).await;
    assert_ne!(first_connection, second_connection);

    shutdown.cancel();
    fixture_shutdown.cancel();
    timeout(Duration::from_secs(2), relay)
        .await
        .expect("relay shutdown timeout")
        .expect("join relay")
        .expect("relay loop succeeds");
    fixture.await.expect("join fixture");
}

#[tokio::test]
async fn websocket_unauthorized_refreshes_the_token_without_replacing_identity() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture_shutdown = CancellationToken::new();
    let fixture_shutdown_task = fixture_shutdown.clone();
    let fixture = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept stale websocket");
        let request = read_http_request(&mut stream).await;
        assert!(request.to_ascii_lowercase().contains("authorization: bearer server-token"));
        write_http_response(&mut stream, "401 Unauthorized", br#"{"error":"expired"}"#).await;

        let (mut stream, _) = listener.accept().await.expect("accept refresh");
        let request = read_http_request(&mut stream).await;
        assert!(request.starts_with("POST /backend-api/wham/remote/control/server/refresh HTTP/1.1"));
        assert!(!request.contains("/server/enroll"));
        write_http_response(
            &mut stream,
            "200 OK",
            br#"{"server_id":"server-a","environment_id":"env-a","remote_control_token":"refreshed-token","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .await;

        let (stream, _) = listener.accept().await.expect("accept refreshed websocket");
        let mut websocket = accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
            assert_eq!(request.headers()["x-codex-server-id"], "server-a");
            assert_eq!(request.headers()["authorization"], "Bearer refreshed-token");
            Ok(response)
        })
        .await
        .expect("accept refreshed websocket handshake");
        websocket
            .send(Message::Text(
                serde_json::to_string(&initialize_envelope("client-a", "stream-a"))
                    .expect("serialize initialize"),
            ))
            .await
            .expect("send initialize");
        fixture_shutdown_task.cancelled().await;
        websocket.close(None).await.expect("close websocket");
    });

    let (relay, shutdown, current_enrollment, mut transport_rx, _state_dir) = spawn_relay_loop(
        address,
        Arc::new(TestAuthProvider::new("access-token", "account-a")),
        test_enrollment_for_address(address, "server-a", "server-token"),
    )
    .await;
    let (connection_id, _) = opened_connection(&mut transport_rx).await;
    assert_initialize(&mut transport_rx, connection_id).await;

    let enrollment = current_enrollment.lock().await.clone().expect("current enrollment");
    assert_eq!(enrollment.server_id, "server-a");
    assert_eq!(enrollment.remote_control_token.as_deref(), Some("refreshed-token"));
    shutdown.cancel();
    fixture_shutdown.cancel();
    timeout(Duration::from_secs(2), relay)
        .await
        .expect("relay shutdown timeout")
        .expect("join relay")
        .expect("relay loop succeeds");
    fixture.await.expect("join fixture");
}

#[tokio::test]
async fn websocket_replaces_only_a_recognized_stale_enrollment() {
    assert_stale_enrollment_behavior(
        br#"{"detail":"Remote app server not found"}"#,
        "server-new",
        true,
    )
    .await;
    assert_stale_enrollment_behavior(br#"{"detail":"another missing route"}"#, "server-old", false)
        .await;
}

#[tokio::test]
async fn websocket_loop_reconnects_when_the_authenticated_account_changes() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture_shutdown = CancellationToken::new();
    let fixture_shutdown_task = fixture_shutdown.clone();
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept account A websocket");
        let mut websocket = accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
            assert_eq!(request.headers()["x-codex-account-id"], "account-a");
            Ok(response)
        })
        .await
        .expect("accept account A websocket handshake");
        websocket
            .send(Message::Text(
                serde_json::to_string(&initialize_envelope("client-a", "stream-a"))
                    .expect("serialize initialize"),
            ))
            .await
            .expect("send account A initialize");
        let _ = websocket.next().await;

        let (mut stream, _) = listener.accept().await.expect("accept account B enroll");
        let request = read_http_request(&mut stream).await;
        assert!(request.starts_with("POST /backend-api/wham/remote/control/server/enroll HTTP/1.1"));
        assert!(request.to_ascii_lowercase().contains("chatgpt-account-id: account-b"));
        write_http_response(
            &mut stream,
            "200 OK",
            br#"{"server_id":"server-b","environment_id":"env-b","remote_control_token":"token-b","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .await;

        let (stream, _) = listener.accept().await.expect("accept account B websocket");
        let mut websocket = accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
            assert_eq!(request.headers()["x-codex-account-id"], "account-b");
            assert_eq!(request.headers()["x-codex-server-id"], "server-b");
            Ok(response)
        })
        .await
        .expect("accept account B websocket handshake");
        websocket
            .send(Message::Text(
                serde_json::to_string(&initialize_envelope("client-b", "stream-b"))
                    .expect("serialize initialize"),
            ))
            .await
            .expect("send account B initialize");
        fixture_shutdown_task.cancelled().await;
        websocket.close(None).await.expect("close websocket");
    });

    let auth_provider = Arc::new(TestAuthProvider::new("access-a", "account-a"));
    let (relay, shutdown, current_enrollment, mut transport_rx, _state_dir) = spawn_relay_loop(
        address,
        auth_provider.clone(),
        test_enrollment_for_address(address, "server-a", "token-a"),
    )
    .await;
    let (connection_a, _) = opened_connection(&mut transport_rx).await;
    assert_initialize(&mut transport_rx, connection_a).await;
    auth_provider.replace_auth("access-b", "account-b");
    assert_closed(&mut transport_rx, connection_a).await;
    let (connection_b, _) = opened_connection(&mut transport_rx).await;
    assert_initialize(&mut transport_rx, connection_b).await;

    let enrollment = current_enrollment.lock().await.clone().expect("current enrollment");
    assert_eq!(enrollment.account_id, "account-b");
    assert_eq!(enrollment.server_id, "server-b");
    shutdown.cancel();
    fixture_shutdown.cancel();
    timeout(Duration::from_secs(2), relay)
        .await
        .expect("relay shutdown timeout")
        .expect("join relay")
        .expect("relay loop succeeds");
    fixture.await.expect("join fixture");
}

#[tokio::test]
async fn websocket_loop_shutdown_cancels_reconnect_backoff() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reserve unused address");
    let address = listener.local_addr().expect("unused address");
    drop(listener);
    let (relay, shutdown, _, _, _state_dir) = spawn_relay_loop(
        address,
        Arc::new(TestAuthProvider::new("access-token", "account-a")),
        test_enrollment_for_address(address, "server-a", "server-token"),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    shutdown.cancel();
    timeout(Duration::from_millis(250), relay)
        .await
        .expect("shutdown must interrupt reconnect backoff")
        .expect("join relay")
        .expect("relay loop succeeds");
}

#[tokio::test]
async fn websocket_shutdown_interrupts_a_saturated_transport_queue() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept websocket");
        let mut websocket = accept_hdr_async(stream, |_request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| Ok(response))
            .await
            .expect("accept websocket handshake");
        websocket
            .send(Message::Text(
                serde_json::to_string(&initialize_envelope("client-a", "stream-a"))
                    .expect("serialize initialize"),
            ))
            .await
            .expect("send initialize");
        let _ = websocket.next().await;
    });
    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    let shutdown = CancellationToken::new();
    let (transport_tx, _transport_rx) = mpsc::channel(CHANNEL_CAPACITY);
    for index in 0..CHANNEL_CAPACITY {
        transport_tx
            .try_send(TransportEvent::ConnectionClosed {
                connection_id: ConnectionId(10_000 + index as u64),
            })
            .expect("fill transport queue");
    }
    let relay = tokio::spawn(run_remote_control_websocket_once(
        test_enrollment(target, "account-a", "env-a", "server-a", "server-token"),
        "installation-a".to_string(),
        HostDevice::for_testing("deck", "linux", "x86_64", Some("desktop")),
        None,
        transport_tx,
        shutdown.clone(),
    ));

    tokio::time::sleep(Duration::from_millis(50)).await;
    shutdown.cancel();
    let error = timeout(Duration::from_millis(250), relay)
        .await
        .expect("shutdown must interrupt the saturated queue")
        .expect("join relay")
        .expect_err("the interrupted session reports cancellation");
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    fixture.await.expect("join fixture");
}

async fn next_text(
    websocket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> String {
    loop {
        match websocket.next().await.expect("websocket remains open").expect("read websocket") {
            Message::Text(text) => return text,
            Message::Ping(payload) => websocket
                .send(Message::Pong(payload))
                .await
                .expect("answer ping"),
            Message::Binary(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => {}
        }
    }
}

async fn opened_connection(
    transport_rx: &mut mpsc::Receiver<TransportEvent>,
) -> (ConnectionId, mpsc::Sender<OutgoingMessage>) {
    match transport_rx.recv().await.expect("receive open event") {
        TransportEvent::ConnectionOpened {
            connection_id,
            writer,
            ..
        } => (connection_id, writer),
        event => panic!("expected open event, got {event:?}"),
    }
}

async fn assert_initialize(
    transport_rx: &mut mpsc::Receiver<TransportEvent>,
    expected_connection_id: ConnectionId,
) {
    match transport_rx.recv().await.expect("receive initialize") {
        TransportEvent::IncomingMessage {
            connection_id,
            message: mcp_types::JSONRPCMessage::Request(request),
        } => {
            assert_eq!(connection_id, expected_connection_id);
            assert_eq!(request.method, "initialize");
        }
        event => panic!("expected initialize request, got {event:?}"),
    }
}

async fn assert_closed(
    transport_rx: &mut mpsc::Receiver<TransportEvent>,
    expected_connection_id: ConnectionId,
) {
    loop {
        match timeout(Duration::from_secs(2), transport_rx.recv())
            .await
            .expect("close event timeout")
            .expect("transport remains open")
        {
            TransportEvent::ConnectionClosed { connection_id } => {
                assert_eq!(connection_id, expected_connection_id);
                return;
            }
            TransportEvent::ConnectionOpened { .. } | TransportEvent::IncomingMessage { .. } => {}
        }
    }
}

fn initialize_envelope(client_id: &str, stream_id: &str) -> ClientEnvelope {
    initialize_envelope_with_cursor(client_id, stream_id, None)
}

fn initialize_envelope_with_cursor(
    client_id: &str,
    stream_id: &str,
    cursor: Option<&str>,
) -> ClientEnvelope {
    ClientEnvelope {
        event: ClientEvent::ClientMessage {
            message: serde_json::from_value(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"clientInfo": {"name": "remote-test", "version": "1"}}
            }))
            .expect("parse initialize"),
        },
        client_id: ClientId(client_id.to_string()),
        stream_id: Some(StreamId(stream_id.to_string())),
        seq_id: Some(1),
        cursor: cursor.map(str::to_string),
    }
}

fn notification(method: &str, text: String) -> OutgoingMessage {
    OutgoingMessage::Notification(OutgoingNotification {
        method: method.to_string(),
        params: Some(json!({"text": text})),
    })
}

fn test_enrollment(
    target: super::protocol::RemoteControlTarget,
    account_id: &str,
    environment_id: &str,
    server_id: &str,
    token: &str,
) -> RemoteControlEnrollment {
    RemoteControlEnrollment {
        remote_control_target: target,
        account_id: account_id.to_string(),
        environment_id: environment_id.to_string(),
        server_id: server_id.to_string(),
        server_name: "deck".to_string(),
        remote_control_token: Some(token.to_string()),
        expires_at: Some(OffsetDateTime::now_utc() + time::Duration::hours(1)),
        next_refresh_at: None,
    }
}

fn test_enrollment_for_address(
    address: std::net::SocketAddr,
    server_id: &str,
    token: &str,
) -> RemoteControlEnrollment {
    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    test_enrollment(target, "account-a", "env-a", server_id, token)
}

async fn spawn_relay_loop(
    address: std::net::SocketAddr,
    auth_provider: Arc<TestAuthProvider>,
    enrollment: RemoteControlEnrollment,
) -> (
    tokio::task::JoinHandle<io::Result<()>>,
    CancellationToken,
    Arc<tokio::sync::Mutex<Option<RemoteControlEnrollment>>>,
    mpsc::Receiver<TransportEvent>,
    tempfile::TempDir,
) {
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    let current_enrollment = Arc::new(tokio::sync::Mutex::new(Some(enrollment)));
    let shutdown = CancellationToken::new();
    let (transport_event_tx, transport_event_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let relay = tokio::spawn(run_remote_control_websocket(RemoteControlWebsocketConfig {
        state,
        target,
        auth_provider,
        installation_id: "installation-a".to_string(),
        host: HostDevice::for_testing("deck", "linux", "x86_64", Some("desktop")),
        app_server_client_name: Some("desktop".to_string()),
        remote_control_enabled: Some(true),
        current_enrollment: current_enrollment.clone(),
        transport_event_tx,
        shutdown: shutdown.clone(),
    }));
    (
        relay,
        shutdown,
        current_enrollment,
        transport_event_rx,
        state_dir,
    )
}

async fn assert_stale_enrollment_behavior(
    first_response_body: &'static [u8],
    expected_server_id: &'static str,
    expect_enroll: bool,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture_shutdown = CancellationToken::new();
    let fixture_shutdown_task = fixture_shutdown.clone();
    let fixture = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept stale websocket");
        let request = read_http_request(&mut stream).await;
        assert!(request.contains("x-codex-server-id: server-old"));
        write_http_response(&mut stream, "404 Not Found", first_response_body).await;

        if expect_enroll {
            let (mut stream, _) = listener.accept().await.expect("accept replacement enroll");
            let request = read_http_request(&mut stream).await;
            assert!(request.starts_with("POST /backend-api/wham/remote/control/server/enroll HTTP/1.1"));
            write_http_response(
                &mut stream,
                "200 OK",
                br#"{"server_id":"server-new","environment_id":"env-new","remote_control_token":"token-new","expires_at":"2030-01-01T00:00:00Z"}"#,
            )
            .await;
        }

        let (stream, _) = listener.accept().await.expect("accept next websocket");
        let mut websocket = accept_hdr_async(stream, move |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
            assert_eq!(request.headers()["x-codex-server-id"], expected_server_id);
            Ok(response)
        })
        .await
        .expect("accept next websocket handshake");
        websocket
            .send(Message::Text(
                serde_json::to_string(&initialize_envelope("client-a", "stream-a"))
                    .expect("serialize initialize"),
            ))
            .await
            .expect("send initialize");
        fixture_shutdown_task.cancelled().await;
        websocket.close(None).await.expect("close websocket");
    });

    let (relay, shutdown, current_enrollment, mut transport_rx, _state_dir) = spawn_relay_loop(
        address,
        Arc::new(TestAuthProvider::new("access-token", "account-a")),
        test_enrollment_for_address(address, "server-old", "token-old"),
    )
    .await;
    let (connection_id, _) = opened_connection(&mut transport_rx).await;
    assert_initialize(&mut transport_rx, connection_id).await;
    assert_eq!(
        current_enrollment
            .lock()
            .await
            .as_ref()
            .expect("current enrollment")
            .server_id,
        expected_server_id,
    );
    shutdown.cancel();
    fixture_shutdown.cancel();
    timeout(Duration::from_secs(2), relay)
        .await
        .expect("relay shutdown timeout")
        .expect("join relay")
        .expect("relay loop succeeds");
    fixture.await.expect("join fixture");
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 2048];
    loop {
        let read = stream.read(&mut buffer).await.expect("read request");
        assert!(read > 0, "request ended before headers");
        request.extend_from_slice(&buffer[..read]);
        let Some(header_at) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
            continue;
        };
        let header_end = header_at + 4;
        let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let read = stream.read(&mut buffer).await.expect("read request body");
            assert!(read > 0, "request body ended early");
            request.extend_from_slice(&buffer[..read]);
        }
        return String::from_utf8_lossy(&request[..header_end + content_length]).into_owned();
    }
}

async fn write_http_response(stream: &mut tokio::net::TcpStream, status: &str, body: &[u8]) {
    stream
        .write_all(
            format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .expect("write response headers");
    stream.write_all(body).await.expect("write response body");
}
