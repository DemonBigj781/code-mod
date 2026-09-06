use super::auth::RemoteControlAuth;
use super::enroll::RemoteControlEnrollment;
use super::host_device::HostDevice;
use super::protocol::normalize_remote_control_url;
use super::server_api::enroll_remote_control_server;
use super::server_api::refresh_remote_control_server;
use serde_json::Value;
use std::io;
use std::time::Duration;
use time::OffsetDateTime;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

#[tokio::test]
async fn enrollment_posts_exact_headers_and_body() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local fixture");
    let address = listener.local_addr().expect("read fixture address");
    let fixture = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept request");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 2048];
        loop {
            let read = stream.read(&mut buffer).await.expect("read request");
            assert!(read > 0, "request ended before headers");
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let header_end = header_end + 4;
            let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .expect("content length header");
            while request.len() < header_end + content_length {
                let read = stream.read(&mut buffer).await.expect("read body");
                assert!(read > 0, "request ended before body");
                request.extend_from_slice(&buffer[..read]);
            }
            let body: Value = serde_json::from_slice(
                &request[header_end..header_end + content_length],
            )
            .expect("parse request body");
            let response_body = br#"{"server_id":"server-a","environment_id":"env-a","remote_control_token":"server-token","expires_at":"2030-01-01T00:00:00Z"}"#;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        response_body.len()
                    )
                    .as_bytes(),
                )
                .await
                .expect("write response headers");
            stream
                .write_all(response_body)
                .await
                .expect("write response body");
            return (headers, body);
        }
    });

    let target = normalize_remote_control_url(&format!("http://{address}/backend-api"))
        .expect("normalize fixture URL");
    let auth = RemoteControlAuth::for_testing("account-token", "account-a");
    let host = HostDevice::for_testing("deck", "linux", "x86_64", Some("desktop"));
    let enrollment = enroll_remote_control_server(&target, &auth, "installation-a", &host)
        .await
        .expect("enroll server");
    let (headers, body) = fixture.await.expect("join fixture");

    assert!(headers.starts_with("POST /backend-api/wham/remote/control/server/enroll HTTP/1.1"));
    assert!(headers.to_ascii_lowercase().contains("authorization: bearer account-token"));
    assert!(headers.to_ascii_lowercase().contains("chatgpt-account-id: account-a"));
    assert!(headers.to_ascii_lowercase().contains("x-codex-installation-id: installation-a"));
    assert!(headers.to_ascii_lowercase().contains("x-codex-host-device-kind: desktop"));
    assert_eq!(body["name"], "deck");
    assert_eq!(body["os"], "linux");
    assert_eq!(body["arch"], "x86_64");
    assert_eq!(body["installation_id"], "installation-a");
    assert_eq!(enrollment.server_id, "server-a");
    assert_eq!(enrollment.environment_id, "env-a");
    assert_eq!(enrollment.remote_control_token.as_deref(), Some("server-token"));
}

#[tokio::test]
async fn refresh_maps_stale_enrollment_and_timeout_errors() {
    let stale_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stale fixture");
    let stale_address = stale_listener.local_addr().expect("fixture address");
    let stale_fixture = tokio::spawn(async move {
        let (mut stream, _) = stale_listener.accept().await.expect("accept request");
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).await.expect("read request");
        let body = br#"{"remote_control_token":"must-not-leak"}"#;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .expect("write headers");
        stream.write_all(body).await.expect("write body");
    });
    let target = normalize_remote_control_url(&format!("http://{stale_address}/backend-api"))
        .expect("normalize target");
    let auth = RemoteControlAuth::for_testing("account-token", "account-a");
    let enrollment = RemoteControlEnrollment {
        remote_control_target: target,
        account_id: "account-a".to_string(),
        environment_id: "env-a".to_string(),
        server_id: "server-a".to_string(),
        server_name: "deck".to_string(),
        remote_control_token: None,
        expires_at: None,
        next_refresh_at: None,
    };
    let error = refresh_remote_control_server(&enrollment, &auth, "installation-a")
        .await
        .expect_err("404 refresh must fail");
    stale_fixture.await.expect("join stale fixture");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(!error.to_string().contains("must-not-leak"));

    let timeout_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind timeout fixture");
    let timeout_address = timeout_listener.local_addr().expect("fixture address");
    let timeout_fixture = tokio::spawn(async move {
        let (_stream, _) = timeout_listener.accept().await.expect("accept request");
        tokio::time::sleep(Duration::from_millis(250)).await;
    });
    let target = normalize_remote_control_url(&format!("http://{timeout_address}/backend-api"))
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
    let error = refresh_remote_control_server(&enrollment, &auth, "installation-a")
        .await
        .expect_err("timeout refresh must fail");
    timeout_fixture.await.expect("join timeout fixture");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
}
