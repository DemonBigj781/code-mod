use std::io;
use std::io::ErrorKind;
use url::Host;
use url::Url;

const REMOTE_CONTROL_SERVER_PATH: &str = "wham/remote/control/server";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteControlTarget {
    pub websocket_url: String,
    pub enroll_url: String,
    pub refresh_url: String,
    pub pair_url: String,
    pub pair_status_url: String,
}

pub(crate) fn normalize_remote_control_url(
    remote_control_url: &str,
) -> io::Result<RemoteControlTarget> {
    let base_url = normalize_remote_control_base_url(remote_control_url)?;
    let map_parse_error = |error: url::ParseError| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid remote control URL `{remote_control_url}`: {error}"),
        )
    };

    let enroll_url = base_url
        .join(&format!("{REMOTE_CONTROL_SERVER_PATH}/enroll"))
        .map_err(map_parse_error)?;
    let refresh_url = base_url
        .join(&format!("{REMOTE_CONTROL_SERVER_PATH}/refresh"))
        .map_err(map_parse_error)?;
    let pair_url = base_url
        .join(&format!("{REMOTE_CONTROL_SERVER_PATH}/pair"))
        .map_err(map_parse_error)?;
    let pair_status_url = base_url
        .join(&format!("{REMOTE_CONTROL_SERVER_PATH}/pair/status"))
        .map_err(map_parse_error)?;
    let mut websocket_url = base_url
        .join(REMOTE_CONTROL_SERVER_PATH)
        .map_err(map_parse_error)?;
    websocket_url
        .set_scheme(if base_url.scheme() == "https" {
            "wss"
        } else {
            "ws"
        })
        .map_err(|()| invalid_remote_control_scheme(remote_control_url))?;

    Ok(RemoteControlTarget {
        websocket_url: websocket_url.to_string(),
        enroll_url: enroll_url.to_string(),
        refresh_url: refresh_url.to_string(),
        pair_url: pair_url.to_string(),
        pair_status_url: pair_status_url.to_string(),
    })
}

fn normalize_remote_control_base_url(remote_control_url: &str) -> io::Result<Url> {
    let mut url = Url::parse(remote_control_url).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid remote control URL `{remote_control_url}`: {error}"),
        )
    })?;
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err(invalid_remote_control_scheme(remote_control_url));
    }

    let host = url.host();
    match url.scheme() {
        "https" if is_localhost(&host) || is_allowed_chatgpt_host(&host) => {}
        "http" if is_localhost(&host) => {}
        _ => return Err(invalid_remote_control_scheme(remote_control_url)),
    }

    let mut path = url.path().trim_end_matches('/').to_string();
    for suffix in [
        "/wham/remote/control/server/pair/status",
        "/wham/remote/control/server/enroll",
        "/wham/remote/control/server/refresh",
        "/wham/remote/control/server/pair",
        "/wham/remote/control/server",
    ] {
        if let Some(base) = path.strip_suffix(suffix) {
            path = base.to_string();
            break;
        }
    }
    path.push('/');
    url.set_path(&path);
    Ok(url)
}

fn is_allowed_chatgpt_host(host: &Option<Host<&str>>) -> bool {
    let Some(Host::Domain(host)) = *host else {
        return false;
    };
    host == "chatgpt.com"
        || host == "chatgpt-staging.com"
        || host.ends_with(".chatgpt.com")
        || host.ends_with(".chatgpt-staging.com")
}

fn is_localhost(host: &Option<Host<&str>>) -> bool {
    match host {
        Some(Host::Domain("localhost")) => true,
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

fn invalid_remote_control_scheme(remote_control_url: &str) -> io::Error {
    io::Error::new(
        ErrorKind::InvalidInput,
        format!(
            "invalid remote control URL `{remote_control_url}`; expected HTTPS URL for chatgpt.com or chatgpt-staging.com, or HTTP/HTTPS URL for localhost"
        ),
    )
}
