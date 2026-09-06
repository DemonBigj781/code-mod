use super::enroll::preview_remote_control_response_body;

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
