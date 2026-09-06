use super::auth::RemoteControlAuth;
use super::auth::RemoteControlAuthProvider;
use super::enroll::RemoteControlEnrollmentSelection;
use super::enroll::RemoteControlEnrollment;
use super::enroll::load_persisted_remote_control_enrollment;
use super::enroll::preview_remote_control_response_body;
use super::enroll::resolve_remote_control_enrollment;
use super::enroll::update_persisted_remote_control_enrollment;
use super::host_device::HostDevice;
use super::protocol::normalize_remote_control_url;
use super::state::RemoteControlState;
use async_trait::async_trait;
use code_app_server_protocol::RemoteControlPairingStartParams;
use code_app_server_protocol::RemoteControlPairingStatusParams;
use std::io;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use time::OffsetDateTime;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::watch;

struct TestAuthProvider {
    loads: AtomicUsize,
    recoveries: AtomicUsize,
    account_id: String,
    change_tx: watch::Sender<u64>,
}

impl TestAuthProvider {
    fn new(account_id: &str) -> Self {
        let (change_tx, _) = watch::channel(0);
        Self {
            loads: AtomicUsize::new(0),
            recoveries: AtomicUsize::new(0),
            account_id: account_id.to_string(),
            change_tx,
        }
    }
}

#[async_trait]
impl RemoteControlAuthProvider for TestAuthProvider {
    async fn load(&self) -> io::Result<RemoteControlAuth> {
        let load = self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(RemoteControlAuth::for_testing(
            if load == 0 { "stale-token" } else { "fresh-token" },
            &self.account_id,
        ))
    }

    async fn recover_unauthorized(&self) -> io::Result<bool> {
        self.recoveries.fetch_add(1, Ordering::SeqCst);
        self.change_tx.send_modify(|revision| *revision += 1);
        Ok(true)
    }

    fn subscribe(&self) -> watch::Receiver<u64> {
        self.change_tx.subscribe()
    }
}

#[test]
fn response_preview_redacts_credentials_and_is_bounded() {
    let body = serde_json::json!({
        "remote_control_token": "server-secret",
        "pairing_code": "pair-secret",
        "manual_pairing_code": "manual-secret",
        "message": "x".repeat(5000),
    });
    let preview = preview_remote_control_response_body(body.to_string().as_bytes());

    assert!(!preview.contains("server-secret"));
    assert!(!preview.contains("pair-secret"));
    assert!(!preview.contains("manual-secret"));
    assert!(preview.len() <= 4099);
    assert!(preview.ends_with("..."));
}

#[test]
fn expired_server_token_requires_refresh_even_during_refresh_backoff() {
    let enrollment = RemoteControlEnrollment {
        remote_control_target: normalize_remote_control_url("https://chatgpt.com/backend-api")
            .expect("normalize target"),
        account_id: "account-a".to_string(),
        environment_id: "environment-a".to_string(),
        server_id: "server-a".to_string(),
        server_name: "deck".to_string(),
        remote_control_token: Some("expired-token".to_string()),
        expires_at: Some(OffsetDateTime::now_utc() - time::Duration::seconds(1)),
        next_refresh_at: Some(OffsetDateTime::now_utc() + time::Duration::minutes(1)),
    };

    assert!(enrollment.should_refresh_server_token());
}

#[tokio::test]
async fn persistence_round_trip_excludes_short_lived_server_credentials() {
    let code_home = tempfile::tempdir().expect("create temp code home");
    let state = RemoteControlState::open(code_home.path())
        .await
        .expect("open state");
    let target = normalize_remote_control_url("https://chatgpt.com/backend-api")
        .expect("normalize target");
    let enrollment = RemoteControlEnrollment {
        remote_control_target: target.clone(),
        account_id: "account-a".to_string(),
        environment_id: "environment-a".to_string(),
        server_id: "server-a".to_string(),
        server_name: "deck".to_string(),
        remote_control_token: Some("short-lived-secret".to_string()),
        expires_at: Some(OffsetDateTime::now_utc() + time::Duration::hours(1)),
        next_refresh_at: None,
    };

    update_persisted_remote_control_enrollment(
        &state,
        &target,
        "account-a",
        Some("desktop"),
        Some(&enrollment),
        Some(true),
    )
    .await
    .expect("persist enrollment");
    let loaded = load_persisted_remote_control_enrollment(
        &state,
        &target,
        "account-a",
        Some("desktop"),
    )
    .await
    .expect("load enrollment")
    .expect("enrollment exists");

    assert_eq!(loaded.server_id, "server-a");
    assert_eq!(loaded.environment_id, "environment-a");
    assert!(loaded.remote_control_token.is_none());
    assert!(loaded.expires_at.is_none());
    assert!(
        load_persisted_remote_control_enrollment(
            &state,
            &target,
            "account-b",
            Some("desktop"),
        )
        .await
        .expect("load other account")
        .is_none()
    );
}

#[tokio::test]
async fn pairing_uses_the_short_lived_server_token_and_maps_status() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (path, body) in [
            (
                "/backend-api/wham/remote/control/server/pair",
                br#"{"pairing_code":"pair-a","manual_pairing_code":"manual-a","server_id":"server-a","environment_id":"environment-a","expires_at":"2030-01-01T00:00:00Z"}"#.as_slice(),
            ),
            (
                "/backend-api/wham/remote/control/server/pair/status",
                br#"{"claimed":true}"#.as_slice(),
            ),
        ] {
            let (mut stream, _) = listener.accept().await.expect("accept request");
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
                    .expect("content length");
                while request.len() < header_end + content_length {
                    let read = stream.read(&mut buffer).await.expect("read body");
                    assert!(read > 0, "body ended early");
                    request.extend_from_slice(&buffer[..read]);
                }
                let request_body = String::from_utf8_lossy(
                    &request[header_end..header_end + content_length],
                )
                .into_owned();
                assert!(headers.starts_with(&format!("POST {path} HTTP/1.1")));
                assert!(headers.to_ascii_lowercase().contains("authorization: bearer server-token"));
                requests.push(request_body);
                break;
            }
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .expect("write response headers");
            stream.write_all(body).await.expect("write response body");
        }
        requests
    });
    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize target");
    let enrollment = RemoteControlEnrollment {
        remote_control_target: target,
        account_id: "account-a".to_string(),
        environment_id: "environment-a".to_string(),
        server_id: "server-a".to_string(),
        server_name: "deck".to_string(),
        remote_control_token: Some("server-token".to_string()),
        expires_at: Some(OffsetDateTime::now_utc() + time::Duration::hours(1)),
        next_refresh_at: None,
    };

    let pairing = enrollment
        .start_pairing(RemoteControlPairingStartParams { manual_code: true })
        .await
        .expect("start pairing");
    let status = enrollment
        .pairing_status(RemoteControlPairingStatusParams {
            pairing_code: Some(pairing.pairing_code.clone()),
            manual_pairing_code: None,
        })
        .await
        .expect("read pairing status");
    let requests = fixture.await.expect("join fixture");

    assert_eq!(pairing.manual_pairing_code.as_deref(), Some("manual-a"));
    assert_eq!(pairing.environment_id, "environment-a");
    assert!(status.claimed);
    assert_eq!(requests[0], r#"{"manual_code":true}"#);
    assert_eq!(requests[1], r#"{"pairing_code":"pair-a"}"#);
}

#[tokio::test]
async fn reuse_refreshes_persisted_identity_and_recovers_unauthorized_once() {
    let code_home = tempfile::tempdir().expect("create temp code home");
    let state = RemoteControlState::open(code_home.path())
        .await
        .expect("open state");
    let (base_url, fixture) = spawn_http_fixture(vec![
        (
            "401 Unauthorized",
            br#"{"remote_control_token":"must-redact"}"#,
        ),
        (
            "200 OK",
            br#"{"server_id":"server-old","environment_id":"env-old","remote_control_token":"server-token","expires_at":"2030-01-01T00:00:00Z"}"#,
        ),
    ])
    .await;
    let target = normalize_remote_control_url(&base_url).expect("normalize target");
    let persisted = RemoteControlEnrollment {
        remote_control_target: target.clone(),
        account_id: "account-a".to_string(),
        environment_id: "env-old".to_string(),
        server_id: "server-old".to_string(),
        server_name: "old-name".to_string(),
        remote_control_token: None,
        expires_at: None,
        next_refresh_at: None,
    };
    update_persisted_remote_control_enrollment(
        &state,
        &target,
        "account-a",
        Some("desktop"),
        Some(&persisted),
        Some(true),
    )
    .await
    .expect("persist enrollment");
    let provider = TestAuthProvider::new("account-a");
    let host = HostDevice::for_testing("new-name", "linux", "x86_64", Some("desktop"));

    let enrollment = resolve_remote_control_enrollment(
        &state,
        &target,
        &provider,
        "installation-a",
        &host,
        Some("desktop"),
        None,
        Some(true),
        RemoteControlEnrollmentSelection::ReuseOrCreate,
    )
    .await
    .expect("reuse enrollment");
    let requests = fixture.await.expect("join fixture");

    assert_eq!(provider.loads.load(Ordering::SeqCst), 2);
    assert_eq!(provider.recoveries.load(Ordering::SeqCst), 1);
    assert_eq!(enrollment.server_id, "server-old");
    assert_eq!(enrollment.remote_control_token.as_deref(), Some("server-token"));
    assert!(requests.iter().all(|request| request.starts_with(
        "POST /backend-api/wham/remote/control/server/refresh HTTP/1.1"
    )));
    assert!(requests[0]
        .to_ascii_lowercase()
        .contains("authorization: bearer stale-token"));
    assert!(requests[1]
        .to_ascii_lowercase()
        .contains("authorization: bearer fresh-token"));
}

#[tokio::test]
async fn replacement_and_account_change_never_reuse_another_identity() {
    let code_home = tempfile::tempdir().expect("create temp code home");
    let state = RemoteControlState::open(code_home.path())
        .await
        .expect("open state");
    let (base_url, fixture) = spawn_http_fixture(vec![
        (
            "200 OK",
            br#"{"server_id":"server-b","environment_id":"env-b","remote_control_token":"token-b","expires_at":"2030-01-01T00:00:00Z"}"#,
        ),
        (
            "200 OK",
            br#"{"server_id":"server-c","environment_id":"env-c","remote_control_token":"token-c","expires_at":"2030-01-01T00:00:00Z"}"#,
        ),
    ])
    .await;
    let target = normalize_remote_control_url(&base_url).expect("normalize target");
    let account_a = RemoteControlEnrollment {
        remote_control_target: target.clone(),
        account_id: "account-a".to_string(),
        environment_id: "env-a".to_string(),
        server_id: "server-a".to_string(),
        server_name: "old-name".to_string(),
        remote_control_token: Some("old-token".to_string()),
        expires_at: Some(OffsetDateTime::now_utc() + time::Duration::hours(1)),
        next_refresh_at: None,
    };
    update_persisted_remote_control_enrollment(
        &state,
        &target,
        "account-a",
        Some("desktop"),
        Some(&account_a),
        Some(true),
    )
    .await
    .expect("persist account A");
    let host = HostDevice::for_testing("new-name", "linux", "x86_64", Some("desktop"));

    let account_b = resolve_remote_control_enrollment(
        &state,
        &target,
        &TestAuthProvider::new("account-b"),
        "installation-a",
        &host,
        Some("desktop"),
        Some(&account_a),
        Some(false),
        RemoteControlEnrollmentSelection::ReuseOrCreate,
    )
    .await
    .expect("enroll account B");
    let replaced_account_a = resolve_remote_control_enrollment(
        &state,
        &target,
        &TestAuthProvider::new("account-a"),
        "installation-a",
        &host,
        Some("desktop"),
        Some(&account_a),
        Some(true),
        RemoteControlEnrollmentSelection::ReplaceExisting,
    )
    .await
    .expect("replace account A");
    let requests = fixture.await.expect("join fixture");

    assert_eq!(account_b.server_id, "server-b");
    assert_eq!(replaced_account_a.server_id, "server-c");
    assert!(requests.iter().all(|request| request.starts_with(
        "POST /backend-api/wham/remote/control/server/enroll HTTP/1.1"
    )));
    assert_eq!(
        state
            .get_enrollment(&target.websocket_url, "account-a", Some("desktop"))
            .await
            .expect("load account A")
            .expect("account A remains")
            .server_id,
        "server-c"
    );
}

#[tokio::test]
async fn stale_refresh_reenrolls_but_transient_refresh_failure_does_not_replace() {
    let code_home = tempfile::tempdir().expect("create temp code home");
    let state = RemoteControlState::open(code_home.path())
        .await
        .expect("open state");
    let (base_url, fixture) = spawn_http_fixture(vec![
        ("404 Not Found", br#"{"error":"missing"}"#),
        (
            "200 OK",
            br#"{"server_id":"server-new","environment_id":"env-new","remote_control_token":"token-new","expires_at":"2030-01-01T00:00:00Z"}"#,
        ),
    ])
    .await;
    let target = normalize_remote_control_url(&base_url).expect("normalize target");
    let stale = RemoteControlEnrollment {
        remote_control_target: target.clone(),
        account_id: "account-a".to_string(),
        environment_id: "env-old".to_string(),
        server_id: "server-old".to_string(),
        server_name: "old-name".to_string(),
        remote_control_token: None,
        expires_at: None,
        next_refresh_at: None,
    };
    update_persisted_remote_control_enrollment(
        &state,
        &target,
        "account-a",
        Some("desktop"),
        Some(&stale),
        Some(true),
    )
    .await
    .expect("persist stale enrollment");
    let host = HostDevice::for_testing("deck", "linux", "x86_64", Some("desktop"));
    let replacement = resolve_remote_control_enrollment(
        &state,
        &target,
        &TestAuthProvider::new("account-a"),
        "installation-a",
        &host,
        Some("desktop"),
        None,
        Some(true),
        RemoteControlEnrollmentSelection::ReuseOrCreate,
    )
    .await
    .expect("replace stale enrollment");
    let requests = fixture.await.expect("join fixture");
    assert_eq!(replacement.server_id, "server-new");
    assert!(requests[0].contains("/server/refresh"));
    assert!(requests[1].contains("/server/enroll"));

    let (base_url, fixture) =
        spawn_http_fixture(vec![("500 Internal Server Error", br#"{"error":"temporary"}"#)])
            .await;
    let target = normalize_remote_control_url(&base_url).expect("normalize target");
    let transient = RemoteControlEnrollment {
        remote_control_target: target.clone(),
        account_id: "account-a".to_string(),
        environment_id: "env-old".to_string(),
        server_id: "server-old".to_string(),
        server_name: "deck".to_string(),
        remote_control_token: None,
        expires_at: None,
        next_refresh_at: None,
    };
    update_persisted_remote_control_enrollment(
        &state,
        &target,
        "account-a",
        Some("desktop"),
        Some(&transient),
        Some(true),
    )
    .await
    .expect("persist transient enrollment");
    let error = resolve_remote_control_enrollment(
        &state,
        &target,
        &TestAuthProvider::new("account-a"),
        "installation-a",
        &host,
        Some("desktop"),
        None,
        Some(true),
        RemoteControlEnrollmentSelection::ReuseOrCreate,
    )
    .await
    .expect_err("transient refresh must not replace identity");
    let requests = fixture.await.expect("join fixture");
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("/server/refresh"));
}

async fn spawn_http_fixture(
    responses: Vec<(&'static str, &'static [u8])>,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().await.expect("accept request");
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
                if request.len() < header_end + content_length {
                    continue;
                }
                requests.push(String::from_utf8_lossy(&request).into_owned());
                break;
            }
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .expect("write headers");
            stream.write_all(body).await.expect("write body");
        }
        requests
    });
    (format!("http://{address}/backend-api"), fixture)
}
