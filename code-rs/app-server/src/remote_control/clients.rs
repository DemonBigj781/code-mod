use super::auth::RemoteControlAuth;
use super::auth::RemoteControlAuthProvider;
use super::enroll::format_headers;
use super::enroll::preview_remote_control_response_body;
use super::protocol::normalize_remote_control_base_url;
use code_app_server_protocol::RemoteControlClient;
use code_app_server_protocol::RemoteControlClientsListOrder;
use code_app_server_protocol::RemoteControlClientsListParams;
use code_app_server_protocol::RemoteControlClientsListResponse;
use code_app_server_protocol::RemoteControlClientsRevokeParams;
use code_app_server_protocol::RemoteControlClientsRevokeResponse;
use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use std::io;
use std::io::ErrorKind;
use std::time::Duration;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use url::Url;

#[cfg(not(test))]
const CLIENT_MANAGEMENT_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(test)]
const CLIENT_MANAGEMENT_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Debug, Deserialize)]
struct ListClientsResponse {
    items: Vec<ClientResponse>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClientResponse {
    client_id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    device_type: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    os_version: Option<String>,
    #[serde(default)]
    device_model: Option<String>,
    #[serde(default)]
    app_version: Option<String>,
    #[serde(default)]
    last_seen_at: Option<String>,
}

enum ClientRequest<'a> {
    List {
        url: &'a Url,
        params: &'a RemoteControlClientsListParams,
    },
    Revoke {
        url: &'a Url,
    },
}

struct ClientResponseBytes {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

pub(crate) async fn list_remote_control_clients(
    remote_control_url: &str,
    auth_provider: &(impl RemoteControlAuthProvider + ?Sized),
    params: RemoteControlClientsListParams,
) -> io::Result<RemoteControlClientsListResponse> {
    if params.environment_id.is_empty() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "remote control client list requires environmentId",
        ));
    }
    if params
        .limit
        .is_some_and(|limit| !(1..=100).contains(&limit))
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "remote control client list limit must be between 1 and 100",
        ));
    }
    let url = environment_clients_url(remote_control_url, &params.environment_id)?;
    let request = ClientRequest::List {
        url: &url,
        params: &params,
    };
    let response = send_with_recovery(auth_provider, &request, "list remote control clients").await?;
    ensure_success(&response, &url, "client list")?;
    let body_preview = preview_remote_control_response_body(&response.body);
    let response_body: ListClientsResponse = serde_json::from_slice(&response.body).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("failed to parse remote control client list response: body: {body_preview}, decode error: {error}"),
        )
    })?;
    Ok(RemoteControlClientsListResponse {
        data: response_body
            .items
            .into_iter()
            .map(RemoteControlClient::try_from)
            .collect::<io::Result<Vec<_>>>()?,
        next_cursor: response_body.cursor,
    })
}

pub(crate) async fn revoke_remote_control_client(
    remote_control_url: &str,
    auth_provider: &(impl RemoteControlAuthProvider + ?Sized),
    params: RemoteControlClientsRevokeParams,
) -> io::Result<RemoteControlClientsRevokeResponse> {
    if params.environment_id.is_empty() || params.client_id.is_empty() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "remote control client revoke requires environmentId and clientId",
        ));
    }
    let mut url = environment_clients_url(remote_control_url, &params.environment_id)?;
    url.path_segments_mut()
        .map_err(|()| io::Error::new(ErrorKind::InvalidInput, "remote control URL cannot be a base"))?
        .push(&params.client_id);
    let response = send_with_recovery(
        auth_provider,
        &ClientRequest::Revoke { url: &url },
        "revoke remote control client",
    )
    .await?;
    ensure_success(&response, &url, "client revoke")?;
    Ok(RemoteControlClientsRevokeResponse {})
}

async fn send_with_recovery(
    auth_provider: &(impl RemoteControlAuthProvider + ?Sized),
    request: &ClientRequest<'_>,
    action: &str,
) -> io::Result<ClientResponseBytes> {
    let auth = auth_provider.load().await?;
    let response = send_once(&auth, request, action).await?;
    if !matches!(response.status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
        || !auth_provider.recover_unauthorized().await?
    {
        return Ok(response);
    }
    let auth = auth_provider.load().await?;
    send_once(&auth, request, action).await
}

async fn send_once(
    auth: &RemoteControlAuth,
    request: &ClientRequest<'_>,
    action: &str,
) -> io::Result<ClientResponseBytes> {
    let client = reqwest::Client::new();
    let builder = match request {
        ClientRequest::List { url, params } => {
            let mut query = Vec::new();
            if let Some(cursor) = &params.cursor {
                query.push(("cursor", cursor.clone()));
            }
            if let Some(limit) = params.limit {
                query.push(("limit", limit.to_string()));
            }
            if let Some(order) = params.order {
                query.push((
                    "order",
                    match order {
                        RemoteControlClientsListOrder::Asc => "asc",
                        RemoteControlClientsListOrder::Desc => "desc",
                    }
                    .to_string(),
                ));
            }
            client.get((*url).clone()).query(&query)
        }
        ClientRequest::Revoke { url } => client.delete((*url).clone()),
    };
    let response = builder
        .timeout(CLIENT_MANAGEMENT_TIMEOUT)
        .headers(auth.request_headers()?)
        .send()
        .await
        .map_err(|error| {
            io::Error::new(
                if error.is_timeout() {
                    ErrorKind::TimedOut
                } else {
                    ErrorKind::Other
                },
                format!("failed to {action}: {error}"),
            )
        })?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .bytes()
        .await
        .map_err(|error| io::Error::other(format!("failed to read {action} response: {error}")))?
        .to_vec();
    Ok(ClientResponseBytes {
        status,
        headers,
        body,
    })
}

fn ensure_success(
    response: &ClientResponseBytes,
    url: &Url,
    response_kind: &str,
) -> io::Result<()> {
    if response.status.is_success() {
        return Ok(());
    }
    let kind = match response.status.as_u16() {
        400 => ErrorKind::InvalidInput,
        401 | 403 => ErrorKind::PermissionDenied,
        404 => ErrorKind::NotFound,
        _ => ErrorKind::Other,
    };
    Err(io::Error::new(
        kind,
        format!(
            "remote control {response_kind} failed at `{url}`: HTTP {}, {}, body: {}",
            response.status,
            format_headers(&response.headers),
            preview_remote_control_response_body(&response.body)
        ),
    ))
}

fn environment_clients_url(remote_control_url: &str, environment_id: &str) -> io::Result<Url> {
    let mut url = normalize_remote_control_base_url(remote_control_url)?
        .join("wham/remote/control/environments")
        .map_err(io::Error::other)?;
    url.path_segments_mut()
        .map_err(|()| io::Error::new(ErrorKind::InvalidInput, "remote control URL cannot be a base"))?
        .push(environment_id)
        .push("clients");
    Ok(url)
}

impl TryFrom<ClientResponse> for RemoteControlClient {
    type Error = io::Error;

    fn try_from(client: ClientResponse) -> Result<Self, Self::Error> {
        Ok(Self {
            client_id: client.client_id,
            display_name: client.display_name,
            device_type: client.device_type,
            platform: client.platform,
            os_version: client.os_version,
            device_model: client.device_model,
            app_version: client.app_version,
            last_seen_at: client
                .last_seen_at
                .map(|value| {
                    OffsetDateTime::parse(&value, &Rfc3339)
                        .map(OffsetDateTime::unix_timestamp)
                        .map_err(|error| {
                            io::Error::new(
                                ErrorKind::InvalidData,
                                format!("invalid remote control client last_seen_at: {error}"),
                            )
                        })
                })
                .transpose()?,
        })
    }
}
