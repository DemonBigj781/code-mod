use super::protocol::ClientEnvelope;
use super::protocol::ClientEvent;
use super::protocol::ClientId;
use super::protocol::ServerEnvelope;
use super::protocol::ServerEvent;
use super::protocol::StreamId;
use base64::Engine;
use mcp_types::JSONRPCMessage;
use std::collections::HashMap;
use std::io;
use std::io::ErrorKind;
use std::io::Write;
use tokio::time::Instant;
use tracing::warn;

pub(crate) const REMOTE_CONTROL_SEGMENT_TARGET_BYTES: usize = 100 * 1024;
pub(crate) const REMOTE_CONTROL_SEGMENT_MAX_BYTES: usize = 150 * 1024;
pub(crate) const REMOTE_CONTROL_REASSEMBLED_MAX_BYTES: usize = 100 * 1024 * 1024;
pub(crate) const REMOTE_CONTROL_SEGMENT_COUNT_MAX: usize = 1024;
const REMOTE_CONTROL_SEGMENT_ASSEMBLY_MAX_COUNT: usize = 128;
const REMOTE_CONTROL_SEGMENT_ASSEMBLY_MAX_BUFFERED_BYTES: usize = 100 * 1024 * 1024;

#[derive(Debug)]
struct ClientSegmentAssembly {
    stream_id: StreamId,
    metadata: ClientSegmentMetadata,
    raw: Vec<u8>,
    next_segment_id: usize,
    last_chunk_seen_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClientSegmentMetadata {
    seq_id: u64,
    segment_count: usize,
    message_size_bytes: usize,
}

pub(crate) struct ClientSegmentReassembler {
    assemblies: HashMap<ClientId, ClientSegmentAssembly>,
    buffered_bytes: usize,
    max_assemblies: usize,
    max_buffered_bytes: usize,
}

impl Default for ClientSegmentReassembler {
    fn default() -> Self {
        Self {
            assemblies: HashMap::new(),
            buffered_bytes: 0,
            max_assemblies: REMOTE_CONTROL_SEGMENT_ASSEMBLY_MAX_COUNT,
            max_buffered_bytes: REMOTE_CONTROL_SEGMENT_ASSEMBLY_MAX_BUFFERED_BYTES,
        }
    }
}

pub(crate) enum ClientSegmentObservation {
    Forward(Box<ClientEnvelope>),
    Pending,
    Dropped,
}

impl ClientSegmentReassembler {
    #[cfg(test)]
    pub(crate) fn with_limits(max_assemblies: usize, max_buffered_bytes: usize) -> Self {
        Self {
            assemblies: HashMap::new(),
            buffered_bytes: 0,
            max_assemblies,
            max_buffered_bytes,
        }
    }

    #[cfg(test)]
    pub(crate) fn assembly_count(&self) -> usize {
        self.assemblies.len()
    }

    #[cfg(test)]
    pub(crate) fn buffered_bytes(&self) -> usize {
        self.buffered_bytes
    }

    pub(crate) fn observe(&mut self, envelope: ClientEnvelope) -> ClientSegmentObservation {
        let ClientEvent::ClientMessageChunk {
            segment_id,
            segment_count,
            message_size_bytes,
            message_chunk_base64,
        } = &envelope.event
        else {
            return ClientSegmentObservation::Forward(Box::new(envelope));
        };
        let segment_id = *segment_id;
        let segment_count = *segment_count;
        let message_size_bytes = *message_size_bytes;
        let Some(stream_id) = envelope.stream_id.clone() else {
            warn!(client_id = envelope.client_id.0, "dropping segmented remote-control envelope without stream_id");
            return ClientSegmentObservation::Dropped;
        };
        let Some(seq_id) = envelope.seq_id else {
            warn!(client_id = envelope.client_id.0, "dropping segmented remote-control envelope without seq_id");
            return ClientSegmentObservation::Dropped;
        };
        if self.should_ignore_chunk(&envelope.client_id, &stream_id, seq_id, segment_id) {
            return ClientSegmentObservation::Dropped;
        }
        if segment_count == 0
            || segment_count > REMOTE_CONTROL_SEGMENT_COUNT_MAX
            || segment_id >= segment_count
            || message_size_bytes == 0
            || message_size_bytes > REMOTE_CONTROL_REASSEMBLED_MAX_BYTES
            || message_chunk_base64.is_empty()
            || message_chunk_base64.len() > REMOTE_CONTROL_SEGMENT_MAX_BYTES
        {
            self.remove_assembly(&envelope.client_id, &stream_id);
            return ClientSegmentObservation::Dropped;
        }
        let chunk = match base64::engine::general_purpose::STANDARD.decode(message_chunk_base64) {
            Ok(chunk) if !chunk.is_empty() && chunk.len() <= REMOTE_CONTROL_SEGMENT_MAX_BYTES => {
                chunk
            }
            Ok(_) | Err(_) => {
                self.remove_assembly(&envelope.client_id, &stream_id);
                return ClientSegmentObservation::Dropped;
            }
        };
        let metadata = ClientSegmentMetadata {
            seq_id,
            segment_count,
            message_size_bytes,
        };
        if chunk.len() > self.max_buffered_bytes {
            return ClientSegmentObservation::Dropped;
        }
        let now = Instant::now();

        if self
            .assemblies
            .get(&envelope.client_id)
            .is_some_and(|assembly| assembly.stream_id != stream_id)
        {
            self.remove_client(&envelope.client_id);
        }
        if !self.assemblies.contains_key(&envelope.client_id) {
            self.evict_for_assembly_slot();
            if self.max_assemblies == 0 || self.assemblies.len() >= self.max_assemblies {
                return ClientSegmentObservation::Dropped;
            }
            self.assemblies.insert(
                envelope.client_id.clone(),
                ClientSegmentAssembly {
                    stream_id: stream_id.clone(),
                    metadata: metadata.clone(),
                    raw: Vec::new(),
                    next_segment_id: 0,
                    last_chunk_seen_at: now,
                },
            );
        }

        let update = match self.assemblies.get(&envelope.client_id) {
            Some(assembly) if metadata.seq_id < assembly.metadata.seq_id => AssemblyUpdate::Ignore,
            Some(assembly) if assembly.metadata != metadata => AssemblyUpdate::Drop,
            Some(assembly) if segment_id < assembly.next_segment_id => AssemblyUpdate::Ignore,
            Some(assembly) if segment_id != assembly.next_segment_id => AssemblyUpdate::Drop,
            Some(assembly)
                if assembly.raw.len().saturating_add(chunk.len()) > message_size_bytes =>
            {
                AssemblyUpdate::Drop
            }
            Some(_) => AssemblyUpdate::Append,
            None => AssemblyUpdate::Drop,
        };
        match update {
            AssemblyUpdate::Ignore => return ClientSegmentObservation::Dropped,
            AssemblyUpdate::Drop => {
                self.remove_assembly(&envelope.client_id, &stream_id);
                return ClientSegmentObservation::Dropped;
            }
            AssemblyUpdate::Append => {}
            AssemblyUpdate::Complete(_) => unreachable!("completion follows append"),
        }

        if !self.make_buffer_room(chunk.len(), &envelope.client_id) {
            self.remove_assembly(&envelope.client_id, &stream_id);
            return ClientSegmentObservation::Dropped;
        }
        let result = {
            let Some(assembly) = self.assemblies.get_mut(&envelope.client_id) else {
                return ClientSegmentObservation::Dropped;
            };
            assembly.raw.extend_from_slice(&chunk);
            assembly.next_segment_id += 1;
            assembly.last_chunk_seen_at = now;
            self.buffered_bytes += chunk.len();
            if assembly.next_segment_id < segment_count {
                AssemblyUpdate::Append
            } else if assembly.raw.len() != message_size_bytes {
                AssemblyUpdate::Drop
            } else {
                match serde_json::from_slice::<JSONRPCMessage>(&assembly.raw) {
                    Ok(message) => AssemblyUpdate::Complete(message),
                    Err(_) => AssemblyUpdate::Drop,
                }
            }
        };

        match result {
            AssemblyUpdate::Append => ClientSegmentObservation::Pending,
            AssemblyUpdate::Drop => {
                self.remove_assembly(&envelope.client_id, &stream_id);
                ClientSegmentObservation::Dropped
            }
            AssemblyUpdate::Complete(message) => {
                self.remove_assembly(&envelope.client_id, &stream_id);
                ClientSegmentObservation::Forward(Box::new(ClientEnvelope {
                    event: ClientEvent::ClientMessage { message },
                    ..envelope
                }))
            }
            AssemblyUpdate::Ignore => ClientSegmentObservation::Dropped,
        }
    }

    pub(crate) fn invalidate_stream(&mut self, client_id: &ClientId, stream_id: &StreamId) {
        self.remove_assembly(client_id, stream_id);
    }

    pub(crate) fn invalidate_client(&mut self, client_id: &ClientId) {
        self.remove_client(client_id);
    }

    pub(crate) fn should_ignore_chunk(
        &self,
        client_id: &ClientId,
        stream_id: &StreamId,
        seq_id: u64,
        segment_id: usize,
    ) -> bool {
        self.assemblies.get(client_id).is_some_and(|assembly| {
            assembly.stream_id == *stream_id
                && (seq_id < assembly.metadata.seq_id
                    || (seq_id == assembly.metadata.seq_id
                        && segment_id < assembly.next_segment_id))
        })
    }

    fn make_buffer_room(&mut self, additional_bytes: usize, protected_client: &ClientId) -> bool {
        if additional_bytes > self.max_buffered_bytes {
            return false;
        }
        while self.buffered_bytes.saturating_add(additional_bytes) > self.max_buffered_bytes {
            let Some(client_id) = self.oldest_client(Some(protected_client)) else {
                return false;
            };
            self.remove_client(&client_id);
        }
        true
    }

    fn evict_for_assembly_slot(&mut self) {
        while self.max_assemblies > 0 && self.assemblies.len() >= self.max_assemblies {
            let Some(client_id) = self.oldest_client(None) else {
                return;
            };
            self.remove_client(&client_id);
        }
    }

    fn oldest_client(&self, excluded: Option<&ClientId>) -> Option<ClientId> {
        self.assemblies
            .iter()
            .filter(|(client_id, _)| excluded != Some(*client_id))
            .min_by_key(|(_, assembly)| assembly.last_chunk_seen_at)
            .map(|(client_id, _)| client_id.clone())
    }

    fn remove_assembly(&mut self, client_id: &ClientId, stream_id: &StreamId) {
        if self
            .assemblies
            .get(client_id)
            .is_some_and(|assembly| &assembly.stream_id == stream_id)
        {
            self.remove_client(client_id);
        }
    }

    fn remove_client(&mut self, client_id: &ClientId) {
        if let Some(assembly) = self.assemblies.remove(client_id) {
            self.buffered_bytes = self.buffered_bytes.saturating_sub(assembly.raw.len());
        }
    }
}

enum AssemblyUpdate {
    Append,
    Ignore,
    Drop,
    Complete(JSONRPCMessage),
}

pub(crate) fn split_server_envelope_for_transport(
    envelope: ServerEnvelope,
) -> io::Result<Vec<ServerEnvelope>> {
    if !matches!(envelope.event, ServerEvent::ServerMessage { .. }) {
        return Ok(vec![envelope]);
    }
    if serialized_len(&envelope)? <= REMOTE_CONTROL_SEGMENT_MAX_BYTES {
        return Ok(vec![envelope]);
    }

    let ServerEvent::ServerMessage { message } = &envelope.event else {
        unreachable!("server message variant checked above");
    };
    let raw = serde_json::to_vec(message.as_ref()).map_err(io::Error::other)?;
    let message_size_bytes = raw.len();
    if message_size_bytes > REMOTE_CONTROL_REASSEMBLED_MAX_BYTES {
        warn!("dropping remote-control server envelope that exceeds reassembled size limit");
        return Ok(Vec::new());
    }
    let minimal_segment_count = usize::min(
        message_size_bytes.max(1),
        REMOTE_CONTROL_SEGMENT_COUNT_MAX,
    );
    let minimal_chunk = &raw[..usize::min(raw.len(), 1)];
    if serialized_chunk_len(
        &envelope,
        0,
        minimal_segment_count,
        message_size_bytes,
        minimal_chunk,
    )? > REMOTE_CONTROL_SEGMENT_MAX_BYTES
    {
        warn!("dropping remote-control server envelope that cannot fit a wire segment");
        return Ok(Vec::new());
    }

    let mut segment_count = usize::max(
        2,
        message_size_bytes.div_ceil(REMOTE_CONTROL_SEGMENT_TARGET_BYTES),
    );
    loop {
        if segment_count > REMOTE_CONTROL_SEGMENT_COUNT_MAX {
            warn!("dropping remote-control server envelope that exceeds segment count limit");
            return Ok(Vec::new());
        }
        let chunk_size = usize::max(1, message_size_bytes.div_ceil(segment_count));
        segment_count = message_size_bytes.div_ceil(chunk_size);
        let segments_fit = raw
            .chunks(chunk_size)
            .enumerate()
            .all(|(segment_id, chunk)| {
                serialized_chunk_len(
                    &envelope,
                    segment_id,
                    segment_count,
                    message_size_bytes,
                    chunk,
                )
                .is_ok_and(|size| size <= REMOTE_CONTROL_SEGMENT_MAX_BYTES)
            });
        if segments_fit {
            return raw
                .chunks(chunk_size)
                .enumerate()
                .map(|(segment_id, chunk)| {
                    build_chunk_envelope(
                        &envelope,
                        segment_id,
                        segment_count,
                        message_size_bytes,
                        chunk,
                    )
                })
                .collect();
        }
        if chunk_size == 1 {
            return Ok(Vec::new());
        }
        let next_segment_count = segment_count + 1;
        let next_chunk_size = usize::max(1, message_size_bytes.div_ceil(next_segment_count));
        segment_count = if next_chunk_size == chunk_size {
            message_size_bytes
        } else {
            next_segment_count
        };
    }
}

fn serialized_chunk_len(
    envelope: &ServerEnvelope,
    segment_id: usize,
    segment_count: usize,
    message_size_bytes: usize,
    chunk: &[u8],
) -> io::Result<usize> {
    serialized_len(&build_chunk_envelope(
        envelope,
        segment_id,
        segment_count,
        message_size_bytes,
        chunk,
    )?)
}

fn build_chunk_envelope(
    envelope: &ServerEnvelope,
    segment_id: usize,
    segment_count: usize,
    message_size_bytes: usize,
    chunk: &[u8],
) -> io::Result<ServerEnvelope> {
    if segment_count > REMOTE_CONTROL_SEGMENT_COUNT_MAX {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "remote-control segment count exceeds maximum",
        ));
    }
    Ok(ServerEnvelope {
        event: ServerEvent::ServerMessageChunk {
            segment_id,
            segment_count,
            message_size_bytes,
            message_chunk_base64: base64::engine::general_purpose::STANDARD.encode(chunk),
        },
        client_id: envelope.client_id.clone(),
        stream_id: envelope.stream_id.clone(),
        seq_id: envelope.seq_id,
    })
}

fn serialized_len(value: &impl serde::Serialize) -> io::Result<usize> {
    let mut writer = CountingWriter::default();
    serde_json::to_writer(&mut writer, value).map_err(io::Error::other)?;
    Ok(writer.len)
}

#[derive(Default)]
struct CountingWriter {
    len: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.len += buffer.len();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
