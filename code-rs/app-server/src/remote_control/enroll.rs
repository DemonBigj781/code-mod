use super::auth::RemoteControlAuth;
use super::auth::RemoteControlAuthProvider;
use super::host_device::HostDevice;
use super::protocol::RemoteControlPairingStatusRequest;
use super::protocol::RemoteControlPairingStatusResponse as BackendRemoteControlPairingStatusResponse;
use super::protocol::RemoteControlTarget;
use super::protocol::StartRemoteControlPairingRequest;
use super::protocol::StartRemoteControlPairingResponse;
use super::state::RemoteControlEnrollmentRecord;
use super::state::RemoteControlState;
use super::server_api::enroll_remote_control_server;
use super::server_api::refresh_remote_control_server;
use code_app_server_protocol::RemoteControlPairingStartParams;
use code_app_server_protocol::RemoteControlPairingStartResponse;
use code_app_server_protocol::RemoteControlPairingStatusParams;
use code_app_server_protocol::RemoteControlPairingStatusResponse;
use reqwest::header::HeaderMap;
use std::fmt;
use std::io;
use std::io::ErrorKind;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const REMOTE_CONTROL_PAIRING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const REMOTE_CONTROL_RESPONSE_BODY_MAX_BYTES: usize = 4096;
const REMOTE_CONTROL_SERVER_TOKEN_REFRESH_SKEW_SECS: i64 = 5 * 60;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteControlEnrollmentSelection {
    ReuseOrCreate,
    ReplaceExisting,
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

impl RemoteControlEnrollment {
    pub(crate) async fn start_pairing(
        &self,
        params: RemoteControlPairingStartParams,
    ) -> io::Result<RemoteControlPairingStartResponse> {
        let remote_control_token = self.usable_server_token()?;
        let request = StartRemoteControlPairingRequest {
            manual_code: params.manual_code,
        };
        let response = reqwest::Client::new()
            .post(&self.remote_control_target.pair_url)
            .timeout(REMOTE_CONTROL_PAIRING_TIMEOUT)
            .bearer_auth(remote_control_token)
            .json(&request)
            .send()
            .await
            .map_err(|error| remote_control_request_error("start pairing", error))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.map_err(|error| {
            io::Error::other(format!(
                "failed to read remote control pairing response from `{}`: {error}",
                self.remote_control_target.pair_url
            ))
        })?;
        let preview = preview_remote_control_response_body(&body);
        if !status.is_success() {
            return Err(io::Error::new(
                match status.as_u16() {
                    401 | 403 => ErrorKind::PermissionDenied,
                    404 => ErrorKind::NotFound,
                    _ => ErrorKind::Other,
                },
                format!(
                    "remote control pairing failed at `{}`: HTTP {status}, {}, body: {preview}",
                    self.remote_control_target.pair_url,
                    format_headers(&headers)
                ),
            ));
        }
        let pairing: StartRemoteControlPairingResponse = serde_json::from_slice(&body).map_err(
            |error| {
                io::Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "failed to parse remote control pairing response from `{}`: HTTP {status}, {}, body: {preview}, decode error: {error}",
                        self.remote_control_target.pair_url,
                        format_headers(&headers)
                    ),
                )
            },
        )?;
        if pairing.server_id != self.server_id
            || pairing.environment_id != self.environment_id
        {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "remote control pairing returned mismatched enrollment: expected server_id={}, environment_id={}; got server_id={}, environment_id={}",
                    self.server_id,
                    self.environment_id,
                    pairing.server_id,
                    pairing.environment_id
                ),
            ));
        }
        let expires_at = OffsetDateTime::parse(&pairing.expires_at, &Rfc3339)
            .map_err(|error| {
                io::Error::new(
                    ErrorKind::InvalidData,
                    format!("invalid remote control pairing expiry: {error}"),
                )
            })?
            .unix_timestamp();
        Ok(RemoteControlPairingStartResponse {
            pairing_code: pairing.pairing_code,
            manual_pairing_code: pairing.manual_pairing_code,
            environment_id: pairing.environment_id,
            expires_at,
        })
    }

    pub(crate) async fn pairing_status(
        &self,
        params: RemoteControlPairingStatusParams,
    ) -> io::Result<RemoteControlPairingStatusResponse> {
        if params.pairing_code.is_some() == params.manual_pairing_code.is_some() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "exactly one remote control pairing code must be provided",
            ));
        }
        let remote_control_token = self.usable_server_token()?;
        let request = RemoteControlPairingStatusRequest {
            pairing_code: params.pairing_code,
            manual_pairing_code: params.manual_pairing_code,
        };
        let response = reqwest::Client::new()
            .post(&self.remote_control_target.pair_status_url)
            .timeout(REMOTE_CONTROL_PAIRING_TIMEOUT)
            .bearer_auth(remote_control_token)
            .json(&request)
            .send()
            .await
            .map_err(|error| remote_control_request_error("check pairing status", error))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.map_err(|error| {
            io::Error::other(format!(
                "failed to read remote control pairing status response from `{}`: {error}",
                self.remote_control_target.pair_status_url
            ))
        })?;
        let preview = preview_remote_control_response_body(&body);
        if !status.is_success() {
            return Err(io::Error::new(
                match status.as_u16() {
                    401 | 403 => ErrorKind::PermissionDenied,
                    404 | 410 => ErrorKind::InvalidInput,
                    _ => ErrorKind::Other,
                },
                format!(
                    "remote control pairing status failed at `{}`: HTTP {status}, {}, body: {preview}",
                    self.remote_control_target.pair_status_url,
                    format_headers(&headers)
                ),
            ));
        }
        let response: BackendRemoteControlPairingStatusResponse =
            serde_json::from_slice(&body).map_err(|error| {
                io::Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "failed to parse remote control pairing status response from `{}`: HTTP {status}, {}, body: {preview}, decode error: {error}",
                        self.remote_control_target.pair_status_url,
                        format_headers(&headers)
                    ),
                )
            })?;
        Ok(RemoteControlPairingStatusResponse {
            claimed: response.claimed,
        })
    }

    fn usable_server_token(&self) -> io::Result<&str> {
        let token = self
            .remote_control_token
            .as_deref()
            .ok_or_else(pairing_unavailable_error)?;
        if self.expires_at.is_none_or(|expires_at| expires_at <= OffsetDateTime::now_utc()) {
            return Err(pairing_unavailable_error());
        }
        Ok(token)
    }

    pub(crate) fn should_refresh_server_token(&self) -> bool {
        let now = OffsetDateTime::now_utc();
        let Some(expires_at) = self.remote_control_token.as_ref().and(self.expires_at) else {
            return true;
        };
        if expires_at <= now {
            return true;
        }
        expires_at <= now + time::Duration::seconds(REMOTE_CONTROL_SERVER_TOKEN_REFRESH_SKEW_SECS)
            && self
                .next_refresh_at
                .is_none_or(|next_refresh_at| next_refresh_at <= now)
    }
}

fn pairing_unavailable_error() -> io::Error {
    io::Error::new(
        ErrorKind::NotConnected,
        "remote control pairing is unavailable until the server token is refreshed",
    )
}

fn remote_control_request_error(action: &str, error: reqwest::Error) -> io::Error {
    io::Error::new(
        if error.is_timeout() {
            ErrorKind::TimedOut
        } else {
            ErrorKind::Other
        },
        format!("failed to {action} for remote control: {error}"),
    )
}

pub(crate) async fn resolve_remote_control_enrollment(
    state: &RemoteControlState,
    target: &RemoteControlTarget,
    auth_provider: &(impl RemoteControlAuthProvider + ?Sized),
    installation_id: &str,
    host: &HostDevice,
    app_server_client_name: Option<&str>,
    current_enrollment: Option<&RemoteControlEnrollment>,
    remote_control_enabled: Option<bool>,
    selection: RemoteControlEnrollmentSelection,
) -> io::Result<RemoteControlEnrollment> {
    let mut recovered_unauthorized = false;
    loop {
        let auth = auth_provider.load().await?;
        match resolve_remote_control_enrollment_with_auth(
            state,
            target,
            &auth,
            installation_id,
            host,
            app_server_client_name,
            current_enrollment,
            remote_control_enabled,
            selection,
        )
        .await
        {
            Err(error)
                if error.kind() == ErrorKind::PermissionDenied && !recovered_unauthorized =>
            {
                if !auth_provider.recover_unauthorized().await? {
                    return Err(error);
                }
                recovered_unauthorized = true;
            }
            result => return result,
        }
    }
}

async fn resolve_remote_control_enrollment_with_auth(
    state: &RemoteControlState,
    target: &RemoteControlTarget,
    auth: &RemoteControlAuth,
    installation_id: &str,
    host: &HostDevice,
    app_server_client_name: Option<&str>,
    current_enrollment: Option<&RemoteControlEnrollment>,
    remote_control_enabled: Option<bool>,
    selection: RemoteControlEnrollmentSelection,
) -> io::Result<RemoteControlEnrollment> {
    let mut enrollment = match selection {
        RemoteControlEnrollmentSelection::ReuseOrCreate => {
            if let Some(enrollment) = current_enrollment
                .filter(|enrollment| enrollment.account_id == auth.account_id())
                .cloned()
            {
                Some(enrollment)
            } else {
                load_persisted_remote_control_enrollment(
                    state,
                    target,
                    auth.account_id(),
                    app_server_client_name,
                )
                .await?
            }
        }
        RemoteControlEnrollmentSelection::ReplaceExisting => None,
    };

    if enrollment.as_ref().is_some_and(|enrollment| {
        enrollment.server_id.is_empty() || enrollment.environment_id.is_empty()
    }) {
        enrollment = None;
    }
    if let Some(enrollment) = enrollment.as_mut() {
        enrollment.remote_control_target = target.clone();
        enrollment.server_name.clone_from(&host.name);
    }

    let enrollment = match enrollment {
        Some(enrollment) if !enrollment.should_refresh_server_token() => enrollment,
        Some(enrollment) => {
            match refresh_remote_control_server(&enrollment, auth, installation_id).await {
                Ok(enrollment) => enrollment,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    enroll_remote_control_server(target, auth, installation_id, host).await?
                }
                Err(error) => return Err(error),
            }
        }
        None => enroll_remote_control_server(target, auth, installation_id, host).await?,
    };

    update_persisted_remote_control_enrollment(
        state,
        target,
        auth.account_id(),
        app_server_client_name,
        Some(&enrollment),
        remote_control_enabled,
    )
    .await?;
    Ok(enrollment)
}

pub(crate) async fn load_persisted_remote_control_enrollment(
    state: &RemoteControlState,
    target: &RemoteControlTarget,
    account_id: &str,
    app_server_client_name: Option<&str>,
) -> io::Result<Option<RemoteControlEnrollment>> {
    state
        .get_enrollment(
            &target.websocket_url,
            account_id,
            app_server_client_name,
        )
        .await
        .map_err(io::Error::other)
        .map(|record| {
            record.map(|record| RemoteControlEnrollment {
                remote_control_target: target.clone(),
                account_id: record.account_id,
                environment_id: record.environment_id,
                server_id: record.server_id,
                server_name: record.server_name,
                remote_control_token: None,
                expires_at: None,
                next_refresh_at: None,
            })
        })
}

pub(crate) async fn update_persisted_remote_control_enrollment(
    state: &RemoteControlState,
    target: &RemoteControlTarget,
    account_id: &str,
    app_server_client_name: Option<&str>,
    enrollment: Option<&RemoteControlEnrollment>,
    remote_control_enabled: Option<bool>,
) -> io::Result<()> {
    if let Some(enrollment) = enrollment {
        if enrollment.account_id != account_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "remote control enrollment account does not match the active account",
            ));
        }
        state
            .upsert_enrollment(&RemoteControlEnrollmentRecord {
                websocket_url: target.websocket_url.clone(),
                account_id: account_id.to_string(),
                app_server_client_name: app_server_client_name.map(str::to_string),
                server_id: enrollment.server_id.clone(),
                environment_id: enrollment.environment_id.clone(),
                server_name: enrollment.server_name.clone(),
                remote_control_enabled,
            })
            .await
            .map_err(io::Error::other)
    } else {
        state
            .delete_enrollment(
                &target.websocket_url,
                account_id,
                app_server_client_name,
            )
            .await
            .map(|_| ())
            .map_err(io::Error::other)
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
