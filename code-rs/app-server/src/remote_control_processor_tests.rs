use crate::error_code::INTERNAL_ERROR_CODE;
use crate::error_code::INVALID_REQUEST_ERROR_CODE;
use crate::message_processor::ConnectionSessionState;
use crate::message_processor::MessageProcessor;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingEnvelope;
use crate::outgoing_message::OutgoingMessage;
use crate::outgoing_message::OutgoingMessageSender;
use crate::outgoing_message::OutgoingNotification;
use crate::remote_control::RemoteControlPolicy;
use crate::remote_control::RemoteControlStartConfig;
use crate::remote_control::RemoteControlStartupMode;
use crate::remote_control::auth::RemoteControlAuth;
use crate::remote_control::auth::RemoteControlAuthProvider;
use crate::remote_control::client_tracker::ClientTracker;
use crate::remote_control::client_tracker::QueuedServerEnvelope;
use crate::remote_control::host_device::HostDevice;
use crate::remote_control::protocol::ClientEnvelope;
use crate::remote_control::protocol::ClientEvent;
use crate::remote_control::protocol::ClientId;
use crate::remote_control::protocol::ServerEvent;
use crate::remote_control::protocol::StreamId;
use crate::remote_control::protocol::normalize_remote_control_url;
use crate::remote_control::start_remote_control;
use crate::remote_control::state::RemoteControlEnrollmentRecord;
use crate::remote_control::state::RemoteControlState;
use crate::remote_control_processor::RemoteControlRequestProcessor;
use crate::remote_control_processor::map_client_management_error;
use crate::remote_control_processor::map_pairing_error;
use crate::remote_control_processor::validate_pairing_status_params;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::OutboundConnectionState;
use crate::transport::TransportEvent;
use crate::transport::route_outgoing_envelope;
use async_trait::async_trait;
use code_app_server_protocol::RemoteControlPairingStartParams;
use code_app_server_protocol::RemoteControlPairingStatusParams;
use code_core::config::ConfigBuilder;
use mcp_types::JSONRPCRequest;
use mcp_types::RequestId;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

struct TestAuthProvider {
    auth: RemoteControlAuth,
    change_tx: watch::Sender<u64>,
}

impl TestAuthProvider {
    fn new() -> Self {
        let (change_tx, _) = watch::channel(0);
        Self {
            auth: RemoteControlAuth::for_testing("access-token", "account-a"),
            change_tx,
        }
    }
}

#[async_trait]
impl RemoteControlAuthProvider for TestAuthProvider {
    async fn load(&self) -> io::Result<RemoteControlAuth> {
        Ok(self.auth.clone())
    }

    async fn recover_unauthorized(&self) -> io::Result<bool> {
        Ok(false)
    }

    fn subscribe(&self) -> watch::Receiver<u64> {
        self.change_tx.subscribe()
    }
}

#[tokio::test]
async fn absent_remote_control_handle_is_an_internal_error() {
    let processor = RemoteControlRequestProcessor::new(None);
    let error = processor
        .pairing_start(RemoteControlPairingStartParams::default(), Some("desktop"))
        .await
        .expect_err("missing handle must fail");

    assert_eq!(error.code, INTERNAL_ERROR_CODE);
    assert_eq!(error.message, "remote control is unavailable for this app-server");
}

#[test]
fn pairing_status_requires_exactly_one_code() {
    for params in [
        RemoteControlPairingStatusParams {
            pairing_code: None,
            manual_pairing_code: None,
        },
        RemoteControlPairingStatusParams {
            pairing_code: Some("pair-a".to_string()),
            manual_pairing_code: Some("ABCD-EFGH".to_string()),
        },
    ] {
        let error = validate_pairing_status_params(&params).expect_err("invalid code selection");
        assert_eq!(error.code, INVALID_REQUEST_ERROR_CODE);
    }
}

#[test]
fn remote_control_errors_map_by_user_actionability() {
    assert_eq!(
        map_pairing_error(io::Error::new(io::ErrorKind::InvalidInput, "invalid pairing")).code,
        INVALID_REQUEST_ERROR_CODE,
    );
    assert_eq!(
        map_pairing_error(io::Error::other("backend failed")).code,
        INTERNAL_ERROR_CODE,
    );
    for kind in [
        io::ErrorKind::InvalidInput,
        io::ErrorKind::NotFound,
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::WouldBlock,
    ] {
        assert_eq!(
            map_client_management_error(io::Error::new(kind, "client unavailable")).code,
            INVALID_REQUEST_ERROR_CODE,
        );
    }
}

#[tokio::test]
async fn serialized_remote_control_requests_require_initialize_and_reach_every_dispatch_arm() {
    let (mut processor, mut outgoing_rx) = test_message_processor(None).await;
    let mut session = ConnectionSessionState::default();
    let initialized = AtomicBool::new(false);
    let opted_out = RwLock::new(HashSet::new());

    process_json(
        &mut processor,
        &mut session,
        &initialized,
        &opted_out,
        json!({"jsonrpc":"2.0","id":1,"method":"remoteControl/status/read"}),
    )
    .await;
    assert_error(
        outgoing_rx.recv().await.expect("pre-initialize response"),
        1,
        INVALID_REQUEST_ERROR_CODE,
        "Not initialized",
    );

    initialize(
        &mut processor,
        &mut outgoing_rx,
        &mut session,
        &initialized,
        &opted_out,
    )
    .await;
    assert_eq!(session.app_server_client_name.as_deref(), Some("desktop"));

    let requests = [
        json!({"jsonrpc":"2.0","id":10,"method":"remoteControl/enable","params":{"ephemeral":true}}),
        json!({"jsonrpc":"2.0","id":11,"method":"remoteControl/disable","params":{"ephemeral":true}}),
        json!({"jsonrpc":"2.0","id":12,"method":"remoteControl/status/read"}),
        json!({"jsonrpc":"2.0","id":13,"method":"remoteControl/pairing/start","params":{"manualCode":false}}),
        json!({"jsonrpc":"2.0","id":14,"method":"remoteControl/pairing/status","params":{"pairingCode":"pair-a"}}),
        json!({"jsonrpc":"2.0","id":15,"method":"remoteControl/client/list","params":{"environmentId":"env-a"}}),
        json!({"jsonrpc":"2.0","id":16,"method":"remoteControl/client/revoke","params":{"environmentId":"env-a","clientId":"client-a"}}),
    ];
    for (offset, request) in requests.into_iter().enumerate() {
        process_json(
            &mut processor,
            &mut session,
            &initialized,
            &opted_out,
            request,
        )
        .await;
        assert_error(
            outgoing_rx.recv().await.expect("remote control response"),
            10 + offset as i64,
            INTERNAL_ERROR_CODE,
            "remote control is unavailable for this app-server",
        );
    }

    process_json(
        &mut processor,
        &mut session,
        &initialized,
        &opted_out,
        json!({"jsonrpc":"2.0","id":17,"method":"remoteControl/pairing/status","params":{}}),
    )
    .await;
    assert_error(
        outgoing_rx.recv().await.expect("invalid pairing response"),
        17,
        INVALID_REQUEST_ERROR_CODE,
        "requires pairingCode or manualPairingCode",
    );
}

#[tokio::test]
async fn serialized_ephemeral_flags_and_client_name_persistence_are_honored() {
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let target = normalize_remote_control_url("http://127.0.0.1:9/backend-api")
        .expect("normalize target");
    state
        .upsert_enrollment(&RemoteControlEnrollmentRecord {
            websocket_url: target.websocket_url.clone(),
            account_id: "account-a".to_string(),
            app_server_client_name: Some("desktop".to_string()),
            server_id: "server-a".to_string(),
            environment_id: "env-a".to_string(),
            server_name: "deck".to_string(),
            remote_control_enabled: Some(true),
        })
        .await
        .expect("persist enrollment");
    let shutdown = CancellationToken::new();
    let (transport_tx, _transport_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (remote_task, remote_handle) = start_remote_control(
        RemoteControlStartConfig {
            remote_control_url: "http://127.0.0.1:9/backend-api".to_string(),
            installation_id: "installation-a".to_string(),
            host: HostDevice::detect("deck".to_string()),
            policy: RemoteControlPolicy::Allowed,
        },
        Some(state.clone()),
        Arc::new(TestAuthProvider::new()),
        transport_tx,
        shutdown.clone(),
        RemoteControlStartupMode::DisabledEphemeral,
    )
    .await
    .expect("start remote control");
    let (mut processor, mut outgoing_rx) = test_message_processor(Some(remote_handle)).await;
    let mut session = ConnectionSessionState::default();
    let initialized = AtomicBool::new(false);
    let opted_out = RwLock::new(HashSet::new());
    initialize(
        &mut processor,
        &mut outgoing_rx,
        &mut session,
        &initialized,
        &opted_out,
    )
    .await;

    process_json(
        &mut processor,
        &mut session,
        &initialized,
        &opted_out,
        json!({"jsonrpc":"2.0","id":20,"method":"remoteControl/enable","params":{"ephemeral":true}}),
    )
    .await;
    assert_response_status(
        outgoing_rx.recv().await.expect("ephemeral enable response"),
        20,
        "connecting",
    );
    assert_eq!(
        state
            .get_enrollment(&target.websocket_url, "account-a", Some("desktop"))
            .await
            .expect("load enrollment")
            .expect("enrollment exists")
            .remote_control_enabled,
        Some(true),
    );

    process_json(
        &mut processor,
        &mut session,
        &initialized,
        &opted_out,
        json!({"jsonrpc":"2.0","id":21,"method":"remoteControl/disable","params":{"ephemeral":true}}),
    )
    .await;
    assert_response_status(
        outgoing_rx.recv().await.expect("ephemeral disable response"),
        21,
        "disabled",
    );
    assert_eq!(
        state
            .get_enrollment(&target.websocket_url, "account-a", Some("desktop"))
            .await
            .expect("load enrollment")
            .expect("enrollment exists")
            .remote_control_enabled,
        Some(true),
    );

    process_json(
        &mut processor,
        &mut session,
        &initialized,
        &opted_out,
        json!({"jsonrpc":"2.0","id":22,"method":"remoteControl/disable"}),
    )
    .await;
    assert_response_status(
        outgoing_rx.recv().await.expect("persistent disable response"),
        22,
        "disabled",
    );
    assert_eq!(
        state
            .get_enrollment(&target.websocket_url, "account-a", Some("desktop"))
            .await
            .expect("load enrollment")
            .expect("enrollment exists")
            .remote_control_enabled,
        Some(false),
    );

    shutdown.cancel();
    timeout(Duration::from_secs(2), remote_task)
        .await
        .expect("remote task shutdown")
        .expect("join remote task")
        .expect("remote task succeeds");
}

#[tokio::test]
async fn local_and_remote_connections_share_processor_and_initialized_broadcast_path() {
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let shutdown = CancellationToken::new();
    let (remote_transport_tx, _remote_transport_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (remote_task, remote_handle) = start_remote_control(
        RemoteControlStartConfig {
            remote_control_url: "http://127.0.0.1:9/backend-api".to_string(),
            installation_id: "installation-a".to_string(),
            host: HostDevice::detect("deck".to_string()),
            policy: RemoteControlPolicy::Allowed,
        },
        Some(state),
        Arc::new(TestAuthProvider::new()),
        remote_transport_tx,
        shutdown.clone(),
        RemoteControlStartupMode::DisabledEphemeral,
    )
    .await
    .expect("start remote control");
    let status_handle = remote_handle.clone();

    let code_home = tempfile::tempdir().expect("create code home").keep();
    let config = ConfigBuilder::new()
        .with_code_home(code_home)
        .load()
        .expect("load isolated config");
    let (outgoing_tx, mut outgoing_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let outgoing = Arc::new(OutgoingMessageSender::new_with_routed_sender(outgoing_tx));
    let mut processor = MessageProcessor::new_with_remote_control(
        Arc::clone(&outgoing),
        None,
        Arc::new(config),
        Vec::new(),
        Vec::new(),
        Some(remote_handle),
        None,
    );

    let local_connection_id = ConnectionId(700);
    let uninitialized_connection_id = ConnectionId(701);
    let (local_writer_tx, mut local_writer_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (uninitialized_writer_tx, mut uninitialized_writer_rx) =
        mpsc::channel(CHANNEL_CAPACITY);
    let local_initialized = Arc::new(AtomicBool::new(false));
    let local_opted_out = Arc::new(RwLock::new(HashSet::new()));
    let uninitialized = Arc::new(AtomicBool::new(false));
    let uninitialized_opted_out = Arc::new(RwLock::new(HashSet::new()));
    let mut outbound_connections = HashMap::from([
        (
            local_connection_id,
            OutboundConnectionState::new(
                local_writer_tx,
                Arc::clone(&local_initialized),
                Arc::clone(&local_opted_out),
                None,
            ),
        ),
        (
            uninitialized_connection_id,
            OutboundConnectionState::new(
                uninitialized_writer_tx,
                uninitialized,
                uninitialized_opted_out,
                None,
            ),
        ),
    ]);

    let (server_event_tx, mut server_event_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (tracker_transport_tx, mut tracker_transport_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let mut tracker = ClientTracker::new(
        server_event_tx,
        tracker_transport_tx,
        shutdown.clone(),
    );
    tracker
        .handle_envelope(remote_request_envelope(
            1,
            1,
            "initialize",
            Some(json!({
                "clientInfo": {"name": "remote-client", "version": "2.0.0"}
            })),
        ))
        .await
        .expect("open remote connection");
    let (remote_connection_id, remote_writer_tx) = match tracker_transport_rx
        .recv()
        .await
        .expect("remote open event")
    {
        TransportEvent::ConnectionOpened {
            connection_id,
            writer,
            ..
        } => (connection_id, writer),
        event => panic!("expected remote open event, got {event:?}"),
    };
    let remote_initialize = incoming_request(
        tracker_transport_rx
            .recv()
            .await
            .expect("remote initialize event"),
        remote_connection_id,
    );
    let remote_initialized = Arc::new(AtomicBool::new(false));
    let remote_opted_out = Arc::new(RwLock::new(HashSet::new()));
    outbound_connections.insert(
        remote_connection_id,
        OutboundConnectionState::new(
            remote_writer_tx,
            Arc::clone(&remote_initialized),
            Arc::clone(&remote_opted_out),
            None,
        ),
    );

    let mut local_session = ConnectionSessionState::default();
    processor
        .process_request(
            local_connection_id,
            json_request(
                1,
                "initialize",
                Some(json!({
                    "clientInfo": {"name": "local-client", "version": "1.0.0"}
                })),
            ),
            &mut local_session,
            local_initialized.as_ref(),
            local_opted_out.as_ref(),
        )
        .await;
    processor
        .send_initialize_notifications(local_connection_id)
        .await;

    let mut remote_session = ConnectionSessionState::default();
    processor
        .process_request(
            remote_connection_id,
            remote_initialize,
            &mut remote_session,
            remote_initialized.as_ref(),
            remote_opted_out.as_ref(),
        )
        .await;
    processor
        .send_initialize_notifications(remote_connection_id)
        .await;

    for _ in 0..4 {
        route_next_outgoing(&mut outgoing_rx, &mut outbound_connections).await;
    }
    assert_response_id(
        local_writer_rx.recv().await.expect("local initialize response"),
        1,
    );
    assert_status_notification(
        local_writer_rx.recv().await.expect("local initial status"),
    );
    assert_remote_response_id(
        server_event_rx.recv().await.expect("remote initialize response"),
        1,
    );
    assert_remote_status_notification(
        server_event_rx.recv().await.expect("remote initial status"),
    );

    processor
        .process_request(
            local_connection_id,
            json_request(10, "getUserAgent", None),
            &mut local_session,
            local_initialized.as_ref(),
            local_opted_out.as_ref(),
        )
        .await;
    route_next_outgoing(&mut outgoing_rx, &mut outbound_connections).await;
    assert_user_agent_response(
        local_writer_rx.recv().await.expect("local read response"),
        10,
        "local-client; 1.0.0",
    );

    tracker
        .handle_envelope(remote_request_envelope(2, 11, "getUserAgent", None))
        .await
        .expect("forward remote read request");
    let remote_read = incoming_request(
        tracker_transport_rx
            .recv()
            .await
            .expect("remote read event"),
        remote_connection_id,
    );
    processor
        .process_request(
            remote_connection_id,
            remote_read,
            &mut remote_session,
            remote_initialized.as_ref(),
            remote_opted_out.as_ref(),
        )
        .await;
    route_next_outgoing(&mut outgoing_rx, &mut outbound_connections).await;
    assert_remote_user_agent_response(
        server_event_rx.recv().await.expect("remote read response"),
        11,
        "remote-client; 2.0.0",
    );

    let status = status_handle
        .enable_ephemeral()
        .expect("enable ephemeral remote control");
    route_outgoing_envelope(
        &mut outbound_connections,
        OutgoingEnvelope::Broadcast {
            message: OutgoingMessage::Notification(OutgoingNotification {
                method: "remoteControl/status/changed".to_owned(),
                params: Some(serde_json::to_value(status).expect("serialize status")),
            }),
        },
    )
    .await;
    assert_status_notification(local_writer_rx.recv().await.expect("local status broadcast"));
    assert_remote_status_notification(
        server_event_rx.recv().await.expect("remote status broadcast"),
    );
    assert!(
        timeout(Duration::from_millis(25), uninitialized_writer_rx.recv())
            .await
            .is_err(),
        "uninitialized connection must not receive broadcasts",
    );

    tracker.shutdown().await;
    shutdown.cancel();
    timeout(Duration::from_secs(2), remote_task)
        .await
        .expect("remote task shutdown")
        .expect("join remote task")
        .expect("remote task succeeds");
}

async fn test_message_processor(
    remote_control_handle: Option<crate::remote_control::RemoteControlHandle>,
) -> (MessageProcessor, mpsc::Receiver<OutgoingEnvelope>) {
    let code_home = tempfile::tempdir().expect("create code home").keep();
    let config = ConfigBuilder::new()
        .with_code_home(code_home)
        .load()
        .expect("load isolated config");
    let (outgoing_tx, outgoing_rx) = mpsc::channel(32);
    let outgoing = Arc::new(OutgoingMessageSender::new_with_routed_sender(outgoing_tx));
    (
        MessageProcessor::new_with_remote_control(
            outgoing,
            None,
            Arc::new(config),
            Vec::new(),
            Vec::new(),
            remote_control_handle,
            None,
        ),
        outgoing_rx,
    )
}

async fn initialize(
    processor: &mut MessageProcessor,
    outgoing_rx: &mut mpsc::Receiver<OutgoingEnvelope>,
    session: &mut ConnectionSessionState,
    initialized: &AtomicBool,
    opted_out: &RwLock<HashSet<String>>,
) {
    process_json(
        processor,
        session,
        initialized,
        opted_out,
        json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"initialize",
            "params":{
                "clientInfo":{"name":"desktop","version":"1.0.0"},
                "capabilities":{"experimentalApi":true}
            }
        }),
    )
    .await;
    match outgoing_rx.recv().await.expect("initialize response") {
        OutgoingEnvelope::ToConnection {
            connection_id: ConnectionId(7),
            message: OutgoingMessage::Response(response),
        } => assert_eq!(response.id, RequestId::Integer(2)),
        envelope => panic!("expected initialize response, got {envelope:?}"),
    }
}

async fn process_json(
    processor: &mut MessageProcessor,
    session: &mut ConnectionSessionState,
    initialized: &AtomicBool,
    opted_out: &RwLock<HashSet<String>>,
    request: Value,
) {
    let request: JSONRPCRequest = serde_json::from_value(request).expect("parse JSON-RPC request");
    processor
        .process_request(ConnectionId(7), request, session, initialized, opted_out)
        .await;
}

fn assert_error(
    envelope: OutgoingEnvelope,
    id: i64,
    code: i64,
    expected_message: &str,
) {
    match envelope {
        OutgoingEnvelope::ToConnection {
            connection_id: ConnectionId(7),
            message: OutgoingMessage::Error(error),
        } => {
            assert_eq!(error.id, RequestId::Integer(id));
            assert_eq!(error.error.code, code);
            assert!(error.error.message.contains(expected_message));
        }
        envelope => panic!("expected error response, got {envelope:?}"),
    }
}

fn assert_response_status(envelope: OutgoingEnvelope, id: i64, expected_status: &str) {
    match envelope {
        OutgoingEnvelope::ToConnection {
            connection_id: ConnectionId(7),
            message: OutgoingMessage::Response(response),
        } => {
            assert_eq!(response.id, RequestId::Integer(id));
            assert_eq!(
                response.result.get("status").and_then(Value::as_str),
                Some(expected_status),
            );
        }
        envelope => panic!("expected response, got {envelope:?}"),
    }
}

async fn route_next_outgoing(
    outgoing_rx: &mut mpsc::Receiver<OutgoingEnvelope>,
    outbound_connections: &mut HashMap<ConnectionId, OutboundConnectionState>,
) {
    let envelope = outgoing_rx.recv().await.expect("outgoing envelope");
    assert!(
        route_outgoing_envelope(outbound_connections, envelope)
            .await
            .is_empty(),
        "test connections should stay open",
    );
}

fn remote_request_envelope(
    sequence_id: u64,
    request_id: i64,
    method: &str,
    params: Option<Value>,
) -> ClientEnvelope {
    ClientEnvelope {
        event: ClientEvent::ClientMessage {
            message: mcp_types::JSONRPCMessage::Request(json_request(request_id, method, params)),
        },
        client_id: ClientId("remote-client-id".to_owned()),
        stream_id: Some(StreamId("remote-stream-id".to_owned())),
        seq_id: Some(sequence_id),
        cursor: None,
    }
}

fn incoming_request(event: TransportEvent, expected_connection_id: ConnectionId) -> JSONRPCRequest {
    match event {
        TransportEvent::IncomingMessage {
            connection_id,
            message: mcp_types::JSONRPCMessage::Request(request),
        } => {
            assert_eq!(connection_id, expected_connection_id);
            request
        }
        event => panic!("expected incoming request, got {event:?}"),
    }
}

fn json_request(id: i64, method: &str, params: Option<Value>) -> JSONRPCRequest {
    JSONRPCRequest {
        jsonrpc: mcp_types::JSONRPC_VERSION.to_owned(),
        id: RequestId::Integer(id),
        method: method.to_owned(),
        params,
    }
}

fn assert_response_id(message: OutgoingMessage, expected_id: i64) {
    match message {
        OutgoingMessage::Response(response) => {
            assert_eq!(response.id, RequestId::Integer(expected_id));
        }
        message => panic!("expected response, got {message:?}"),
    }
}

fn assert_user_agent_response(message: OutgoingMessage, expected_id: i64, expected_suffix: &str) {
    match message {
        OutgoingMessage::Response(response) => {
            assert_eq!(response.id, RequestId::Integer(expected_id));
            let user_agent = response
                .result
                .get("userAgent")
                .and_then(Value::as_str)
                .expect("user agent response");
            assert!(user_agent.contains(expected_suffix));
        }
        message => panic!("expected response, got {message:?}"),
    }
}

fn assert_status_notification(message: OutgoingMessage) {
    match message {
        OutgoingMessage::Notification(notification) => {
            assert_eq!(notification.method, "remoteControl/status/changed");
        }
        message => panic!("expected status notification, got {message:?}"),
    }
}

fn assert_remote_response_id(envelope: QueuedServerEnvelope, expected_id: i64) {
    match envelope.event {
        ServerEvent::ServerMessage { message } => match *message {
            mcp_types::JSONRPCMessage::Response(response) => {
                assert_eq!(response.id, RequestId::Integer(expected_id));
            }
            message => panic!("expected remote response, got {message:?}"),
        },
        event => panic!("expected remote server message, got {event:?}"),
    }
}

fn assert_remote_user_agent_response(
    envelope: QueuedServerEnvelope,
    expected_id: i64,
    expected_suffix: &str,
) {
    match envelope.event {
        ServerEvent::ServerMessage { message } => match *message {
            mcp_types::JSONRPCMessage::Response(response) => {
                assert_eq!(response.id, RequestId::Integer(expected_id));
                let user_agent = response
                    .result
                    .get("userAgent")
                    .and_then(Value::as_str)
                    .expect("remote user agent response");
                assert!(user_agent.contains(expected_suffix));
            }
            message => panic!("expected remote response, got {message:?}"),
        },
        event => panic!("expected remote server message, got {event:?}"),
    }
}

fn assert_remote_status_notification(envelope: QueuedServerEnvelope) {
    match envelope.event {
        ServerEvent::ServerMessage { message } => match *message {
            mcp_types::JSONRPCMessage::Notification(notification) => {
                assert_eq!(notification.method, "remoteControl/status/changed");
            }
            message => panic!("expected remote status notification, got {message:?}"),
        },
        event => panic!("expected remote server message, got {event:?}"),
    }
}
