use super::protocol::RemoteControlTarget;
use super::protocol::normalize_remote_control_url;
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
