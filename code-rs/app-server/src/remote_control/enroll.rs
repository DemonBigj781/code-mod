use super::protocol::RemoteControlTarget;
use reqwest::header::HeaderMap;
use std::fmt;
use time::OffsetDateTime;

const REMOTE_CONTROL_RESPONSE_BODY_MAX_BYTES: usize = 4096;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RemoteControlEnrollment {
    pub remote_control_target: RemoteControlTarget,
    pub account_id: String,
    pub environment_id: String,
    pub server_id: String,
    pub server_name: String,
    pub remote_control_token: Option<String>,
    pub expires_at: Option<OffsetDateTime>,
    pub next_refresh_at: Option<OffsetDateTime>,
}

impl fmt::Debug for RemoteControlEnrollment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteControlEnrollment")
            .field("remote_control_target", &self.remote_control_target)
            .field("account_id", &self.account_id)
            .field("environment_id", &self.environment_id)
            .field("server_id", &self.server_id)
            .field("server_name", &self.server_name)
            .field(
                "remote_control_token",
                &self.remote_control_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_at", &self.expires_at)
            .field("next_refresh_at", &self.next_refresh_at)
            .finish()
    }
}

pub(crate) fn preview_remote_control_response_body(body: &[u8]) -> String {
    let body = String::from_utf8_lossy(body);
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "<empty>".to_string();
    }
    let redacted = redact_remote_control_response_body(trimmed);
    if redacted.len() <= REMOTE_CONTROL_RESPONSE_BODY_MAX_BYTES {
        return redacted;
    }
    let mut cut = REMOTE_CONTROL_RESPONSE_BODY_MAX_BYTES;
    while !redacted.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", &redacted[..cut])
}

fn redact_remote_control_response_body(body: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.to_string();
    };
    redact_sensitive_fields(&mut value);
    value.to_string()
}

fn redact_sensitive_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "remote_control_token" | "pairing_code" | "manual_pairing_code"
                ) || key.to_ascii_lowercase().contains("token")
                {
                    *value = serde_json::Value::String("<redacted>".to_string());
                } else {
                    redact_sensitive_fields(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_sensitive_fields(value);
            }
        }
        _ => {}
    }
}

pub(crate) fn format_headers(headers: &HeaderMap) -> String {
    let request_id = headers
        .get("x-request-id")
        .or_else(|| headers.get("x-oai-request-id"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<none>");
    let cf_ray = headers
        .get("cf-ray")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<none>");
    format!("request-id: {request_id}, cf-ray: {cf_ray}")
}
