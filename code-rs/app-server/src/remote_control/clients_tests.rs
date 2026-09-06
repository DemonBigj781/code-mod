use super::auth::RemoteControlAuth;
use super::auth::RemoteControlAuthProvider;
use super::clients::list_remote_control_clients;
use super::clients::revoke_remote_control_client;
use async_trait::async_trait;
use code_app_server_protocol::RemoteControlClientsListOrder;
use code_app_server_protocol::RemoteControlClientsListParams;
use code_app_server_protocol::RemoteControlClientsRevokeParams;
use std::io;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::watch;

struct RecoveringAuthProvider {
    loads: AtomicUsize,
    recoveries: AtomicUsize,
    change_tx: watch::Sender<u64>,
}

impl RecoveringAuthProvider {
    fn new() -> Self {
        let (change_tx, _) = watch::channel(0);
        Self {
            loads: AtomicUsize::new(0),
            recoveries: AtomicUsize::new(0),
            change_tx,
        }
    }
}

#[async_trait]
impl RemoteControlAuthProvider for RecoveringAuthProvider {
    async fn load(&self) -> io::Result<RemoteControlAuth> {
        let load = self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(RemoteControlAuth::for_testing(
            if load == 0 { "stale-token" } else { "fresh-token" },
            "account-a",
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

#[tokio::test]
async fn client_list_retries_once_after_unauthorized_and_maps_response() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        let mut requests = Vec::new();
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            loop {
                let read = stream.read(&mut buffer).await.expect("read request");
                assert!(read > 0, "request ended before headers");
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            requests.push(String::from_utf8_lossy(&request).into_owned());
            let (status, body): (&str, &[u8]) = if attempt == 0 {
                ("401 Unauthorized", br#"{"remote_control_token":"must-redact"}"#)
            } else {
                ("200 OK", br#"{"items":[{"client_id":"client-a","display_name":"Phone","last_seen_at":"2026-09-06T12:00:00Z"}],"cursor":"next"}"#)
            };
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
    let provider = RecoveringAuthProvider::new();
    let response = list_remote_control_clients(
        &format!("http://{address}/backend-api"),
        &provider,
        RemoteControlClientsListParams {
            environment_id: "env/a".to_string(),
            cursor: Some("cursor value".to_string()),
            limit: Some(25),
            order: Some(RemoteControlClientsListOrder::Desc),
        },
    )
    .await
    .expect("list clients");
    let requests = fixture.await.expect("join fixture");

    assert_eq!(provider.recoveries.load(Ordering::SeqCst), 1);
    assert_eq!(provider.loads.load(Ordering::SeqCst), 2);
    assert!(requests[0].starts_with("GET /backend-api/wham/remote/control/environments/env%2Fa/clients?cursor=cursor+value&limit=25&order=desc HTTP/1.1"));
    assert!(requests[0].to_ascii_lowercase().contains("authorization: bearer stale-token"));
    assert!(requests[1].to_ascii_lowercase().contains("authorization: bearer fresh-token"));
    assert_eq!(response.data.len(), 1);
    assert_eq!(response.data[0].client_id, "client-a");
    assert_eq!(response.data[0].last_seen_at, Some(1_788_696_000));
    assert_eq!(response.next_cursor.as_deref(), Some("next"));
}

#[tokio::test]
async fn client_list_rejects_invalid_limits_without_network_access() {
    let provider = RecoveringAuthProvider::new();
    let error = list_remote_control_clients(
        "https://chatgpt.com/backend-api",
        &provider,
        RemoteControlClientsListParams {
            environment_id: "env-a".to_string(),
            cursor: None,
            limit: Some(101),
            order: None,
        },
    )
    .await
    .expect_err("invalid limit must fail");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(provider.loads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn client_revoke_retries_independently_and_encodes_the_client_id() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        let mut requests = Vec::new();
        for status in ["403 Forbidden", "204 No Content"] {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            loop {
                let read = stream.read(&mut buffer).await.expect("read request");
                assert!(read > 0, "request ended before headers");
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            requests.push(String::from_utf8_lossy(&request).into_owned());
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .expect("write response");
        }
        requests
    });
    let provider = RecoveringAuthProvider::new();

    revoke_remote_control_client(
        &format!("http://{address}/backend-api"),
        &provider,
        RemoteControlClientsRevokeParams {
            environment_id: "env/a".to_string(),
            client_id: "client/a".to_string(),
        },
    )
    .await
    .expect("revoke client");
    let requests = fixture.await.expect("join fixture");

    assert_eq!(provider.recoveries.load(Ordering::SeqCst), 1);
    assert_eq!(provider.loads.load(Ordering::SeqCst), 2);
    assert!(requests[0].starts_with(
        "DELETE /backend-api/wham/remote/control/environments/env%2Fa/clients/client%2Fa HTTP/1.1"
    ));
    assert!(requests[0]
        .to_ascii_lowercase()
        .contains("authorization: bearer stale-token"));
    assert!(requests[1]
        .to_ascii_lowercase()
        .contains("authorization: bearer fresh-token"));
}

#[tokio::test]
async fn client_list_maps_timeout_and_redacts_invalid_response_details() {
    let timeout_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind timeout fixture");
    let timeout_address = timeout_listener.local_addr().expect("fixture address");
    let timeout_fixture = tokio::spawn(async move {
        let (_stream, _) = timeout_listener.accept().await.expect("accept request");
        tokio::time::sleep(Duration::from_millis(250)).await;
    });
    let provider = RecoveringAuthProvider::new();
    let error = list_remote_control_clients(
        &format!("http://{timeout_address}/backend-api"),
        &provider,
        RemoteControlClientsListParams {
            environment_id: "env-a".to_string(),
            cursor: None,
            limit: None,
            order: None,
        },
    )
    .await
    .expect_err("timeout must fail");
    timeout_fixture.await.expect("join timeout fixture");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);

    let invalid_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind invalid response fixture");
    let invalid_address = invalid_listener.local_addr().expect("fixture address");
    let invalid_fixture = tokio::spawn(async move {
        let (mut stream, _) = invalid_listener.accept().await.expect("accept request");
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).await.expect("read request");
        let body = br#"{"remote_control_token":"must-not-leak"}"#;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .expect("write headers");
        stream.write_all(body).await.expect("write body");
    });
    let error = list_remote_control_clients(
        &format!("http://{invalid_address}/backend-api"),
        &provider,
        RemoteControlClientsListParams {
            environment_id: "env-a".to_string(),
            cursor: None,
            limit: None,
            order: None,
        },
    )
    .await
    .expect_err("invalid response must fail");
    invalid_fixture.await.expect("join invalid fixture");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(!error.to_string().contains("must-not-leak"));
    assert!(error.to_string().contains("<redacted>"));
}
