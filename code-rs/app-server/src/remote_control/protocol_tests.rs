use super::protocol::ClientEnvelope;
use super::protocol::ServerEnvelope;
use super::protocol::RemoteControlTarget;
use super::protocol::normalize_remote_control_url;
use serde_json::json;
use std::io::ErrorKind;

#[test]
fn normalizes_chatgpt_and_staging_urls() {
    assert_eq!(
        normalize_remote_control_url("https://chatgpt.com/backend-api")
            .expect("normalize production URL"),
        RemoteControlTarget {
            websocket_url: "wss://chatgpt.com/backend-api/wham/remote/control/server".to_string(),
            enroll_url: "https://chatgpt.com/backend-api/wham/remote/control/server/enroll"
                .to_string(),
            refresh_url: "https://chatgpt.com/backend-api/wham/remote/control/server/refresh"
                .to_string(),
            pair_url: "https://chatgpt.com/backend-api/wham/remote/control/server/pair"
                .to_string(),
            pair_status_url:
                "https://chatgpt.com/backend-api/wham/remote/control/server/pair/status"
                    .to_string(),
        }
    );
    assert_eq!(
        normalize_remote_control_url("https://api.chatgpt-staging.com/backend-api/")
            .expect("normalize staging URL")
            .websocket_url,
        "wss://api.chatgpt-staging.com/backend-api/wham/remote/control/server"
    );
}

#[test]
fn normalizes_localhost_urls_and_pre_normalized_paths() {
    assert_eq!(
        normalize_remote_control_url("http://localhost:8080/backend-api")
            .expect("normalize localhost HTTP URL")
            .websocket_url,
        "ws://localhost:8080/backend-api/wham/remote/control/server"
    );
    assert_eq!(
        normalize_remote_control_url("https://127.0.0.1:8443/backend-api/")
            .expect("normalize localhost HTTPS URL")
            .websocket_url,
        "wss://127.0.0.1:8443/backend-api/wham/remote/control/server"
    );
    assert_eq!(
        normalize_remote_control_url(
            "https://chatgpt.com/backend-api/wham/remote/control/server"
        )
        .expect("normalize pre-normalized URL")
        .websocket_url,
        "wss://chatgpt.com/backend-api/wham/remote/control/server"
    );
}

#[test]
fn rejects_insecure_or_lookalike_production_hosts() {
    for url in [
        "http://chatgpt.com/backend-api",
        "https://chatgpt.com.evil.test/backend-api",
        "https://evilchatgpt.com/backend-api",
        "https://example.com/backend-api",
        "https://foo.localhost/backend-api",
    ] {
        let error = normalize_remote_control_url(url).expect_err("URL must be rejected");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
}

#[test]
fn client_envelope_variants_match_the_hosted_relay_wire_format() {
    for fixture in [
        json!({
            "type": "client_message",
            "message": {"jsonrpc": "2.0", "method": "initialized"},
            "client_id": "client-1",
            "stream_id": "stream-1",
            "seq_id": 7,
            "cursor": "cursor-1"
        }),
        json!({
            "type": "client_message_chunk",
            "segment_id": 0,
            "segment_count": 2,
            "message_size_bytes": 123,
            "message_chunk_base64": "e30=",
            "client_id": "client-1",
            "stream_id": "stream-1",
            "seq_id": 8
        }),
        json!({
            "type": "ack",
            "segment_id": 1,
            "client_id": "client-1",
            "stream_id": "stream-1",
            "seq_id": 9
        }),
        json!({"type": "ping", "client_id": "client-1"}),
        json!({
            "type": "client_closed",
            "client_id": "client-1",
            "stream_id": "stream-1"
        }),
    ] {
        let envelope: ClientEnvelope =
            serde_json::from_value(fixture.clone()).expect("fixture must deserialize");
        assert_eq!(
            serde_json::to_value(envelope).expect("envelope must serialize"),
            fixture
        );
    }
}

#[test]
fn server_envelope_variants_match_the_hosted_relay_wire_format() {
    for fixture in [
        json!({
            "type": "server_message",
            "message": {"jsonrpc": "2.0", "id": 3, "result": {"ok": true}},
            "client_id": "client-1",
            "stream_id": "stream-1",
            "seq_id": 10
        }),
        json!({
            "type": "server_message_chunk",
            "segment_id": 0,
            "segment_count": 2,
            "message_size_bytes": 123,
            "message_chunk_base64": "e30=",
            "client_id": "client-1",
            "stream_id": "stream-1",
            "seq_id": 11
        }),
        json!({
            "type": "ack",
            "client_id": "client-1",
            "stream_id": "stream-1",
            "seq_id": 12
        }),
        json!({
            "type": "pong",
            "status": "active",
            "client_id": "client-1",
            "stream_id": "stream-1",
            "seq_id": 13
        }),
    ] {
        let envelope: ServerEnvelope =
            serde_json::from_value(fixture.clone()).expect("fixture must deserialize");
        assert_eq!(
            serde_json::to_value(envelope).expect("envelope must serialize"),
            fixture
        );
    }
}
