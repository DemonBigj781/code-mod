use super::enroll::RemoteControlEnrollment;
use super::host_device::HostDevice;
use super::protocol::ClientEnvelope;
use super::protocol::ClientEvent;
use super::protocol::ClientId;
use super::protocol::ServerEnvelope;
use super::protocol::ServerEvent;
use super::protocol::StreamId;
use super::protocol::normalize_remote_control_url;
use super::websocket::run_remote_control_websocket_once;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessage;
use crate::outgoing_message::OutgoingNotification;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::TransportEvent;
use futures::SinkExt;
use futures::StreamExt;
use serde_json::json;
use time::OffsetDateTime;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

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
            assert_eq!(request.headers()["x-codex-host-device-kind"], "desktop");
            assert_eq!(request.headers()["authorization"], "Bearer server-token");
            Ok(response)
        })
        .await
        .expect("accept websocket handshake");
        for (client_id, stream_id) in [("client-1", "stream-1"), ("client-2", "stream-2")] {
            websocket
                .send(Message::Text(
                    serde_json::to_string(&initialize_envelope(client_id, stream_id))
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
    relay
        .await
        .expect("join relay")
        .expect("relay session succeeds");
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

fn initialize_envelope(client_id: &str, stream_id: &str) -> ClientEnvelope {
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
        cursor: None,
    }
}

fn notification(method: &str, text: String) -> OutgoingMessage {
    OutgoingMessage::Notification(OutgoingNotification {
        method: method.to_string(),
        params: Some(json!({"text": text})),
    })
}
