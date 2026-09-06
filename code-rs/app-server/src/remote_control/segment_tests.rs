use super::protocol::ClientEnvelope;
use super::protocol::ClientEvent;
use super::protocol::ClientId;
use super::protocol::ServerEnvelope;
use super::protocol::ServerEvent;
use super::protocol::StreamId;
use super::segment::ClientSegmentObservation;
use super::segment::ClientSegmentReassembler;
use super::segment::REMOTE_CONTROL_REASSEMBLED_MAX_BYTES;
use super::segment::REMOTE_CONTROL_SEGMENT_COUNT_MAX;
use super::segment::REMOTE_CONTROL_SEGMENT_MAX_BYTES;
use super::segment::REMOTE_CONTROL_SEGMENT_TARGET_BYTES;
use super::segment::split_server_envelope_for_transport;
use base64::Engine;
use mcp_types::JSONRPCMessage;
use serde_json::json;

#[test]
fn reassembles_ordered_client_chunks_and_releases_the_buffer() {
    let message = rpc_message(json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {"text": "hello"}
    }));
    let raw = serde_json::to_vec(&message).expect("serialize message");
    let split = raw.len() / 2;
    let mut reassembler = ClientSegmentReassembler::with_limits(4, 1024);

    assert!(matches!(
        reassembler.observe(chunk_envelope(0, 2, raw.len(), &raw[..split])),
        ClientSegmentObservation::Pending
    ));
    assert_eq!(reassembler.assembly_count(), 1);
    assert_eq!(reassembler.buffered_bytes(), split);

    let envelope = match reassembler.observe(chunk_envelope(1, 2, raw.len(), &raw[split..])) {
        ClientSegmentObservation::Forward(envelope) => *envelope,
        ClientSegmentObservation::Pending | ClientSegmentObservation::Dropped => {
            panic!("complete message must be forwarded")
        }
    };
    assert_eq!(envelope.seq_id, Some(7));
    assert_eq!(envelope.stream_id, Some(StreamId("stream-1".to_string())));
    assert_eq!(reassembler.assembly_count(), 0);
    assert_eq!(reassembler.buffered_bytes(), 0);
    match envelope.event {
        ClientEvent::ClientMessage {
            message: reassembled,
        } => assert_eq!(reassembled, message),
        event => panic!("expected client message, got {event:?}"),
    }
}

#[test]
fn rejects_malformed_sequences_without_corrupting_a_valid_assembly() {
    let message = rpc_message(json!({"jsonrpc": "2.0", "method": "initialized"}));
    let raw = serde_json::to_vec(&message).expect("serialize message");
    let split = raw.len() / 2;
    let mut reassembler = ClientSegmentReassembler::with_limits(4, 1024);

    assert!(matches!(
        reassembler.observe(chunk_envelope(1, 2, raw.len(), &raw[split..])),
        ClientSegmentObservation::Dropped
    ));
    assert!(matches!(
        reassembler.observe(chunk_envelope(0, 2, raw.len(), &raw[..split])),
        ClientSegmentObservation::Pending
    ));
    assert!(matches!(
        reassembler.observe(chunk_envelope(0, 2, raw.len(), b"not-the-original")),
        ClientSegmentObservation::Dropped
    ));
    assert_eq!(reassembler.assembly_count(), 1);
    assert!(matches!(
        reassembler.observe(chunk_envelope(1, 3, raw.len(), &raw[split..])),
        ClientSegmentObservation::Dropped
    ));
    assert_eq!(reassembler.assembly_count(), 0);

    let mut missing_stream = chunk_envelope(0, 2, raw.len(), &raw[..split]);
    missing_stream.stream_id = None;
    assert!(matches!(
        reassembler.observe(missing_stream),
        ClientSegmentObservation::Dropped
    ));
    let mut missing_sequence = chunk_envelope(0, 2, raw.len(), &raw[..split]);
    missing_sequence.seq_id = None;
    assert!(matches!(
        reassembler.observe(missing_sequence),
        ClientSegmentObservation::Dropped
    ));
    assert!(matches!(
        reassembler.observe(chunk_envelope(0, 2, raw.len(), b"")),
        ClientSegmentObservation::Dropped
    ));
    assert!(matches!(
        reassembler.observe(chunk_envelope(
            0,
            2,
            REMOTE_CONTROL_REASSEMBLED_MAX_BYTES + 1,
            &raw[..split],
        )),
        ClientSegmentObservation::Dropped
    ));
    let mut invalid_base64 = chunk_envelope(0, 2, raw.len(), &raw[..split]);
    if let ClientEvent::ClientMessageChunk {
        message_chunk_base64,
        ..
    } = &mut invalid_base64.event
    {
        *message_chunk_base64 = "%%%".to_string();
    }
    assert!(matches!(
        reassembler.observe(invalid_base64),
        ClientSegmentObservation::Dropped
    ));
}

#[test]
fn enforces_concurrent_assembly_and_total_buffer_limits() {
    let mut reassembler = ClientSegmentReassembler::with_limits(2, 8);
    for (client, chunk) in [("client-1", b"1234"), ("client-2", b"5678")] {
        assert!(matches!(
            reassembler.observe(chunk_for(client, "stream-1", 0, 2, 8, chunk)),
            ClientSegmentObservation::Pending
        ));
    }
    assert_eq!(reassembler.assembly_count(), 2);
    assert_eq!(reassembler.buffered_bytes(), 8);

    assert!(matches!(
        reassembler.observe(chunk_for("client-3", "stream-1", 0, 2, 8, b"abcd")),
        ClientSegmentObservation::Pending
    ));
    assert_eq!(reassembler.assembly_count(), 2);
    assert!(reassembler.buffered_bytes() <= 8);
    assert!(matches!(
        reassembler.observe(chunk_for("client-3", "stream-1", 1, 2, 8, b"efgh")),
        ClientSegmentObservation::Dropped
    ));
    assert_eq!(reassembler.buffered_bytes(), 0);
}

#[test]
fn accepts_limit_metadata_and_rejects_values_above_the_limits() {
    let mut reassembler = ClientSegmentReassembler::with_limits(2, 16);
    assert!(matches!(
        reassembler.observe(chunk_envelope(
            0,
            REMOTE_CONTROL_SEGMENT_COUNT_MAX,
            REMOTE_CONTROL_REASSEMBLED_MAX_BYTES,
            b"x",
        )),
        ClientSegmentObservation::Pending
    ));
    reassembler.invalidate_client(&ClientId("client-1".to_string()));
    assert_eq!(reassembler.buffered_bytes(), 0);

    assert!(matches!(
        reassembler.observe(chunk_envelope(
            0,
            REMOTE_CONTROL_SEGMENT_COUNT_MAX + 1,
            1,
            b"x",
        )),
        ClientSegmentObservation::Dropped
    ));
    assert!(matches!(
        reassembler.observe(chunk_envelope(
            0,
            1,
            REMOTE_CONTROL_REASSEMBLED_MAX_BYTES + 1,
            b"x",
        )),
        ClientSegmentObservation::Dropped
    ));
}

#[test]
fn leaves_small_server_messages_unsegmented() {
    let message = rpc_message(json!({
        "jsonrpc": "2.0",
        "method": "example/event",
        "params": {"text": "x"}
    }));
    let envelope = ServerEnvelope {
        event: ServerEvent::ServerMessage {
            message: Box::new(message.clone()),
        },
        client_id: ClientId("client-1".to_string()),
        stream_id: StreamId("stream-1".to_string()),
        seq_id: 9,
    };

    let segments = split_server_envelope_for_transport(envelope).expect("split envelope");
    assert_eq!(segments.len(), 1);
    match &segments[0].event {
        ServerEvent::ServerMessage { message: forwarded } => {
            assert_eq!(forwarded.as_ref(), &message);
        }
        event => panic!("expected unsegmented message, got {event:?}"),
    }
}

#[test]
fn target_sized_server_message_remains_within_the_wire_limit() {
    let message = rpc_message(json!({
        "jsonrpc": "2.0",
        "method": "example/event",
        "params": {"text": "x".repeat(REMOTE_CONTROL_SEGMENT_TARGET_BYTES)}
    }));
    let envelope = ServerEnvelope {
        event: ServerEvent::ServerMessage {
            message: Box::new(message),
        },
        client_id: ClientId("client-1".to_string()),
        stream_id: StreamId("stream-1".to_string()),
        seq_id: 9,
    };

    let segments = split_server_envelope_for_transport(envelope).expect("split envelope");
    assert_eq!(segments.len(), 1);
    assert!(
        serde_json::to_vec(&segments[0])
            .expect("serialize envelope")
            .len()
            <= REMOTE_CONTROL_SEGMENT_MAX_BYTES
    );
}

#[test]
fn splits_large_unicode_server_messages_with_bounded_wire_segments() {
    assert_eq!(REMOTE_CONTROL_SEGMENT_TARGET_BYTES, 100 * 1024);
    assert_eq!(REMOTE_CONTROL_SEGMENT_MAX_BYTES, 150 * 1024);
    assert_eq!(REMOTE_CONTROL_SEGMENT_COUNT_MAX, 1024);
    let message = rpc_message(json!({
        "jsonrpc": "2.0",
        "method": "example/event",
        "params": {"text": format!("snowman {}", '\u{2603}').repeat(30_000)}
    }));
    let expected = serde_json::to_vec(&message).expect("serialize message");
    let envelope = ServerEnvelope {
        event: ServerEvent::ServerMessage {
            message: Box::new(message),
        },
        client_id: ClientId("client-1".to_string()),
        stream_id: StreamId("stream-1".to_string()),
        seq_id: 9,
    };

    let segments = split_server_envelope_for_transport(envelope).expect("split envelope");
    assert!(segments.len() > 1);
    assert!(segments.len() <= REMOTE_CONTROL_SEGMENT_COUNT_MAX);
    let mut reassembled = Vec::new();
    for (expected_segment_id, segment) in segments.iter().enumerate() {
        assert!(
            serde_json::to_vec(segment)
                .expect("serialize segment")
                .len()
                <= REMOTE_CONTROL_SEGMENT_MAX_BYTES
        );
        match &segment.event {
            ServerEvent::ServerMessageChunk {
                segment_id,
                segment_count,
                message_size_bytes,
                message_chunk_base64,
            } => {
                assert_eq!(*segment_id, expected_segment_id);
                assert_eq!(*segment_count, segments.len());
                assert_eq!(*message_size_bytes, expected.len());
                reassembled.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(message_chunk_base64)
                        .expect("decode chunk"),
                );
            }
            event => panic!("expected chunk, got {event:?}"),
        }
    }
    assert_eq!(reassembled, expected);
}

fn rpc_message(value: serde_json::Value) -> JSONRPCMessage {
    serde_json::from_value(value).expect("parse JSON-RPC message")
}

fn chunk_envelope(
    segment_id: usize,
    segment_count: usize,
    message_size_bytes: usize,
    chunk: &[u8],
) -> ClientEnvelope {
    chunk_for(
        "client-1",
        "stream-1",
        segment_id,
        segment_count,
        message_size_bytes,
        chunk,
    )
}

fn chunk_for(
    client_id: &str,
    stream_id: &str,
    segment_id: usize,
    segment_count: usize,
    message_size_bytes: usize,
    chunk: &[u8],
) -> ClientEnvelope {
    ClientEnvelope {
        event: ClientEvent::ClientMessageChunk {
            segment_id,
            segment_count,
            message_size_bytes,
            message_chunk_base64: base64::engine::general_purpose::STANDARD.encode(chunk),
        },
        client_id: ClientId(client_id.to_string()),
        stream_id: Some(StreamId(stream_id.to_string())),
        seq_id: Some(7),
        cursor: None,
    }
}
