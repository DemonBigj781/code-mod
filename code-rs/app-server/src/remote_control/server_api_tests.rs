use super::auth::RemoteControlAuth;
use super::host_device::HostDevice;
use super::protocol::normalize_remote_control_url;
use super::server_api::enroll_remote_control_server;
use serde_json::Value;
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
