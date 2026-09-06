use super::protocol::ClientEnvelope;
use super::protocol::ClientEvent;
use super::protocol::ClientId;
use super::protocol::PongStatus;
use super::protocol::ServerEvent;
use super::protocol::StreamId;
use super::segment::ClientSegmentObservation;
use super::segment::ClientSegmentReassembler;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessage;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::TransportEvent;
use crate::transport::next_connection_id;
use mcp_types::JSONRPCMessage;
use std::collections::HashMap;
use std::io;
use std::io::ErrorKind;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::Duration;
use tokio::time::timeout;

const REMOTE_CONTROL_TRANSPORT_EVENT_SEND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub(crate) struct QueuedServerEnvelope {
    pub event: ServerEvent,
    pub client_id: ClientId,
    pub stream_id: StreamId,
}

struct ClientState {
    connection_id: ConnectionId,
    disconnect_notify: Arc<Notify>,
    last_inbound_seq_id: Option<u64>,
}

pub(crate) struct ClientTracker {
    clients: HashMap<(ClientId, StreamId), ClientState>,
    legacy_stream_ids: HashMap<ClientId, StreamId>,
    join_set: JoinSet<(ClientId, StreamId)>,
    server_event_tx: mpsc::Sender<QueuedServerEnvelope>,
    transport_event_tx: mpsc::Sender<TransportEvent>,
    reassembler: ClientSegmentReassembler,
}

impl ClientTracker {
    pub(crate) fn new(
        server_event_tx: mpsc::Sender<QueuedServerEnvelope>,
        transport_event_tx: mpsc::Sender<TransportEvent>,
    ) -> Self {
        Self {
            clients: HashMap::new(),
            legacy_stream_ids: HashMap::new(),
            join_set: JoinSet::new(),
            server_event_tx,
            transport_event_tx,
            reassembler: ClientSegmentReassembler::default(),
        }
    }

    pub(crate) async fn handle_envelope(&mut self, envelope: ClientEnvelope) -> io::Result<()> {
        let envelope = match self.reassembler.observe(envelope) {
            ClientSegmentObservation::Forward(envelope) => *envelope,
            ClientSegmentObservation::Pending | ClientSegmentObservation::Dropped => return Ok(()),
        };
        let ClientEnvelope {
            client_id,
            event,
            stream_id,
            seq_id,
            cursor: _,
        } = envelope;
        let is_initialize = matches!(
            &event,
            ClientEvent::ClientMessage { message } if message_starts_connection(message)
        );
        let legacy_stream = stream_id.is_none();
        let stream_id = match stream_id {
            Some(stream_id) => stream_id,
            None if is_initialize => self
                .legacy_stream_ids
                .remove(&client_id)
                .unwrap_or_else(StreamId::new_random),
            None => self
                .legacy_stream_ids
                .get(&client_id)
                .cloned()
                .unwrap_or_else(|| {
                    if matches!(event, ClientEvent::Ping) {
                        StreamId::new_random()
                    } else {
                        StreamId(String::new())
                    }
                }),
        };
        if stream_id.0.is_empty() {
            return Ok(());
        }
        let client_key = (client_id.clone(), stream_id.clone());

        match event {
            ClientEvent::ClientMessage { message } => {
                if !is_initialize
                    && seq_id.is_some_and(|seq_id| {
                        self.clients
                            .get(&client_key)
                            .and_then(|client| client.last_inbound_seq_id)
                            .is_some_and(|last_seq_id| last_seq_id >= seq_id)
                    })
                {
                    return Ok(());
                }
                if is_initialize && self.clients.contains_key(&client_key) {
                    self.close_client(&client_key).await?;
                }
                if let Some(connection_id) = self
                    .clients
                    .get(&client_key)
                    .map(|client| client.connection_id)
                {
                    self.send_transport_event(TransportEvent::IncomingMessage {
                        connection_id,
                        message,
                    })
                    .await?;
                    self.record_delivery(&client_key, seq_id);
                    return Ok(());
                }
                if !is_initialize {
                    return Ok(());
                }

                let connection_id = next_connection_id();
                let (writer_tx, writer_rx) = mpsc::channel(CHANNEL_CAPACITY);
                let disconnect_notify = Arc::new(Notify::new());
                self.send_transport_event(TransportEvent::ConnectionOpened {
                    connection_id,
                    writer: writer_tx,
                    disconnect_notify: Some(Arc::clone(&disconnect_notify)),
                })
                .await?;
                self.join_set.spawn(run_client_outbound(
                    client_id.clone(),
                    stream_id.clone(),
                    self.server_event_tx.clone(),
                    writer_rx,
                    Arc::clone(&disconnect_notify),
                ));
                self.clients.insert(
                    client_key.clone(),
                    ClientState {
                        connection_id,
                        disconnect_notify,
                        last_inbound_seq_id: None,
                    },
                );
                if legacy_stream {
                    self.legacy_stream_ids
                        .insert(client_id.clone(), stream_id.clone());
                }
                if let Err(error) = self
                    .send_transport_event(TransportEvent::IncomingMessage {
                        connection_id,
                        message,
                    })
                    .await
                {
                    if let Some(client) = self.remove_client(&client_key) {
                        client.disconnect_notify.notify_one();
                    }
                    return Err(error);
                }
                self.record_delivery(&client_key, seq_id);
                Ok(())
            }
            ClientEvent::Ping => {
                let status = if self.clients.contains_key(&client_key) {
                    PongStatus::Active
                } else {
                    PongStatus::Unknown
                };
                self.server_event_tx
                    .send(QueuedServerEnvelope {
                        event: ServerEvent::Pong { status },
                        client_id,
                        stream_id,
                    })
                    .await
                    .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "relay writer unavailable"))
            }
            ClientEvent::ClientClosed => {
                self.reassembler.invalidate_stream(&client_id, &stream_id);
                self.close_client(&client_key).await
            }
            ClientEvent::Ack { .. } | ClientEvent::ClientMessageChunk { .. } => Ok(()),
        }
    }

    pub(crate) async fn bookkeep_finished_client(&mut self) -> io::Result<()> {
        let Some(result) = self.join_set.join_next().await else {
            return futures::future::pending().await;
        };
        if let Ok(client_key) = result {
            self.close_client(&client_key).await?;
        }
        Ok(())
    }

    pub(crate) async fn drain_finished_clients(&mut self) -> io::Result<()> {
        while let Some(result) = self.join_set.try_join_next() {
            if let Ok(client_key) = result {
                self.close_client(&client_key).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn shutdown(&mut self) {
        let client_keys: Vec<_> = self.clients.keys().cloned().collect();
        for client_key in client_keys {
            let _ = self.close_client(&client_key).await;
        }
        while self.join_set.join_next().await.is_some() {}
    }

    async fn close_client(&mut self, client_key: &(ClientId, StreamId)) -> io::Result<()> {
        let Some(client) = self.remove_client(client_key) else {
            return Ok(());
        };
        client.disconnect_notify.notify_one();
        self.send_transport_event(TransportEvent::ConnectionClosed {
            connection_id: client.connection_id,
        })
        .await
    }

    fn remove_client(&mut self, client_key: &(ClientId, StreamId)) -> Option<ClientState> {
        let client = self.clients.remove(client_key)?;
        self.reassembler.invalidate_stream(&client_key.0, &client_key.1);
        if self
            .legacy_stream_ids
            .get(&client_key.0)
            .is_some_and(|stream_id| stream_id == &client_key.1)
        {
            self.legacy_stream_ids.remove(&client_key.0);
        }
        Some(client)
    }

    fn record_delivery(&mut self, client_key: &(ClientId, StreamId), seq_id: Option<u64>) {
        if let Some(seq_id) = seq_id
            && let Some(client) = self.clients.get_mut(client_key)
        {
            client.last_inbound_seq_id = Some(seq_id);
        }
    }

    async fn send_transport_event(&self, event: TransportEvent) -> io::Result<()> {
        timeout(
            REMOTE_CONTROL_TRANSPORT_EVENT_SEND_TIMEOUT,
            self.transport_event_tx.send(event),
        )
        .await
        .map_err(|_| io::Error::new(ErrorKind::TimedOut, "remote transport event timed out"))?
        .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "app-server processor unavailable"))
    }
}

async fn run_client_outbound(
    client_id: ClientId,
    stream_id: StreamId,
    server_event_tx: mpsc::Sender<QueuedServerEnvelope>,
    mut writer_rx: mpsc::Receiver<OutgoingMessage>,
    disconnect_notify: Arc<Notify>,
) -> (ClientId, StreamId) {
    loop {
        let outgoing = tokio::select! {
            _ = disconnect_notify.notified() => break,
            outgoing = writer_rx.recv() => outgoing,
        };
        let Some(outgoing) = outgoing else {
            break;
        };
        let send = server_event_tx.send(QueuedServerEnvelope {
                event: ServerEvent::ServerMessage {
                    message: Box::new(outgoing.into()),
                },
                client_id: client_id.clone(),
                stream_id: stream_id.clone(),
            });
        let sent = tokio::select! {
            _ = disconnect_notify.notified() => false,
            result = send => result.is_ok(),
        };
        if !sent {
            break;
        }
    }
    (client_id, stream_id)
}

fn message_starts_connection(message: &JSONRPCMessage) -> bool {
    matches!(message, JSONRPCMessage::Request(request) if request.method == "initialize")
}
