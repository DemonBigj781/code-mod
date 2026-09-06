use super::auth::RemoteControlAuth;
use super::enroll::RemoteControlEnrollment;
use super::enroll::format_headers;
use super::enroll::preview_remote_control_response_body;
use super::host_device::HostDevice;
use super::protocol::EnrollRemoteServerRequest;
use super::protocol::EnrollRemoteServerResponse;
use super::protocol::RefreshRemoteServerRequest;
use super::protocol::RemoteControlTarget;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io;
use std::io::ErrorKind;
use std::time::Duration;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const REMOTE_CONTROL_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const REMOTE_CONTROL_INSTALLATION_ID_HEADER: &str = "x-codex-installation-id";
pub(crate) const REMOTE_CONTROL_HOST_DEVICE_KIND_HEADER: &str = "x-codex-host-device-kind";

pub(crate) async fn enroll_remote_control_server(
    target: &RemoteControlTarget,
    auth: &RemoteControlAuth,
    installation_id: &str,
    host: &HostDevice,
) -> io::Result<RemoteControlEnrollment> {
    let request = EnrollRemoteServerRequest {
        name: &host.name,
        os: &host.os,
        arch: &host.arch,
        app_server_version: env!("CARGO_PKG_VERSION"),
        installation_id,
    };
    let response: EnrollRemoteServerResponse = send_request(
        &target.enroll_url,
        auth,
        installation_id,
        host.device_kind.as_deref(),
        &request,
        "server enrollment",
    )
    .await?;
    enrollment_from_response(target, auth, &host.name, response)
}

pub(crate) async fn refresh_remote_control_server(
    enrollment: &RemoteControlEnrollment,
    auth: &RemoteControlAuth,
    installation_id: &str,
) -> io::Result<RemoteControlEnrollment> {
    let request = RefreshRemoteServerRequest {
        server_id: &enrollment.server_id,
        installation_id,
    };
    let response: EnrollRemoteServerResponse = send_request(
        &enrollment.remote_control_target.refresh_url,
        auth,
        installation_id,
        None,
        &request,
        "server refresh",
    )
    .await?;
    if response.server_id != enrollment.server_id
        || response.environment_id != enrollment.environment_id
    {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "remote control refresh returned a mismatched enrollment",
        ));
    }
    enrollment_from_response(
        &enrollment.remote_control_target,
        auth,
        &enrollment.server_name,
        response,
    )
}

async fn send_request<Request, Response>(
    url: &str,
    auth: &RemoteControlAuth,
    installation_id: &str,
    host_device_kind: Option<&str>,
    request: &Request,
    response_kind: &str,
) -> io::Result<Response>
where
    Request: Serialize,
    Response: DeserializeOwned,
{
    let mut request_builder = reqwest::Client::new()
        .post(url)
        .timeout(REMOTE_CONTROL_REQUEST_TIMEOUT)
        .headers(auth.request_headers()?)
        .header(REMOTE_CONTROL_INSTALLATION_ID_HEADER, installation_id)
        .json(request);
    if let Some(host_device_kind) = host_device_kind {
        request_builder = request_builder.header(
            REMOTE_CONTROL_HOST_DEVICE_KIND_HEADER,
            host_device_kind,
        );
    }
    let response = request_builder.send().await.map_err(|error| {
        io::Error::new(
            if error.is_timeout() {
                ErrorKind::TimedOut
            } else {
                ErrorKind::Other
            },
            format!("failed to send remote control {response_kind} request: {error}"),
        )
    })?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(io::Error::other)?;
    let preview = preview_remote_control_response_body(&body);
    if !status.is_success() {
        let kind = match status.as_u16() {
            400 => ErrorKind::InvalidInput,
            401 | 403 => ErrorKind::PermissionDenied,
            404 => ErrorKind::NotFound,
            _ => ErrorKind::Other,
        };
        return Err(io::Error::new(
            kind,
            format!(
                "remote control {response_kind} failed at `{url}`: HTTP {status}, {}, body: {preview}",
                format_headers(&headers)
            ),
        ));
    }
    serde_json::from_slice(&body).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "failed to parse remote control {response_kind} response from `{url}`: body: {preview}, decode error: {error}"
            ),
        )
    })
}

fn enrollment_from_response(
    target: &RemoteControlTarget,
    auth: &RemoteControlAuth,
    server_name: &str,
    response: EnrollRemoteServerResponse,
) -> io::Result<RemoteControlEnrollment> {
    let expires_at = OffsetDateTime::parse(&response.expires_at, &Rfc3339).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("invalid remote control token expiry: {error}"),
        )
    })?;
    Ok(RemoteControlEnrollment {
        remote_control_target: target.clone(),
        account_id: auth.account_id().to_string(),
        environment_id: response.environment_id,
        server_id: response.server_id,
        server_name: server_name.to_string(),
        remote_control_token: Some(response.remote_control_token),
        expires_at: Some(expires_at),
        next_refresh_at: None,
    })
}
