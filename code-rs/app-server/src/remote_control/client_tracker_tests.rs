use super::client_tracker::ClientTracker;
use super::client_tracker::QueuedServerEnvelope;
use super::protocol::ClientEnvelope;
use super::protocol::ClientEvent;
use super::protocol::ClientId;
use super::protocol::ServerEvent;
use super::protocol::StreamId;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessage;
use crate::outgoing_message::OutgoingNotification;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::TransportEvent;
use mcp_types::JSONRPCMessage;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn routes_two_remote_streams_bidirectionally_and_closes_them_independently() {
    let (server_tx, mut server_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (transport_tx, mut transport_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let mut tracker = ClientTracker::new(server_tx, transport_tx, CancellationToken::new());

    tracker
        .handle_envelope(initialize_envelope("client-1", "stream-1", 1))
        .await
        .expect("open first client");
    let (connection_1, writer_1) = opened_connection(&mut transport_rx).await;
    assert_incoming(&mut transport_rx, connection_1, "initialize").await;

    tracker
        .handle_envelope(initialize_envelope("client-2", "stream-2", 1))
        .await
        .expect("open second client");
    let (connection_2, writer_2) = opened_connection(&mut transport_rx).await;
    assert_ne!(connection_1, connection_2);
    assert_incoming(&mut transport_rx, connection_2, "initialize").await;

    tracker
        .handle_envelope(message_envelope(
            "client-1",
            "stream-1",
            2,
            json!({"jsonrpc": "2.0", "method": "initialized"}),
        ))
        .await
        .expect("forward first client notification");
    assert_incoming(&mut transport_rx, connection_1, "initialized").await;

    writer_1
        .send(notification("direct/one"))
        .await
        .expect("queue direct response");
    let direct = server_rx.recv().await.expect("receive direct response");
    assert_server_notification(&direct, "client-1", "stream-1", "direct/one");
    assert!(timeout(Duration::from_millis(25), server_rx.recv()).await.is_err());

    writer_1
        .send(notification("broadcast/all"))
        .await
        .expect("queue first broadcast copy");
    writer_2
        .send(notification("broadcast/all"))
        .await
        .expect("queue second broadcast copy");
    let first = server_rx.recv().await.expect("receive first broadcast copy");
    let second = server_rx.recv().await.expect("receive second broadcast copy");
    let mut recipients = [first.client_id.0, second.client_id.0];
    recipients.sort();
    assert_eq!(recipients, ["client-1", "client-2"]);

    tracker
        .handle_envelope(ClientEnvelope {
            event: ClientEvent::ClientClosed,
            client_id: ClientId("client-1".to_string()),
            stream_id: Some(StreamId("stream-1".to_string())),
            seq_id: Some(3),
            cursor: None,
        })
        .await
        .expect("close first client");
    match transport_rx.recv().await.expect("receive close event") {
        TransportEvent::ConnectionClosed { connection_id } => {
            assert_eq!(connection_id, connection_1);
        }
        event => panic!("expected close event, got {event:?}"),
    }

    writer_2
        .send(notification("still/active"))
        .await
        .expect("second client remains active");
    let active = server_rx.recv().await.expect("receive active response");
    assert_server_notification(&active, "client-2", "stream-2", "still/active");
    tracker.shutdown().await;
}

#[tokio::test]
async fn ignores_unknown_and_duplicate_messages_and_answers_ping_status() {
    let (server_tx, mut server_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (transport_tx, mut transport_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let mut tracker = ClientTracker::new(server_tx, transport_tx, CancellationToken::new());

    tracker
        .handle_envelope(message_envelope(
            "unknown",
            "stream-x",
            1,
            json!({"jsonrpc": "2.0", "method": "initialized"}),
        ))
        .await
        .expect("ignore unknown client");
    assert!(timeout(Duration::from_millis(25), transport_rx.recv()).await.is_err());

    tracker
        .handle_envelope(initialize_envelope("client-1", "stream-1", 1))
        .await
        .expect("open client");
    let (connection_id, _writer) = opened_connection(&mut transport_rx).await;
    assert_incoming(&mut transport_rx, connection_id, "initialize").await;
    let notification = message_envelope(
        "client-1",
        "stream-1",
        2,
        json!({"jsonrpc": "2.0", "method": "initialized"}),
    );
    tracker
        .handle_envelope(notification.clone())
        .await
        .expect("forward notification");
    assert_incoming(&mut transport_rx, connection_id, "initialized").await;
    tracker
        .handle_envelope(notification)
        .await
        .expect("ignore duplicate notification");
    assert!(timeout(Duration::from_millis(25), transport_rx.recv()).await.is_err());

    tracker
        .handle_envelope(ClientEnvelope {
            event: ClientEvent::Ping,
            client_id: ClientId("client-1".to_string()),
            stream_id: Some(StreamId("stream-1".to_string())),
            seq_id: Some(3),
            cursor: None,
        })
        .await
        .expect("answer active ping");
    let pong = server_rx.recv().await.expect("receive pong");
    assert!(matches!(pong.event, ServerEvent::Pong { .. }));
    assert_eq!(pong.client_id.0, "client-1");
    tracker.shutdown().await;
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

async fn assert_incoming(
    transport_rx: &mut mpsc::Receiver<TransportEvent>,
    expected_connection_id: ConnectionId,
    expected_method: &str,
) {
    match transport_rx.recv().await.expect("receive incoming message") {
        TransportEvent::IncomingMessage {
            connection_id,
            message,
        } => {
            assert_eq!(connection_id, expected_connection_id);
            let method = match message {
                JSONRPCMessage::Request(request) => request.method,
                JSONRPCMessage::Notification(notification) => notification.method,
                message => panic!("expected request or notification, got {message:?}"),
            };
            assert_eq!(method, expected_method);
        }
        event => panic!("expected incoming event, got {event:?}"),
    }
}

fn initialize_envelope(client_id: &str, stream_id: &str, seq_id: u64) -> ClientEnvelope {
    message_envelope(
        client_id,
        stream_id,
        seq_id,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {"name": "remote-test", "version": "1"}}
        }),
    )
}

fn message_envelope(
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    message: serde_json::Value,
) -> ClientEnvelope {
    ClientEnvelope {
        event: ClientEvent::ClientMessage {
            message: serde_json::from_value(message).expect("parse message"),
        },
        client_id: ClientId(client_id.to_string()),
        stream_id: Some(StreamId(stream_id.to_string())),
        seq_id: Some(seq_id),
        cursor: None,
    }
}

fn notification(method: &str) -> OutgoingMessage {
    OutgoingMessage::Notification(OutgoingNotification {
        method: method.to_string(),
        params: None,
    })
}

fn assert_server_notification(
    envelope: &QueuedServerEnvelope,
    client_id: &str,
    stream_id: &str,
    method: &str,
) {
    assert_eq!(envelope.client_id.0, client_id);
    assert_eq!(envelope.stream_id.0, stream_id);
    match &envelope.event {
        ServerEvent::ServerMessage { message } => match message.as_ref() {
            JSONRPCMessage::Notification(notification) => assert_eq!(notification.method, method),
            message => panic!("expected notification, got {message:?}"),
        },
        event => panic!("expected server message, got {event:?}"),
    }
}
