use super::RemoteControlHandle;
use super::RemoteControlPolicy;
use super::RemoteControlStartConfig;
use super::RemoteControlStartupMode;
use super::auth::RemoteControlAuth;
use super::auth::RemoteControlAuthProvider;
use super::desired_state::RemoteControlDesiredState;
use super::enroll::RemoteControlEnrollment;
use super::host_device::HostDevice;
use super::protocol::normalize_remote_control_url;
use super::start_remote_control;
use super::state::RemoteControlEnrollmentRecord;
use super::state::RemoteControlState;
use crate::transport::CHANNEL_CAPACITY;
use async_trait::async_trait;
use code_app_server_protocol::RemoteControlConnectionStatus;
use code_app_server_protocol::RemoteControlPairingStartParams;
use code_app_server_protocol::RemoteControlPairingStatusParams;
use code_app_server_protocol::RemoteControlStatusChangedNotification;
use std::io;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use time::OffsetDateTime;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::accept_async;
use tokio_util::sync::CancellationToken;

struct TestAuthProvider {
    auth: StdMutex<RemoteControlAuth>,
    change_tx: watch::Sender<u64>,
    load_delay: Duration,
    active_loads: AtomicUsize,
    max_active_loads: AtomicUsize,
}

impl TestAuthProvider {
    fn new(access_token: &str, account_id: &str) -> Self {
        Self::with_load_delay(access_token, account_id, Duration::ZERO)
    }

    fn with_load_delay(access_token: &str, account_id: &str, load_delay: Duration) -> Self {
        let (change_tx, _) = watch::channel(0);
        Self {
            auth: StdMutex::new(RemoteControlAuth::for_testing(access_token, account_id)),
            change_tx,
            load_delay,
            active_loads: AtomicUsize::new(0),
            max_active_loads: AtomicUsize::new(0),
        }
    }

    fn replace_auth(&self, access_token: &str, account_id: &str) {
        *self.auth.lock().expect("lock auth") =
            RemoteControlAuth::for_testing(access_token, account_id);
        self.change_tx.send_modify(|revision| *revision += 1);
    }
}

#[async_trait]
impl RemoteControlAuthProvider for TestAuthProvider {
    async fn load(&self) -> io::Result<RemoteControlAuth> {
        let active = self.active_loads.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active_loads.fetch_max(active, Ordering::SeqCst);
        if !self.load_delay.is_zero() {
            tokio::time::sleep(self.load_delay).await;
        }
        let auth = self.auth.lock().expect("lock auth").clone();
        self.active_loads.fetch_sub(1, Ordering::SeqCst);
        Ok(auth)
    }

    async fn recover_unauthorized(&self) -> io::Result<bool> {
        Ok(false)
    }

    fn subscribe(&self) -> watch::Receiver<u64> {
        self.change_tx.subscribe()
    }
}

#[tokio::test]
async fn startup_disabled_and_persisted_enabled_resolution_are_distinct() {
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let auth_provider = Arc::new(TestAuthProvider::new("access-token", "account-a"));
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
        .expect("persist enabled preference");

    let (disabled_task, disabled, disabled_shutdown) = start_test_remote_control(
        state.clone(),
        auth_provider.clone(),
        RemoteControlStartupMode::DisabledEphemeral,
    )
    .await;
    assert_eq!(disabled.status().status, RemoteControlConnectionStatus::Disabled);
    disabled_shutdown.cancel();
    disabled_task.await.expect("join disabled task").expect("disabled task succeeds");

    let (resolved_task, resolved, resolved_shutdown) = start_test_remote_control(
        state,
        auth_provider,
        RemoteControlStartupMode::ResolvePersisted,
    )
    .await;
    assert!(
        resolved
            .resolve_persisted_preference(Some("desktop"))
            .await
            .expect("resolve preference")
    );
    assert_ne!(resolved.status().status, RemoteControlConnectionStatus::Disabled);
    resolved_shutdown.cancel();
    resolved_task.await.expect("join resolved task").expect("resolved task succeeds");
}

#[tokio::test]
async fn policy_and_missing_state_prevent_enablement() {
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let auth_provider = Arc::new(TestAuthProvider::new("access-token", "account-a"));
    let target = normalize_remote_control_url("http://127.0.0.1:9/backend-api")
        .expect("normalize target");
    let mut managed = standalone_handle(state, auth_provider.clone(), target.clone());
    managed.policy = RemoteControlPolicy::DisabledByRequirements;
    assert!(
        !managed
            .resolve_persisted_preference(Some("desktop"))
            .await
            .expect("managed disable resolves without reading persistence")
    );
    assert!(matches!(
        managed.enable_ephemeral(),
        Err(super::RemoteControlEnableError::DisabledByRequirements(_))
    ));
    assert_eq!(
        managed
            .enable(Some("desktop"))
            .await
            .expect_err("managed policy blocks persistent enable")
            .kind(),
        io::ErrorKind::PermissionDenied,
    );

    let mut unavailable = standalone_handle(
        RemoteControlState::open(state_dir.path()).await.expect("reopen state"),
        auth_provider,
        target,
    );
    unavailable.state = None;
    assert!(matches!(
        unavailable.enable_ephemeral(),
        Err(super::RemoteControlEnableError::Unavailable(_))
    ));
}

#[tokio::test]
async fn persistent_and_ephemeral_transitions_update_only_the_requested_state() {
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let auth_provider = Arc::new(TestAuthProvider::new("access-token", "account-a"));
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
            remote_control_enabled: Some(false),
        })
        .await
        .expect("persist disabled preference");
    let handle = standalone_handle(state.clone(), auth_provider, target.clone());
    *handle.current_enrollment.lock().await = Some(test_enrollment(target));

    assert_eq!(
        handle
            .enable(Some("desktop"))
            .await
            .expect("persist enable")
            .status,
        RemoteControlConnectionStatus::Connecting,
    );
    assert_eq!(persisted_enabled(&state).await, Some(true));
    assert_eq!(
        handle.disable_ephemeral().await.status,
        RemoteControlConnectionStatus::Disabled,
    );
    assert_eq!(persisted_enabled(&state).await, Some(true));
    assert_eq!(
        handle.enable_ephemeral().expect("ephemeral enable").status,
        RemoteControlConnectionStatus::Connecting,
    );
    assert_eq!(persisted_enabled(&state).await, Some(true));
    assert_eq!(
        handle
            .disable(Some("desktop"))
            .await
            .expect("persist disable")
            .status,
        RemoteControlConnectionStatus::Disabled,
    );
    assert_eq!(persisted_enabled(&state).await, Some(false));
}

#[tokio::test]
async fn persistent_enable_enrolls_and_persists_before_returning() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept enrollment");
        let request = read_http_request(&mut stream).await;
        assert!(request.contains("/server/enroll"));
        write_http_response(
            &mut stream,
            "200 OK",
            br#"{"server_id":"server-a","environment_id":"env-a","remote_control_token":"token-a","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .await;
    });
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    let handle = standalone_handle(
        state.clone(),
        Arc::new(TestAuthProvider::new("access-token", "account-a")),
        target.clone(),
    );

    let status = handle
        .enable(Some("desktop"))
        .await
        .expect("persistent enable succeeds");
    timeout(Duration::from_secs(2), fixture)
        .await
        .expect("persistent enable must enroll before returning")
        .expect("join fixture");
    assert_eq!(status.status, RemoteControlConnectionStatus::Connecting);
    assert_eq!(status.environment_id.as_deref(), Some("env-a"));
    assert_eq!(
        state
            .get_enrollment(&target.websocket_url, "account-a", Some("desktop"))
            .await
            .expect("load enrollment")
            .expect("enrollment exists")
            .remote_control_enabled,
        Some(true),
    );
    assert_eq!(
        handle
            .current_enrollment
            .lock()
            .await
            .as_ref()
            .map(|enrollment| enrollment.server_id.as_str()),
        Some("server-a"),
    );
}

#[tokio::test]
async fn persistent_enable_rejects_an_account_change_during_enrollment() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let release = Arc::new(Semaphore::new(0));
    let release_fixture = release.clone();
    let (request_seen_tx, request_seen_rx) = oneshot::channel();
    let fixture = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept enrollment");
        let request = read_http_request(&mut stream).await;
        assert!(request.contains("/server/enroll"));
        request_seen_tx.send(()).expect("signal enrollment request");
        release_fixture.acquire().await.expect("release enrollment").forget();
        write_http_response(
            &mut stream,
            "200 OK",
            br#"{"server_id":"server-a","environment_id":"env-a","remote_control_token":"token-a","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .await;
    });
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let auth_provider = Arc::new(TestAuthProvider::new("access-a", "account-a"));
    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    let handle = standalone_handle(state.clone(), auth_provider.clone(), target.clone());

    let enabling_handle = handle.clone();
    let enabling = tokio::spawn(async move { enabling_handle.enable(Some("desktop")).await });
    timeout(Duration::from_secs(2), request_seen_rx)
        .await
        .expect("persistent enable must start enrollment")
        .expect("enrollment request signal");
    auth_provider.replace_auth("access-b", "account-b");
    release.add_permits(1);

    let error = enabling
        .await
        .expect("join enable")
        .expect_err("account change invalidates persistent enable");
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    let stale_record = state
        .get_enrollment(&target.websocket_url, "account-a", Some("desktop"))
        .await
        .expect("load stale enrollment")
        .expect("enrollment identity is retained");
    assert_ne!(stale_record.remote_control_enabled, Some(true));
    assert!(handle.current_enrollment.lock().await.is_none());
    fixture.await.expect("join fixture");
}

#[tokio::test]
async fn concurrent_persistent_enable_calls_are_serialized() {
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let auth_provider = Arc::new(TestAuthProvider::with_load_delay(
        "access-token",
        "account-a",
        Duration::from_millis(50),
    ));
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
            remote_control_enabled: Some(false),
        })
        .await
        .expect("persist disabled preference");
    let handle = standalone_handle(state, auth_provider.clone(), target.clone());
    *handle.current_enrollment.lock().await = Some(test_enrollment(target));

    let first = handle.enable(Some("desktop"));
    let second = handle.enable(Some("desktop"));
    let (first, second) = tokio::join!(first, second);
    first.expect("first enable");
    second.expect("second enable");
    assert_eq!(auth_provider.max_active_loads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn persisted_resolution_does_not_overwrite_a_newer_ephemeral_enable() {
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let auth_provider = Arc::new(TestAuthProvider::with_load_delay(
        "access-token",
        "account-a",
        Duration::from_millis(50),
    ));
    let target = normalize_remote_control_url("http://127.0.0.1:9/backend-api")
        .expect("normalize target");
    let handle = standalone_handle(state, auth_provider, target);
    handle
        .desired_state_tx
        .send_replace(RemoteControlDesiredState::Unknown);

    let resolving_handle = handle.clone();
    let resolving = tokio::spawn(async move {
        resolving_handle
            .resolve_persisted_preference(Some("desktop"))
            .await
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    handle.enable_ephemeral().expect("enable ephemeral");

    assert!(
        resolving
            .await
            .expect("join preference resolution")
            .expect("resolve preference")
    );
    assert_eq!(handle.status().status, RemoteControlConnectionStatus::Connecting);
}

#[tokio::test]
async fn lifecycle_publishes_connected_then_disabled_and_shuts_down_cleanly() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture_shutdown = CancellationToken::new();
    let fixture_shutdown_task = fixture_shutdown.clone();
    let fixture = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept enrollment");
        let request = read_http_request(&mut stream).await;
        assert!(request.contains("/server/enroll"));
        write_http_response(
            &mut stream,
            "200 OK",
            br#"{"server_id":"server-a","environment_id":"env-a","remote_control_token":"token-a","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .await;
        let (stream, _) = listener.accept().await.expect("accept websocket");
        let mut websocket = accept_async(stream).await.expect("accept websocket handshake");
        fixture_shutdown_task.cancelled().await;
        websocket.close(None).await.expect("close websocket");
    });
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let auth_provider = Arc::new(TestAuthProvider::new("access-token", "account-a"));
    let shutdown = CancellationToken::new();
    let (transport_tx, _transport_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (task, handle) = start_remote_control(
        RemoteControlStartConfig {
            remote_control_url: format!("http://{address}/backend-api"),
            installation_id: "installation-a".to_string(),
            host: HostDevice::for_testing("deck", "linux", "x86_64", Some("desktop")),
            policy: RemoteControlPolicy::Allowed,
        },
        Some(state),
        auth_provider,
        transport_tx,
        shutdown.clone(),
        RemoteControlStartupMode::EnabledEphemeral,
    )
    .await
    .expect("start remote control");
    let mut status_rx = handle.status_receiver();
    timeout(Duration::from_secs(2), status_rx.wait_for(|status| {
        status.status == RemoteControlConnectionStatus::Connected
    }))
    .await
    .expect("connected status timeout")
    .expect("status channel remains open");
    assert_eq!(handle.status().environment_id.as_deref(), Some("env-a"));

    assert_eq!(
        handle.disable_ephemeral().await.status,
        RemoteControlConnectionStatus::Disabled,
    );
    assert!(handle.status().environment_id.is_none());
    fixture_shutdown.cancel();
    shutdown.cancel();
    timeout(Duration::from_secs(2), task)
        .await
        .expect("lifecycle shutdown timeout")
        .expect("join lifecycle")
        .expect("lifecycle succeeds");
    fixture.await.expect("join fixture");
}

#[tokio::test]
async fn pairing_requests_are_serialized_by_the_shared_enrollment_lock() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let release_first = Arc::new(Semaphore::new(0));
    let release_first_fixture = release_first.clone();
    let fixture = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.expect("accept first pairing");
        let first_request = read_http_request(&mut first).await;
        assert!(first_request.contains("/server/pair"));
        assert!(timeout(Duration::from_millis(100), listener.accept()).await.is_err());
        release_first_fixture.acquire().await.expect("release first").forget();
        write_http_response(
            &mut first,
            "200 OK",
            br#"{"pairing_code":"pair-a","manual_pairing_code":null,"server_id":"server-a","environment_id":"env-a","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .await;
        let (mut second, _) = listener.accept().await.expect("accept second pairing");
        let second_request = read_http_request(&mut second).await;
        assert!(second_request.contains("/server/pair"));
        write_http_response(
            &mut second,
            "200 OK",
            br#"{"pairing_code":"pair-b","manual_pairing_code":null,"server_id":"server-a","environment_id":"env-a","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .await;
    });
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let auth_provider = Arc::new(TestAuthProvider::new("access-a", "account-a"));
    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    let handle = standalone_handle(state, auth_provider.clone(), target.clone());
    *handle.current_enrollment.lock().await = Some(test_enrollment(target));
    handle.enable_ephemeral().expect("enable pairing");

    let first_handle = handle.clone();
    let first = tokio::spawn(async move {
        first_handle
            .start_pairing(RemoteControlPairingStartParams { manual_code: false }, Some("desktop"))
            .await
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    let second_handle = handle.clone();
    let second = tokio::spawn(async move {
        second_handle
            .start_pairing(RemoteControlPairingStartParams { manual_code: false }, Some("desktop"))
            .await
    });
    tokio::time::sleep(Duration::from_millis(125)).await;
    release_first.add_permits(1);

    assert_eq!(
        first
            .await
            .expect("join first pairing")
            .expect("first pairing succeeds")
            .pairing_code,
        "pair-a",
    );
    assert_eq!(
        second
            .await
            .expect("join second pairing")
            .expect("second pairing succeeds")
            .pairing_code,
        "pair-b",
    );
    fixture.await.expect("join fixture");
}

#[tokio::test]
async fn pairing_rejects_an_account_change_during_the_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let release = Arc::new(Semaphore::new(0));
    let release_fixture = release.clone();
    let fixture = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept pairing");
        let request = read_http_request(&mut stream).await;
        assert!(request.contains("/server/pair"));
        release_fixture.acquire().await.expect("release pairing").forget();
        write_http_response(
            &mut stream,
            "200 OK",
            br#"{"pairing_code":"pair-a","manual_pairing_code":null,"server_id":"server-a","environment_id":"env-a","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .await;
    });
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let auth_provider = Arc::new(TestAuthProvider::new("access-a", "account-a"));
    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    let handle = standalone_handle(state, auth_provider.clone(), target.clone());
    *handle.current_enrollment.lock().await = Some(test_enrollment(target));
    handle.enable_ephemeral().expect("enable pairing");

    let pairing_handle = handle.clone();
    let pairing = tokio::spawn(async move {
        pairing_handle
            .start_pairing(RemoteControlPairingStartParams { manual_code: false }, Some("desktop"))
            .await
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    auth_provider.replace_auth("access-b", "account-b");
    release.add_permits(1);
    let error = pairing
        .await
        .expect("join pairing")
        .expect_err("account change invalidates pairing");
    assert_eq!(error.kind(), io::ErrorKind::NotConnected);
    fixture.await.expect("join fixture");
}

#[tokio::test]
async fn pairing_refreshes_a_rejected_server_token_and_retries_once() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        let (mut first_pair, _) = listener.accept().await.expect("accept first pairing");
        let request = read_http_request(&mut first_pair).await;
        assert!(request.contains("/server/pair"));
        assert!(request.to_ascii_lowercase().contains("authorization: bearer server-token"));
        write_http_response(&mut first_pair, "401 Unauthorized", br#"{"error":"expired"}"#)
            .await;

        let (mut refresh, _) = listener.accept().await.expect("accept refresh");
        let request = read_http_request(&mut refresh).await;
        assert!(request.contains("/server/refresh"));
        write_http_response(
            &mut refresh,
            "200 OK",
            br#"{"server_id":"server-a","environment_id":"env-a","remote_control_token":"refreshed-token","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .await;

        let (mut second_pair, _) = listener.accept().await.expect("accept second pairing");
        let request = read_http_request(&mut second_pair).await;
        assert!(request.contains("/server/pair"));
        assert!(request.to_ascii_lowercase().contains("authorization: bearer refreshed-token"));
        write_http_response(
            &mut second_pair,
            "200 OK",
            br#"{"pairing_code":"pair-a","manual_pairing_code":null,"server_id":"server-a","environment_id":"env-a","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .await;
    });
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    let handle = standalone_handle(
        state,
        Arc::new(TestAuthProvider::new("access-token", "account-a")),
        target.clone(),
    );
    *handle.current_enrollment.lock().await = Some(test_enrollment(target));
    handle.enable_ephemeral().expect("enable pairing");

    let pairing = handle
        .start_pairing(RemoteControlPairingStartParams { manual_code: false }, Some("desktop"))
        .await
        .expect("pairing recovers");
    assert_eq!(pairing.pairing_code, "pair-a");
    fixture.await.expect("join fixture");
}

#[tokio::test]
async fn pairing_status_refreshes_a_rejected_server_token_and_retries_once() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        let (mut first_status, _) = listener.accept().await.expect("accept first status");
        let request = read_http_request(&mut first_status).await;
        assert!(request.contains("/server/pair/status"));
        assert!(request.to_ascii_lowercase().contains("authorization: bearer server-token"));
        write_http_response(&mut first_status, "401 Unauthorized", br#"{"error":"expired"}"#)
            .await;

        let (mut refresh, _) = listener.accept().await.expect("accept refresh");
        let request = read_http_request(&mut refresh).await;
        assert!(request.contains("/server/refresh"));
        write_http_response(
            &mut refresh,
            "200 OK",
            br#"{"server_id":"server-a","environment_id":"env-a","remote_control_token":"refreshed-token","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .await;

        let (mut second_status, _) = listener.accept().await.expect("accept second status");
        let request = read_http_request(&mut second_status).await;
        assert!(request.contains("/server/pair/status"));
        assert!(request.to_ascii_lowercase().contains("authorization: bearer refreshed-token"));
        write_http_response(&mut second_status, "200 OK", br#"{"claimed":true}"#).await;
    });
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    let handle = standalone_handle(
        state,
        Arc::new(TestAuthProvider::new("access-token", "account-a")),
        target.clone(),
    );
    *handle.current_enrollment.lock().await = Some(test_enrollment(target));
    handle.enable_ephemeral().expect("enable pairing");

    let pairing = handle
        .pairing_status(RemoteControlPairingStatusParams {
            pairing_code: Some("pair-a".to_string()),
            manual_pairing_code: None,
        })
        .await
        .expect("pairing status recovers");
    assert!(pairing.claimed);
    fixture.await.expect("join fixture");
}

#[tokio::test]
async fn pairing_status_does_not_create_a_new_server_without_a_current_pairing_identity() {
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let target = normalize_remote_control_url("http://127.0.0.1:9/backend-api")
        .expect("normalize target");
    let handle = standalone_handle(
        state,
        Arc::new(TestAuthProvider::new("access-token", "account-a")),
        target,
    );
    handle.enable_ephemeral().expect("enable pairing");

    let error = handle
        .pairing_status(RemoteControlPairingStatusParams {
            pairing_code: Some("pair-a".to_string()),
            manual_pairing_code: None,
        })
        .await
        .expect_err("missing pairing identity is unavailable");
    assert_eq!(error.kind(), io::ErrorKind::NotConnected);
}

#[tokio::test]
async fn lifecycle_shutdown_interrupts_an_in_progress_connection_attempt() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reserve unused address");
    let address = listener.local_addr().expect("unused address");
    drop(listener);
    let state_dir = tempfile::tempdir().expect("create state dir");
    let state = RemoteControlState::open(state_dir.path()).await.expect("open state");
    let shutdown = CancellationToken::new();
    let (transport_tx, _transport_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (task, _handle) = start_remote_control(
        RemoteControlStartConfig {
            remote_control_url: format!("http://{address}/backend-api"),
            installation_id: "installation-a".to_string(),
            host: HostDevice::for_testing("deck", "linux", "x86_64", Some("desktop")),
            policy: RemoteControlPolicy::Allowed,
        },
        Some(state),
        Arc::new(TestAuthProvider::new("access-token", "account-a")),
        transport_tx,
        shutdown.clone(),
        RemoteControlStartupMode::EnabledEphemeral,
    )
    .await
    .expect("start remote control");

    tokio::time::sleep(Duration::from_millis(50)).await;
    shutdown.cancel();
    timeout(Duration::from_millis(250), task)
        .await
        .expect("shutdown must interrupt connection retry")
        .expect("join lifecycle")
        .expect("lifecycle succeeds");
}

async fn start_test_remote_control(
    state: Arc<RemoteControlState>,
    auth_provider: Arc<TestAuthProvider>,
    startup_mode: RemoteControlStartupMode,
) -> (
    tokio::task::JoinHandle<io::Result<()>>,
    RemoteControlHandle,
    CancellationToken,
) {
    let shutdown = CancellationToken::new();
    let (transport_tx, _transport_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (task, handle) = start_remote_control(
        RemoteControlStartConfig {
            remote_control_url: "http://127.0.0.1:9/backend-api".to_string(),
            installation_id: "installation-a".to_string(),
            host: HostDevice::for_testing("deck", "linux", "x86_64", Some("desktop")),
            policy: RemoteControlPolicy::Allowed,
        },
        Some(state),
        auth_provider,
        transport_tx,
        shutdown.clone(),
        startup_mode,
    )
    .await
    .expect("start remote control");
    (task, handle, shutdown)
}

fn standalone_handle(
    state: Arc<RemoteControlState>,
    auth_provider: Arc<TestAuthProvider>,
    target: super::protocol::RemoteControlTarget,
) -> RemoteControlHandle {
    let (desired_state_tx, _) = watch::channel(RemoteControlDesiredState::Disabled);
    let (status_tx, _) = watch::channel(RemoteControlStatusChangedNotification {
        status: RemoteControlConnectionStatus::Disabled,
        server_name: "deck".to_string(),
        installation_id: "installation-a".to_string(),
        environment_id: None,
    });
    RemoteControlHandle {
        policy: RemoteControlPolicy::Allowed,
        desired_state_tx: Arc::new(desired_state_tx),
        desired_state_transition_lock: Arc::new(Semaphore::new(1)),
        desired_state_persistence_lock: Arc::new(Semaphore::new(1)),
        status_tx: Arc::new(status_tx),
        state: Some(state),
        target,
        installation_id: "installation-a".to_string(),
        host: HostDevice::for_testing("deck", "linux", "x86_64", Some("desktop")),
        current_enrollment: Arc::new(Mutex::new(None)),
        app_server_client_name: Arc::new(Mutex::new(None)),
        auth_provider,
    }
}

async fn persisted_enabled(state: &RemoteControlState) -> Option<bool> {
    state
        .get_enrollment(
            "ws://127.0.0.1:9/backend-api/wham/remote/control/server",
            "account-a",
            Some("desktop"),
        )
        .await
        .expect("load enrollment")
        .expect("enrollment exists")
        .remote_control_enabled
}

fn test_enrollment(target: super::protocol::RemoteControlTarget) -> RemoteControlEnrollment {
    RemoteControlEnrollment {
        remote_control_target: target,
        account_id: "account-a".to_string(),
        environment_id: "env-a".to_string(),
        server_id: "server-a".to_string(),
        server_name: "deck".to_string(),
        remote_control_token: Some("server-token".to_string()),
        expires_at: Some(OffsetDateTime::now_utc() + time::Duration::hours(1)),
        next_refresh_at: None,
    }
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 2048];
    loop {
        let read = stream.read(&mut buffer).await.expect("read request");
        assert!(read > 0, "request ended early");
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
